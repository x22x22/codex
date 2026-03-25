use crate::remote_browser_api::RemoteBrowserApi;
use crate::remote_browser_api::RemoteBrowserCommandOutcome;
use codex_app_server_protocol::BrowserSessionArtifacts;
use codex_app_server_protocol::BrowserSessionCommandParams;
use codex_app_server_protocol::BrowserSessionState;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallResponse;
use codex_app_server_protocol::DynamicToolSpec;
use serde_json::Map;
use serde_json::Value as JsonValue;
use serde_json::json;

const ATLAS_COMMAND_TOOL: &str = "atlas_command";
const ATLAS_DISPLAY_TRUNCATE_MAX_CHARS: u32 = 4_000;

#[derive(Debug)]
pub(crate) struct AtlasCommandExecution {
    pub(crate) browser_session_id: Option<String>,
    pub(crate) browser_state: Option<BrowserSessionState>,
    pub(crate) artifacts: Option<BrowserSessionArtifacts>,
    pub(crate) response: DynamicToolCallResponse,
}

pub(crate) fn merge_atlas_command_dynamic_tool(
    mut tools: Vec<DynamicToolSpec>,
) -> Vec<DynamicToolSpec> {
    if !tools.iter().any(|tool| tool.name == ATLAS_COMMAND_TOOL) {
        tools.push(DynamicToolSpec {
            name: ATLAS_COMMAND_TOOL.to_string(),
            description: "Hidden bridge for AgentLib inside js_repl. Call via codex.tool(...) only."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "payload": {
                        "type": "object",
                        "additionalProperties": true
                    }
                },
                "required": ["payload"],
                "additionalProperties": false
            }),
            defer_loading: true,
        });
    }

    tools
}

pub(crate) fn is_atlas_command_dynamic_tool(name: &str) -> bool {
    name == ATLAS_COMMAND_TOOL
}

pub(crate) async fn execute_atlas_command(
    remote_browser_api: &RemoteBrowserApi,
    browser_session_id: Option<String>,
    arguments: JsonValue,
) -> AtlasCommandExecution {
    match execute_atlas_command_impl(remote_browser_api, browser_session_id, arguments).await {
        Ok(result) => result,
        Err(message) => AtlasCommandExecution {
            browser_session_id: None,
            browser_state: None,
            artifacts: None,
            response: atlas_json_response(json!({
                "type": "codex_repl",
                "error": message,
            }), None),
        },
    }
}

