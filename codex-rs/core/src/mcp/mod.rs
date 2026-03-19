pub mod auth;
pub mod config;
pub mod manager;
mod skill_dependencies;
pub mod snapshot;
pub mod types;

pub(crate) use config::with_codex_apps_mcp;
pub(crate) use manager::McpManager;
pub(crate) use skill_dependencies::maybe_prompt_and_install_mcp_dependencies;
pub(crate) use snapshot::collect_mcp_snapshot_from_manager;
pub(crate) use types::split_qualified_tool_name;

use std::collections::HashMap;

use crate::plugins::PluginCapabilitySummary;

pub(crate) const CODEX_APPS_MCP_SERVER_NAME: &str = "codex_apps";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPluginProvenance {
    plugin_display_names_by_connector_id: HashMap<String, Vec<String>>,
    plugin_display_names_by_mcp_server_name: HashMap<String, Vec<String>>,
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

    pub(crate) fn from_capability_summaries(
        capability_summaries: &[PluginCapabilitySummary],
    ) -> Self {
        let mut tool_plugin_provenance = Self::default();
        for plugin in capability_summaries {
            for connector_id in &plugin.app_connector_ids {
                tool_plugin_provenance
                    .plugin_display_names_by_connector_id
                    .entry(connector_id.0.clone())
                    .or_default()
                    .push(plugin.display_name.clone());
            }

            for server_name in &plugin.mcp_server_names {
                tool_plugin_provenance
                    .plugin_display_names_by_mcp_server_name
                    .entry(server_name.clone())
                    .or_default()
                    .push(plugin.display_name.clone());
            }
        }

        for plugin_names in tool_plugin_provenance
            .plugin_display_names_by_connector_id
            .values_mut()
            .chain(
                tool_plugin_provenance
                    .plugin_display_names_by_mcp_server_name
                    .values_mut(),
            )
        {
            plugin_names.sort_unstable();
            plugin_names.dedup();
        }

        tool_plugin_provenance
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
