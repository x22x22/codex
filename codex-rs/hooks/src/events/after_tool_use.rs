use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;

use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::engine::output_parser;
use crate::schema::AfterToolUseCommandInput;

#[derive(Debug, Clone)]
pub struct AfterToolUseRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub cwd: PathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub executed: bool,
    pub success: bool,
    pub duration_ms: u64,
    pub mutating: bool,
    pub sandbox: String,
    pub sandbox_policy: String,
    pub output_preview: String,
}

#[derive(Debug)]
pub struct AfterToolUseOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
    pub should_stop: bool,
    pub stop_reason: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct AfterToolUseHandlerData {
    should_stop: bool,
    stop_reason: Option<String>,
}

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    request: &AfterToolUseRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        HookEventName::AfterToolUse,
        Some(request.tool_name.as_str()),
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: AfterToolUseRequest,
) -> AfterToolUseOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::AfterToolUse,
        Some(request.tool_name.as_str()),
    );
    if matched.is_empty() {
        return AfterToolUseOutcome {
            hook_events: Vec::new(),
            should_stop: false,
            stop_reason: None,
        };
    }

    let input_json = match serde_json::to_string(&AfterToolUseCommandInput::new(
        request.session_id.to_string(),
        request.transcript_path.clone(),
        request.cwd.display().to_string(),
        request.model.clone(),
        request.permission_mode.clone(),
        request.turn_id.clone(),
        request.call_id.clone(),
        request.tool_name.clone(),
        request.executed,
        request.success,
        request.duration_ms,
        request.mutating,
        request.sandbox.clone(),
        request.sandbox_policy.clone(),
        request.output_preview.clone(),
    )) {
        Ok(input_json) => input_json,
        Err(error) => {
            return serialization_failure_outcome(
                matched,
                Some(request.turn_id),
                format!("failed to serialize after_tool_use hook input: {error}"),
            );
        }
    };

    let results = dispatcher::execute_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        Some(request.turn_id),
        parse_completed,
    )
    .await;

    let should_stop = results.iter().any(|result| result.data.should_stop);
    let stop_reason = results
        .iter()
        .find_map(|result| result.data.stop_reason.clone());

    AfterToolUseOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
        should_stop,
        stop_reason,
    }
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<AfterToolUseHandlerData> {
    let mut entries = Vec::new();
    let mut status = HookRunStatus::Completed;
    let mut should_stop = false;
    let mut stop_reason = None;

    match run_result.error.as_deref() {
        Some(error) => {
            status = HookRunStatus::Failed;
            entries.push(HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error.to_string(),
            });
        }
        None => match run_result.exit_code {
            Some(0) => {
                let trimmed_stdout = run_result.stdout.trim();
                if trimmed_stdout.is_empty() {
                } else if let Some(parsed) = output_parser::parse_after_tool_use(&run_result.stdout)
                {
                    if let Some(system_message) = parsed.universal.system_message {
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Warning,
                            text: system_message,
                        });
                    }
                    if !parsed.universal.continue_processing {
                        status = HookRunStatus::Stopped;
                        should_stop = true;
                        stop_reason = parsed.universal.stop_reason.clone();
                        if let Some(stop_reason_text) = parsed.universal.stop_reason {
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Stop,
                                text: stop_reason_text,
                            });
                        }
                    }
                } else if trimmed_stdout.starts_with('{') || trimmed_stdout.starts_with('[') {
                    status = HookRunStatus::Failed;
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Error,
                        text: "hook returned invalid after_tool_use JSON output".to_string(),
                    });
                }
            }
            Some(exit_code) => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: format!("hook exited with code {exit_code}"),
                });
            }
            None => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: "hook exited without a status code".to_string(),
                });
            }
        },
    }

    let completed = HookCompletedEvent {
        turn_id,
        run: dispatcher::completed_summary(handler, &run_result, status, entries),
    };

    dispatcher::ParsedHandler {
        completed,
        data: AfterToolUseHandlerData {
            should_stop,
            stop_reason,
        },
    }
}

fn serialization_failure_outcome(
    handlers: Vec<ConfiguredHandler>,
    turn_id: Option<String>,
    error_message: String,
) -> AfterToolUseOutcome {
    let hook_events = handlers
        .into_iter()
        .map(|handler| {
            let mut run = dispatcher::running_summary(&handler);
            run.status = HookRunStatus::Failed;
            run.completed_at = Some(run.started_at);
            run.duration_ms = Some(0);
            run.entries = vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error_message.clone(),
            }];
            HookCompletedEvent {
                turn_id: turn_id.clone(),
                run,
            }
        })
        .collect();

    AfterToolUseOutcome {
        hook_events,
        should_stop: false,
        stop_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use codex_protocol::protocol::HookEventName;
    use codex_protocol::protocol::HookOutputEntry;
    use codex_protocol::protocol::HookOutputEntryKind;
    use codex_protocol::protocol::HookRunStatus;
    use pretty_assertions::assert_eq;

    use super::AfterToolUseHandlerData;
    use super::parse_completed;
    use crate::engine::ConfiguredHandler;
    use crate::engine::command_runner::CommandRunResult;

    #[test]
    fn empty_stdout_is_a_noop() {
        let parsed = parse_completed(&handler(), run_result(Some(0), "", ""), None);

        assert_eq!(
            parsed.data,
            AfterToolUseHandlerData {
                should_stop: false,
                stop_reason: None,
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Completed);
        assert_eq!(parsed.completed.run.entries, vec![]);
    }

    #[test]
    fn continue_true_is_a_noop() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), r#"{"continue":true}"#, ""),
            None,
        );

        assert_eq!(
            parsed.data,
            AfterToolUseHandlerData {
                should_stop: false,
                stop_reason: None,
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Completed);
    }

    #[test]
    fn continue_false_stops_session() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), r#"{"continue":false,"stopReason":"blocked"}"#, ""),
            Some("turn-1".to_string()),
        );

        assert_eq!(
            parsed.data,
            AfterToolUseHandlerData {
                should_stop: true,
                stop_reason: Some("blocked".to_string()),
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Stopped);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Stop,
                text: "blocked".to_string(),
            }]
        );
    }

    #[test]
    fn invalid_json_like_stdout_fails() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), r#"{"continue":false"#, ""),
            None,
        );

        assert_eq!(
            parsed.data,
            AfterToolUseHandlerData {
                should_stop: false,
                stop_reason: None,
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Failed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "hook returned invalid after_tool_use JSON output".to_string(),
            }]
        );
    }

    fn handler() -> ConfiguredHandler {
        ConfiguredHandler {
            event_name: HookEventName::AfterToolUse,
            matcher: None,
            command: "echo hook".to_string(),
            timeout_sec: 600,
            status_message: None,
            source_path: PathBuf::from("/tmp/hooks.json"),
            display_order: 0,
        }
    }

    fn run_result(exit_code: Option<i32>, stdout: &str, stderr: &str) -> CommandRunResult {
        CommandRunResult {
            started_at: 1,
            completed_at: 2,
            duration_ms: 1,
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            error: None,
        }
    }
}
