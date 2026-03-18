use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use codex_app_server_protocol::FsCopyParams;
use codex_app_server_protocol::FsCopyResponse;
use codex_app_server_protocol::FsCreateDirectoryParams;
use codex_app_server_protocol::FsCreateDirectoryResponse;
use codex_app_server_protocol::FsGetMetadataParams;
use codex_app_server_protocol::FsGetMetadataResponse;
use codex_app_server_protocol::FsReadDirectoryParams;
use codex_app_server_protocol::FsReadDirectoryResponse;
use codex_app_server_protocol::FsReadFileParams;
use codex_app_server_protocol::FsReadFileResponse;
use codex_app_server_protocol::FsRemoveParams;
use codex_app_server_protocol::FsRemoveResponse;
use codex_app_server_protocol::FsWriteFileParams;
use codex_app_server_protocol::FsWriteFileResponse;
use codex_utils_pty::ExecCommandSession;
use codex_utils_pty::TerminalSize;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tracing::warn;

use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecResponse;
use crate::protocol::ExecSandboxMode;
use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::ProcessOutputChunk;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteResponse;
use crate::server::filesystem::ExecServerFileSystem;
use crate::server::routing::ExecServerOutboundMessage;
use crate::server::routing::ExecServerServerNotification;
use crate::server::routing::internal_error;
use crate::server::routing::invalid_params;
use crate::server::routing::invalid_request;

const RETAINED_OUTPUT_BYTES_PER_PROCESS: usize = 1024 * 1024;

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

pub(crate) struct ExecServerHandler {
    outbound_tx: mpsc::Sender<ExecServerOutboundMessage>,
    file_system: ExecServerFileSystem,
    // Keyed by client-chosen logical `processId` scoped to this connection.
    // This is a protocol handle, not an OS pid.
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
    initialize_requested: bool,
    initialized: bool,
}

impl ExecServerHandler {
    pub(crate) fn new(outbound_tx: mpsc::Sender<ExecServerOutboundMessage>) -> Self {
        Self {
            outbound_tx,
            file_system: ExecServerFileSystem::default(),
            processes: Arc::new(Mutex::new(HashMap::new())),
            initialize_requested: false,
            initialized: false,
        }
    }

    pub(crate) async fn shutdown(&self) {
        let remaining = {
            let mut processes = self.processes.lock().await;
            processes
                .drain()
                .map(|(_, process)| process)
                .collect::<Vec<_>>()
        };
        for process in remaining {
            process.session.terminate();
        }
    }

    pub(crate) fn initialized(&mut self) -> Result<(), String> {
        if !self.initialize_requested {
            return Err("received `initialized` notification before `initialize`".into());
        }
        self.initialized = true;
        Ok(())
    }

    pub(crate) fn initialize(
        &mut self,
    ) -> Result<InitializeResponse, codex_app_server_protocol::JSONRPCErrorError> {
        if self.initialize_requested {
            return Err(invalid_request(
                "initialize may only be sent once per connection".to_string(),
            ));
        }
        self.initialize_requested = true;
        Ok(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
        })
    }

    fn require_initialized(&self) -> Result<(), codex_app_server_protocol::JSONRPCErrorError> {
        if !self.initialize_requested {
            return Err(invalid_request(
                "client must call initialize before using exec methods".to_string(),
            ));
        }
        if !self.initialized {
            return Err(invalid_request(
                "client must send initialized before using exec methods".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn exec(
        &self,
        params: crate::protocol::ExecParams,
    ) -> Result<ExecResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        let process_id = params.process_id.clone();
        // Same-connection requests are serialized by the RPC processor, and the
        // in-process client holds the handler mutex across this full call. That
        // makes this pre-spawn duplicate check safe for the current entrypoints.
        {
            let process_map = self.processes.lock().await;
            if process_map.contains_key(&process_id) {
                return Err(invalid_request(format!(
                    "process {process_id} already exists"
                )));
            }
        }

        if matches!(
            params.sandbox.as_ref().map(|sandbox| sandbox.mode),
            Some(ExecSandboxMode::HostDefault)
        ) {
            return Err(invalid_request(
                "sandbox mode `hostDefault` is not supported by exec-server yet".to_string(),
            ));
        }

        let (program, args) = params
            .argv
            .split_first()
            .ok_or_else(|| invalid_params("argv must not be empty".to_string()))?;

        let spawned = if params.tty {
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
        }
        .map_err(|err| internal_error(err.to_string()))?;

        let output_notify = Arc::new(Notify::new());
        {
            let mut process_map = self.processes.lock().await;
            process_map.insert(
                process_id.clone(),
                RunningProcess {
                    session: spawned.session,
                    tty: params.tty,
                    output: std::collections::VecDeque::new(),
                    retained_bytes: 0,
                    next_seq: 1,
                    exit_code: None,
                    output_notify: Arc::clone(&output_notify),
                },
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
            self.outbound_tx.clone(),
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
            self.outbound_tx.clone(),
            Arc::clone(&self.processes),
            Arc::clone(&output_notify),
        ));
        tokio::spawn(watch_exit(
            process_id.clone(),
            spawned.exit_rx,
            self.outbound_tx.clone(),
            Arc::clone(&self.processes),
            output_notify,
        ));

        Ok(ExecResponse { process_id })
    }

    pub(crate) async fn fs_read_file(
        &self,
        params: FsReadFileParams,
    ) -> Result<FsReadFileResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.read_file(params).await
    }

    pub(crate) async fn fs_write_file(
        &self,
        params: FsWriteFileParams,
    ) -> Result<FsWriteFileResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.write_file(params).await
    }

    pub(crate) async fn fs_create_directory(
        &self,
        params: FsCreateDirectoryParams,
    ) -> Result<FsCreateDirectoryResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.create_directory(params).await
    }

    pub(crate) async fn fs_get_metadata(
        &self,
        params: FsGetMetadataParams,
    ) -> Result<FsGetMetadataResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.get_metadata(params).await
    }

    pub(crate) async fn fs_read_directory(
        &self,
        params: FsReadDirectoryParams,
    ) -> Result<FsReadDirectoryResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.read_directory(params).await
    }

    pub(crate) async fn fs_remove(
        &self,
        params: FsRemoveParams,
    ) -> Result<FsRemoveResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.remove(params).await
    }

    pub(crate) async fn fs_copy(
        &self,
        params: FsCopyParams,
    ) -> Result<FsCopyResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        self.file_system.copy(params).await
    }

    pub(crate) async fn read(
        &self,
        params: crate::protocol::ReadParams,
    ) -> Result<ReadResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
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

    pub(crate) async fn write(
        &self,
        params: crate::protocol::WriteParams,
    ) -> Result<WriteResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        let writer_tx = {
            let process_map = self.processes.lock().await;
            let process = process_map.get(&params.process_id).ok_or_else(|| {
                invalid_request(format!("unknown process id {}", params.process_id))
            })?;
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
        params: crate::protocol::TerminateParams,
    ) -> Result<TerminateResponse, codex_app_server_protocol::JSONRPCErrorError> {
        self.require_initialized()?;
        let running = {
            let process_map = self.processes.lock().await;
            if let Some(process) = process_map.get(&params.process_id) {
                process.session.terminate();
                true
            } else {
                false
            }
        };

        Ok(TerminateResponse { running })
    }
}

