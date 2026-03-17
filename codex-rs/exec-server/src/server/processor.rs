use std::collections::HashMap;
use std::collections::VecDeque;
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
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
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

pub(crate) async fn run_connection(connection: JsonRpcConnection) {
    let (outgoing_tx, mut incoming_rx) = connection.into_parts();
    let mut processor = ExecServerConnectionProcessor::new(outgoing_tx);

    while let Some(event) = incoming_rx.recv().await {
        match event {
            JsonRpcConnectionEvent::Message(message) => {
                if let Err(err) = processor.handle_message(message).await {
                    warn!("closing exec-server connection after protocol error: {err}");
                    break;
                }
            }
            JsonRpcConnectionEvent::Disconnected { reason } => {
                if let Some(reason) = reason {
                    debug!("exec-server connection disconnected: {reason}");
                }
                break;
            }
        }
    }

    processor.shutdown().await;
}

struct ExecServerConnectionProcessor {
    outgoing_tx: mpsc::Sender<JSONRPCMessage>,
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
    initialize_requested: bool,
    initialized: bool,
}

impl ExecServerConnectionProcessor {
    fn new(outgoing_tx: mpsc::Sender<JSONRPCMessage>) -> Self {
        Self {
            outgoing_tx,
            processes: Arc::new(Mutex::new(HashMap::new())),
            initialize_requested: false,
            initialized: false,
        }
    }

    async fn shutdown(&self) {
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

    async fn handle_message(&mut self, message: JSONRPCMessage) -> Result<(), String> {
        match message {
            JSONRPCMessage::Request(request) => self.handle_request(request).await,
            JSONRPCMessage::Notification(notification) => self.handle_notification(notification),
            JSONRPCMessage::Response(response) => Err(format!(
                "unexpected client response for request id {:?}",
                response.id
            )),
            JSONRPCMessage::Error(error) => Err(format!(
                "unexpected client error for request id {:?}",
                error.id
            )),
        }
    }

    async fn handle_request(&mut self, request: JSONRPCRequest) -> Result<(), String> {
        let response = match request.method.as_str() {
            INITIALIZE_METHOD => self.handle_initialize_request(),
            EXEC_METHOD => match self.require_initialized() {
                Ok(()) => self.handle_exec_request(request.params).await,
                Err(err) => Err(err),
            },
            EXEC_WRITE_METHOD => match self.require_initialized() {
                Ok(()) => self.handle_write_request(request.params).await,
                Err(err) => Err(err),
            },
            EXEC_TERMINATE_METHOD => match self.require_initialized() {
                Ok(()) => self.handle_terminate_request(request.params).await,
                Err(err) => Err(err),
            },
            other => Err(invalid_request(format!("unknown method: {other}"))),
        };

        match response {
            Ok(result) => {
                self.send_response(JSONRPCResponse {
                    id: request.id,
                    result,
                })
                .await;
            }
            Err(error) => {
                self.send_error(request.id, error).await;
            }
        }

        Ok(())
    }

    fn handle_notification(&mut self, notification: JSONRPCNotification) -> Result<(), String> {
        match notification.method.as_str() {
            INITIALIZED_METHOD => {
                if !self.initialize_requested {
                    return Err("received `initialized` notification before `initialize`".into());
                }
                self.initialized = true;
                Ok(())
            }
            other => Err(format!("unexpected notification method: {other}")),
        }
    }

    fn handle_initialize_request(&mut self) -> Result<serde_json::Value, JSONRPCErrorError> {
        if self.initialize_requested {
            return Err(invalid_request(
                "initialize may only be sent once per connection".to_string(),
            ));
        }
        self.initialize_requested = true;
        json_value(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
        })
    }

    fn require_initialized(&self) -> Result<(), JSONRPCErrorError> {
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

    async fn handle_exec_request(
        &self,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, JSONRPCErrorError> {
        let params: ExecParams = serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
            .map_err(|err| invalid_params(err.to_string()))?;

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

        let stdout_buffer = Arc::new(StdMutex::new(BoundedBytesBuffer::new(
            params.output_bytes_cap,
        )));
        let stderr_buffer = Arc::new(StdMutex::new(BoundedBytesBuffer::new(
            params.output_bytes_cap,
        )));

        let process_id = params.process_id.clone();
        {
            let mut process_map = self.processes.lock().await;
            if process_map.contains_key(&process_id) {
                spawned.session.terminate();
                return Err(invalid_request(format!(
                    "process {process_id} already exists"
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
            self.outgoing_tx.clone(),
            Arc::clone(&stdout_buffer),
        ));
        tokio::spawn(stream_output(
            process_id.clone(),
            ExecOutputStream::Stderr,
            spawned.stderr_rx,
            self.outgoing_tx.clone(),
            Arc::clone(&stderr_buffer),
        ));
        tokio::spawn(watch_exit(
            process_id.clone(),
            spawned.exit_rx,
            self.outgoing_tx.clone(),
            Arc::clone(&self.processes),
        ));

        json_value(ExecResponse {
            process_id,
            running: true,
            exit_code: None,
            stdout: None,
            stderr: None,
        })
    }

    async fn handle_write_request(
        &self,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, JSONRPCErrorError> {
        let params: WriteParams = serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
            .map_err(|err| invalid_params(err.to_string()))?;

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

        json_value(WriteResponse { accepted: true })
    }

    async fn handle_terminate_request(
        &self,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, JSONRPCErrorError> {
        let params: TerminateParams =
            serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
                .map_err(|err| invalid_params(err.to_string()))?;

        let process = {
            let mut process_map = self.processes.lock().await;
            process_map.remove(&params.process_id)
        };

        if let Some(process) = process {
            process.session.terminate();
            json_value(TerminateResponse { running: true })
        } else {
            json_value(TerminateResponse { running: false })
        }
    }

    async fn send_response(&self, response: JSONRPCResponse) {
        let _ = self
            .outgoing_tx
            .send(JSONRPCMessage::Response(response))
            .await;
    }

    async fn send_error(&self, id: RequestId, error: JSONRPCErrorError) {
        let _ = self
            .outgoing_tx
            .send(JSONRPCMessage::Error(JSONRPCError { error, id }))
            .await;
    }
}

async fn stream_output(
    process_id: String,
    stream: ExecOutputStream,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    outgoing_tx: mpsc::Sender<JSONRPCMessage>,
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
        if send_notification(&outgoing_tx, EXEC_OUTPUT_DELTA_METHOD, &notification)
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
    outgoing_tx: mpsc::Sender<JSONRPCMessage>,
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
        &outgoing_tx,
        EXEC_EXITED_METHOD,
        &ExecExitedNotification {
            process_id,
            exit_code,
        },
    )
    .await;
}

async fn send_notification<T: serde::Serialize>(
    outgoing_tx: &mpsc::Sender<JSONRPCMessage>,
    method: &str,
    params: &T,
) -> Result<(), serde_json::Error> {
    outgoing_tx
        .send(JSONRPCMessage::Notification(JSONRPCNotification {
            method: method.to_string(),
            params: Some(serde_json::to_value(params)?),
        }))
        .await
        .map_err(|_| serde_json::Error::io(std::io::Error::other("connection closed")))
}

fn json_value<T: serde::Serialize>(value: T) -> Result<serde_json::Value, JSONRPCErrorError> {
    serde_json::to_value(value).map_err(|err| internal_error(err.to_string()))
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
