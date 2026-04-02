use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use crate::ExecServerClient;
use crate::ExecServerError;
use crate::RemoteExecServerConnectArgs;
use crate::file_system::ExecutorFileSystem;
use crate::local_file_system::LocalFileSystem;
use crate::local_process::LocalProcess;
use crate::process::ExecBackend;
use crate::process::StartedExecProcess;
use crate::protocol::ExecParams;
use crate::remote_file_system::RemoteFileSystem;
use crate::remote_process::RemoteProcess;

pub const CODEX_EXEC_SERVER_URL_ENV_VAR: &str = "CODEX_EXEC_SERVER_URL";

pub trait ExecutorEnvironment: Send + Sync {
    fn get_exec_backend(&self) -> Arc<dyn ExecBackend>;
}

#[derive(Debug, Default)]
pub struct EnvironmentManager {
    executor_mode: ExecutorMode,
    current_environment: OnceCell<Arc<Environment>>,
}

impl EnvironmentManager {
    pub fn new(exec_server_url: Option<String>) -> Self {
        Self {
            executor_mode: parse_executor_mode(exec_server_url),
            current_environment: OnceCell::new(),
        }
    }

    pub fn from_environment(environment: &Environment) -> Self {
        Self {
            executor_mode: environment.executor_mode.clone(),
            current_environment: OnceCell::new(),
        }
    }

    pub fn from_env() -> Self {
        Self::new(std::env::var(CODEX_EXEC_SERVER_URL_ENV_VAR).ok())
    }

    pub fn exec_server_url(&self) -> Option<&str> {
        self.executor_mode.remote_exec_server_url()
    }

