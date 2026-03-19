use crate::ExecServerClient;
use crate::ExecServerClientConnectOptions;
use crate::ExecServerError;
use crate::ExecServerEvent;
use crate::ExecProcess;
use crate::ExecutorEnvironment;
use crate::RemoteExecServerConnectArgs;
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteResponse;
use crate::fs;
use crate::fs::ExecutorFileSystem;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::OnceCell;

#[derive(Clone)]
struct LocalExecProcess {
    client: Arc<OnceCell<ExecServerClient>>,
}

impl Default for LocalExecProcess {
    fn default() -> Self {
        Self {
            client: Arc::new(OnceCell::new()),
        }
    }
}

#[derive(Clone)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    remote_exec_server_client: Option<ExecServerClient>,
    local_exec_process: LocalExecProcess,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            experimental_exec_server_url: None,
            remote_exec_server_client: None,
            local_exec_process: LocalExecProcess::default(),
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
                &self.remote_exec_server_client.is_some(),
            )
            .finish()
    }
}

impl Environment {
    pub async fn create(
        experimental_exec_server_url: Option<String>,
    ) -> Result<Self, ExecServerError> {
        let remote_exec_server_client =
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

        Ok(Self {
            experimental_exec_server_url,
            remote_exec_server_client,
            local_exec_process: LocalExecProcess::default(),
        })
    }

    pub fn experimental_exec_server_url(&self) -> Option<&str> {
        self.experimental_exec_server_url.as_deref()
    }

    pub fn remote_exec_server_client(&self) -> Option<&ExecServerClient> {
        self.remote_exec_server_client.as_ref()
    }

    pub fn process(&self) -> &(dyn ExecProcess + '_) {
        self.remote_exec_server_client
            .as_ref()
            .map_or(&self.local_exec_process as &dyn ExecProcess, |client| client)
    }

    pub fn get_filesystem(&self) -> impl ExecutorFileSystem + use<> {
        fs::LocalFileSystem
    }
}

impl ExecutorEnvironment for Environment {
    fn process(&self) -> &(dyn ExecProcess + '_) {
        self.process()
    }
}

#[async_trait]
impl ExecProcess for LocalExecProcess {
    async fn start(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        self.client().await?.start(params).await
    }

    async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        self.client().await?.read(params).await
    }

    async fn write(
        &self,
        process_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        self.client().await?.write(process_id, chunk).await
    }

    async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError> {
        self.client().await?.terminate(process_id).await
    }

    fn subscribe_events(&self) -> broadcast::Receiver<ExecServerEvent> {
        if let Some(client) = self.client.get() {
            client.event_receiver()
        } else {
            let (_tx, rx) = broadcast::channel(1);
            rx
        }
    }
}

impl LocalExecProcess {
    async fn client(&self) -> Result<&ExecServerClient, ExecServerError> {
        self.client
            .get_or_try_init(|| async {
                ExecServerClient::connect_in_process(ExecServerClientConnectOptions::default()).await
            })
            .await;
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
        assert!(environment.remote_exec_server_client().is_none());
    }
}
