use std::future::Future;
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
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCNotification;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::debug;
use tracing::warn;

use crate::client_api::ExecServerClientConnectOptions;
use crate::client_api::ExecServerEvent;
use crate::client_api::RemoteExecServerConnectArgs;
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
use crate::protocol::InitializeResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;
use crate::rpc::RpcClientEvent;
use crate::server::ExecServerOutboundMessage;
use crate::server::ExecServerServerNotification;

mod jsonrpc_backend;
mod local_backend;
#[cfg(test)]
mod process;
use jsonrpc_backend::JsonRpcBackend;
use local_backend::LocalBackend;
#[cfg(test)]
use process::ExecServerOutput;
#[cfg(test)]
use process::ExecServerProcess;
#[cfg(test)]
use process::RemoteProcessStatus;

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

enum ClientBackend {
    JsonRpc(JsonRpcBackend),
    InProcess(LocalBackend),
}

impl ClientBackend {
    fn as_local(&self) -> Option<&LocalBackend> {
        match self {
            ClientBackend::JsonRpc(_) => None,
            ClientBackend::InProcess(backend) => Some(backend),
        }
    }
}

struct Inner {
    backend: ClientBackend,
    events_tx: broadcast::Sender<ExecServerEvent>,
    reader_task: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(backend) = self.backend.as_local()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            let backend = backend.clone();
            handle.spawn(async move {
                backend.shutdown().await;
            });
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
        let backend = LocalBackend::new(crate::server::ExecServerHandler::new(outbound_tx));

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
                        return;
                    }
                }
            });

            Inner {
                backend: ClientBackend::InProcess(backend),
                events_tx: broadcast::channel(256).0,
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
        let (backend, events_rx) = JsonRpcBackend::connect_stdio(stdin, stdout);
        Self::connect(backend, events_rx, options).await
    }

    pub async fn connect_websocket(
        args: RemoteExecServerConnectArgs,
    ) -> Result<Self, ExecServerError> {
        let options = args.clone().into();
        let (backend, events_rx) = JsonRpcBackend::connect_websocket(&args).await?;
        Self::connect(backend, events_rx, options).await
    }

    async fn connect(
        backend: JsonRpcBackend,
        mut rpc_events_rx: mpsc::Receiver<RpcClientEvent>,
        options: ExecServerClientConnectOptions,
    ) -> Result<Self, ExecServerError> {
        let inner = Arc::new_cyclic(|weak| {
            let weak = weak.clone();
            let reader_task = tokio::spawn(async move {
                while let Some(event) = rpc_events_rx.recv().await {
                    match event {
                        RpcClientEvent::Notification(notification) => {
                            if let Some(inner) = weak.upgrade()
                                && let Err(err) =
                                    handle_server_notification(&inner, notification).await
                            {
                                warn!("exec-server client closing after protocol error: {err}");
                                return;
                            }
                        }
                        RpcClientEvent::Disconnected { reason } => {
                            if let Some(reason) = reason {
                                warn!("exec-server client transport disconnected: {reason}");
                            }
                            return;
                        }
                    }
                }
            });

            Inner {
                backend: ClientBackend::JsonRpc(backend),
                events_tx: broadcast::channel(256).0,
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
        self.request_or_local(EXEC_METHOD, params, |backend, params| async move {
            backend.exec(params).await
        })
        .await
    }

    pub async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        self.request_or_local(EXEC_READ_METHOD, params, |backend, params| async move {
            backend.exec_read(params).await
        })
        .await
    }

    pub async fn write(
        &self,
        process_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        let params = WriteParams {
            process_id: process_id.to_string(),
            chunk: chunk.into(),
        };
        self.request_or_local(EXEC_WRITE_METHOD, params, |backend, params| async move {
            backend.exec_write(params).await
        })
        .await
    }

    pub async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError> {
        let params = TerminateParams {
            process_id: process_id.to_string(),
        };
        self.request_or_local(
            EXEC_TERMINATE_METHOD,
            params,
            |backend, params| async move { backend.terminate(params).await },
        )
        .await
    }

    pub async fn fs_read_file(
        &self,
        params: FsReadFileParams,
    ) -> Result<FsReadFileResponse, ExecServerError> {
        self.request_or_local(FS_READ_FILE_METHOD, params, |backend, params| async move {
            backend.fs_read_file(params).await
        })
        .await
    }

    pub async fn fs_write_file(
        &self,
        params: FsWriteFileParams,
    ) -> Result<FsWriteFileResponse, ExecServerError> {
        self.request_or_local(FS_WRITE_FILE_METHOD, params, |backend, params| async move {
            backend.fs_write_file(params).await
        })
        .await
    }

    pub async fn fs_create_directory(
        &self,
        params: FsCreateDirectoryParams,
    ) -> Result<FsCreateDirectoryResponse, ExecServerError> {
        self.request_or_local(
            FS_CREATE_DIRECTORY_METHOD,
            params,
            |backend, params| async move { backend.fs_create_directory(params).await },
        )
        .await
    }

    pub async fn fs_get_metadata(
        &self,
        params: FsGetMetadataParams,
    ) -> Result<FsGetMetadataResponse, ExecServerError> {
        self.request_or_local(
            FS_GET_METADATA_METHOD,
            params,
            |backend, params| async move { backend.fs_get_metadata(params).await },
        )
        .await
    }

    pub async fn fs_read_directory(
        &self,
        params: FsReadDirectoryParams,
    ) -> Result<FsReadDirectoryResponse, ExecServerError> {
        self.request_or_local(
            FS_READ_DIRECTORY_METHOD,
            params,
            |backend, params| async move { backend.fs_read_directory(params).await },
        )
        .await
    }

    pub async fn fs_remove(
        &self,
        params: FsRemoveParams,
    ) -> Result<FsRemoveResponse, ExecServerError> {
        self.request_or_local(FS_REMOVE_METHOD, params, |backend, params| async move {
            backend.fs_remove(params).await
        })
        .await
    }

    pub async fn fs_copy(&self, params: FsCopyParams) -> Result<FsCopyResponse, ExecServerError> {
        self.request_or_local(FS_COPY_METHOD, params, |backend, params| async move {
            backend.fs_copy(params).await
        })
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
            if let Some(backend) = self.inner.backend.as_local() {
                backend.initialize().await?;
            } else {
                let params = crate::protocol::InitializeParams { client_name };
                let _: InitializeResponse =
                    self.send_remote_request(INITIALIZE_METHOD, &params).await?;
            }
            self.notify(INITIALIZED_METHOD, &serde_json::json!({}))
                .await
        })
        .await
        .map_err(|_| ExecServerError::InitializeTimedOut {
            timeout: initialize_timeout,
        })?
    }

    async fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<(), ExecServerError> {
        match &self.inner.backend {
            ClientBackend::JsonRpc(backend) => backend.notify(method, params).await,
            ClientBackend::InProcess(backend) => backend.notify(method).await,
        }
    }

    async fn send_remote_request<P, T>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<T, ExecServerError>
    where
        P: Serialize,
        T: serde::de::DeserializeOwned,
    {
        let ClientBackend::JsonRpc(backend) = &self.inner.backend else {
            unreachable!("in-process requests return before JSON-RPC setup");
        };
        backend.call(method, params).await
    }

    #[cfg(test)]
    async fn pending_request_count(&self) -> usize {
        match &self.inner.backend {
            ClientBackend::JsonRpc(backend) => backend.pending_request_count().await,
            ClientBackend::InProcess(_) => 0,
        }
    }

    async fn request_or_local<P, T, Fut>(
        &self,
        method: &str,
        params: P,
        call_local: impl FnOnce(LocalBackend, P) -> Fut,
    ) -> Result<T, ExecServerError>
    where
        P: Serialize,
        T: serde::de::DeserializeOwned,
        Fut: Future<Output = Result<T, ExecServerError>>,
    {
        if let Some(backend) = self.inner.backend.as_local() {
            return call_local(backend.clone(), params).await;
        }

        self.send_remote_request(method, &params).await
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

#[cfg(test)]
mod tests;