    pub async fn current(&self) -> Result<Arc<Environment>, ExecServerError> {
        self.current_environment
            .get_or_try_init(|| async {
                Ok(Arc::new(
                    Environment::create_with_mode(self.executor_mode.clone()).await?,
                ))
            })
            .await
            .map(Arc::clone)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum ExecutorMode {
    #[default]
    LocalExecutor,
    RemoteExecutor {
        url: String,
    },
    NoExecutor,
}

impl ExecutorMode {
    fn remote_exec_server_url(&self) -> Option<&str> {
        match self {
            Self::RemoteExecutor { url } => Some(url.as_str()),
            Self::LocalExecutor | Self::NoExecutor => None,
        }
    }

    fn has_attached_executor(&self) -> bool {
        !matches!(self, Self::NoExecutor)
    }
}

#[derive(Clone)]
pub struct Environment {
    executor_mode: ExecutorMode,
    remote_exec_server_client: Option<ExecServerClient>,
    exec_backend: Arc<dyn ExecBackend>,
}

impl Default for Environment {
    fn default() -> Self {
        let local_process = LocalProcess::default();
        if let Err(err) = local_process.initialize() {
            panic!("default local process initialization should succeed: {err:?}");
        }
        if let Err(err) = local_process.initialized() {
            panic!("default local process should accept initialized notification: {err}");
        }

        Self {
            executor_mode: ExecutorMode::LocalExecutor,
            remote_exec_server_client: None,
            exec_backend: Arc::new(local_process),
        }
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("executor_mode", &self.executor_mode)
            .finish_non_exhaustive()
    }
}

impl Environment {
    pub async fn create(exec_server_url: Option<String>) -> Result<Self, ExecServerError> {
        Self::create_with_mode(parse_executor_mode(exec_server_url)).await
    }

    async fn create_with_mode(executor_mode: ExecutorMode) -> Result<Self, ExecServerError> {
        let remote_exec_server_client = if let Some(url) = executor_mode.remote_exec_server_url() {
            Some(
                ExecServerClient::connect_websocket(RemoteExecServerConnectArgs {
                    websocket_url: url.to_string(),
                    client_name: "codex-environment".to_string(),
                    connect_timeout: std::time::Duration::from_secs(5),
                    initialize_timeout: std::time::Duration::from_secs(5),
                })
                .await?,
            )
        } else {
            None
        };

        let exec_backend: Arc<dyn ExecBackend> =
            if let Some(client) = remote_exec_server_client.clone() {
                Arc::new(RemoteProcess::new(client))
            } else if matches!(executor_mode, ExecutorMode::NoExecutor) {
                Arc::new(NoAttachedExecutorBackend)
            } else {
                let local_process = LocalProcess::default();
                local_process
                    .initialize()
                    .map_err(|err| ExecServerError::Protocol(err.message))?;
                local_process
                    .initialized()
                    .map_err(ExecServerError::Protocol)?;
                Arc::new(local_process)
            };

        Ok(Self {
            executor_mode,
            remote_exec_server_client,
            exec_backend,
        })
    }

    pub fn exec_server_url(&self) -> Option<&str> {
        self.executor_mode.remote_exec_server_url()
    }

    pub fn has_attached_executor(&self) -> bool {
        self.executor_mode.has_attached_executor()
    }

    pub fn get_exec_backend(&self) -> Arc<dyn ExecBackend> {
        Arc::clone(&self.exec_backend)
    }

    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        if let Some(client) = self.remote_exec_server_client.clone() {
            Arc::new(RemoteFileSystem::new(client))
        } else {
            Arc::new(LocalFileSystem)
        }
    }
}

#[derive(Clone, Default)]
struct NoAttachedExecutorBackend;

#[async_trait]
impl ExecBackend for NoAttachedExecutorBackend {
    async fn start(&self, _params: ExecParams) -> Result<StartedExecProcess, ExecServerError> {
        Err(ExecServerError::Protocol(
            "no attached executor is configured for this session".to_string(),
        ))
    }
}

fn parse_executor_mode(exec_server_url: Option<String>) -> ExecutorMode {
    match exec_server_url.as_deref().map(str::trim) {
        None | Some("") => ExecutorMode::LocalExecutor,
        Some(url) if url.eq_ignore_ascii_case("none") => ExecutorMode::NoExecutor,
        Some(url) => ExecutorMode::RemoteExecutor {
            url: url.to_string(),
        },
    }
}

impl ExecutorEnvironment for Environment {
    fn get_exec_backend(&self) -> Arc<dyn ExecBackend> {
        Arc::clone(&self.exec_backend)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Environment;
    use super::EnvironmentManager;
    use crate::ProcessId;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn create_without_remote_exec_server_url_does_not_connect() {
        let environment = Environment::create(/*exec_server_url*/ None)
            .await
            .expect("create environment");

        assert_eq!(environment.exec_server_url(), None);
        assert!(environment.has_attached_executor());
        assert_eq!(environment.executor_mode, ExecutorMode::LocalExecutor);
        assert!(environment.remote_exec_server_client.is_none());
    }

    #[test]
    fn environment_manager_normalizes_empty_url() {
        let manager = EnvironmentManager::new(Some(String::new()));

        assert_eq!(manager.executor_mode, ExecutorMode::LocalExecutor);
    }

    #[test]
    fn environment_manager_preserves_no_executor_setting() {
        let manager = EnvironmentManager::new(Some("none".to_string()));

        assert_eq!(manager.executor_mode, ExecutorMode::NoExecutor);
    }

    #[test]
    fn parse_executor_mode_preserves_no_executor_semantics() {
        assert_eq!(parse_executor_mode(None), ExecutorMode::LocalExecutor);
        assert_eq!(
            parse_executor_mode(Some(String::new())),
            ExecutorMode::LocalExecutor
        );
        assert_eq!(
            parse_executor_mode(Some("none".to_string())),
            ExecutorMode::NoExecutor
        );
        assert_eq!(
            parse_executor_mode(Some("NONE".to_string())),
            ExecutorMode::NoExecutor
        );
        assert_eq!(
            parse_executor_mode(Some("ws://localhost:1234".to_string())),
            ExecutorMode::RemoteExecutor {
                url: "ws://localhost:1234".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn environment_manager_current_caches_environment() {
        let manager = EnvironmentManager::new(/*exec_server_url*/ None);

        let first = manager.current().await.expect("get current environment");
        let second = manager.current().await.expect("get current environment");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn default_environment_has_ready_local_executor() {
        let environment = Environment::default();

        let response = environment
            .get_exec_backend()
            .start(crate::ExecParams {
                process_id: ProcessId::from("default-env-proc"),
                argv: vec!["true".to_string()],
                cwd: std::env::current_dir().expect("read current dir"),
                env: Default::default(),
                tty: false,
                arg0: None,
            })
            .await
            .expect("start process");

        assert_eq!(response.process.process_id().as_str(), "default-env-proc");
    }

    #[tokio::test]
    async fn no_executor_environment_disables_attached_executor() {
        let environment = Environment::create(Some("none".to_string()))
            .await
            .expect("create environment");

        assert_eq!(environment.exec_server_url(), None);
        assert!(!environment.has_attached_executor());
        assert_eq!(environment.executor_mode, ExecutorMode::NoExecutor);
        assert!(environment.remote_exec_server_client.is_none());
    }

    #[tokio::test]
    async fn no_executor_environment_rejects_exec_start() {
        let environment = Environment::create(Some("none".to_string()))
            .await
            .expect("create environment");

        let err = environment
            .get_exec_backend()
            .start(crate::ExecParams {
                process_id: ProcessId::from("no-executor-proc"),
                argv: vec!["true".to_string()],
                cwd: std::env::current_dir().expect("read current dir"),
                env: Default::default(),
                tty: false,
                arg0: None,
            })
            .await
            .expect_err("no-executor backend should reject starts");

        assert_eq!(
            err.to_string(),
            "exec-server protocol error: no attached executor is configured for this session"
        );
    }
}
