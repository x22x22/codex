use std::sync::Arc;

use codex_core::CodexThread;
use codex_core::NewThread;
use codex_core::ThreadManager;
use codex_core::config::Config;
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
                let message = format!("Failed to initialize codex: {err}");
                tracing::error!("{message}");
                app_event_tx_clone.send(AppEvent::CodexEvent(Event {
                    id: "".to_string(),
                    msg: EventMsg::Error(err.to_error_event(None)),
                }));
                app_event_tx_clone.send(AppEvent::FatalExitRequest(message));
                tracing::error!("failed to initialize codex: {err}");
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
    use crate::app_event::AppEvent;
    use codex_core::CodexAuth;
    use codex_core::config::ConfigBuilder;
    use codex_core::config::ConfigOverrides;
    use codex_core::config_loader::LoaderOverrides;
    use codex_protocol::protocol::EventMsg;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::Duration;
    use tokio::time::timeout;

    async fn build_malformed_rules_config(
        codex_home: &Path,
        cwd: &Path,
    ) -> std::io::Result<Config> {
        let cwd_display = cwd.display().to_string().replace('\'', "''");
        let config_contents = format!(
            r#"model_provider = "ollama"

[projects.'{cwd_display}']
trust_level = "trusted"
"#
        );
        std::fs::write(codex_home.join("config.toml"), config_contents)?;

        ConfigBuilder::default()
            .codex_home(codex_home.to_path_buf())
            .harness_overrides(ConfigOverrides {
                cwd: Some(cwd.to_path_buf()),
                ..ConfigOverrides::default()
            })
            .loader_overrides(LoaderOverrides {
                #[cfg(target_os = "macos")]
                managed_preferences_base64: Some(String::new()),
                macos_managed_config_requirements_base64: Some(String::new()),
                ..LoaderOverrides::default()
            })
            .build()
            .await
    }

    #[tokio::test]
    async fn malformed_rules_emit_graceful_startup_error() {
        let codex_home = tempdir().expect("temp codex home");
        let project_dir = tempdir().expect("temp project dir");
        std::fs::write(
            codex_home.path().join("rules"),
            "rules should be a directory not a file",
        )
        .expect("write malformed rules fixture");

        let config = build_malformed_rules_config(codex_home.path(), project_dir.path())
            .await
            .expect("load config");
        let manager = Arc::new(
            codex_core::test_support::thread_manager_with_models_provider_and_home(
                CodexAuth::from_api_key("dummy"),
                config.model_provider.clone(),
                config.codex_home.clone(),
            ),
        );
        let (app_event_tx, mut app_event_rx) = unbounded_channel();

        let _codex_op_tx = spawn_agent(config, AppEventSender::new(app_event_tx), manager);

        let mut startup_error_message = None;
        let fatal_message = timeout(Duration::from_secs(5), async {
            loop {
                match app_event_rx.recv().await {
                    Some(AppEvent::CodexEvent(event)) => {
                        if let EventMsg::Error(err) = event.msg {
                            startup_error_message = Some(err.message);
                        }
                    }
                    Some(AppEvent::FatalExitRequest(message)) => break message,
                    Some(_) => {}
                    None => panic!("app event channel closed before fatal startup error"),
                }
            }
        })
        .await
        .expect("wait for startup failure");

        assert!(
            fatal_message.contains("Failed to initialize codex:"),
            "expected fatal startup prefix, got: {fatal_message}"
        );
        assert!(
            fatal_message.contains("failed to read rules files"),
            "expected rules read error in fatal exit, got: {fatal_message}"
        );

        let startup_error_message =
            startup_error_message.expect("error event should precede fatal exit");
        assert!(
            startup_error_message.contains("failed to read rules files"),
            "expected rules read error event, got: {startup_error_message}"
        );
    }
}
