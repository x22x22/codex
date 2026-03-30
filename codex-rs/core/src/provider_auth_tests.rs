use super::*;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
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

#[tokio::test]
async fn refresh_returns_bearer_only_external_auth_tokens() {
    let script = ProviderAuthScript::new(&["first-token"]).unwrap();
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

    let tokens = crate::auth::auth::ExternalAuthRefresher::refresh(
        &resolver,
        crate::auth::auth::ExternalAuthRefreshContext {
            reason: crate::auth::auth::ExternalAuthRefreshReason::Unauthorized,
            previous_account_id: Some("ignored".to_string()),
        },
    )
    .await
    .unwrap();

    assert_eq!(tokens.access_token, "first-token");
    assert_eq!(tokens.chatgpt_metadata, None);
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
            timeout_ms: non_zero_u64(/*value*/ 1_000),
            refresh_interval_ms: non_zero_u64(/*value*/ 60_000),
            cwd: match codex_utils_absolute_path::AbsolutePathBuf::try_from(self.tempdir.path()) {
                Ok(cwd) => cwd,
                Err(err) => panic!("tempdir should be absolute: {err}"),
            },
        }
    }
}

fn non_zero_u64(value: u64) -> NonZeroU64 {
    match NonZeroU64::new(value) {
        Some(value) => value,
        None => panic!("expected non-zero value: {value}"),
    }
}
