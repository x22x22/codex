pub use codex_protocol::protocol::RealtimeAudioFrame;
pub use codex_protocol::protocol::RealtimeCloseRequested;
pub use codex_protocol::protocol::RealtimeEvent;
pub use codex_protocol::protocol::RealtimeHandoffRequested;
pub use codex_protocol::protocol::RealtimeInputAudioSpeechStarted;
pub use codex_protocol::protocol::RealtimeInterruptRequested;
pub use codex_protocol::protocol::RealtimeOutputAudioDelta;
pub use codex_protocol::protocol::RealtimeResponseCancelled;
pub use codex_protocol::protocol::RealtimeToolAction;
pub use codex_protocol::protocol::RealtimeToolActionRequested;
pub use codex_protocol::protocol::RealtimeTranscriptDelta;
pub use codex_protocol::protocol::RealtimeTranscriptEntry;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
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
    #[serde(rename = "conversation.item.truncate")]
    ConversationItemTruncate {
        item_id: String,
        content_index: u32,
        audio_end_ms: u32,
    },
    #[serde(rename = "session.update")]
    SessionUpdate { session: Box<SessionUpdateSession> },
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
    pub(super) noise_reduction: SessionNoiseReduction,
    pub(super) turn_detection: SessionTurnDetection,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionAudioFormat {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) rate: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionNoiseReduction {
    #[serde(rename = "type")]
    pub(super) kind: String,
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
    pub(super) properties: BTreeMap<String, SessionToolProperty>,
    pub(super) required: Vec<String>,
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

        "conversation.output_audio.delta"
        | "response.output_audio.delta"
        | "response.audio.delta" => {
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
            Some(RealtimeEvent::AudioOut(RealtimeOutputAudioDelta {
                frame: RealtimeAudioFrame {
                    data,
                    sample_rate,
                    num_channels,
                    samples_per_channel: parsed
                        .get("samples_per_channel")
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok()),
                },
                item_id: parsed
                    .get("item_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }))
        }
        "input_audio_buffer.speech_started" => Some(RealtimeEvent::InputAudioSpeechStarted(
            RealtimeInputAudioSpeechStarted {
                item_id: parsed
                    .get("item_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        )),
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
                send_immediately: false,
                active_transcript: Vec::new(),
            }))
        }
        "response.done" => {
            if let Some(handoff) = parse_handoff_requested(&parsed) {
                return Some(RealtimeEvent::HandoffRequested(handoff));
            }
            if let Some(interrupt) = parse_interrupt_requested(&parsed) {
                return Some(RealtimeEvent::InterruptRequested(interrupt));
            }
            if let Some(close) = parse_close_requested(&parsed) {
                return Some(RealtimeEvent::CloseRequested(close));
            }
            if let Some(tool_action) = parse_tool_action_requested(&parsed) {
                return Some(RealtimeEvent::ToolActionRequested(tool_action));
            }
            if let Some(cancelled) = parse_response_cancelled(&parsed) {
                return Some(RealtimeEvent::ResponseCancelled(cancelled));
            }
            Some(RealtimeEvent::ConversationItemAdded(parsed))
        }
        "response.cancelled" => Some(RealtimeEvent::ResponseCancelled(
            RealtimeResponseCancelled {
                response_id: parsed
                    .get("response")
                    .and_then(Value::as_object)
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        parsed
                            .get("response_id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
            },
        )),
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
    let function_call = find_function_call(parsed, "codex")?;
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
    let parsed_arguments = parse_handoff_arguments(arguments);
    Some(RealtimeHandoffRequested {
        handoff_id,
        item_id,
        input_transcript: parsed_arguments.input_transcript,
        send_immediately: parsed_arguments.send_immediately,
        active_transcript: Vec::new(),
    })
}

fn parse_interrupt_requested(parsed: &Value) -> Option<RealtimeInterruptRequested> {
    let function_call = find_function_call(parsed, "cancel_current_operation")?;
    Some(RealtimeInterruptRequested {
        call_id: function_call
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_string)?,
    })
}

fn parse_close_requested(parsed: &Value) -> Option<RealtimeCloseRequested> {
    let function_call = find_function_call(parsed, "turn_off_realtime_mode")?;
    Some(RealtimeCloseRequested {
        call_id: function_call
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_string)?,
    })
}

fn parse_tool_action_requested(parsed: &Value) -> Option<RealtimeToolActionRequested> {
    #[derive(Debug, Deserialize, Default)]
    struct ReplaceLastQueuedMessageArguments {
        #[serde(default)]
        message: String,
    }

    #[derive(Debug, Deserialize, Default)]
    struct ManageMessageQueueArguments {
        #[serde(default)]
        action: String,
        #[serde(default)]
        message: Option<String>,
    }

    #[derive(Debug, Deserialize, Default)]
    struct UpdateRuntimeSettingsArguments {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        working_directory: Option<String>,
        #[serde(default)]
        reasoning_effort: Option<String>,
        #[serde(default)]
        fast_mode: Option<bool>,
        #[serde(default)]
        personality: Option<String>,
        #[serde(default)]
        collaboration_mode: Option<String>,
    }

    #[derive(Debug, Deserialize, Default)]
    struct RunTuiCommandArguments {
        #[serde(default)]
        command: String,
        #[serde(default)]
        prompt: Option<String>,
    }

    let parse_call_id = |function_call: &Value| {
        function_call
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    if let Some(function_call) = find_function_call(parsed, "manage_message_queue") {
        let arguments = function_call
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parsed_arguments =
            serde_json::from_str::<ManageMessageQueueArguments>(arguments).unwrap_or_default();
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::ManageMessageQueue {
                action: parsed_arguments.action,
                message: parsed_arguments.message,
            },
        });
    }

    if let Some(function_call) = find_function_call(parsed, "list_message_queue") {
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::ListMessageQueue,
        });
    }

    if let Some(function_call) = find_function_call(parsed, "replace_last_queued_message") {
        let arguments = function_call
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parsed_arguments = serde_json::from_str::<ReplaceLastQueuedMessageArguments>(arguments)
            .unwrap_or_else(|_| ReplaceLastQueuedMessageArguments {
                message: arguments.to_string(),
            });
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::ReplaceLastQueuedMessage {
                message: parsed_arguments.message,
            },
        });
    }

    if let Some(function_call) = find_function_call(parsed, "remove_last_queued_message") {
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::RemoveLastQueuedMessage,
        });
    }

    if let Some(function_call) = find_function_call(parsed, "clear_queued_messages") {
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::ClearQueuedMessages,
        });
    }

    if let Some(function_call) = find_function_call(parsed, "list_runtime_settings") {
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::ListRuntimeSettings,
        });
    }

    if let Some(function_call) = find_function_call(parsed, "manage_runtime_settings") {
        let arguments = function_call
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parsed_arguments =
            serde_json::from_str::<UpdateRuntimeSettingsArguments>(arguments).unwrap_or_default();
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::ManageRuntimeSettings {
                model: parsed_arguments.model,
                working_directory: parsed_arguments.working_directory,
                reasoning_effort: parsed_arguments.reasoning_effort,
                fast_mode: parsed_arguments.fast_mode,
                personality: parsed_arguments.personality,
                collaboration_mode: parsed_arguments.collaboration_mode,
            },
        });
    }

    if let Some(function_call) = find_function_call(parsed, "update_runtime_settings") {
        let arguments = function_call
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parsed_arguments =
            serde_json::from_str::<UpdateRuntimeSettingsArguments>(arguments).unwrap_or_default();
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::UpdateRuntimeSettings {
                model: parsed_arguments.model,
                working_directory: parsed_arguments.working_directory,
                reasoning_effort: parsed_arguments.reasoning_effort,
                fast_mode: parsed_arguments.fast_mode,
                personality: parsed_arguments.personality,
                collaboration_mode: parsed_arguments.collaboration_mode,
            },
        });
    }

    if let Some(function_call) = find_function_call(parsed, "run_tui_command") {
        let arguments = function_call
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parsed_arguments =
            serde_json::from_str::<RunTuiCommandArguments>(arguments).unwrap_or_default();
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::RunTuiCommand {
                command: parsed_arguments.command,
                prompt: parsed_arguments.prompt,
            },
        });
    }

    if let Some(function_call) = find_function_call(parsed, "compact_conversation") {
        return Some(RealtimeToolActionRequested {
            call_id: parse_call_id(function_call)?,
            action: RealtimeToolAction::CompactConversation,
        });
    }

    None
}

