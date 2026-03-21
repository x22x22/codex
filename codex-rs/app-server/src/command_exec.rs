use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use codex_app_server_protocol::CommandExecOutputDeltaNotification;
use codex_app_server_protocol::CommandExecOutputStream;
use codex_app_server_protocol::CommandExecResizeParams;
use codex_app_server_protocol::CommandExecResizeResponse;
use codex_app_server_protocol::CommandExecResponse;
use codex_app_server_protocol::CommandExecTerminalSize;
use codex_app_server_protocol::CommandExecTerminateParams;
use codex_app_server_protocol::CommandExecTerminateResponse;
use codex_app_server_protocol::CommandExecWriteParams;
use codex_app_server_protocol::CommandExecWriteResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ServerNotification;
use codex_core::bytes_to_string_smart;
use codex_core::config::StartedNetworkProxy;
use codex_core::exec::DEFAULT_EXEC_COMMAND_TIMEOUT_MS;
use codex_core::exec::ExecExpiration;
use codex_core::exec::IO_DRAIN_TIMEOUT_MS;
use codex_core::exec::SandboxType;
use codex_core::sandboxing::ExecRequest;
use codex_exec_server::Environment;
use codex_exec_server::ExecOutputStream as ExecutorOutputStream;
use codex_exec_server::ExecParams as ExecutorExecParams;
use codex_exec_server::ExecProcess;
use codex_exec_server::ExecResizeParams as ExecutorExecResizeParams;
use codex_exec_server::ExecWaitParams as ExecutorExecWaitParams;
use codex_exec_server::ReadParams as ExecutorReadParams;
use codex_exec_server::WriteParams as ExecutorWriteParams;
use codex_utils_pty::DEFAULT_OUTPUT_BYTES_CAP;
use codex_utils_pty::TerminalSize;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;

use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_PARAMS_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use crate::outgoing_message::OutgoingMessageSender;

const EXEC_TIMEOUT_EXIT_CODE: i32 = 124;

#[derive(Clone)]
pub(crate) struct CommandExecManager {
    sessions: Arc<Mutex<HashMap<ConnectionProcessId, CommandExecSession>>>,
    next_generated_process_id: Arc<AtomicI64>,
}

impl Default for CommandExecManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_generated_process_id: Arc::new(AtomicI64::new(1)),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ConnectionProcessId {
    connection_id: ConnectionId,
    process_id: InternalProcessId,
}

#[derive(Clone)]
enum CommandExecSession {
    Active {
        environment_id: String,
        control_tx: mpsc::Sender<CommandControlRequest>,
    },
    UnsupportedWindowsSandbox {
        environment_id: String,
    },
}

enum CommandControl {
    Write { delta: Vec<u8>, close_stdin: bool },
    Resize { size: TerminalSize },
    Terminate,
}

struct CommandControlRequest {
    control: CommandControl,
    response_tx: Option<oneshot::Sender<Result<(), JSONRPCErrorError>>>,
}

pub(crate) struct StartCommandExecParams {
    pub(crate) outgoing: Arc<OutgoingMessageSender>,
    pub(crate) request_id: ConnectionRequestId,
    pub(crate) experimental_exec_server_url: Option<String>,
    pub(crate) environment_id: String,
    pub(crate) process_id: Option<String>,
    pub(crate) exec_request: ExecRequest,
    pub(crate) started_network_proxy: Option<StartedNetworkProxy>,
    pub(crate) tty: bool,
    pub(crate) stream_stdin: bool,
    pub(crate) stream_stdout_stderr: bool,
    pub(crate) output_bytes_cap: Option<usize>,
    pub(crate) size: Option<TerminalSize>,
}

struct RunExecutorCommandParams {
    outgoing: Arc<OutgoingMessageSender>,
    request_id: ConnectionRequestId,
    process_id: Option<String>,
    executor_process_id: String,
    executor: Arc<dyn ExecProcess>,
    control_rx: mpsc::Receiver<CommandControlRequest>,
    tty: bool,
    stream_stdin: bool,
    stream_stdout_stderr: bool,
    expiration: ExecExpiration,
    output_bytes_cap: Option<usize>,
}

