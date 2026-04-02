use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::AnyToolResult;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::request_user_input::RequestUserInputArgs;
use codex_tools::REQUEST_USER_INPUT_TOOL_NAME;
use codex_tools::normalize_request_user_input_args;
use codex_tools::request_user_input_unavailable_message;
use futures::future::BoxFuture;

pub struct RequestUserInputHandler {
    pub default_mode_request_user_input: bool,
}

impl ToolHandler for RequestUserInputHandler {
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
                payload,
                ..
            } = invocation;

            let payload_for_result = payload.clone();
            let arguments = match payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "{REQUEST_USER_INPUT_TOOL_NAME} handler received unsupported payload"
                    )));
                }
            };

            let mode = session.collaboration_mode().await.mode;
            if let Some(message) =
                request_user_input_unavailable_message(mode, self.default_mode_request_user_input)
            {
                return Err(FunctionCallError::RespondToModel(message));
            }

            let args: RequestUserInputArgs = parse_arguments(&arguments)?;
            let args = normalize_request_user_input_args(args)
                .map_err(FunctionCallError::RespondToModel)?;
            let response = session
                .request_user_input(turn.as_ref(), call_id.clone(), args)
                .await
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(format!(
                        "{REQUEST_USER_INPUT_TOOL_NAME} was cancelled before receiving a response"
                    ))
                })?;

            let content = serde_json::to_string(&response).map_err(|err| {
                FunctionCallError::Fatal(format!(
                    "failed to serialize {REQUEST_USER_INPUT_TOOL_NAME} response: {err}"
                ))
            })?;

            Ok(AnyToolResult {
                call_id,
                payload: payload_for_result,
                result: Box::new(FunctionToolOutput::from_text(content, Some(true))),
            })
        })
    }
}
