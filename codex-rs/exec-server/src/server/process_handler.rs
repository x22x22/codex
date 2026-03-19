use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use codex_app_server_protocol::JSONRPCErrorError;
use codex_utils_pty::ExecCommandSession;
use codex_utils_pty::TerminalSize;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tracing::warn;

use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::ProcessOutputChunk;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;
use crate::rpc::RpcNotificationSender;
use crate::rpc::internal_error;
use crate::rpc::invalid_params;
use crate::rpc::invalid_request;

const RETAINED_OUTPUT_BYTES_PER_PROCESS: usize = 1024 * 1024;
#[cfg(test)]
const EXITED_PROCESS_RETENTION: Duration = Duration::from_millis(25);
#[cfg(not(test))]
const EXITED_PROCESS_RETENTION: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct RetainedOutputChunk {
    seq: u64,
    stream: ExecOutputStream,
    chunk: Vec<u8>,
}

struct RunningProcess {
    session: ExecCommandSession,
    tty: bool,
    output: VecDeque<RetainedOutputChunk>,
    retained_bytes: usize,
    next_seq: u64,
    exit_code: Option<i32>,
    output_notify: Arc<Notify>,
}

enum ProcessEntry {
    Starting,
    Running(Box<RunningProcess>),
}

#[derive(Clone)]
pub(crate) struct ProcessHandler {
    notifications: RpcNotificationSender,
    processes: Arc<Mutex<HashMap<String, ProcessEntry>>>,
}

