use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use tokio::sync::broadcast;

use super::ExecServerClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecServerOutput {
    pub(super) stream: crate::protocol::ExecOutputStream,
    pub(super) chunk: Vec<u8>,
}

pub(super) struct ExecServerProcess {
    pub(super) process_id: String,
    pub(super) output_rx: broadcast::Receiver<ExecServerOutput>,
    pub(super) status: Arc<RemoteProcessStatus>,
    pub(super) client: ExecServerClient,
}

impl ExecServerProcess {
    pub(super) fn output_receiver(&self) -> broadcast::Receiver<ExecServerOutput> {
        self.output_rx.resubscribe()
    }

    pub(super) fn has_exited(&self) -> bool {
        self.status.has_exited()
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        self.status.exit_code()
    }

    pub(super) fn terminate(&self) {
        let client = self.client.clone();
        let process_id = self.process_id.clone();
        tokio::spawn(async move {
            let _ = client.terminate(&process_id).await;
        });
    }
}

pub(super) struct RemoteProcessStatus {
    exited: AtomicBool,
    exit_code: StdMutex<Option<i32>>,
}

impl RemoteProcessStatus {
    pub(super) fn new() -> Self {
        Self {
            exited: AtomicBool::new(false),
            exit_code: StdMutex::new(None),
        }
    }

    pub(super) fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        self.exit_code.lock().ok().and_then(|guard| *guard)
    }

    pub(super) fn mark_exited(&self, exit_code: Option<i32>) {
        self.exited.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.exit_code.lock() {
            *guard = exit_code;
        }
    }
}
