use std::collections::HashMap;
use std::collections::VecDeque;
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_utils_pty::ExecCommandSession;
use codex_utils_pty::TerminalSize;
use serde::Serialize;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;
use tokio::sync::Mutex;

use crate::protocol::EXEC_EXITED_METHOD;
use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::EXEC_TERMINATE_METHOD;
use crate::protocol::EXEC_WRITE_METHOD;
use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;

struct RunningProcess {
    session: ExecCommandSession,
    tty: bool,
    stdout_buffer: Arc<StdMutex<BoundedBytesBuffer>>,
    stderr_buffer: Arc<StdMutex<BoundedBytesBuffer>>,
}

#[derive(Debug)]
struct BoundedBytesBuffer {
    max_bytes: usize,
    bytes: VecDeque<u8>,
}

impl BoundedBytesBuffer {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            bytes: VecDeque::with_capacity(max_bytes.min(8192)),
        }
    }

    fn push_chunk(&mut self, chunk: &[u8]) {
        if self.max_bytes == 0 {
            return;
        }
        for byte in chunk {
            self.bytes.push_back(*byte);
            if self.bytes.len() > self.max_bytes {
                self.bytes.pop_front();
            }
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }
}

pub async fn run_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let writer = Arc::new(Mutex::new(BufWriter::new(tokio::io::stdout())));
    let processes = Arc::new(Mutex::new(HashMap::<String, RunningProcess>::new()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let message = serde_json::from_str::<JSONRPCMessage>(&line)?;
        if let JSONRPCMessage::Request(request) = message {
            handle_request(request, &writer, &processes).await;
            continue;
        }

        if let JSONRPCMessage::Notification(notification) = message {
            if notification.method != INITIALIZED_METHOD {
                send_error(
                    &writer,
                    RequestId::Integer(-1),
                    invalid_request(format!(
                        "unexpected notification method: {}",
                        notification.method
                    )),
                )
                .await;
            }
            continue;
        }
    }

    let remaining = {
        let mut processes = processes.lock().await;
        processes
            .drain()
            .map(|(_, process)| process)
            .collect::<Vec<_>>()
    };
    for process in remaining {
        process.session.terminate();
    }

    Ok(())
}

async fn handle_request(
    request: JSONRPCRequest,
    writer: &Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    processes: &Arc<Mutex<HashMap<String, RunningProcess>>>,
) {
    let response = match request.method.as_str() {
        INITIALIZE_METHOD => serde_json::to_value(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
        })
        .map_err(|err| internal_error(err.to_string())),
        EXEC_METHOD => handle_exec_request(request.params, writer, processes).await,
        EXEC_WRITE_METHOD => handle_write_request(request.params, processes).await,
        EXEC_TERMINATE_METHOD => handle_terminate_request(request.params, processes).await,
        other => Err(invalid_request(format!("unknown method: {other}"))),
    };

    match response {
        Ok(result) => {
            send_response(
                writer,
                JSONRPCResponse {
                    id: request.id,
                    result,
                },
            )
            .await;
        }
        Err(err) => {
            send_error(writer, request.id, err).await;
        }
    }
}

