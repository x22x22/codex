use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::ExecServerClient;
use crate::ExecServerError;
use crate::RemoteExecServerConnectArgs;
use crate::fs;
use crate::fs::ExecutorFileSystem;
use crate::local_process::LocalProcess;
use crate::process::ExecProcess;
use crate::protocol::EnvironmentCapabilities;
use crate::protocol::EnvironmentCapabilitiesParams;
use crate::protocol::EnvironmentGetParams;
use crate::protocol::EnvironmentGetResponse;
use crate::protocol::EnvironmentInfo;
use crate::protocol::EnvironmentListResponse;
use crate::remote_process::RemoteProcess;

#[derive(Clone)]
struct EnvironmentFileSystem {
    filesystem: Arc<dyn ExecutorFileSystem>,
}

#[async_trait]
impl ExecutorFileSystem for EnvironmentFileSystem {
    async fn read_file(&self, path: &AbsolutePathBuf) -> crate::fs::FileSystemResult<Vec<u8>> {
        self.filesystem.read_file(path).await
    }

    async fn write_file(
        &self,
        path: &AbsolutePathBuf,
        contents: Vec<u8>,
    ) -> crate::fs::FileSystemResult<()> {
        self.filesystem.write_file(path, contents).await
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: crate::fs::CreateDirectoryOptions,
    ) -> crate::fs::FileSystemResult<()> {
        self.filesystem.create_directory(path, options).await
    }

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
    ) -> crate::fs::FileSystemResult<crate::fs::FileMetadata> {
        self.filesystem.get_metadata(path).await
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
    ) -> crate::fs::FileSystemResult<Vec<crate::fs::ReadDirectoryEntry>> {
        self.filesystem.read_directory(path).await
    }

    async fn remove(
        &self,
        path: &AbsolutePathBuf,
        options: crate::fs::RemoveOptions,
    ) -> crate::fs::FileSystemResult<()> {
        self.filesystem.remove(path, options).await
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: crate::fs::CopyOptions,
    ) -> crate::fs::FileSystemResult<()> {
        self.filesystem
            .copy(source_path, destination_path, options)
            .await
    }
}

#[derive(Clone)]
struct RemoteEnvironmentFileSystem {
    client: ExecServerClient,
}

#[async_trait]
impl ExecutorFileSystem for RemoteEnvironmentFileSystem {
    async fn read_file(&self, path: &AbsolutePathBuf) -> crate::fs::FileSystemResult<Vec<u8>> {
        let response = self
            .client
            .fs_read_file(codex_app_server_protocol::FsReadFileParams {
                environment_id: None,
                path: path.clone(),
            })
            .await
            .map_err(exec_server_error_to_io)?;
        base64::engine::general_purpose::STANDARD
            .decode(response.data_base64)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    }

    async fn write_file(
        &self,
        path: &AbsolutePathBuf,
        contents: Vec<u8>,
    ) -> crate::fs::FileSystemResult<()> {
        self.client
            .fs_write_file(codex_app_server_protocol::FsWriteFileParams {
                environment_id: None,
                path: path.clone(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(contents),
            })
            .await
            .map_err(exec_server_error_to_io)?;
        Ok(())
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: crate::fs::CreateDirectoryOptions,
    ) -> crate::fs::FileSystemResult<()> {
        self.client
            .fs_create_directory(codex_app_server_protocol::FsCreateDirectoryParams {
                environment_id: None,
                path: path.clone(),
                recursive: Some(options.recursive),
            })
            .await
            .map_err(exec_server_error_to_io)?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
    ) -> crate::fs::FileSystemResult<crate::fs::FileMetadata> {
        let response = self
            .client
            .fs_get_metadata(codex_app_server_protocol::FsGetMetadataParams {
                environment_id: None,
                path: path.clone(),
            })
            .await
            .map_err(exec_server_error_to_io)?;
        Ok(crate::fs::FileMetadata {
            is_directory: response.is_directory,
            is_file: response.is_file,
            created_at_ms: response.created_at_ms,
            modified_at_ms: response.modified_at_ms,
        })
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
    ) -> crate::fs::FileSystemResult<Vec<crate::fs::ReadDirectoryEntry>> {
        let response = self
            .client
            .fs_read_directory(codex_app_server_protocol::FsReadDirectoryParams {
                environment_id: None,
                path: path.clone(),
            })
            .await
            .map_err(exec_server_error_to_io)?;
        Ok(response
            .entries
            .into_iter()
            .map(|entry| crate::fs::ReadDirectoryEntry {
                file_name: entry.file_name,
                is_directory: entry.is_directory,
                is_file: entry.is_file,
            })
            .collect())
    }

    async fn remove(
        &self,
        path: &AbsolutePathBuf,
        options: crate::fs::RemoveOptions,
    ) -> crate::fs::FileSystemResult<()> {
        self.client
            .fs_remove(codex_app_server_protocol::FsRemoveParams {
                environment_id: None,
                path: path.clone(),
                recursive: Some(options.recursive),
                force: Some(options.force),
            })
            .await
            .map_err(exec_server_error_to_io)?;
        Ok(())
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: crate::fs::CopyOptions,
    ) -> crate::fs::FileSystemResult<()> {
        self.client
            .fs_copy(codex_app_server_protocol::FsCopyParams {
                environment_id: None,
                source_path: source_path.clone(),
                destination_path: destination_path.clone(),
                recursive: options.recursive,
            })
            .await
            .map_err(exec_server_error_to_io)?;
        Ok(())
    }
}

pub trait ExecutorEnvironment: Send + Sync {
    fn get_executor(&self) -> Arc<dyn ExecProcess>;
}

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    environment_id: String,
    capabilities: EnvironmentCapabilities,
    executor: Arc<dyn ExecProcess>,
    filesystem: Arc<dyn ExecutorFileSystem>,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            experimental_exec_server_url: None,
            environment_id: "local".to_string(),
            capabilities: EnvironmentCapabilities::default(),
            executor: Arc::new(LocalProcess::default()),
            filesystem: Arc::new(fs::LocalFileSystem),
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
            .field("environment_id", &self.environment_id)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

impl Environment {
    pub fn default_environment_id(experimental_exec_server_url: Option<&str>) -> String {
        experimental_exec_server_url.map_or_else(
            || "local".to_string(),
            |websocket_url| format!("remote:{websocket_url}"),
        )
    }

