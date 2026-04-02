use std::sync::Arc;

use tokio::sync::OnceCell;

use crate::ExecServerClient;
use crate::ExecServerError;
use crate::RemoteExecServerConnectArgs;
use crate::file_system::ExecutorFileSystem;
use crate::local_file_system::LocalFileSystem;
use crate::local_process::LocalProcess;
use crate::process::ExecBackend;
use crate::remote_file_system::RemoteFileSystem;
use crate::remote_process::RemoteProcess;

pub const CODEX_EXEC_SERVER_URL_ENV_VAR: &str = "CODEX_EXEC_SERVER_URL";

/// Exec backend access for a concrete executor attachment.
///
/// Implementations should return a backend bound to the same environment
/// instance so process execution and filesystem operations stay on one
/// transport.
pub trait ExecutorEnvironment: Send + Sync {
    fn get_exec_backend(&self) -> Arc<dyn ExecBackend>;
}

/// Executor and filesystem transports attached to one environment.
#[derive(Clone)]
pub struct ExecutorAttachment {
    exec_server_url: Option<String>,
    exec_backend: Arc<dyn ExecBackend>,
    file_system: Arc<dyn ExecutorFileSystem>,
}

impl std::fmt::Debug for ExecutorAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutorAttachment")
            .field("exec_server_url", &self.exec_server_url)
            .finish_non_exhaustive()
    }
}

impl ExecutorAttachment {
    fn new_local(exec_backend: Arc<dyn ExecBackend>) -> Self {
        Self {
            exec_server_url: None,
            exec_backend,
            file_system: Arc::new(LocalFileSystem),
        }
    }

    fn new_remote(exec_server_url: String, client: ExecServerClient) -> Self {
        Self {
            exec_server_url: Some(exec_server_url),
            exec_backend: Arc::new(RemoteProcess::new(client.clone())),
            file_system: Arc::new(RemoteFileSystem::new(client)),
        }
    }

    pub fn exec_server_url(&self) -> Option<&str> {
        self.exec_server_url.as_deref()
    }

    pub fn get_exec_backend(&self) -> Arc<dyn ExecBackend> {
        Arc::clone(&self.exec_backend)
    }

    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.file_system)
    }
}

impl ExecutorEnvironment for ExecutorAttachment {
    fn get_exec_backend(&self) -> Arc<dyn ExecBackend> {
        self.get_exec_backend()
    }
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
    Local,
    Remote {
        url: String,
    },
    None,
}

impl ExecutorMode {
    fn remote_exec_server_url(&self) -> Option<&str> {
        match self {
            Self::Remote { url } => Some(url.as_str()),
            Self::Local | Self::None => None,
        }
    }

    fn has_attached_executor(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Session/turn environment plus an optional executor attachment.
#[derive(Clone)]
pub struct Environment {
    executor_mode: ExecutorMode,
    executor_attachment: Option<Arc<ExecutorAttachment>>,
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
            executor_mode: ExecutorMode::Local,
            executor_attachment: Some(Arc::new(ExecutorAttachment::new_local(Arc::new(
                local_process,
            )))),
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
        let executor_attachment = if let Some(url) = executor_mode.remote_exec_server_url() {
            let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs {
                websocket_url: url.to_string(),
                client_name: "codex-environment".to_string(),
                connect_timeout: std::time::Duration::from_secs(5),
                initialize_timeout: std::time::Duration::from_secs(5),
            })
            .await?;
            Some(Arc::new(ExecutorAttachment::new_remote(
                url.to_string(),
                client,
            )))
        } else if matches!(executor_mode, ExecutorMode::None) {
            None
        } else {
            let local_process = LocalProcess::default();
            local_process
                .initialize()
                .map_err(|err| ExecServerError::Protocol(err.message))?;
            local_process
                .initialized()
                .map_err(ExecServerError::Protocol)?;
            Some(Arc::new(ExecutorAttachment::new_local(Arc::new(
                local_process,
            ))))
        };

        Ok(Self {
            executor_mode,
            executor_attachment,
        })
    }

    pub fn exec_server_url(&self) -> Option<&str> {
        self.executor_mode.remote_exec_server_url()
    }

    pub fn has_attached_executor(&self) -> bool {
        self.executor_mode.has_attached_executor()
    }

    pub fn executor_attachment(&self) -> Option<Arc<ExecutorAttachment>> {
        self.executor_attachment.clone()
    }
}

fn parse_executor_mode(exec_server_url: Option<String>) -> ExecutorMode {
    match exec_server_url.as_deref().map(str::trim) {
        None | Some("") => ExecutorMode::Local,
        Some(url) if url.eq_ignore_ascii_case("none") => ExecutorMode::None,
        Some(url) => ExecutorMode::Remote {
            url: url.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Environment;
    use super::EnvironmentManager;
    use super::ExecutorMode;
    use super::parse_executor_mode;
    use crate::ProcessId;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn create_without_remote_exec_server_url_does_not_connect() {
        let environment = Environment::create(/*exec_server_url*/ None)
            .await
            .expect("create environment");

        assert_eq!(environment.exec_server_url(), None);
        assert!(environment.has_attached_executor());
        assert_eq!(environment.executor_mode, ExecutorMode::Local);
        assert!(environment.executor_attachment().is_some());
    }

    #[test]
    fn environment_manager_normalizes_empty_url() {
        let manager = EnvironmentManager::new(Some(String::new()));

        assert_eq!(manager.executor_mode, ExecutorMode::Local);
    }

    #[test]
    fn environment_manager_preserves_no_executor_setting() {
        let manager = EnvironmentManager::new(Some("none".to_string()));

        assert_eq!(manager.executor_mode, ExecutorMode::None);
    }

    #[test]
    fn parse_executor_mode_preserves_no_executor_semantics() {
        assert_eq!(parse_executor_mode(None), ExecutorMode::Local);
        assert_eq!(
            parse_executor_mode(Some(String::new())),
            ExecutorMode::Local
        );
        assert_eq!(
            parse_executor_mode(Some("none".to_string())),
            ExecutorMode::None
        );
        assert_eq!(
            parse_executor_mode(Some("NONE".to_string())),
            ExecutorMode::None
        );
        assert_eq!(
            parse_executor_mode(Some("ws://localhost:1234".to_string())),
            ExecutorMode::Remote {
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
            .executor_attachment()
            .expect("default environment has executor attachment")
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
    async fn no_executor_environment_disables_executor_attachment() {
        let environment = Environment::create(Some("none".to_string()))
            .await
            .expect("create environment");

        assert_eq!(environment.exec_server_url(), None);
        assert!(!environment.has_attached_executor());
        assert_eq!(environment.executor_mode, ExecutorMode::None);
        assert!(environment.executor_attachment().is_none());
    }

    #[tokio::test]
    async fn no_executor_environment_has_no_executor_attachment() {
        let environment = Environment::create(Some("none".to_string()))
            .await
            .expect("create environment");

        assert!(environment.executor_attachment().is_none());
    }
}
