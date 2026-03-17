use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::Mutex;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
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
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::protocol::InitializeResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerClientConnectOptions {
    pub client_name: String,
}

impl Default for ExecServerClientConnectOptions {
    fn default() -> Self {
        Self {
            client_name: "codex-core".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecServerConnectArgs {
    pub websocket_url: String,
    pub client_name: String,
}

impl From<RemoteExecServerConnectArgs> for ExecServerClientConnectOptions {
    fn from(value: RemoteExecServerConnectArgs) -> Self {
        Self {
            client_name: value.client_name,
        }
    }
}

pub struct ExecServerProcess {
    process_id: String,
    output_rx: broadcast::Receiver<Vec<u8>>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    status: Arc<RemoteProcessStatus>,
    client: ExecServerClient,
}

impl std::fmt::Debug for ExecServerProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecServerProcess")
            .field("process_id", &self.process_id)
            .field("has_exited", &self.has_exited())
            .field("exit_code", &self.exit_code())
            .finish()
    }
}

impl ExecServerProcess {
    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub fn output_receiver(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_rx.resubscribe()
    }

    pub fn has_exited(&self) -> bool {
        self.status.has_exited()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.status.exit_code()
    }

    pub fn terminate(&self) {
        self.status.mark_exited(None);
        let client = self.client.clone();
        let process_id = self.process_id.clone();
        tokio::spawn(async move {
            let _ = client.terminate_process(&process_id).await;
        });
    }
}

impl std::fmt::Debug for RemoteProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteProcessStatus")
            .field("exited", &self.has_exited())
            .field("exit_code", &self.exit_code())
            .finish()
    }
}

struct RemoteProcessStatus {
    exited: AtomicBool,
    exit_code: StdMutex<Option<i32>>,
}

impl RemoteProcessStatus {
    fn new() -> Self {
        Self {
            exited: AtomicBool::new(false),
            exit_code: StdMutex::new(None),
        }
    }

    fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    fn exit_code(&self) -> Option<i32> {
        self.exit_code.lock().ok().and_then(|guard| *guard)
    }

    fn mark_exited(&self, exit_code: Option<i32>) {
        self.exited.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.exit_code.lock() {
            *guard = exit_code;
        }
    }
}

struct RegisteredProcess {
    output_tx: broadcast::Sender<Vec<u8>>,
    status: Arc<RemoteProcessStatus>,
}

