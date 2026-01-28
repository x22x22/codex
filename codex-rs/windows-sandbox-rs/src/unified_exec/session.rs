//weasel: help me understand the scope of this file. It looks like a high level session/spawn manager? Is it for elevated/legacy/both? Is it specific to unified-exec? Is most of the stuff here new, or moved around?
//! Unified exec session spawner for Windows sandboxing.
//!
//! This module implements the **unified_exec session** paths for Windows by returning a
//! long‑lived `SpawnedProcess` wired for stdin/out/exit. It covers both the legacy
//! restricted‑token path (direct spawn under a restricted token) and the elevated path
//! (spawn via the command runner IPC). It is not used for non‑unified exec capture flows,
//! which continue to use the one‑shot capture APIs.

use crate::acl::add_allow_ace;
use crate::acl::add_deny_write_ace;
use crate::acl::allow_null_device;
use crate::acl::revoke_ace;
use crate::allow::compute_allow_paths;
use crate::allow::AllowDenyPaths;
use crate::cap::load_or_create_cap_sids;
use crate::conpty::spawn_conpty_process_as_user;
use crate::env::apply_no_network_to_env;
use crate::env::ensure_non_interactive_pager;
use crate::env::inherit_path_env;
use crate::env::normalize_null_device_env;
use crate::identity::require_logon_sandbox_creds;
use crate::identity::SandboxCreds;
use crate::ipc_framed::decode_bytes;
use crate::ipc_framed::encode_bytes;
use crate::ipc_framed::read_frame;
use crate::ipc_framed::write_frame;
use crate::ipc_framed::EmptyPayload;
use crate::ipc_framed::FramedMessage;
use crate::ipc_framed::Message;
use crate::ipc_framed::SpawnRequest;
use crate::ipc_framed::StdinPayload;
use crate::logging::log_failure;
use crate::logging::log_start;
use crate::logging::log_success;
use crate::policy::parse_policy;
use crate::policy::SandboxPolicy;
use crate::process::read_handle_loop;
use crate::process::spawn_process_with_pipes;
use crate::process::StderrMode;
use crate::process::StdinMode;
use crate::runner_pipe::connect_pipe;
use crate::runner_pipe::create_named_pipe;
use crate::runner_pipe::find_runner_exe;
use crate::runner_pipe::pipe_pair;
use crate::runner_pipe::PIPE_ACCESS_INBOUND;
use crate::runner_pipe::PIPE_ACCESS_OUTBOUND;
use crate::sandbox_utils::ensure_codex_home_exists;
use crate::token::convert_string_sid_to_sid;
use crate::token::create_readonly_token_with_cap;
use crate::token::create_workspace_write_token_with_cap;
use crate::token::get_current_token_for_restriction;
use crate::token::get_logon_sid_bytes;
use crate::winutil::quote_windows_arg;
use crate::winutil::to_wide;
use anyhow::Result;
use codex_utils_pty::spawn_from_driver;
use codex_utils_pty::ProcessDriver;
use codex_utils_pty::SpawnedProcess;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::Console::ClosePseudoConsole;
use windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode;
use windows_sys::Win32::System::Threading::CreateProcessWithLogonW;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::PROCESS_INFORMATION;
use windows_sys::Win32::System::Threading::TerminateProcess;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::System::Threading::LOGON_WITH_PROFILE;
use windows_sys::Win32::System::Threading::STARTUPINFOW;

const WAIT_TIMEOUT: u32 = 0x0000_0102;

/// Returns true if the sandbox policy requires network blocking.
fn should_apply_network_block(policy: &SandboxPolicy) -> bool {
    !policy.has_full_network_access()
}

struct LegacyContext {
    policy: SandboxPolicy,
    logs_base_dir: Option<PathBuf>,
    is_workspace_write: bool,
}

struct LegacyProcessHandles {
    process: PROCESS_INFORMATION,
    output_join: std::thread::JoinHandle<()>,
    writer_handle: tokio::task::JoinHandle<()>,
    hpc: Option<HANDLE>,
}

