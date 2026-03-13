use std::sync::Arc;

use codex_core::CodexThread;
use codex_core::NewThread;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::error::CodexErr;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::unbounded_channel;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

const TUI_NOTIFY_CLIENT: &str = "codex-tui";

async fn initialize_app_server_client_name(thread: &CodexThread) {
    if let Err(err) = thread
        .set_app_server_client_name(Some(TUI_NOTIFY_CLIENT.to_string()))
        .await
    {
        tracing::error!("failed to set app server client name: {err}");
    }
}

fn emit_startup_error(app_event_tx: &AppEventSender, err: &CodexErr) {
    let message = format!("Failed to initialize codex: {err}");
    tracing::error!("{message}");
    app_event_tx.send(AppEvent::CodexEvent(Event {
        id: String::new(),
        msg: EventMsg::Error(err.to_error_event(None)),
    }));
    app_event_tx.send(AppEvent::FatalExitRequest(message));
    tracing::error!("failed to initialize codex: {err}");
}

/// Spawn the agent bootstrapper and op forwarding loop, returning the
/// `UnboundedSender<Op>` used by the UI to submit operations.
pub(crate) fn spawn_agent(
    config: Config,
    app_event_tx: AppEventSender,
    server: Arc<ThreadManager>,
) -> UnboundedSender<Op> {
    let (codex_op_tx, mut codex_op_rx) = unbounded_channel::<Op>();

    let app_event_tx_clone = app_event_tx;
    tokio::spawn(async move {
        let NewThread {
            thread,
            session_configured,
            ..
        } = match server.start_thread(config).await {
            Ok(v) => v,
            Err(err) => {
                emit_startup_error(&app_event_tx_clone, &err);
                return;
            }
        };
        initialize_app_server_client_name(thread.as_ref()).await;

        // Forward the captured `SessionConfigured` event so it can be rendered in the UI.
        let ev = codex_protocol::protocol::Event {
            // The `id` does not matter for rendering, so we can use a fake value.
            id: "".to_string(),
            msg: codex_protocol::protocol::EventMsg::SessionConfigured(session_configured),
        };
        app_event_tx_clone.send(AppEvent::CodexEvent(ev));

        let thread_clone = thread.clone();
        tokio::spawn(async move {
            while let Some(op) = codex_op_rx.recv().await {
                let id = thread_clone.submit(op).await;
                if let Err(e) = id {
                    tracing::error!("failed to submit op: {e}");
                }
            }
        });

        while let Ok(event) = thread.next_event().await {
            let is_shutdown_complete = matches!(event.msg, EventMsg::ShutdownComplete);
            app_event_tx_clone.send(AppEvent::CodexEvent(event));
            if is_shutdown_complete {
                // ShutdownComplete is terminal for a thread; drop this receiver task so
                // the Arc<CodexThread> can be released and thread resources can clean up.
                break;
            }
        }
    });

    codex_op_tx
}

/// Spawn agent loops for an existing thread (e.g., a forked thread).
/// Sends the provided `SessionConfiguredEvent` immediately, then forwards subsequent
/// events and accepts Ops for submission.
pub(crate) fn spawn_agent_from_existing(
    thread: std::sync::Arc<CodexThread>,
    session_configured: codex_protocol::protocol::SessionConfiguredEvent,
    app_event_tx: AppEventSender,
) -> UnboundedSender<Op> {
    let (codex_op_tx, mut codex_op_rx) = unbounded_channel::<Op>();

    let app_event_tx_clone = app_event_tx;
    tokio::spawn(async move {
        initialize_app_server_client_name(thread.as_ref()).await;

        // Forward the captured `SessionConfigured` event so it can be rendered in the UI.
        let ev = codex_protocol::protocol::Event {
            id: "".to_string(),
            msg: codex_protocol::protocol::EventMsg::SessionConfigured(session_configured),
        };
        app_event_tx_clone.send(AppEvent::CodexEvent(ev));

        let thread_clone = thread.clone();
        tokio::spawn(async move {
            while let Some(op) = codex_op_rx.recv().await {
                let id = thread_clone.submit(op).await;
                if let Err(e) = id {
                    tracing::error!("failed to submit op: {e}");
                }
            }
        });

        while let Ok(event) = thread.next_event().await {
            let is_shutdown_complete = matches!(event.msg, EventMsg::ShutdownComplete);
            app_event_tx_clone.send(AppEvent::CodexEvent(event));
            if is_shutdown_complete {
                // ShutdownComplete is terminal for a thread; drop this receiver task so
                // the Arc<CodexThread> can be released and thread resources can clean up.
                break;
            }
        }
    });

    codex_op_tx
}

/// Spawn an op-forwarding loop for an existing thread without subscribing to events.
pub(crate) fn spawn_op_forwarder(thread: std::sync::Arc<CodexThread>) -> UnboundedSender<Op> {
    let (codex_op_tx, mut codex_op_rx) = unbounded_channel::<Op>();

    tokio::spawn(async move {
        initialize_app_server_client_name(thread.as_ref()).await;
        while let Some(op) = codex_op_rx.recv().await {
            if let Err(e) = thread.submit(op).await {
                tracing::error!("failed to submit op: {e}");
            }
        }
    });

    codex_op_tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;
    #[test]
    fn startup_errors_are_forwarded_to_the_ui() {
        let (app_event_tx, mut app_event_rx) = unbounded_channel();
        let err = CodexErr::Io(std::io::Error::other("failed to read rules files"));

        emit_startup_error(&AppEventSender::new(app_event_tx), &err);

        let error_event = app_event_rx
            .try_recv()
            .expect("error event should be forwarded");
        let fatal_exit = app_event_rx
            .try_recv()
            .expect("fatal exit should be forwarded");

        let AppEvent::CodexEvent(event) = error_event else {
            panic!("expected CodexEvent, got {error_event:?}");
        };
        let EventMsg::Error(err) = event.msg else {
            panic!("expected Error event, got {:?}", event.msg);
        };
        assert!(
            err.message.contains("failed to read rules files"),
            "expected rules read error in forwarded event, got: {}",
            err.message
        );

        let AppEvent::FatalExitRequest(message) = fatal_exit else {
            panic!("expected FatalExitRequest, got {fatal_exit:?}");
        };
        assert!(
            message.contains("Failed to initialize codex:"),
            "expected fatal startup prefix, got: {message}"
        );
        assert!(
            message.contains("failed to read rules files"),
            "expected rules read error in fatal exit, got: {message}"
        );
    }
}
