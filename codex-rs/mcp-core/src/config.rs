use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub transport: McpServerTransportConfig,
    pub enabled: bool,
    pub required: bool,
    pub startup_timeout_sec: Option<Duration>,
    pub tool_timeout_sec: Option<Duration>,
    pub enabled_tools: Option<Vec<String>>,
    pub disabled_tools: Option<Vec<String>>,
    pub scopes: Option<Vec<String>>,
    pub oauth_resource: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerTransportConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
        env_vars: Vec<String>,
        cwd: Option<PathBuf>,
    },
    StreamableHttp {
        url: String,
        bearer_token_env_var: Option<String>,
        http_headers: Option<HashMap<String, String>>,
        env_http_headers: Option<HashMap<String, String>>,
    },
}
