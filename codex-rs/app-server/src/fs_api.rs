use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use codex_app_server_protocol::FsCopyParams;
use codex_app_server_protocol::FsCopyResponse;
use codex_app_server_protocol::FsCreateDirectoryParams;
use codex_app_server_protocol::FsCreateDirectoryResponse;
use codex_app_server_protocol::FsGetMetadataParams;
use codex_app_server_protocol::FsGetMetadataResponse;
use codex_app_server_protocol::FsReadDirectoryEntry;
use codex_app_server_protocol::FsReadDirectoryParams;
use codex_app_server_protocol::FsReadDirectoryResponse;
use codex_app_server_protocol::FsReadFileParams;
use codex_app_server_protocol::FsReadFileResponse;
use codex_app_server_protocol::FsRemoveParams;
use codex_app_server_protocol::FsRemoveResponse;
use codex_app_server_protocol::FsWriteFileParams;
use codex_app_server_protocol::FsWriteFileResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_exec_server::CopyOptions;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::Environment;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::RemoveOptions;
use std::io;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct FsApi {
    environment_id: String,
    experimental_exec_server_url: Option<String>,
}

impl Default for FsApi {
    fn default() -> Self {
        Self::new(Environment::default_environment_id(None), None)
    }
}

impl FsApi {
    pub(crate) fn new(
        environment_id: String,
        experimental_exec_server_url: Option<String>,
    ) -> Self {
        Self {
            environment_id,
            experimental_exec_server_url,
        }
    }

    async fn file_system(&self) -> Result<Arc<dyn ExecutorFileSystem>, JSONRPCErrorError> {
        let environment = Environment::create(self.experimental_exec_server_url.clone())
            .await
            .map_err(|err| JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: format!("failed to bind environment for fs RPC: {err}"),
                data: None,
            })?;
        Ok(environment.filesystem())
    }
}

impl FsApi {
    fn validate_environment_id(
        &self,
        environment_id: Option<&str>,
        method: &str,
    ) -> Result<(), JSONRPCErrorError> {
        if let Some(environment_id) = environment_id
            && environment_id != self.environment_id
        {
            return Err(invalid_request(format!(
                "{method} does not support environmentId `{environment_id}`; configured environment is `{}`",
                self.environment_id
            )));
        }
        Ok(())
    }

    pub(crate) async fn read_file(
        &self,
        params: FsReadFileParams,
    ) -> Result<FsReadFileResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/readFile")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            path = ?params.path,
            "fs/readFile"
        );
        let file_system = self.file_system().await?;
        let bytes = file_system
            .read_file(&params.path)
            .await
            .map_err(map_fs_error)?;
        Ok(FsReadFileResponse {
            data_base64: STANDARD.encode(bytes),
        })
    }

    pub(crate) async fn write_file(
        &self,
        params: FsWriteFileParams,
    ) -> Result<FsWriteFileResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/writeFile")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            path = ?params.path,
            "fs/writeFile"
        );
        let bytes = STANDARD.decode(params.data_base64).map_err(|err| {
            invalid_request(format!(
                "fs/writeFile requires valid base64 dataBase64: {err}"
            ))
        })?;
        let file_system = self.file_system().await?;
        file_system
            .write_file(&params.path, bytes)
            .await
            .map_err(map_fs_error)?;
        Ok(FsWriteFileResponse {})
    }

    pub(crate) async fn create_directory(
        &self,
        params: FsCreateDirectoryParams,
    ) -> Result<FsCreateDirectoryResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/createDirectory")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            path = ?params.path,
            "fs/createDirectory"
        );
        let file_system = self.file_system().await?;
        file_system
            .create_directory(
                &params.path,
                CreateDirectoryOptions {
                    recursive: params.recursive.unwrap_or(true),
                },
            )
            .await
            .map_err(map_fs_error)?;
        Ok(FsCreateDirectoryResponse {})
    }

    pub(crate) async fn get_metadata(
        &self,
        params: FsGetMetadataParams,
    ) -> Result<FsGetMetadataResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/getMetadata")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            path = ?params.path,
            "fs/getMetadata"
        );
        let file_system = self.file_system().await?;
        let metadata = file_system
            .get_metadata(&params.path)
            .await
            .map_err(map_fs_error)?;
        Ok(FsGetMetadataResponse {
            is_directory: metadata.is_directory,
            is_file: metadata.is_file,
            created_at_ms: metadata.created_at_ms,
            modified_at_ms: metadata.modified_at_ms,
        })
    }

    pub(crate) async fn read_directory(
        &self,
        params: FsReadDirectoryParams,
    ) -> Result<FsReadDirectoryResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/readDirectory")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            path = ?params.path,
            "fs/readDirectory"
        );
        let file_system = self.file_system().await?;
        let entries = file_system
            .read_directory(&params.path)
            .await
            .map_err(map_fs_error)?;
        Ok(FsReadDirectoryResponse {
            entries: entries
                .into_iter()
                .map(|entry| FsReadDirectoryEntry {
                    file_name: entry.file_name,
                    is_directory: entry.is_directory,
                    is_file: entry.is_file,
                })
                .collect(),
        })
    }

    pub(crate) async fn remove(
        &self,
        params: FsRemoveParams,
    ) -> Result<FsRemoveResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/remove")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            path = ?params.path,
            "fs/remove"
        );
        let file_system = self.file_system().await?;
        file_system
            .remove(
                &params.path,
                RemoveOptions {
                    recursive: params.recursive.unwrap_or(true),
                    force: params.force.unwrap_or(true),
                },
            )
            .await
            .map_err(map_fs_error)?;
        Ok(FsRemoveResponse {})
    }

    pub(crate) async fn copy(
        &self,
        params: FsCopyParams,
    ) -> Result<FsCopyResponse, JSONRPCErrorError> {
        self.validate_environment_id(params.environment_id.as_deref(), "fs/copy")?;
        tracing::debug!(
            environment_id = %self.environment_id,
            source_path = ?params.source_path,
            destination_path = ?params.destination_path,
            "fs/copy"
        );
        let file_system = self.file_system().await?;
        file_system
            .copy(
                &params.source_path,
                &params.destination_path,
                CopyOptions {
                    recursive: params.recursive,
                },
            )
            .await
            .map_err(map_fs_error)?;
        Ok(FsCopyResponse {})
    }
}

fn invalid_request(message: impl Into<String>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_REQUEST_ERROR_CODE,
        message: message.into(),
        data: None,
    }
}

fn map_fs_error(err: io::Error) -> JSONRPCErrorError {
    if err.kind() == io::ErrorKind::InvalidInput {
        invalid_request(err.to_string())
    } else {
        JSONRPCErrorError {
            code: INTERNAL_ERROR_CODE,
            message: err.to_string(),
            data: None,
        }
    }
}
