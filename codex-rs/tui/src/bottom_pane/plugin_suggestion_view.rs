use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::mcp::RequestId as McpRequestId;
use codex_protocol::protocol::Op;
use codex_utils_absolute_path::AbsolutePathBuf;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use textwrap::wrap;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::mcp_server_elicitation::ToolSuggestionType;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height;
use super::selection_popup_common::render_rows;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::style::user_message_style;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginSuggestionElicitationTarget {
    pub(crate) thread_id: ThreadId,
    pub(crate) server_name: String,
    pub(crate) request_id: McpRequestId,
}

pub(crate) struct PluginSuggestionViewParams {
    pub(crate) plugin_id: String,
    pub(crate) title: String,
    pub(crate) plugin_name: String,
    pub(crate) marketplace_path: AbsolutePathBuf,
    pub(crate) suggest_reason: String,
    pub(crate) suggest_type: ToolSuggestionType,
    pub(crate) elicitation_target: PluginSuggestionElicitationTarget,
}

pub(crate) struct PluginSuggestionView {
    plugin_id: String,
    title: String,
    plugin_name: String,
    marketplace_path: AbsolutePathBuf,
    suggest_reason: String,
    suggest_type: ToolSuggestionType,
    elicitation_target: PluginSuggestionElicitationTarget,
    app_event_tx: AppEventSender,
    selected_action: usize,
    complete: bool,
}

impl PluginSuggestionView {
    pub(crate) fn new(params: PluginSuggestionViewParams, app_event_tx: AppEventSender) -> Self {
        let PluginSuggestionViewParams {
            plugin_id,
            title,
            plugin_name,
            marketplace_path,
            suggest_reason,
            suggest_type,
            elicitation_target,
        } = params;
        Self {
            plugin_id,
            title,
            plugin_name,
            marketplace_path,
            suggest_reason,
            suggest_type,
            elicitation_target,
            app_event_tx,
            selected_action: 0,
            complete: false,
        }
    }

    fn action_labels(&self) -> [&'static str; 2] {
        match self.suggest_type {
            ToolSuggestionType::Install => ["Install plugin", "Back"],
            ToolSuggestionType::Enable => ["Enable plugin", "Back"],
        }
    }

    fn instructions(&self) -> &'static str {
        match self.suggest_type {
            ToolSuggestionType::Install => {
                "Install this plugin in Codex to make its tools available."
            }
            ToolSuggestionType::Enable => {
                "Enable this plugin in Codex to use it for the current request."
            }
        }
    }

    fn move_selection_prev(&mut self) {
        self.selected_action = self.selected_action.saturating_sub(1);
    }

    fn move_selection_next(&mut self) {
        self.selected_action = (self.selected_action + 1).min(self.action_labels().len() - 1);
    }

    fn resolve_elicitation(&self, decision: ElicitationAction) {
        self.app_event_tx.send(AppEvent::SubmitThreadOp {
            thread_id: self.elicitation_target.thread_id,
            op: Op::ResolveElicitation {
                server_name: self.elicitation_target.server_name.clone(),
                request_id: self.elicitation_target.request_id.clone(),
                decision,
                content: None,
                meta: None,
            },
        });
    }

    fn decline(&mut self) {
        self.resolve_elicitation(ElicitationAction::Decline);
        self.complete = true;
    }

    fn submit_action(&mut self) {
        match self.suggest_type {
            ToolSuggestionType::Install => {
                self.app_event_tx.send(AppEvent::InstallSuggestedPlugin {
                    marketplace_path: self.marketplace_path.clone(),
                    plugin_name: self.plugin_name.clone(),
                })
            }
            ToolSuggestionType::Enable => self.app_event_tx.send(AppEvent::EnableSuggestedPlugin {
                plugin_id: self.plugin_id.clone(),
            }),
        }
        self.resolve_elicitation(ElicitationAction::Accept);
        self.complete = true;
    }

    fn activate_selected_action(&mut self) {
        match self.selected_action {
            0 => self.submit_action(),
            1 => self.decline(),
            _ => {}
        }
    }

    fn content_lines(&self, width: u16) -> Vec<Line<'static>> {
        let usable_width = width.max(1) as usize;
        let mut lines = vec![Line::from(self.title.clone().bold()), Line::from("")];

        for line in wrap(self.suggest_reason.trim(), usable_width) {
            lines.push(Line::from(line.into_owned().italic()));
        }
        lines.push(Line::from(""));

        for line in wrap(self.instructions(), usable_width) {
            lines.push(Line::from(line.into_owned()));
        }
        for line in wrap(
            "Changes take effect after the next router rebuild or next turn.",
            usable_width,
        ) {
            lines.push(Line::from(line.into_owned()));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            "Plugin ID: ".dim(),
            self.plugin_id.clone().into(),
        ]));

        lines
    }

    fn action_rows(&self) -> Vec<GenericDisplayRow> {
        self.action_labels()
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let prefix = if self.selected_action == index {
                    '›'
                } else {
                    ' '
                };
                GenericDisplayRow {
                    name: format!("{prefix} {}. {label}", index + 1),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn action_state(&self) -> ScrollState {
        let mut state = ScrollState::new();
        state.selected_idx = Some(self.selected_action);
        state
    }

    fn action_rows_height(&self, width: u16) -> u16 {
        let rows = self.action_rows();
        let state = self.action_state();
        measure_rows_height(&rows, &state, rows.len().max(1), width.max(1))
    }

    fn hint_line(&self) -> Line<'static> {
        Line::from(vec![
            "Use ".into(),
            key_hint::plain(KeyCode::Tab).into(),
            " / ".into(),
            key_hint::plain(KeyCode::Up).into(),
            " ".into(),
            key_hint::plain(KeyCode::Down).into(),
            " to move, ".into(),
            key_hint::plain(KeyCode::Enter).into(),
            " to select, ".into(),
            key_hint::plain(KeyCode::Esc).into(),
            " to close".into(),
        ])
    }
}

