use crate::ExecServerClient;
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
use tokio::sync::broadcast;

#[derive(Debug)]
struct UnavailableExecProcess;

static UNAVAILABLE_EXEC_PROCESS: UnavailableExecProcess = UnavailableExecProcess;

#[derive(Clone, Default)]
pub struct Environment {
    experimental_exec_server_url: Option<String>,
    remote_exec_server_client: Option<ExecServerClient>,
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
            .map_or(&UNAVAILABLE_EXEC_PROCESS as &dyn ExecProcess, |client| client)
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
impl ExecProcess for UnavailableExecProcess {
    async fn start(&self, _params: ExecParams) -> Result<ExecResponse, ExecServerError> {
        Err(unavailable_exec_process_error())
    }

    async fn read(&self, _params: ReadParams) -> Result<ReadResponse, ExecServerError> {
        Err(unavailable_exec_process_error())
    }

    async fn write(
        &self,
        _process_id: &str,
        _chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError> {
        Err(unavailable_exec_process_error())
    }

    async fn terminate(&self, _process_id: &str) -> Result<TerminateResponse, ExecServerError> {
        Err(unavailable_exec_process_error())
    }

    fn subscribe_events(&self) -> broadcast::Receiver<ExecServerEvent> {
        let (_tx, rx) = broadcast::channel(1);
        rx
    }
}

fn unavailable_exec_process_error() -> ExecServerError {
    ExecServerError::Protocol(
        "exec process capability is unavailable for a local default Environment".to_string(),
    )
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
