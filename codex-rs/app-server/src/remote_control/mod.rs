mod client_manager;
mod connection_manager;
mod entrollment_manager;

#[cfg(test)]
mod test_support;

use self::entrollment_manager::EnrollmentManager;
use crate::outgoing_message::OutgoingMessage;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::ConnectionIdAllocator;
use crate::transport::TransportEvent;
use codex_app_server_protocol::JSONRPCMessage;
use codex_core::AuthManager;
use serde::Deserialize;
use serde::Serialize;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const REMOTE_CONTROL_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const REMOTE_CONTROL_REQUEST_ID_HEADER: &str = "x-request-id";
const REMOTE_CONTROL_STATE_FILE: &str = "remote_control.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteControlTarget {
    websocket_url: String,
    enroll_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteControlEnrollment {
    server_id: String,
    server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteControlConnectionAuth {
    bearer_token: String,
    account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
struct ClientId(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClientActivityState {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientEvent {
    ClientMessage {
        client_id: ClientId,
        message: JSONRPCMessage,
    },
    Ping {
        client_id: ClientId,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<ClientActivityState>,
    },
    ClientClosed {
        client_id: ClientId,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerEvent {
    ServerMessage {
        client_id: ClientId,
        message: Box<OutgoingMessage>,
    },
    Pong {
        client_id: ClientId,
        status: PongStatus,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PongStatus {
    Active,
    Unknown,
}

pub(crate) async fn start_remote_control(
    remote_control_url: String,
    codex_home: PathBuf,
    auth_manager: Arc<AuthManager>,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown_token: CancellationToken,
    connection_id_allocator: ConnectionIdAllocator,
) -> IoResult<JoinHandle<()>> {
    let remote_control_target = normalize_remote_control_url(&remote_control_url)?;
    let enrollment_manager = EnrollmentManager::new(remote_control_target.clone(), codex_home);

    Ok(tokio::spawn(async move {
        let local_shutdown_token = shutdown_token.child_token();
        let (client_event_tx, client_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (server_event_tx, server_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (writer_exited_tx, writer_exited_rx) = mpsc::channel(CHANNEL_CAPACITY);

        let mut websocket_task = tokio::spawn(connection_manager::run(
            auth_manager,
            remote_control_target,
            enrollment_manager,
            client_event_tx,
            server_event_rx,
            local_shutdown_token.clone(),
        ));
        let mut manager_task = tokio::spawn(client_manager::run(
            transport_event_tx,
            client_event_rx,
            server_event_tx,
            writer_exited_tx,
            writer_exited_rx,
            local_shutdown_token.clone(),
            connection_id_allocator,
        ));

        tokio::select! {
            _ = local_shutdown_token.cancelled() => {}
            _ = &mut websocket_task => {
                local_shutdown_token.cancel();
            }
            _ = &mut manager_task => {
                local_shutdown_token.cancel();
            }
        }

        let _ = websocket_task.await;
        let _ = manager_task.await;
    }))
}

fn normalize_remote_control_url(remote_control_url: &str) -> IoResult<RemoteControlTarget> {
    let remote_control_url = remote_control_url.trim_end_matches('/');

    if let Some(rest) = remote_control_url.strip_prefix("http://") {
        return Ok(normalize_http_remote_control_url(rest, "http://", "ws://"));
    }
    if let Some(rest) = remote_control_url.strip_prefix("https://") {
        return Ok(normalize_http_remote_control_url(
            rest, "https://", "wss://",
        ));
    }

    Err(std::io::Error::new(
        ErrorKind::InvalidInput,
        format!("invalid remote control URL `{remote_control_url}`; expected http:// or https://"),
    ))
}

fn normalize_http_remote_control_url(
    rest: &str,
    http_scheme: &str,
    websocket_scheme: &str,
) -> RemoteControlTarget {
    let rest = normalize_chatgpt_remote_control_base(rest);
    let rest = if let Some(rest) = rest.strip_suffix("/remote/control/server/enroll") {
        format!("{rest}/remote/control/server")
    } else if rest.ends_with("/remote/control/server") {
        rest
    } else if let Some(rest) = rest.strip_suffix("/server/enroll") {
        format!("{rest}/server")
    } else if rest.ends_with("/server") {
        rest
    } else {
        format!("{rest}/remote/control/server")
    };

    RemoteControlTarget {
        websocket_url: format!("{websocket_scheme}{rest}"),
        enroll_url: format!("{http_scheme}{rest}/enroll"),
    }
}

fn normalize_chatgpt_remote_control_base(rest: &str) -> String {
    let trimmed = rest.trim_end_matches('/');
    let (host, path) = match trimmed.split_once('/') {
        Some((host, path)) => (host, Some(path)),
        None => (trimmed, None),
    };
    if host != "chatgpt.com" && host != "chat.openai.com" {
        return trimmed.to_string();
    }

    let Some(path) = path else {
        return format!("{host}/backend-api/wham");
    };

    if path == "backend-api/wham" || path.starts_with("backend-api/wham/") {
        return trimmed.to_string();
    }

    for internal_prefix in ["api/codex", "backend-api"] {
        if path == internal_prefix {
            return format!("{host}/backend-api/wham");
        }
        if let Some(suffix) = path.strip_prefix(&format!("{internal_prefix}/")) {
            if suffix == "remote/control/server" || suffix == "remote/control/server/enroll" {
                return format!("{host}/backend-api/wham/{suffix}");
            }
            return format!("{host}/backend-api/wham");
        }
    }

    if path == "remote/control/server" || path == "remote/control/server/enroll" {
        return format!("{host}/backend-api/wham/{path}");
    }

    trimmed.to_string()
}

async fn load_remote_control_auth(
    auth_manager: &AuthManager,
) -> IoResult<RemoteControlConnectionAuth> {
    let auth = match auth_manager.auth().await {
        Some(auth) => auth,
        None => {
            auth_manager.reload();
            auth_manager.auth().await.ok_or_else(|| {
                std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "remote control requires ChatGPT authentication",
                )
            })?
        }
    };

    if !auth.is_chatgpt_auth() {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "remote control requires ChatGPT authentication; API key auth is not supported",
        ));
    }

    Ok(RemoteControlConnectionAuth {
        bearer_token: auth.get_token().map_err(std::io::Error::other)?,
        account_id: auth.get_account_id(),
    })
}