impl BottomPaneView for PluginSuggestionView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.on_ctrl_c();
            }
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Left,
                ..
            }
            | KeyEvent {
                code: KeyCode::BackTab,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_selection_prev(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Right,
                ..
            }
            | KeyEvent {
                code: KeyCode::Tab, ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_selection_next(),
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if let Some(index) = c
                    .to_digit(10)
                    .and_then(|digit| digit.checked_sub(1))
                    .map(|index| index as usize)
                    && index < self.action_labels().len()
                {
                    self.selected_action = index;
                    self.activate_selected_action();
                }
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.activate_selected_action(),
            _ => {}
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.decline();
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.complete
    }
}

impl crate::render::renderable::Renderable for PluginSuggestionView {
    fn desired_height(&self, width: u16) -> u16 {
        let content_width = width.saturating_sub(4).max(1);
        let content_rows = Paragraph::new(self.content_lines(content_width))
            .wrap(Wrap { trim: false })
            .line_count(content_width)
            .max(1) as u16;
        let action_rows_height = self.action_rows_height(content_width);
        content_rows + action_rows_height + 3
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        Block::default()
            .style(user_message_style())
            .render(area, buf);

        let actions_height = self.action_rows_height(area.width.saturating_sub(4));
        let [content_area, actions_area, hint_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(actions_height),
            Constraint::Length(1),
        ])
        .areas(area);

        let inner = content_area.inset(Insets::vh(1, 2));
        let content_width = inner.width.max(1);
        Paragraph::new(self.content_lines(content_width))
            .wrap(Wrap { trim: false })
            .render(inner, buf);

        if actions_area.height > 0 {
            let actions_area = Rect {
                x: actions_area.x.saturating_add(2),
                y: actions_area.y,
                width: actions_area.width.saturating_sub(2),
                height: actions_area.height,
            };
            let action_rows = self.action_rows();
            let action_state = self.action_state();
            render_rows(
                actions_area,
                buf,
                &action_rows,
                &action_state,
                action_rows.len().max(1),
                "No actions",
            );
        }

