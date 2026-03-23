mod windows_impl {
    use crate::helper_materialization::resolve_helper_for_launch;
    use crate::helper_materialization::HelperExecutable;
    use crate::ipc_framed::decode_bytes;
    use crate::ipc_framed::read_frame;
    use crate::ipc_framed::write_frame;
    use crate::ipc_framed::FramedMessage;
    use crate::ipc_framed::Message;
    use crate::ipc_framed::OutputStream;
    use crate::ipc_framed::SpawnRequest;
    use crate::logging::log_failure;
    use crate::logging::log_success;
    use crate::spawn_prep::prepare_elevated_spawn_context;
    use crate::winutil::quote_windows_arg;
    use crate::winutil::resolve_sid;
    use crate::winutil::string_from_sid_bytes;
    use crate::winutil::to_wide;
    use anyhow::Result;
    use rand::rngs::SmallRng;
    use rand::Rng;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use std::path::PathBuf;
    use std::ptr;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::PSECURITY_DESCRIPTOR;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode;
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
    use windows_sys::Win32::System::Pipes::CreateNamedPipeW;
    const PIPE_ACCESS_INBOUND: u32 = 0x0000_0001;
    const PIPE_ACCESS_OUTBOUND: u32 = 0x0000_0002;
    use windows_sys::Win32::System::Pipes::PIPE_READMODE_BYTE;
    use windows_sys::Win32::System::Pipes::PIPE_TYPE_BYTE;
    use windows_sys::Win32::System::Pipes::PIPE_WAIT;
    use windows_sys::Win32::System::Threading::CreateProcessWithLogonW;
    use windows_sys::Win32::System::Threading::LOGON_WITH_PROFILE;
    use windows_sys::Win32::System::Threading::PROCESS_INFORMATION;
    use windows_sys::Win32::System::Threading::STARTUPINFOW;

    /// Resolves the command runner path, preferring CODEX_HOME/.sandbox/bin.
    fn find_runner_exe(codex_home: &Path, log_dir: Option<&Path>) -> PathBuf {
        resolve_helper_for_launch(HelperExecutable::CommandRunner, codex_home, log_dir)
    }

    /// Generates a unique named-pipe path used to communicate with the runner process.
    fn pipe_name(suffix: &str) -> String {
        let mut rng = SmallRng::from_entropy();
        format!(r"\\.\pipe\codex-runner-{:x}-{}", rng.gen::<u128>(), suffix)
    }

    /// Creates a named pipe whose DACL only allows the sandbox user to connect.
    fn create_named_pipe(name: &str, access: u32, sandbox_sid: &str) -> io::Result<HANDLE> {
        let sddl = to_wide(format!("D:(A;;GA;;;{sandbox_sid})"));
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut sd,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(unsafe {
                GetLastError() as i32
            }));
        }
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let wide = to_wide(name);
        let h = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                access,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                65536,
                65536,
                0,
                &mut sa as *mut SECURITY_ATTRIBUTES,
            )
        };
        if h == 0 || h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(io::Error::from_raw_os_error(unsafe {
                GetLastError() as i32
            }));
        }
        Ok(h)
    }

    /// Waits for a client connection on the named pipe, tolerating an existing connection.
    fn connect_pipe(h: HANDLE) -> io::Result<()> {
        let ok = unsafe { ConnectNamedPipe(h, ptr::null_mut()) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            const ERROR_PIPE_CONNECTED: u32 = 535;
            if err != ERROR_PIPE_CONNECTED {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
        }
        Ok(())
    }

    pub use crate::windows_impl::CaptureResult;

    fn read_spawn_ready(pipe_read: &mut File) -> Result<()> {
        let msg = read_frame(pipe_read)?
            .ok_or_else(|| anyhow::anyhow!("runner pipe closed before spawn_ready"))?;
        match msg.message {
            Message::SpawnReady { .. } => Ok(()),
            Message::Error { payload } => Err(anyhow::anyhow!("runner error: {}", payload.message)),
            other => Err(anyhow::anyhow!(
                "expected spawn_ready from runner, got {other:?}"
            )),
        }
    }

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
        let sandbox_sid =
            resolve_sid(&elevated.sandbox_creds.username).map_err(|err: anyhow::Error| {
                io::Error::new(io::ErrorKind::PermissionDenied, err.to_string())
            })?;
        let sandbox_sid = string_from_sid_bytes(&sandbox_sid)
            .map_err(|err| io::Error::new(io::ErrorKind::PermissionDenied, err))?;

        let pipe_in_name = pipe_name("in");
        let pipe_out_name = pipe_name("out");
        let h_pipe_in = create_named_pipe(&pipe_in_name, PIPE_ACCESS_OUTBOUND, &sandbox_sid)?;
        let h_pipe_out = create_named_pipe(&pipe_out_name, PIPE_ACCESS_INBOUND, &sandbox_sid)?;

        // Launch runner as sandbox user via CreateProcessWithLogonW.
        let runner_exe = find_runner_exe(codex_home, logs_base_dir);
        let runner_cmdline = runner_exe
            .to_str()
            .map(|s: &str| s.to_string())
            .unwrap_or_else(|| "codex-command-runner.exe".to_string());
        let runner_full_cmd = format!(
            "{} {} {}",
            quote_windows_arg(&runner_cmdline),
            quote_windows_arg(&format!("--pipe-in={pipe_in_name}")),
            quote_windows_arg(&format!("--pipe-out={pipe_out_name}"))
        );
        let mut cmdline_vec: Vec<u16> = to_wide(&runner_full_cmd);
        let exe_w: Vec<u16> = to_wide(&runner_cmdline);
        let cwd_w: Vec<u16> = to_wide(cwd);

        // Minimal CPWL launch: inherit env, no desktop override, no handle inheritance.
        let env_block: Option<Vec<u16>> = None;
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let user_w = to_wide(&elevated.sandbox_creds.username);
        let domain_w = to_wide(".");
        let password_w = to_wide(&elevated.sandbox_creds.password);
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

        if let Err(err) = connect_pipe(h_pipe_in) {
            unsafe {
                CloseHandle(h_pipe_in);
                CloseHandle(h_pipe_out);
                if pi.hThread != 0 {
                    CloseHandle(pi.hThread);
                }
                if pi.hProcess != 0 {
                    CloseHandle(pi.hProcess);
                }
            }
            return Err(err.into());
        }
        if let Err(err) = connect_pipe(h_pipe_out) {
            unsafe {
                CloseHandle(h_pipe_in);
                CloseHandle(h_pipe_out);
                if pi.hThread != 0 {
                    CloseHandle(pi.hThread);
                }
                if pi.hProcess != 0 {
                    CloseHandle(pi.hProcess);
                }
            }
            return Err(err.into());
        }

        let result = (|| -> Result<CaptureResult> {
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
                        codex_home: elevated.common.sandbox_base.clone(),
                        real_codex_home: codex_home.to_path_buf(),
                        cap_sids: elevated.cap_sids.clone(),
                        timeout_ms,
                        tty: false,
                        stdin_open: false,
                        use_private_desktop,
                    }),
                },
            };
            write_frame(&mut pipe_write, &spawn_request)?;
            read_spawn_ready(&mut pipe_read)?;
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

        unsafe {
            if pi.hThread != 0 {
                CloseHandle(pi.hThread);
            }
            if pi.hProcess != 0 {
                CloseHandle(pi.hProcess);
            }
        }

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
