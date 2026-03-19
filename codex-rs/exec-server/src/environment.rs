use crate::ExecServerClient;
use crate::ExecServerError;
use crate::ExecServerEvent;
use crate::ExecProcess;
use crate::ExecutorEnvironment;
use crate::RemoteExecServerConnectArgs;
use crate::protocol::EXEC_EXITED_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;
use crate::rpc::RpcNotificationSender;
use crate::rpc::RpcServerOutboundMessage;
use crate::server::ExecServerHandler;
use crate::fs;
use crate::fs::ExecutorFileSystem;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::sync::OnceCell;

#[derive(Clone)]
struct LocalExecProcess {
    handler: Arc<ExecServerHandler>,
    events_tx: broadcast::Sender<ExecServerEvent>,
    outgoing_rx: Arc<Mutex<Option<mpsc::Receiver<RpcServerOutboundMessage>>>>,
    reader_task_started: Arc<OnceCell<()>>,
}

impl Default for LocalExecProcess {
    fn default() -> Self {
        let (outgoing_tx, outgoing_rx) = mpsc::channel(256);
        let handler = Arc::new(ExecServerHandler::new(RpcNotificationSender::new(outgoing_tx)));
        let (_initialize_response) = handler
            .initialize()
            .expect("new local exec process should initialize once");
        handler
            .initialized()
            .expect("new local exec process should accept initialized notification");

        Self {
            handler,
            events_tx: broadcast::channel(256).0,
            outgoing_rx: Arc::new(Mutex::new(Some(outgoing_rx))),
            reader_task_started: Arc::new(OnceCell::new()),
        }
    }
}

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    remote_exec_server_client: Option<ExecServerClient>,
    local_exec_process: LocalExecProcess,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            experimental_exec_server_url: None,
            remote_exec_server_client: None,
            local_exec_process: LocalExecProcess::default(),
        }
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field(
                "experimental_exec_server_url",
                &self.experimental_exec_server_url,
            )
            .field(
                "has_remote_exec_server_client",
                &self.remote_exec_server_client.is_some(),
            )
            .finish()
    }
}

impl Environment {
    pub async fn create(
        experimental_exec_server_url: Option<String>,
    ) -> Result<Self, ExecServerError> {
        let remote_exec_server_client =
            if let Some(websocket_url) = experimental_exec_server_url.as_deref() {
                Some(
                    ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
                    websocket_url.to_string(),
                    "codex-core".to_string(),
                ))
                    .await?,
                )
            } else {
                None
            };

        Ok(Self {
            experimental_exec_server_url,
            remote_exec_server_client,
            local_exec_process: LocalExecProcess::default(),
        })
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    pub fn remote_exec_server_client(&self) -> Option<&ExecServerClient> {
        self.remote_exec_server_client.as_ref()
    }

    pub fn process(&self) -> &(dyn ExecProcess + '_) {
        self.remote_exec_server_client
            .as_ref()
            .map_or(&self.local_exec_process as &dyn ExecProcess, |client| client)
    }

    pub fn get_filesystem(&self) -> impl ExecutorFileSystem + use<> {
        fs::LocalFileSystem
    }
}

impl ExecutorEnvironment for Environment {
    fn process(&self) -> &(dyn ExecProcess + '_) {
        self.process()
    }
}

#[async_trait]
impl ExecProcess for LocalExecProcess {
    async fn start(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.ensure_reader_task().await;
        self.handler.exec(params).await.map_err(local_server_error)
    }

    async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        self.ensure_reader_task().await;
        self.handler.exec_read(params).await.map_err(local_server_error)
    }

    async fn write(
        &self,
        process_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        self.ensure_reader_task().await;
        self.handler
            .exec_write(WriteParams {
                process_id: process_id.to_string(),
                chunk: chunk.into(),
            })
            .await
            .map_err(local_server_error)
    }

    async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError> {
        self.ensure_reader_task().await;
        self.handler
            .terminate(TerminateParams {
                process_id: process_id.to_string(),
            })
            .await
            .map_err(local_server_error)
    }

    fn subscribe_events(&self) -> broadcast::Receiver<ExecServerEvent> {
        self.events_tx.subscribe()
    }
}

impl LocalExecProcess {
    async fn ensure_reader_task(&self) {
        let _ = self
            .reader_task_started
            .get_or_init(|| async {
                let mut outgoing_rx = self.outgoing_rx.lock().await;
                let Some(mut outgoing_rx) = outgoing_rx.take() else {
                    return;
                };
                let events_tx = self.events_tx.clone();
                tokio::spawn(async move {
                    while let Some(message) = outgoing_rx.recv().await {
                        if let RpcServerOutboundMessage::Notification(notification) = message {
                            match notification.method.as_str() {
                                EXEC_OUTPUT_DELTA_METHOD => {
                                    if let Ok(params) = serde_json::from_value::<
                                        ExecOutputDeltaNotification,
                                    >(
                                        notification.params.unwrap_or(Value::Null)
                                    ) {
                                        let _ = events_tx.send(ExecServerEvent::OutputDelta(params));
                                    }
                                }
                                EXEC_EXITED_METHOD => {
                                    if let Ok(params) =
                                        serde_json::from_value::<ExecExitedNotification>(
                                            notification.params.unwrap_or(Value::Null),
                                        )
                                    {
                                        let _ = events_tx.send(ExecServerEvent::Exited(params));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                });
            })
            .await;
    }
}

fn local_server_error(error: codex_app_server_protocol::JSONRPCErrorError) -> ExecServerError {
    ExecServerError::Server {
        code: error.code,
        message: error.message,
    }
}

#[cfg(test)]
mod tests {
    use super::Environment;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn create_without_remote_exec_server_url_does_not_connect() {
        let environment = Environment::create(None).await.expect("create environment");

        assert_eq!(environment.experimental_exec_server_url(), None);
        assert!(environment.remote_exec_server_client().is_none());
    }
}
