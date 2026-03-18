use std::collections::HashMap;

pub use codex_mcp::auth::McpAuthStatusEntry;
pub use codex_mcp::auth::McpOAuthLoginConfig;
pub use codex_mcp::auth::McpOAuthLoginSupport;
pub use codex_mcp::auth::McpOAuthScopesSource;
pub use codex_mcp::auth::ResolvedMcpOAuthScopes;
pub use codex_mcp::auth::resolve_oauth_scopes;
pub use codex_mcp::auth::should_retry_without_scopes;

use codex_rmcp_client::OAuthCredentialsStoreMode;

use crate::config::types::McpServerConfig;
use crate::config::types::McpServerTransportConfig;
use crate::mcp_connection_manager::to_mcp_server_config;
use crate::mcp_connection_manager::to_mcp_server_transport_config;

pub async fn oauth_login_support(transport: &McpServerTransportConfig) -> McpOAuthLoginSupport {
    codex_mcp::auth::oauth_login_support(&to_mcp_server_transport_config(transport)).await
}

pub async fn discover_supported_scopes(
    transport: &McpServerTransportConfig,
) -> Option<Vec<String>> {
    codex_mcp::auth::discover_supported_scopes(&to_mcp_server_transport_config(transport)).await
}

pub async fn compute_auth_statuses<'a, I>(
    servers: I,
    store_mode: OAuthCredentialsStoreMode,
) -> HashMap<String, McpAuthStatusEntry>
where
    I: IntoIterator<Item = (&'a String, &'a McpServerConfig)>,
{
    let servers = servers
        .into_iter()
        .map(|(name, config)| (name.clone(), to_mcp_server_config(config)))
        .collect::<HashMap<_, _>>();
    codex_mcp::auth::compute_auth_statuses(servers.iter(), store_mode).await
}