impl ProcessHandler {
    pub(crate) fn new(notifications: RpcNotificationSender) -> Self {
        Self {
            notifications,
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn shutdown(&self) {
        let remaining = {
            let mut processes = self.processes.lock().await;
            processes
                .drain()
                .filter_map(|(_, process)| match process {
                    ProcessEntry::Starting => None,
                    ProcessEntry::Running(process) => Some(process),
                })
                .collect::<Vec<_>>()
        };
        for process in remaining {
            process.session.terminate();
        }
    }

    pub(crate) async fn exec(&self, params: ExecParams) -> Result<ExecResponse, JSONRPCErrorError> {
        let process_id = params.process_id.clone();

        let (program, args) = params
            .argv
            .split_first()
            .ok_or_else(|| invalid_params("argv must not be empty".to_string()))?;

        {
            let mut process_map = self.processes.lock().await;
            if process_map.contains_key(&process_id) {
                return Err(invalid_request(format!(
                    "process {process_id} already exists"
                )));
            }
            process_map.insert(process_id.clone(), ProcessEntry::Starting);
        }

        let spawned_result = if params.tty {
            codex_utils_pty::spawn_pty_process(
                program,
                args,
                params.cwd.as_path(),
                &params.env,
                &params.arg0,
                TerminalSize::default(),
            )
            .await
        } else {
            codex_utils_pty::spawn_pipe_process_no_stdin(
                program,
                args,
                params.cwd.as_path(),
                &params.env,
                &params.arg0,
            )
            .await
        };
        let spawned = match spawned_result {
            Ok(spawned) => spawned,
            Err(err) => {
                let mut process_map = self.processes.lock().await;
                if matches!(process_map.get(&process_id), Some(ProcessEntry::Starting)) {
                    process_map.remove(&process_id);
                }
                return Err(internal_error(err.to_string()));
            }
        };

        let output_notify = Arc::new(Notify::new());
        {
            let mut process_map = self.processes.lock().await;
            process_map.insert(
                process_id.clone(),
                ProcessEntry::Running(Box::new(RunningProcess {
                    session: spawned.session,
                    tty: params.tty,
                    output: VecDeque::new(),
                    retained_bytes: 0,
                    next_seq: 1,
                    exit_code: None,
                    output_notify: Arc::clone(&output_notify),
                })),
            );
        }

        tokio::spawn(stream_output(
            process_id.clone(),
            if params.tty {
                ExecOutputStream::Pty
            } else {
                ExecOutputStream::Stdout
            },
            spawned.stdout_rx,
            self.notifications.clone(),
            Arc::clone(&self.processes),
            Arc::clone(&output_notify),
        ));
        tokio::spawn(stream_output(
            process_id.clone(),
            if params.tty {
                ExecOutputStream::Pty
            } else {
                ExecOutputStream::Stderr
            },
            spawned.stderr_rx,
            self.notifications.clone(),
            Arc::clone(&self.processes),
            Arc::clone(&output_notify),
        ));
        tokio::spawn(watch_exit(
            process_id.clone(),
            spawned.exit_rx,
            self.notifications.clone(),
            Arc::clone(&self.processes),
            output_notify,
        ));

        Ok(ExecResponse { process_id })
    }

    pub(crate) async fn exec_read(
        &self,
        params: ReadParams,
    ) -> Result<ReadResponse, JSONRPCErrorError> {
        let after_seq = params.after_seq.unwrap_or(0);
        let max_bytes = params.max_bytes.unwrap_or(usize::MAX);
        let wait = Duration::from_millis(params.wait_ms.unwrap_or(0));
        let deadline = tokio::time::Instant::now() + wait;

        loop {
            let (response, output_notify) = {
                let process_map = self.processes.lock().await;
                let process = process_map.get(&params.process_id).ok_or_else(|| {
                    invalid_request(format!("unknown process id {}", params.process_id))
                })?;
                let ProcessEntry::Running(process) = process else {
                    return Err(invalid_request(format!(
                        "process id {} is starting",
                        params.process_id
                    )));
                };

                let mut chunks = Vec::new();
                let mut total_bytes = 0;
                let mut next_seq = process.next_seq;
                for retained in process.output.iter().filter(|chunk| chunk.seq > after_seq) {
                    let chunk_len = retained.chunk.len();
                    if !chunks.is_empty() && total_bytes + chunk_len > max_bytes {
                        break;
                    }
                    total_bytes += chunk_len;
                    chunks.push(ProcessOutputChunk {
                        seq: retained.seq,
                        stream: retained.stream,
                        chunk: retained.chunk.clone().into(),
                    });
                    next_seq = retained.seq + 1;
                    if total_bytes >= max_bytes {
                        break;
                    }
                }

                (
                    ReadResponse {
                        chunks,
                        next_seq,
                        exited: process.exit_code.is_some(),
                        exit_code: process.exit_code,
                    },
                    Arc::clone(&process.output_notify),
                )
            };

            if !response.chunks.is_empty()
                || response.exited
                || tokio::time::Instant::now() >= deadline
            {
                return Ok(response);
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(response);
            }
            let _ = tokio::time::timeout(remaining, output_notify.notified()).await;
        }
    }

    pub(crate) async fn exec_write(
        &self,
        params: WriteParams,
    ) -> Result<WriteResponse, JSONRPCErrorError> {
        let writer_tx = {
            let process_map = self.processes.lock().await;
            let process = process_map.get(&params.process_id).ok_or_else(|| {
                invalid_request(format!("unknown process id {}", params.process_id))
            })?;
            let ProcessEntry::Running(process) = process else {
                return Err(invalid_request(format!(
                    "process id {} is starting",
                    params.process_id
                )));
            };
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

        Ok(WriteResponse { accepted: true })
    }

    pub(crate) async fn terminate(
        &self,
        params: TerminateParams,
    ) -> Result<TerminateResponse, JSONRPCErrorError> {
        let running = {
            let process_map = self.processes.lock().await;
            match process_map.get(&params.process_id) {
                Some(ProcessEntry::Running(process)) => {
                    if process.exit_code.is_some() {
                        return Ok(TerminateResponse { running: false });
                    }
                    process.session.terminate();
                    true
                }
                Some(ProcessEntry::Starting) | None => false,
            }
        };

        Ok(TerminateResponse { running })
    }
}

async fn stream_output(
    process_id: String,
    stream: ExecOutputStream,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    notifications: RpcNotificationSender,
    processes: Arc<Mutex<HashMap<String, ProcessEntry>>>,
    output_notify: Arc<Notify>,
) {
    while let Some(chunk) = receiver.recv().await {
        let notification = {
            let mut processes = processes.lock().await;
            let Some(entry) = processes.get_mut(&process_id) else {
                break;
            };
            let ProcessEntry::Running(process) = entry else {
                break;
            };
            let seq = process.next_seq;
            process.next_seq += 1;
            process.retained_bytes += chunk.len();
            process.output.push_back(RetainedOutputChunk {
                seq,
                stream,
                chunk: chunk.clone(),
            });
            while process.retained_bytes > RETAINED_OUTPUT_BYTES_PER_PROCESS {
                let Some(evicted) = process.output.pop_front() else {
                    break;
                };
                process.retained_bytes = process.retained_bytes.saturating_sub(evicted.chunk.len());
                warn!(
                    "retained output cap exceeded for process {process_id}; dropping oldest output"
                );
            }
            ExecOutputDeltaNotification {
                process_id: process_id.clone(),
                stream,
                chunk: chunk.into(),
            }
        };
        output_notify.notify_waiters();

        if notifications
            .notify(crate::protocol::EXEC_OUTPUT_DELTA_METHOD, &notification)
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
    notifications: RpcNotificationSender,
    processes: Arc<Mutex<HashMap<String, ProcessEntry>>>,
    output_notify: Arc<Notify>,
) {
    let exit_code = exit_rx.await.unwrap_or(-1);
    {
        let mut processes = processes.lock().await;
        if let Some(ProcessEntry::Running(process)) = processes.get_mut(&process_id) {
            process.exit_code = Some(exit_code);
        }
    }
    output_notify.notify_waiters();
    if notifications
        .notify(
            crate::protocol::EXEC_EXITED_METHOD,
            &ExecExitedNotification {
                process_id: process_id.clone(),
                exit_code,
            },
        )
        .await
        .is_err()
    {
        return;
    }

    tokio::time::sleep(EXITED_PROCESS_RETENTION).await;
    let mut processes = processes.lock().await;
    if matches!(
        processes.get(&process_id),
        Some(ProcessEntry::Running(process)) if process.exit_code == Some(exit_code)
    ) {
        processes.remove(&process_id);
    }
}
