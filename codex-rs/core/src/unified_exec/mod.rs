//! Unified Exec: interactive process execution orchestrated with approvals + sandboxing.
//!
//! Responsibilities
//! - Manages interactive processes (create, reuse, buffer output with caps).
//! - Uses the shared ToolOrchestrator to handle approval, sandbox selection, and
//!   retry semantics in a single, descriptive flow.
//! - Spawns the PTY from a sandbox-transformed `ExecRequest`; on sandbox denial,
//!   retries without sandbox when policy allows (no re‑prompt thanks to caching).
//! - Uses the shared `is_likely_sandbox_denied` heuristic to keep denial messages
//!   consistent with other exec paths.
//!
//! Flow at a glance (open process)
//! 1) Build a small request `{ command, cwd }`.
//! 2) Orchestrator: approval (bypass/cache/prompt) → select sandbox → run.
//! 3) Runtime: transform `CommandSpec` -> `ExecRequest` -> spawn PTY.
//! 4) If denial, orchestrator retries with `SandboxType::None`.
//! 5) Process handle is returned with streaming output + metadata.
//!
//! This keeps policy logic and user interaction centralized while the PTY/process
//! concerns remain isolated here. The implementation is split between:
//! - `process.rs`: PTY process lifecycle + output buffering.
//! - `process_manager.rs`: orchestration (approvals, sandboxing, reuse) and request handling.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::Weak;
use std::sync::atomic::Ordering;

use codex_exec_server::process::ExecProcess;
use codex_network_proxy::NetworkProxy;
use codex_protocol::models::PermissionProfile;
use rand::Rng;
use rand::rng;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::sandboxing::SandboxPermissions;
use crate::exec::is_likely_sandbox_denied;
use crate::exec::ExecToolCallOutput;
use crate::exec::SandboxType;
use crate::exec::StreamOutput;
use crate::truncate::TruncationPolicy;
use crate::truncate::formatted_truncate_text;

mod async_watcher;
mod errors;
mod head_tail_buffer;
mod process;
mod process_manager;

pub(crate) fn set_deterministic_process_ids_for_tests(enabled: bool) {
    process_manager::set_deterministic_process_ids_for_tests(enabled);
}

pub(crate) use errors::UnifiedExecError;
pub(crate) use process::NoopSpawnLifecycle;
#[cfg(unix)]
pub(crate) use process::SpawnLifecycle;
pub(crate) use process::SpawnLifecycleHandle;
pub(crate) use process::UnifiedExecProcess;

pub(crate) const MIN_YIELD_TIME_MS: u64 = 250;
// Minimum yield time for an empty `write_stdin`.
pub(crate) const MIN_EMPTY_YIELD_TIME_MS: u64 = 5_000;
pub(crate) const MAX_YIELD_TIME_MS: u64 = 30_000;
pub(crate) const DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: usize = 10_000;
pub(crate) const UNIFIED_EXEC_OUTPUT_MAX_BYTES: usize = 1024 * 1024; // 1 MiB
pub(crate) const UNIFIED_EXEC_OUTPUT_MAX_TOKENS: usize = UNIFIED_EXEC_OUTPUT_MAX_BYTES / 4;
pub(crate) const MAX_UNIFIED_EXEC_PROCESSES: usize = 64;

// Send a warning message to the models when it reaches this number of processes.
pub(crate) const WARNING_UNIFIED_EXEC_PROCESSES: usize = 60;

pub(crate) struct UnifiedExecContext {
    pub session: Arc<Session>,
    pub turn: Arc<TurnContext>,
    pub call_id: String,
}

