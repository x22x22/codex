use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use tokio::process::Command;
use tokio::sync::Mutex;

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
            return Ok(Some(cached_token.token.clone()));
        }

        let token = run_provider_auth_command(&state.config).await?;
        *cached = Some(CachedProviderToken {
            token: token.clone(),
            fetched_at: Instant::now(),
        });
        Ok(Some(token))
    }

    pub(crate) async fn refresh_after_unauthorized(&self) -> Result<Option<bool>> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };

        let mut cached = state.cached_token.lock().await;
        let previous_token = cached.as_ref().map(|token| token.token.clone());
        let token = run_provider_auth_command(&state.config).await?;
        let auth_state_changed = previous_token
            .as_ref()
            .map(|previous_token| previous_token != &token);
        *cached = Some(CachedProviderToken {
            token,
            fetched_at: Instant::now(),
        });
        Ok(auth_state_changed)
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
    token: String,
    fetched_at: Instant,
}

async fn run_provider_auth_command(config: &ModelProviderAuthInfo) -> Result<String> {
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
                config.command, config.timeout_ms
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

    Ok(token)
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn caches_command_output_until_refreshed() {
        let script = ProviderAuthScript::new(&["first-token", "second-token"]).unwrap();
        let provider = ModelProviderInfo {
            name: "Test".to_string(),
            base_url: None,
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: Some(script.auth_config()),
            wire_api: crate::WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        };
        let resolver = ProviderAuthResolver::new(&provider);

        let first = resolver.resolve_token().await.unwrap();
        let second = resolver.resolve_token().await.unwrap();
        let changed = resolver.refresh_after_unauthorized().await.unwrap();
        let refreshed = resolver.resolve_token().await.unwrap();

        assert_eq!(first.as_deref(), Some("first-token"));
        assert_eq!(second.as_deref(), Some("first-token"));
        assert_eq!(changed, Some(true));
        assert_eq!(refreshed.as_deref(), Some("second-token"));
    }

    struct ProviderAuthScript {
        tempdir: TempDir,
        command: String,
        args: Vec<String>,
    }

    impl ProviderAuthScript {
        fn new(tokens: &[&str]) -> Result<Self> {
            let tempdir = tempfile::tempdir()?;
            let token_file = tempdir.path().join("tokens.txt");
            std::fs::write(&token_file, format!("{}\n", tokens.join("\n")))?;

            #[cfg(unix)]
            let (command, args) = {
                let script_path = tempdir.path().join("print-token.sh");
                std::fs::write(
                    &script_path,
                    "#!/bin/sh\nfirst_line=$(sed -n '1p' tokens.txt)\nprintf '%s\\n' \"$first_line\"\ntail -n +2 tokens.txt > tokens.next\nmv tokens.next tokens.txt\n",
                )?;
                let mut permissions = std::fs::metadata(&script_path)?.permissions();
                {
                    use std::os::unix::fs::PermissionsExt;
                    permissions.set_mode(0o755);
                }
                std::fs::set_permissions(&script_path, permissions)?;
                ("./print-token.sh".to_string(), Vec::new())
            };

            #[cfg(windows)]
            let (command, args) = {
                let script_path = tempdir.path().join("print-token.ps1");
                std::fs::write(
                    &script_path,
                    "$lines = Get-Content -Path tokens.txt\nif ($lines.Count -eq 0) { exit 1 }\nWrite-Output $lines[0]\n$lines | Select-Object -Skip 1 | Set-Content -Path tokens.txt\n",
                )?;
                (
                    "powershell".to_string(),
                    vec![
                        "-NoProfile".to_string(),
                        "-ExecutionPolicy".to_string(),
                        "Bypass".to_string(),
                        "-File".to_string(),
                        ".\\print-token.ps1".to_string(),
                    ],
                )
            };

            Ok(Self {
                tempdir,
                command,
                args,
            })
        }

        fn auth_config(&self) -> ModelProviderAuthInfo {
            ModelProviderAuthInfo {
                command: self.command.clone(),
                args: self.args.clone(),
                timeout_ms: 1_000,
                refresh_interval_ms: 60_000,
                cwd: codex_utils_absolute_path::AbsolutePathBuf::try_from(self.tempdir.path())
                    .expect("tempdir should be absolute"),
            }
        }
    }
}
