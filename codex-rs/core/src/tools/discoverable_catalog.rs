use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use codex_app_server_protocol::AppInfo;
use codex_utils_absolute_path::AbsolutePathBuf;
use tracing::warn;

use crate::CodexAuth;
use crate::config::Config;
use crate::connectors;
use crate::plugins::AppConnectorId;
use crate::plugins::PluginCapabilitySummary;
use crate::plugins::PluginLoadOutcome;
use crate::plugins::PluginReadRequest;
use crate::plugins::PluginsManager;
use crate::tools::discoverable::DiscoverablePluginInfo;
use crate::tools::discoverable::DiscoverableTool;

pub(crate) const TRUSTED_DISCOVERABLE_PLUGIN_IDS: &[&str] = &[
    "calendar@openai-curated",
    "gmail@openai-curated",
    "linear@openai-curated",
    "slack@openai-curated",
];

pub(crate) async fn load_discoverable_tools(
    config: &Config,
    auth: Option<&CodexAuth>,
    plugins_manager: &PluginsManager,
    loaded_plugins: &PluginLoadOutcome,
    cwd: &Path,
    accessible_connectors: &[AppInfo],
) -> anyhow::Result<Vec<DiscoverableTool>> {
    let directory_connectors =
        connectors::list_directory_connectors_with_auth(config, auth).await?;
    let mut discoverable_tools = build_discoverable_connector_tools(
        directory_connectors,
        accessible_connectors,
        &loaded_plugins.effective_apps(),
        loaded_plugins.capability_summaries(),
    )
    .into_iter()
    .map(DiscoverableTool::from)
    .collect::<Vec<_>>();
    discoverable_tools.extend(
        load_discoverable_plugins(config, plugins_manager, cwd)?
            .into_iter()
            .map(DiscoverableTool::from),
    );
    discoverable_tools.sort_by(|left, right| {
        left.name()
            .cmp(right.name())
            .then_with(|| left.id().cmp(right.id()))
    });
    Ok(discoverable_tools)
}

pub(crate) fn build_discoverable_connector_tools(
    directory_connectors: Vec<AppInfo>,
    accessible_connectors: &[AppInfo],
    enabled_plugin_apps: &[AppConnectorId],
    capability_summaries: &[PluginCapabilitySummary],
) -> Vec<AppInfo> {
    let accessible_by_id = accessible_connectors
        .iter()
        .map(|connector| (connector.id.as_str(), connector))
        .collect::<HashMap<_, _>>();
    let mut discoverable_by_id = connectors::filter_tool_suggest_discoverable_tools(
        directory_connectors.clone(),
        accessible_connectors,
    )
    .into_iter()
    .map(|mut connector| {
        if let Some(accessible_connector) = accessible_by_id.get(connector.id.as_str()) {
            connector.is_accessible = accessible_connector.is_accessible;
            connector.is_enabled = accessible_connector.is_enabled;
        }
        (connector.id.clone(), connector)
    })
    .collect::<HashMap<_, _>>();
    let accessible_enabled_ids = accessible_connectors
        .iter()
        .filter(|connector| connector.is_accessible && connector.is_enabled)
        .map(|connector| connector.id.as_str())
        .collect::<HashSet<_>>();
    let plugin_connector_ids = enabled_plugin_apps
        .iter()
        .map(|connector_id| connector_id.0.as_str())
        .collect::<HashSet<_>>();
    let plugin_display_names = build_plugin_display_names_by_connector_id(capability_summaries);

    for mut connector in connectors::filter_disallowed_connectors(connectors::merge_plugin_apps(
        directory_connectors,
        enabled_plugin_apps.to_vec(),
    )) {
        if accessible_enabled_ids.contains(connector.id.as_str())
            || !plugin_connector_ids.contains(connector.id.as_str())
        {
            continue;
        }
        connector.plugin_display_names = plugin_display_names
            .get(connector.id.as_str())
            .cloned()
            .unwrap_or_default();
        discoverable_by_id
            .entry(connector.id.clone())
            .and_modify(|existing| {
                existing
                    .plugin_display_names
                    .extend(connector.plugin_display_names.iter().cloned());
                existing.plugin_display_names.sort_unstable();
                existing.plugin_display_names.dedup();
            })
            .or_insert(connector);
    }

    let mut discoverable = discoverable_by_id.into_values().collect::<Vec<_>>();
    discoverable.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    discoverable
}

