use std::sync::Arc;

use async_trait::async_trait;
use codex_exec_server::Environment;
use codex_exec_server::ExecSpawnRequest;
use codex_exec_server::InheritedFd;
use codex_exec_server::SandboxKind;
use codex_utils_absolute_path::AbsolutePathBuf;
use tracing::debug;

use crate::exec::SandboxType;
use crate::sandboxing::ExecRequest;
use crate::unified_exec::SpawnLifecycleHandle;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcess;

pub(crate) type UnifiedExecSessionFactoryHandle = Arc<dyn UnifiedExecSessionFactory>;

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

pub(crate) struct EnvironmentUnifiedExecSessionFactory {
    environment: Arc<Environment>,
}

impl std::fmt::Debug for EnvironmentUnifiedExecSessionFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentUnifiedExecSessionFactory")
            .finish_non_exhaustive()
    }
}

impl EnvironmentUnifiedExecSessionFactory {
    pub(crate) fn new(environment: Arc<Environment>) -> UnifiedExecSessionFactoryHandle {
        Arc::new(Self { environment })
    }
}

#[async_trait]
impl UnifiedExecSessionFactory for EnvironmentUnifiedExecSessionFactory {
    async fn open_session(
        &self,
        process_id: i32,
        env: &ExecRequest,
        tty: bool,
        mut spawn_lifecycle: SpawnLifecycleHandle,
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

        if env.sandbox == SandboxType::WindowsRestrictedToken {
            debug!(
                process_id,
                "falling back to local unified-exec backend because Windows restricted-token execution is not modeled by exec-server",
            );
            return open_local_session(env, tty, spawn_lifecycle).await;
        }

        let session = self
            .environment
            .executor()
            .spawn(ExecSpawnRequest {
                process_id: process_id.to_string(),
                argv: env.command.clone(),
                cwd: AbsolutePathBuf::try_from(env.cwd.clone())
                    .map_err(|err| UnifiedExecError::create_process(err.to_string()))?,
                env: env.env.clone(),
                arg0: env.arg0.clone(),
                tty,
                sandbox: sandbox_kind(env.sandbox),
                inherited_fds: inherited_fds
                    .into_iter()
                    .map(|target_fd| InheritedFd { target_fd })
                    .collect(),
            })
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        spawn_lifecycle.after_spawn();
        UnifiedExecProcess::from_exec_session(session, env.sandbox, tty, spawn_lifecycle).await
    }
}

pub(crate) fn unified_exec_session_factory_for_environment(
    environment: &Environment,
) -> UnifiedExecSessionFactoryHandle {
    EnvironmentUnifiedExecSessionFactory::new(Arc::new(environment.clone()))
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

fn sandbox_kind(sandbox: SandboxType) -> SandboxKind {
    match sandbox {
        SandboxType::None => SandboxKind::None,
        SandboxType::MacosSeatbelt => SandboxKind::MacosSeatbelt,
        SandboxType::LinuxSeccomp => SandboxKind::LinuxSeccomp,
        SandboxType::WindowsRestrictedToken => SandboxKind::WindowsRestrictedToken,
    }
}
