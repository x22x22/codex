use core::fmt;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use portable_pty::MasterPty;
use portable_pty::SlavePty;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;

pub(crate) trait ChildTerminator: Send + Sync {
    fn kill(&mut self) -> io::Result<()>;
}

pub struct PtyHandles {
    pub _slave: Option<Box<dyn SlavePty + Send>>,
    pub _master: Box<dyn MasterPty + Send>,
}

impl fmt::Debug for PtyHandles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtyHandles").finish()
    }
}

/// Handle for driving an interactive process (PTY or pipe).
pub struct ProcessHandle {
    writer_tx: mpsc::Sender<Vec<u8>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    killer: StdMutex<Option<Box<dyn ChildTerminator>>>,
    reader_handle: StdMutex<Option<JoinHandle<()>>>,
    reader_abort_handles: StdMutex<Vec<AbortHandle>>,
    writer_handle: StdMutex<Option<JoinHandle<()>>>,
    wait_handle: StdMutex<Option<JoinHandle<()>>>,
    exit_status: Arc<AtomicBool>,
    exit_code: Arc<StdMutex<Option<i32>>>,
    // PtyHandles must be preserved because the process will receive Control+C if the
    // slave is closed
    _pty_handles: StdMutex<Option<PtyHandles>>,
}

impl fmt::Debug for ProcessHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessHandle").finish()
    }
}

impl ProcessHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        output_tx: broadcast::Sender<Vec<u8>>,
        initial_output_rx: broadcast::Receiver<Vec<u8>>,
        killer: Box<dyn ChildTerminator>,
        reader_handle: JoinHandle<()>,
        reader_abort_handles: Vec<AbortHandle>,
        writer_handle: JoinHandle<()>,
        wait_handle: JoinHandle<()>,
        exit_status: Arc<AtomicBool>,
        exit_code: Arc<StdMutex<Option<i32>>>,
        pty_handles: Option<PtyHandles>,
    ) -> (Self, broadcast::Receiver<Vec<u8>>) {
        (
            Self {
                writer_tx,
                output_tx,
                killer: StdMutex::new(Some(killer)),
                reader_handle: StdMutex::new(Some(reader_handle)),
                reader_abort_handles: StdMutex::new(reader_abort_handles),
                writer_handle: StdMutex::new(Some(writer_handle)),
                wait_handle: StdMutex::new(Some(wait_handle)),
                exit_status,
                exit_code,
                _pty_handles: StdMutex::new(pty_handles),
            },
            initial_output_rx,
        )
    }

    /// Returns a channel sender for writing raw bytes to the child stdin.
    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx.clone()
    }

    /// Returns a broadcast receiver that yields stdout/stderr chunks.
    pub fn output_receiver(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    /// True if the child process has exited.
    pub fn has_exited(&self) -> bool {
        self.exit_status.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Returns the exit code if known.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code.lock().ok().and_then(|guard| *guard)
    }

    /// Attempts to kill the child and abort helper tasks.
    pub fn terminate(&self) {
        if let Ok(mut killer_opt) = self.killer.lock() {
            if let Some(mut killer) = killer_opt.take() {
                let _ = killer.kill();
            }
        }

        if let Ok(mut h) = self.reader_handle.lock() {
            if let Some(handle) = h.take() {
                handle.abort();
            }
        }
        if let Ok(mut handles) = self.reader_abort_handles.lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
        if let Ok(mut h) = self.writer_handle.lock() {
            if let Some(handle) = h.take() {
                handle.abort();
            }
        }
        if let Ok(mut h) = self.wait_handle.lock() {
            if let Some(handle) = h.take() {
                handle.abort();
            }
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Adapts a closure into a `ChildTerminator` implementation.
struct ClosureTerminator {
    inner: Option<Box<dyn FnMut() + Send + Sync>>,
}

impl ChildTerminator for ClosureTerminator {
    fn kill(&mut self) -> io::Result<()> {
        if let Some(inner) = self.inner.as_mut() {
            (inner)();
        }
        Ok(())
    }
}

/// Driver-backed process handles for non-standard spawn backends.
pub struct ProcessDriver {
    pub writer_tx: mpsc::Sender<Vec<u8>>,
    pub output_rx: broadcast::Receiver<Vec<u8>>,
    pub exit_rx: oneshot::Receiver<i32>,
    pub terminator: Option<Box<dyn FnMut() + Send + Sync>>,
    pub writer_handle: Option<JoinHandle<()>>,
}

/// Return value from spawn helpers (PTY or pipe).
#[derive(Debug)]
pub struct SpawnedProcess {
    pub session: ProcessHandle,
    pub output_rx: broadcast::Receiver<Vec<u8>>,
    pub exit_rx: oneshot::Receiver<i32>,
}

/// Build a `SpawnedProcess` from a driver that supplies stdin/output/exit channels.
pub fn spawn_from_driver(driver: ProcessDriver) -> SpawnedProcess {
    let ProcessDriver {
        writer_tx,
        mut output_rx,
        exit_rx,
        terminator,
        writer_handle,
    } = driver;

    let (output_tx, _) = broadcast::channel::<Vec<u8>>(256);
    let initial_output_rx = output_tx.subscribe();
    let output_tx_clone = output_tx.clone();
    let reader_handle = tokio::spawn(async move {
        loop {
            match output_rx.recv().await {
                Ok(chunk) => {
                    let _ = output_tx_clone.send(chunk);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let writer_handle = writer_handle.unwrap_or_else(|| tokio::spawn(async {}));

    let (exit_tx, exit_rx_out) = oneshot::channel::<i32>();
    let exit_status = Arc::new(AtomicBool::new(false));
    let wait_exit_status = Arc::clone(&exit_status);
    let exit_code = Arc::new(StdMutex::new(None));
    let wait_exit_code = Arc::clone(&exit_code);
    let wait_handle = tokio::spawn(async move {
        let code = exit_rx.await.unwrap_or(-1);
        wait_exit_status.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = wait_exit_code.lock() {
            *guard = Some(code);
        }
        let _ = exit_tx.send(code);
    });

    let killer = Box::new(ClosureTerminator { inner: terminator });
    let (handle, output_rx) = ProcessHandle::new(
        writer_tx,
        output_tx,
        initial_output_rx,
        killer,
        reader_handle,
        Vec::new(),
        writer_handle,
        wait_handle,
        exit_status,
        exit_code,
        None,
    );

    SpawnedProcess {
        session: handle,
        output_rx,
        exit_rx: exit_rx_out,
    }
}
