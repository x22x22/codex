use super::protocol::EnrollRemoteServerRequest;
use super::protocol::EnrollRemoteServerResponse;
use super::protocol::PersistedRemoteControlEnrollment;
use super::protocol::RemoteControlStateToml;
use super::protocol::RemoteControlTarget;
use base64::Engine;
use codex_core::AuthManager;
use codex_core::default_client::build_reqwest_client;
use codex_core::path_utils::write_atomically;
use codex_utils_rustls_provider::ensure_rustls_crypto_provider;
use gethostname::gethostname;
use io::ErrorKind;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tracing::warn;

const REMOTE_CONTROL_ENROLL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES: usize = 4096;
pub(super) const REMOTE_CONTROL_PROTOCOL_VERSION: &str = "2";
pub(super) const REMOTE_CONTROL_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const REMOTE_CONTROL_SUBSCRIBE_CURSOR_HEADER: &str = "x-codex-subscribe-cursor";
const REMOTE_CONTROL_STATE_FILE: &str = "remote_control.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteControlEnrollment {
    pub(super) server_id: String,
    pub(super) server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteControlConnectionAuth {
    pub(super) bearer_token: String,
    pub(super) account_id: Option<String>,
}

pub(super) struct RemoteControlWebsocketConnection {
    pub(super) websocket_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

pub(super) fn remote_control_state_path(codex_home: &Path) -> PathBuf {
    codex_home.join(REMOTE_CONTROL_STATE_FILE)
}

fn matches_persisted_remote_control_enrollment(
    entry: &PersistedRemoteControlEnrollment,
    remote_control_target: &RemoteControlTarget,
    account_id: Option<&str>,
) -> bool {
    entry.websocket_url == remote_control_target.websocket_url
        && entry.account_id.as_deref() == account_id
}

async fn load_remote_control_state(state_path: &Path) -> io::Result<RemoteControlStateToml> {
    let contents = match tokio::fs::read_to_string(state_path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(RemoteControlStateToml::default());
        }
        Err(err) => return Err(err),
    };

    toml::from_str(&contents).map_err(|err| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "failed to parse remote control state `{}`: {err}",
                state_path.display()
            ),
        )
    })
}

async fn write_remote_control_state(
    state_path: &Path,
    state: &RemoteControlStateToml,
) -> io::Result<()> {
    if let Some(parent) = state_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let state_path = state_path.to_owned();
    let contents: String = toml::to_string(state).map_err(io::Error::other)?;
    tokio::task::spawn_blocking(move || write_atomically(&state_path, &contents)).await?
}

pub(super) async fn load_persisted_remote_control_enrollment(
    state_path: &Path,
    remote_control_target: &RemoteControlTarget,
    account_id: Option<&str>,
) -> Option<RemoteControlEnrollment> {
    let state = match load_remote_control_state(state_path).await {
        Ok(state) => state,
        Err(err) => {
            warn!("{err}");
            return None;
        }
    };

    state
        .enrollments
        .into_iter()
        .find(|entry| {
            matches_persisted_remote_control_enrollment(entry, remote_control_target, account_id)
        })
        .map(|entry| RemoteControlEnrollment {
            server_id: entry.server_id,
            server_name: entry.server_name,
        })
}

