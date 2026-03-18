use std::sync::Arc;

use tokio::sync::Mutex;

use crate::protocol::InitializeResponse;
use crate::server::ExecServerHandler;

use super::ExecServerError;

#[derive(Clone)]
pub(super) struct LocalBackend {
    handler: Arc<Mutex<ExecServerHandler>>,
}

impl LocalBackend {
    pub(super) fn new(handler: ExecServerHandler) -> Self {
        Self {
            handler: Arc::new(Mutex::new(handler)),
        }
    }

    pub(super) async fn shutdown(&self) {
        self.handler.lock().await.shutdown().await;
    }

    pub(super) async fn initialize(&self) -> Result<InitializeResponse, ExecServerError> {
        self.handler
            .lock()
            .await
            .initialize()
            .map_err(|error| ExecServerError::Server {
                code: error.code,
                message: error.message,
            })
    }

    pub(super) async fn initialized(&self) -> Result<(), ExecServerError> {
        self.handler
            .lock()
            .await
            .initialized()
            .map_err(ExecServerError::Protocol)
    }
}