fn prepare_legacy_context(
    policy_json_or_preset: &str,
    codex_home: &Path,
    env_map: &mut HashMap<String, String>,
    command: &[String],
) -> Result<LegacyContext> {
    let policy = parse_policy(policy_json_or_preset)?;
    let apply_network_block = should_apply_network_block(&policy);
    normalize_null_device_env(env_map);
    ensure_non_interactive_pager(env_map);
    if apply_network_block {
        apply_no_network_to_env(env_map)?;
    }
    ensure_codex_home_exists(codex_home)?;
    let sandbox_base = codex_home.join(".sandbox");
    std::fs::create_dir_all(&sandbox_base)?;
    let logs_base_dir = Some(sandbox_base.as_path());
    log_start(command, logs_base_dir);
    let is_workspace_write = matches!(&policy, SandboxPolicy::WorkspaceWrite { .. });
    Ok(LegacyContext {
        policy,
        logs_base_dir: logs_base_dir.map(|p| p.to_path_buf()),
        is_workspace_write,
    })
}

fn create_restricted_token_for_policy(
    policy: &SandboxPolicy,
    codex_home: &Path,
) -> Result<(HANDLE, *mut c_void, String)> {
    let caps = load_or_create_cap_sids(codex_home)?;
    let (h_token, psid_to_use, cap_sid_str): (HANDLE, *mut c_void, String) = unsafe {
        match policy {
            SandboxPolicy::ReadOnly => {
                let psid = convert_string_sid_to_sid(&caps.readonly).unwrap();
                let (h_token, psid) = create_readonly_token_with_cap(psid)?;
                (h_token, psid, caps.readonly.clone())
            }
            SandboxPolicy::WorkspaceWrite { .. } => {
                let psid = convert_string_sid_to_sid(&caps.workspace).unwrap();
                let (h_token, psid) = create_workspace_write_token_with_cap(psid)?;
                (h_token, psid, caps.workspace.clone())
            }
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. } => {
                unreachable!("DangerFullAccess handled above")
            }
        }
    };
    Ok((h_token, psid_to_use, cap_sid_str))
}

fn allow_null_device_for_workspace_write(is_workspace_write: bool) {
    if !is_workspace_write {
        return;
    }
    unsafe {
        if let Ok(base) = get_current_token_for_restriction() {
            if let Ok(bytes) = get_logon_sid_bytes(base) {
                let mut tmp = bytes.clone();
                let psid2 = tmp.as_mut_ptr() as *mut c_void;
                allow_null_device(psid2);
            }
            CloseHandle(base);
        }
    }
}

fn apply_legacy_acl_rules(
    policy: &SandboxPolicy,
    sandbox_policy_cwd: &Path,
    current_dir: &Path,
    env_map: &HashMap<String, String>,
    psid_to_use: *mut c_void,
    persist_aces: bool,
) -> Vec<PathBuf> {
    let AllowDenyPaths { allow, deny } =
        compute_allow_paths(policy, sandbox_policy_cwd, current_dir, env_map);
    let mut guards: Vec<PathBuf> = Vec::new();
    unsafe {
        for p in &allow {
            if matches!(add_allow_ace(p, psid_to_use), Ok(true)) && !persist_aces {
                guards.push(p.clone());
            }
        }
        for p in &deny {
            if let Ok(added) = add_deny_write_ace(p, psid_to_use) {
                if added && !persist_aces {
                    guards.push(p.clone());
                }
            }
        }
        allow_null_device(psid_to_use);
    }
    guards
}

