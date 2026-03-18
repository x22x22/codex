use std::collections::HashMap;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;
#[cfg(test)]
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

use crate::client_api::ExecServerClientConnectOptions;
use crate::client_api::ExecServerEvent;
use crate::client_api::RemoteExecServerConnectArgs;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
use crate::protocol::EXEC_EXITED_METHOD;
use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::EXEC_READ_METHOD;
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
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;
use crate::server::ExecServerHandler;
use crate::server::ExecServerOutboundMessage;
use crate::server::ExecServerServerNotification;

impl Default for ExecServerClientConnectOptions {
    fn default() -> Self {
        Self {
            client_name: "codex-core".to_string(),
            initialize_timeout: INITIALIZE_TIMEOUT,
        }
    }
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

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecServerOutput {
    stream: crate::protocol::ExecOutputStream,
    chunk: Vec<u8>,
}

#[cfg(test)]
struct ExecServerProcess {
    process_id: String,
    output_rx: broadcast::Receiver<ExecServerOutput>,
    status: Arc<RemoteProcessStatus>,
    client: ExecServerClient,
}

#[cfg(test)]
impl ExecServerProcess {
    fn output_receiver(&self) -> broadcast::Receiver<ExecServerOutput> {
        self.output_rx.resubscribe()
    }

    fn has_exited(&self) -> bool {
        self.status.has_exited()
    }

    fn exit_code(&self) -> Option<i32> {
        self.status.exit_code()
    }

