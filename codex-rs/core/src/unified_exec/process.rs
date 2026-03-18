#![allow(clippy::module_inception)]

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot::error::TryRecvError;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::exec::ExecToolCallOutput;
use crate::exec::SandboxType;
use crate::exec::StreamOutput;
use crate::exec::is_likely_sandbox_denied;
use crate::sandboxing::ExecRequest;
use crate::truncate::TruncationPolicy;
use crate::truncate::formatted_truncate_text;
use codex_exec_server::ExecParams;
use codex_exec_server::ExecServerClient;
use codex_exec_server::ExecServerEvent;
use codex_utils_pty::ExecCommandSession;
use codex_utils_pty::SpawnedPty;

use super::UNIFIED_EXEC_OUTPUT_MAX_TOKENS;
use super::UnifiedExecError;
use super::head_tail_buffer::HeadTailBuffer;

pub(crate) trait SpawnLifecycle: std::fmt::Debug + Send + Sync {
    /// Returns file descriptors that must stay open across the child `exec()`.
    ///
    /// The returned descriptors must already be valid in the parent process and
    /// stay valid until `after_spawn()` runs, which is the first point where
    /// the parent may release its copies.
    fn inherited_fds(&self) -> Vec<i32> {
        Vec::new()
    }

    fn after_spawn(&mut self) {}
}

pub(crate) type SpawnLifecycleHandle = Box<dyn SpawnLifecycle>;

#[derive(Debug, Default)]
pub(crate) struct NoopSpawnLifecycle;

impl SpawnLifecycle for NoopSpawnLifecycle {}

pub(crate) type OutputBuffer = Arc<Mutex<HeadTailBuffer>>;
pub(crate) struct OutputHandles {
    pub(crate) output_buffer: OutputBuffer,
    pub(crate) output_notify: Arc<Notify>,
    pub(crate) output_closed: Arc<AtomicBool>,
    pub(crate) output_closed_notify: Arc<Notify>,
    pub(crate) cancellation_token: CancellationToken,
}

#[derive(Debug)]
pub(crate) struct UnifiedExecProcess {
    process_handle: ProcessBackend,
    output_rx: broadcast::Receiver<Vec<u8>>,
    output_buffer: OutputBuffer,
    output_notify: Arc<Notify>,
    output_closed: Arc<AtomicBool>,
    output_closed_notify: Arc<Notify>,
    cancellation_token: CancellationToken,
    output_drained: Arc<Notify>,
    output_task: JoinHandle<()>,
    sandbox_type: SandboxType,
    _spawn_lifecycle: SpawnLifecycleHandle,
}

enum ProcessBackend {
    Local(ExecCommandSession),
    Remote(RemoteExecSession),
}

impl std::fmt::Debug for ProcessBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(process_handle) => f.debug_tuple("Local").field(process_handle).finish(),
            Self::Remote(process_handle) => f.debug_tuple("Remote").field(process_handle).finish(),
        }
    }
}

#[derive(Clone)]
struct RemoteExecSession {
    process_key: String,
    client: ExecServerClient,
    writer_tx: mpsc::Sender<Vec<u8>>,
    exited: Arc<AtomicBool>,
    exit_code: Arc<StdMutex<Option<i32>>>,
}

impl std::fmt::Debug for RemoteExecSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteExecSession")
            .field("process_key", &self.process_key)
            .field("exited", &self.exited.load(Ordering::SeqCst))
            .field(
                "exit_code",
                &self.exit_code.lock().ok().and_then(|guard| *guard),
            )
            .finish_non_exhaustive()
    }
}