#[cfg(test)]
impl ExecServerHandler {
    async fn handle_message(
        &mut self,
        message: crate::server::routing::ExecServerInboundMessage,
    ) -> Result<(), String> {
        match message {
            crate::server::routing::ExecServerInboundMessage::Request(request) => {
                self.handle_request(request).await
            }
            crate::server::routing::ExecServerInboundMessage::Notification(
                crate::server::routing::ExecServerClientNotification::Initialized,
            ) => self.initialized(),
        }
    }

    async fn handle_request(
        &mut self,
        request: crate::server::routing::ExecServerRequest,
    ) -> Result<(), String> {
        let outbound = match request {
            crate::server::routing::ExecServerRequest::Initialize { request_id, .. } => {
                Self::request_outbound(
                    request_id,
                    self.initialize()
                        .map(crate::server::routing::ExecServerResponseMessage::Initialize),
                )
            }
            crate::server::routing::ExecServerRequest::Exec { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.exec(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::Exec),
                )
            }
            crate::server::routing::ExecServerRequest::Read { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.read(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::Read),
                )
            }
            crate::server::routing::ExecServerRequest::Write { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.write(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::Write),
                )
            }
            crate::server::routing::ExecServerRequest::Terminate { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.terminate(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::Terminate),
                )
            }
            crate::server::routing::ExecServerRequest::FsReadFile { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_read_file(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsReadFile),
                )
            }
            crate::server::routing::ExecServerRequest::FsWriteFile { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_write_file(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsWriteFile),
                )
            }
            crate::server::routing::ExecServerRequest::FsCreateDirectory { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_create_directory(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsCreateDirectory),
                )
            }
            crate::server::routing::ExecServerRequest::FsGetMetadata { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_get_metadata(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsGetMetadata),
                )
            }
            crate::server::routing::ExecServerRequest::FsReadDirectory { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_read_directory(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsReadDirectory),
                )
            }
            crate::server::routing::ExecServerRequest::FsRemove { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_remove(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsRemove),
                )
            }
            crate::server::routing::ExecServerRequest::FsCopy { request_id, params } => {
                Self::request_outbound(
                    request_id,
                    self.fs_copy(params)
                        .await
                        .map(crate::server::routing::ExecServerResponseMessage::FsCopy),
                )
            }
        };
        self.outbound_tx
            .send(outbound)
            .await
            .map_err(|_| "outbound channel closed".to_string())
    }

    fn request_outbound(
        request_id: codex_app_server_protocol::RequestId,
        result: Result<
            crate::server::routing::ExecServerResponseMessage,
            codex_app_server_protocol::JSONRPCErrorError,
        >,
    ) -> crate::server::routing::ExecServerOutboundMessage {
        match result {
            Ok(response) => crate::server::routing::ExecServerOutboundMessage::Response {
                request_id,
                response,
            },
            Err(error) => {
                crate::server::routing::ExecServerOutboundMessage::Error { request_id, error }
            }
        }
    }
}

async fn stream_output(
    process_id: String,
    stream: ExecOutputStream,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    outbound_tx: mpsc::Sender<ExecServerOutboundMessage>,
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
    output_notify: Arc<Notify>,
) {
    while let Some(chunk) = receiver.recv().await {
        let notification = {
            let mut processes = processes.lock().await;
            let Some(process) = processes.get_mut(&process_id) else {
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

        if outbound_tx
            .send(ExecServerOutboundMessage::Notification(
                ExecServerServerNotification::OutputDelta(notification),
            ))
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
    outbound_tx: mpsc::Sender<ExecServerOutboundMessage>,
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
    output_notify: Arc<Notify>,
) {
    let exit_code = exit_rx.await.unwrap_or(-1);
    {
        let mut processes = processes.lock().await;
        if let Some(process) = processes.get_mut(&process_id) {
            process.exit_code = Some(exit_code);
        }
    }
    output_notify.notify_waiters();
    let _ = outbound_tx
        .send(ExecServerOutboundMessage::Notification(
            ExecServerServerNotification::Exited(ExecExitedNotification {
                process_id,
                exit_code,
            }),
        ))
        .await;
}

#[cfg(test)]
mod tests;
