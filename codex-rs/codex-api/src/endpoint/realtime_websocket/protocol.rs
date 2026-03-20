use codex_protocol::protocol::RealtimeCloseRequested;
use codex_protocol::protocol::RealtimeEvent;
use codex_protocol::protocol::RealtimeHandoffRequested;
use codex_protocol::protocol::RealtimeInputAudioSpeechStarted;
use codex_protocol::protocol::RealtimeInterruptRequested;
use codex_protocol::protocol::RealtimeResponseCancelled;
use codex_protocol::protocol::RealtimeToolAction;
use codex_protocol::protocol::RealtimeToolActionRequested;
use codex_protocol::protocol::RealtimeTranscriptDelta;
use serde::Deserialize;
use serde_json::Value;
use std::string::ToString;
use tracing::debug;

pub(crate) fn parse_realtime_event(payload: &str) -> Option<RealtimeEvent> {
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
    let output = parsed
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)?;
    output.iter().find(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call")
            && item.get("name").and_then(Value::as_str) == Some(name)
    })
}

#[derive(Debug, Default)]
struct ParsedHandoffArguments {
    input_transcript: String,
    send_immediately: bool,
}

fn parse_handoff_arguments(arguments: &str) -> ParsedHandoffArguments {
    #[derive(Debug, Deserialize, Default)]
    struct RawHandoffArguments {
        #[serde(default)]
        input_transcript: String,
        #[serde(default)]
        send_immediately: bool,
    }

    serde_json::from_str::<RawHandoffArguments>(arguments)
        .map(|raw| ParsedHandoffArguments {
            input_transcript: raw.input_transcript,
            send_immediately: raw.send_immediately,
        })
        .unwrap_or_else(|_| ParsedHandoffArguments {
            input_transcript: arguments.to_string(),
            send_immediately: false,
        })
}
