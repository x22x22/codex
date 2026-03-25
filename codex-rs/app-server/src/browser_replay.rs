use crate::atlas_command::is_atlas_command_dynamic_tool;
use crate::remote_browser_tools::is_remote_browser_dynamic_tool;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_app_server_protocol::BrowserReplayFrame;
use codex_app_server_protocol::BrowserReplayRenderMode;
use codex_app_server_protocol::BrowserReplayTextSnapshotFormat;
use codex_app_server_protocol::BrowserSessionState;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::Turn;
use image::Delay;
use image::Frame;
use image::Rgba;
use image::RgbaImage;
use image::codecs::gif::GifEncoder;
use image::codecs::gif::Repeat;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::io::Cursor;

const REPLAY_FRAME_DURATION_MS: u32 = 700;
const MAX_TEXT_SNAPSHOT_CHARS: usize = 2_000;

pub(crate) fn augment_turns_with_browser_replays(turns: &mut [Turn]) {
    for turn in turns {
        turn.items
            .retain(|item| !matches!(item, ThreadItem::BrowserReplay { .. }));

        if let Some(replay_item) = build_browser_replay_item(&turn.id, &turn.items) {
            turn.items.push(replay_item);
        }
    }
}

fn build_browser_replay_item(turn_id: &str, items: &[ThreadItem]) -> Option<ThreadItem> {
    let mut frames = Vec::new();
    let mut static_image_urls = Vec::new();
    let mut fallback_animation_url = None;

    for item in items {
        let ThreadItem::DynamicToolCall {
            id,
            tool,
            status,
            content_items: Some(content_items),
            ..
        } = item
        else {
            continue;
        };

        if !is_browser_replay_dynamic_tool(tool) {
            continue;
        }

        let frame_content = inspect_content_items(content_items);
        if let Some(image_url) = frame_content.image_url.clone() {
            static_image_urls.push(image_url);
        }
        if fallback_animation_url.is_none() {
            fallback_animation_url = frame_content.animation_image_url.clone();
        }

        if frame_content.image_url.is_none()
            && frame_content.text_snapshot.is_none()
            && frame_content.browser_state.is_none()
        {
            continue;
        }

        frames.push(BrowserReplayFrame {
            sequence_number: (frames.len() + 1) as u32,
            source_item_id: id.clone(),
            tool: tool.clone(),
            status: status.clone(),
            image_url: frame_content.image_url,
            text_snapshot: frame_content.text_snapshot,
            text_snapshot_format: frame_content.text_snapshot_format,
            browser_state: frame_content.browser_state,
        });
    }

    if frames.is_empty() {
        return None;
    }

    let animation_image_url =
        synthesize_animation_image_url(&static_image_urls).or(fallback_animation_url);
    let render_mode = if animation_image_url.is_some() {
        BrowserReplayRenderMode::Animation
    } else if frames.iter().any(|frame| frame.image_url.is_some()) {
        BrowserReplayRenderMode::Frames
    } else {
        BrowserReplayRenderMode::TextOnly
    };
    let frame_duration_ms =
        (animation_image_url.is_some() || frames.len() > 1).then_some(REPLAY_FRAME_DURATION_MS);

    Some(ThreadItem::BrowserReplay {
        id: format!("browser-replay-{turn_id}"),
        frames,
        render_mode,
        animation_image_url,
        frame_duration_ms,
    })
}

fn is_browser_replay_dynamic_tool(name: &str) -> bool {
    is_remote_browser_dynamic_tool(name) || is_atlas_command_dynamic_tool(name)
}

#[derive(Default)]
struct FrameContent {
    image_url: Option<String>,
    animation_image_url: Option<String>,
    text_snapshot: Option<String>,
    text_snapshot_format: Option<BrowserReplayTextSnapshotFormat>,
    browser_state: Option<BrowserSessionState>,
}

#[derive(Debug, Deserialize)]
struct RemoteBrowserToolTextPayload {
    #[serde(default)]
    result: JsonValue,
    #[serde(default)]
    browser_state: Option<BrowserSessionState>,
}

#[derive(Debug, Deserialize)]
struct AtlasCommandTextPayload {
    #[serde(default)]
    command_output: JsonValue,
    #[serde(default)]
    browser_state: Option<BrowserSessionState>,
}

