//weasel: seems like all the changes to this file are around using framed IPC instead of a request file. Is there anything else?
mod windows_impl {
    use crate::acl::allow_null_device;
    use crate::allow::compute_allow_paths;
    use crate::allow::AllowDenyPaths;
    use crate::cap::load_or_create_cap_sids;
    use crate::env::ensure_non_interactive_pager;
    use crate::env::inherit_path_env;
    use crate::env::normalize_null_device_env;
    use crate::identity::require_logon_sandbox_creds;
    use crate::ipc_framed::read_frame;
    use crate::ipc_framed::write_frame;
    use crate::ipc_framed::FramedMessage;
    use crate::ipc_framed::Message;
    use crate::ipc_framed::SpawnRequest;
    use crate::logging::log_failure;
    use crate::logging::log_start;
    use crate::logging::log_success;
    use crate::policy::parse_policy;
    use crate::policy::SandboxPolicy;
    use crate::runner_pipe::connect_pipe;
    use crate::runner_pipe::create_named_pipe;
    use crate::runner_pipe::find_runner_exe;
    use crate::runner_pipe::pipe_pair;
    use crate::runner_pipe::PIPE_ACCESS_INBOUND;
    use crate::runner_pipe::PIPE_ACCESS_OUTBOUND;
    use crate::sandbox_utils::ensure_codex_home_exists;
    use crate::sandbox_utils::inject_git_safe_directory;
    use crate::token::convert_string_sid_to_sid;
    use crate::winutil::quote_windows_arg;
    use crate::winutil::to_wide;
    use anyhow::Result;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode;
    use windows_sys::Win32::System::Threading::CreateProcessWithLogonW;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::System::Threading::INFINITE;
    use windows_sys::Win32::System::Threading::LOGON_WITH_PROFILE;
    use windows_sys::Win32::System::Threading::PROCESS_INFORMATION;
    use windows_sys::Win32::System::Threading::STARTUPINFOW;

    pub use crate::windows_impl::CaptureResult;

    /// Launches the command runner under the sandbox user and captures its output via IPC.
    pub fn run_windows_sandbox_capture(
        policy_json_or_preset: &str,
        sandbox_policy_cwd: &Path,
        codex_home: &Path,
        command: Vec<String>,
        cwd: &Path,
        mut env_map: HashMap<String, String>,
        timeout_ms: Option<u64>,
    ) -> Result<CaptureResult> {
        let policy = parse_policy(policy_json_or_preset)?;
        normalize_null_device_env(&mut env_map);
        ensure_non_interactive_pager(&mut env_map);
        inherit_path_env(&mut env_map);
        inject_git_safe_directory(&mut env_map, cwd);
        let current_dir = cwd.to_path_buf();
        // Use a temp-based log dir that the sandbox user can write.
        let sandbox_base = codex_home.join(".sandbox");
        ensure_codex_home_exists(&sandbox_base)?;

        let logs_base_dir: Option<&Path> = Some(sandbox_base.as_path());
        log_start(&command, logs_base_dir);
        let sandbox_creds =
            require_logon_sandbox_creds(&policy, sandbox_policy_cwd, cwd, &env_map, codex_home)?;
        // Build capability SID for ACL grants.
        if matches!(
            &policy,
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
        ) {
            anyhow::bail!("DangerFullAccess and ExternalSandbox are not supported for sandboxing")
        }
        let caps = load_or_create_cap_sids(codex_home)?;
        let (psid_to_use, cap_sid_str) = match &policy {
            SandboxPolicy::ReadOnly => (
                unsafe { convert_string_sid_to_sid(&caps.readonly).unwrap() },
                caps.readonly.clone(),
            ),
            SandboxPolicy::WorkspaceWrite { .. } => (
                unsafe { convert_string_sid_to_sid(&caps.workspace).unwrap() },
                caps.workspace.clone(),
            ),
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. } => {
                unreachable!("DangerFullAccess handled above")
            }
        };

        let AllowDenyPaths { allow: _, deny: _ } =
            compute_allow_paths(&policy, sandbox_policy_cwd, &current_dir, &env_map);
        // Deny/allow ACEs are now applied during setup; avoid per-command churn.
        unsafe {
            allow_null_device(psid_to_use);
        }

        // Prepare named pipe for runner.
        let (pipe_in, pipe_out) = pipe_pair();
        let h_pipe_in = create_named_pipe(&pipe_in, PIPE_ACCESS_OUTBOUND)?;
        let h_pipe_out = create_named_pipe(&pipe_out, PIPE_ACCESS_INBOUND)?;

