use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::exec::ExecToolCallOutput;
use crate::exec::StreamOutput;
use crate::features::Feature;
use crate::function_tool::FunctionCallError;
use crate::protocol::ExecCommandSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::events::ToolEventFailure;
use crate::tools::events::ToolEventStage;
use crate::tools::handlers::parse_arguments;
use crate::tools::ps_repl::PS_REPL_PRAGMA_PREFIX;
use crate::tools::ps_repl::PsExecResult;
use crate::tools::ps_repl::PsReplArgs;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;

pub struct PsReplHandler;
pub struct PsReplResetHandler;

fn join_outputs(stdout: &str, stderr: &str) -> String {
    if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn build_ps_repl_exec_output(
    output: &str,
    error: Option<&str>,
    duration: Duration,
) -> ExecToolCallOutput {
    let stdout = output.to_string();
    let stderr = error.unwrap_or("").to_string();
    let aggregated_output = join_outputs(&stdout, &stderr);
    ExecToolCallOutput {
        exit_code: if error.is_some() { 1 } else { 0 },
        stdout: StreamOutput::new(stdout),
        stderr: StreamOutput::new(stderr),
        aggregated_output: StreamOutput::new(aggregated_output),
        duration,
        timed_out: false,
    }
}

async fn emit_ps_repl_exec_begin(
    session: &crate::codex::Session,
    turn: &crate::codex::TurnContext,
    call_id: &str,
) {
    let emitter = ToolEmitter::shell(
        vec!["ps_repl".to_string()],
        turn.cwd.clone(),
        ExecCommandSource::Agent,
        false,
    );
    let ctx = ToolEventCtx::new(session, turn, call_id, None);
    emitter.emit(ctx, ToolEventStage::Begin).await;
}

async fn emit_ps_repl_exec_end(
    session: &crate::codex::Session,
    turn: &crate::codex::TurnContext,
    call_id: &str,
    output: &str,
    error: Option<&str>,
    duration: Duration,
) {
    let exec_output = build_ps_repl_exec_output(output, error, duration);
    let emitter = ToolEmitter::shell(
        vec!["ps_repl".to_string()],
        turn.cwd.clone(),
        ExecCommandSource::Agent,
        false,
    );
    let ctx = ToolEventCtx::new(session, turn, call_id, None);
    let stage = if error.is_some() {
        ToolEventStage::Failure(ToolEventFailure::Output(exec_output))
    } else {
        ToolEventStage::Success(exec_output)
    };
    emitter.emit(ctx, stage).await;
}

#[async_trait]
impl ToolHandler for PsReplHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(
            payload,
            ToolPayload::Function { .. } | ToolPayload::Custom { .. }
        )
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tracker,
            payload,
            call_id,
            ..
        } = invocation;

        if !session.features().enabled(Feature::PsRepl) {
            return Err(FunctionCallError::RespondToModel(
                "ps_repl is disabled by feature flag".to_string(),
            ));
        }

        let args = match payload {
            ToolPayload::Function { arguments } => parse_arguments(&arguments)?,
            ToolPayload::Custom { input } => parse_freeform_args(&input)?,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "ps_repl expects custom or function payload".to_string(),
                ));
            }
        };
        let manager = turn.ps_repl.manager().await?;
        let started_at = Instant::now();
        emit_ps_repl_exec_begin(session.as_ref(), turn.as_ref(), &call_id).await;
        let result = manager
            .execute(Arc::clone(&session), Arc::clone(&turn), tracker, args)
            .await;
        let result = match result {
            Ok(result) => result,
            Err(err) => {
                let message = err.to_string();
                emit_ps_repl_exec_end(
                    session.as_ref(),
                    turn.as_ref(),
                    &call_id,
                    "",
                    Some(&message),
                    started_at.elapsed(),
                )
                .await;
                return Err(err);
            }
        };

        emit_ps_repl_exec_end(
            session.as_ref(),
            turn.as_ref(),
            &call_id,
            &result.output,
            None,
            started_at.elapsed(),
        )
        .await;

        Ok(build_tool_output(result))
    }
}

fn build_tool_output(result: PsExecResult) -> ToolOutput {
    let PsExecResult {
        output,
        content_items,
    } = result;
    let mut items = Vec::with_capacity(content_items.len() + 1);
    if !output.is_empty() {
        items.push(FunctionCallOutputContentItem::InputText {
            text: output.clone(),
        });
    }
    items.extend(content_items);

    ToolOutput::Function {
        body: if items.is_empty() {
            FunctionCallOutputBody::Text(output)
        } else {
            FunctionCallOutputBody::ContentItems(items)
        },
        success: Some(true),
    }
}

#[async_trait]
impl ToolHandler for PsReplResetHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        if !invocation.session.features().enabled(Feature::PsRepl) {
            return Err(FunctionCallError::RespondToModel(
                "ps_repl is disabled by feature flag".to_string(),
            ));
        }
        let manager = invocation.turn.ps_repl.manager().await?;
        manager.reset().await?;
        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text("ps_repl kernel reset".to_string()),
            success: Some(true),
        })
    }
}