fn inspect_content_items(items: &[DynamicToolCallOutputContentItem]) -> FrameContent {
    let mut content = FrameContent::default();

    for item in items {
        match item {
            DynamicToolCallOutputContentItem::InputText { text } => {
                if content.text_snapshot.is_none() {
                    let snapshot = build_text_snapshot(text);
                    content.text_snapshot = snapshot.text;
                    content.text_snapshot_format = snapshot.format;
                }
                if content.browser_state.is_none() {
                    content.browser_state = parse_browser_state(text);
                }
            }
            DynamicToolCallOutputContentItem::InputImage { image_url } => {
                if is_gif_data_url(image_url) {
                    if content.animation_image_url.is_none() {
                        content.animation_image_url = Some(image_url.clone());
                    }
                } else if is_raster_data_url(image_url) && content.image_url.is_none() {
                    content.image_url = Some(image_url.clone());
                }
            }
        }
    }

    content
}

#[derive(Default)]
struct TextSnapshot {
    text: Option<String>,
    format: Option<BrowserReplayTextSnapshotFormat>,
}

fn build_text_snapshot(text: &str) -> TextSnapshot {
    serde_json::from_str::<RemoteBrowserToolTextPayload>(text)
        .ok()
        .and_then(|payload| render_result_snapshot(&payload.result))
        .or_else(|| {
            serde_json::from_str::<AtlasCommandTextPayload>(text)
                .ok()
                .and_then(|payload| render_result_snapshot(&payload.command_output))
        })
        .or_else(|| {
            truncate_snapshot(text).map(|value| TextSnapshot {
                text: Some(value),
                format: Some(BrowserReplayTextSnapshotFormat::Plain),
            })
        })
        .unwrap_or_default()
}

fn parse_browser_state(text: &str) -> Option<BrowserSessionState> {
    serde_json::from_str::<RemoteBrowserToolTextPayload>(text)
        .ok()
        .and_then(|payload| payload.browser_state)
        .or_else(|| {
            serde_json::from_str::<AtlasCommandTextPayload>(text)
                .ok()
                .and_then(|payload| payload.browser_state)
        })
}

fn render_result_snapshot(result: &JsonValue) -> Option<TextSnapshot> {
    if result.is_null() {
        return None;
    }
    match result {
        JsonValue::String(text) => truncate_snapshot(text).map(|value| TextSnapshot {
            text: Some(value),
            format: Some(BrowserReplayTextSnapshotFormat::Plain),
        }),
        _ => serde_json::to_string_pretty(result)
            .ok()
            .and_then(|text| truncate_snapshot(&text))
            .map(|value| TextSnapshot {
                text: Some(value),
                format: Some(BrowserReplayTextSnapshotFormat::Json),
            }),
    }
}

fn truncate_snapshot(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let char_count = trimmed.chars().count();
    if char_count <= MAX_TEXT_SNAPSHOT_CHARS {
        return Some(trimmed.to_string());
    }
    let truncated = trimmed
        .chars()
        .take(MAX_TEXT_SNAPSHOT_CHARS)
        .collect::<String>();
    Some(format!("{truncated}..."))
}

fn synthesize_animation_image_url(image_urls: &[String]) -> Option<String> {
    if image_urls.len() < 2 {
        return None;
    }

    let mut decoded_frames = Vec::new();
    for image_url in image_urls {
        let image = decode_raster_data_url(image_url)?;
        decoded_frames.push(image.to_rgba8());
    }

    if decoded_frames.len() < 2 {
        return None;
    }

    let max_width = decoded_frames.iter().map(RgbaImage::width).max()?;
    let max_height = decoded_frames.iter().map(RgbaImage::height).max()?;
    let delay = Delay::from_numer_denom_ms(REPLAY_FRAME_DURATION_MS, 1);
    let frames = decoded_frames
        .into_iter()
        .map(|frame| Frame::from_parts(normalize_frame(frame, max_width, max_height), 0, 0, delay));

    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).ok()?;
        encoder.encode_frames(frames).ok()?;
    }

    Some(format!(
        "data:image/gif;base64,{}",
        BASE64_STANDARD.encode(bytes.into_inner())
    ))
}

fn normalize_frame(frame: RgbaImage, width: u32, height: u32) -> RgbaImage {
    if frame.width() == width && frame.height() == height {
        return frame;
    }

    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut canvas, &frame, 0, 0);
    canvas
}

fn decode_raster_data_url(image_url: &str) -> Option<image::DynamicImage> {
    if !is_raster_data_url(image_url) {
        return None;
    }
    let (_, encoded) = image_url.split_once(',')?;
    let bytes = BASE64_STANDARD.decode(encoded).ok()?;
    image::load_from_memory(&bytes).ok()
}

fn is_raster_data_url(image_url: &str) -> bool {
    image_url.starts_with("data:image/png;base64,")
        || image_url.starts_with("data:image/jpeg;base64,")
        || image_url.starts_with("data:image/webp;base64,")
}

