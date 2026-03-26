mod windows_impl {
    use crate::ipc_framed::decode_bytes;
    use crate::ipc_framed::read_frame;
    use crate::ipc_framed::Message;
    use crate::ipc_framed::OutputStream;
    use crate::ipc_framed::SpawnRequest;
    use crate::logging::log_failure;
    use crate::logging::log_success;
    use crate::runner_client::spawn_runner_transport;
    use crate::spawn_prep::prepare_elevated_spawn_context;
    use anyhow::Result;
    use std::collections::HashMap;
    use std::path::Path;

    pub use crate::windows_impl::CaptureResult;

    /// Launches the command runner under the sandbox user and captures its output.
    #[allow(clippy::too_many_arguments)]
    pub fn run_windows_sandbox_capture(
        policy_json_or_preset: &str,
        sandbox_policy_cwd: &Path,
        codex_home: &Path,
        command: Vec<String>,
        cwd: &Path,
        mut env_map: HashMap<String, String>,
        timeout_ms: Option<u64>,
        use_private_desktop: bool,
    ) -> Result<CaptureResult> {
        let elevated = prepare_elevated_spawn_context(
            policy_json_or_preset,
            sandbox_policy_cwd,
            codex_home,
            cwd,
            &mut env_map,
            &command,
        )?;
        let logs_base_dir: Option<&Path> = Some(elevated.common.sandbox_base.as_path());
        let mut transport =
            spawn_runner_transport(codex_home, cwd, &elevated.sandbox_creds, logs_base_dir)?;

        let result = (|| -> Result<CaptureResult> {
            transport.send_spawn_request(SpawnRequest {
                command: command.clone(),
                cwd: cwd.to_path_buf(),
                env: env_map.clone(),
                policy_json_or_preset: policy_json_or_preset.to_string(),
                sandbox_policy_cwd: sandbox_policy_cwd.to_path_buf(),
                codex_home: elevated.common.sandbox_base.clone(),
                real_codex_home: codex_home.to_path_buf(),
                cap_sids: elevated.cap_sids.clone(),
                timeout_ms,
                tty: false,
                stdin_open: false,
                use_private_desktop,
            })?;
            transport.read_spawn_ready()?;
            let (pipe_write, mut pipe_read) = transport.into_files();
            drop(pipe_write);

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let (exit_code, timed_out) = loop {
                let msg = read_frame(&mut pipe_read)?
                    .ok_or_else(|| anyhow::anyhow!("runner pipe closed before exit"))?;
                match msg.message {
                    Message::SpawnReady { .. } => {}
                    Message::Output { payload } => {
                        let bytes = decode_bytes(&payload.data_b64)?;
                        match payload.stream {
                            OutputStream::Stdout => stdout.extend_from_slice(&bytes),
                            OutputStream::Stderr => stderr.extend_from_slice(&bytes),
                        }
                    }
                    Message::Stdin { .. } => {}
                    Message::CloseStdin { .. } => {}
                    Message::Resize { .. } => {}
                    Message::Exit { payload } => break (payload.exit_code, payload.timed_out),
                    Message::Error { payload } => {
                        return Err(anyhow::anyhow!("runner error: {}", payload.message));
                    }
                    other => {
                        return Err(anyhow::anyhow!(
                            "unexpected runner message during capture: {other:?}"
                        ));
                    }
                }
            };

            if exit_code == 0 {
                log_success(&command, logs_base_dir);
            } else {
                log_failure(&command, &format!("exit code {}", exit_code), logs_base_dir);
            }

            Ok(CaptureResult {
                exit_code,
                stdout,
                stderr,
                timed_out,
            })
        })();

        result
    }

    #[cfg(test)]
    mod tests {
        use crate::policy::SandboxPolicy;

        fn workspace_policy(network_access: bool) -> SandboxPolicy {
            SandboxPolicy::WorkspaceWrite {
                writable_roots: Vec::new(),
                read_only_access: Default::default(),
                network_access,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }
        }

        #[test]
        fn applies_network_block_when_access_is_disabled() {
            assert!(!workspace_policy(false).has_full_network_access());
        }

        #[test]
        fn skips_network_block_when_access_is_allowed() {
            assert!(workspace_policy(true).has_full_network_access());
        }

        #[test]
        fn applies_network_block_for_read_only() {
            assert!(!SandboxPolicy::new_read_only_policy().has_full_network_access());
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::run_windows_sandbox_capture;

#[cfg(not(target_os = "windows"))]
mod stub {
    use anyhow::bail;
    use anyhow::Result;
    use codex_protocol::protocol::SandboxPolicy;
    use std::collections::HashMap;
    use std::path::Path;

    #[derive(Debug, Default)]
    pub struct CaptureResult {
        pub exit_code: i32,
        pub stdout: Vec<u8>,
        pub stderr: Vec<u8>,
        pub timed_out: bool,
    }

    /// Stub implementation for non-Windows targets; sandboxing only works on Windows.
    #[allow(clippy::too_many_arguments)]
    pub fn run_windows_sandbox_capture(
        _policy_json_or_preset: &str,
        _sandbox_policy_cwd: &Path,
        _codex_home: &Path,
        _command: Vec<String>,
        _cwd: &Path,
        _env_map: HashMap<String, String>,
        _timeout_ms: Option<u64>,
        _use_private_desktop: bool,
    ) -> Result<CaptureResult> {
        bail!("Windows sandbox is only available on Windows")
    }
}

#[cfg(not(target_os = "windows"))]
pub use stub::run_windows_sandbox_capture;
