use futures::future::BoxFuture;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::AnyToolResult;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

use super::DEFAULT_WAIT_YIELD_TIME_MS;
use super::ExecContext;
use super::WAIT_TOOL_NAME;
use super::handle_runtime_response;

pub struct CodeModeWaitHandler;

#[derive(Debug, Deserialize)]
struct ExecWaitArgs {
    cell_id: String,
    #[serde(default = "default_wait_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    terminate: bool,
}

fn default_wait_yield_time_ms() -> u64 {
    DEFAULT_WAIT_YIELD_TIME_MS
}

fn parse_arguments<T>(arguments: &str) -> Result<T, FunctionCallError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
    })
}

impl ToolHandler for CodeModeWaitHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> BoxFuture<'_, Result<AnyToolResult, FunctionCallError>> {
        Box::pin(async move {
            let ToolInvocation {
                session,
                turn,
                call_id,
                tool_name,
                payload,
                ..
            } = invocation;
            let payload_for_result = payload.clone();

            let result = match payload {
                ToolPayload::Function { arguments } if tool_name == WAIT_TOOL_NAME => {
                    let args: ExecWaitArgs = parse_arguments(&arguments)?;
                    let exec = ExecContext { session, turn };
                    let started_at = std::time::Instant::now();
                    let response = exec
                        .session
                        .services
                        .code_mode_service
                        .wait(codex_code_mode::WaitRequest {
                            cell_id: args.cell_id,
                            yield_time_ms: args.yield_time_ms,
                            terminate: args.terminate,
                        })
                        .await
                        .map_err(FunctionCallError::RespondToModel)?;
                    handle_runtime_response(&exec, response, args.max_tokens, started_at)
                        .await
                        .map_err(FunctionCallError::RespondToModel)
                }
                _ => Err(FunctionCallError::RespondToModel(format!(
                    "{WAIT_TOOL_NAME} expects JSON arguments"
                ))),
            }?;

            Ok(AnyToolResult {
                call_id,
                payload: payload_for_result,
                result: Box::new(result),
            })
        })
    }
}