struct Inner {
    write_tx: mpsc::Sender<JSONRPCMessage>,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, JSONRPCErrorError>>>>,
    processes: Mutex<HashMap<String, RegisteredProcess>>,
    next_request_id: AtomicI64,
    reader_task: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

#[derive(Clone)]
pub struct ExecServerClient {
    inner: Arc<Inner>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecServerError {
    #[error("failed to spawn exec-server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("failed to connect to exec-server websocket `{url}`: {source}")]
    WebSocketConnect {
        url: String,
        #[source]
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[error("exec-server transport closed")]
    Closed,
    #[error("failed to serialize or deserialize exec-server JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("exec-server protocol error: {0}")]
    Protocol(String),
    #[error("exec-server rejected request ({code}): {message}")]
    Server { code: i64, message: String },
}

impl ExecServerClient {
    pub async fn connect_stdio<R, W>(
        stdin: W,
        stdout: R,
        options: ExecServerClientConnectOptions,
    ) -> Result<Self, ExecServerError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::connect(
            JsonRpcConnection::from_stdio(stdout, stdin, "exec-server stdio".to_string()),
            options,
        )
        .await
    }

    pub async fn connect_websocket(
        args: RemoteExecServerConnectArgs,
    ) -> Result<Self, ExecServerError> {
        let websocket_url = args.websocket_url.clone();
        let (stream, _) = connect_async(websocket_url.as_str())
            .await
            .map_err(|source| ExecServerError::WebSocketConnect {
                url: websocket_url.clone(),
                source,
            })?;

        Self::connect(
            JsonRpcConnection::from_websocket(
                stream,
                format!("exec-server websocket {websocket_url}"),
            ),
            args.into(),
        )
        .await
    }

    async fn connect(
        connection: JsonRpcConnection,
        options: ExecServerClientConnectOptions,
    ) -> Result<Self, ExecServerError> {
        let (write_tx, mut incoming_rx) = connection.into_parts();
        let inner = Arc::new_cyclic(|weak| {
            let weak = weak.clone();
            let reader_task = tokio::spawn(async move {
                while let Some(event) = incoming_rx.recv().await {
                    match event {
                        JsonRpcConnectionEvent::Message(message) => {
                            if let Some(inner) = weak.upgrade()
                                && let Err(err) = handle_server_message(&inner, message).await
                            {
                                warn!("exec-server client closing after protocol error: {err}");
                                handle_transport_shutdown(&inner).await;
                                return;
                            }
                        }
                        JsonRpcConnectionEvent::Disconnected { reason } => {
                            if let Some(reason) = reason {
                                warn!("exec-server client transport disconnected: {reason}");
                            }
                            if let Some(inner) = weak.upgrade() {
                                handle_transport_shutdown(&inner).await;
                            }
                            return;
                        }
                    }
                }

                if let Some(inner) = weak.upgrade() {
                    handle_transport_shutdown(&inner).await;
                }
            });

            Inner {
                write_tx,
                pending: Mutex::new(HashMap::new()),
                processes: Mutex::new(HashMap::new()),
                next_request_id: AtomicI64::new(1),
                reader_task,
            }
        });

        let client = Self { inner };
        client.initialize(options).await?;
        Ok(client)
    }

    pub async fn start_process(
        &self,
        params: ExecParams,
    ) -> Result<ExecServerProcess, ExecServerError> {
        let process_id = params.process_id.clone();
        let status = Arc::new(RemoteProcessStatus::new());
        let (output_tx, output_rx) = broadcast::channel(256);
        self.inner.processes.lock().await.insert(
            process_id.clone(),
            RegisteredProcess {
                output_tx,
                status: Arc::clone(&status),
            },
        );

        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
        let client = self.clone();
        let write_process_id = process_id.clone();
        tokio::spawn(async move {
            while let Some(chunk) = writer_rx.recv().await {
                let request = WriteParams {
                    process_id: write_process_id.clone(),
                    chunk: chunk.into(),
                };
                if client.write_process(request).await.is_err() {
                    break;
                }
            }
        });

        let response = match self.request::<_, ExecResponse>(EXEC_METHOD, &params).await {
            Ok(response) => response,
            Err(err) => {
                self.inner.processes.lock().await.remove(&process_id);
                return Err(err);
            }
        };

        if !response.running {
            status.mark_exited(response.exit_code);
        }

        if let Some(stdout) = response.stdout {
            let _ = self
                .inner
                .processes
                .lock()
                .await
                .get(&process_id)
                .map(|process| process.output_tx.send(stdout.into_inner()));
        }
        if let Some(stderr) = response.stderr {
            let _ = self
                .inner
                .processes
                .lock()
                .await
                .get(&process_id)
                .map(|process| process.output_tx.send(stderr.into_inner()));
        }

        if let Some(exit_code) = response.exit_code {
            status.mark_exited(Some(exit_code));
        }

        Ok(ExecServerProcess {
            process_id,
            output_rx,
            writer_tx,
            status,
            client: self.clone(),
        })
    }

    async fn initialize(
        &self,
        options: ExecServerClientConnectOptions,
    ) -> Result<(), ExecServerError> {
        let _: InitializeResponse = self
            .request(
                INITIALIZE_METHOD,
                &InitializeParams {
                    client_name: options.client_name,
                },
            )
            .await?;
        self.notify(INITIALIZED_METHOD, &serde_json::json!({}))
            .await
    }

    async fn write_process(&self, params: WriteParams) -> Result<WriteResponse, ExecServerError> {
        self.request(EXEC_WRITE_METHOD, &params).await
    }

    async fn terminate_process(
        &self,
        process_id: &str,
    ) -> Result<TerminateResponse, ExecServerError> {
        self.request(
            EXEC_TERMINATE_METHOD,
            &TerminateParams {
                process_id: process_id.to_string(),
            },
        )
        .await
    }

    async fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<(), ExecServerError> {
        let params = serde_json::to_value(params)?;
        self.inner
            .write_tx
            .send(JSONRPCMessage::Notification(JSONRPCNotification {
                method: method.to_string(),
                params: Some(params),
            }))
            .await
            .map_err(|_| ExecServerError::Closed)
    }

    async fn request<P, R>(&self, method: &str, params: &P) -> Result<R, ExecServerError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let request_id =
            RequestId::Integer(self.inner.next_request_id.fetch_add(1, Ordering::SeqCst));
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), response_tx);

