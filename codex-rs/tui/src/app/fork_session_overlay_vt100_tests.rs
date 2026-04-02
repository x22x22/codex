use super::super::FeedbackAudience;
use super::super::WindowsSandboxState;
use super::super::agent_navigation::AgentNavigationState;
use super::*;
use crate::app::App;
use crate::app_backtrack::BacktrackState;
use crate::chatwidget::tests::make_chatwidget_manual_with_sender;
use crate::custom_terminal::Terminal;
use crate::file_search::FileSearchManager;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;
use crate::test_backend::VT100Backend;
use codex_config::types::ApprovalsReviewer;
use codex_core::config::ConfigOverrides;
use codex_login::CodexAuth;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use codex_protocol::protocol::SessionSource;
use insta::assert_snapshot;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tempfile::NamedTempFile;

async fn make_test_app() -> App {
    let (chat_widget, app_event_tx, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
    let config = chat_widget.config_ref().clone();
    let _server = Arc::new(
        codex_core::test_support::thread_manager_with_models_provider(
            CodexAuth::from_api_key("Test API Key"),
            config.model_provider.clone(),
        ),
    );
    let _auth_manager =
        codex_core::test_support::auth_manager_from_auth(CodexAuth::from_api_key("Test API Key"));
    let file_search = FileSearchManager::new(config.cwd.to_path_buf(), app_event_tx.clone());
    let model = codex_core::test_support::get_model_offline(config.model.as_deref());
    let session_telemetry = SessionTelemetry::new(
        ThreadId::new(),
        model.as_str(),
        model.as_str(),
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "test".to_string(),
        SessionSource::Cli,
    );

    App {
        model_catalog: chat_widget.model_catalog(),
        session_telemetry,
        app_event_tx,
        chat_widget,
        config,
        active_profile: None,
        cli_kv_overrides: Vec::new(),
        harness_overrides: ConfigOverrides::default(),
        runtime_approval_policy_override: None,
        runtime_sandbox_policy_override: None,
        file_search,
        transcript_cells: Vec::new(),
        overlay: None,
        fork_session_overlay: None,
        deferred_history_lines: Vec::new(),
        has_emitted_history_lines: false,
        enhanced_keys_supported: false,
        commit_anim_running: Arc::new(AtomicBool::new(false)),
        status_line_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        terminal_title_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        backtrack: BacktrackState::default(),
        backtrack_render_pending: false,
        feedback: codex_feedback::CodexFeedback::new(),
        feedback_audience: FeedbackAudience::External,
        remote_app_server_url: None,
        remote_app_server_auth_token: None,
        pending_update_action: None,
        pending_shutdown_exit_thread_id: None,
        windows_sandbox: WindowsSandboxState::default(),
        thread_event_channels: HashMap::new(),
        thread_event_listener_tasks: HashMap::new(),
        agent_navigation: AgentNavigationState::default(),
        active_thread_id: None,
        active_thread_rx: None,
        primary_thread_id: None,
        last_subagent_backfill_attempt: None,
        primary_session_configured: None,
        pending_primary_events: VecDeque::new(),
        pending_app_server_requests: Default::default(),
    }
}

fn configure_chat_widget(app: &mut App) {
    let rollout_file = NamedTempFile::new().expect("rollout file");
    app.chat_widget.handle_codex_event(Event {
        id: "configured".to_string(),
        msg: EventMsg::SessionConfigured(SessionConfiguredEvent {
            session_id: ThreadId::new(),
            forked_from_id: None,
            thread_name: None,
            model: "gpt-5.4".to_string(),
            model_provider_id: "test-provider".to_string(),
            service_tier: None,
            approval_policy: AskForApproval::Never,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: SandboxPolicy::new_read_only_policy(),
            cwd: PathBuf::from("/tmp/worktree"),
            reasoning_effort: Some(ReasoningEffortConfig::XHigh),
            history_log_id: 0,
            history_entry_count: 0,
            initial_messages: None,
            network_proxy: None,
            rollout_path: Some(rollout_file.path().to_path_buf()),
        }),
    });
}

fn vt100_contents(terminal: &Terminal<VT100Backend>) -> String {
    terminal.backend().vt100().screen().contents()
}

fn draw_vt100_terminal_frame(
    terminal: &mut Terminal<VT100Backend>,
    pending_history_lines: &mut Vec<ratatui::text::Line<'static>>,
    height: u16,
    draw_fn: impl FnOnce(&mut crate::custom_terminal::Frame),
) -> Result<Rect> {
    let size = terminal.size()?;

    let mut area = terminal.viewport_area;
    area.height = height.min(size.height);
    area.width = size.width;
    if area.bottom() > size.height {
        terminal
            .backend_mut()
            .scroll_region_up(0..area.top(), area.bottom() - size.height)?;
        area.y = size.height - area.height;
    }
    if area != terminal.viewport_area {
        terminal.clear()?;
        terminal.set_viewport_area(area);
    }

    if !pending_history_lines.is_empty() {
        crate::insert_history::insert_history_lines(terminal, pending_history_lines.clone())?;
        pending_history_lines.clear();
    }

    terminal.draw(|frame| {
        draw_fn(frame);
    })?;

    Ok(area)
}

#[tokio::test]
#[ignore = "manual harness for inline viewport /fork overlay rendering"]
async fn fork_session_overlay_open_from_inline_viewport_snapshot() {
    let width = 100;
    let height = 28;
    let mut app = make_test_app().await;
    configure_chat_widget(&mut app);
    app.chat_widget.set_composer_text(
        "Summarize recent commits".to_string(),
        Vec::new(),
        Vec::new(),
    );
    app.transcript_cells = vec![
        Arc::new(history_cell::new_user_prompt(
            "count to 10".to_string(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )),
        Arc::new(PlainHistoryCell::new(
            (1..=10)
                .map(|n| n.to_string().into())
                .collect::<Vec<ratatui::text::Line<'static>>>(),
        )),
    ];

    let mut child = vt100::Parser::new(18, 74, 0);
    child.process(
        b"\x1b[48;2;58;61;67m count to 10 \x1b[0m\r\n\
\r\n\
1\r\n\
2\r\n\
3\r\n\
4\r\n\
5\r\n\
6\r\n\
7\r\n\
8\r\n\
9\r\n\
10\r\n\
\r\n\
\x1b[38;2;94;129;172m\xE2\x80\xA2\x1b[0m Thread forked from 019cecc9-0318-74d3-a020-2a21605271f8\r\n\
\r\n\
\x1b[48;2;58;61;67m \x1b[0m> /skills to list available skills\r\n",
    );

    let mut terminal =
        Terminal::with_options(VT100Backend::new(width, height)).expect("create vt100 terminal");
    terminal.set_viewport_area(Rect::new(0, height - 7, width, 7));

    app.fork_session_overlay = Some(ForkSessionOverlayStack::new(ForkSessionOverlayState {
        terminal: ForkSessionTerminal::for_test(child, /*exit_code*/ None),
        popup: default_popup_rect(Rect::new(0, 0, width, height)),
        command_state: OverlayCommandState::PassThrough,
        drag_state: None,
    }));

    let mut pending_history_lines = Vec::new();
    draw_vt100_terminal_frame(&mut terminal, &mut pending_history_lines, height, |frame| {
        app.render_fork_session_background(frame);
        if let Some((x, y)) = app.render_fork_session_overlay_frame(frame) {
            frame.set_cursor_position((x, y));
        }
    })
    .expect("draw overlay frame");

    assert_snapshot!(
        "fork_session_overlay_open_from_inline_viewport_vt100",
        vt100_contents(&terminal)
    );
}