impl UnifiedExecProcess {
    fn new(
        process_handle: ProcessBackend,
        initial_output_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
        sandbox_type: SandboxType,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Self {
        let output_buffer = Arc::new(Mutex::new(HeadTailBuffer::default()));
        let output_notify = Arc::new(Notify::new());
        let output_closed = Arc::new(AtomicBool::new(false));
        let output_closed_notify = Arc::new(Notify::new());
        let cancellation_token = CancellationToken::new();
        let output_drained = Arc::new(Notify::new());
        let mut receiver = initial_output_rx;
        let output_rx = receiver.resubscribe();
        let buffer_clone = Arc::clone(&output_buffer);
        let notify_clone = Arc::clone(&output_notify);
        let output_closed_clone = Arc::clone(&output_closed);
        let output_closed_notify_clone = Arc::clone(&output_closed_notify);
        let output_task = tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(chunk) => {
                        let mut guard = buffer_clone.lock().await;
                        guard.push_chunk(chunk);
                        drop(guard);
                        notify_clone.notify_waiters();
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        output_closed_clone.store(true, Ordering::Release);
                        output_closed_notify_clone.notify_waiters();
                        break;
                    }
                };
            }
        });

        Self {
            process_handle,
            output_rx,
            output_buffer,
            output_notify,
            output_closed,
            output_closed_notify,
            cancellation_token,
            output_drained,
            output_task,
            sandbox_type,
            _spawn_lifecycle: spawn_lifecycle,
        }
    }

    pub(super) fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        match &self.process_handle {
            ProcessBackend::Local(process_handle) => process_handle.writer_sender(),
            ProcessBackend::Remote(process_handle) => process_handle.writer_tx.clone(),
        }
    }

    pub(super) fn output_handles(&self) -> OutputHandles {
        OutputHandles {
            output_buffer: Arc::clone(&self.output_buffer),
            output_notify: Arc::clone(&self.output_notify),
            output_closed: Arc::clone(&self.output_closed),
            output_closed_notify: Arc::clone(&self.output_closed_notify),
            cancellation_token: self.cancellation_token.clone(),
        }
    }

    pub(super) fn output_receiver(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>> {
        self.output_rx.resubscribe()
    }

    pub(super) fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    pub(super) fn output_drained_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.output_drained)
    }

    pub(super) fn has_exited(&self) -> bool {
        match &self.process_handle {
            ProcessBackend::Local(process_handle) => process_handle.has_exited(),
            ProcessBackend::Remote(process_handle) => process_handle.exited.load(Ordering::SeqCst),
        }
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        match &self.process_handle {
            ProcessBackend::Local(process_handle) => process_handle.exit_code(),
            ProcessBackend::Remote(process_handle) => process_handle
                .exit_code
                .lock()
                .ok()
                .and_then(|guard| *guard),
        }
    }

    pub(super) fn terminate(&self) {
        self.output_closed.store(true, Ordering::Release);
        self.output_closed_notify.notify_waiters();
        match &self.process_handle {
            ProcessBackend::Local(process_handle) => process_handle.terminate(),
            ProcessBackend::Remote(process_handle) => {
                let client = process_handle.client.clone();
                let process_key = process_handle.process_key.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let _ = client.terminate(&process_key).await;
                    });
                }
            }
        }
        self.cancellation_token.cancel();
        self.output_task.abort();
    }

    async fn snapshot_output(&self) -> Vec<Vec<u8>> {
        let guard = self.output_buffer.lock().await;
        guard.snapshot_chunks()
    }

    pub(crate) fn sandbox_type(&self) -> SandboxType {
        self.sandbox_type
    }

    pub(super) async fn check_for_sandbox_denial(&self) -> Result<(), UnifiedExecError> {
        let _ =
            tokio::time::timeout(Duration::from_millis(20), self.output_notify.notified()).await;

        let collected_chunks = self.snapshot_output().await;
        let mut aggregated: Vec<u8> = Vec::new();
        for chunk in collected_chunks {
            aggregated.extend_from_slice(&chunk);
        }
        let aggregated_text = String::from_utf8_lossy(&aggregated).to_string();
        self.check_for_sandbox_denial_with_text(&aggregated_text)
            .await?;

        Ok(())
    }

    pub(super) async fn check_for_sandbox_denial_with_text(
        &self,
        text: &str,
    ) -> Result<(), UnifiedExecError> {
        let sandbox_type = self.sandbox_type();
        if sandbox_type == SandboxType::None || !self.has_exited() {
            return Ok(());
        }

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

    pub(super) async fn from_spawned(
        spawned: SpawnedPty,
        sandbox_type: SandboxType,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<Self, UnifiedExecError> {
        let SpawnedPty {
            session: process_handle,
            stdout_rx,
            stderr_rx,
            mut exit_rx,
        } = spawned;
        let output_rx = codex_utils_pty::combine_output_receivers(stdout_rx, stderr_rx);
        let managed = Self::new(
            ProcessBackend::Local(process_handle),
            output_rx,
            sandbox_type,
            spawn_lifecycle,
        );

        let exit_ready = matches!(exit_rx.try_recv(), Ok(_) | Err(TryRecvError::Closed));

        if exit_ready {
            managed.signal_exit();
            managed.check_for_sandbox_denial().await?;
            return Ok(managed);
        }

        if tokio::time::timeout(Duration::from_millis(150), &mut exit_rx)
            .await
            .is_ok()
        {
            managed.signal_exit();
            managed.check_for_sandbox_denial().await?;
            return Ok(managed);
        }

        tokio::spawn({
            let cancellation_token = managed.cancellation_token.clone();
            async move {
                let _ = exit_rx.await;
                cancellation_token.cancel();
            }
        });

        Ok(managed)
    }

    pub(super) async fn from_exec_server(
        client: ExecServerClient,
        process_id: i32,
        env: &ExecRequest,
        tty: bool,
        spawn_lifecycle: SpawnLifecycleHandle,
    ) -> Result<Self, UnifiedExecError> {
        let process_key = process_id.to_string();
        let mut events_rx = client.event_receiver();
        client
            .exec(ExecParams {
                process_id: process_key.clone(),
                argv: env.command.clone(),
                cwd: env.cwd.clone(),
                env: env.env.clone(),
                tty,
                arg0: env.arg0.clone(),
                sandbox: None,
            })
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;

        let (output_tx, output_rx) = broadcast::channel(256);
        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(256);
        let exited = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(StdMutex::new(None));

        let managed = Self::new(
            ProcessBackend::Remote(RemoteExecSession {
                process_key: process_key.clone(),
                client: client.clone(),
                writer_tx,
                exited: Arc::clone(&exited),
                exit_code: Arc::clone(&exit_code),
            }),
            output_rx,
            env.sandbox,
            spawn_lifecycle,
        );

        {
            let client = client.clone();
            tokio::spawn(async move {
                while let Some(chunk) = writer_rx.recv().await {
                    if client.write(&process_key, chunk).await.is_err() {
                        break;
                    }
                }
            });
        }

        {
            let process_key = process_id.to_string();
            let exited = Arc::clone(&exited);
            let exit_code = Arc::clone(&exit_code);
            let cancellation_token = managed.cancellation_token();
            tokio::spawn(async move {
                while let Ok(event) = events_rx.recv().await {
                    match event {
                        ExecServerEvent::OutputDelta(notification)
                            if notification.process_id == process_key =>
                        {
                            let _ = output_tx.send(notification.chunk.into_inner());
                        }
                        ExecServerEvent::Exited(notification)
                            if notification.process_id == process_key =>
                        {
                            exited.store(true, Ordering::SeqCst);
                            if let Ok(mut guard) = exit_code.lock() {
                                *guard = Some(notification.exit_code);
                            }
                            cancellation_token.cancel();
                            break;
                        }
                        ExecServerEvent::OutputDelta(_) | ExecServerEvent::Exited(_) => {}
                    }
                }
            });
        }

        Ok(managed)
    }

    fn signal_exit(&self) {
        self.cancellation_token.cancel();
    }
}

impl Drop for UnifiedExecProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}
