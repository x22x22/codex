use crate::codex::Session;
use crate::codex::TurnContext;
use crate::error::CodexErr;
use crate::error::SandboxErr;
use crate::exec::ExecExpiration;
use crate::exec::ExecStdin;
use crate::exec::ExecToolCallRawOutput;
use crate::sandboxing::CommandSpec;
use crate::sandboxing::SandboxPermissions;
use crate::sandboxing::execute_env_raw_output;
use crate::sandboxing::merge_permission_profiles;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::SandboxablePreference;
use codex_fs_ops::CODEX_CORE_FS_OPS_ARG1;
use codex_fs_ops::FsError;
use codex_fs_ops::FsErrorKind;
use codex_protocol::models::PermissionProfile;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const SANDBOXED_FS_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn read_file(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    path: &Path,
) -> Result<Vec<u8>, SandboxedFsError> {
    let output = run_request(session, turn, path, "read", ExecStdin::Closed).await?;
    Ok(output.stdout.text)
}

#[allow(dead_code)]
pub(crate) async fn write_file(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    path: &Path,
    contents: &[u8],
) -> Result<(), SandboxedFsError> {
    run_request(
        session,
        turn,
        path,
        "write",
        ExecStdin::Bytes(contents.to_vec()),
    )
    .await?;
    Ok(())
}

async fn run_request(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    path: &Path,
    operation: &str,
    stdin: ExecStdin,
) -> Result<ExecToolCallRawOutput, SandboxedFsError> {
    let exe = std::env::current_exe().map_err(|error| SandboxedFsError::ResolveExe {
        message: error.to_string(),
    })?;
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
    let mut exec_request = attempt
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
            /*network*/ None,
        )
        .map_err(|error| SandboxedFsError::ProcessFailed {
            path: path.to_path_buf(),
            exit_code: -1,
            message: error.to_string(),
        })?;
    exec_request.stdin = stdin;

    let output = execute_env_raw_output(exec_request, /*stdout_stream*/ None)
        .await
        .map_err(|error| map_exec_error(path, error))?;
    if output.exit_code != 0 {
        return Err(parse_helper_failure(
            path,
            output.exit_code,
            &output.stderr.text,
            &output.stdout.text,
        ));
    }

    Ok(output)
}

async fn effective_granted_permissions(session: &Session) -> Option<PermissionProfile> {
    let granted_session_permissions = session.granted_session_permissions().await;
    let granted_turn_permissions = session.granted_turn_permissions().await;
    merge_permission_profiles(
        granted_session_permissions.as_ref(),
        granted_turn_permissions.as_ref(),
    )
}

fn map_exec_error(path: &Path, error: CodexErr) -> SandboxedFsError {
    match error {
        CodexErr::Sandbox(SandboxErr::Timeout { .. }) => SandboxedFsError::TimedOut {
            path: path.to_path_buf(),
        },
        _ => SandboxedFsError::ProcessFailed {
            path: path.to_path_buf(),
            exit_code: -1,
            message: error.to_string(),
        },
    }
}

fn parse_helper_failure(
    path: &Path,
    exit_code: i32,
    stderr: &[u8],
    stdout: &[u8],
) -> SandboxedFsError {
    if let Ok(error) = serde_json::from_slice::<FsError>(stderr) {
        return SandboxedFsError::Operation {
            path: path.to_path_buf(),
            error,
        };
    }

    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    let message = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else if !stdout.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        "no error details emitted".to_string()
    };

    SandboxedFsError::ProcessFailed {
        path: path.to_path_buf(),
        exit_code,
        message,
    }
}

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
            Self::ResolveExe { .. } | Self::ProcessFailed { .. } => {
                std::io::Error::other(self.to_string())
            }
        }
    }
}
