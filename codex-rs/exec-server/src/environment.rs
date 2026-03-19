use std::sync::Arc;

use crate::executor::{Executor, LocalExecutor, RemoteExecutor};
use crate::fs;
use crate::fs::ExecutorFileSystem;
use crate::{ExecServerClient, ExecServerError, RemoteExecServerConnectArgs};

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    exec_server_client: Option<ExecServerClient>,
    file_system: Arc<dyn ExecutorFileSystem>,
    executor: Arc<dyn Executor>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field(
                "experimental_exec_server_url",
                &self.experimental_exec_server_url,
            )
            .field("has_exec_server_client", &self.exec_server_client.is_some())
            .finish()
    }
}

impl Environment {
    /// Create a purely local environment.
    pub fn local() -> Self {
        Self {
            experimental_exec_server_url: None,
            exec_server_client: None,
            file_system: Arc::new(fs::LocalFileSystem),
            executor: Arc::new(LocalExecutor::new()),
        }
    }

    pub async fn create(
        experimental_exec_server_url: Option<String>,
    ) -> Result<Self, ExecServerError> {
        let exec_server_client =
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

        let file_system: Arc<dyn ExecutorFileSystem> = if let Some(client) = &exec_server_client {
            Arc::new(fs::RemoteFileSystem::new(client.clone()))
        } else {
            Arc::new(fs::LocalFileSystem)
        };

        let executor: Arc<dyn Executor> = if let Some(client) = &exec_server_client {
            Arc::new(RemoteExecutor::from_client(Arc::new(client.clone())))
        } else {
            Arc::new(LocalExecutor::new())
        };

        Ok(Self {
            experimental_exec_server_url,
            exec_server_client,
            file_system,
            executor,
        })
    }

    pub fn from_exec_server_client(client: ExecServerClient) -> Self {
        let client = Arc::new(client);
        Self {
            experimental_exec_server_url: None,
            exec_server_client: Some((*client).clone()),
            file_system: Arc::new(fs::RemoteFileSystem::new((*client).clone())),
            executor: Arc::new(RemoteExecutor::from_client(Arc::clone(&client))),
        }
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    /// Preferred filesystem accessor for new callers.
    pub fn filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.file_system)
    }

    /// Compatibility accessor for existing callers.
    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        self.filesystem()
    }

    /// Compatibility accessor for existing core unified-exec wiring.
    pub fn exec_server_client(&self) -> Option<ExecServerClient> {
        self.exec_server_client.clone()
    }

    /// Preferred execution accessor for new callers.
    pub fn executor(&self) -> Arc<dyn Executor> {
        Arc::clone(&self.executor)
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::local()
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