pub(super) async fn update_persisted_remote_control_enrollment(
    state_path: &Path,
    remote_control_target: &RemoteControlTarget,
    account_id: Option<&str>,
    enrollment: Option<&RemoteControlEnrollment>,
) -> io::Result<()> {
    let mut state = match load_remote_control_state(state_path).await {
        Ok(state) => state,
        Err(err) if err.kind() == ErrorKind::InvalidData => {
            warn!("{err}");
            RemoteControlStateToml::default()
        }
        Err(err) => return Err(err),
    };

    state.enrollments.retain(|entry| {
        !matches_persisted_remote_control_enrollment(entry, remote_control_target, account_id)
    });

    if let Some(enrollment) = enrollment {
        state.enrollments.push(PersistedRemoteControlEnrollment {
            websocket_url: remote_control_target.websocket_url.clone(),
            account_id: account_id.map(str::to_owned),
            server_id: enrollment.server_id.clone(),
            server_name: enrollment.server_name.clone(),
        });
    }

    if state.enrollments.is_empty() {
        match tokio::fs::remove_file(state_path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    } else {
        write_remote_control_state(state_path, &state).await
    }
}

pub(super) async fn load_remote_control_auth(
    auth_manager: &AuthManager,
) -> io::Result<RemoteControlConnectionAuth> {
    let auth = match auth_manager.auth().await {
        Some(auth) => auth,
        None => {
            auth_manager.reload();
            auth_manager.auth().await.ok_or_else(|| {
                io::Error::new(
                    ErrorKind::PermissionDenied,
                    "remote control requires ChatGPT authentication",
                )
            })?
        }
    };

    if !auth.is_chatgpt_auth() {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "remote control requires ChatGPT authentication; API key auth is not supported",
        ));
    }

    Ok(RemoteControlConnectionAuth {
        bearer_token: auth.get_token().map_err(io::Error::other)?,
        account_id: auth.get_account_id(),
    })
}

fn preview_remote_control_response_body(body: &[u8]) -> String {
    let body = String::from_utf8_lossy(body);
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    if trimmed.len() <= REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES {
        return trimmed.to_string();
    }

    let mut cut = REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES;
    while !trimmed.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    let mut truncated = trimmed[..cut].to_string();
    truncated.push_str("...");
    truncated
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

    let mut headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            format!(
                "{}: {}",
                name.as_str(),
                value.to_str().unwrap_or("<invalid utf-8>")
            )
        })
        .collect::<Vec<_>>();
    headers.sort();
    message.push_str(&format!(", headers: {{{}}}", headers.join(", ")));
    if let Some(body) = response.body().as_ref()
        && !body.is_empty()
    {
        let body_preview = preview_remote_control_response_body(body);
        message.push_str(&format!(", body: {body_preview}"));
    }

    message
}

pub(super) async fn enroll_remote_control_server(
    remote_control_target: &RemoteControlTarget,
    auth: &RemoteControlConnectionAuth,
) -> io::Result<RemoteControlEnrollment> {
    let enroll_url = &remote_control_target.enroll_url;
    let server_name = gethostname().to_string_lossy().trim().to_string();
    let request = EnrollRemoteServerRequest {
        name: server_name.clone(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        app_server_version: env!("CARGO_PKG_VERSION"),
    };
    let client = build_reqwest_client();
    let mut http_request = client
        .post(enroll_url)
        .timeout(REMOTE_CONTROL_ENROLL_TIMEOUT)
        .bearer_auth(&auth.bearer_token)
        .json(&request);
    if let Some(account_id) = auth.account_id.as_deref() {
        http_request = http_request.header(REMOTE_CONTROL_ACCOUNT_ID_HEADER, account_id);
    }

    let response = http_request.send().await.map_err(|err| {
        io::Error::other(format!(
            "failed to enroll remote control server at `{enroll_url}`: {err}"
        ))
    })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|err| {
        io::Error::other(format!(
            "failed to read remote control enrollment response from `{enroll_url}`: {err}"
        ))
    })?;
    let body_preview = preview_remote_control_response_body(&body);
    if !status.is_success() {
        return Err(io::Error::other(format!(
            "remote control server enrollment failed at `{enroll_url}`: HTTP {status}, body: {body_preview}"
        )));
    }

    let enrollment = serde_json::from_slice::<EnrollRemoteServerResponse>(&body).map_err(|err| {
        io::Error::other(format!(
            "failed to parse remote control enrollment response from `{enroll_url}`: HTTP {status}, body: {body_preview}, decode error: {err}"
        ))
    })?;

    Ok(RemoteControlEnrollment {
        server_id: enrollment.server_id,
        server_name,
    })
}

