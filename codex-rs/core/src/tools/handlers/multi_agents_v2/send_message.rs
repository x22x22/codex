use super::message_tool::MessageDeliveryMode;
use super::message_tool::SendMessageArgs;
use super::message_tool::handle_message_string_tool;
use super::*;

pub(crate) struct Handler;

impl ToolHandler for Handler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> BoxFuture<'_, Result<AnyToolResult, FunctionCallError>> {
        Box::pin(async move {
            let call_id = invocation.call_id.clone();
            let payload_for_result = invocation.payload.clone();
            let arguments = function_arguments(invocation.payload.clone())?;
            let args: SendMessageArgs = parse_arguments(&arguments)?;
            let result = handle_message_string_tool(
                invocation,
                MessageDeliveryMode::QueueOnly,
                args.target,
                args.message,
                /*interrupt*/ false,
            )
            .await?;

            Ok(AnyToolResult {
                call_id,
                payload: payload_for_result,
                result: Box::new(result),
            })
        })
    }
}