async fn handle_exec_request(
    params: Option<serde_json::Value>,
    writer: &Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    processes: &Arc<Mutex<HashMap<String, RunningProcess>>>,
) -> Result<serde_json::Value, JSONRPCErrorError> {
    let params: ExecParams = serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
        .map_err(|err| invalid_params(err.to_string()))?;

    let (program, args) = params
        .argv
        .split_first()
        .ok_or_else(|| invalid_params("argv must not be empty".to_string()))?;
    let resolved_program = resolve_remote_program(program, &params.env);
    if resolved_program != *program {
        tracing::info!(
            original_program = program,
            resolved_program,
            "resolved missing shell path for remote exec"
        );
    }
    let resolved_cwd = resolve_remote_cwd(params.cwd.as_path());

    let spawn_result = if params.tty {
        codex_utils_pty::spawn_pty_process(
            &resolved_program,
            args,
            resolved_cwd.as_path(),
            &params.env,
            &params.arg0,
            TerminalSize::default(),
        )
        .await
    } else {
        codex_utils_pty::spawn_pipe_process_no_stdin(
            &resolved_program,
            args,
            resolved_cwd.as_path(),
            &params.env,
            &params.arg0,
        )
        .await
    };
    let spawned = spawn_result.map_err(|err| {
        let path_value = params.env.get("PATH").map(String::as_str).unwrap_or("");
        eprintln!(
            "exec-server spawn failed: program={resolved_program:?} argv={:?} cwd={:?} arg0={:?} tty={} path={path_value:?} err={err}",
            params.argv,
            resolved_cwd,
            params.arg0,
            params.tty,
        );
        internal_error(err.to_string())
    })?;

    let pid = spawned.session.pid();
    let stdout_buffer = Arc::new(StdMutex::new(BoundedBytesBuffer::new(
        params.output_bytes_cap,
    )));
    let stderr_buffer = Arc::new(StdMutex::new(BoundedBytesBuffer::new(
        params.output_bytes_cap,
    )));

    let process_id = params.process_id.clone();
    {
        let mut process_map = processes.lock().await;
        if process_map.contains_key(&process_id) {
            spawned.session.terminate();
            return Err(invalid_request(format!(
                "process {} already exists",
                params.process_id
            )));
        }
        process_map.insert(
            process_id.clone(),
            RunningProcess {
                session: spawned.session,
                tty: params.tty,
                stdout_buffer: Arc::clone(&stdout_buffer),
                stderr_buffer: Arc::clone(&stderr_buffer),
            },
        );
    }

    tokio::spawn(stream_output(
        process_id.clone(),
        ExecOutputStream::Stdout,
        spawned.stdout_rx,
        Arc::clone(writer),
        Arc::clone(&stdout_buffer),
    ));
    tokio::spawn(stream_output(
        process_id.clone(),
        ExecOutputStream::Stderr,
        spawned.stderr_rx,
        Arc::clone(writer),
        Arc::clone(&stderr_buffer),
    ));
    tokio::spawn(watch_exit(
        process_id.clone(),
        spawned.exit_rx,
        Arc::clone(writer),
        Arc::clone(processes),
    ));

    serde_json::to_value(ExecResponse {
        process_id,
        pid,
        running: true,
        exit_code: None,
        stdout: None,
        stderr: None,
    })
    .map_err(|err| internal_error(err.to_string()))
}

async fn handle_write_request(
    params: Option<serde_json::Value>,
    processes: &Arc<Mutex<HashMap<String, RunningProcess>>>,
) -> Result<serde_json::Value, JSONRPCErrorError> {
    let params: WriteParams = serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
        .map_err(|err| invalid_params(err.to_string()))?;

    let writer_tx = {
        let process_map = processes.lock().await;
        let process = process_map
            .get(&params.process_id)
            .ok_or_else(|| invalid_request(format!("unknown process id {}", params.process_id)))?;
        if !process.tty {
            return Err(invalid_request(format!(
                "stdin is closed for process {}",
                params.process_id
            )));
        }
        process.session.writer_sender()
    };

    writer_tx
        .send(params.chunk.into_inner())
        .await
        .map_err(|_| internal_error("failed to write to process stdin".to_string()))?;

    serde_json::to_value(WriteResponse { accepted: true })
        .map_err(|err| internal_error(err.to_string()))
}

async fn handle_terminate_request(
    params: Option<serde_json::Value>,
    processes: &Arc<Mutex<HashMap<String, RunningProcess>>>,
) -> Result<serde_json::Value, JSONRPCErrorError> {
    let params: TerminateParams = serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
        .map_err(|err| invalid_params(err.to_string()))?;

    let process = {
        let mut process_map = processes.lock().await;
        process_map.remove(&params.process_id)
    };

    if let Some(process) = process {
        process.session.terminate();
        serde_json::to_value(TerminateResponse { running: true })
            .map_err(|err| internal_error(err.to_string()))
    } else {
        serde_json::to_value(TerminateResponse { running: false })
            .map_err(|err| internal_error(err.to_string()))
    }
}

