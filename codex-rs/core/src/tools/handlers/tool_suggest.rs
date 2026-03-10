use std::collections::BTreeMap;

use async_trait::async_trait;
use codex_app_server_protocol::McpElicitationObjectType;
use codex_app_server_protocol::McpElicitationSchema;
use codex_app_server_protocol::McpServerElicitationRequest;
use codex_app_server_protocol::McpServerElicitationRequestParams;
use codex_rmcp_client::ElicitationAction;
use codex_rmcp_client::ElicitationResponse;
use rmcp::model::RequestId;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::connectors;
use crate::function_tool::FunctionCallError;
use crate::mcp::CODEX_APPS_MCP_SERVER_NAME;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ToolSuggestHandler;

pub(crate) const TOOL_SUGGEST_TOOL_NAME: &str = "tool_suggest";
const TOOL_SUGGEST_DECISION_INSTALL: &str = "install";
const TOOL_SUGGEST_DECISION_NOT_NOW: &str = "not_now";
const TOOL_SUGGEST_META_KIND_KEY: &str = "codex_approval_kind";
const TOOL_SUGGEST_META_KIND_VALUE: &str = "app_install_suggestion";

#[derive(Deserialize)]
struct ToolSuggestArgs {
    connector_id: String,
}

#[derive(Deserialize)]
struct ToolSuggestElicitationContent {
    decision: String,
}

fn tool_suggest_message(
    connector_name: &str,
    connector_description: Option<&str>,
    install_url: &str,
) -> String {
    let mut parts = vec![format!("Install {connector_name} to continue?")];
    if let Some(description) = connector_description.map(str::trim)
        && !description.is_empty()
    {
        parts.push(description.to_string());
    }
    parts.push(format!("Install URL: {install_url}"));
    parts.join(" | ")
}

fn tool_suggest_requested_schema() -> McpElicitationSchema {
    McpElicitationSchema {
        schema_uri: None,
        type_: McpElicitationObjectType::Object,
        properties: BTreeMap::new(),
        required: None,
    }
}

fn tool_suggest_elicitation_meta(
    connector_id: &str,
    connector_name: &str,
    connector_description: Option<&str>,
    install_url: &str,
) -> Value {
    json!({
        TOOL_SUGGEST_META_KIND_KEY: TOOL_SUGGEST_META_KIND_VALUE,
        "connector_id": connector_id,
        "connector_name": connector_name,
        "connector_description": connector_description,
        "install_url": install_url,
    })
}

fn parse_tool_suggest_elicitation_response(
    response: ElicitationResponse,
) -> (&'static str, String) {
    let elicitation_action = match response.action {
        ElicitationAction::Accept => "accept",
        ElicitationAction::Decline => "decline",
        ElicitationAction::Cancel => "cancel",
    };

    let user_decision = match response.action {
        ElicitationAction::Accept => response
            .content
            .and_then(|content| {
                serde_json::from_value::<ToolSuggestElicitationContent>(content).ok()
            })
            .map(|content| content.decision)
            .filter(|decision| {
                matches!(
                    decision.as_str(),
                    TOOL_SUGGEST_DECISION_INSTALL | TOOL_SUGGEST_DECISION_NOT_NOW
                )
            })
            .unwrap_or_else(|| TOOL_SUGGEST_DECISION_INSTALL.to_string()),
        ElicitationAction::Decline | ElicitationAction::Cancel => {
            TOOL_SUGGEST_DECISION_NOT_NOW.to_string()
        }
    };

    (elicitation_action, user_decision)
}

