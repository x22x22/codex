use std::sync::Arc;

use async_trait::async_trait;

use crate::sandboxing::ExecRequest;
use crate::unified_exec::SpawnLifecycleHandle;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcess;

pub(crate) type UnifiedExecSessionFactoryHandle = Arc<dyn UnifiedExecSessionFactory>;

#[async_trait]
pub(crate) trait UnifiedExecSessionFactory: std::fmt::Debug + Send + Sync {
    async fn open_session(
        &self,
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
        env: &ExecRequest,
        tty: bool,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<UnifiedExecProcess, UnifiedExecError> {
        open_local_session(env, tty, spawn_lifecycle).await
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