fn spawn_legacy_process(
    h_token: HANDLE,
    command: &[String],
    cwd: &Path,
    env_map: &HashMap<String, String>,
    tty: bool,
    output_tx: broadcast::Sender<Vec<u8>>,
    writer_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<LegacyProcessHandles> {
    let (pi, output_join, writer_handle, hpc) = if tty {
        let (pi, conpty) = spawn_conpty_process_as_user(h_token, command, cwd, env_map)?;
        let (hpc, input_write, output_read) = conpty.into_raw();
        unsafe {
            CloseHandle(h_token);
        }
        let output_join = spawn_output_reader(output_read, output_tx);
        let writer_handle = spawn_input_writer(Some(input_write), writer_rx);
        (pi, output_join, writer_handle, Some(hpc))
    } else {
        let pipe_handles = spawn_process_with_pipes(
            h_token,
            command,
            cwd,
            env_map,
            StdinMode::Closed,
            StderrMode::MergeStdout,
        )?;
        unsafe {
            CloseHandle(h_token);
        }
        let output_join = spawn_output_reader(pipe_handles.stdout_read, output_tx);
        let writer_handle = spawn_input_writer(None, writer_rx);
        (pipe_handles.process, output_join, writer_handle, None)
    };
    Ok(LegacyProcessHandles {
        process: pi,
        output_join,
        writer_handle,
        hpc,
    })
}

/// Read process output and forward chunks into a broadcast channel.
fn spawn_output_reader(
    output_read: HANDLE,
    output_tx: broadcast::Sender<Vec<u8>>,
) -> std::thread::JoinHandle<()> {
    read_handle_loop(output_read, move |chunk| {
        let _ = output_tx.send(chunk.to_vec());
    })
}

/// Write stdin chunks from a channel into the child process input handle.
fn spawn_input_writer(
    input_write: Option<HANDLE>,
    mut writer_rx: mpsc::Receiver<Vec<u8>>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        while let Some(bytes) = writer_rx.blocking_recv() {
            let Some(handle) = input_write else {
                continue;
            };
            let mut written: u32 = 0;
            unsafe {
                let _ = WriteFile(
                    handle,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                );
            }
        }
        if let Some(handle) = input_write {
            unsafe {
                CloseHandle(handle);
            }
        }
    })
}

fn launch_runner_pipes(
    cwd: &Path,
    sandbox_creds: &SandboxCreds,
    pipe_in: String,
    pipe_out: String,
) -> Result<(File, File)> {
    let h_pipe_in = create_named_pipe(&pipe_in, PIPE_ACCESS_OUTBOUND)?;
    let h_pipe_out = create_named_pipe(&pipe_out, PIPE_ACCESS_INBOUND)?;
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
    let env_block: Option<Vec<u16>> = None;
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let user_w = to_wide(&sandbox_creds.username);
    let domain_w = to_wide(".");
    let password_w = to_wide(&sandbox_creds.password);
    let _ = unsafe { SetErrorMode(0x0001 | 0x0002) };
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
    let pipe_write = unsafe { File::from_raw_handle(h_pipe_in as _) };
    let pipe_read = unsafe { File::from_raw_handle(h_pipe_out as _) };
    Ok((pipe_write, pipe_read))
}

fn send_spawn_request(pipe_write: &mut File, request: SpawnRequest) -> Result<()> {
    let spawn_request = FramedMessage {
        version: 1,
        message: Message::SpawnRequest {
            payload: Box::new(request),
        },
    };
    write_frame(pipe_write, &spawn_request)?;
    Ok(())
}

fn read_spawn_ready(pipe_read: &mut File) -> Result<()> {
    let first = read_frame(pipe_read)?
        .ok_or_else(|| anyhow::anyhow!("runner pipe closed before spawn_ready"))?;
    match first.message {
        Message::SpawnReady { .. } => Ok(()),
        Message::Error { payload } => Err(anyhow::anyhow!("runner error: {}", payload.message)),
        other => Err(anyhow::anyhow!("unexpected runner message: {other:?}")),
    }
}

fn start_runner_stdin_writer(
    mut writer_rx: mpsc::Receiver<Vec<u8>>,
    mut pipe_write: File,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        while let Some(bytes) = writer_rx.blocking_recv() {
            let msg = FramedMessage {
                version: 1,
                message: Message::Stdin {
                    payload: StdinPayload {
                        data_b64: encode_bytes(&bytes),
                    },
                },
            };
            let _ = write_frame(&mut pipe_write, &msg);
        }
    })
}

fn start_runner_stdout_reader(
    mut pipe_read: File,
    output_tx: broadcast::Sender<Vec<u8>>,
    exit_tx: oneshot::Sender<i32>,
) {
    std::thread::spawn(move || loop {
        let msg = match read_frame(&mut pipe_read) {
            Ok(Some(v)) => v,
            Ok(None) => {
                let _ = exit_tx.send(-1);
                break;
            }
            Err(_err) => {
                let _ = exit_tx.send(-1);
                break;
            }
        };

        match msg.message {
            Message::Output { payload } => {
                if let Ok(data) = decode_bytes(&payload.data_b64) {
                    let _ = output_tx.send(data);
                }
            }
            Message::Exit { payload } => {
                let _ = exit_tx.send(payload.exit_code);
                break;
            }
            Message::Error { payload: _ } => {
                let _ = exit_tx.send(-1);
                break;
            }
            Message::SpawnReady { .. } => {}
            Message::Stdin { .. } => {}
            Message::SpawnRequest { .. } => {}
            Message::Terminate { .. } => {}
        }
    });
}

