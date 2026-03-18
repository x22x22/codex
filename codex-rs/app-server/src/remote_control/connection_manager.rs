use crate::remote_control::entrollment_manager::EnrollmentManager;
use crate::remote_control::entrollment_manager::preview_remote_control_response_body;
use crate::transport::colorize;
use codex_core::AuthManager;
use codex_utils_rustls_provider::ensure_rustls_crypto_provider;
use futures::SinkExt;
use futures::StreamExt;
use owo_colors::Style;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self};
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::info;
use tracing::warn;

use super::ClientEvent;
use super::REMOTE_CONTROL_ACCOUNT_ID_HEADER;
use super::REMOTE_CONTROL_REQUEST_ID_HEADER;
use super::RemoteControlConnectionAuth;
use super::RemoteControlEnrollment;
use super::ServerEvent;
use super::load_remote_control_auth;

const REMOTE_CONTROL_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const REMOTE_CONTROL_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(30);
const REMOTE_CONTROL_PROTOCOL_VERSION: &str = "2";

const REMOTE_CONTROL_OAI_REQUEST_ID_HEADER: &str = "x-oai-request-id";
const REMOTE_CONTROL_CF_RAY_HEADER: &str = "cf-ray";

struct RemoteControlWebsocketConnection {
    websocket_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    #[cfg_attr(not(test), allow(dead_code))]
    request_id: Option<String>,
}

#[allow(clippy::print_stderr)]
fn print_remote_control_connection_banner(
    remote_control_target: &super::RemoteControlTarget,
    reconnect_attempt: u64,
    reconnect_backoff: Duration,
    reconnect_reason: Option<&str>,
) {
    let title = colorize("app-server remote-control", Style::new().bold().yellow());
    let control_server_label = colorize("control server:", Style::new().dimmed());
    let control_server_url = remote_control_target.websocket_url.as_str();
    let control_server_url = colorize(control_server_url, Style::new().green());
    eprintln!("{title}");
    eprintln!("  {control_server_label} {control_server_url}");

    if reconnect_attempt > 0 {
        let attempt_label = colorize("attempt:", Style::new().dimmed());
        let after_label = colorize("after:", Style::new().dimmed());
        eprintln!("  {attempt_label} {reconnect_attempt}");
        eprintln!("  {after_label} {reconnect_backoff:?}");
    }
    if let Some(reason) = reconnect_reason {
        let reason_label = colorize("reason:", Style::new().dimmed());
        eprintln!("  {reason_label} {reason}");
    }
}

#[allow(clippy::print_stderr)]
pub(super) async fn run(
    auth_manager: std::sync::Arc<AuthManager>,
    remote_control_target: super::RemoteControlTarget,
    mut enrollment_manager: EnrollmentManager,
    client_event_tx: mpsc::Sender<ClientEvent>,
    mut server_event_rx: mpsc::Receiver<ServerEvent>,
    shutdown_token: CancellationToken,
) {
    let mut reconnect_backoff = REMOTE_CONTROL_RECONNECT_INITIAL_BACKOFF;
    let mut reconnect_attempt = 0_u64;
    let mut reconnect_reason = None::<String>;
    let mut wait_before_connect = false;
    let mut pending_server_event = None::<ServerEvent>;

    loop {
        let connect_delay = if wait_before_connect {
            tokio::select! {
                _ = shutdown_token.cancelled() => {
                    break;
                }
                _ = tokio::time::sleep(reconnect_backoff) => {
                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                }
            }
            reconnect_backoff
        } else {
            wait_before_connect = true;
            Duration::ZERO
        };

        print_remote_control_connection_banner(
            &remote_control_target,
            reconnect_attempt,
            connect_delay,
            reconnect_reason.as_deref(),
        );

        let websocket_connection = tokio::select! {
            _ = shutdown_token.cancelled() => {
                break;
            }
            connect_result = connect_remote_control_websocket(
                auth_manager.as_ref(),
                &remote_control_target,
                &mut enrollment_manager,
            ) => {
                match connect_result {
                    Ok(websocket_connection) => {
                        reconnect_backoff = REMOTE_CONTROL_RECONNECT_INITIAL_BACKOFF;
                        reconnect_attempt = 0;
                        info!(
                            "connected to app-server remote control websocket: {}",
                            remote_control_target.websocket_url
                        );
                        websocket_connection
                    }
                    Err(err) => {
                        warn!("{err}");
                        reconnect_reason = Some(err.to_string());
                        if connect_delay != Duration::ZERO {
                            reconnect_backoff = reconnect_backoff
                                .saturating_mul(2)
                                .min(REMOTE_CONTROL_RECONNECT_MAX_BACKOFF);
                        }
                        continue;
                    }
                }
            }
        };

        let (mut websocket_writer, mut websocket_reader) =
            websocket_connection.websocket_stream.split();
        loop {
            if let Some(server_event) = pending_server_event.take() {
                let payload = match serde_json::to_string(&server_event) {
                    Ok(payload) => payload,
                    Err(err) => {
                        error!("failed to serialize remote-control server event: {err}");
                        continue;
                    }
                };
                if let Err(err) = websocket_writer
                    .send(TungsteniteMessage::Text(payload.into()))
                    .await
                {
                    warn!("remote control websocket send failed: {err}");
                    reconnect_reason = Some(format!("send failed: {err}"));
                    pending_server_event = Some(server_event);
                    break;
                }
                continue;
            }

            tokio::select! {
                _ = shutdown_token.cancelled() => {
                    return;
                }
                incoming_message = websocket_reader.next() => {
                    match incoming_message {
                        Some(Ok(TungsteniteMessage::Text(text))) => {
                            match serde_json::from_str::<ClientEvent>(&text) {
                                Ok(client_event) => {
                                    if client_event_tx.send(client_event).await.is_err() {
                                        return;
                                    }
                                }
                                Err(_) => {
                                    warn!("failed to deserialize remote-control client event");
                                }
                            }
                        }
                        Some(Ok(TungsteniteMessage::Ping(payload))) => {
                            if let Err(err) = websocket_writer
                                .send(TungsteniteMessage::Pong(payload))
                                .await
                            {
                                warn!("remote control websocket pong failed: {err}");
                                reconnect_reason = Some(format!("pong failed: {err}"));
                                break;
                            }
                        }
                        Some(Ok(TungsteniteMessage::Pong(_))) => {}
                        Some(Ok(TungsteniteMessage::Binary(_))) => {
                            warn!("dropping unsupported binary remote-control websocket message");
                        }
                        Some(Ok(TungsteniteMessage::Frame(_))) => {}
                        Some(Ok(TungsteniteMessage::Close(_))) | None => {
                            warn!("remote control websocket disconnected");
                            reconnect_reason = Some("server closed the websocket".to_string());
                            break;
                        }
                        Some(Err(err)) => {
                            warn!("remote control websocket receive error: {err}");
                            reconnect_reason = Some(format!("receive error: {err}"));
                            break;
                        }
                    }
                }
                server_event = server_event_rx.recv() => {
                    let Some(server_event) = server_event else {
                        return;
                    };
                    pending_server_event = Some(server_event);
                }
            }
        }
    }
}

