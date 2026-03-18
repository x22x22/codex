use std::sync::Arc;

use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::InitializeResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;
use crate::server::ExecServerHandler;

use super::ExecServerError;

#[derive(Clone)]
pub(super) struct LocalBackend {
    handler: Arc<ExecServerHandler>,
}

impl LocalBackend {
    pub(super) fn new(handler: ExecServerHandler) -> Self {
        Self {
            handler: Arc::new(handler),
        }
    }

    pub(super) async fn shutdown(&self) {
        self.handler.shutdown().await;
    }

    pub(super) async fn initialize(&self) -> Result<InitializeResponse, ExecServerError> {
        self.handler
            .initialize()
            .map_err(|error| ExecServerError::Server {
                code: error.code,
                message: error.message,
            })
    }

    pub(super) async fn initialized(&self) -> Result<(), ExecServerError> {
        self.handler
            .initialized()
            .map_err(ExecServerError::Protocol)
    }

    pub(super) async fn exec(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.handler
            .exec(params)
            .await
            .map_err(|error| ExecServerError::Server {
                code: error.code,
                message: error.message,
            })
    }

    pub(super) async fn exec_read(
        &self,
        params: ReadParams,
    ) -> Result<ReadResponse, ExecServerError> {
        self.handler
            .exec_read(params)
            .await
            .map_err(|error| ExecServerError::Server {
                code: error.code,
                message: error.message,
            })
    }

    pub(super) async fn exec_write(
        &self,
        params: WriteParams,
    ) -> Result<WriteResponse, ExecServerError> {
        self.handler
            .exec_write(params)
            .await
            .map_err(|error| ExecServerError::Server {
                code: error.code,
                message: error.message,
            })
    }

    pub(super) async fn terminate(
        &self,
        params: TerminateParams,
    ) -> Result<TerminateResponse, ExecServerError> {
        self.handler
            .terminate(params)
            .await
            .map_err(|error| ExecServerError::Server {
                code: error.code,
                message: error.message,
            })
    }
}