/// Finalize process exit, emit exit code, and cleanup handles/ACLs.
#[allow(clippy::too_many_arguments)]
fn finalize_exit(
    exit_tx: oneshot::Sender<i32>,
    process_handle: Arc<StdMutex<Option<HANDLE>>>,
    thread_handle: HANDLE,
    output_join: std::thread::JoinHandle<()>,
    guards: Vec<PathBuf>,
    cap_sid: Option<String>,
    logs_base_dir: Option<&Path>,
    command: Vec<String>,
) {
    let exit_code = {
        let mut raw_exit: u32 = 1;
        if let Ok(guard) = process_handle.lock() {
            if let Some(handle) = guard.as_ref() {
                unsafe {
                    GetExitCodeProcess(*handle, &mut raw_exit);
                }
            }
        }
        raw_exit as i32
    };

    // Avoid blocking exit propagation if the output reader gets stuck.
    let _ = exit_tx.send(exit_code);

    let _ = output_join.join();

    unsafe {
        if thread_handle != 0 && thread_handle != INVALID_HANDLE_VALUE {
            CloseHandle(thread_handle);
        }
        if let Ok(mut guard) = process_handle.lock() {
            if let Some(handle) = guard.take() {
                CloseHandle(handle);
            }
        }
    }

    if exit_code == 0 {
        log_success(&command, logs_base_dir);
    } else {
        log_failure(&command, &format!("exit code {}", exit_code), logs_base_dir);
    }

    if !guards.is_empty() {
        if let Some(cap_sid) = cap_sid {
            if let Some(sid) = unsafe { convert_string_sid_to_sid(&cap_sid) } {
                unsafe {
                    for p in guards {
                        revoke_ace(&p, sid);
                    }
                }
            }
        }
    }

    // exit_tx already sent above.
}

#[allow(clippy::too_many_arguments)]
/// Spawn a sandboxed process under a restricted token and return a live session.
/// weasel: this is the "non-elevated" path right? If so, this function should be named as such. Is there an opportunity to merge any part of these two spawn_windows_sandbox_... functions?
pub async fn spawn_windows_sandbox_session_legacy(
    policy_json_or_preset: &str,
    sandbox_policy_cwd: &Path,
    codex_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    mut env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    tty: bool,
) -> Result<SpawnedProcess> {
    let LegacyContext {
        policy,
        logs_base_dir,
        is_workspace_write,
    } = prepare_legacy_context(policy_json_or_preset, codex_home, &mut env_map, &command)?;
    let current_dir = cwd.to_path_buf();

    if matches!(
        &policy,
        SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
    ) {
        anyhow::bail!("DangerFullAccess and ExternalSandbox are not supported for sandboxing")
    }
    let (h_token, psid_to_use, cap_sid_str) =
        create_restricted_token_for_policy(&policy, codex_home)?;
    allow_null_device_for_workspace_write(is_workspace_write);

    let persist_aces = is_workspace_write;
    let guards = apply_legacy_acl_rules(
        &policy,
        sandbox_policy_cwd,
        &current_dir,
        &env_map,
        psid_to_use,
        persist_aces,
    );

    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(128);
    let (output_tx, output_rx) = broadcast::channel::<Vec<u8>>(256);
    let (exit_tx, exit_rx) = oneshot::channel::<i32>();

    let LegacyProcessHandles {
        process: pi,
        output_join,
        writer_handle,
        hpc,
    } = spawn_legacy_process(
        h_token,
        &command,
        cwd,
        &env_map,
        tty,
        output_tx.clone(),
        writer_rx,
    )?;

    let process_handle = Arc::new(StdMutex::new(Some(pi.hProcess)));
    let wait_handle = Arc::clone(&process_handle);
    let command_for_wait = command.clone();
    let cap_sid_for_wait = if persist_aces {
        None
    } else {
        Some(cap_sid_str.clone())
    };
    let guards_for_wait = if persist_aces { Vec::new() } else { guards };
    std::thread::spawn(move || {
        let timeout = timeout_ms.map(|ms| ms as u32).unwrap_or(INFINITE);
        let wait_res = unsafe { WaitForSingleObject(pi.hProcess, timeout) };
        if wait_res == WAIT_TIMEOUT {
            unsafe {
                if let Ok(guard) = wait_handle.lock() {
                    if let Some(handle) = guard.as_ref() {
                        let _ = TerminateProcess(*handle, 1);
                    }
                }
            }
        }
        if let Some(hpc) = hpc {
            unsafe {
                ClosePseudoConsole(hpc);
            }
        }
        finalize_exit(
            exit_tx,
            wait_handle,
            pi.hThread,
            output_join,
            guards_for_wait,
            cap_sid_for_wait,
            logs_base_dir.as_deref(),
            command_for_wait,
        );
    });

    let terminator = {
        let process_handle = Arc::clone(&process_handle);
        Some(Box::new(move || {
            if let Ok(guard) = process_handle.lock() {
                if let Some(handle) = guard.as_ref() {
                    unsafe {
                        let _ = TerminateProcess(*handle, 1);
                    }
                }
            }
        }) as Box<dyn FnMut() + Send + Sync>)
    };

    let driver = ProcessDriver {
        writer_tx,
        output_rx,
        exit_rx,
        terminator,
        writer_handle: Some(writer_handle),
    };

    Ok(spawn_from_driver(driver))
}