fn parse_response_cancelled(parsed: &Value) -> Option<RealtimeResponseCancelled> {
    let response = parsed.get("response")?.as_object()?;
    let status = response.get("status").and_then(Value::as_str)?;
    if status != "cancelled" {
        return None;
    }

    Some(RealtimeResponseCancelled {
        response_id: response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn find_function_call<'a>(parsed: &'a Value, name: &str) -> Option<&'a Value> {
    parsed
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)?
        .iter()
        .find(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("name").and_then(Value::as_str) == Some(name)
        })
}

struct ParsedHandoffArguments {
    input_transcript: String,
    send_immediately: bool,
}

fn parse_handoff_arguments(arguments: &str) -> ParsedHandoffArguments {
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
        send_immediately: bool,
        #[serde(default)]
        messages: Vec<RealtimeTranscriptEntry>,
    }

    let Some(parsed) = serde_json::from_str::<HandoffArguments>(arguments).ok() else {
        return ParsedHandoffArguments {
            input_transcript: arguments.to_string(),
            send_immediately: false,
        };
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
            return ParsedHandoffArguments {
                input_transcript: value,
                send_immediately: parsed.send_immediately,
            };
        }
    }

    if let Some(message) = parsed
        .messages
        .into_iter()
        .find(|message| message.role == "user" && !message.text.is_empty())
    {
        return ParsedHandoffArguments {
            input_transcript: message.text,
            send_immediately: parsed.send_immediately,
        };
    }

    ParsedHandoffArguments {
        input_transcript: String::new(),
        send_immediately: parsed.send_immediately,
    }
}
