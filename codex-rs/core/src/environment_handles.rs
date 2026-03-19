use std::sync::Arc;

use crate::unified_exec::UnifiedExecProcessManager;
use codex_exec_server::Environment;
use codex_exec_server::Executor;
use codex_exec_server::ExecutorFileSystem;

/// Core-side facade for incremental migration to Environment-backed execution.
///
/// This keeps the existing unified-exec stack intact while giving new callers a
/// single place to access:
/// - the environment filesystem abstraction
/// - the environment direct executor abstraction
/// - the existing unified-exec manager
///
/// Existing callers can continue using `UnifiedExecProcessManager` directly.
/// New tools or skills can opt into either backend intentionally through this
/// facade without changing the legacy runtime path.
pub(crate) struct EnvironmentHandles<'a> {
    environment: &'a Environment,
    unified_exec_manager: &'a UnifiedExecProcessManager,
}

impl<'a> EnvironmentHandles<'a> {
    pub(crate) fn new(
        environment: &'a Environment,
        unified_exec_manager: &'a UnifiedExecProcessManager,
    ) -> Self {
        Self {
            environment,
            unified_exec_manager,
        }
    }

    pub(crate) fn filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        self.environment.filesystem()
    }

    pub(crate) fn direct_executor(&self) -> Arc<dyn Executor> {
        self.environment.executor()
    }

    pub(crate) fn unified_exec(&self) -> &'a UnifiedExecProcessManager {
        self.unified_exec_manager
    }

    pub(crate) fn environment(&self) -> &'a Environment {
        self.environment
    }
}
