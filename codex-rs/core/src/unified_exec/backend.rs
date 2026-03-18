use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use codex_environment::Environment;
use codex_exec_server::ExecServerClient;
use codex_exec_server::ExecServerClientConnectOptions;
use codex_exec_server::ExecServerLaunchCommand;
use codex_exec_server::RemoteExecServerConnectArgs;
use codex_exec_server::SpawnedExecServer;
use codex_exec_server::spawn_local_exec_server;
use tracing::debug;

use crate::config::Config;
use crate::exec::SandboxType;
use crate::exec_server_filesystem::ExecServerFileSystem;
use crate::exec_server_path_mapper::RemoteWorkspacePathMapper;
use crate::sandboxing::ExecRequest;
use crate::unified_exec::SpawnLifecycleHandle;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcess;

pub(crate) type UnifiedExecSessionFactoryHandle = Arc<dyn UnifiedExecSessionFactory>;

pub(crate) struct SessionExecutionBackends {
    pub(crate) unified_exec_session_factory: UnifiedExecSessionFactoryHandle,
    pub(crate) environment: Arc<Environment>,
}

#[async_trait]
pub(crate) trait UnifiedExecSessionFactory: std::fmt::Debug + Send + Sync {
    async fn open_session(
        &self,
        process_id: i32,
        env: &ExecRequest,
        tty: bool,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<UnifiedExecProcess, UnifiedExecError>;
}

#[derive(Debug, Default)]
pub(crate) struct LocalUnifiedExecSessionFactory;

pub(crate) fn local_unified_exec_session_factory() -> UnifiedExecSessionFactoryHandle {
    Arc::new(LocalUnifiedExecSessionFactory)
}

#[async_trait]
impl UnifiedExecSessionFactory for LocalUnifiedExecSessionFactory {
    async fn open_session(
        &self,
        _process_id: i32,
        env: &ExecRequest,
        tty: bool,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<UnifiedExecProcess, UnifiedExecError> {
        open_local_session(env, tty, spawn_lifecycle).await
    }
}

pub(crate) struct ExecServerUnifiedExecSessionFactory {
    client: ExecServerClient,
    _spawned_server: Option<Arc<SpawnedExecServer>>,
    path_mapper: Option<RemoteWorkspacePathMapper>,
}

impl std::fmt::Debug for ExecServerUnifiedExecSessionFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecServerUnifiedExecSessionFactory")
            .field("owns_spawned_server", &self._spawned_server.is_some())
            .finish_non_exhaustive()
    }
}

impl ExecServerUnifiedExecSessionFactory {
    pub(crate) fn from_client(
        client: ExecServerClient,
        path_mapper: Option<RemoteWorkspacePathMapper>,
    ) -> UnifiedExecSessionFactoryHandle {
        Arc::new(Self {
            client,
            _spawned_server: None,
            path_mapper,
        })
    }

    pub(crate) fn from_spawned_server(
        spawned_server: Arc<SpawnedExecServer>,
        path_mapper: Option<RemoteWorkspacePathMapper>,
    ) -> UnifiedExecSessionFactoryHandle {
        Arc::new(Self {
            client: spawned_server.client().clone(),
            _spawned_server: Some(spawned_server),
            path_mapper,
        })
    }
}