fn set_remote_control_header(
    headers: &mut tungstenite::http::HeaderMap,
    name: &'static str,
    value: &str,
) -> io::Result<()> {
    let header_value = HeaderValue::from_str(value).map_err(|err| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control header `{name}`: {err}"),
        )
    })?;
    headers.insert(name, header_value);
    Ok(())
}

fn build_remote_control_websocket_request(
    websocket_url: &str,
    enrollment: &RemoteControlEnrollment,
    auth: &RemoteControlConnectionAuth,
    subscribe_cursor: Option<&str>,
) -> io::Result<tungstenite::http::Request<()>> {
    let mut request = websocket_url.into_client_request().map_err(|err| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control websocket URL `{websocket_url}`: {err}"),
        )
    })?;
    let headers = request.headers_mut();
    set_remote_control_header(headers, "x-codex-server-id", &enrollment.server_id)?;
    set_remote_control_header(
        headers,
        "x-codex-name",
        &base64::engine::general_purpose::STANDARD.encode(&enrollment.server_name),
    )?;
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
    if let Some(subscribe_cursor) = subscribe_cursor {
        set_remote_control_header(
            headers,
            REMOTE_CONTROL_SUBSCRIBE_CURSOR_HEADER,
            subscribe_cursor,
        )?;
    }
    Ok(request)
}

pub(super) async fn connect_remote_control_websocket(
    remote_control_target: &RemoteControlTarget,
    remote_control_state_path: &Path,
    auth_manager: &AuthManager,
    enrollment: &mut Option<RemoteControlEnrollment>,
    subscribe_cursor: Option<&str>,
) -> io::Result<RemoteControlWebsocketConnection> {
    ensure_rustls_crypto_provider();

    let auth = load_remote_control_auth(auth_manager).await?;
    if enrollment.is_none() {
        *enrollment = load_persisted_remote_control_enrollment(
            remote_control_state_path,
            remote_control_target,
            auth.account_id.as_deref(),
        )
        .await;
    }

    if enrollment.is_none() {
        let new_enrollment = enroll_remote_control_server(remote_control_target, &auth).await?;
        if let Err(err) = update_persisted_remote_control_enrollment(
            remote_control_state_path,
            remote_control_target,
            auth.account_id.as_deref(),
            Some(&new_enrollment),
        )
        .await
        {
            warn!(
                "failed to persist remote control enrollment in `{}`: {err}",
                remote_control_state_path.display()
            );
        }
        *enrollment = Some(new_enrollment);
    }

    let enrollment_ref = enrollment.as_ref().ok_or_else(|| {
        io::Error::other("missing remote control enrollment after enrollment step")
    })?;
    let request = build_remote_control_websocket_request(
        &remote_control_target.websocket_url,
        enrollment_ref,
        &auth,
        subscribe_cursor,
    )?;

    let (websocket_stream, _response) = match connect_async(request).await {
        Ok((websocket_stream, response)) => (websocket_stream, response),
        Err(err) => {
            if matches!(
                &err,
                tungstenite::Error::Http(response) if response.status().as_u16() == 404
            ) {
                if let Err(clear_err) = update_persisted_remote_control_enrollment(
                    remote_control_state_path,
                    remote_control_target,
                    auth.account_id.as_deref(),
                    /*enrollment*/ None,
                )
                .await
                {
                    warn!(
                        "failed to clear stale remote control enrollment in `{}`: {clear_err}",
                        remote_control_state_path.display()
                    );
                }
                *enrollment = None;
            }
            return Err(io::Error::other(
                format_remote_control_websocket_connect_error(
                    &remote_control_target.websocket_url,
                    &err,
                ),
            ));
        }
    };

    Ok(RemoteControlWebsocketConnection { websocket_stream })
}
