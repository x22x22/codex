use std::collections::HashMap;
use std::sync::Arc;

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
                },
            );
        }

        tokio::spawn(stream_output(
            process_id.clone(),
            ExecOutputStream::Stdout,
            spawned.stdout_rx,
            self.outgoing_tx.clone(),
        ));
        tokio::spawn(stream_output(
            process_id.clone(),
            ExecOutputStream::Stderr,
            spawned.stderr_rx,
            self.outgoing_tx.clone(),
        ));
        tokio::spawn(watch_exit(
            process_id.clone(),
            spawned.exit_rx,
            self.outgoing_tx.clone(),
            Arc::clone(&self.processes),
        ));

        json_value(ExecResponse { process_id })
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
) {
    while let Some(chunk) = receiver.recv().await {
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
    {
        let mut processes = processes.lock().await;
        processes.remove(&process_id);
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio::time::timeout;

    use super::ExecServerConnectionProcessor;
    use crate::protocol::EXEC_METHOD;
    use crate::protocol::INITIALIZE_METHOD;
    use crate::protocol::INITIALIZED_METHOD;
    use crate::protocol::PROTOCOL_VERSION;
    use codex_app_server_protocol::JSONRPCMessage;
    use codex_app_server_protocol::JSONRPCNotification;
    use codex_app_server_protocol::JSONRPCRequest;
    use codex_app_server_protocol::RequestId;

    fn request(id: i64, method: &str, params: serde_json::Value) -> JSONRPCMessage {
        JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(id),
            method: method.to_string(),
            params: Some(params),
            trace: None,
        })
    }

    async fn recv_outgoing_json(
        outgoing_rx: &mut tokio::sync::mpsc::Receiver<JSONRPCMessage>,
    ) -> serde_json::Value {
        let recv_result = timeout(Duration::from_secs(1), outgoing_rx.recv()).await;
        let maybe_message = match recv_result {
            Ok(maybe_message) => maybe_message,
            Err(err) => panic!("timed out waiting for processor output: {err}"),
        };
        let message = match maybe_message {
            Some(message) => message,
            None => panic!("processor output channel closed unexpectedly"),
        };
        serde_json::to_value(message)
            .unwrap_or_else(|err| panic!("failed to serialize processor output: {err}"))
    }

    #[tokio::test]
    async fn initialize_response_reports_protocol_version() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(1);
        let mut processor = ExecServerConnectionProcessor::new(outgoing_tx);

        if let Err(err) = processor
            .handle_message(request(
                1,
                INITIALIZE_METHOD,
                json!({ "clientName": "test" }),
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }

        let outgoing = recv_outgoing_json(&mut outgoing_rx).await;
        assert_eq!(
            outgoing,
            json!({
                "id": 1,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION
                }
            })
        );
    }

    #[tokio::test]
    async fn exec_methods_require_initialize() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(1);
        let mut processor = ExecServerConnectionProcessor::new(outgoing_tx);

        if let Err(err) = processor
            .handle_message(request(7, EXEC_METHOD, json!({ "processId": "proc-1" })))
            .await
        {
            panic!("request handling should not fail the connection: {err}");
        }

        let outgoing = recv_outgoing_json(&mut outgoing_rx).await;
        assert_eq!(
            outgoing,
            json!({
                "id": 7,
                "error": {
                    "code": -32600,
                    "message": "client must call initialize before using exec methods"
                }
            })
        );
    }

    #[tokio::test]
    async fn exec_methods_require_initialized_notification_after_initialize() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
        let mut processor = ExecServerConnectionProcessor::new(outgoing_tx);

        if let Err(err) = processor
            .handle_message(request(
                1,
                INITIALIZE_METHOD,
                json!({ "clientName": "test" }),
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outgoing_json(&mut outgoing_rx).await;

        if let Err(err) = processor
            .handle_message(request(2, EXEC_METHOD, json!({ "processId": "proc-1" })))
            .await
        {
            panic!("request handling should not fail the connection: {err}");
        }

        let outgoing = recv_outgoing_json(&mut outgoing_rx).await;
        assert_eq!(
            outgoing,
            json!({
                "id": 2,
                "error": {
                    "code": -32600,
                    "message": "client must send initialized before using exec methods"
                }
            })
        );
    }

    #[tokio::test]
    async fn initialized_before_initialize_is_a_protocol_error() {
        let (outgoing_tx, _outgoing_rx) = tokio::sync::mpsc::channel(1);
        let mut processor = ExecServerConnectionProcessor::new(outgoing_tx);

        let result = processor
            .handle_message(JSONRPCMessage::Notification(JSONRPCNotification {
                method: INITIALIZED_METHOD.to_string(),
                params: Some(json!({})),
            }))
            .await;

        match result {
            Err(err) => {
                assert_eq!(
                    err,
                    "received `initialized` notification before `initialize`"
                );
            }
            Ok(()) => panic!("expected protocol error for early initialized notification"),
        }
    }

    #[tokio::test]
    async fn initialize_may_only_be_sent_once_per_connection() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
        let mut processor = ExecServerConnectionProcessor::new(outgoing_tx);

        if let Err(err) = processor
            .handle_message(request(
                1,
                INITIALIZE_METHOD,
                json!({ "clientName": "test" }),
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outgoing_json(&mut outgoing_rx).await;

        if let Err(err) = processor
            .handle_message(request(
                2,
                INITIALIZE_METHOD,
                json!({ "clientName": "test" }),
            ))
            .await
        {
            panic!("duplicate initialize should not fail the connection: {err}");
        }

        let outgoing = recv_outgoing_json(&mut outgoing_rx).await;
        assert_eq!(
            outgoing,
            json!({
                "id": 2,
                "error": {
                    "code": -32600,
                    "message": "initialize may only be sent once per connection"
                }
            })
        );
    }
}
