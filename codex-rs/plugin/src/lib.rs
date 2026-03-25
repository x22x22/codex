//! Shared plugin identifiers and telemetry-facing summaries.

pub mod mention_syntax;

mod plugin_id;
mod plugin_namespace;

pub use plugin_id::PluginId;
pub use plugin_id::PluginIdError;
pub use plugin_id::validate_plugin_segment;
pub use plugin_namespace::PLUGIN_MANIFEST_PATH;
pub use plugin_namespace::plugin_namespace_for_skill_path;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppConnectorId(pub String);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginCapabilitySummary {
    pub config_name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub has_skills: bool,
    pub mcp_server_names: Vec<String>,
    pub app_connector_ids: Vec<AppConnectorId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTelemetryMetadata {
    pub plugin_id: PluginId,
    pub capability_summary: Option<PluginCapabilitySummary>,
}

impl PluginTelemetryMetadata {
    pub fn from_plugin_id(plugin_id: &PluginId) -> Self {
        Self {
            plugin_id: plugin_id.clone(),
            capability_summary: None,
        }
    }
}

impl PluginCapabilitySummary {
    pub fn telemetry_metadata(&self) -> Option<PluginTelemetryMetadata> {
        PluginId::parse(&self.config_name)
            .ok()
            .map(|plugin_id| PluginTelemetryMetadata {
                plugin_id,
                capability_summary: Some(self.clone()),
            })
    }
}
