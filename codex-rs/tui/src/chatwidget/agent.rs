use codex_protocol::protocol::Op;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::unbounded_channel;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

/// Spawn an op forwarder that routes all widget submissions back onto the app event bus.
///
/// The app layer owns app-server request translation during the migration, so the chat widget
/// should no longer need a direct thread handle for normal runtime operation.
pub(crate) fn spawn_app_event_forwarder(app_event_tx: AppEventSender) -> UnboundedSender<Op> {
    let (codex_op_tx, mut codex_op_rx) = unbounded_channel::<Op>();

    tokio::spawn(async move {
        while let Some(op) = codex_op_rx.recv().await {
            app_event_tx.send(AppEvent::CodexOp(op));
        }
    });

    codex_op_tx
}
