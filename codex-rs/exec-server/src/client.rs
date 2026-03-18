use std::collections::HashMap;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
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
use crate::protocol::FS_COPY_METHOD;
use crate::protocol::FS_CREATE_DIRECTORY_METHOD;
use crate::protocol::FS_GET_METADATA_METHOD;
use crate::protocol::FS_READ_DIRECTORY_METHOD;
use crate::protocol::FS_READ_FILE_METHOD;
use crate::protocol::FS_REMOVE_METHOD;
use crate::protocol::FS_WRITE_FILE_METHOD;
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
    FsReadFile(oneshot::Sender<Result<FsReadFileResponse, JSONRPCErrorError>>),
    FsWriteFile(oneshot::Sender<Result<FsWriteFileResponse, JSONRPCErrorError>>),
    FsCreateDirectory(oneshot::Sender<Result<FsCreateDirectoryResponse, JSONRPCErrorError>>),
    FsGetMetadata(oneshot::Sender<Result<FsGetMetadataResponse, JSONRPCErrorError>>),
    FsReadDirectory(oneshot::Sender<Result<FsReadDirectoryResponse, JSONRPCErrorError>>),
    FsRemove(oneshot::Sender<Result<FsRemoveResponse, JSONRPCErrorError>>),
    FsCopy(oneshot::Sender<Result<FsCopyResponse, JSONRPCErrorError>>),
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
            PendingRequest::FsReadFile(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::FsWriteFile(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::FsCreateDirectory(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::FsGetMetadata(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::FsReadDirectory(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::FsRemove(tx) => {
                let _ = tx.send(Ok(serde_json::from_value(result)?));
            }
            PendingRequest::FsCopy(tx) => {
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
            PendingRequest::FsReadFile(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::FsWriteFile(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::FsCreateDirectory(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::FsGetMetadata(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::FsReadDirectory(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::FsRemove(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingRequest::FsCopy(tx) => {
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
    transport_tasks: Vec<JoinHandle<()>>,
    reader_task: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let ClientBackend::InProcess { handler } = &self.backend
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            let handler = Arc::clone(handler);
            handle.spawn(async move {
                handler.lock().await.shutdown().await;
            });
        }
        for task in &self.transport_tasks {
            task.abort();
        }
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
                transport_tasks: Vec::new(),
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
        let (write_tx, mut incoming_rx, transport_tasks) = connection.into_parts();
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
                transport_tasks,
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

    pub async fn fs_read_file(
        &self,
        params: FsReadFileParams,
    ) -> Result<FsReadFileResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_read_file(params).await);
        }

        self.send_pending_request(FS_READ_FILE_METHOD, &params, PendingRequest::FsReadFile)
            .await
    }

    pub async fn fs_write_file(
        &self,
        params: FsWriteFileParams,
    ) -> Result<FsWriteFileResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_write_file(params).await);
        }

        self.send_pending_request(FS_WRITE_FILE_METHOD, &params, PendingRequest::FsWriteFile)
            .await
    }

    pub async fn fs_create_directory(
        &self,
        params: FsCreateDirectoryParams,
    ) -> Result<FsCreateDirectoryResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_create_directory(params).await);
        }

        self.send_pending_request(
            FS_CREATE_DIRECTORY_METHOD,
            &params,
            PendingRequest::FsCreateDirectory,
        )
        .await
    }

    pub async fn fs_get_metadata(
        &self,
        params: FsGetMetadataParams,
    ) -> Result<FsGetMetadataResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_get_metadata(params).await);
        }

        self.send_pending_request(
            FS_GET_METADATA_METHOD,
            &params,
            PendingRequest::FsGetMetadata,
        )
        .await
    }

    pub async fn fs_read_directory(
        &self,
        params: FsReadDirectoryParams,
    ) -> Result<FsReadDirectoryResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_read_directory(params).await);
        }

        self.send_pending_request(
            FS_READ_DIRECTORY_METHOD,
            &params,
            PendingRequest::FsReadDirectory,
        )
        .await
    }

    pub async fn fs_remove(
        &self,
        params: FsRemoveParams,
    ) -> Result<FsRemoveResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_remove(params).await);
        }

        self.send_pending_request(FS_REMOVE_METHOD, &params, PendingRequest::FsRemove)
            .await
    }

    pub async fn fs_copy(&self, params: FsCopyParams) -> Result<FsCopyResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.fs_copy(params).await);
        }

        self.send_pending_request(FS_COPY_METHOD, &params, PendingRequest::FsCopy)
            .await
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

        self.send_pending_request(EXEC_METHOD, &params, PendingRequest::Exec)
            .await
    }

    async fn write_process(&self, params: WriteParams) -> Result<WriteResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.write(params).await);
        }

        self.send_pending_request(EXEC_WRITE_METHOD, &params, PendingRequest::Write)
            .await
    }

    async fn request_read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        if let ClientBackend::InProcess { handler } = &self.inner.backend {
            return server_result_to_client(handler.lock().await.read(params).await);
        }

        self.send_pending_request(EXEC_READ_METHOD, &params, PendingRequest::Read)
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

        self.send_pending_request(EXEC_TERMINATE_METHOD, &params, PendingRequest::Terminate)
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

        self.send_pending_request(INITIALIZE_METHOD, &params, PendingRequest::Initialize)
            .await
    }

    fn next_request_id(&self) -> RequestId {
        RequestId::Integer(self.inner.next_request_id.fetch_add(1, Ordering::SeqCst))
    }

    async fn send_pending_request<P, T>(
        &self,
        method: &str,
        params: &P,
        build_pending: impl FnOnce(oneshot::Sender<Result<T, JSONRPCErrorError>>) -> PendingRequest,
    ) -> Result<T, ExecServerError>
    where
        P: Serialize,
    {
        let request_id = self.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.clone(), build_pending(response_tx));
        let ClientBackend::JsonRpc { write_tx } = &self.inner.backend else {
            unreachable!("in-process requests return before JSON-RPC setup");
        };
        let send_result = send_jsonrpc_request(write_tx, request_id.clone(), method, params).await;
        self.finish_request(request_id, send_result, response_rx)
            .await
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
mod tests;
