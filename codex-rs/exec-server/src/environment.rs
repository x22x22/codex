use std::sync::Arc;

use crate::ExecServerClient;
use crate::ExecServerError;
use crate::RemoteExecServerConnectArgs;
use crate::fs;
use crate::fs::ExecutorFileSystem;
use crate::local_process::LocalProcess;
use crate::process::ExecProcess;
use crate::remote_process::RemoteProcess;

pub trait ExecutorEnvironment: Send + Sync {
    fn get_executor(&self) -> Arc<dyn ExecProcess>;
}

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    executor: Arc<dyn ExecProcess>,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            experimental_exec_server_url: None,
            executor: Arc::new(LocalProcess::default()),
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
                &self.experimental_exec_server_url.is_some(),
            )
            .finish()
    }
}

impl Environment {
    pub async fn create(
        experimental_exec_server_url: Option<String>,
    ) -> Result<Self, ExecServerError> {
        let executor: Arc<dyn ExecProcess> =
            if let Some(websocket_url) = experimental_exec_server_url.as_deref() {
                Arc::new(RemoteProcess::new(
                    ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
                        websocket_url.to_string(),
                        "codex-core".to_string(),
                    ))
                    .await?,
                ))
            } else {
                let process = LocalProcess::default();
                process
                    .initialize()
                    .map_err(|error| ExecServerError::Server {
                        code: error.code,
                        message: error.message,
                    })?;
                process.initialized().map_err(ExecServerError::Protocol)?;
                Arc::new(process)
            };

        Ok(Self {
            experimental_exec_server_url,
            executor,
        })
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    pub fn get_executor(&self) -> Arc<dyn ExecProcess> {
        Arc::clone(&self.executor)
    }

    pub fn get_filesystem(&self) -> impl ExecutorFileSystem + use<> {
        fs::LocalFileSystem
    }
}

impl ExecutorEnvironment for Environment {
    fn get_executor(&self) -> Arc<dyn ExecProcess> {
        self.get_executor()
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
    }
}
