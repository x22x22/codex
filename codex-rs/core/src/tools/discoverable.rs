use crate::plugins::PluginDetailSummary;
use codex_app_server_protocol::AppInfo;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiscoverableToolType {
    Connector,
    Plugin,
}

impl DiscoverableToolType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Connector => "connector",
            Self::Plugin => "plugin",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiscoverableToolAction {
    Install,
    Enable,
}

impl DiscoverableToolAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Enable => "enable",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DiscoverableTool {
    Connector(Box<AppInfo>),
    Plugin(Box<DiscoverablePluginInfo>),
}

impl DiscoverableTool {
    pub(crate) fn tool_type(&self) -> DiscoverableToolType {
        match self {
            Self::Connector(_) => DiscoverableToolType::Connector,
            Self::Plugin(_) => DiscoverableToolType::Plugin,
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Connector(connector) => connector.id.as_str(),
            Self::Plugin(plugin) => plugin.id.as_str(),
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Connector(connector) => connector.name.as_str(),
            Self::Plugin(plugin) => plugin.name.as_str(),
        }
    }

    pub(crate) fn action(&self) -> DiscoverableToolAction {
        match self {
            Self::Connector(connector) => {
                if connector.is_accessible && !connector.is_enabled {
                    DiscoverableToolAction::Enable
                } else {
                    DiscoverableToolAction::Install
                }
            }
            Self::Plugin(plugin) => {
                if plugin.installed && !plugin.enabled {
                    DiscoverableToolAction::Enable
                } else {
                    DiscoverableToolAction::Install
                }
            }
        }
    }
}

impl From<AppInfo> for DiscoverableTool {
    fn from(value: AppInfo) -> Self {
        Self::Connector(Box::new(value))
    }
}

impl From<DiscoverablePluginInfo> for DiscoverableTool {
    fn from(value: DiscoverablePluginInfo) -> Self {
        Self::Plugin(Box::new(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiscoverablePluginInfo {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) has_skills: bool,
    pub(crate) mcp_server_names: Vec<String>,
    pub(crate) app_connector_ids: Vec<String>,
    pub(crate) marketplace_path: AbsolutePathBuf,
    pub(crate) plugin_name: String,
    pub(crate) installed: bool,
    pub(crate) enabled: bool,
}

impl DiscoverablePluginInfo {
    pub(crate) fn from_plugin_detail(
        marketplace_path: AbsolutePathBuf,
        plugin: PluginDetailSummary,
    ) -> Self {
        let display_name = plugin
            .interface
            .as_ref()
            .and_then(|interface| interface.display_name.clone())
            .unwrap_or_else(|| plugin.name.clone());
        let description = plugin.description.clone().or_else(|| {
            plugin
                .interface
                .as_ref()
                .and_then(|interface| interface.short_description.clone())
        });

        Self {
            id: plugin.id,
            name: display_name,
            description,
            has_skills: !plugin.skills.is_empty(),
            mcp_server_names: plugin.mcp_server_names,
            app_connector_ids: plugin
                .apps
                .into_iter()
                .map(|connector_id| connector_id.0)
                .collect(),
            marketplace_path,
            plugin_name: plugin.name,
            installed: plugin.installed,
            enabled: plugin.enabled,
        }
    }
}
