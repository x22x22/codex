use crate::ExecServerClient;
use crate::ExecServerClientConnectOptions;
use crate::ExecServerError;
use crate::RemoteExecServerConnectArgs;
use crate::fs;
use crate::fs::ExecutorFileSystem;
use crate::local_process::LocalExecProcess;
use crate::process::ExecProcess;
use crate::remote_process::RemoteExecProcess;
use std::sync::Arc;

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    executor: Arc<dyn ExecProcess>,
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
                let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
                    websocket_url.to_string(),
                    "codex-core".to_string(),
                ))
                .await?;
                Arc::new(RemoteExecProcess::new(client))
            } else {
                Arc::new(LocalExecProcess::new())
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

impl crate::ExecutorEnvironment for Environment {
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
