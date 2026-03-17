use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
use tokio::time::timeout;
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
use crate::server::ExecServerClientNotification;
use crate::server::ExecServerHandler;
use crate::server::ExecServerInboundMessage;
use crate::server::ExecServerOutboundMessage;
use crate::server::ExecServerRequest;
use crate::server::ExecServerResponseMessage;
use crate::server::ExecServerServerNotification;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerClientConnectOptions {
    pub client_name: String,
    pub initialize_timeout: Duration,
}

impl Default for ExecServerClientConnectOptions {
    fn default() -> Self {
        Self {
            client_name: "codex-core".to_string(),
            initialize_timeout: INITIALIZE_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecServerConnectArgs {
    pub websocket_url: String,
    pub client_name: String,
    pub connect_timeout: Duration,
    pub initialize_timeout: Duration,
}

impl From<RemoteExecServerConnectArgs> for ExecServerClientConnectOptions {
    fn from(value: RemoteExecServerConnectArgs) -> Self {
        Self {
            client_name: value.client_name,
            initialize_timeout: value.initialize_timeout,
        }
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);

impl RemoteExecServerConnectArgs {
    pub fn new(websocket_url: String, client_name: String) -> Self {
        Self {
            websocket_url,
            client_name,
            connect_timeout: CONNECT_TIMEOUT,
            initialize_timeout: INITIALIZE_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerOutput {
    pub stream: crate::protocol::ExecOutputStream,
    pub chunk: Vec<u8>,
}

pub struct ExecServerProcess {
    session_id: String,
    output_rx: broadcast::Receiver<ExecServerOutput>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    status: Arc<RemoteProcessStatus>,
    client: ExecServerClient,
}

impl std::fmt::Debug for ExecServerProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecServerProcess")
            .field("session_id", &self.session_id)
            .field("has_exited", &self.has_exited())
            .field("exit_code", &self.exit_code())
            .finish()
    }
}

impl ExecServerProcess {
    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub fn output_receiver(&self) -> broadcast::Receiver<ExecServerOutput> {
        self.output_rx.resubscribe()
    }

    pub fn has_exited(&self) -> bool {
        self.status.has_exited()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.status.exit_code()
    }

    pub fn terminate(&self) {
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let _ = client.terminate_session(&session_id).await;
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
    output_tx: broadcast::Sender<ExecServerOutput>,
    status: Arc<RemoteProcessStatus>,
}

enum ClientBackend {
    JsonRpc {
        write_tx: mpsc::Sender<JSONRPCMessage>,
    },
    InProcess {
        write_tx: mpsc::Sender<ExecServerInboundMessage>,
    },
}

struct Inner {
    backend: ClientBackend,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, JSONRPCErrorError>>>>,
    processes: Mutex<HashMap<String, RegisteredProcess>>,
    next_request_id: AtomicI64,
    reader_task: JoinHandle<()>,
    server_task: Option<JoinHandle<()>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.reader_task.abort();
        if let Some(server_task) = &self.server_task {
            server_task.abort();
        }
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
    #[error("timed out connecting to exec-server websocket `{url}` after {timeout:?}")]
    WebSocketConnectTimeout { url: String, timeout: Duration },
    #[error("failed to connect to exec-server websocket `{url}`: {source}")]
    WebSocketConnect {
        url: String,
        #[source]
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[error("timed out waiting for exec-server initialize handshake after {timeout:?}")]
    InitializeTimedOut { timeout: Duration },
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
    pub async fn connect_in_process(
        options: ExecServerClientConnectOptions,
    ) -> Result<Self, ExecServerError> {
        let (write_tx, mut inbound_rx) = mpsc::channel::<ExecServerInboundMessage>(256);
        let (outbound_tx, mut outgoing_rx) = mpsc::channel::<ExecServerOutboundMessage>(256);

        let server_task = tokio::spawn(async move {
            let mut handler = ExecServerHandler::new(outbound_tx);
            while let Some(message) = inbound_rx.recv().await {
                if let Err(err) = handler.handle_message(message).await {
                    warn!("in-process exec-server handler stopped after protocol error: {err}");
                    break;
                }
            }
            handler.shutdown().await;
        });

        let inner = Arc::new_cyclic(|weak| {
            let weak = weak.clone();
            let reader_task = tokio::spawn(async move {
                while let Some(message) = outgoing_rx.recv().await {
                    if let Some(inner) = weak.upgrade()
                        && let Err(err) = handle_in_process_outbound_message(&inner, message).await
                    {
                        warn!("in-process exec-server client closing after protocol error: {err}");
                        handle_transport_shutdown(&inner).await;
                        return;
                    }
                }

                if let Some(inner) = weak.upgrade() {
                    handle_transport_shutdown(&inner).await;
                }
            });

            Inner {
                backend: ClientBackend::InProcess { write_tx },
                pending: Mutex::new(HashMap::new()),
                processes: Mutex::new(HashMap::new()),
                next_request_id: AtomicI64::new(1),
                reader_task,
                server_task: Some(server_task),
            }
        });

        let client = Self { inner };
        client.initialize(options).await?;
        Ok(client)
    }

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
        let connect_timeout = args.connect_timeout;
        let (stream, _) = timeout(connect_timeout, connect_async(websocket_url.as_str()))
            .await
            .map_err(|_| ExecServerError::WebSocketConnectTimeout {
                url: websocket_url.clone(),
                timeout: connect_timeout,
            })?
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
                backend: ClientBackend::JsonRpc { write_tx },
                pending: Mutex::new(HashMap::new()),
                processes: Mutex::new(HashMap::new()),
                next_request_id: AtomicI64::new(1),
                reader_task,
                server_task: None,
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
        let response = self.exec(params).await?;
        let session_id = response.session_id;
        let status = Arc::new(RemoteProcessStatus::new());
        let (output_tx, output_rx) = broadcast::channel(256);
        {
            let mut processes = self.inner.processes.lock().await;
            if processes.contains_key(&session_id) {
                return Err(ExecServerError::Protocol(format!(
                    "session `{session_id}` already exists"
                )));
            }
            processes.insert(
                session_id.clone(),
                RegisteredProcess {
                    output_tx,
                    status: Arc::clone(&status),
                },
            );
        }

        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
        let client = self.clone();
        let write_session_id = session_id.clone();
        tokio::spawn(async move {
            while let Some(chunk) = writer_rx.recv().await {
                let request = WriteParams {
                    session_id: write_session_id.clone(),
                    chunk: chunk.into(),
                };
                if client.write_process(request).await.is_err() {
                    break;
                }
            }
        });

        Ok(ExecServerProcess {
            session_id,
            output_rx,
            writer_tx,
            status,
            client: self.clone(),
        })
    }

    pub async fn exec(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.request_exec(params).await
    }

    pub async fn write(
        &self,
        session_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        self.write_process(WriteParams {
            session_id: session_id.to_string(),
            chunk: chunk.into(),
        })
        .await
    }

    pub async fn terminate(&self, session_id: &str) -> Result<TerminateResponse, ExecServerError> {
        self.terminate_session(session_id).await
    }

    async fn initialize(
        &self,
        options: ExecServerClientConnectOptions,
    ) -> Result<(), ExecServerError> {
        let ExecServerClientConnectOptions {
            client_name,
            initialize_timeout,
        } = options;
        timeout(initialize_timeout, async {
            let _: InitializeResponse = self
                .request(INITIALIZE_METHOD, &InitializeParams { client_name })
                .await?;
            self.notify(INITIALIZED_METHOD, &serde_json::json!({}))
                .await
        })
        .await
        .map_err(|_| ExecServerError::InitializeTimedOut {
            timeout: initialize_timeout,
        })?
    }

    async fn request_exec(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.request(EXEC_METHOD, &params).await
    }

    async fn write_process(&self, params: WriteParams) -> Result<WriteResponse, ExecServerError> {
        self.request(EXEC_WRITE_METHOD, &params).await
    }

    async fn terminate_session(
        &self,
        session_id: &str,
    ) -> Result<TerminateResponse, ExecServerError> {
        self.request(
            EXEC_TERMINATE_METHOD,
            &TerminateParams {
                session_id: session_id.to_string(),
            },
        )
        .await
    }

    async fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<(), ExecServerError> {
        match &self.inner.backend {
            ClientBackend::JsonRpc { write_tx } => {
                let params = serde_json::to_value(params)?;
                write_tx
                    .send(JSONRPCMessage::Notification(JSONRPCNotification {
                        method: method.to_string(),
                        params: Some(params),
                    }))
                    .await
                    .map_err(|_| ExecServerError::Closed)
            }
            ClientBackend::InProcess { write_tx } => {
                let message = match method {
                    INITIALIZED_METHOD => ExecServerInboundMessage::Notification(
                        ExecServerClientNotification::Initialized,
                    ),
                    other => {
                        return Err(ExecServerError::Protocol(format!(
                            "unsupported in-process notification method `{other}`"
                        )));
                    }
                };
                write_tx
                    .send(message)
                    .await
                    .map_err(|_| ExecServerError::Closed)
            }
        }
    }

    async fn request<P, R>(&self, method: &str, params: &P) -> Result<R, ExecServerError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params)?;
        let request_id =
            RequestId::Integer(self.inner.next_request_id.fetch_add(1, Ordering::SeqCst));
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), response_tx);

        match &self.inner.backend {
            ClientBackend::JsonRpc { write_tx } => {
                let message = JSONRPCMessage::Request(JSONRPCRequest {
                    id: request_id.clone(),
                    method: method.to_string(),
                    params: Some(params),
                    trace: None,
                });

                if write_tx.send(message).await.is_err() {
                    self.inner.pending.lock().await.remove(&request_id);
                    return Err(ExecServerError::Closed);
                }
            }
            ClientBackend::InProcess { write_tx } => {
                let message = in_process_request(method, request_id.clone(), params)?;
                if write_tx.send(message).await.is_err() {
                    self.inner.pending.lock().await.remove(&request_id);
                    return Err(ExecServerError::Closed);
                }
            }
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

fn in_process_request(
    method: &str,
    request_id: RequestId,
    params: Value,
) -> Result<ExecServerInboundMessage, ExecServerError> {
    match method {
        INITIALIZE_METHOD => Ok(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id,
                params: serde_json::from_value(params)?,
            },
        )),
        EXEC_METHOD => Ok(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id,
            params: serde_json::from_value(params)?,
        })),
        EXEC_WRITE_METHOD => Ok(ExecServerInboundMessage::Request(
            ExecServerRequest::Write {
                request_id,
                params: serde_json::from_value(params)?,
            },
        )),
        EXEC_TERMINATE_METHOD => Ok(ExecServerInboundMessage::Request(
            ExecServerRequest::Terminate {
                request_id,
                params: serde_json::from_value(params)?,
            },
        )),
        other => Err(ExecServerError::Protocol(format!(
            "unsupported in-process request method `{other}`"
        ))),
    }
}

async fn handle_in_process_outbound_message(
    inner: &Arc<Inner>,
    message: ExecServerOutboundMessage,
) -> Result<(), ExecServerError> {
    match message {
        ExecServerOutboundMessage::Response {
            request_id,
            response,
        } => {
            if let Some(tx) = inner.pending.lock().await.remove(&request_id) {
                let result = match response {
                    ExecServerResponseMessage::Initialize(response) => {
                        serde_json::to_value(response)?
                    }
                    ExecServerResponseMessage::Exec(response) => serde_json::to_value(response)?,
                    ExecServerResponseMessage::Write(response) => serde_json::to_value(response)?,
                    ExecServerResponseMessage::Terminate(response) => {
                        serde_json::to_value(response)?
                    }
                };
                let _ = tx.send(Ok(result));
            }
        }
        ExecServerOutboundMessage::Error { request_id, error } => {
            if let Some(tx) = inner.pending.lock().await.remove(&request_id) {
                let _ = tx.send(Err(error));
            }
        }
        ExecServerOutboundMessage::Notification(notification) => {
            handle_in_process_notification(inner, notification).await;
        }
    }

    Ok(())
}

async fn handle_in_process_notification(
    inner: &Arc<Inner>,
    notification: ExecServerServerNotification,
) {
    match notification {
        ExecServerServerNotification::OutputDelta(params) => {
            let output = ExecServerOutput {
                stream: params.stream,
                chunk: params.chunk.into_inner(),
            };
            let processes = inner.processes.lock().await;
            if let Some(process) = processes.get(&params.session_id) {
                let _ = process.output_tx.send(output);
            }
        }
        ExecServerServerNotification::Exited(params) => {
            let mut processes = inner.processes.lock().await;
            if let Some(process) = processes.remove(&params.session_id) {
                process.status.mark_exited(Some(params.exit_code));
            }
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
            let output = ExecServerOutput {
                stream: params.stream,
                chunk: params.chunk.into_inner(),
            };
            let processes = inner.processes.lock().await;
            if let Some(process) = processes.get(&params.session_id) {
                let _ = process.output_tx.send(output);
            }
        }
        EXEC_EXITED_METHOD => {
            let params: ExecExitedNotification =
                serde_json::from_value(notification.params.unwrap_or(Value::Null))?;
            let mut processes = inner.processes.lock().await;
            if let Some(process) = processes.remove(&params.session_id) {
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
    use super::ExecServerOutput;
    use crate::protocol::EXEC_METHOD;
    use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
    use crate::protocol::EXEC_TERMINATE_METHOD;
    use crate::protocol::ExecOutputStream;
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

    fn test_options() -> ExecServerClientConnectOptions {
        ExecServerClientConnectOptions {
            client_name: "test-client".to_string(),
            initialize_timeout: Duration::from_secs(1),
        }
    }

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

        let client =
            ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await;
        if let Err(err) = client {
            panic!("failed to connect test client: {err}");
        }

        if let Err(err) = server.await {
            panic!("server task failed: {err}");
        }
    }

    #[tokio::test]
    async fn connect_in_process_starts_processes_without_jsonrpc_transport() {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let process = match client
            .start_process(ExecParams {
                argv: vec!["printf".to_string(), "hello".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: false,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start in-process child: {err}"),
        };

        let mut output = process.output_receiver();
        let output = timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap_or_else(|err| panic!("timed out waiting for process output: {err}"))
            .unwrap_or_else(|err| panic!("failed to receive process output: {err}"));
        assert_eq!(
            output,
            ExecServerOutput {
                stream: crate::protocol::ExecOutputStream::Stdout,
                chunk: b"hello".to_vec(),
            }
        );
    }

    #[tokio::test]
    async fn connect_in_process_rejects_invalid_exec_params_from_handler() {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let result = client
            .start_process(ExecParams {
                argv: Vec::new(),
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: false,
                arg0: None,
            })
            .await;

        match result {
            Err(ExecServerError::Server { code, message }) => {
                assert_eq!(code, -32602);
                assert_eq!(message, "argv must not be empty");
            }
            Err(err) => panic!("unexpected in-process exec failure: {err}"),
            Ok(_) => panic!("expected invalid params error"),
        }
    }

    #[tokio::test]
    async fn connect_in_process_rejects_writes_to_unknown_processes() {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let result = client
            .write_process(crate::protocol::WriteParams {
                session_id: "missing".to_string(),
                chunk: b"input".to_vec().into(),
            })
            .await;

        match result {
            Err(ExecServerError::Server { code, message }) => {
                assert_eq!(code, -32600);
                assert_eq!(message, "unknown session id missing");
            }
            Err(err) => panic!("unexpected in-process write failure: {err}"),
            Ok(_) => panic!("expected unknown process error"),
        }
    }

    #[tokio::test]
    async fn connect_in_process_terminate_marks_process_exited() {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let process = match client
            .start_process(ExecParams {
                argv: vec!["sleep".to_string(), "30".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: false,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start in-process child: {err}"),
        };

        if let Err(err) = client.terminate_session(&process.session_id).await {
            panic!("failed to terminate in-process child: {err}");
        }

        timeout(Duration::from_secs(2), async {
            loop {
                if process.has_exited() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|err| panic!("timed out waiting for in-process child to exit: {err}"));

        assert!(process.has_exited());
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

        let result =
            ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await;

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
            test_options(),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let result = client
            .start_process(ExecParams {
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
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

    #[tokio::test]
    async fn connect_stdio_times_out_during_initialize_handshake() {
        let (client_stdin, server_reader) = tokio::io::duplex(4096);
        let (_server_writer, client_stdout) = tokio::io::duplex(4096);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_reader).lines();
            let _ = read_jsonrpc_line(&mut lines).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let result = ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            ExecServerClientConnectOptions {
                client_name: "test-client".to_string(),
                initialize_timeout: Duration::from_millis(25),
            },
        )
        .await;

        match result {
            Err(ExecServerError::InitializeTimedOut { timeout }) => {
                assert_eq!(timeout, Duration::from_millis(25));
            }
            Err(err) => panic!("unexpected initialize timeout failure: {err}"),
            Ok(_) => panic!("expected initialize timeout"),
        }
    }

    #[tokio::test]
    async fn start_process_preserves_output_stream_metadata() {
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
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result: serde_json::json!({ "sessionId": "proc-1" }),
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(25)).await;
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Notification(JSONRPCNotification {
                    method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                    params: Some(serde_json::json!({
                        "sessionId": "proc-1",
                        "stream": "stderr",
                        "chunk": "ZXJyb3IK"
                    })),
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let client = match ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            test_options(),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let process = match client
            .start_process(ExecParams {
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start process: {err}"),
        };

        let mut output = process.output_receiver();
        let output = timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap_or_else(|err| panic!("timed out waiting for process output: {err}"))
            .unwrap_or_else(|err| panic!("failed to receive process output: {err}"));
        assert_eq!(output.stream, ExecOutputStream::Stderr);
        assert_eq!(output.chunk, b"error\n".to_vec());
    }

    #[tokio::test]
    async fn terminate_does_not_mark_process_exited_before_exit_notification() {
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
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result: serde_json::json!({ "sessionId": "proc-1" }),
                }),
            )
            .await;

            let terminate_request = read_jsonrpc_line(&mut lines).await;
            let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = terminate_request
            else {
                panic!("expected terminate request");
            };
            assert_eq!(method, EXEC_TERMINATE_METHOD);
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result: serde_json::json!({ "running": true }),
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let client = match ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            test_options(),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let process = match client
            .start_process(ExecParams {
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start process: {err}"),
        };

        process.terminate();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!process.has_exited(), "terminate should not imply exit");
        assert_eq!(process.exit_code(), None);
    }

    #[tokio::test]
    async fn start_process_uses_server_assigned_session_ids() {
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
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result: serde_json::json!({ "sessionId": "other-proc" }),
                }),
            )
            .await;
        });

        let client = match ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            test_options(),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let process = match client
            .start_process(ExecParams {
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start process: {err}"),
        };

        assert_eq!(process.session_id, "other-proc");
    }

    #[tokio::test]
    async fn start_process_routes_output_for_server_assigned_session_ids() {
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
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result: serde_json::json!({ "sessionId": "proc-1" }),
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(25)).await;
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Notification(JSONRPCNotification {
                    method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                    params: Some(serde_json::json!({
                        "sessionId": "proc-1",
                        "stream": "stdout",
                        "chunk": "YWxpdmUK"
                    })),
                }),
            )
            .await;
        });

        let client = match ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            test_options(),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let first_process = match client
            .start_process(ExecParams {
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start first process: {err}"),
        };

        let mut output = first_process.output_receiver();
        let output = timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap_or_else(|err| panic!("timed out waiting for process output: {err}"))
            .unwrap_or_else(|err| panic!("failed to receive process output: {err}"));
        assert_eq!(output.stream, ExecOutputStream::Stdout);
        assert_eq!(output.chunk, b"alive\n".to_vec());
    }

    #[tokio::test]
    async fn transport_shutdown_marks_processes_exited_without_exit_codes() {
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
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result: serde_json::json!({ "sessionId": "proc-1" }),
                }),
            )
            .await;
            drop(server_writer);
        });

        let client = match ExecServerClient::connect_stdio(
            client_stdin,
            client_stdout,
            test_options(),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

        let process = match client
            .start_process(ExecParams {
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: true,
                arg0: None,
            })
            .await
        {
            Ok(process) => process,
            Err(err) => panic!("failed to start process: {err}"),
        };

        let _ = process;
    }
}