struct SpawnProcessOutputParams {
    connection_id: ConnectionId,
    process_id: Option<String>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    stdio_timeout_rx: watch::Receiver<bool>,
    outgoing: Arc<OutgoingMessageSender>,
    stream: CommandExecOutputStream,
    stream_output: bool,
    output_bytes_cap: Option<usize>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum InternalProcessId {
    Generated(i64),
    Client(String),
}

trait InternalProcessIdExt {
    fn error_repr(&self) -> String;
}

impl InternalProcessIdExt for InternalProcessId {
    fn error_repr(&self) -> String {
        match self {
            Self::Generated(id) => id.to_string(),
            Self::Client(id) => serde_json::to_string(id).unwrap_or_else(|_| format!("{id:?}")),
        }
    }
}

impl CommandExecManager {
    pub(crate) async fn start(
        &self,
        params: StartCommandExecParams,
    ) -> Result<(), JSONRPCErrorError> {
        let StartCommandExecParams {
            outgoing,
            request_id,
            experimental_exec_server_url,
            environment_id,
            process_id,
            exec_request,
            started_network_proxy,
            tty,
            stream_stdin,
            stream_stdout_stderr,
            output_bytes_cap,
            size,
        } = params;
        if process_id.is_none() && (tty || stream_stdin || stream_stdout_stderr) {
            return Err(invalid_request(
                "command/exec tty or streaming requires a client-supplied processId".to_string(),
            ));
        }
        let process_id = process_id.map_or_else(
            || {
                InternalProcessId::Generated(
                    self.next_generated_process_id
                        .fetch_add(1, Ordering::Relaxed),
                )
            },
            InternalProcessId::Client,
        );
        let process_key = ConnectionProcessId {
            connection_id: request_id.connection_id,
            process_id: process_id.clone(),
        };

        if matches!(exec_request.sandbox, SandboxType::WindowsRestrictedToken) {
            if tty || stream_stdin || stream_stdout_stderr {
                return Err(invalid_request(
                    "streaming command/exec is not supported with windows sandbox".to_string(),
                ));
            }
            if output_bytes_cap != Some(DEFAULT_OUTPUT_BYTES_CAP) {
                return Err(invalid_request(
                    "custom outputBytesCap is not supported with windows sandbox".to_string(),
                ));
            }
            if let InternalProcessId::Client(_) = &process_id {
                let mut sessions = self.sessions.lock().await;
                if sessions.contains_key(&process_key) {
                    return Err(invalid_request(format!(
                        "duplicate active command/exec process id: {}",
                        process_key.process_id.error_repr(),
                    )));
                }
                sessions.insert(
                    process_key.clone(),
                    CommandExecSession::UnsupportedWindowsSandbox {
                        environment_id: environment_id.clone(),
                    },
                );
            }
            let sessions = Arc::clone(&self.sessions);
            tokio::spawn(async move {
                let _started_network_proxy = started_network_proxy;
                match codex_core::sandboxing::execute_env(exec_request, /*stdout_stream*/ None)
                    .await
                {
                    Ok(output) => {
                        outgoing
                            .send_response(
                                request_id,
                                CommandExecResponse {
                                    exit_code: output.exit_code,
                                    stdout: output.stdout.text,
                                    stderr: output.stderr.text,
                                },
                            )
                            .await;
                    }
                    Err(err) => {
                        outgoing
                            .send_error(request_id, internal_error(format!("exec failed: {err}")))
                            .await;
                    }
                }
                sessions.lock().await.remove(&process_key);
            });
            return Ok(());
        }

        let ExecRequest {
            command,
            cwd,
            env,
            expiration,
            sandbox: _sandbox,
            arg0,
            ..
        } = exec_request;

        let stream_stdin = tty || stream_stdin;
        let stream_stdout_stderr = tty || stream_stdout_stderr;
        let (control_tx, control_rx) = mpsc::channel(32);
        let notification_process_id = match &process_id {
            InternalProcessId::Generated(_) => None,
            InternalProcessId::Client(process_id) => Some(process_id.clone()),
        };

        let sessions = Arc::clone(&self.sessions);
        let (program, args) = command
            .split_first()
            .ok_or_else(|| invalid_request("command must not be empty".to_string()))?;
        {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&process_key) {
                return Err(invalid_request(format!(
                    "duplicate active command/exec process id: {}",
                    process_key.process_id.error_repr(),
                )));
            }
            sessions.insert(
                process_key.clone(),
                CommandExecSession::Active {
                    environment_id: environment_id.clone(),
                    control_tx,
                },
            );
        }
        tracing::debug!(
            environment_id = %environment_id,
            process_id = %process_key.process_id.error_repr(),
            "command/exec start"
        );
        let environment = match Environment::create(experimental_exec_server_url).await {
            Ok(environment) => environment,
            Err(err) => {
                self.sessions.lock().await.remove(&process_key);
                return Err(internal_error(format!(
                    "failed to bind environment for command/exec: {err}"
                )));
            }
        };
        let executor = environment.get_executor();
        let executor_process_id = process_key.process_id.error_repr();
        let start_result = executor
            .start(ExecutorExecParams {
                process_id: executor_process_id.clone(),
                argv: {
                    let mut argv = Vec::with_capacity(1 + args.len());
                    argv.push(program.to_string());
                    argv.extend(args.iter().cloned());
                    argv
                },
                cwd,
                env,
                tty,
                stdin: stream_stdin,
                arg0,
            })
            .await;
        let Err(err) = start_result else {
            if tty
                && let Some(size) = size
                && let Err(err) = executor
                    .resize(ExecutorExecResizeParams {
                        process_id: executor_process_id.clone(),
                        size: codex_exec_server::ExecTerminalSize {
                            rows: size.rows,
                            cols: size.cols,
                        },
                    })
                    .await
            {
                let _ = executor.terminate(&executor_process_id).await;
                self.sessions.lock().await.remove(&process_key);
                return Err(internal_error(format!(
                    "failed to initialize command PTY size: {err}"
                )));
            }
            let executor = Arc::clone(&executor);
            tokio::spawn(async move {
                let _started_network_proxy = started_network_proxy;
                run_command_with_executor(RunExecutorCommandParams {
                    outgoing,
                    request_id: request_id.clone(),
                    process_id: notification_process_id,
                    executor_process_id,
                    executor,
                    control_rx,
                    tty,
                    stream_stdin,
                    stream_stdout_stderr,
                    expiration,
                    output_bytes_cap,
                })
                .await;
                sessions.lock().await.remove(&process_key);
            });
            return Ok(());
        };
        self.sessions.lock().await.remove(&process_key);
        return Err(internal_error(format!("failed to spawn command: {err}")));
    }