pub(crate) fn build_discoverable_plugin_tools(
    discoverable_plugins: Vec<DiscoverablePluginInfo>,
) -> Vec<DiscoverablePluginInfo> {
    let trusted_plugin_ids = TRUSTED_DISCOVERABLE_PLUGIN_IDS
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut discoverable_plugins = discoverable_plugins
        .into_iter()
        .filter(|plugin| trusted_plugin_ids.contains(plugin.id.as_str()))
        .filter(|plugin| !(plugin.installed && plugin.enabled))
        .collect::<Vec<_>>();
    discoverable_plugins.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    discoverable_plugins
}

pub(crate) fn plugin_marketplace_roots(cwd: &Path) -> Vec<AbsolutePathBuf> {
    AbsolutePathBuf::try_from(cwd.to_path_buf())
        .map(|cwd| vec![cwd])
        .unwrap_or_default()
}

pub(crate) fn plugin_completion_verified(
    plugins_manager: &PluginsManager,
    config: &Config,
    cwd: &Path,
    plugin_id: &str,
) -> bool {
    plugins_manager
        .list_marketplaces_for_config(config, &plugin_marketplace_roots(cwd))
        .ok()
        .and_then(|marketplaces| {
            marketplaces
                .into_iter()
                .flat_map(|marketplace| marketplace.plugins.into_iter())
                .find(|plugin| plugin.id == plugin_id)
        })
        .is_some_and(|plugin| plugin.installed && plugin.enabled)
}

fn load_discoverable_plugins(
    config: &Config,
    plugins_manager: &PluginsManager,
    cwd: &Path,
) -> anyhow::Result<Vec<DiscoverablePluginInfo>> {
    let marketplaces = match plugins_manager
        .list_marketplaces_for_config(config, &plugin_marketplace_roots(cwd))
    {
        Ok(marketplaces) => marketplaces,
        Err(err) => {
            warn!("failed to list plugin marketplaces for tool suggestions: {err}");
            return Ok(Vec::new());
        }
    };

    let mut discoverable_plugins = Vec::new();
    for marketplace in marketplaces {
        for plugin in marketplace.plugins {
            if !TRUSTED_DISCOVERABLE_PLUGIN_IDS.contains(&plugin.id.as_str())
                || (plugin.installed && plugin.enabled)
            {
                continue;
            }

            let request = PluginReadRequest {
                marketplace_path: marketplace.path.clone(),
                plugin_name: plugin.name.clone(),
            };
            match plugins_manager.read_plugin_for_config(config, &request) {
                Ok(plugin_detail) => {
                    discoverable_plugins.push(DiscoverablePluginInfo::from_plugin_detail(
                        plugin_detail.marketplace_path,
                        plugin_detail.plugin,
                    ))
                }
                Err(err) => {
                    warn!(
                        plugin_id = plugin.id,
                        "failed to read plugin details for tool suggestion: {err}"
                    );
                }
            }
        }
    }

    Ok(build_discoverable_plugin_tools(discoverable_plugins))
}

fn build_plugin_display_names_by_connector_id(
    capability_summaries: &[PluginCapabilitySummary],
) -> HashMap<&str, Vec<String>> {
    let mut plugin_display_names_by_connector_id: HashMap<&str, BTreeSet<String>> = HashMap::new();
    for plugin in capability_summaries {
        for connector_id in &plugin.app_connector_ids {
            plugin_display_names_by_connector_id
                .entry(connector_id.0.as_str())
                .or_default()
                .insert(plugin.display_name.clone());
        }
    }

    plugin_display_names_by_connector_id
        .into_iter()
        .map(|(connector_id, plugin_display_names)| {
            (
                connector_id,
                plugin_display_names.into_iter().collect::<Vec<_>>(),
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "discoverable_catalog_tests.rs"]
mod tests;
