use codex_app_server_protocol::AppInfo;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use super::*;
use crate::config::Config;
use crate::config::ConfigBuilder;
use crate::plugins::AppConnectorId;
use crate::plugins::PluginCapabilitySummary;
use crate::plugins::PluginInstallRequest;
use crate::plugins::PluginsManager;
use crate::tools::discoverable::DiscoverablePluginInfo;
use crate::tools::discoverable::DiscoverableToolAction;
use crate::tools::discoverable::DiscoverableToolType;

fn connector(id: &str, name: &str) -> AppInfo {
    AppInfo {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        logo_url: None,
        logo_url_dark: None,
        distribution_channel: None,
        branding: None,
        app_metadata: None,
        labels: None,
        install_url: None,
        is_accessible: false,
        is_enabled: true,
        plugin_display_names: Vec::new(),
    }
}

fn discoverable_plugin(
    id: &str,
    name: &str,
    installed: bool,
    enabled: bool,
) -> DiscoverablePluginInfo {
    DiscoverablePluginInfo {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        has_skills: false,
        mcp_server_names: Vec::new(),
        app_connector_ids: Vec::new(),
        marketplace_path: AbsolutePathBuf::from_absolute_path("/tmp").expect("absolute path"),
        plugin_name: name.to_string(),
        installed,
        enabled,
    }
}

fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("file should have a parent")).expect("create dir");
    fs::write(path, contents).expect("write file");
}