fn parse_freeform_args(input: &str) -> Result<PsReplArgs, FunctionCallError> {
    if input.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "ps_repl expects raw PowerShell tool input (non-empty). Provide PowerShell source text, optionally with first-line `# codex-ps-repl: ...`."
                .to_string(),
        ));
    }

    let mut args = PsReplArgs {
        code: input.to_string(),
        timeout_ms: None,
    };

    let mut lines = input.splitn(2, '\n');
    let first_line = lines.next().unwrap_or_default();
    let rest = lines.next().unwrap_or_default();
    let trimmed = first_line.trim_start();
    let Some(pragma) = trimmed.strip_prefix(PS_REPL_PRAGMA_PREFIX) else {
        reject_json_or_quoted_source(&args.code)?;
        return Ok(args);
    };

    let mut timeout_ms: Option<u64> = None;
    let directive = pragma.trim();
    if !directive.is_empty() {
        for token in directive.split_whitespace() {
            let (key, value) = token.split_once('=').ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "ps_repl pragma expects space-separated key=value pairs (supported keys: timeout_ms); got `{token}`"
                ))
            })?;
            match key {
                "timeout_ms" => {
                    if timeout_ms.is_some() {
                        return Err(FunctionCallError::RespondToModel(
                            "ps_repl pragma specifies timeout_ms more than once".to_string(),
                        ));
                    }
                    let parsed = value.parse::<u64>().map_err(|_| {
                        FunctionCallError::RespondToModel(format!(
                            "ps_repl pragma timeout_ms must be an integer; got `{value}`"
                        ))
                    })?;
                    timeout_ms = Some(parsed);
                }
                _ => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "ps_repl pragma only supports timeout_ms; got `{key}`"
                    )));
                }
            }
        }
    }

    if rest.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "ps_repl pragma must be followed by PowerShell source on subsequent lines".to_string(),
        ));
    }

    reject_json_or_quoted_source(rest)?;
    args.code = rest.to_string();
    args.timeout_ms = timeout_ms;
    Ok(args)
}

fn reject_json_or_quoted_source(code: &str) -> Result<(), FunctionCallError> {
    let trimmed = code.trim();
    if trimmed.starts_with("```") {
        return Err(FunctionCallError::RespondToModel(
            "ps_repl expects raw PowerShell source, not markdown code fences. Resend plain PowerShell only (optional first line `# codex-ps-repl: ...`)."
                .to_string(),
        ));
    }
    if is_quoted_source(trimmed) {
        return Err(FunctionCallError::RespondToModel(
            "ps_repl is a freeform tool and expects raw PowerShell source. Resend plain PowerShell only (optional first line `# codex-ps-repl: ...`); do not send quoted code or markdown fences."
                .to_string(),
        ));
    }
    let Ok(value) = serde_json::from_str::<JsonValue>(trimmed) else {
        return Ok(());
    };
    match value {
        JsonValue::Object(_) | JsonValue::String(_) => Err(FunctionCallError::RespondToModel(
            "ps_repl is a freeform tool and expects raw PowerShell source. Resend plain PowerShell only (optional first line `# codex-ps-repl: ...`); do not send JSON (`{\"code\":...}`), quoted code, or markdown fences."
                .to_string(),
        )),
        _ => Ok(()),
    }
}

fn is_quoted_source(input: &str) -> bool {
    input.len() >= 2
        && ((input.starts_with('"') && input.ends_with('"'))
            || (input.starts_with('\'') && input.ends_with('\'')))
}

#[cfg(test)]
mod tests {
    use super::parse_freeform_args;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_freeform_args_without_pragma() {
        let args = parse_freeform_args("Write-Output 'ok'").expect("parse args");
        assert_eq!(args.code, "Write-Output 'ok'");
        assert_eq!(args.timeout_ms, None);
    }

    #[test]
    fn parse_freeform_args_with_pragma() {
        let input = "# codex-ps-repl: timeout_ms=15000\nWrite-Output 'ok'";
        let args = parse_freeform_args(input).expect("parse args");
        assert_eq!(args.code, "Write-Output 'ok'");
        assert_eq!(args.timeout_ms, Some(15_000));
    }

    #[test]
    fn parse_freeform_args_rejects_unknown_key() {
        let err = parse_freeform_args("# codex-ps-repl: nope=1\nWrite-Output 'ok'")
            .expect_err("expected error");
        assert_eq!(
            err.to_string(),
            "ps_repl pragma only supports timeout_ms; got `nope`"
        );
    }

    #[test]
    fn parse_freeform_args_rejects_json_wrapped_code() {
        let err =
            parse_freeform_args(r#"{"code":"Write-Output 'ok'"}"#).expect_err("expected error");
        assert_eq!(
            err.to_string(),
            "ps_repl is a freeform tool and expects raw PowerShell source. Resend plain PowerShell only (optional first line `# codex-ps-repl: ...`); do not send JSON (`{\"code\":...}`), quoted code, or markdown fences."
        );
    }

    #[test]
    fn parse_freeform_args_rejects_quoted_source() {
        let err = parse_freeform_args("'Write-Output ok'").expect_err("expected error");
        assert_eq!(
            err.to_string(),
            "ps_repl is a freeform tool and expects raw PowerShell source. Resend plain PowerShell only (optional first line `# codex-ps-repl: ...`); do not send quoted code or markdown fences."
        );
    }
}