        if hint_area.height > 0 {
            let hint_area = Rect {
                x: hint_area.x.saturating_add(2),
                y: hint_area.y,
                width: hint_area.width.saturating_sub(2),
                height: hint_area.height,
            };
            self.hint_line().dim().render(hint_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use crate::render::renderable::Renderable;
    use insta::assert_snapshot;
    use tokio::sync::mpsc::unbounded_channel;

    fn suggestion_target() -> PluginSuggestionElicitationTarget {
        PluginSuggestionElicitationTarget {
            thread_id: ThreadId::try_from("00000000-0000-0000-0000-000000000001")
                .expect("valid thread id"),
            server_name: "codex_apps".to_string(),
            request_id: McpRequestId::String("request-1".to_string()),
        }
    }

    fn render_snapshot(view: &PluginSuggestionView, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| {
                        let symbol = buf[(x, y)].symbol();
                        if symbol.is_empty() {
                            ' '
                        } else {
                            symbol.chars().next().unwrap_or(' ')
                        }
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn marketplace_path() -> AbsolutePathBuf {
        AbsolutePathBuf::try_from("/tmp/marketplaces/openai-curated").expect("absolute path")
    }

    #[test]
    fn install_plugin_suggestion_sends_install_event_and_accepts() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut view = PluginSuggestionView::new(
            PluginSuggestionViewParams {
                plugin_id: "gmail@openai-curated".to_string(),
                title: "Gmail Plugin".to_string(),
                plugin_name: "gmail".to_string(),
                marketplace_path: marketplace_path(),
                suggest_reason: "Search your inbox directly from Codex".to_string(),
                suggest_type: ToolSuggestionType::Install,
                elicitation_target: suggestion_target(),
            },
            tx,
        );

        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match rx.try_recv() {
            Ok(AppEvent::InstallSuggestedPlugin {
                marketplace_path: installed_marketplace_path,
                plugin_name,
            }) => {
                assert_eq!(installed_marketplace_path, marketplace_path());
                assert_eq!(plugin_name, "gmail");
            }
            Ok(other) => panic!("unexpected app event: {other:?}"),
            Err(err) => panic!("missing app event: {err}"),
        }
        match rx.try_recv() {
            Ok(AppEvent::SubmitThreadOp { thread_id, op }) => {
                assert_eq!(thread_id, suggestion_target().thread_id);
                assert_eq!(
                    op,
                    Op::ResolveElicitation {
                        server_name: "codex_apps".to_string(),
                        request_id: McpRequestId::String("request-1".to_string()),
                        decision: ElicitationAction::Accept,
                        content: None,
                        meta: None,
                    }
                );
            }
            Ok(other) => panic!("unexpected app event: {other:?}"),
            Err(err) => panic!("missing app event: {err}"),
        }
        assert!(view.is_complete());
    }

    #[test]
    fn enable_plugin_suggestion_sends_enable_event_and_accepts() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut view = PluginSuggestionView::new(
            PluginSuggestionViewParams {
                plugin_id: "gmail@openai-curated".to_string(),
                title: "Gmail Plugin".to_string(),
                plugin_name: "gmail".to_string(),
                marketplace_path: marketplace_path(),
                suggest_reason: "Search your inbox directly from Codex".to_string(),
                suggest_type: ToolSuggestionType::Enable,
                elicitation_target: suggestion_target(),
            },
            tx,
        );

        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match rx.try_recv() {
            Ok(AppEvent::EnableSuggestedPlugin { plugin_id }) => {
                assert_eq!(plugin_id, "gmail@openai-curated");
            }
            Ok(other) => panic!("unexpected app event: {other:?}"),
            Err(err) => panic!("missing app event: {err}"),
        }
        match rx.try_recv() {
            Ok(AppEvent::SubmitThreadOp { thread_id, op }) => {
                assert_eq!(thread_id, suggestion_target().thread_id);
                assert_eq!(
                    op,
                    Op::ResolveElicitation {
                        server_name: "codex_apps".to_string(),
                        request_id: McpRequestId::String("request-1".to_string()),
                        decision: ElicitationAction::Accept,
                        content: None,
                        meta: None,
                    }
                );
            }
            Ok(other) => panic!("unexpected app event: {other:?}"),
            Err(err) => panic!("missing app event: {err}"),
        }
        assert!(view.is_complete());
    }

    #[test]
    fn declined_plugin_suggestion_resolves_decline() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut view = PluginSuggestionView::new(
            PluginSuggestionViewParams {
                plugin_id: "gmail@openai-curated".to_string(),
                title: "Gmail Plugin".to_string(),
                plugin_name: "gmail".to_string(),
                marketplace_path: marketplace_path(),
                suggest_reason: "Search your inbox directly from Codex".to_string(),
                suggest_type: ToolSuggestionType::Install,
                elicitation_target: suggestion_target(),
            },
            tx,
        );

        view.handle_key_event(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));

        match rx.try_recv() {
            Ok(AppEvent::SubmitThreadOp { thread_id, op }) => {
                assert_eq!(thread_id, suggestion_target().thread_id);
                assert_eq!(
                    op,
                    Op::ResolveElicitation {
                        server_name: "codex_apps".to_string(),
                        request_id: McpRequestId::String("request-1".to_string()),
                        decision: ElicitationAction::Decline,
                        content: None,
                        meta: None,
                    }
                );
            }
            Ok(other) => panic!("unexpected app event: {other:?}"),
            Err(err) => panic!("missing app event: {err}"),
        }
        assert!(view.is_complete());
    }

    #[test]
    fn install_plugin_suggestion_snapshot() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let view = PluginSuggestionView::new(
            PluginSuggestionViewParams {
                plugin_id: "gmail@openai-curated".to_string(),
                title: "Gmail Plugin".to_string(),
                plugin_name: "gmail".to_string(),
                marketplace_path: marketplace_path(),
                suggest_reason: "Search your inbox directly from Codex".to_string(),
                suggest_type: ToolSuggestionType::Install,
                elicitation_target: suggestion_target(),
            },
            tx,
        );

        assert_snapshot!(
            "plugin_suggestion_view_install",
            render_snapshot(&view, Rect::new(0, 0, 72, view.desired_height(72)))
        );
    }

    #[test]
    fn enable_plugin_suggestion_snapshot() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let view = PluginSuggestionView::new(
            PluginSuggestionViewParams {
                plugin_id: "gmail@openai-curated".to_string(),
                title: "Gmail Plugin".to_string(),
                plugin_name: "gmail".to_string(),
                marketplace_path: marketplace_path(),
                suggest_reason: "Search your inbox directly from Codex".to_string(),
                suggest_type: ToolSuggestionType::Enable,
                elicitation_target: suggestion_target(),
            },
            tx,
        );

        assert_snapshot!(
            "plugin_suggestion_view_enable",
            render_snapshot(&view, Rect::new(0, 0, 72, view.desired_height(72)))
        );
    }
}