    pub(crate) async fn write(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecWriteParams,
    ) -> Result<CommandExecWriteResponse, JSONRPCErrorError> {
        if params.delta_base64.is_none() && !params.close_stdin {
            return Err(invalid_params(
                "command/exec/write requires deltaBase64 or closeStdin".to_string(),
            ));
        }

        let delta = match params.delta_base64 {
            Some(delta_base64) => STANDARD
                .decode(delta_base64)
                .map_err(|err| invalid_params(format!("invalid deltaBase64: {err}")))?,
            None => Vec::new(),
        };

        let target_process_id = ConnectionProcessId {
            connection_id: request_id.connection_id,
            process_id: InternalProcessId::Client(params.process_id),
        };
        self.send_control(
            target_process_id,
            CommandControl::Write {
                delta,
                close_stdin: params.close_stdin,
            },
        )
        .await?;

        Ok(CommandExecWriteResponse {})
    }

    pub(crate) async fn terminate(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecTerminateParams,
    ) -> Result<CommandExecTerminateResponse, JSONRPCErrorError> {
        let target_process_id = ConnectionProcessId {
            connection_id: request_id.connection_id,
            process_id: InternalProcessId::Client(params.process_id),
        };
        self.send_control(target_process_id, CommandControl::Terminate)
            .await?;
        Ok(CommandExecTerminateResponse {})
    }

    pub(crate) async fn resize(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecResizeParams,
    ) -> Result<CommandExecResizeResponse, JSONRPCErrorError> {
        let target_process_id = ConnectionProcessId {
            connection_id: request_id.connection_id,
            process_id: InternalProcessId::Client(params.process_id),
        };
        self.send_control(
            target_process_id,
            CommandControl::Resize {
                size: terminal_size_from_protocol(params.size)?,
            },
        )
        .await?;
        Ok(CommandExecResizeResponse {})
    }

    pub(crate) async fn connection_closed(&self, connection_id: ConnectionId) {
        let controls = {
            let mut sessions = self.sessions.lock().await;
            let process_ids = sessions
                .keys()
                .filter(|process_id| process_id.connection_id == connection_id)
                .cloned()
                .collect::<Vec<_>>();
            let mut controls = Vec::with_capacity(process_ids.len());
            for process_id in process_ids {
                if let Some(control) = sessions.remove(&process_id) {
                    controls.push(control);
                }
            }
            controls
        };

        for control in controls {
            match control {
                CommandExecSession::Active {
                    environment_id,
                    control_tx,
                } => {
                    tracing::debug!(
                        environment_id = %environment_id,
                        "command/exec connection closed"
                    );
                    let _ = control_tx
                        .send(CommandControlRequest {
                            control: CommandControl::Terminate,
                            response_tx: None,
                        })
                        .await;
                }
                CommandExecSession::UnsupportedWindowsSandbox { environment_id } => {
                    tracing::debug!(
                        environment_id = %environment_id,
                        "command/exec connection closed for windows sandbox"
                    );
                }
            }
        }
    }