async fn execute_atlas_command_impl(
    remote_browser_api: &RemoteBrowserApi,
    browser_session_id: Option<String>,
    arguments: JsonValue,
) -> Result<AtlasCommandExecution, String> {
    let payload = arguments
        .as_object()
        .and_then(|args| args.get("payload"))
        .cloned()
        .ok_or_else(|| "atlas_command requires an object payload".to_string())?;
    let payload_object = payload
        .as_object()
        .ok_or_else(|| "atlas_command payload must be an object".to_string())?;
    let command_type = payload_object
        .get("type")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "atlas_command payload.type is required".to_string())?;

    let mut session_id = browser_session_id;
    let mut last_browser_state = None;
    let mut last_artifacts = None;

    let command_output = match command_type {
        "runtime_config" => json!({
            "display_truncate_max_chars": ATLAS_DISPLAY_TRUNCATE_MAX_CHARS,
        }),
        "create_tab" => {
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                "create_tab",
                json!({ "select": true }),
            )
            .await?;
            let result = outcome.result.clone();
            last_browser_state = outcome.browser_state.to_public_state();
            last_artifacts = outcome.to_public_artifacts();
            json!({
                "id": result.get("tab_id").and_then(JsonValue::as_str).unwrap_or_default(),
            })
        }
        "list_tabs" => {
            let outcome =
                call_remote(remote_browser_api, &mut session_id, "list_tabs", json!({})).await?;
            let tabs = outcome
                .result
                .get("tabs")
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|tab| {
                    let tab_object = tab.as_object().cloned().unwrap_or_default();
                    json!({
                        "id": tab_object
                            .get("id")
                            .cloned()
                            .unwrap_or(JsonValue::String(String::new())),
                        "title": tab_object.get("title").cloned().unwrap_or(JsonValue::Null),
                        "url": tab_object.get("url").cloned().unwrap_or(JsonValue::Null),
                    })
                })
                .collect::<Vec<_>>();
            last_browser_state = outcome.browser_state.to_public_state();
            json!({ "tabs": tabs })
        }
        "selected_tab" => {
            let outcome =
                call_remote(remote_browser_api, &mut session_id, "selected_tab", json!({}))
                    .await?;
            let selected_tab_id = outcome
                .result
                .get("selected_tab_id")
                .and_then(JsonValue::as_str);
            last_browser_state = outcome.browser_state.to_public_state();
            json!({
                "id": selected_tab_id,
            })
        }
        "navigate_tab_url" => {
            let remote_args = normalize_passthrough_payload(payload_object);
            let outcome =
                call_remote(remote_browser_api, &mut session_id, "navigate_tab_url", remote_args)
                    .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            last_artifacts = outcome.to_public_artifacts();
            json!({})
        }
        "playwright_wait_for_load_state" => {
            let remote_args = normalize_passthrough_payload(payload_object);
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                "playwright_wait_for_load_state",
                remote_args,
            )
            .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            last_artifacts = outcome.to_public_artifacts();
            json!({})
        }
        "playwright_locator_count"
        | "playwright_locator_text_content"
        | "playwright_locator_inner_text"
        | "playwright_locator_get_attribute"
        | "playwright_locator_is_visible"
        | "playwright_locator_is_enabled" => {
            let remote_args = normalize_passthrough_payload(payload_object);
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                command_type,
                remote_args,
            )
            .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            outcome.result.clone()
        }
        "playwright_locator_click" | "playwright_locator_dblclick" => {
            let remote_args = normalize_passthrough_payload(payload_object);
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                command_type,
                remote_args,
            )
            .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            last_artifacts = outcome.to_public_artifacts();
            json!({})
        }
        "playwright_locator_wait_for" => {
            let remote_args = normalize_passthrough_payload(payload_object);
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                "playwright_locator_wait_for",
                remote_args,
            )
            .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            json!({})
        }
        "playwright_screenshot" => {
            if has_any_keys(
                payload_object,
                &[
                    "cropX",
                    "cropY",
                    "cropWidth",
                    "cropHeight",
                    "crop_x",
                    "crop_y",
                    "crop_width",
                    "crop_height",
                ],
            ) {
                return Err(
                    "atlas_command playwright_screenshot crop options are not supported by the remote browser bridge yet"
                        .to_string(),
                );
            }
            let remote_args = normalize_passthrough_payload(payload_object);
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                "playwright_screenshot",
                remote_args,
            )
            .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            last_artifacts = outcome.to_public_artifacts();
            json!({
                "data": outcome
                    .screenshot_base64()
                    .ok_or_else(|| "remote browser did not return screenshot data".to_string())?,
            })
        }
        "cua_get_visible_screenshot" => {
            let tab_id = payload_object
                .get("tab_id")
                .cloned()
                .ok_or_else(|| "cua_get_visible_screenshot requires tab_id".to_string())?;
            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                "playwright_screenshot",
                json!({
                    "tab_id": tab_id,
                    "full_page": false,
                }),
            )
            .await?;
            last_browser_state = outcome.browser_state.to_public_state();
            last_artifacts = outcome.to_public_artifacts();
            json!({
                "data": outcome
                    .screenshot_base64()
                    .ok_or_else(|| "remote browser did not return screenshot data".to_string())?,
            })
        }
        "tabs_content" => {
            let urls = payload_object
                .get("urls")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| "tabs_content requires urls".to_string())?;
            let content_type = payload_object
                .get("content_type")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "tabs_content requires content_type".to_string())?;
            if content_type != "text" {
                return Err(format!(
                    "atlas_command tabs_content only supports content_type=\"text\" today; got {content_type:?}"
                ));
            }

            let timeout_ms = payload_object.get("timeout_ms").cloned();
            let mut tab_ids = Vec::with_capacity(urls.len());
            for url in urls {
                let url = url
                    .as_str()
                    .ok_or_else(|| "tabs_content urls must be strings".to_string())?;
                let mut create_args = Map::new();
                create_args.insert("url".to_string(), JsonValue::String(url.to_string()));
                create_args.insert("select".to_string(), JsonValue::Bool(false));
                if let Some(timeout_ms) = timeout_ms.clone() {
                    create_args.insert("timeout_ms".to_string(), timeout_ms);
                }
                let outcome = call_remote(
                    remote_browser_api,
                    &mut session_id,
                    "create_tab",
                    JsonValue::Object(create_args),
                )
                .await?;
                let tab_id = outcome
                    .result
                    .get("tab_id")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| "remote browser create_tab did not return tab_id".to_string())?;
                tab_ids.push(tab_id.to_string());
            }

            let outcome = call_remote(
                remote_browser_api,
                &mut session_id,
                "tabs_content",
                json!({ "tab_ids": tab_ids }),
            )
            .await?;
            let tabs = outcome
                .result
                .get("tabs")
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|tab| {
                    let tab_object = tab.as_object().cloned().unwrap_or_default();
                    json!({
                        "url": tab_object
                            .get("url")
                            .cloned()
                            .unwrap_or(JsonValue::String(String::new())),
                        "title": tab_object.get("title").cloned().unwrap_or(JsonValue::Null),
                        "content": tab_object.get("content").cloned().unwrap_or(JsonValue::Null),
                    })
                })
                .collect::<Vec<_>>();
            last_browser_state = outcome.browser_state.to_public_state();
            json!({ "results": tabs })
        }
        other => {
            return Err(format!(
                "atlas_command does not support AgentLib command type {other:?} yet"
            ));
        }
    };

    let response = atlas_json_response(
        json!({
            "type": "codex_repl",
            "command_output": command_output,
            "browser_state": last_browser_state.clone(),
            "artifacts": last_artifacts.clone(),
        }),
        last_artifacts.as_ref(),
    );

    Ok(AtlasCommandExecution {
        browser_session_id: session_id,
        browser_state: last_browser_state,
        artifacts: last_artifacts,
        response,
    })
}