        let params = serde_json::to_value(params)?;
        let message = JSONRPCMessage::Request(JSONRPCRequest {
            id: request_id.clone(),
            method: method.to_string(),
            params: Some(params),
            trace: None,
        });

        if self.inner.write_tx.send(message).await.is_err() {
            self.inner.pending.lock().await.remove(&request_id);
            return Err(ExecServerError::Closed);
        }

        let result = response_rx.await.map_err(|_| ExecServerError::Closed)?;
        match result {
            Ok(value) => serde_json::from_value(value).map_err(ExecServerError::from),
            Err(error) => Err(ExecServerError::Server {
                code: error.code,
                message: error.message,
            }),
        }
    }
}

async fn handle_server_message(
    inner: &Arc<Inner>,
    message: JSONRPCMessage,
) -> Result<(), ExecServerError> {
    match message {
        JSONRPCMessage::Response(JSONRPCResponse { id, result }) => {
            if let Some(tx) = inner.pending.lock().await.remove(&id) {
                let _ = tx.send(Ok(result));
            }
        }
        JSONRPCMessage::Error(JSONRPCError { id, error }) => {
            if let Some(tx) = inner.pending.lock().await.remove(&id) {
                let _ = tx.send(Err(error));
            }
        }
        JSONRPCMessage::Notification(notification) => {
            handle_server_notification(inner, notification).await?;
        }
        JSONRPCMessage::Request(request) => {
            return Err(ExecServerError::Protocol(format!(
                "unexpected exec-server request from remote server: {}",
                request.method
            )));
        }
    }

    Ok(())
}

async fn handle_server_notification(
    inner: &Arc<Inner>,
    notification: JSONRPCNotification,
) -> Result<(), ExecServerError> {
    match notification.method.as_str() {
        EXEC_OUTPUT_DELTA_METHOD => {
            let params: ExecOutputDeltaNotification =
                serde_json::from_value(notification.params.unwrap_or(Value::Null))?;
            let chunk = params.chunk.into_inner();
            let processes = inner.processes.lock().await;
            if let Some(process) = processes.get(&params.process_id) {
                let _ = process.output_tx.send(chunk);
            }
        }
        EXEC_EXITED_METHOD => {
            let params: ExecExitedNotification =
                serde_json::from_value(notification.params.unwrap_or(Value::Null))?;
            let mut processes = inner.processes.lock().await;
            if let Some(process) = processes.remove(&params.process_id) {
                process.status.mark_exited(Some(params.exit_code));
            }
        }
        other => {
            debug!("ignoring unknown exec-server notification: {other}");
        }
    }
    Ok(())
}

