use std::sync::Arc;

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
use tokio::sync::Mutex;

use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;
use crate::server::ExecServerHandler;

use super::ExecServerError;
use super::server_result_to_client;

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
        server_result_to_client(self.handler.lock().await.initialize())
    }

    pub(super) async fn notify(&self, method: &str) -> Result<(), ExecServerError> {
        match method {
            INITIALIZED_METHOD => self
                .handler
                .lock()
                .await
                .initialized()
                .map_err(ExecServerError::Protocol),
            other => Err(ExecServerError::Protocol(format!(
                "unsupported in-process notification method `{other}`"
            ))),
        }
    }

    pub(super) async fn exec(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.exec(params).await)
    }

    pub(super) async fn exec_read(
        &self,
        params: ReadParams,
    ) -> Result<ReadResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.exec_read(params).await)
    }

    pub(super) async fn exec_write(
        &self,
        params: WriteParams,
    ) -> Result<WriteResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.exec_write(params).await)
    }

    pub(super) async fn terminate(
        &self,
        params: TerminateParams,
    ) -> Result<TerminateResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.terminate(params).await)
    }

    pub(super) async fn fs_read_file(
        &self,
        params: FsReadFileParams,
    ) -> Result<FsReadFileResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_read_file(params).await)
    }

    pub(super) async fn fs_write_file(
        &self,
        params: FsWriteFileParams,
    ) -> Result<FsWriteFileResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_write_file(params).await)
    }

    pub(super) async fn fs_create_directory(
        &self,
        params: FsCreateDirectoryParams,
    ) -> Result<FsCreateDirectoryResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_create_directory(params).await)
    }

    pub(super) async fn fs_get_metadata(
        &self,
        params: FsGetMetadataParams,
    ) -> Result<FsGetMetadataResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_get_metadata(params).await)
    }

    pub(super) async fn fs_read_directory(
        &self,
        params: FsReadDirectoryParams,
    ) -> Result<FsReadDirectoryResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_read_directory(params).await)
    }

    pub(super) async fn fs_remove(
        &self,
        params: FsRemoveParams,
    ) -> Result<FsRemoveResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_remove(params).await)
    }

    pub(super) async fn fs_copy(
        &self,
        params: FsCopyParams,
    ) -> Result<FsCopyResponse, ExecServerError> {
        server_result_to_client(self.handler.lock().await.fs_copy(params).await)
    }
}