fn atlas_json_response(
    payload: JsonValue,
    artifacts: Option<&BrowserSessionArtifacts>,
) -> DynamicToolCallResponse {
    let mut content_items = vec![DynamicToolCallOutputContentItem::InputText {
            text: serde_json::to_string(&payload).unwrap_or_else(|_| {
                "{\"type\":\"codex_repl\",\"error\":\"serialization failure\"}".to_string()
            }),
        }];

    if let Some(image_url) = artifacts.and_then(|value| value.screenshot_image_url.clone()) {
        content_items.push(DynamicToolCallOutputContentItem::InputImage { image_url });
    }
    if let Some(image_url) = artifacts.and_then(|value| value.replay_gif_image_url.clone()) {
        content_items.push(DynamicToolCallOutputContentItem::InputImage { image_url });
    }

    DynamicToolCallResponse {
        content_items,
        success: true,
    }
}

async fn call_remote(
    remote_browser_api: &RemoteBrowserApi,
    browser_session_id: &mut Option<String>,
    command: &str,
    arguments: JsonValue,
) -> Result<RemoteBrowserCommandOutcome, String> {
    let outcome = remote_browser_api
        .command_with_artifacts(BrowserSessionCommandParams {
            browser_session_id: browser_session_id.clone(),
            command: command.to_string(),
            arguments: Some(arguments),
        })
        .await
        .map_err(|err| err.message)?;
    *browser_session_id = Some(outcome.browser_session_id.clone());
    Ok(outcome)
}

fn normalize_passthrough_payload(payload: &Map<String, JsonValue>) -> JsonValue {
    let mut args = payload.clone();
    args.remove("type");
    JsonValue::Object(args)
}

fn has_any_keys(payload: &Map<String, JsonValue>, keys: &[&str]) -> bool {
    keys.iter().any(|key| payload.contains_key(*key))
}