    fn terminate(&self) {
        let client = self.client.clone();
        let process_id = self.process_id.clone();
        tokio::spawn(async move {
            let _ = client.terminate_session(&process_id).await;
        });
    }
}

#[cfg(test)]
struct RemoteProcessStatus {
    exited: AtomicBool,
    exit_code: StdMutex<Option<i32>>,
}

#[cfg(test)]
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

enum PendingRequest {
    Initialize(oneshot::Sender<Result<InitializeResponse, JSONRPCErrorError>>),
    Exec(oneshot::Sender<Result<ExecResponse, JSONRPCErrorError>>),
    Read(oneshot::Sender<Result<ReadResponse, JSONRPCErrorError>>),
    Write(oneshot::Sender<Result<WriteResponse, JSONRPCErrorError>>),
    Terminate(oneshot::Sender<Result<TerminateResponse, JSONRPCErrorError>>),
}

impl PendingRequest {
    fn resolve_json(self, result: Value) -> Result<(), ExecServerError> {
        match self {
            PendingRequest::Initialize(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::Exec(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::Read(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::Write(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::Terminate(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
        }
        Ok(())
    }

    fn resolve_error(self, error: JSONRPCErrorError) {
        match self {
            PendingRequest::Initialize(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::Exec(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::Read(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::Write(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::Terminate(tx) => {
                let _ = tx.send(Err(error));
            }
        }
    }
}

enum ClientBackend {
    JsonRpc {
        write_tx: mpsc::Sender<JSONRPCMessage>,
    },
    InProcess {
        handler: Arc<Mutex<ExecServerHandler>>,
    },
}

struct Inner {
    backend: ClientBackend,
    pending: Mutex<HashMap<RequestId, PendingRequest>>,
    events_tx: broadcast::Sender<ExecServerEvent>,
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
        let (outbound_tx, mut outgoing_rx) = mpsc::channel::<ExecServerOutboundMessage>(256);
        let handler = Arc::new(Mutex::new(ExecServerHandler::new(outbound_tx)));

        let inner = Arc::new_cyclic(|weak| {
            let weak = weak.clone();
            let reader_task = tokio::spawn(async move {
                while let Some(message) = outgoing_rx.recv().await {
                    if let Some(inner) = weak.upgrade()
                        && let Err(err) = handle_in_process_outbound_message(&inner, message).await
                    {
                        warn!(
                            "in-process exec-server client closing after unexpected response: {err}"
                        );
                        handle_transport_shutdown(&inner).await;
                        return;
                    }
                }

                if let Some(inner) = weak.upgrade() {
                    handle_transport_shutdown(&inner).await;
                }
            });

            Inner {
                backend: ClientBackend::InProcess { handler },
                pending: Mutex::new(HashMap::new()),
                events_tx: broadcast::channel(256).0,
                next_request_id: AtomicI64::new(1),
                reader_task,
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
                events_tx: broadcast::channel(256).0,
                next_request_id: AtomicI64::new(1),
                reader_task,
            }
        });

        let client = Self { inner };
        client.initialize(options).await?;
        Ok(client)
    }

    pub fn event_receiver(&self) -> broadcast::Receiver<ExecServerEvent> {
        self.inner.events_tx.subscribe()
    }

    #[cfg(test)]
    async fn start_process(
        &self,
        params: ExecParams,
    ) -> Result<ExecServerProcess, ExecServerError> {
        let response = self.exec(params).await?;
        let process_id = response.process_id;
        let status = Arc::new(RemoteProcessStatus::new());
        let (output_tx, output_rx) = broadcast::channel(256);
        let mut events_rx = self.event_receiver();
        let status_watcher = Arc::clone(&status);
        let watch_process_id = process_id.clone();
        tokio::spawn(async move {
            while let Ok(event) = events_rx.recv().await {
                match event {
                    ExecServerEvent::OutputDelta(notification)
                        if notification.process_id == watch_process_id =>
                    {
                        let _ = output_tx.send(ExecServerOutput {
                            stream: notification.stream,
                            chunk: notification.chunk.into_inner(),
                        });
                    }
                    ExecServerEvent::Exited(notification)
                        if notification.process_id == watch_process_id =>
                    {
                        status_watcher.mark_exited(Some(notification.exit_code));
                        break;
                    }
                    ExecServerEvent::OutputDelta(_) | ExecServerEvent::Exited(_) => {}
                }
            }
        });

        Ok(ExecServerProcess {
            process_id,
            output_rx,
            status,
            client: self.clone(),
        })
    }

    pub async fn exec(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.request_exec(params).await
    }

    pub async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        self.request_read(params).await
    }

    pub async fn write(
        &self,
        process_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        self.write_process(WriteParams {
            process_id: process_id.to_string(),
            chunk: chunk.into(),
        })
        .await
    }

    pub async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError> {
        self.terminate_session(process_id).await
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
                .request_initialize(InitializeParams { client_name })
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
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.exec(params).await);
        }

        let request_id = self.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), PendingRequest::Exec(response_tx));
        let ClientBackend::JsonRpc { write_tx } = &self.inner.backend else {
            unreachable!("in-process exec requests return before JSON-RPC setup");
        };
        let send_result =
            send_jsonrpc_request(write_tx, request_id.clone(), EXEC_METHOD, &params).await;
        self.finish_request(request_id, send_result, response_rx)
            .await
    }

    async fn write_process(&self, params: WriteParams) -> Result<WriteResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.write(params).await);
        }

        let request_id = self.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), PendingRequest::Write(response_tx));
        let ClientBackend::JsonRpc { write_tx } = &self.inner.backend else {
            unreachable!("in-process write requests return before JSON-RPC setup");
        };
        let send_result =
            send_jsonrpc_request(write_tx, request_id.clone(), EXEC_WRITE_METHOD, &params).await;
        self.finish_request(request_id, send_result, response_rx)
            .await
    }

    async fn request_read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.read(params).await);
        }

        let request_id = self.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), PendingRequest::Read(response_tx));
        let ClientBackend::JsonRpc { write_tx } = &self.inner.backend else {
            unreachable!("in-process read requests return before JSON-RPC setup");
        };
        let send_result =
            send_jsonrpc_request(write_tx, request_id.clone(), EXEC_READ_METHOD, &params).await;
        self.finish_request(request_id, send_result, response_rx)
            .await
    }

    async fn terminate_session(
        &self,
        process_id: &str,
    ) -> Result<TerminateResponse, ExecServerError> {
        let params = TerminateParams {
            process_id: process_id.to_string(),
        };
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.terminate(params).await);
        }

        let request_id = self.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), PendingRequest::Terminate(response_tx));
        let ClientBackend::JsonRpc { write_tx } = &self.inner.backend else {
            unreachable!("in-process terminate requests return before JSON-RPC setup");
        };
        let send_result =
            send_jsonrpc_request(write_tx, request_id.clone(), EXEC_TERMINATE_METHOD, &params)
                .await;
        self.finish_request(request_id, send_result, response_rx)
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
            ClientBackend::InProcess { handler } => match method {
                INITIALIZED_METHOD => handler
                    .lock()
                    .await
                    .initialized()
                    .map_err(ExecServerError::Protocol),
                other => Err(ExecServerError::Protocol(format!(
                    "unsupported in-process notification method `{other}`"
                ))),
            },
        }
    }

    async fn request_initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.initialize());
        }

        let request_id = self.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), PendingRequest::Initialize(response_tx));
        let ClientBackend::JsonRpc { write_tx } = &self.inner.backend else {
            unreachable!("in-process initialize requests return before JSON-RPC setup");
        };
        let send_result =
            send_jsonrpc_request(write_tx, request_id.clone(), INITIALIZE_METHOD, &params).await;
        self.finish_request(request_id, send_result, response_rx)
            .await
    }

    fn next_request_id(&self) -> RequestId {
        RequestId::Integer(self.inner.next_request_id.fetch_add(1, Ordering::SeqCst))
    }

    async fn finish_request<T>(
        &self,
        request_id: RequestId,
        send_result: Result<(), ExecServerError>,
        response_rx: oneshot::Receiver<Result<T, JSONRPCErrorError>>,
    ) -> Result<T, ExecServerError> {
        if let Err(err) = send_result {
            self.inner.pending.lock().await.remove(&request_id);
            return Err(err);
        }
        receive_typed_response(response_rx).await
    }
}

