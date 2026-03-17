use crate::codex::Session;
use crate::codex::TurnContext;
use crate::exec::ExecExpiration;
use crate::sandboxing::CommandSpec;
use crate::sandboxing::SandboxPermissions;
use crate::sandboxing::execute_env;
use crate::sandboxing::merge_permission_profiles;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::SandboxablePreference;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_fs_ops::CODEX_CORE_FS_OPS_ARG1;
use codex_fs_ops::FsError;
use codex_fs_ops::FsErrorKind;
use codex_fs_ops::FsPayload;
use codex_fs_ops::FsResponse;
use codex_protocol::models::PermissionProfile;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const SANDBOXED_FS_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub(crate) enum SandboxedFsError {
    #[error("failed to determine codex executable: {message}")]
    ResolveExe { message: String },
    #[error("sandboxed fs helper timed out while accessing `{path}`")]
    TimedOut { path: PathBuf },
    #[error("sandboxed fs helper exited with code {exit_code} while accessing `{path}`: {message}")]
    ProcessFailed {
        path: PathBuf,
        exit_code: i32,
        message: String,
    },
    #[error("sandboxed fs helper returned invalid output for `{path}`: {message}")]
    InvalidResponse { path: PathBuf, message: String },
    #[error("sandboxed fs helper could not access `{path}`: {error}")]
    Operation { path: PathBuf, error: FsError },
}

impl SandboxedFsError {
    pub(crate) fn operation_error_kind(&self) -> Option<&FsErrorKind> {
        match self {
            Self::Operation { error, .. } => Some(&error.kind),
            _ => None,
        }
    }

    pub(crate) fn operation_error_message(&self) -> Option<&str> {
        match self {
            Self::Operation { error, .. } => Some(error.message.as_str()),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn to_io_error(&self) -> std::io::Error {
        match self {
            Self::Operation { error, .. } => error.to_io_error(),
            Self::TimedOut { .. } => {
                std::io::Error::new(std::io::ErrorKind::TimedOut, self.to_string())
            }
            Self::ResolveExe { .. } | Self::ProcessFailed { .. } | Self::InvalidResponse { .. } => {
                std::io::Error::other(self.to_string())
            }
        }
    }
}

pub(crate) async fn read_bytes(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    path: &Path,
) -> Result<Vec<u8>, SandboxedFsError> {
    let path_buf = path.to_path_buf();
    let payload = run_request(session, turn, "read_bytes", &path_buf).await?;
    let FsPayload::Bytes { base64 } = payload else {
        return Err(SandboxedFsError::InvalidResponse {
            path: path_buf,
            message: "expected bytes payload".to_string(),
        });
    };
    BASE64_STANDARD
        .decode(base64)
        .map_err(|error| SandboxedFsError::InvalidResponse {
            path: path.to_path_buf(),
            message: format!("failed to decode base64 payload: {error}"),
        })
}

#[allow(dead_code)]
pub(crate) async fn read_text(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    path: &Path,
) -> Result<String, SandboxedFsError> {
    let path_buf = path.to_path_buf();
    let payload = run_request(session, turn, "read_text", &path_buf).await?;
    let FsPayload::Text { text } = payload else {
        return Err(SandboxedFsError::InvalidResponse {
            path: path_buf,
            message: "expected text payload".to_string(),
        });
    };
    Ok(text)
}

async fn run_request(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    operation: &str,
    path: &Path,
) -> Result<FsPayload, SandboxedFsError> {
    let exe = resolve_codex_exe(&turn.config.codex_home)?;
    let additional_permissions = effective_granted_permissions(session).await;
    let sandbox_manager = crate::sandboxing::SandboxManager::new();
    let attempt = SandboxAttempt {
        sandbox: sandbox_manager.select_initial(
            &turn.file_system_sandbox_policy,
            turn.network_sandbox_policy,
            SandboxablePreference::Auto,
            turn.windows_sandbox_level,
            /*has_managed_network_requirements*/ false,
        ),
        policy: &turn.sandbox_policy,
        file_system_policy: &turn.file_system_sandbox_policy,
        network_policy: turn.network_sandbox_policy,
        enforce_managed_network: false,
        manager: &sandbox_manager,
        sandbox_cwd: &turn.cwd,
        codex_linux_sandbox_exe: turn.codex_linux_sandbox_exe.as_ref(),
        use_legacy_landlock: turn.features.use_legacy_landlock(),
        windows_sandbox_level: turn.windows_sandbox_level,
        windows_sandbox_private_desktop: turn.config.permissions.windows_sandbox_private_desktop,
    };
    let exec_request = attempt
        .env_for(
            CommandSpec {
                program: exe.to_string_lossy().to_string(),
                args: vec![
                    CODEX_CORE_FS_OPS_ARG1.to_string(),
                    operation.to_string(),
                    path.to_string_lossy().to_string(),
                ],
                cwd: turn.cwd.clone(),
                env: HashMap::new(),
                expiration: ExecExpiration::Timeout(SANDBOXED_FS_TIMEOUT),
                sandbox_permissions: SandboxPermissions::UseDefault,
                additional_permissions,
                justification: None,
            },
            None,
        )
        .map_err(|error| SandboxedFsError::ProcessFailed {
            path: path.to_path_buf(),
            exit_code: -1,
            message: error.to_string(),
        })?;
    let output =
        execute_env(exec_request, None)
            .await
            .map_err(|error| SandboxedFsError::ProcessFailed {
                path: path.to_path_buf(),
                exit_code: -1,
                message: error.to_string(),
            })?;

    if output.timed_out {
        return Err(SandboxedFsError::TimedOut {
            path: path.to_path_buf(),
        });
    }
    if output.exit_code != 0 {
        let stderr = output.stderr.text.trim();
        let stdout = output.stdout.text.trim();
        let message = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            "no error details emitted".to_string()
        };
        return Err(SandboxedFsError::ProcessFailed {
            path: path.to_path_buf(),
            exit_code: output.exit_code,
            message,
        });
    }

    let response: FsResponse =
        serde_json::from_str(output.stdout.text.trim()).map_err(|error| {
            SandboxedFsError::InvalidResponse {
                path: path.to_path_buf(),
                message: format!("failed to parse helper response: {error}"),
            }
        })?;

    match response {
        FsResponse::Success { payload } => Ok(payload),
        FsResponse::Error { error } => Err(SandboxedFsError::Operation {
            path: path.to_path_buf(),
            error,
        }),
    }
}

async fn effective_granted_permissions(session: &Session) -> Option<PermissionProfile> {
    let granted_session_permissions = session.granted_session_permissions().await;
    let granted_turn_permissions = session.granted_turn_permissions().await;
    merge_permission_profiles(
        granted_session_permissions.as_ref(),
        granted_turn_permissions.as_ref(),
    )
}

fn resolve_codex_exe(codex_home: &Path) -> Result<PathBuf, SandboxedFsError> {
    #[cfg(target_os = "windows")]
    {
        Ok(codex_windows_sandbox::resolve_current_exe_for_launch(
            codex_home,
            "codex.exe",
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = codex_home;
        std::env::current_exe().map_err(|error| SandboxedFsError::ResolveExe {
            message: error.to_string(),
        })
    }
}