    async fn send_control(
        &self,
        process_id: ConnectionProcessId,
        control: CommandControl,
    ) -> Result<(), JSONRPCErrorError> {
        let session = {
            self.sessions
                .lock()
                .await
                .get(&process_id)
                .cloned()
                .ok_or_else(|| {
                    invalid_request(format!(
                        "no active command/exec for process id {}",
                        process_id.process_id.error_repr(),
                    ))
                })?
        };
        let control_tx = match session {
            CommandExecSession::Active {
                environment_id,
                control_tx,
            } => {
                tracing::debug!(
                    environment_id = %environment_id,
                    "command/exec control"
                );
                control_tx
            }
            CommandExecSession::UnsupportedWindowsSandbox { environment_id } => {
                tracing::debug!(
                    environment_id = %environment_id,
                    "command/exec control rejected for windows sandbox"
                );
                return Err(invalid_request(
                    "command/exec/write, command/exec/terminate, and command/exec/resize are not supported for windows sandbox processes".to_string(),
                ));
            }
        };
        let (response_tx, response_rx) = oneshot::channel();
        let request = CommandControlRequest {
            control,
            response_tx: Some(response_tx),
        };
        control_tx
            .send(request)
            .await
            .map_err(|_| command_no_longer_running_error(&process_id.process_id))?;
        response_rx
            .await
            .map_err(|_| command_no_longer_running_error(&process_id.process_id))?
    }
}