async fn receive_typed_response<T>(
    response_rx: oneshot::Receiver<Result<T, JSONRPCErrorError>>,
) -> Result<T, ExecServerError> {
    let result = response_rx.await.map_err(|_| ExecServerError::Closed)?;
    match result {
        Ok(response) => Ok(response),
        Err(error) => Err(ExecServerError::Server {
            code: error.code,
            message: error.message,
        }),
    }
}

fn server_result_to_client<T>(result: Result<T, JSONRPCErrorError>) -> Result<T, ExecServerError> {
    match result {
        Ok(response) => Ok(response),
        Err(error) => Err(ExecServerError::Server {
            code: error.code,
            message: error.message,
        }),
    }
}

async fn send_jsonrpc_request<P: Serialize>(
    write_tx: &mpsc::Sender<JSONRPCMessage>,
    request_id: RequestId,
    method: &str,
    params: &P,
) -> Result<(), ExecServerError> {
    let params = serde_json::to_value(params)?;
    write_tx
        .send(JSONRPCMessage::Request(JSONRPCRequest {
            id: request_id,
            method: method.to_string(),
            params: Some(params),
            trace: None,
        }))
        .await
        .map_err(|_| ExecServerError::Closed)
}

async fn handle_in_process_outbound_message(
    inner: &Arc<Inner>,
    message: ExecServerOutboundMessage,
) -> Result<(), ExecServerError> {
    match message {
        ExecServerOutboundMessage::Response { .. } | ExecServerOutboundMessage::Error { .. } => {
            return Err(ExecServerError::Protocol(
                "unexpected in-process RPC response".to_string(),
            ));
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
            let _ = inner.events_tx.send(ExecServerEvent::OutputDelta(params));
        }
        ExecServerServerNotification::Exited(params) => {
            let _ = inner.events_tx.send(ExecServerEvent::Exited(params));
        }
    }
}

