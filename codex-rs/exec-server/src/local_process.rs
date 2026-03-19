use std::sync::Arc;

use async_trait::async_trait;
use codex_app_server_protocol::JSONRPCErrorError;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::ExecProcess;
use crate::ExecServerError;
use crate::ExecServerEvent;
use crate::process::ExecServerEvent::Exited;
use crate::process::ExecServerEvent::OutputDelta;
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
use crate::server::ProcessHandler;

#[derive(Clone)]
pub(crate) struct LocalExecProcess {
    inner: Arc<Inner>,
}

struct Inner {
    process_handler: ProcessHandler,
    events_tx: broadcast::Sender<ExecServerEvent>,
    reader_task: tokio::task::JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let process_handler = self.process_handler.clone();
            handle.spawn(async move {
                process_handler.shutdown().await;
            });
        }
        self.reader_task.abort();
    }
}

impl LocalExecProcess {
    pub(crate) fn new() -> Self {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<RpcServerOutboundMessage>(256);
        let process_handler = ProcessHandler::new(RpcNotificationSender::new(outgoing_tx));
        let events_tx = broadcast::channel(256).0;
        let events_tx_for_task = events_tx.clone();
        let reader_task = tokio::spawn(async move {
            while let Some(message) = outgoing_rx.recv().await {
                if let RpcServerOutboundMessage::Notification(notification) = message {
                    match notification.method.as_str() {
                        EXEC_OUTPUT_DELTA_METHOD => {
                            if let Ok(params) = serde_json::from_value::<ExecOutputDeltaNotification>(
                                notification.params.unwrap_or(Value::Null),
                            ) {
                                let _ = events_tx_for_task.send(OutputDelta(params));
                            }
                        }
                        EXEC_EXITED_METHOD => {
                            if let Ok(params) = serde_json::from_value::<ExecExitedNotification>(
                                notification.params.unwrap_or(Value::Null),
                            ) {
                                let _ = events_tx_for_task.send(Exited(params));
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        Self {
            inner: Arc::new(Inner {
                process_handler,
                events_tx,
                reader_task,
            }),
        }
    }
}

#[async_trait]
impl ExecProcess for LocalExecProcess {
    async fn start(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.inner
            .process_handler
            .exec(params)
            .await
            .map_err(map_local_error)
    }

    async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        self.inner
            .process_handler
            .exec_read(params)
            .await
            .map_err(map_local_error)
    }

    async fn write(
        &self,
        process_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        self.inner
            .process_handler
            .exec_write(WriteParams {
                process_id: process_id.to_string(),
                chunk: chunk.into(),
            })
            .await
            .map_err(map_local_error)
    }

    async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError> {
        self.inner
            .process_handler
            .terminate(TerminateParams {
                process_id: process_id.to_string(),
            })
            .await
            .map_err(map_local_error)
    }

    fn subscribe_events(&self) -> broadcast::Receiver<ExecServerEvent> {
        self.inner.events_tx.subscribe()
    }
}

fn map_local_error(error: JSONRPCErrorError) -> ExecServerError {
    ExecServerError::Server {
        code: error.code,
        message: error.message,
    }
}