#[async_trait]
impl UnifiedExecSessionFactory for ExecServerUnifiedExecSessionFactory {
    async fn open_session(
        &self,
        process_id: i32,
        env: &ExecRequest,
        tty: bool,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<UnifiedExecProcess, UnifiedExecError> {
        let inherited_fds = spawn_lifecycle.inherited_fds();
        if !inherited_fds.is_empty() {
            debug!(
                process_id,
                inherited_fd_count = inherited_fds.len(),
                "falling back to local unified-exec backend because exec-server does not support inherited fds",
            );
            return open_local_session(env, tty, spawn_lifecycle).await;
        }

        if env.sandbox != SandboxType::None {
            debug!(
                process_id,
                sandbox = ?env.sandbox,
                "falling back to local unified-exec backend because sandboxed execution is not modeled by exec-server",
            );
            return open_local_session(env, tty, spawn_lifecycle).await;
        }

        UnifiedExecProcess::from_exec_server(
            self.client.clone(),
            process_id,
            env,
            tty,
            spawn_lifecycle,
            self.path_mapper.as_ref(),
        )
        .await
    }
}

pub(crate) async fn session_execution_backends_for_config(
    config: &Config,
    local_exec_server_command: Option<ExecServerLaunchCommand>,
) -> Result<SessionExecutionBackends, UnifiedExecError> {
    let path_mapper = config
        .experimental_unified_exec_exec_server_workspace_root
        .clone()
        .map(|remote_root| {
            RemoteWorkspacePathMapper::new(
                config
                    .cwd
                    .clone()
                    .try_into()
                    .expect("config cwd should be absolute"),
                remote_root,
            )
        });
    if !config.experimental_unified_exec_use_exec_server {
        return Ok(SessionExecutionBackends {
            unified_exec_session_factory: local_unified_exec_session_factory(),
            environment: Arc::new(Environment::default()),
        });
    }

    if let Some(websocket_url) = config
        .experimental_unified_exec_exec_server_websocket_url
        .clone()
    {
        let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
            websocket_url,
            "codex-core".to_string(),
        ))
        .await
        .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        return Ok(exec_server_backends_from_client(client, path_mapper));
    }

    if config.experimental_unified_exec_spawn_local_exec_server {
        let command = local_exec_server_command.unwrap_or_else(default_local_exec_server_command);
        let spawned_server =
            spawn_local_exec_server(command, ExecServerClientConnectOptions::default())
                .await
                .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        return Ok(exec_server_backends_from_spawned_server(
            Arc::new(spawned_server),
            path_mapper,
        ));
    }

    let client = ExecServerClient::connect_in_process(ExecServerClientConnectOptions::default())
        .await
        .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
    Ok(exec_server_backends_from_client(client, path_mapper))
}

pub async fn executor_environment_for_config(
    config: &Config,
    local_exec_server_command: Option<ExecServerLaunchCommand>,
) -> io::Result<Arc<Environment>> {
    session_execution_backends_for_config(config, local_exec_server_command)
        .await
        .map(|backends| backends.environment)
        .map_err(|err| io::Error::other(err.to_string()))
}

fn default_local_exec_server_command() -> ExecServerLaunchCommand {
    let binary_name = if cfg!(windows) {
        "codex-exec-server.exe"
    } else {
        "codex-exec-server"
    };
    let program = std::env::current_exe()
        .ok()
        .map(|current_exe| current_exe.with_file_name(binary_name))
        .filter(|candidate| candidate.exists())
        .unwrap_or_else(|| PathBuf::from(binary_name));
    ExecServerLaunchCommand {
        program,
        args: Vec::new(),
    }
}

fn exec_server_backends_from_client(
    client: ExecServerClient,
    path_mapper: Option<RemoteWorkspacePathMapper>,
) -> SessionExecutionBackends {
    SessionExecutionBackends {
        unified_exec_session_factory: ExecServerUnifiedExecSessionFactory::from_client(
            client.clone(),
            path_mapper.clone(),
        ),
        environment: Arc::new(Environment::new(Arc::new(ExecServerFileSystem::new(
            client,
            path_mapper,
        )))),
    }
}

fn exec_server_backends_from_spawned_server(
    spawned_server: Arc<SpawnedExecServer>,
    path_mapper: Option<RemoteWorkspacePathMapper>,
) -> SessionExecutionBackends {
    SessionExecutionBackends {
        unified_exec_session_factory: ExecServerUnifiedExecSessionFactory::from_spawned_server(
            Arc::clone(&spawned_server),
            path_mapper.clone(),
        ),
        environment: Arc::new(Environment::new(Arc::new(ExecServerFileSystem::new(
            spawned_server.client().clone(),
            path_mapper,
        )))),
    }
}

async fn open_local_session(
    env: &ExecRequest,
    tty: bool,
    mut spawn_lifecycle: SpawnLifecycleHandle,
) -> Result<UnifiedExecProcess, UnifiedExecError> {
    let (program, args) = env
        .command
        .split_first()
        .ok_or(UnifiedExecError::MissingCommandLine)?;
    let inherited_fds = spawn_lifecycle.inherited_fds();

    let spawn_result = if tty {
        codex_utils_pty::pty::spawn_process_with_inherited_fds(
            program,
            args,
            env.cwd.as_path(),
            &env.env,
            &env.arg0,
            codex_utils_pty::TerminalSize::default(),
            &inherited_fds,
        )
        .await
    } else {
        codex_utils_pty::pipe::spawn_process_no_stdin_with_inherited_fds(
            program,
            args,
            env.cwd.as_path(),
            &env.env,
            &env.arg0,
            &inherited_fds,
        )
        .await
    };
    let spawned = spawn_result.map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
    spawn_lifecycle.after_spawn();
    UnifiedExecProcess::from_spawned(spawned, env.sandbox, spawn_lifecycle).await
}