    pub async fn create(
        experimental_exec_server_url: Option<String>,
    ) -> Result<Self, ExecServerError> {
        let (environment_id, executor, filesystem, capabilities) =
            if let Some(websocket_url) = experimental_exec_server_url.as_deref() {
                let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
                    websocket_url.to_string(),
                    "codex-core".to_string(),
                ))
                .await?;
                let environment_id = Environment::default_environment_id(Some(websocket_url));
                let capabilities = client
                    .environment_capabilities(EnvironmentCapabilitiesParams {
                        environment_id: environment_id.clone(),
                    })
                    .await
                    .map(|response| response.capabilities)
                    .unwrap_or_else(|_| EnvironmentCapabilities::default());
                let executor = Arc::new(RemoteProcess::new(client.clone())) as Arc<dyn ExecProcess>;
                let filesystem =
                    Arc::new(RemoteEnvironmentFileSystem { client }) as Arc<dyn ExecutorFileSystem>;
                (environment_id, executor, filesystem, capabilities)
            } else {
                let process = LocalProcess::default();
                process
                    .initialize()
                    .map_err(|error| ExecServerError::Server {
                        code: error.code,
                        message: error.message,
                    })?;
                process.initialized().map_err(ExecServerError::Protocol)?;
                (
                    Environment::default_environment_id(None),
                    Arc::new(process) as Arc<dyn ExecProcess>,
                    Arc::new(fs::LocalFileSystem) as Arc<dyn ExecutorFileSystem>,
                    EnvironmentCapabilities::default(),
                )
            };

        Ok(Self {
            experimental_exec_server_url,
            environment_id,
            capabilities,
            executor,
            filesystem,
        })
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }

    pub fn capabilities(&self) -> &EnvironmentCapabilities {
        &self.capabilities
    }

    pub fn info(&self) -> EnvironmentInfo {
        EnvironmentInfo {
            environment_id: self.environment_id.clone(),
            experimental_exec_server_url: self.experimental_exec_server_url.clone(),
            capabilities: self.capabilities.clone(),
        }
    }

    pub fn list_response(&self) -> EnvironmentListResponse {
        EnvironmentListResponse {
            environments: vec![self.info()],
        }
    }

    pub fn get_response(
        &self,
        params: EnvironmentGetParams,
    ) -> Result<EnvironmentGetResponse, ExecServerError> {
        if params.environment_id != self.environment_id {
            return Err(ExecServerError::Protocol(format!(
                "unknown environment id `{}`",
                params.environment_id
            )));
        }
        Ok(EnvironmentGetResponse {
            environment: self.info(),
        })
    }

    pub fn get_executor(&self) -> Arc<dyn ExecProcess> {
        Arc::clone(&self.executor)
    }

    pub fn filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.filesystem)
    }

    pub fn get_filesystem(&self) -> impl ExecutorFileSystem + use<> {
        EnvironmentFileSystem {
            filesystem: Arc::clone(&self.filesystem),
        }
    }
}

fn exec_server_error_to_io(err: ExecServerError) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

impl ExecutorEnvironment for Environment {
    fn get_executor(&self) -> Arc<dyn ExecProcess> {
        Arc::clone(&self.executor)
    }
}

#[cfg(test)]
mod tests {
    use super::Environment;
    use super::EnvironmentCapabilities;
    use crate::fs::ExecutorFileSystem;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[tokio::test]
    async fn default_environment_uses_local_identity_and_capabilities() {
        let environment = Environment::default();

        assert_eq!(environment.experimental_exec_server_url(), None);
        assert_eq!(environment.environment_id(), "local");
        assert_eq!(
            environment.capabilities(),
            &EnvironmentCapabilities::default()
        );
    }

    #[tokio::test]
    async fn create_without_remote_exec_server_url_uses_local_identity_and_capabilities() {
        let environment = Environment::create(None).await.expect("create environment");

        assert_eq!(environment.experimental_exec_server_url(), None);
        assert_eq!(environment.environment_id(), "local");
        assert_eq!(
            environment.capabilities(),
            &EnvironmentCapabilities::default()
        );
    }

    #[tokio::test]
    async fn default_environment_filesystem_delegates_to_local_filesystem() {
        let temp_dir = tempdir().expect("temp dir");
        let file_path = AbsolutePathBuf::try_from(temp_dir.path().join("example.txt"))
            .expect("absolute file path");
        std::fs::write(file_path.as_path(), b"delegated filesystem").expect("write sample file");

        let bytes = Environment::default()
            .get_filesystem()
            .read_file(&file_path)
            .await
            .expect("read sample file");

        assert_eq!(bytes, b"delegated filesystem");
    }
}