impl UnifiedExecContext {
    pub fn new(session: Arc<Session>, turn: Arc<TurnContext>, call_id: String) -> Self {
        Self {
            session,
            turn,
            call_id,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ExecCommandRequest {
    pub command: Vec<String>,
    pub process_id: i32,
    pub yield_time_ms: u64,
    pub max_output_tokens: Option<usize>,
    pub workdir: Option<PathBuf>,
    pub network: Option<NetworkProxy>,
    pub tty: bool,
    pub sandbox_permissions: SandboxPermissions,
    pub additional_permissions: Option<PermissionProfile>,
    pub additional_permissions_preapproved: bool,
    pub justification: Option<String>,
    pub prefix_rule: Option<Vec<String>>,
}

#[derive(Debug)]
pub(crate) struct WriteStdinRequest<'a> {
    pub process_id: i32,
    pub input: &'a str,
    pub yield_time_ms: u64,
    pub max_output_tokens: Option<usize>,
}

#[derive(Default)]
pub(crate) struct ProcessStore {
    processes: HashMap<i32, ProcessEntry>,
    reserved_process_ids: HashSet<i32>,
}

impl ProcessStore {
    fn remove(&mut self, process_id: i32) -> Option<ProcessEntry> {
        self.reserved_process_ids.remove(&process_id);
        self.processes.remove(&process_id)
    }
}

pub(crate) struct UnifiedExecProcessManager {
    process_store: Mutex<ProcessStore>,
    max_write_stdin_yield_time_ms: u64,
}

impl UnifiedExecProcessManager {
    pub(crate) fn new(max_write_stdin_yield_time_ms: u64) -> Self {
        Self {
            process_store: Mutex::new(ProcessStore::default()),
            max_write_stdin_yield_time_ms: max_write_stdin_yield_time_ms
                .max(MIN_EMPTY_YIELD_TIME_MS),
        }
    }
}

impl Default for UnifiedExecProcessManager {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS)
    }
}

struct ProcessEntry {
    backend: ProcessBackend,
    call_id: String,
    process_id: i32,
    command: Vec<String>,
    tty: bool,
    network_approval_id: Option<String>,
    session: Weak<Session>,
    last_used: tokio::time::Instant,
}

#[derive(Debug, Clone)]
pub(crate) enum ProcessBackend {
    Local {
        process: Arc<UnifiedExecProcess>,
    },
    ExecServer {
        process_id: String,
        executor: Arc<dyn ExecProcess>,
        output_buffer: crate::unified_exec::process::OutputBuffer,
        output_notify: Arc<Notify>,
        output_closed: Arc<AtomicBool>,
        output_closed_notify: Arc<Notify>,
        output_drained: Arc<Notify>,
        cancellation_token: CancellationToken,
        exit_code: Arc<RwLock<Option<i32>>>,
        has_exited: Arc<AtomicBool>,
        sandbox_type: SandboxType,
        output_seq: Arc<AtomicU64>,
    },
}

impl ProcessBackend {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    pub(crate) fn as_local_process(&self) -> Option<&Arc<UnifiedExecProcess>> {
        match self {
            Self::Local { process } => Some(process),
            Self::ExecServer { .. } => None,
        }
    }

    pub(crate) fn output_handles(
        &self,
    ) -> (
        crate::unified_exec::process::OutputBuffer,
        Arc<Notify>,
        Arc<AtomicBool>,
        Arc<Notify>,
        CancellationToken,
    ) {
        match self {
            Self::Local { process } => {
                let handles = process.output_handles();
                (
                    handles.output_buffer,
                    handles.output_notify,
                    handles.output_closed,
                    handles.output_closed_notify,
                    handles.cancellation_token,
                )
            }
            Self::ExecServer {
                output_buffer,
                output_notify,
                output_closed,
                output_closed_notify,
                cancellation_token,
                ..
            } => (
                Arc::clone(output_buffer),
                Arc::clone(output_notify),
                Arc::clone(output_closed),
                Arc::clone(output_closed_notify),
                cancellation_token.clone(),
            ),
        }
    }

    pub(crate) async fn check_for_sandbox_denial_with_text(
        &self,
        text: &str,
    ) -> Result<(), UnifiedExecError> {
        let sandbox_type = self.sandbox_type();
        if sandbox_type == SandboxType::None || !self.has_exited() {
            return Ok(());
        }

        match self {
            Self::Local { process } => {
                process.check_for_sandbox_denial_with_text(text).await
            }
            Self::ExecServer { .. } => {
                let exit_code = self.exit_code().unwrap_or(-1);
                let exec_output = ExecToolCallOutput {
                    exit_code,
                    stderr: StreamOutput::new(text.to_string()),
                    aggregated_output: StreamOutput::new(text.to_string()),
                    ..Default::default()
                };

                if is_likely_sandbox_denied(sandbox_type, &exec_output) {
                    let snippet = formatted_truncate_text(
                        text,
                        TruncationPolicy::Tokens(UNIFIED_EXEC_OUTPUT_MAX_TOKENS),
                    );
                    let message = if snippet.is_empty() {
                        format!("Process exited with code {exit_code}")
                    } else {
                        snippet
                    };
                    return Err(UnifiedExecError::sandbox_denied(message, exec_output));
                }

                Ok(())
            }
        }
    }

    fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Local { process } => process.exit_code(),
            Self::ExecServer { exit_code, .. } => *exit_code.read().unwrap_or_else(|err| err.into_inner()),
        }
    }

    pub(crate) fn set_exit_code(&self, exit_code: i32) {
        if let Self::ExecServer { exit_code: state, .. } = self {
            let mut guard = state.write().unwrap_or_else(|err| err.into_inner());
            *guard = Some(exit_code);
        }
    }

    pub(crate) fn mark_exited(&self) {
        if let Self::ExecServer { has_exited, .. } = self {
            has_exited.store(true, Ordering::Release);
        }
    }

    pub(crate) fn has_exited(&self) -> bool {
        match self {
            Self::Local { process } => process.has_exited(),
            Self::ExecServer { has_exited, .. } => has_exited.load(Ordering::Acquire),
        }
    }

    pub(crate) fn cancellation_token(&self) -> Option<CancellationToken> {
        match self {
            Self::Local { process } => Some(process.cancellation_token()),
            Self::ExecServer {
                cancellation_token, ..
            } => Some(cancellation_token.clone()),
        }
    }

    pub(crate) fn output_drained(&self) -> Option<Arc<Notify>> {
        match self {
            Self::Local { process } => Some(process.output_drained_notify()),
            Self::ExecServer { output_drained, .. } => Some(Arc::clone(output_drained)),
        }
    }

    pub(crate) fn exit_code_handle(&self) -> Option<Arc<RwLock<Option<i32>>> {
        match self {
            Self::ExecServer { exit_code, .. } => Some(Arc::clone(exit_code)),
            Self::Local { .. } => None,
        }
    }

    pub(crate) fn remote_output_seq(&self) -> Option<Arc<AtomicU64>> {
        match self {
            Self::ExecServer { output_seq, .. } => Some(Arc::clone(output_seq)),
            Self::Local { .. } => None,
        }
    }

    pub(crate) async fn write_stdin(&self, data: &[u8]) -> Result<(), UnifiedExecError> {
        match self {
            Self::Local { process } => process
                .writer_sender()
                .send(data.to_vec())
                .await
                .map_err(|_| UnifiedExecError::WriteToStdin)?,
            Self::ExecServer {
                process_id,
                executor,
                ..
            } => {
                let response = executor
                    .write(process_id.as_str(), data.to_vec())
                    .await
                    .map_err(|_| UnifiedExecError::WriteToStdin)?;
                if !response.accepted {
                    return Err(UnifiedExecError::WriteToStdin);
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn terminate(&self) {
        match self {
            Self::Local { process } => process.terminate(),
            Self::ExecServer {
                process_id,
                executor,
                output_closed,
                output_closed_notify,
                output_drained,
                cancellation_token,
                has_exited,
                ..
            } => {
                has_exited.store(true, Ordering::Release);
                output_closed.store(true, Ordering::Release);
                output_closed_notify.notify_waiters();
                output_drained.notify_one();
                cancellation_token.cancel();
                let _ = executor.terminate(process_id.as_str()).await;
            }
        }
    }

    pub(crate) fn sandbox_type(&self) -> SandboxType {
        match self {
            Self::Local { process } => process.sandbox_type(),
            Self::ExecServer { sandbox_type, .. } => *sandbox_type,
        }
    }

    pub(crate) fn remote_exec_state(&self) -> Option<(&str, &Arc<dyn ExecProcess>)> {
        match self {
            Self::ExecServer {
                process_id,
                executor,
                ..
            } => Some((process_id.as_str(), executor)),
            Self::Local { .. } => None,
        }
    }
}

pub(crate) fn clamp_yield_time(yield_time_ms: u64) -> u64 {
    yield_time_ms.clamp(MIN_YIELD_TIME_MS, MAX_YIELD_TIME_MS)
}

pub(crate) fn resolve_max_tokens(max_tokens: Option<usize>) -> usize {
    max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
}

pub(crate) fn generate_chunk_id() -> String {
    let mut rng = rng();
    (0..6)
        .map(|_| format!("{:x}", rng.random_range(0..16)))
        .collect()
}

#[cfg(test)]
#[cfg(unix)]
#[path = "mod_tests.rs"]
mod tests;