async fn stream_output(
    process_id: String,
    stream: ExecOutputStream,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    writer: Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    buffer: Arc<StdMutex<BoundedBytesBuffer>>,
) {
    while let Some(chunk) = receiver.recv().await {
        if let Ok(mut guard) = buffer.lock() {
            guard.push_chunk(&chunk);
        }
        let notification = ExecOutputDeltaNotification {
            process_id: process_id.clone(),
            stream,
            chunk: chunk.into(),
        };
        if send_notification(&writer, EXEC_OUTPUT_DELTA_METHOD, &notification)
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn watch_exit(
    process_id: String,
    exit_rx: tokio::sync::oneshot::Receiver<i32>,
    writer: Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
) {
    let exit_code = exit_rx.await.unwrap_or(-1);
    let removed = {
        let mut processes = processes.lock().await;
        processes.remove(&process_id)
    };
    if let Some(process) = removed {
        let _ = process.stdout_buffer.lock().map(|buffer| buffer.snapshot());
        let _ = process.stderr_buffer.lock().map(|buffer| buffer.snapshot());
    }
    let _ = send_notification(
        &writer,
        EXEC_EXITED_METHOD,
        &ExecExitedNotification {
            process_id,
            exit_code,
        },
    )
    .await;
}

async fn send_response(
    writer: &Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    response: JSONRPCResponse,
) {
    let _ = send_message(writer, JSONRPCMessage::Response(response)).await;
}

async fn send_error(
    writer: &Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    id: RequestId,
    error: JSONRPCErrorError,
) {
    let _ = send_message(writer, JSONRPCMessage::Error(JSONRPCError { error, id })).await;
}

async fn send_notification<T: Serialize>(
    writer: &Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    method: &str,
    params: &T,
) -> Result<(), serde_json::Error> {
    send_message(
        writer,
        JSONRPCMessage::Notification(JSONRPCNotification {
            method: method.to_string(),
            params: Some(serde_json::to_value(params)?),
        }),
    )
    .await
    .map_err(serde_json::Error::io)
}

async fn send_message(
    writer: &Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    message: JSONRPCMessage,
) -> std::io::Result<()> {
    let encoded =
        serde_json::to_vec(&message).map_err(|err| std::io::Error::other(err.to_string()))?;
    let mut writer = writer.lock().await;
    writer.write_all(&encoded).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

fn invalid_request(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32600,
        data: None,
        message,
    }
}

fn invalid_params(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32602,
        data: None,
        message,
    }
}

fn internal_error(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        data: None,
        message,
    }
}

fn resolve_remote_program(program: &str, env_map: &HashMap<String, String>) -> String {
    let program_path = Path::new(program);
    if !program_path.is_absolute() || program_path.is_file() {
        return program.to_string();
    }

    let Some(program_name) = program_path.file_name().and_then(|name| name.to_str()) else {
        return program.to_string();
    };

    let candidates = match program_name {
        "zsh" => &[
            "zsh",
            "/bin/zsh",
            "/usr/bin/zsh",
            "bash",
            "/bin/bash",
            "/usr/bin/bash",
            "sh",
            "/bin/sh",
            "/usr/bin/sh",
        ][..],
        "bash" => &[
            "bash",
            "/bin/bash",
            "/usr/bin/bash",
            "sh",
            "/bin/sh",
            "/usr/bin/sh",
        ][..],
        "sh" => &["sh", "/bin/sh", "/usr/bin/sh"][..],
        "pwsh" | "pwsh.exe" => &[
            "pwsh",
            "/usr/local/bin/pwsh",
            "/usr/bin/pwsh",
            "powershell",
            "powershell.exe",
        ][..],
        "powershell" | "powershell.exe" => &[
            "powershell",
            "powershell.exe",
            "pwsh",
            "/usr/local/bin/pwsh",
            "/usr/bin/pwsh",
        ][..],
        "cmd" | "cmd.exe" => &["cmd.exe", "cmd"][..],
        _ => return program.to_string(),
    };

    resolve_candidate_from_env(candidates, env_map).unwrap_or_else(|| program.to_string())
}