fn set_remote_control_header(
    headers: &mut tungstenite::http::HeaderMap,
    name: &'static str,
    value: &str,
) -> IoResult<()> {
    let header_value = HeaderValue::from_str(value).map_err(|err| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control header `{name}`: {err}"),
        )
    })?;
    headers.insert(name, header_value);
    Ok(())
}

fn build_remote_control_websocket_request(
    auth: &RemoteControlConnectionAuth,
    websocket_url: &str,
    enrollment: &RemoteControlEnrollment,
) -> IoResult<tungstenite::http::Request<()>> {
    let mut request = websocket_url.into_client_request().map_err(|err| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control websocket URL `{websocket_url}`: {err}"),
        )
    })?;
    let headers = request.headers_mut();
    set_remote_control_header(headers, "x-codex-server-id", &enrollment.server_id)?;
    set_remote_control_header(headers, "x-codex-name", &enrollment.server_name)?;
    set_remote_control_header(
        headers,
        "x-codex-protocol-version",
        REMOTE_CONTROL_PROTOCOL_VERSION,
    )?;
    set_remote_control_header(
        headers,
        "authorization",
        &format!("Bearer {}", auth.bearer_token),
    )?;
    if let Some(account_id) = auth.account_id.as_deref() {
        set_remote_control_header(headers, REMOTE_CONTROL_ACCOUNT_ID_HEADER, account_id)?;
    }
    Ok(request)
}

async fn connect_remote_control_websocket(
    auth_manager: &AuthManager,
    remote_control_target: &super::RemoteControlTarget,
    enrollment_manager: &mut EnrollmentManager,
) -> IoResult<RemoteControlWebsocketConnection> {
    ensure_rustls_crypto_provider();
    let websocket_url = remote_control_target.websocket_url.clone();
    let auth = load_remote_control_auth(auth_manager).await?;
    let enrollment = enrollment_manager.enroll(&auth).await?;
    let request = build_remote_control_websocket_request(&auth, &websocket_url, &enrollment)?;

    let (websocket_stream, response) = match connect_async(request).await {
        Ok(connection) => connection,
        Err(err) => {
            return Err(std::io::Error::other(
                format_remote_control_websocket_connect_error(&websocket_url, &err),
            ));
        }
    };

    let request_id = remote_control_request_id(response.headers());

    Ok(RemoteControlWebsocketConnection {
        websocket_stream,
        request_id,
    })
}

fn remote_control_header_value(
    headers: &tungstenite::http::HeaderMap,
    header_name: &str,
) -> Option<String> {
    headers
        .get(header_name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn remote_control_request_id(headers: &tungstenite::http::HeaderMap) -> Option<String> {
    remote_control_header_value(headers, REMOTE_CONTROL_REQUEST_ID_HEADER)
        .or_else(|| remote_control_header_value(headers, REMOTE_CONTROL_OAI_REQUEST_ID_HEADER))
}

fn format_remote_control_websocket_connect_error(
    websocket_url: &str,
    err: &tungstenite::Error,
) -> String {
    let mut message =
        format!("failed to connect app-server remote control websocket `{websocket_url}`: {err}");
    let tungstenite::Error::Http(response) = err else {
        return message;
    };

    if let Some(request_id) = remote_control_request_id(response.headers()) {
        message.push_str(&format!(", request id: {request_id}"));
    }
    if let Some(cf_ray) =
        remote_control_header_value(response.headers(), REMOTE_CONTROL_CF_RAY_HEADER)
    {
        message.push_str(&format!(", cf-ray: {cf_ray}"));
    }
    if let Some(body) = response.body().as_ref()
        && !body.is_empty()
    {
        let body_preview = preview_remote_control_response_body(body);
        message.push_str(&format!(", body: {body_preview}"));
    }

    message
}

#[cfg(test)]
#[path = "connection_manager_tests.rs"]
mod connection_manager_tests;
