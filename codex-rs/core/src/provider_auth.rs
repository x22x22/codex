use std::fmt;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use tokio::process::Command;
use tokio::sync::Mutex;

use crate::auth::auth::ExternalAuthRefreshContext;
use crate::auth::auth::ExternalAuthRefreshReason;
use crate::auth::auth::ExternalAuthRefresher;
use crate::auth::auth::ExternalAuthTokens;
use crate::error::CodexErr;
use crate::error::Result;
use crate::model_provider_info::ModelProviderAuthInfo;
use crate::model_provider_info::ModelProviderInfo;

#[derive(Clone, Default)]
pub(crate) struct ProviderAuthResolver {
    state: Option<Arc<ProviderAuthState>>,
}

impl ProviderAuthResolver {
    pub(crate) fn new(provider: &ModelProviderInfo) -> Self {
        Self {
            state: provider
                .auth
                .clone()
                .map(ProviderAuthState::new)
                .map(Arc::new),
        }
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) async fn resolve_token(&self) -> Result<Option<String>> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };

        let mut cached = state.cached_token.lock().await;
        if let Some(cached_token) = cached.as_ref()
            && cached_token.fetched_at.elapsed() < state.config.refresh_interval()
        {
            return Ok(Some(cached_token.tokens.access_token.clone()));
        }

        let tokens = run_provider_auth_command(&state.config).await?;
        *cached = Some(CachedProviderToken {
            tokens: tokens.clone(),
            fetched_at: Instant::now(),
        });
        Ok(Some(tokens.access_token))
    }

    pub(crate) async fn refresh_after_unauthorized(&self) -> Result<Option<bool>> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };

        let previous_token = state
            .cached_token
            .lock()
            .await
            .as_ref()
            .map(|token| token.tokens.access_token.clone());
        let refreshed = ExternalAuthRefresher::refresh(
            self,
            ExternalAuthRefreshContext {
                reason: ExternalAuthRefreshReason::Unauthorized,
                previous_account_id: None,
            },
        )
        .await
        .map_err(map_provider_refresh_error)?;
        let auth_state_changed = previous_token
            .as_ref()
            .map(|previous_token| previous_token != &refreshed.access_token);
        Ok(auth_state_changed)
    }
}

#[async_trait::async_trait]
impl ExternalAuthRefresher for ProviderAuthResolver {
    async fn refresh(
        &self,
        _context: ExternalAuthRefreshContext,
    ) -> io::Result<ExternalAuthTokens> {
        let Some(state) = self.state.as_ref() else {
            return Err(io::Error::other(
                "provider auth refresher is not configured",
            ));
        };

        let tokens = run_provider_auth_command(&state.config)
            .await
            .map_err(codex_err_to_io)?;
        let mut cached = state.cached_token.lock().await;
        *cached = Some(CachedProviderToken {
            tokens: tokens.clone(),
            fetched_at: Instant::now(),
        });
        Ok(tokens)
    }
}

impl fmt::Debug for ProviderAuthResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderAuthResolver")
            .field("configured", &self.is_configured())
            .finish()
    }
}

struct ProviderAuthState {
    config: ModelProviderAuthInfo,
    cached_token: Mutex<Option<CachedProviderToken>>,
}

impl ProviderAuthState {
    fn new(config: ModelProviderAuthInfo) -> Self {
        Self {
            config,
            cached_token: Mutex::new(None),
        }
    }
}

struct CachedProviderToken {
    tokens: ExternalAuthTokens,
    fetched_at: Instant,
}

async fn run_provider_auth_command(config: &ModelProviderAuthInfo) -> Result<ExternalAuthTokens> {
    let program = resolve_provider_auth_program(&config.command, &config.cwd)?;
    let mut command = Command::new(&program);
    command
        .args(&config.args)
        .current_dir(config.cwd.as_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = tokio::time::timeout(config.timeout(), command.output())
        .await
        .map_err(|_| {
            CodexErr::InvalidRequest(format!(
                "provider auth command `{}` timed out after {} ms",
                config.command,
                config.timeout_ms.get()
            ))
        })?
        .map_err(|err| {
            CodexErr::InvalidRequest(format!(
                "provider auth command `{}` failed to start: {err}",
                config.command
            ))
        })?;

    if !output.status.success() {
        let status = output.status;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stderr_suffix = if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        };
        return Err(CodexErr::InvalidRequest(format!(
            "provider auth command `{}` exited with status {status}{stderr_suffix}",
            config.command
        )));
    }

    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        CodexErr::InvalidRequest(format!(
            "provider auth command `{}` wrote non-UTF-8 data to stdout",
            config.command
        ))
    })?;
    let token = stdout.trim().to_string();
    if token.is_empty() {
        return Err(CodexErr::InvalidRequest(format!(
            "provider auth command `{}` produced an empty token",
            config.command
        )));
    }

    Ok(ExternalAuthTokens::access_token_only(token))
}

fn resolve_provider_auth_program(command: &str, cwd: &Path) -> Result<PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() || path.components().count() > 1 {
        return Ok(
            codex_utils_absolute_path::AbsolutePathBuf::resolve_path_against_base(path, cwd)?
                .into_path_buf(),
        );
    }

    Ok(PathBuf::from(command))
}

fn codex_err_to_io(error: CodexErr) -> io::Error {
    io::Error::other(error.to_string())
}

fn map_provider_refresh_error(error: io::Error) -> CodexErr {
    CodexErr::InvalidRequest(error.to_string())
}

#[cfg(test)]
#[path = "provider_auth_tests.rs"]
mod tests;