#[allow(clippy::too_many_arguments)]
/// Spawn a sandboxed process via the elevated runner IPC path and return a live session.
/// weasel: this method is *MASSIVE* - let's think about how to break some of it into helper functions or something
pub async fn spawn_windows_sandbox_session_elevated(
    policy_json_or_preset: &str,
    sandbox_policy_cwd: &Path,
    codex_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    mut env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    tty: bool,
) -> Result<SpawnedProcess> {
    let _ = timeout_ms;
    let policy = parse_policy(policy_json_or_preset)?;
    normalize_null_device_env(&mut env_map);
    ensure_non_interactive_pager(&mut env_map);
    inherit_path_env(&mut env_map);
    let current_dir = cwd.to_path_buf();
    let sandbox_base = codex_home.join(".sandbox");
    ensure_codex_home_exists(&sandbox_base)?;
    let logs_base_dir = Some(sandbox_base.clone());
    log_start(&command, logs_base_dir.as_deref());
    let sandbox_creds =
        require_logon_sandbox_creds(&policy, sandbox_policy_cwd, cwd, &env_map, codex_home)?;
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
    unsafe {
        allow_null_device(psid_to_use);
    }

    let (pipe_in, pipe_out) = pipe_pair();
    let (mut pipe_write, mut pipe_read) =
        launch_runner_pipes(cwd, &sandbox_creds, pipe_in, pipe_out)?;

    let spawn_request = SpawnRequest {
        command: command.clone(),
        cwd: cwd.to_path_buf(),
        env: env_map.clone(),
        policy_json_or_preset: policy_json_or_preset.to_string(),
        sandbox_policy_cwd: sandbox_policy_cwd.to_path_buf(),
        codex_home: sandbox_base.clone(),
        real_codex_home: codex_home.to_path_buf(),
        cap_sid: cap_sid_str.clone(),
        timeout_ms: None,
        tty,
    };
    send_spawn_request(&mut pipe_write, spawn_request)?;
    read_spawn_ready(&mut pipe_read)?;

    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(128);
    let (output_tx, output_rx) = broadcast::channel::<Vec<u8>>(256);
    let (exit_tx, exit_rx) = oneshot::channel::<i32>();

    let writer_handle = start_runner_stdin_writer(writer_rx, pipe_write.try_clone()?);

    let pipe_write = Arc::new(StdMutex::new(pipe_write));
    let terminator = {
        let pipe_write = Arc::clone(&pipe_write);
        Some(Box::new(move || {
            if let Ok(mut guard) = pipe_write.lock() {
                let msg = FramedMessage {
                    version: 1,
                    message: Message::Terminate {
                        payload: EmptyPayload::default(),
                    },
                };
                let _ = write_frame(&mut *guard, &msg);
            }
        }) as Box<dyn FnMut() + Send + Sync>)
    };

    start_runner_stdout_reader(pipe_read, output_tx, exit_tx);

    Ok(spawn_from_driver(ProcessDriver {
        writer_tx,
        output_rx,
        exit_rx,
        terminator,
        writer_handle: Some(writer_handle),
    }))
}

