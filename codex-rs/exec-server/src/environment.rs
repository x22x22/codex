use std::sync::Arc;

use crate::ExecServerClient;
use crate::ExecServerError;
use crate::RemoteExecServerConnectArgs;
use crate::executor::Executor;
use crate::executor::LocalExecutor;
use crate::executor::RemoteExecutor;
use crate::fs;
use crate::fs::ExecutorFileSystem;

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    exec_server_client: Option<ExecServerClient>,
    executor: Arc<dyn Executor>,
    file_system: Arc<dyn ExecutorFileSystem>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field(
                "experimental_exec_server_url",
                &self.experimental_exec_server_url,
            )
            .field("has_exec_server_client", &self.exec_server_client.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("failed to initialize executor backend: {0}")]
    InitializeExecutor(String),

    #[error("failed to initialize filesystem backend: {0}")]
    InitializeFilesystem(String),
}

impl Environment {
    pub fn local() -> Self {
        Self {
            experimental_exec_server_url: None,
            exec_server_client: None,
            executor: Arc::new(LocalExecutor::new()),
            file_system: Arc::new(fs::LocalFileSystem),
        }
    }

    pub async fn create(
        experimental_exec_server_url: Option<String>,
    ) -> Result<Self, EnvironmentError> {
        let Some(websocket_url) = experimental_exec_server_url else {
            return Ok(Self::local());
        };

        let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
            websocket_url.clone(),
            "codex-core".to_string(),
        ))
        .await
        .map_err(|err| EnvironmentError::InitializeExecutor(err.to_string()))?;

        Ok(Self {
            experimental_exec_server_url: Some(websocket_url),
            exec_server_client: Some(client.clone()),
            executor: Arc::new(RemoteExecutor::new(client.clone())),
            file_system: Arc::new(fs::RemoteFileSystem::new(client)),
        })
    }

    pub fn from_exec_server_client(client: ExecServerClient) -> Self {
        Self {
            experimental_exec_server_url: None,
            exec_server_client: Some(client.clone()),
            executor: Arc::new(RemoteExecutor::new(client.clone())),
            file_system: Arc::new(fs::RemoteFileSystem::new(client)),
        }
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    pub fn exec_server_client(&self) -> Option<ExecServerClient> {
        self.exec_server_client.clone()
    }

    pub fn filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.file_system)
    }

    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        self.filesystem()
    }

    pub fn executor(&self) -> Arc<dyn Executor> {
        Arc::clone(&self.executor)
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::local()
    }
}

impl From<ExecServerError> for EnvironmentError {
    fn from(err: ExecServerError) -> Self {
        Self::InitializeExecutor(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Environment;
    use crate::ExecServerClient;
    use crate::ExecServerClientConnectOptions;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn create_without_remote_exec_server_url_does_not_connect() {
        let environment = Environment::create(None).await.expect("create environment");

        assert_eq!(environment.experimental_exec_server_url(), None);
        assert!(environment.exec_server_client().is_none());
    }

    #[tokio::test]
    async fn environment_uses_remote_filesystem_abstraction_when_client_is_provided() {
        let client =
            ExecServerClient::connect_in_process(ExecServerClientConnectOptions::default())
                .await
                .expect("connect in-process client");
        let environment = Environment::from_exec_server_client(client);
        let tempdir = TempDir::new().expect("tempdir");
        let path = AbsolutePathBuf::try_from(tempdir.path().join("marker.txt")).expect("path");

        environment
            .filesystem()
            .write_file(&path, b"hello".to_vec())
            .await
            .expect("write file through environment abstraction");

        let bytes = environment
            .filesystem()
            .read_file(&path)
            .await
            .expect("read file through environment abstraction");

        assert_eq!(bytes, b"hello");
    }
}