async fn handle_transport_shutdown(inner: &Arc<Inner>) {
    let pending = {
        let mut pending = inner.pending.lock().await;
        pending.drain().map(|(_, tx)| tx).collect::<Vec<_>>()
    };
    for tx in pending {
        let _ = tx.send(Err(JSONRPCErrorError {
            code: -32000,
            data: None,
            message: "exec-server transport closed".to_string(),
        }));
    }

    let processes = {
        let mut processes = inner.processes.lock().await;
        processes
            .drain()
            .map(|(_, process)| process)
            .collect::<Vec<_>>()
    };
    for process in processes {
        process.status.mark_exited(None);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::time::timeout;

    use super::ExecServerClient;
    use super::ExecServerClientConnectOptions;
    use super::ExecServerError;
    use crate::protocol::EXEC_METHOD;
    use crate::protocol::ExecParams;
    use crate::protocol::INITIALIZE_METHOD;
    use crate::protocol::INITIALIZED_METHOD;
    use crate::protocol::PROTOCOL_VERSION;
    use codex_app_server_protocol::JSONRPCError;
    use codex_app_server_protocol::JSONRPCErrorError;
    use codex_app_server_protocol::JSONRPCMessage;
    use codex_app_server_protocol::JSONRPCNotification;
    use codex_app_server_protocol::JSONRPCRequest;
    use codex_app_server_protocol::JSONRPCResponse;

    async fn read_jsonrpc_line<R>(lines: &mut tokio::io::Lines<BufReader<R>>) -> JSONRPCMessage
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let next_line = timeout(Duration::from_secs(1), lines.next_line()).await;
        let line_result = match next_line {
            Ok(line_result) => line_result,
            Err(err) => panic!("timed out waiting for JSON-RPC line: {err}"),
        };
        let maybe_line = match line_result {
            Ok(maybe_line) => maybe_line,
            Err(err) => panic!("failed to read JSON-RPC line: {err}"),
        };
        let line = match maybe_line {
            Some(line) => line,
            None => panic!("server connection closed before JSON-RPC line arrived"),
        };
        match serde_json::from_str::<JSONRPCMessage>(&line) {
            Ok(message) => message,
            Err(err) => panic!("failed to parse JSON-RPC line: {err}"),
        }
    }

    async fn write_jsonrpc_line<W>(writer: &mut W, message: JSONRPCMessage)
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let encoded = match serde_json::to_string(&message) {
            Ok(encoded) => encoded,
            Err(err) => panic!("failed to encode JSON-RPC message: {err}"),
        };
        if let Err(err) = writer.write_all(format!("{encoded}\n").as_bytes()).await {
            panic!("failed to write JSON-RPC line: {err}");
        }
    }

    #[tokio::test]
    async fn connect_stdio_performs_initialize_handshake() {
        let (client_stdin, server_reader) = tokio::io::duplex(4096);
        let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(server_reader).lines();

            let initialize = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Request(request) = initialize else {
                panic!("expected initialize request");
            };
            assert_eq!(request.method, INITIALIZE_METHOD);
            assert_eq!(
                request.params,
                Some(serde_json::json!({ "clientName": "test-client" }))
            );
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Response(JSONRPCResponse {
                    id: request.id,
                    result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
                }),
            )
            .await;

            let initialized = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Notification(JSONRPCNotification { method, params }) = initialized
            else {
                panic!("expected initialized notification");
            };
            assert_eq!(method, INITIALIZED_METHOD);
            assert_eq!(params, Some(serde_json::json!({})));
        });

        let client = ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            ExecServerClientConnectOptions {
                client_name: "test-client".to_string(),
            },
        )
        .await;
        if let Err(err) = client {
            panic!("failed to connect test client: {err}");
        }

        if let Err(err) = server.await {
            panic!("server task failed: {err}");
        }
    }

    #[tokio::test]
    async fn connect_stdio_returns_initialize_errors() {
        let (client_stdin, server_reader) = tokio::io::duplex(4096);
        let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_reader).lines();

            let initialize = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Request(request) = initialize else {
                panic!("expected initialize request");
            };
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Error(JSONRPCError {
                    id: request.id,
                    error: JSONRPCErrorError {
                        code: -32600,
                        message: "rejected".to_string(),
                        data: None,
                    },
                }),
            )
            .await;
        });

        let result = ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            ExecServerClientConnectOptions {
                client_name: "test-client".to_string(),
            },
        )
        .await;

        match result {
            Err(ExecServerError::Server { code, message }) => {
                assert_eq!(code, -32600);
                assert_eq!(message, "rejected");
            }
            Err(err) => panic!("unexpected initialize failure: {err}"),
            Ok(_) => panic!("expected initialize failure"),
        }
    }

    #[tokio::test]
    async fn start_process_cleans_up_registered_process_after_request_error() {
        let (client_stdin, server_reader) = tokio::io::duplex(4096);
        let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_reader).lines();

            let initialize = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Request(initialize_request) = initialize else {
                panic!("expected initialize request");
            };
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Response(JSONRPCResponse {
                    id: initialize_request.id,
                    result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
                }),
            )
            .await;

            let initialized = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Notification(notification) = initialized else {
                panic!("expected initialized notification");
            };
            assert_eq!(notification.method, INITIALIZED_METHOD);

            let exec_request = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
                panic!("expected exec request");
            };
            assert_eq!(method, EXEC_METHOD);
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Error(JSONRPCError {
                    id,
                    error: JSONRPCErrorError {
                        code: -32600,
                        message: "duplicate process".to_string(),
                        data: None,
                    },
                }),
            )
            .await;
        });

        let client = match ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            ExecServerClientConnectOptions {
                client_name: "test-client".to_string(),
            },
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let result = client
            .start_process(ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
                output_bytes_cap: 4096,
                arg0: None,
            })
            .await;

        match result {
            Err(ExecServerError::Server { code, message }) => {
                assert_eq!(code, -32600);
                assert_eq!(message, "duplicate process");
            }
            Err(err) => panic!("unexpected start_process failure: {err}"),
            Ok(_) => panic!("expected start_process failure"),
        }

        assert!(
            client.inner.processes.lock().await.is_empty(),
            "failed requests should not leave registered process state behind"
        );
    }
}
