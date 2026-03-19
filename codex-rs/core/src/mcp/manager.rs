use std::collections::HashMap;
use std::sync::Arc;

use crate::CodexAuth;
use crate::config::Config;
use crate::config::types::McpServerConfig;
use crate::plugins::PluginsManager;

use crate::mcp::ToolPluginProvenance;

pub struct McpManager {
    plugins_manager: Arc<PluginsManager>,
}

impl McpManager {
    pub fn new(plugins_manager: Arc<PluginsManager>) -> Self {
        Self { plugins_manager }
    }

    pub fn configured_servers(&self, config: &Config) -> HashMap<String, McpServerConfig> {
        configured_mcp_servers(config, self.plugins_manager.as_ref())
    }

    pub fn effective_servers(
        &self,
        config: &Config,
        auth: Option<&CodexAuth>,
    ) -> HashMap<String, McpServerConfig> {
        effective_mcp_servers(config, auth, self.plugins_manager.as_ref())
    }

    pub fn tool_plugin_provenance(&self, config: &Config) -> ToolPluginProvenance {
        let loaded_plugins = self.plugins_manager.plugins_for_config(config);
        ToolPluginProvenance::from_capability_summaries(loaded_plugins.capability_summaries())
    }
}

fn configured_mcp_servers(
    config: &Config,
    plugins_manager: &PluginsManager,
) -> HashMap<String, McpServerConfig> {
    let loaded_plugins = plugins_manager.plugins_for_config(config);
    let mut servers = config.mcp_servers.get().clone();
    for (name, plugin_server) in loaded_plugins.effective_mcp_servers() {
        servers.entry(name).or_insert(plugin_server);
    }
    servers
}

fn effective_mcp_servers(
    config: &Config,
    auth: Option<&CodexAuth>,
    plugins_manager: &PluginsManager,
) -> HashMap<String, McpServerConfig> {
    let servers = configured_mcp_servers(config, plugins_manager);
    crate::mcp::config::with_codex_apps_mcp(
        servers,
        config.features.apps_enabled_for_auth(auth),
        auth,
        config,
    )
}
