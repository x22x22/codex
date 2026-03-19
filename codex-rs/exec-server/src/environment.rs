use crate::ExecServerClient;
use crate::ExecServerClientConnectOptions;
use crate::ExecServerError;
use crate::ExecProcess;
use crate::ExecutorEnvironment;
use crate::RemoteExecServerConnectArgs;
use crate::fs;
use crate::fs::ExecutorFileSystem;

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    exec_server_client: ExecServerClient,
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
        let exec_server_client = if let Some(websocket_url) = experimental_exec_server_url.as_deref()
        {
            ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
                websocket_url.to_string(),
                "codex-core".to_string(),
            ))
            .await?
        } else {
            ExecServerClient::connect_in_process(ExecServerClientConnectOptions::default()).await?
        };

        Ok(Self {
            experimental_exec_server_url,
            exec_server_client,
        })
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    pub fn get_executor(&self) -> &(dyn ExecProcess + '_) {
        &self.exec_server_client
    }

    pub fn get_filesystem(&self) -> impl ExecutorFileSystem + use<> {
        fs::LocalFileSystem
    }
}

impl ExecutorEnvironment for Environment {
    fn get_executor(&self) -> &(dyn ExecProcess + '_) {
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
