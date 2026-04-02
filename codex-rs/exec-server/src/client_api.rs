use std::path::PathBuf;
use std::time::Duration;

/// Connection options for any exec-server client transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerClientConnectOptions {
    pub client_name: String,
    pub initialize_timeout: Duration,
}

/// WebSocket connection arguments for a remote exec-server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecServerConnectArgs {
    pub websocket_url: String,
    pub client_name: String,
    pub connect_timeout: Duration,
    pub initialize_timeout: Duration,
    pub path_translation: Option<RemoteExecPathTranslation>,
}

/// Local-to-remote root rewrite applied to outbound exec-server paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecPathTranslation {
    pub local_root: PathBuf,
    pub remote_root: PathBuf,
}
