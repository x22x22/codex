use std::collections::HashMap;

pub use codex_mcp::CodexAppsToolsCacheKey;
pub use codex_mcp::MCP_SANDBOX_STATE_CAPABILITY;
pub use codex_mcp::MCP_SANDBOX_STATE_METHOD;
pub use codex_mcp::McpConnectionManager;
pub use codex_mcp::SandboxState;
pub use codex_mcp::ToolInfo;
pub use codex_mcp::filter_non_codex_apps_mcp_tools_only;

use crate::CodexAuth;
use crate::config::types::McpServerConfig;
use crate::config::types::McpServerTransportConfig;

pub fn codex_apps_tools_cache_key(auth: Option<&CodexAuth>) -> CodexAppsToolsCacheKey {
    let token_data = auth.and_then(|auth| auth.get_token_data().ok());
    codex_mcp::codex_apps_tools_cache_key_from_token_data(
        token_data
            .as_ref()
            .and_then(|token_data| token_data.account_id.clone()),
        token_data
            .as_ref()
            .and_then(|token_data| token_data.id_token.chatgpt_user_id.clone()),
        token_data
            .as_ref()
            .is_some_and(|token_data| token_data.id_token.is_workspace_account()),
    )
}

pub fn to_mcp_server_config(config: &McpServerConfig) -> codex_mcp::McpServerConfig {
    codex_mcp::McpServerConfig {
        transport: to_mcp_server_transport_config(&config.transport),
        enabled: config.enabled,
        required: config.required,
        startup_timeout_sec: config.startup_timeout_sec,
        tool_timeout_sec: config.tool_timeout_sec,
        enabled_tools: config.enabled_tools.clone(),
        disabled_tools: config.disabled_tools.clone(),
        scopes: config.scopes.clone(),
        oauth_resource: config.oauth_resource.clone(),
    }
}

pub fn to_mcp_server_configs(
    configs: &HashMap<String, McpServerConfig>,
) -> HashMap<String, codex_mcp::McpServerConfig> {
    configs
        .iter()
        .map(|(name, config)| (name.clone(), to_mcp_server_config(config)))
        .collect()
}

pub fn to_mcp_server_transport_config(
    transport: &McpServerTransportConfig,
) -> codex_mcp::McpServerTransportConfig {
    match transport {
        McpServerTransportConfig::Stdio {
            command,
            args,
            env,
            env_vars,
            cwd,
        } => codex_mcp::McpServerTransportConfig::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
            env_vars: env_vars.clone(),
            cwd: cwd.clone(),
        },
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => codex_mcp::McpServerTransportConfig::StreamableHttp {
            url: url.clone(),
            bearer_token_env_var: bearer_token_env_var.clone(),
            http_headers: http_headers.clone(),
            env_http_headers: env_http_headers.clone(),
        },
    }
}