        // Launch runner as sandbox user via CreateProcessWithLogonW.
        let runner_exe = find_runner_exe();
        let runner_cmdline = runner_exe
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "codex-command-runner.exe".to_string());
        let runner_full_cmd = format!(
            "{} {} {}",
            quote_windows_arg(&runner_cmdline),
            quote_windows_arg(&format!("--pipe-in={pipe_in}")),
            quote_windows_arg(&format!("--pipe-out={pipe_out}"))
        );
        let mut cmdline_vec: Vec<u16> = to_wide(&runner_full_cmd);
        let exe_w: Vec<u16> = to_wide(&runner_cmdline);
        let cwd_w: Vec<u16> = to_wide(cwd);

        // Minimal CPWL launch: inherit env, no desktop override, no handle inheritance.
        let env_block: Option<Vec<u16>> = None;
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let user_w = to_wide(&sandbox_creds.username);
        let domain_w = to_wide(".");
        let password_w = to_wide(&sandbox_creds.password);
        // Suppress WER/UI popups from the runner process so we can collect exit codes.
        let _ = unsafe { SetErrorMode(0x0001 | 0x0002) }; // SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX

        // Ensure command line buffer is mutable and includes the exe as argv[0].
        let spawn_res = unsafe {
            CreateProcessWithLogonW(
                user_w.as_ptr(),
                domain_w.as_ptr(),
                password_w.as_ptr(),
                LOGON_WITH_PROFILE,
                exe_w.as_ptr(),
                cmdline_vec.as_mut_ptr(),
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_UNICODE_ENVIRONMENT,
                env_block
                    .as_ref()
                    .map(|b| b.as_ptr() as *const c_void)
                    .unwrap_or(ptr::null()),
                cwd_w.as_ptr(),
                &si,
                &mut pi,
            )
        };
        if spawn_res == 0 {
            let err = unsafe { GetLastError() } as i32;
            return Err(anyhow::anyhow!("CreateProcessWithLogonW failed: {}", err));
        }

        connect_pipe(h_pipe_in)?;
        connect_pipe(h_pipe_out)?;
        let mut pipe_write = unsafe { File::from_raw_handle(h_pipe_in as _) };
        let mut pipe_read = unsafe { File::from_raw_handle(h_pipe_out as _) };

        let spawn_request = FramedMessage {
            version: 1,
            message: Message::SpawnRequest {
                payload: Box::new(SpawnRequest {
                    command: command.clone(),
                    cwd: cwd.to_path_buf(),
                    env: env_map.clone(),
                    policy_json_or_preset: policy_json_or_preset.to_string(),
                    sandbox_policy_cwd: sandbox_policy_cwd.to_path_buf(),
                    codex_home: sandbox_base.clone(),
                    real_codex_home: codex_home.to_path_buf(),
                    cap_sid: cap_sid_str.clone(),
                    timeout_ms,
                    //weasel: does tty=true not go through this file at all?
                    tty: false,
                }),
            },
        };
        write_frame(&mut pipe_write, &spawn_request)?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code = 1;
        let mut timed_out = false;
        loop {
            let msg = match read_frame(&mut pipe_read)? {
                Some(v) => v,
                None => break,
            };
            match msg.message {
                Message::SpawnReady { .. } => {}
                Message::Output { payload } => {
                    if let Ok(data) = crate::ipc_framed::decode_bytes(&payload.data_b64) {
                        match payload.stream {
                            crate::ipc_framed::OutputStream::Stdout => {
                                stdout.extend_from_slice(&data);
                            }
                            crate::ipc_framed::OutputStream::Stderr => {
                                stderr.extend_from_slice(&data);
                            }
                        }
                    }
                }
                Message::Exit { payload } => {
                    exit_code = payload.exit_code;
                    timed_out = payload.timed_out;
                    break;
                }
                Message::Error { payload } => {
                    return Err(anyhow::anyhow!("runner error: {}", payload.message));
                }
                Message::Stdin { .. } => {}
                Message::SpawnRequest { .. } => {}
                Message::Terminate { .. } => {}
            }
        }

        let timeout = timeout_ms.map(|ms| ms as u32).unwrap_or(INFINITE);
        let _ = unsafe { WaitForSingleObject(pi.hProcess, timeout) };

        unsafe {
            if pi.hThread != 0 {
                CloseHandle(pi.hThread);
            }
            if pi.hProcess != 0 {
                CloseHandle(pi.hProcess);
            }
        }

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
    }

    #[cfg(test)]
    mod tests {
        use crate::policy::SandboxPolicy;

        fn workspace_policy(network_access: bool) -> SandboxPolicy {
            SandboxPolicy::WorkspaceWrite {
                writable_roots: Vec::new(),
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
            assert!(!SandboxPolicy::ReadOnly.has_full_network_access());
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
    pub fn run_windows_sandbox_capture(
        _policy_json_or_preset: &str,
        _sandbox_policy_cwd: &Path,
        _codex_home: &Path,
        _command: Vec<String>,
        _cwd: &Path,
        _env_map: HashMap<String, String>,
        _timeout_ms: Option<u64>,
    ) -> Result<CaptureResult> {
        bail!("Windows sandbox is only available on Windows")
    }
}

#[cfg(not(target_os = "windows"))]
pub use stub::run_windows_sandbox_capture;