#[async_trait]
impl ToolHandler for ToolSuggestHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            payload,
            turn,
            call_id,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::Fatal(format!(
                    "{TOOL_SUGGEST_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        let args: ToolSuggestArgs = parse_arguments(&arguments)?;
        let connector_id = args.connector_id.trim();
        if connector_id.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "connector_id must not be empty".to_string(),
            ));
        }

        let connectors = match connectors::list_cached_connectors(&turn.config).await {
            Some(connectors) => connectors,
            None => connectors::list_connectors(&turn.config)
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to load installable apps: {err}"
                    ))
                })?,
        };

        let connector = connectors
            .into_iter()
            .find(|connector| connector.id == connector_id)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!("unknown connector_id `{connector_id}`"))
            })?;

        if connector.is_accessible {
            return Err(FunctionCallError::RespondToModel(format!(
                "connector_id `{connector_id}` is already available; use available tools or search_tool_bm25 with mode `available` instead"
            )));
        }

        let install_url = connector
            .install_url
            .clone()
            .unwrap_or_else(|| connectors::connector_install_url(&connector.name, &connector.id));
        let request_id = RequestId::String(format!("{TOOL_SUGGEST_TOOL_NAME}_{call_id}").into());
        let elicitation_response = session
            .request_mcp_server_elicitation(
                turn.as_ref(),
                request_id,
                McpServerElicitationRequestParams {
                    thread_id: session.conversation_id.to_string(),
                    turn_id: Some(turn.sub_id.clone()),
                    server_name: CODEX_APPS_MCP_SERVER_NAME.to_string(),
                    request: McpServerElicitationRequest::Form {
                        meta: Some(tool_suggest_elicitation_meta(
                            &connector.id,
                            &connector.name,
                            connector.description.as_deref(),
                            &install_url,
                        )),
                        message: tool_suggest_message(
                            &connector.name,
                            connector.description.as_deref(),
                            &install_url,
                        ),
                        requested_schema: tool_suggest_requested_schema(),
                    },
                },
            )
            .await
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "tool_suggest was cancelled before receiving a response".to_string(),
                )
            })?;
        let (elicitation_action, user_decision) =
            parse_tool_suggest_elicitation_response(elicitation_response);
        let assistant_instruction = if user_decision == TOOL_SUGGEST_DECISION_INSTALL {
            "The user confirmed they installed this app. Treat it as installed for this request, but verify its tools appear in an available-tool search before trying to use them."
        } else {
            "The user did not install this app in this flow, either because they chose not to or did not finish setup. Do not try to use its tools in this turn."
        };
        let content = json!({
            "connector_id": connector.id,
            "connector_name": connector.name,
            "connector_description": connector.description,
            "install_url": install_url,
            "elicitation_action": elicitation_action,
            "user_decision": user_decision,
            "assistant_instruction": assistant_instruction,
        })
        .to_string();

        Ok(FunctionToolOutput::from_text(content, Some(true)))
    }
}

#[cfg(test)]
mod tests {
    use codex_rmcp_client::ElicitationAction;
    use codex_rmcp_client::ElicitationResponse;
    use pretty_assertions::assert_eq;

    use super::TOOL_SUGGEST_DECISION_INSTALL;
    use super::TOOL_SUGGEST_DECISION_NOT_NOW;
    use super::parse_tool_suggest_elicitation_response;
    use super::tool_suggest_message;

    #[test]
    fn tool_suggest_message_uses_single_line_text() {
        let message = tool_suggest_message(
            "Docs & Notes",
            Some("Install <now> & sync"),
            "https://example.com/apps?name=Docs",
        );

        assert_eq!(
            message,
            "Install Docs & Notes to continue? | Install <now> & sync | Install URL: https://example.com/apps?name=Docs"
        );
    }

    #[test]
    fn accepted_tool_suggest_defaults_to_install_without_form_content() {
        let (elicitation_action, user_decision) =
            parse_tool_suggest_elicitation_response(ElicitationResponse {
                action: ElicitationAction::Accept,
                content: None,
                meta: None,
            });

        assert_eq!(elicitation_action, "accept");
        assert_eq!(user_decision, TOOL_SUGGEST_DECISION_INSTALL);
    }

    #[test]
    fn declined_tool_suggest_maps_to_not_now() {
        let (elicitation_action, user_decision) =
            parse_tool_suggest_elicitation_response(ElicitationResponse {
                action: ElicitationAction::Decline,
                content: None,
                meta: None,
            });

        assert_eq!(elicitation_action, "decline");
        assert_eq!(user_decision, TOOL_SUGGEST_DECISION_NOT_NOW);
    }

    #[test]
    fn cancelled_tool_suggest_maps_to_not_now() {
        let (elicitation_action, user_decision) =
            parse_tool_suggest_elicitation_response(ElicitationResponse {
                action: ElicitationAction::Cancel,
                content: None,
                meta: None,
            });

        assert_eq!(elicitation_action, "cancel");
        assert_eq!(user_decision, TOOL_SUGGEST_DECISION_NOT_NOW);
    }
}
