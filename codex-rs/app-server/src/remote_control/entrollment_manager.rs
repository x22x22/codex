use codex_core::default_client::build_reqwest_client;
use serde::Deserialize;
use serde::Serialize;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;

use super::REMOTE_CONTROL_ACCOUNT_ID_HEADER;
use super::REMOTE_CONTROL_STATE_FILE;
use super::RemoteControlConnectionAuth;
use super::RemoteControlEnrollment;
use super::RemoteControlTarget;

const REMOTE_CONTROL_ENROLL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES: usize = 4096;

const REMOTE_CONTROL_SERVER_NAME: &str = "codex-app-server";

struct CachedRemoteControlEnrollment {
    account_id: Option<String>,
    enrollment: RemoteControlEnrollment,
}

pub(super) struct EnrollmentManager {
    remote_control_target: RemoteControlTarget,
    remote_control_state_path: PathBuf,
    cached_enrollment: Option<CachedRemoteControlEnrollment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct RemoteControlStateToml {
    #[serde(default)]
    enrollments: Vec<PersistedRemoteControlEnrollment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedRemoteControlEnrollment {
    websocket_url: String,
    account_id: Option<String>,
    server_id: String,
    server_name: String,
}

#[derive(Debug, Serialize)]
struct EnrollRemoteServerRequest<'a> {
    name: &'a str,
    os: &'a str,
    arch: &'a str,
    app_server_version: &'a str,
}

#[derive(Debug, Deserialize)]
struct EnrollRemoteServerResponse {
    server_id: String,
}

impl EnrollmentManager {
    pub(super) fn new(remote_control_target: RemoteControlTarget, codex_home: PathBuf) -> Self {
        Self {
            remote_control_target,
            remote_control_state_path: remote_control_state_path(codex_home.as_path()),
            cached_enrollment: None,
        }
    }

    pub(super) async fn enroll(
        &mut self,
        auth: &RemoteControlConnectionAuth,
    ) -> IoResult<RemoteControlEnrollment> {
        if self
            .cached_enrollment
            .as_ref()
            .and_then(|cached| cached.account_id.as_deref())
            != auth.account_id.as_deref()
        {
            self.cached_enrollment = None;
        }

        if self.cached_enrollment.is_none() {
            self.cached_enrollment = load_persisted_remote_control_enrollment(
                self.remote_control_state_path.as_path(),
                &self.remote_control_target,
                auth.account_id.as_deref(),
            )
            .await
            .map(|enrollment| CachedRemoteControlEnrollment {
                account_id: auth.account_id.clone(),
                enrollment,
            });
        }

        if self.cached_enrollment.is_none() {
            let new_enrollment =
                enroll_remote_control_server(&self.remote_control_target, auth).await?;
            if let Err(err) = update_persisted_remote_control_enrollment(
                self.remote_control_state_path.as_path(),
                &self.remote_control_target,
                auth.account_id.as_deref(),
                Some(&new_enrollment),
            )
            .await
            {
                warn!(
                    "failed to persist remote control enrollment in `{}`: {err}",
                    self.remote_control_state_path.display()
                );
            }
            self.cached_enrollment = Some(CachedRemoteControlEnrollment {
                account_id: auth.account_id.clone(),
                enrollment: new_enrollment,
            });
        }

        let enrollment = self
            .cached_enrollment
            .as_ref()
            .map(|cached| cached.enrollment.clone())
            .ok_or_else(|| {
                std::io::Error::other("missing remote control enrollment after enrollment step")
            })?;

        Ok(enrollment)
    }
}

fn remote_control_server_name() -> String {
    let host_name = gethostname::gethostname();
    let host_name = host_name.to_string_lossy();
    let host_name = host_name.trim();
    if host_name.is_empty() {
        REMOTE_CONTROL_SERVER_NAME.to_string()
    } else {
        host_name.to_owned()
    }
}

fn matches_persisted_remote_control_enrollment(
    entry: &PersistedRemoteControlEnrollment,
    remote_control_target: &RemoteControlTarget,
    account_id: Option<&str>,
) -> bool {
    entry.websocket_url == remote_control_target.websocket_url
        && entry.account_id.as_deref() == account_id
}

async fn load_remote_control_state(state_path: &Path) -> IoResult<RemoteControlStateToml> {
    let contents = match tokio::fs::read_to_string(state_path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(RemoteControlStateToml::default());
        }
        Err(err) => return Err(err),
    };

    toml::from_str(&contents).map_err(|err| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "failed to parse remote control state `{}`: {err}",
                state_path.display()
            ),
        )
    })
}

fn remote_control_state_path(codex_home: &Path) -> PathBuf {
    codex_home.join(REMOTE_CONTROL_STATE_FILE)
}

async fn write_remote_control_state(
    state_path: &Path,
    state: &RemoteControlStateToml,
) -> IoResult<()> {
    if let Some(parent) = state_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let serialized = toml::to_string(state).map_err(std::io::Error::other)?;
    tokio::fs::write(state_path, serialized).await
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
) -> IoResult<()> {
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

pub(super) fn preview_remote_control_response_body(body: &[u8]) -> String {
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

async fn enroll_remote_control_server(
    remote_control_target: &RemoteControlTarget,
    auth: &RemoteControlConnectionAuth,
) -> IoResult<RemoteControlEnrollment> {
    let server_name = remote_control_server_name();
    let request = EnrollRemoteServerRequest {
        name: &server_name,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        app_server_version: env!("CARGO_PKG_VERSION"),
    };
    let client = build_reqwest_client();

    let mut http_request = client
        .post(&remote_control_target.enroll_url)
        .timeout(REMOTE_CONTROL_ENROLL_TIMEOUT)
        .bearer_auth(&auth.bearer_token)
        .json(&request);

    if let Some(account_id) = auth.account_id.as_deref() {
        http_request = http_request.header(REMOTE_CONTROL_ACCOUNT_ID_HEADER, account_id);
    }

    let response = http_request.send().await.map_err(|err| {
        std::io::Error::other(format!(
            "failed to enroll remote control server at `{}`: {err}",
            remote_control_target.enroll_url
        ))
    })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|err| {
        std::io::Error::other(format!(
            "failed to read remote control enrollment response from `{}`: {err}",
            remote_control_target.enroll_url
        ))
    })?;
    let body_preview = preview_remote_control_response_body(&body);
    if !status.is_success() {
        return Err(std::io::Error::other(format!(
            "remote control server enrollment failed at `{}`: HTTP {status}, body: {body_preview}",
            remote_control_target.enroll_url
        )));
    }

    let enrollment = serde_json::from_slice::<EnrollRemoteServerResponse>(&body).map_err(|err| {
        std::io::Error::other(format!(
            "failed to parse remote control enrollment response from `{}`: HTTP {status}, body: {body_preview}, decode error: {err}",
            remote_control_target.enroll_url
        ))
    })?;

    Ok(RemoteControlEnrollment {
        server_id: enrollment.server_id,
        server_name,
    })
}

#[cfg(test)]
#[path = "enrollment_tests.rs"]
mod enrollment_tests;
