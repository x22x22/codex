use super::*;
use crate::agent::control::ListedAgent;

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
            let ToolInvocation {
                session,
                turn,
                payload,
                call_id,
                ..
            } = invocation;
            let payload_for_result = payload.clone();
            let arguments = function_arguments(payload)?;
            let args: ListAgentsArgs = parse_arguments(&arguments)?;
            session
                .services
                .agent_control
                .register_session_root(session.conversation_id, &turn.session_source);
            let agents = session
                .services
                .agent_control
                .list_agents(&turn.session_source, args.path_prefix.as_deref())
                .await
                .map_err(collab_spawn_error)?;

            Ok(AnyToolResult {
                call_id,
                payload: payload_for_result,
                result: Box::new(ListAgentsResult { agents }),
            })
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAgentsArgs {
    path_prefix: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListAgentsResult {
    agents: Vec<ListedAgent>,
}

impl ToolOutput for ListAgentsResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "list_agents")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "list_agents")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "list_agents")
    }
}