async fn run_command_with_executor(params: RunExecutorCommandParams) {
    let RunExecutorCommandParams {
        outgoing,
        request_id,
        process_id,
        executor_process_id,
        executor,
        control_rx,
        tty,
        stream_stdin,
        stream_stdout_stderr,
        expiration,
        output_bytes_cap,
    } = params;
    let mut control_rx = control_rx;
    let mut control_open = true;
    let expiration = async {
        match expiration {
            ExecExpiration::Timeout(duration) => tokio::time::sleep(duration).await,
            ExecExpiration::DefaultTimeout => {
                tokio::time::sleep(Duration::from_millis(DEFAULT_EXEC_COMMAND_TIMEOUT_MS)).await;
            }
            ExecExpiration::Cancellation(cancel) => {
                cancel.cancelled().await;
            }
        }
    };
    tokio::pin!(expiration);

    let mut after_seq = 0_u64;
    let mut timed_out = false;
    let mut exit_code = -1;
    let mut exit_deadline: Option<Instant> = None;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_observed_num_bytes = 0_usize;
    let mut stderr_observed_num_bytes = 0_usize;

    loop {
        tokio::select! {
            control = control_rx.recv(), if control_open => {
                match control {
                    Some(CommandControlRequest { control, response_tx }) => {
                        let result = match control {
                            CommandControl::Write { delta, close_stdin } => {
                                handle_executor_write(
                                    &executor,
                                    &executor_process_id,
                                    stream_stdin,
                                    delta,
                                    close_stdin,
                                )
                                .await
                            }
                            CommandControl::Resize { size } => {
                                handle_executor_resize(&executor, &executor_process_id, tty, size).await
                            }
                            CommandControl::Terminate => {
                                let result =
                                    handle_executor_terminate(&executor, &executor_process_id)
                                        .await;
                                if result.is_ok()
                                    && let Some(waited_exit_code) = wait_for_executor_exit(
                                        &executor,
                                        &executor_process_id,
                                        /*wait_ms*/ 500,
                                    )
                                    .await
                                {
                                    exit_code = waited_exit_code;
                                    exit_deadline.get_or_insert_with(Instant::now);
                                }
                                result
                            }
                        };
                        if let Some(response_tx) = response_tx {
                            let _ = response_tx.send(result);
                        }
                    }
                    None => {
                        control_open = false;
                        let _ = handle_executor_terminate(&executor, &executor_process_id).await;
                    }
                }
            }
            _ = &mut expiration, if !timed_out => {
                timed_out = true;
                let _ = handle_executor_terminate(&executor, &executor_process_id).await;
                if wait_for_executor_exit(&executor, &executor_process_id, /*wait_ms*/ 50)
                    .await
                    .is_some()
                {
                    exit_code = EXEC_TIMEOUT_EXIT_CODE;
                    exit_deadline.get_or_insert_with(|| {
                        Instant::now() + Duration::from_millis(IO_DRAIN_TIMEOUT_MS)
                    });
                }
            }
            read = executor.read(ExecutorReadParams {
                process_id: executor_process_id.clone(),
                after_seq: Some(after_seq),
                max_bytes: None,
                wait_ms: Some(50),
            }) => {
                let read = match read {
                    Ok(read) => read,
                    Err(err) => {
                        outgoing
                            .send_error(
                                request_id,
                                internal_error(format!("failed to read command output: {err}")),
                            )
                            .await;
                        return;
                    }
                };
                after_seq = read.next_seq;
                for chunk in read.chunks {
                    let output_stream = match chunk.stream {
                        ExecutorOutputStream::Stdout | ExecutorOutputStream::Pty => {
                            CommandExecOutputStream::Stdout
                        }
                        ExecutorOutputStream::Stderr => CommandExecOutputStream::Stderr,
                    };
                    let chunk = chunk.chunk.into_inner();
                    let (buffer, observed_num_bytes) = match output_stream {
                        CommandExecOutputStream::Stdout => {
                            (&mut stdout, &mut stdout_observed_num_bytes)
                        }
                        CommandExecOutputStream::Stderr => {
                            (&mut stderr, &mut stderr_observed_num_bytes)
                        }
                    };
                    let capped_chunk = match output_bytes_cap {
                        Some(output_bytes_cap) => {
                            let capped_chunk_len = output_bytes_cap
                                .saturating_sub(*observed_num_bytes)
                                .min(chunk.len());
                            *observed_num_bytes += capped_chunk_len;
                            &chunk[0..capped_chunk_len]
                        }
                        None => chunk.as_slice(),
                    };
                    let cap_reached = Some(*observed_num_bytes) == output_bytes_cap;
                    if stream_stdout_stderr {
                        if let Some(process_id) = process_id.as_ref() && !capped_chunk.is_empty() {
                            outgoing
                                .send_server_notification_to_connections(
                                    &[request_id.connection_id],
                                    ServerNotification::CommandExecOutputDelta(
                                        CommandExecOutputDeltaNotification {
                                            process_id: process_id.clone(),
                                            stream: output_stream,
                                            delta_base64: STANDARD.encode(capped_chunk),
                                            cap_reached,
                                        },
                                    ),
                                )
                                .await;
                        }
                    } else {
                        buffer.extend_from_slice(capped_chunk);
                    }
                }
                if read.exited {
                    if timed_out {
                        exit_code = EXEC_TIMEOUT_EXIT_CODE;
                    } else {
                        exit_code = read.exit_code.unwrap_or(-1);
                    }
                    exit_deadline.get_or_insert_with(|| {
                        Instant::now() + Duration::from_millis(IO_DRAIN_TIMEOUT_MS)
                    });
                }
            }
        }

        if let Some(deadline) = exit_deadline
            && Instant::now() >= deadline
        {
            break;
        }
        if !control_open && exit_deadline.is_none() && !timed_out {
            continue;
        }
        if exit_deadline.is_some() && Instant::now() < exit_deadline.expect("checked above") {
            continue;
        }
        if exit_deadline.is_none() {
            continue;
        }
    }

    outgoing
        .send_response(
            request_id,
            CommandExecResponse {
                exit_code,
                stdout: bytes_to_string_smart(&stdout),
                stderr: bytes_to_string_smart(&stderr),
            },
        )
        .await;
}

async fn handle_executor_write(
    executor: &Arc<dyn ExecProcess>,
    process_id: &str,
    stream_stdin: bool,
    delta: Vec<u8>,
    close_stdin: bool,
) -> Result<(), JSONRPCErrorError> {
    if !stream_stdin {
        return Err(invalid_request(
            "stdin streaming is not enabled for this command/exec".to_string(),
        ));
    }
    executor
        .write(ExecutorWriteParams {
            process_id: process_id.to_string(),
            chunk: delta.into(),
            close_stdin,
        })
        .await
        .map_err(|err| invalid_request(format!("stdin is already closed: {err}")))?;
    Ok(())
}

async fn handle_executor_resize(
    executor: &Arc<dyn ExecProcess>,
    process_id: &str,
    tty: bool,
    size: TerminalSize,
) -> Result<(), JSONRPCErrorError> {
    if !tty {
        return Err(invalid_request(
            "command/exec resize requires tty: true".to_string(),
        ));
    }
    executor
        .resize(ExecutorExecResizeParams {
            process_id: process_id.to_string(),
            size: codex_exec_server::ExecTerminalSize {
                rows: size.rows,
                cols: size.cols,
            },
        })
        .await
        .map_err(|err| invalid_request(format!("failed to resize PTY: {err}")))?;
    Ok(())
}

