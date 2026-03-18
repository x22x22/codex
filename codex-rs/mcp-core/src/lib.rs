mod config;
mod connection_manager;
mod tool_call;

pub mod auth;
pub mod tool_approval_templates;

use std::collections::HashMap;

use codex_protocol::mcp::Tool;

pub use config::McpServerConfig;
pub use config::McpServerTransportConfig;
pub use connection_manager::CodexAppsToolsCacheKey;
pub use connection_manager::MCP_SANDBOX_STATE_CAPABILITY;
pub use connection_manager::MCP_SANDBOX_STATE_METHOD;
pub use connection_manager::McpConnectionManager;
pub use connection_manager::SandboxState;
pub use connection_manager::ToolInfo;
pub use connection_manager::codex_apps_tools_cache_key_from_token_data;
pub use connection_manager::filter_non_codex_apps_mcp_tools_only;
pub use tool_call::MCP_TOOL_APPROVAL_ACCEPT;
pub use tool_call::MCP_TOOL_APPROVAL_ACCEPT_FOR_SESSION;
pub use tool_call::MCP_TOOL_APPROVAL_DECLINE_SYNTHETIC;
pub use tool_call::MCP_TOOL_APPROVAL_QUESTION_ID_PREFIX;
pub use tool_call::is_mcp_tool_approval_question_id;

const MCP_TOOL_NAME_PREFIX: &str = "mcp";
const MCP_TOOL_NAME_DELIMITER: &str = "__";
pub const CODEX_APPS_MCP_SERVER_NAME: &str = "codex_apps";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPluginProvenance {
    pub plugin_display_names_by_connector_id: HashMap<String, Vec<String>>,
    pub plugin_display_names_by_mcp_server_name: HashMap<String, Vec<String>>,
}

impl ToolPluginProvenance {
    pub fn plugin_display_names_for_connector_id(&self, connector_id: &str) -> &[String] {
        self.plugin_display_names_by_connector_id
            .get(connector_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn plugin_display_names_for_mcp_server_name(&self, server_name: &str) -> &[String] {
        self.plugin_display_names_by_mcp_server_name
            .get(server_name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn record_connector_plugin_name(
        &mut self,
        connector_id: impl Into<String>,
        plugin_display_name: impl Into<String>,
    ) {
        self.plugin_display_names_by_connector_id
            .entry(connector_id.into())
            .or_default()
            .push(plugin_display_name.into());
    }

    pub fn record_server_plugin_name(
        &mut self,
        server_name: impl Into<String>,
        plugin_display_name: impl Into<String>,
    ) {
        self.plugin_display_names_by_mcp_server_name
            .entry(server_name.into())
            .or_default()
            .push(plugin_display_name.into());
    }

    pub fn sort_and_dedup(&mut self) {
        for plugin_names in self
            .plugin_display_names_by_connector_id
            .values_mut()
            .chain(self.plugin_display_names_by_mcp_server_name.values_mut())
        {
            plugin_names.sort_unstable();
            plugin_names.dedup();
        }
    }
}

pub fn split_qualified_tool_name(qualified_name: &str) -> Option<(String, String)> {
    let mut parts = qualified_name.split(MCP_TOOL_NAME_DELIMITER);
    let prefix = parts.next()?;
    if prefix != MCP_TOOL_NAME_PREFIX {
        return None;
    }
    let server_name = parts.next()?;
    let tool_name = parts.collect::<Vec<_>>().join(MCP_TOOL_NAME_DELIMITER);
    if tool_name.is_empty() {
        return None;
    }
    Some((server_name.to_string(), tool_name))
}

pub fn group_tools_by_server(
    tools: &HashMap<String, Tool>,
) -> HashMap<String, HashMap<String, Tool>> {
    let mut grouped = HashMap::new();
    for (qualified_name, tool) in tools {
        if let Some((server_name, tool_name)) = split_qualified_tool_name(qualified_name) {
            grouped
                .entry(server_name)
                .or_insert_with(HashMap::new)
                .insert(tool_name, tool.clone());
        }
    }
    grouped
}
