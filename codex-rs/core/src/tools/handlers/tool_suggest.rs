use std::collections::BTreeMap;
use std::collections::HashSet;

use async_trait::async_trait;
use codex_app_server_protocol::AppInfo;
use codex_app_server_protocol::McpElicitationObjectType;
use codex_app_server_protocol::McpElicitationSchema;
use codex_app_server_protocol::McpServerElicitationRequest;
use codex_app_server_protocol::McpServerElicitationRequestParams;
use codex_rmcp_client::ElicitationAction;
use codex_utils_absolute_path::AbsolutePathBuf;
use rmcp::model::RequestId;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use tracing::warn;

use crate::CodexAuth;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::connectors;
use crate::function_tool::FunctionCallError;
use crate::mcp::CODEX_APPS_MCP_SERVER_NAME;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::discoverable::DiscoverableTool;
use crate::tools::discoverable::DiscoverableToolAction;
use crate::tools::discoverable::DiscoverableToolType;
use crate::tools::discoverable_catalog;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ToolSuggestHandler;

pub(crate) const TOOL_SUGGEST_TOOL_NAME: &str = "tool_suggest";
const TOOL_SUGGEST_APPROVAL_KIND_VALUE: &str = "tool_suggestion";

#[derive(Debug, Deserialize)]
struct ToolSuggestArgs {
    tool_type: DiscoverableToolType,
    action_type: DiscoverableToolAction,
    tool_id: String,
    suggest_reason: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ToolSuggestResult {
    completed: bool,
    user_confirmed: bool,
    tool_type: DiscoverableToolType,
    action_type: DiscoverableToolAction,
    tool_id: String,
    tool_name: String,
    suggest_reason: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ToolSuggestMeta {
    codex_approval_kind: &'static str,
    suggest_type: DiscoverableToolAction,
    suggest_reason: String,
    tool_id: String,
    tool_name: String,
    #[serde(flatten)]
    tool_metadata: ToolSuggestMetaToolMetadata,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "tool_type", rename_all = "snake_case")]
enum ToolSuggestMetaToolMetadata {
    Connector {
        install_url: String,
    },
    Plugin {
        marketplace_path: AbsolutePathBuf,
        plugin_name: String,
    },
}

#[async_trait]
impl ToolHandler for ToolSuggestHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            payload,
            session,
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
        let suggest_reason = args.suggest_reason.trim();
        if suggest_reason.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "suggest_reason must not be empty".to_string(),
            ));
        }

        let auth = session.services.auth_manager.auth().await;
        let manager = session.services.mcp_connection_manager.read().await;
        let mcp_tools = manager.list_all_tools().await;
        drop(manager);
        let accessible_connectors = connectors::with_app_enabled_state(
            connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
            &turn.config,
        );
        let loaded_plugins = session
            .services
            .plugins_manager
            .plugins_for_config(&turn.config);
        let discoverable_tools = discoverable_catalog::load_discoverable_tools(
            &turn.config,
            auth.as_ref(),
            session.services.plugins_manager.as_ref(),
            &loaded_plugins,
            turn.cwd.as_path(),
            &accessible_connectors,
        )
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "tool suggestions are unavailable right now: {err}"
            ))
        })?;

        let tool = discoverable_tools
            .into_iter()
            .find(|tool| tool.id() == args.tool_id)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "tool_id must match one of the discoverable tools exposed by {TOOL_SUGGEST_TOOL_NAME}"
                ))
            })?;
        if args.tool_type != tool.tool_type() {
            return Err(FunctionCallError::RespondToModel(format!(
                "tool_type must match the discoverable tool entry for tool_id `{}`",
                args.tool_id
            )));
        }
        if args.action_type != tool.action() {
            return Err(FunctionCallError::RespondToModel(format!(
                "action_type must match the discoverable tool entry for tool_id `{}`",
                args.tool_id
            )));
        }

        let request_id = RequestId::String(format!("tool_suggestion_{call_id}").into());
        let params = build_tool_suggestion_elicitation_request(
            session.conversation_id.to_string(),
            turn.sub_id.clone(),
            &args,
            suggest_reason,
            &tool,
        );
        let response = session
            .request_mcp_server_elicitation(turn.as_ref(), request_id, params)
            .await;
        let user_confirmed = response
            .as_ref()
            .is_some_and(|response| response.action == ElicitationAction::Accept);

        let completed = if user_confirmed {
            match &tool {
                DiscoverableTool::Connector(connector) => {
                    refresh_and_verify_connector_suggestion_completed(
                        session.as_ref(),
                        turn.as_ref(),
                        auth.as_ref(),
                        args.action_type,
                        connector.id.as_str(),
                    )
                    .await
                }
                DiscoverableTool::Plugin(plugin) => {
                    refresh_and_verify_plugin_suggestion_completed(
                        session.as_ref(),
                        turn.as_ref(),
                        plugin.id.as_str(),
                    )
                    .await
                }
            }
        } else {
            false
        };

        if completed && let DiscoverableTool::Connector(connector) = &tool {
            session
                .merge_connector_selection(HashSet::from([connector.id.clone()]))
                .await;
        }

        let content = serde_json::to_string(&ToolSuggestResult {
            completed,
            user_confirmed,
            tool_type: args.tool_type,
            action_type: args.action_type,
            tool_id: tool.id().to_string(),
            tool_name: tool.name().to_string(),
            suggest_reason: suggest_reason.to_string(),
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to serialize {TOOL_SUGGEST_TOOL_NAME} response: {err}"
            ))
        })?;

        Ok(FunctionToolOutput::from_text(content, Some(true)))
    }
}