async fn handle_executor_terminate(
    executor: &Arc<dyn ExecProcess>,
    process_id: &str,
) -> Result<(), JSONRPCErrorError> {
    executor
        .terminate(process_id)
        .await
        .map_err(|err| invalid_request(format!("failed to terminate command: {err}")))?;
    Ok(())
}

async fn wait_for_executor_exit(
    executor: &Arc<dyn ExecProcess>,
    process_id: &str,
    wait_ms: u64,
) -> Option<i32> {
    let response = executor
        .wait(ExecutorExecWaitParams {
            process_id: process_id.to_string(),
            wait_ms: Some(wait_ms),
        })
        .await
        .ok()?;
    response.exited.then_some(response.exit_code.unwrap_or(-1))
}

fn spawn_process_output(params: SpawnProcessOutputParams) -> tokio::task::JoinHandle<String> {
    let SpawnProcessOutputParams {
        connection_id,
        process_id,
        mut output_rx,
        mut stdio_timeout_rx,
        outgoing,
        stream,
        stream_output,
        output_bytes_cap,
    } = params;
    tokio::spawn(async move {
        let mut buffer: Vec<u8> = Vec::new();
        let mut observed_num_bytes = 0usize;
        loop {
            let chunk = tokio::select! {
                chunk = output_rx.recv() => match chunk {
                    Some(chunk) => chunk,
                    None => break,
                },
                _ = stdio_timeout_rx.wait_for(|&v| v) => break,
            };
            let capped_chunk = match output_bytes_cap {
                Some(output_bytes_cap) => {
                    let capped_chunk_len = output_bytes_cap
                        .saturating_sub(observed_num_bytes)
                        .min(chunk.len());
                    observed_num_bytes += capped_chunk_len;
                    &chunk[0..capped_chunk_len]
                }
                None => chunk.as_slice(),
            };
            let cap_reached = Some(observed_num_bytes) == output_bytes_cap;
            if let (true, Some(process_id)) = (stream_output, process_id.as_ref()) {
                outgoing
                    .send_server_notification_to_connections(
                        &[connection_id],
                        ServerNotification::CommandExecOutputDelta(
                            CommandExecOutputDeltaNotification {
                                process_id: process_id.clone(),
                                stream,
                                delta_base64: STANDARD.encode(capped_chunk),
                                cap_reached,
                            },
                        ),
                    )
                    .await;
            } else if !stream_output {
                buffer.extend_from_slice(capped_chunk);
            }
            if cap_reached {
                break;
            }
        }
        bytes_to_string_smart(&buffer)
    })
}

pub(crate) fn terminal_size_from_protocol(
    size: CommandExecTerminalSize,
) -> Result<TerminalSize, JSONRPCErrorError> {
    if size.rows == 0 || size.cols == 0 {
        return Err(invalid_params(
            "command/exec size rows and cols must be greater than 0".to_string(),
        ));
    }
    Ok(TerminalSize {
        rows: size.rows,
        cols: size.cols,
    })
}

fn command_no_longer_running_error(process_id: &InternalProcessId) -> JSONRPCErrorError {
    invalid_request(format!(
        "command/exec {} is no longer running",
        process_id.error_repr(),
    ))
}

fn invalid_request(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_REQUEST_ERROR_CODE,
        message,
        data: None,
    }
}

fn invalid_params(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_PARAMS_ERROR_CODE,
        message,
        data: None,
    }
}