async fn handle_server_message(
    inner: &Arc<Inner>,
    message: JSONRPCMessage,
) -> Result<(), ExecServerError> {
    match message {
        JSONRPCMessage::Response(JSONRPCResponse { id, result }) => {
            if let Some(pending) = inner.pending.lock().await.remove(&id) {
                pending.resolve_json(result)?;
            }
        }
        JSONRPCMessage::Error(JSONRPCError { id, error }) => {
            if let Some(pending) = inner.pending.lock().await.remove(&id) {
                pending.resolve_error(error);
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
            let _ = inner.events_tx.send(ExecServerEvent::OutputDelta(params));
        }
        EXEC_EXITED_METHOD => {
            let params: ExecExitedNotification =
                serde_json::from_value(notification.params.unwrap_or(Value::Null))?;
            let _ = inner.events_tx.send(ExecServerEvent::Exited(params));
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
        pending
            .drain()
            .map(|(_, pending)| pending)
            .collect::<Vec<_>>()
    };
    for pending in pending {
        pending.resolve_error(JSONRPCErrorError {
            code: -32000,
            data: None,
            message: "exec-server transport closed".to_string(),
        });
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
    use crate::protocol::ReadParams;
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
                    result: serde_json::json!({}),
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
                process_id: "proc-1".to_string(),
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
    async fn connect_in_process_read_returns_retained_output_and_exit_state() {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let response = match client
            .exec(ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["printf".to_string(), "hello".to_string()],
                cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
                env: HashMap::new(),
                tty: false,
                arg0: None,
            })
            .await
        {
            Ok(response) => response,
            Err(err) => panic!("failed to start in-process child: {err}"),
        };

        let read = match client
            .read(ReadParams {
                process_id: response.process_id,
                after_seq: None,
                max_bytes: None,
                wait_ms: Some(1000),
            })
            .await
        {
            Ok(read) => read,
            Err(err) => panic!("failed to read in-process child output: {err}"),
        };

        assert_eq!(read.chunks.len(), 1);
        assert_eq!(read.chunks[0].seq, 1);
        assert_eq!(read.chunks[0].stream, ExecOutputStream::Stdout);
        assert_eq!(read.chunks[0].chunk.clone().into_inner(), b"hello".to_vec());
        assert_eq!(read.next_seq, 2);
        assert!(read.exited);
        assert_eq!(read.exit_code, Some(0));
    }

    #[tokio::test]
    async fn connect_in_process_rejects_invalid_exec_params_from_handler() {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let result = client
            .start_process(ExecParams {
                process_id: "proc-1".to_string(),
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
                process_id: "missing".to_string(),
                chunk: b"input".to_vec().into(),
            })
            .await;

        match result {
            Err(ExecServerError::Server { code, message }) => {
                assert_eq!(code, -32600);
                assert_eq!(message, "unknown process id missing");
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
                process_id: "proc-1".to_string(),
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

        if let Err(err) = client.terminate_session(&process.process_id).await {
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
                    result: serde_json::json!({}),
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
                process_id: "proc-1".to_string(),
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
            client.inner.pending.lock().await.is_empty(),
            "failed requests should not leave pending request state behind"
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
                    result: serde_json::json!({}),
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
                    result: serde_json::json!({ "processId": "proc-1" }),
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(25)).await;
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Notification(JSONRPCNotification {
                    method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                    params: Some(serde_json::json!({
                        "processId": "proc-1",
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
                process_id: "proc-1".to_string(),
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
                    result: serde_json::json!({}),
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
                    result: serde_json::json!({ "processId": "proc-1" }),
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
                process_id: "proc-1".to_string(),
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
    async fn start_process_uses_protocol_process_ids() {
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
                    result: serde_json::json!({}),
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
                    result: serde_json::json!({ "processId": "other-proc" }),
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
                process_id: "proc-1".to_string(),
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

        assert_eq!(process.process_id, "other-proc");
    }

    #[tokio::test]
    async fn start_process_routes_output_for_protocol_process_ids() {
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
                    result: serde_json::json!({}),
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
                    result: serde_json::json!({ "processId": "proc-1" }),
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(25)).await;
            write_jsonrpc_line(
                &mut server_writer,
                JSONRPCMessage::Notification(JSONRPCNotification {
                    method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                    params: Some(serde_json::json!({
                        "processId": "proc-1",
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
                process_id: "proc-1".to_string(),
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
                    result: serde_json::json!({}),
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
                    result: serde_json::json!({ "processId": "proc-1" }),
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
                process_id: "proc-1".to_string(),
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
