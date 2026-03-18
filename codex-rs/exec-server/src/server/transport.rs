use std::net::SocketAddr;
use std::str::FromStr;

use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tracing::warn;

use crate::server::ExecServerConfig;
use crate::server::ExecServerHandler;
use crate::server::processor::handle_connection_message;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecServerTransport {
    WebSocket { bind_address: SocketAddr },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ExecServerTransportParseError {
    UnsupportedListenUrl(String),
    InvalidWebSocketListenUrl(String),
}

impl std::fmt::Display for ExecServerTransportParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecServerTransportParseError::UnsupportedListenUrl(listen_url) => write!(
                f,
                "unsupported --listen URL `{listen_url}`; expected `ws://IP:PORT`"
            ),
            ExecServerTransportParseError::InvalidWebSocketListenUrl(listen_url) => write!(
                f,
                "invalid websocket --listen URL `{listen_url}`; expected `ws://IP:PORT`"
            ),
        }
    }
}

impl std::error::Error for ExecServerTransportParseError {}

impl ExecServerTransport {
    pub const DEFAULT_LISTEN_URL: &str = "ws://127.0.0.1:0";

    pub fn from_listen_url(listen_url: &str) -> Result<Self, ExecServerTransportParseError> {
        if let Some(socket_addr) = listen_url.strip_prefix("ws://") {
            let bind_address = socket_addr.parse::<SocketAddr>().map_err(|_| {
                ExecServerTransportParseError::InvalidWebSocketListenUrl(listen_url.to_string())
            })?;
            return Ok(Self::WebSocket { bind_address });
        }

        Err(ExecServerTransportParseError::UnsupportedListenUrl(
            listen_url.to_string(),
        ))
    }
}

impl FromStr for ExecServerTransport {
    type Err = ExecServerTransportParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_listen_url(s)
    }
}

pub(crate) async fn run_transport(
    transport: ExecServerTransport,
    config: ExecServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match transport {
        ExecServerTransport::WebSocket { bind_address } => {
            run_websocket_listener(bind_address, config).await
        }
    }
}

async fn run_websocket_listener(
    bind_address: SocketAddr,
    config: ExecServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(bind_address).await?;

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let config = config.clone();
        tokio::spawn(async move {
            match accept_async(stream).await {
                Ok(mut websocket) => {
                    let mut handler = ExecServerHandler::new(config.auth_token);
                    while let Some(message) = futures::StreamExt::next(&mut websocket).await {
                        let Ok(message) = message else {
                            break;
                        };
                        let tokio_tungstenite::tungstenite::Message::Text(text) = message else {
                            continue;
                        };
                        let Ok(message) =
                            serde_json::from_str::<codex_app_server_protocol::JSONRPCMessage>(
                                text.as_ref(),
                            )
                        else {
                            continue;
                        };
                        let Ok(response) = handle_connection_message(&mut handler, message).await
                        else {
                            break;
                        };
                        let Some(response) = response else {
                            continue;
                        };
                        let Ok(text) = serde_json::to_string(&response) else {
                            break;
                        };
                        if futures::SinkExt::send(
                            &mut websocket,
                            tokio_tungstenite::tungstenite::Message::Text(text.into()),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(err) => {
                    warn!("failed to accept exec-server websocket connection from {peer_addr}: {err}");
                }
            }
        });
    }
}
