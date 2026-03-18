use std::path::PathBuf;

use codex_exec_server::ExecServerClient;
use codex_exec_server::RemoteExecServerConnectArgs;

use crate::config::Config;
use crate::exec::SandboxType;
use crate::sandboxing::ExecRequest;
use crate::unified_exec::RemoteExecServerFileSystem;
use crate::unified_exec::SpawnLifecycleHandle;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcess;

#[derive(Clone)]
pub(crate) struct RemoteExecServerBackend {
    client: ExecServerClient,
    local_workspace_root: PathBuf,
    remote_workspace_root: Option<PathBuf>,
}

impl std::fmt::Debug for RemoteExecServerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteExecServerBackend")
            .field("local_workspace_root", &self.local_workspace_root)
            .field("remote_workspace_root", &self.remote_workspace_root)
            .finish_non_exhaustive()
    }
}

impl RemoteExecServerBackend {
    pub(crate) async fn connect_for_config(
        config: &Config,
    ) -> Result<Option<Self>, UnifiedExecError> {
        let Some(websocket_url) = config
            .experimental_unified_exec_exec_server_websocket_url
            .clone()
        else {
            return Ok(None);
        };

        let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
            websocket_url,
            "codex-core".to_string(),
        ))
        .await
        .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;

        Ok(Some(Self {
            client,
            local_workspace_root: config.cwd.clone(),
            remote_workspace_root: config
                .experimental_unified_exec_exec_server_workspace_root
                .clone()
                .map(PathBuf::from),
        }))
    }

    pub(crate) async fn open_session(
        &self,
        process_id: i32,
        env: &ExecRequest,
        tty: bool,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<UnifiedExecProcess, UnifiedExecError> {
        if !spawn_lifecycle.inherited_fds().is_empty() {
            return Err(UnifiedExecError::create_process(
                "remote exec-server mode does not support inherited file descriptors".to_string(),
            ));
        }

        if env.sandbox != SandboxType::None {
            return Err(UnifiedExecError::create_process(format!(
                "remote exec-server mode does not support sandboxed execution yet: {:?}",
                env.sandbox
            )));
        }

        let remote_cwd = self.map_remote_cwd(env.cwd.as_path())?;
        UnifiedExecProcess::from_exec_server(
            self.client.clone(),
            process_id,
            env,
            remote_cwd,
            tty,
            spawn_lifecycle,
        )
        .await
    }

    pub(crate) fn file_system(&self) -> RemoteExecServerFileSystem {
        RemoteExecServerFileSystem::new(self.client.clone())
    }

    fn map_remote_cwd(&self, local_cwd: &std::path::Path) -> Result<PathBuf, UnifiedExecError> {
        let Some(remote_root) = self.remote_workspace_root.as_ref() else {
            if local_cwd == self.local_workspace_root.as_path() {
                return Ok(PathBuf::from("."));
            }
            return Err(UnifiedExecError::create_process(format!(
                "remote exec-server mode needs `experimental_unified_exec_exec_server_workspace_root` for non-root cwd `{}`",
                local_cwd.display()
            )));
        };

        let relative =
            UnifiedExecProcess::relative_cwd_under(local_cwd, &self.local_workspace_root)
                .ok_or_else(|| {
                    UnifiedExecError::create_process(format!(
                        "cwd `{}` is not under local workspace root `{}`",
                        local_cwd.display(),
                        self.local_workspace_root.display()
                    ))
                })?;
        Ok(remote_root.join(relative))
    }
}
