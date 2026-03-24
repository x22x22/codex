use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::connectors;
use crate::mcp::CODEX_APPS_MCP_SERVER_NAME;
use crate::mcp_connection_manager::ToolInfo;
use crate::plugins::PluginCapabilitySummary;
use crate::plugins::render_explicit_plugin_instructions;

pub(crate) fn build_plugin_developer_sections(
    mentioned_plugins: &[PluginCapabilitySummary],
    mcp_tools: &HashMap<String, ToolInfo>,
    available_connectors: &[connectors::AppInfo],
) -> Vec<String> {
    if mentioned_plugins.is_empty() {
        return Vec::new();
    }

    // Turn each explicit plugin mention into developer-message sections that
    // can be folded into the canonical pre-user developer envelope for this turn.
    mentioned_plugins
        .iter()
        .filter_map(|plugin| {
            let available_mcp_servers = mcp_tools
                .values()
                .filter(|tool| {
                    tool.server_name != CODEX_APPS_MCP_SERVER_NAME
                        && tool
                            .plugin_display_names
                            .iter()
                            .any(|plugin_name| plugin_name == &plugin.display_name)
                })
                .map(|tool| tool.server_name.clone())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect::<Vec<_>>();
            let available_apps = available_connectors
                .iter()
                .filter(|connector| {
                    connector.is_enabled
                        && connector
                            .plugin_display_names
                            .iter()
                            .any(|plugin_name| plugin_name == &plugin.display_name)
                })
                .map(connectors::connector_display_label)
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect::<Vec<_>>();
            render_explicit_plugin_instructions(plugin, &available_mcp_servers, &available_apps)
        })
        .collect()
}