fn is_gif_data_url(image_url: &str) -> bool {
    image_url.starts_with("data:image/gif;base64,")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::DynamicToolCallStatus;
    use image::DynamicImage;

    fn png_data_url(color: [u8; 4], width: u32, height: u32) -> String {
        let image = RgbaImage::from_pixel(width, height, Rgba(color));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode png");
        format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(bytes.into_inner())
        )
    }

    #[test]
    fn builds_replay_from_remote_browser_tool_calls() {
        let items = vec![
            ThreadItem::DynamicToolCall {
                id: "tool-1".to_string(),
                tool: "create_tab".to_string(),
                arguments: JsonValue::Null,
                status: DynamicToolCallStatus::Completed,
                content_items: Some(vec![
                    DynamicToolCallOutputContentItem::InputText {
                        text: serde_json::json!({
                            "result": { "tab_id": "tab_1" },
                            "browser_state": {
                                "selectedTabId": "tab_1",
                                "tabs": [{
                                    "id": "tab_1",
                                    "title": "First",
                                    "url": "https://example.com/one",
                                    "selected": true
                                }]
                            }
                        })
                        .to_string(),
                    },
                    DynamicToolCallOutputContentItem::InputImage {
                        image_url: png_data_url([255, 0, 0, 255], 10, 10),
                    },
                ]),
                success: Some(true),
                duration_ms: Some(10),
            },
            ThreadItem::DynamicToolCall {
                id: "tool-2".to_string(),
                tool: "tabs_content".to_string(),
                arguments: JsonValue::Null,
                status: DynamicToolCallStatus::Completed,
                content_items: Some(vec![
                    DynamicToolCallOutputContentItem::InputText {
                        text: serde_json::json!({
                            "result": {
                                "tabs": [{
                                    "title": "First",
                                    "content": "headline one\nheadline two"
                                }]
                            },
                            "browser_state": {
                                "selectedTabId": "tab_1",
                                "tabs": [{
                                    "id": "tab_1",
                                    "title": "First",
                                    "url": "https://example.com/one",
                                    "selected": true
                                }]
                            }
                        })
                        .to_string(),
                    },
                    DynamicToolCallOutputContentItem::InputImage {
                        image_url: png_data_url([0, 0, 255, 255], 12, 8),
                    },
                ]),
                success: Some(true),
                duration_ms: Some(10),
            },
        ];

        let replay = build_browser_replay_item("turn-1", &items).expect("replay item");
        let ThreadItem::BrowserReplay {
            id,
            frames,
            render_mode,
            animation_image_url,
            frame_duration_ms,
        } = replay
        else {
            panic!("expected browser replay item");
        };

        assert_eq!(id, "browser-replay-turn-1");
        assert_eq!(frames.len(), 2);
        assert_eq!(render_mode, BrowserReplayRenderMode::Animation);
        assert_eq!(frames[0].source_item_id, "tool-1");
        assert_eq!(frames[1].source_item_id, "tool-2");
        let second_snapshot = frames[1]
            .text_snapshot
            .as_deref()
            .expect("second frame snapshot");
        assert!(second_snapshot.contains("\"title\": \"First\""));
        assert!(second_snapshot.contains("\"content\": \"headline one\\nheadline two\""));
        assert_eq!(
            frames[1].text_snapshot_format,
            Some(BrowserReplayTextSnapshotFormat::Json)
        );
        assert!(
            animation_image_url
                .as_deref()
                .is_some_and(|value| value.starts_with("data:image/gif;base64,"))
        );
        assert_eq!(frame_duration_ms, Some(REPLAY_FRAME_DURATION_MS));
    }

    #[test]
    fn builds_text_only_replay_when_no_images_are_available() {
        let items = vec![ThreadItem::DynamicToolCall {
            id: "tool-1".to_string(),
            tool: "tabs_content".to_string(),
            arguments: JsonValue::Null,
            status: DynamicToolCallStatus::Completed,
            content_items: Some(vec![DynamicToolCallOutputContentItem::InputText {
                text: serde_json::json!({
                    "result": {
                        "tree": {
                            "role": "document",
                            "name": "Example page"
                        }
                    }
                })
                .to_string(),
            }]),
            success: Some(true),
            duration_ms: Some(5),
        }];

        let replay = build_browser_replay_item("turn-2", &items).expect("replay item");
        let ThreadItem::BrowserReplay {
            frames,
            render_mode,
            animation_image_url,
            frame_duration_ms,
            ..
        } = replay
        else {
            panic!("expected browser replay item");
        };

        assert_eq!(frames.len(), 1);
        assert_eq!(render_mode, BrowserReplayRenderMode::TextOnly);
        assert_eq!(
            frames[0].text_snapshot.as_deref(),
            Some(
                "{\n  \"tree\": {\n    \"role\": \"document\",\n    \"name\": \"Example page\"\n  }\n}"
            )
        );
        assert_eq!(
            frames[0].text_snapshot_format,
            Some(BrowserReplayTextSnapshotFormat::Json)
        );
        assert_eq!(animation_image_url, None);
        assert_eq!(frame_duration_ms, None);
    }

    #[test]
    fn assigns_frame_duration_to_multi_frame_text_only_replays() {
        let items = vec![
            ThreadItem::DynamicToolCall {
                id: "tool-1".to_string(),
                tool: "tabs_content".to_string(),
                arguments: JsonValue::Null,
                status: DynamicToolCallStatus::Completed,
                content_items: Some(vec![DynamicToolCallOutputContentItem::InputText {
                    text: serde_json::json!({
                        "result": { "tree": { "role": "document", "name": "Step 1" } }
                    })
                    .to_string(),
                }]),
                success: Some(true),
                duration_ms: Some(5),
            },
            ThreadItem::DynamicToolCall {
                id: "tool-2".to_string(),
                tool: "tabs_content".to_string(),
                arguments: JsonValue::Null,
                status: DynamicToolCallStatus::Completed,
                content_items: Some(vec![DynamicToolCallOutputContentItem::InputText {
                    text: serde_json::json!({
                        "result": { "tree": { "role": "document", "name": "Step 2" } }
                    })
                    .to_string(),
                }]),
                success: Some(true),
                duration_ms: Some(5),
            },
        ];

        let replay = build_browser_replay_item("turn-3", &items).expect("replay item");
        let ThreadItem::BrowserReplay {
            frames,
            render_mode,
            animation_image_url,
            frame_duration_ms,
            ..
        } = replay
        else {
            panic!("expected browser replay item");
        };

        assert_eq!(frames.len(), 2);
        assert_eq!(render_mode, BrowserReplayRenderMode::TextOnly);
        assert_eq!(animation_image_url, None);
        assert_eq!(frame_duration_ms, Some(REPLAY_FRAME_DURATION_MS));
    }

    #[test]
    fn builds_replay_from_atlas_command_tool_calls() {
        let items = vec![ThreadItem::DynamicToolCall {
            id: "tool-1".to_string(),
            tool: "atlas_command".to_string(),
            arguments: JsonValue::Null,
            status: DynamicToolCallStatus::Completed,
            content_items: Some(vec![
                DynamicToolCallOutputContentItem::InputText {
                    text: serde_json::json!({
                        "type": "codex_repl",
                        "command_output": {
                            "results": [{
                                "title": "Atlas search",
                                "content": "result one\nresult two"
                            }]
                        },
                        "browser_state": {
                            "selectedTabId": "tab_1",
                            "tabs": [{
                                "id": "tab_1",
                                "title": "Atlas search",
                                "url": "https://example.com/search",
                                "selected": true
                            }]
                        }
                    })
                    .to_string(),
                },
                DynamicToolCallOutputContentItem::InputImage {
                    image_url: png_data_url([0, 255, 0, 255], 14, 9),
                },
            ]),
            success: Some(true),
            duration_ms: Some(10),
        }];

        let replay = build_browser_replay_item("turn-atlas", &items).expect("replay item");
        let ThreadItem::BrowserReplay {
            frames,
            render_mode,
            animation_image_url,
            frame_duration_ms,
            ..
        } = replay
        else {
            panic!("expected browser replay item");
        };

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].tool, "atlas_command");
        assert_eq!(render_mode, BrowserReplayRenderMode::Frames);
        assert_eq!(
            frames[0].browser_state.as_ref().map(|state| state.selected_tab_id.as_str()),
            Some("tab_1")
        );
        let snapshot = frames[0]
            .text_snapshot
            .as_deref()
            .expect("atlas command snapshot");
        assert!(snapshot.contains("\"title\": \"Atlas search\""));
        assert!(snapshot.contains("\"content\": \"result one\\nresult two\""));
        assert_eq!(
            frames[0].text_snapshot_format,
            Some(BrowserReplayTextSnapshotFormat::Json)
        );
        assert!(frames[0].image_url.is_some());
        assert_eq!(animation_image_url, None);
        assert_eq!(frame_duration_ms, None);
    }
}