fn internal_error(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use codex_protocol::config_types::WindowsSandboxLevel;
    use codex_protocol::permissions::FileSystemSandboxPolicy;
    use codex_protocol::permissions::NetworkSandboxPolicy;
    use codex_protocol::protocol::ReadOnlyAccess;
    use codex_protocol::protocol::SandboxPolicy;
    use pretty_assertions::assert_eq;
    #[cfg(not(target_os = "windows"))]
    use tokio::time::Duration;
    #[cfg(not(target_os = "windows"))]
    use tokio::time::timeout;
    #[cfg(not(target_os = "windows"))]
    use tokio_util::sync::CancellationToken;

    use super::*;
    #[cfg(not(target_os = "windows"))]
    use crate::outgoing_message::OutgoingEnvelope;
    #[cfg(not(target_os = "windows"))]
    use crate::outgoing_message::OutgoingMessage;

    fn windows_sandbox_exec_request() -> ExecRequest {
        let sandbox_policy = SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::FullAccess,
            network_access: false,
        };
        ExecRequest {
            command: vec!["cmd".to_string()],
            cwd: PathBuf::from("."),
            env: HashMap::new(),
            network: None,
            expiration: ExecExpiration::DefaultTimeout,
            sandbox: SandboxType::WindowsRestrictedToken,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
            sandbox_permissions: codex_core::sandboxing::SandboxPermissions::UseDefault,
            sandbox_policy: sandbox_policy.clone(),
            file_system_sandbox_policy: FileSystemSandboxPolicy::from(&sandbox_policy),
            network_sandbox_policy: NetworkSandboxPolicy::from(&sandbox_policy),
            justification: None,
            arg0: None,
        }
    }

    #[tokio::test]
    async fn windows_sandbox_streaming_exec_is_rejected() {
        let (tx, _rx) = mpsc::channel(1);
        let manager = CommandExecManager::default();
        let err = manager
            .start(StartCommandExecParams {
                outgoing: Arc::new(OutgoingMessageSender::new(tx)),
                request_id: ConnectionRequestId {
                    connection_id: ConnectionId(1),
                    request_id: codex_app_server_protocol::RequestId::Integer(42),
                },
                experimental_exec_server_url: None,
                environment_id: "test-env".to_string(),
                process_id: Some("proc-42".to_string()),
                exec_request: windows_sandbox_exec_request(),
                started_network_proxy: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: true,
                output_bytes_cap: None,
                size: None,
            })
            .await
            .expect_err("streaming windows sandbox exec should be rejected");

        assert_eq!(err.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(
            err.message,
            "streaming command/exec is not supported with windows sandbox"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn windows_sandbox_non_streaming_exec_uses_execution_path() {
        let (tx, mut rx) = mpsc::channel(1);
        let manager = CommandExecManager::default();
        let request_id = ConnectionRequestId {
            connection_id: ConnectionId(7),
            request_id: codex_app_server_protocol::RequestId::Integer(99),
        };

        manager
            .start(StartCommandExecParams {
                outgoing: Arc::new(OutgoingMessageSender::new(tx)),
                request_id: request_id.clone(),
                experimental_exec_server_url: None,
                environment_id: "test-env".to_string(),
                process_id: Some("proc-99".to_string()),
                exec_request: windows_sandbox_exec_request(),
                started_network_proxy: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: Some(DEFAULT_OUTPUT_BYTES_CAP),
                size: None,
            })
            .await
            .expect("non-streaming windows sandbox exec should start");

        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for outgoing message")
            .expect("channel closed before outgoing message");
        let OutgoingEnvelope::ToConnection {
            connection_id,
            message,
        } = envelope
        else {
            panic!("expected connection-scoped outgoing message");
        };
        assert_eq!(connection_id, request_id.connection_id);
        let OutgoingMessage::Error(error) = message else {
            panic!("expected execution failure to be reported as an error");
        };
        assert_eq!(error.id, request_id.request_id);
        assert!(error.error.message.starts_with("exec failed:"));
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn cancellation_expiration_keeps_process_alive_until_terminated() {
        let (tx, mut rx) = mpsc::channel(4);
        let manager = CommandExecManager::default();
        let request_id = ConnectionRequestId {
            connection_id: ConnectionId(8),
            request_id: codex_app_server_protocol::RequestId::Integer(100),
        };
        let sandbox_policy = SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::FullAccess,
            network_access: false,
        };

        manager
            .start(StartCommandExecParams {
                outgoing: Arc::new(OutgoingMessageSender::new(tx)),
                request_id: request_id.clone(),
                experimental_exec_server_url: None,
                environment_id: "test-env".to_string(),
                process_id: Some("proc-100".to_string()),
                exec_request: ExecRequest {
                    command: vec!["sh".to_string(), "-lc".to_string(), "sleep 30".to_string()],
                    cwd: PathBuf::from("."),
                    env: HashMap::new(),
                    network: None,
                    expiration: ExecExpiration::Cancellation(CancellationToken::new()),
                    sandbox: SandboxType::None,
                    windows_sandbox_level: WindowsSandboxLevel::Disabled,
                    windows_sandbox_private_desktop: false,
                    sandbox_permissions: codex_core::sandboxing::SandboxPermissions::UseDefault,
                    sandbox_policy: sandbox_policy.clone(),
                    file_system_sandbox_policy: FileSystemSandboxPolicy::from(&sandbox_policy),
                    network_sandbox_policy: NetworkSandboxPolicy::from(&sandbox_policy),
                    justification: None,
                    arg0: None,
                },
                started_network_proxy: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: Some(DEFAULT_OUTPUT_BYTES_CAP),
                size: None,
            })
            .await
            .expect("cancellation-based exec should start");

        assert!(
            timeout(Duration::from_millis(250), rx.recv())
                .await
                .is_err(),
            "command/exec should remain active until explicit termination",
        );

        manager
            .terminate(
                request_id.clone(),
                CommandExecTerminateParams {
                    process_id: "proc-100".to_string(),
                },
            )
            .await
            .expect("terminate should succeed");

        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for outgoing message")
            .expect("channel closed before outgoing message");
        let OutgoingEnvelope::ToConnection {
            connection_id,
            message,
        } = envelope
        else {
            panic!("expected connection-scoped outgoing message");
        };
        assert_eq!(connection_id, request_id.connection_id);
        let OutgoingMessage::Response(response) = message else {
            panic!("expected execution response after termination");
        };
        assert_eq!(response.id, request_id.request_id);
        let response: CommandExecResponse =
            serde_json::from_value(response.result).expect("deserialize command/exec response");
        assert_ne!(response.exit_code, 0);
        assert_eq!(response.stdout, "");
        // The deferred response now drains any already-emitted stderr before
        // replying, so shell startup noise is allowed here.
    }

    #[tokio::test]
    async fn windows_sandbox_process_ids_reject_write_requests() {
        let manager = CommandExecManager::default();
        let request_id = ConnectionRequestId {
            connection_id: ConnectionId(11),
            request_id: codex_app_server_protocol::RequestId::Integer(1),
        };
        let process_id = ConnectionProcessId {
            connection_id: request_id.connection_id,
            process_id: InternalProcessId::Client("proc-11".to_string()),
        };
        manager.sessions.lock().await.insert(
            process_id,
            CommandExecSession::UnsupportedWindowsSandbox {
                environment_id: "test-env".to_string(),
            },
        );

        let err = manager
            .write(
                request_id,
                CommandExecWriteParams {
                    process_id: "proc-11".to_string(),
                    delta_base64: Some(STANDARD.encode("hello")),
                    close_stdin: false,
                },
            )
            .await
            .expect_err("windows sandbox process ids should reject command/exec/write");

        assert_eq!(err.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(
            err.message,
            "command/exec/write, command/exec/terminate, and command/exec/resize are not supported for windows sandbox processes"
        );
    }

    #[tokio::test]
    async fn windows_sandbox_process_ids_reject_terminate_requests() {
        let manager = CommandExecManager::default();
        let request_id = ConnectionRequestId {
            connection_id: ConnectionId(12),
            request_id: codex_app_server_protocol::RequestId::Integer(2),
        };
        let process_id = ConnectionProcessId {
            connection_id: request_id.connection_id,
            process_id: InternalProcessId::Client("proc-12".to_string()),
        };
        manager.sessions.lock().await.insert(
            process_id,
            CommandExecSession::UnsupportedWindowsSandbox {
                environment_id: "test-env".to_string(),
            },
        );

        let err = manager
            .terminate(
                request_id,
                CommandExecTerminateParams {
                    process_id: "proc-12".to_string(),
                },
            )
            .await
            .expect_err("windows sandbox process ids should reject command/exec/terminate");

        assert_eq!(err.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(
            err.message,
            "command/exec/write, command/exec/terminate, and command/exec/resize are not supported for windows sandbox processes"
        );
    }

    #[tokio::test]
    async fn dropped_control_request_is_reported_as_not_running() {
        let manager = CommandExecManager::default();
        let request_id = ConnectionRequestId {
            connection_id: ConnectionId(13),
            request_id: codex_app_server_protocol::RequestId::Integer(3),
        };
        let process_id = InternalProcessId::Client("proc-13".to_string());
        let (control_tx, mut control_rx) = mpsc::channel(1);
        manager.sessions.lock().await.insert(
            ConnectionProcessId {
                connection_id: request_id.connection_id,
                process_id: process_id.clone(),
            },
            CommandExecSession::Active {
                environment_id: "test-env".to_string(),
                control_tx,
            },
        );

        tokio::spawn(async move {
            let _request = control_rx
                .recv()
                .await
                .expect("expected queued control request");
        });

        let err = manager
            .terminate(
                request_id,
                CommandExecTerminateParams {
                    process_id: "proc-13".to_string(),
                },
            )
            .await
            .expect_err("dropped control request should be treated as not running");

        assert_eq!(err.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(err.message, "command/exec \"proc-13\" is no longer running");
    }
}
