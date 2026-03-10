pub use codex_protocol::protocol::RealtimeAudioFrame;
pub use codex_protocol::protocol::RealtimeEvent;
pub use codex_protocol::protocol::RealtimeHandoffRequested;
pub use codex_protocol::protocol::RealtimeTranscriptDelta;
pub use codex_protocol::protocol::RealtimeTranscriptEntry;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::string::ToString;
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSessionConfig {
    pub instructions: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub(super) enum RealtimeOutboundMessage {
    #[serde(rename = "input_audio_buffer.append")]
    InputAudioBufferAppend { audio: String },
    #[serde(rename = "response.create")]
    ResponseCreate,
    #[serde(rename = "session.update")]
    SessionUpdate { session: SessionUpdateSession },
    #[serde(rename = "conversation.item.create")]
    ConversationItemCreate { item: ConversationItem },
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionUpdateSession {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) instructions: String,
    pub(super) output_modalities: Vec<String>,
    pub(super) audio: SessionAudio,
    pub(super) tools: Vec<SessionTool>,
    pub(super) tool_choice: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionAudio {
    pub(super) input: SessionAudioInput,
    pub(super) output: SessionAudioOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionAudioInput {
    pub(super) format: SessionAudioFormat,
    pub(super) turn_detection: SessionTurnDetection,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionAudioFormat {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) rate: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionTurnDetection {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) interrupt_response: bool,
    pub(super) create_response: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionAudioOutput {
    pub(super) format: SessionAudioOutputFormat,
    pub(super) voice: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionAudioOutputFormat {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) rate: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionTool {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: SessionToolParameters,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionToolParameters {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) properties: SessionToolProperties,
    pub(super) required: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionToolProperties {
    pub(super) prompt: SessionToolProperty,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionToolProperty {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) description: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub(super) enum ConversationItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<ConversationItemContent>,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ConversationItemContent {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) text: String,
}

pub(super) fn parse_realtime_event(payload: &str) -> Option<RealtimeEvent> {
    let parsed: Value = match serde_json::from_str(payload) {
        Ok(msg) => msg,
        Err(err) => {
            debug!("failed to parse realtime event: {err}, data: {payload}");
            return None;
        }
    };

    let message_type = match parsed.get("type").and_then(Value::as_str) {
        Some(message_type) => message_type,
        None => {
            debug!("received realtime event without type field: {payload}");
            return None;
        }
    };
    match message_type {
        "session.created" | "session.updated" => {
            let session_id = parsed
                .get("session")
                .and_then(Value::as_object)
                .and_then(|session| session.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let instructions = parsed
                .get("session")
                .and_then(Value::as_object)
                .and_then(|session| session.get("instructions"))
                .and_then(Value::as_str)
                .map(str::to_string);
            session_id.map(|session_id| RealtimeEvent::SessionUpdated {
                session_id,
                instructions,
            })
        }
        "conversation.output_audio.delta" | "response.output_audio.delta" => {
            let data = parsed
                .get("delta")
                .and_then(Value::as_str)
                .or_else(|| parsed.get("data").and_then(Value::as_str))
                .map(str::to_string)?;
            let sample_rate = parsed
                .get("sample_rate")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(24_000);
            let num_channels = parsed
                .get("channels")
                .or_else(|| parsed.get("num_channels"))
                .and_then(Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(1);
            Some(RealtimeEvent::AudioOut(RealtimeAudioFrame {
                data,
                sample_rate,
                num_channels,
                samples_per_channel: parsed
                    .get("samples_per_channel")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok()),
            }))
        }
        "conversation.input_transcript.delta" => parsed
            .get("delta")
            .and_then(Value::as_str)
            .map(str::to_string)
            .map(|delta| RealtimeEvent::InputTranscriptDelta(RealtimeTranscriptDelta { delta })),
        "conversation.output_transcript.delta" => parsed
            .get("delta")
            .and_then(Value::as_str)
            .map(str::to_string)
            .map(|delta| RealtimeEvent::OutputTranscriptDelta(RealtimeTranscriptDelta { delta })),
        "conversation.item.added" => parsed
            .get("item")
            .cloned()
            .map(RealtimeEvent::ConversationItemAdded),
        "conversation.item.done" => parsed
            .get("item")
            .and_then(Value::as_object)
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .map(|item_id| RealtimeEvent::ConversationItemDone { item_id }),
        "conversation.handoff.requested" => {
            let handoff_id = parsed
                .get("handoff_id")
                .and_then(Value::as_str)
                .map(str::to_string)?;
            let item_id = parsed
                .get("item_id")
                .and_then(Value::as_str)
                .map(str::to_string)?;
            let input_transcript = parsed
                .get("input_transcript")
                .and_then(Value::as_str)
                .map(str::to_string)?;
            Some(RealtimeEvent::HandoffRequested(RealtimeHandoffRequested {
                handoff_id,
                item_id,
                input_transcript,
                active_transcript: Vec::new(),
            }))
        }
        "response.done" => {
            if let Some(handoff) = parse_handoff_requested(&parsed) {
                return Some(RealtimeEvent::HandoffRequested(handoff));
            }
            Some(RealtimeEvent::ConversationItemAdded(parsed))
        }
        "error" => parsed
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                parsed
                    .get("error")
                    .and_then(Value::as_object)
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| parsed.get("error").map(ToString::to_string))
            .map(RealtimeEvent::Error),
        _ => Some(RealtimeEvent::ConversationItemAdded(parsed)),
    }
}

fn parse_handoff_requested(parsed: &Value) -> Option<RealtimeHandoffRequested> {
    let outputs = parsed
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)?;
    let function_call = outputs.iter().find(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call")
            && item.get("name").and_then(Value::as_str) == Some("codex")
    })?;
    let handoff_id = function_call
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_string)?;
    let item_id = function_call
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| handoff_id.clone());
    let arguments = function_call
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(RealtimeHandoffRequested {
        handoff_id,
        item_id,
        input_transcript: parse_handoff_arguments(arguments),
        active_transcript: Vec::new(),
    })
}

fn parse_handoff_arguments(arguments: &str) -> String {
    #[derive(Debug, Deserialize)]
    struct HandoffArguments {
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        input: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        input_transcript: Option<String>,
        #[serde(default)]
        messages: Vec<RealtimeTranscriptEntry>,
    }

    let Some(parsed) = serde_json::from_str::<HandoffArguments>(arguments).ok() else {
        return arguments.to_string();
    };

    for value in [
        parsed.prompt,
        parsed.text,
        parsed.input,
        parsed.message,
        parsed.input_transcript,
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_empty() {
            return value;
        }
    }

    if let Some(message) = parsed
        .messages
        .into_iter()
        .find(|message| message.role == "user" && !message.text.is_empty())
    {
        return message.text;
    }

    String::new()
}