fn write_plugin(root: &Path, dir_name: &str, manifest_name: &str) {
    let plugin_root = root.join(dir_name);
    fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("create plugin dir");
    fs::create_dir_all(plugin_root.join("skills")).expect("create skills dir");
    fs::write(
        plugin_root.join(".codex-plugin/plugin.json"),
        format!(r#"{{"name":"{manifest_name}"}}"#),
    )
    .expect("write manifest");
    fs::write(plugin_root.join("skills/SKILL.md"), "skill").expect("write skill");
    fs::write(plugin_root.join(".mcp.json"), r#"{"mcpServers":{}}"#).expect("write mcp config");
}

fn write_openai_curated_marketplace(root: &Path, plugin_names: &[&str]) {
    fs::create_dir_all(root.join(".agents/plugins")).expect("create marketplace dir");
    let plugins = plugin_names
        .iter()
        .map(|plugin_name| {
            format!(
                r#"{{
      "name": "{plugin_name}",
      "source": {{
        "source": "local",
        "path": "./plugins/{plugin_name}"
      }}
    }}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    fs::write(
        root.join(".agents/plugins/marketplace.json"),
        format!(
            r#"{{
  "name": "openai-curated",
  "plugins": [
{plugins}
  ]
}}"#
        ),
    )
    .expect("write marketplace");
    for plugin_name in plugin_names {
        write_plugin(root, &format!("plugins/{plugin_name}"), plugin_name);
    }
}

fn write_curated_plugin_sha(codex_home: &Path, sha: &str) {
    write_file(&codex_home.join(".tmp/plugins.sha"), &format!("{sha}\n"));
}

fn write_plugins_feature_enabled(codex_home: &Path) {
    write_file(
        &codex_home.join("config.toml"),
        "[features]\nplugins = true\n",
    );
}

async fn load_config(codex_home: &Path, cwd: &Path) -> Config {
    ConfigBuilder::default()
        .codex_home(codex_home.to_path_buf())
        .fallback_cwd(Some(cwd.to_path_buf()))
        .build()
        .await
        .expect("config should load")
}

#[test]
fn disabled_accessible_connectors_render_enable_action() {
    let discoverable = build_discoverable_connector_tools(
        vec![connector(
            "connector_68df038e0ba48191908c8434991bbac2",
            "Gmail",
        )],
        &[AppInfo {
            is_accessible: true,
            is_enabled: false,
            ..connector("connector_68df038e0ba48191908c8434991bbac2", "Gmail")
        }],
        &[],
        &[],
    );

    assert_eq!(discoverable.len(), 1);
    let tool = DiscoverableTool::from(discoverable[0].clone());
    assert_eq!(tool.tool_type(), DiscoverableToolType::Connector);
    assert_eq!(tool.action(), DiscoverableToolAction::Enable);
}

#[test]
fn enabled_accessible_connectors_are_excluded() {
    let discoverable = build_discoverable_connector_tools(
        vec![connector(
            "connector_68df038e0ba48191908c8434991bbac2",
            "Gmail",
        )],
        &[AppInfo {
            is_accessible: true,
            is_enabled: true,
            ..connector("connector_68df038e0ba48191908c8434991bbac2", "Gmail")
        }],
        &[],
        &[],
    );

    assert_eq!(discoverable, Vec::<AppInfo>::new());
}

#[test]
fn plugin_sourced_inaccessible_connectors_are_included() {
    let discoverable = build_discoverable_connector_tools(
        Vec::new(),
        &[],
        &[AppConnectorId("connector_plugin_mail".to_string())],
        &[PluginCapabilitySummary {
            config_name: "gmail".to_string(),
            display_name: "Gmail Plugin".to_string(),
            description: None,
            has_skills: false,
            mcp_server_names: Vec::new(),
            app_connector_ids: vec![AppConnectorId("connector_plugin_mail".to_string())],
        }],
    );

    assert_eq!(discoverable.len(), 1);
    assert_eq!(discoverable[0].id, "connector_plugin_mail");
    assert_eq!(
        discoverable[0].plugin_display_names,
        vec!["Gmail Plugin".to_string()]
    );
}

#[test]
fn trusted_plugins_surface_with_install_and_enable_actions() {
    let discoverable_tools = build_discoverable_plugin_tools(vec![
        discoverable_plugin("gmail@openai-curated", "Gmail", false, false),
        discoverable_plugin("slack@openai-curated", "Slack", true, false),
        discoverable_plugin("calendar@openai-curated", "Calendar", true, true),
    ])
    .into_iter()
    .map(DiscoverableTool::from)
    .collect::<Vec<_>>();

    assert_eq!(discoverable_tools.len(), 2);
    assert_eq!(discoverable_tools[0].id(), "gmail@openai-curated");
    assert_eq!(
        discoverable_tools[0].action(),
        DiscoverableToolAction::Install
    );
    assert_eq!(discoverable_tools[1].id(), "slack@openai-curated");
    assert_eq!(
        discoverable_tools[1].action(),
        DiscoverableToolAction::Enable
    );
}

#[test]
fn non_allowlisted_plugins_stay_hidden() {
    let discoverable = build_discoverable_plugin_tools(vec![
        discoverable_plugin("gmail@openai-curated", "Gmail", false, false),
        discoverable_plugin("not-trusted@test", "Not Trusted", false, false),
    ]);

    assert_eq!(discoverable.len(), 1);
    assert_eq!(discoverable[0].id, "gmail@openai-curated");
}

#[tokio::test]
async fn plugin_completion_verified_requires_installed_and_enabled_plugin() {
    let codex_home = TempDir::new().expect("tempdir");
    let repo_root = TempDir::new().expect("tempdir");
    write_plugins_feature_enabled(codex_home.path());
    write_curated_plugin_sha(
        codex_home.path(),
        "0123456789abcdef0123456789abcdef01234567",
    );
    write_openai_curated_marketplace(repo_root.path(), &["gmail"]);

    let plugins_manager = PluginsManager::new(codex_home.path().to_path_buf());
    let config = load_config(codex_home.path(), repo_root.path()).await;
    assert!(!plugin_completion_verified(
        &plugins_manager,
        &config,
        repo_root.path(),
        "gmail@openai-curated",
    ));

    plugins_manager
        .install_plugin(PluginInstallRequest {
            marketplace_path: AbsolutePathBuf::try_from(
                repo_root.path().join(".agents/plugins/marketplace.json"),
            )
            .expect("absolute marketplace path"),
            plugin_name: "gmail".to_string(),
        })
        .await
        .expect("install plugin");

    let refreshed_config = load_config(codex_home.path(), repo_root.path()).await;
    plugins_manager.clear_cache();
    assert!(plugin_completion_verified(
        &plugins_manager,
        &refreshed_config,
        repo_root.path(),
        "gmail@openai-curated",
    ));
}