fn build_tool_suggestion_elicitation_request(
    thread_id: String,
    turn_id: String,
    args: &ToolSuggestArgs,
    suggest_reason: &str,
    tool: &DiscoverableTool,
) -> McpServerElicitationRequestParams {
    let tool_name = tool.name().to_string();
    let message = match tool {
        DiscoverableTool::Connector(_) => format!(
            "{tool_name} could help with this request.\n\n{suggest_reason}\n\nOpen ChatGPT to {} it, then confirm here if you finish.",
            args.action_type.as_str()
        ),
        DiscoverableTool::Plugin(_) => format!(
            "{tool_name} could help with this request.\n\n{suggest_reason}\n\nUse Codex to {} it, then confirm here if you finish.",
            args.action_type.as_str()
        ),
    };

    McpServerElicitationRequestParams {
        thread_id,
        turn_id: Some(turn_id),
        server_name: CODEX_APPS_MCP_SERVER_NAME.to_string(),
        request: McpServerElicitationRequest::Form {
            meta: Some(json!(build_tool_suggestion_meta(
                args.action_type,
                suggest_reason,
                tool,
            ))),
            message,
            requested_schema: McpElicitationSchema {
                schema_uri: None,
                type_: McpElicitationObjectType::Object,
                properties: BTreeMap::new(),
                required: None,
            },
        },
    }
}

fn build_tool_suggestion_meta(
    action_type: DiscoverableToolAction,
    suggest_reason: &str,
    tool: &DiscoverableTool,
) -> ToolSuggestMeta {
    ToolSuggestMeta {
        codex_approval_kind: TOOL_SUGGEST_APPROVAL_KIND_VALUE,
        suggest_type: action_type,
        suggest_reason: suggest_reason.to_string(),
        tool_id: tool.id().to_string(),
        tool_name: tool.name().to_string(),
        tool_metadata: match tool {
            DiscoverableTool::Connector(connector) => ToolSuggestMetaToolMetadata::Connector {
                install_url: connector_install_url(connector.as_ref()),
            },
            DiscoverableTool::Plugin(plugin) => ToolSuggestMetaToolMetadata::Plugin {
                marketplace_path: plugin.marketplace_path.clone(),
                plugin_name: plugin.plugin_name.clone(),
            },
        },
    }
}

async fn refresh_and_verify_connector_suggestion_completed(
    session: &Session,
    turn: &TurnContext,
    auth: Option<&CodexAuth>,
    action_type: DiscoverableToolAction,
    tool_id: &str,
) -> bool {
    let manager = session.services.mcp_connection_manager.read().await;
    match manager.hard_refresh_codex_apps_tools_cache().await {
        Ok(mcp_tools) => {
            let accessible_connectors = connectors::with_app_enabled_state(
                connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
                &turn.config,
            );
            connectors::refresh_accessible_connectors_cache_from_mcp_tools(
                &turn.config,
                auth,
                &mcp_tools,
            );
            verified_connector_suggestion_completed(action_type, tool_id, &accessible_connectors)
        }
        Err(err) => {
            warn!(
                "failed to refresh codex apps tools cache after tool suggestion for {tool_id}: {err:#}"
            );
            false
        }
    }
}

fn verified_connector_suggestion_completed(
    action_type: DiscoverableToolAction,
    tool_id: &str,
    accessible_connectors: &[AppInfo],
) -> bool {
    accessible_connectors
        .iter()
        .find(|connector| connector.id == tool_id)
        .is_some_and(|connector| match action_type {
            DiscoverableToolAction::Install => connector.is_accessible,
            DiscoverableToolAction::Enable => connector.is_accessible && connector.is_enabled,
        })
}

async fn refresh_and_verify_plugin_suggestion_completed(
    session: &Session,
    turn: &TurnContext,
    tool_id: &str,
) -> bool {
    let auth = session.services.auth_manager.auth().await;
    session.reload_user_config_layer().await;
    let refreshed_turn = session
        .new_default_turn_with_sub_id(turn.sub_id.clone())
        .await;
    let refresh_servers = session
        .services
        .mcp_manager
        .effective_servers(&refreshed_turn.config, auth.as_ref());
    session
        .refresh_mcp_servers_now(
            refreshed_turn.as_ref(),
            refresh_servers,
            refreshed_turn.config.mcp_oauth_credentials_store_mode,
        )
        .await;
    discoverable_catalog::plugin_completion_verified(
        session.services.plugins_manager.as_ref(),
        &refreshed_turn.config,
        refreshed_turn.cwd.as_path(),
        tool_id,
    )
}

fn connector_install_url(connector: &AppInfo) -> String {
    connector
        .install_url
        .clone()
        .unwrap_or_else(|| connectors::connector_install_url(&connector.name, &connector.id))
}

#[cfg(test)]
#[path = "tool_suggest_tests.rs"]
mod tests;