fn resolve_remote_cwd(cwd: &Path) -> std::path::PathBuf {
    if let Ok(stripped) = cwd.strip_prefix("/private") {
        let stripped_path = Path::new("/").join(stripped);
        if stripped_path.is_dir() {
            return stripped_path;
        }
    }

    if cwd.is_dir() {
        return cwd.to_path_buf();
    }

    cwd.to_path_buf()
}

fn resolve_candidate_from_env(
    candidates: &[&str],
    env_map: &HashMap<String, String>,
) -> Option<String> {
    for candidate in candidates {
        let candidate_path = Path::new(candidate);
        if candidate_path.is_absolute() {
            if candidate_path.is_file() {
                return Some((*candidate).to_string());
            }
            continue;
        }

        if let Some(found) = find_in_path(candidate, env_map) {
            return Some(found);
        }
    }

    None
}

fn find_in_path(binary: &str, env_map: &HashMap<String, String>) -> Option<String> {
    let path_value = env_map
        .get("PATH")
        .cloned()
        .or_else(|| env::var("PATH").ok())?;
    for dir in env::split_paths(&path_value) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::resolve_remote_cwd;
    use super::resolve_remote_program;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    static NEXT_TEST_DIR_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        let seq = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
        path.push(format!("codex-exec-server-shell-resolve-{nanos}-{seq}"));
        fs::create_dir_all(&path).expect("create test dir");
        path
    }

    #[test]
    fn resolve_remote_program_rewrites_missing_zsh_to_remote_shell_family() {
        let test_dir = unique_test_dir();
        let bash_path = test_dir.join("bash");
        fs::write(&bash_path, "#!/bin/sh\n").expect("write fake bash");

        let mut env_map = HashMap::new();
        env_map.insert("PATH".to_string(), test_dir.to_string_lossy().to_string());

        let resolved = resolve_remote_program("/opt/homebrew/bin/zsh", &env_map);
        assert_ne!(resolved, "/opt/homebrew/bin/zsh");
        let resolved_path = PathBuf::from(&resolved);
        let resolved_name = resolved_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("resolved shell name");
        assert!(matches!(resolved_name, "zsh" | "bash" | "sh"));

        fs::remove_dir_all(&test_dir).expect("cleanup test dir");
    }

    #[test]
    fn resolve_remote_program_keeps_existing_absolute_path() {
        let test_dir = unique_test_dir();
        let shell_path = test_dir.join("zsh");
        fs::write(&shell_path, "#!/bin/sh\n").expect("write fake shell");

        let resolved = resolve_remote_program(&shell_path.to_string_lossy(), &HashMap::new());
        assert_eq!(resolved, shell_path.to_string_lossy());

        fs::remove_dir_all(&test_dir).expect("cleanup test dir");
    }

    #[test]
    fn resolve_remote_program_leaves_unknown_missing_binary_unchanged() {
        let resolved = resolve_remote_program("/does/not/exist/python", &HashMap::new());
        assert_eq!(resolved, "/does/not/exist/python");
    }

    #[test]
    fn resolve_remote_cwd_strips_private_prefix_when_needed() {
        let temp_dir = unique_test_dir();
        let nested_dir = temp_dir.join("nested");
        fs::create_dir_all(&nested_dir).expect("create nested dir");
        let private_alias = PathBuf::from(format!("/private{}", nested_dir.to_string_lossy()));

        let resolved = resolve_remote_cwd(&private_alias);

        assert_eq!(resolved, nested_dir);

        fs::remove_dir_all(&temp_dir).expect("cleanup test dir");
    }

    #[test]
    fn resolve_remote_cwd_keeps_existing_directory() {
        let temp_dir = unique_test_dir();

        let resolved = resolve_remote_cwd(&temp_dir);

        assert_eq!(resolved, temp_dir);
        fs::remove_dir_all(&resolved).expect("cleanup test dir");
    }
}
