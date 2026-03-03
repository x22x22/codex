use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::ChatComposer;
use crate::bottom_pane::ChatComposerConfig;
use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::bottom_pane::scroll_state::ScrollState;
use crate::bottom_pane::selection_popup_common::GenericDisplayRow;
use crate::bottom_pane::selection_popup_common::measure_rows_height;
use crate::bottom_pane::selection_popup_common::menu_surface_inset;
use crate::bottom_pane::selection_popup_common::menu_surface_padding_height;
use crate::bottom_pane::selection_popup_common::render_menu_surface;
use crate::bottom_pane::selection_popup_common::render_rows;
use crate::bottom_pane::selection_popup_common::wrap_styled_line;
use crate::diff_render::DiffSummary;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::history_cell;
use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::render::highlight::highlight_bash_to_lines;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use codex_core::features::Features;
use codex_protocol::ThreadId;
use codex_protocol::mcp::RequestId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::ElicitationAction;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::NetworkApprovalContext;
use codex_protocol::protocol::NetworkPolicyRuleAction;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use unicode_width::UnicodeWidthStr;

const PATCH_REJECT_OPTION_INDEX: usize = 2;
const PATCH_NOTES_PLACEHOLDER: &str = "Tell Codex what to do differently";
const PATCH_EMPTY_NOTES_MESSAGE: &str = "Add guidance before sending.";
const PATCH_MIN_OVERLAY_HEIGHT: u16 = 8;
const PATCH_MIN_COMPOSER_HEIGHT: u16 = 3;
const PATCH_MAX_COMPOSER_HEIGHT: u16 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchFocus {
    Options,
    Notes,
}

struct PatchOverlayState {
    focus: PatchFocus,
    options_state: ScrollState,
    composer: ChatComposer,
    notes_visible: bool,
    note_submit_attempted: bool,
}

fn line_to_owned(line: Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .into_iter()
            .map(|span| Span {
                style: span.style,
                content: Cow::Owned(span.content.into_owned()),
            })
            .collect(),
    }
}

/// Request coming from the agent that needs user approval.
#[derive(Clone, Debug)]
pub(crate) enum ApprovalRequest {
    Exec {
        thread_id: ThreadId,
        thread_label: Option<String>,
        id: String,
        command: Vec<String>,
        reason: Option<String>,
        available_decisions: Vec<ReviewDecision>,
        network_approval_context: Option<NetworkApprovalContext>,
        additional_permissions: Option<PermissionProfile>,
    },
    ApplyPatch {
        thread_id: ThreadId,
        thread_label: Option<String>,
        id: String,
        reason: Option<String>,
        cwd: PathBuf,
        changes: HashMap<PathBuf, FileChange>,
    },
    McpElicitation {
        thread_id: ThreadId,
        thread_label: Option<String>,
        server_name: String,
        request_id: RequestId,
        message: String,
    },
}

impl ApprovalRequest {
    fn thread_id(&self) -> ThreadId {
        match self {
            ApprovalRequest::Exec { thread_id, .. }
            | ApprovalRequest::ApplyPatch { thread_id, .. }
            | ApprovalRequest::McpElicitation { thread_id, .. } => *thread_id,
        }
    }

    fn thread_label(&self) -> Option<&str> {
        match self {
            ApprovalRequest::Exec { thread_label, .. }
            | ApprovalRequest::ApplyPatch { thread_label, .. }
            | ApprovalRequest::McpElicitation { thread_label, .. } => thread_label.as_deref(),
        }
    }
}

/// Modal overlay asking the user to approve or deny one or more requests.
pub(crate) struct ApprovalOverlay {
    current_request: Option<ApprovalRequest>,
    queue: Vec<ApprovalRequest>,
    app_event_tx: AppEventSender,
    list: ListSelectionView,
    options: Vec<ApprovalOption>,
    patch_state: Option<PatchOverlayState>,
    current_complete: bool,
    done: bool,
    features: Features,
}

impl ApprovalOverlay {
    pub fn new(request: ApprovalRequest, app_event_tx: AppEventSender, features: Features) -> Self {
        let mut view = Self {
            current_request: None,
            queue: Vec::new(),
            app_event_tx: app_event_tx.clone(),
            list: ListSelectionView::new(Default::default(), app_event_tx),
            options: Vec::new(),
            patch_state: None,
            current_complete: false,
            done: false,
            features,
        };
        view.set_current(request);
        view
    }

    pub fn enqueue_request(&mut self, req: ApprovalRequest) {
        self.queue.push(req);
    }

    fn set_current(&mut self, request: ApprovalRequest) {
        self.current_complete = false;
        let header = build_header(&request);
        let (options, params) = Self::build_options(&request, header, &self.features);
        self.patch_state =
            matches!(request, ApprovalRequest::ApplyPatch { .. }).then(|| self.new_patch_state());
        self.current_request = Some(request);
        self.options = options;
        self.list = ListSelectionView::new(params, self.app_event_tx.clone());
    }

    fn new_patch_state(&self) -> PatchOverlayState {
        let mut composer = ChatComposer::new_with_config(
            true,
            self.app_event_tx.clone(),
            false,
            PATCH_NOTES_PLACEHOLDER.to_string(),
            false,
            ChatComposerConfig::plain_text(),
        );
        composer.set_footer_hint_override(Some(Vec::new()));
        PatchOverlayState {
            focus: PatchFocus::Options,
            options_state: ScrollState {
                selected_idx: Some(0),
                ..Default::default()
            },
            composer,
            notes_visible: false,
            note_submit_attempted: false,
        }
    }

    fn build_options(
        request: &ApprovalRequest,
        header: Box<dyn Renderable>,
        _features: &Features,
    ) -> (Vec<ApprovalOption>, SelectionViewParams) {
        let (options, title) = match request {
            ApprovalRequest::Exec {
                available_decisions,
                network_approval_context,
                additional_permissions,
                ..
            } => (
                exec_options(
                    available_decisions,
                    network_approval_context.as_ref(),
                    additional_permissions.as_ref(),
                ),
                network_approval_context.as_ref().map_or_else(
                    || "Would you like to run the following command?".to_string(),
                    |network_approval_context| {
                        format!(
                            "Do you want to approve network access to \"{}\"?",
                            network_approval_context.host
                        )
                    },
                ),
            ),
            ApprovalRequest::ApplyPatch { .. } => (
                patch_options(),
                "Would you like to make the following edits?".to_string(),
            ),
            ApprovalRequest::McpElicitation { server_name, .. } => (
                elicitation_options(),
                format!("{server_name} needs your approval."),
            ),
        };

        let header = Box::new(ColumnRenderable::with([
            Line::from(title.bold()).into(),
            Line::from("").into(),
            header,
        ]));

        let items = options
            .iter()
            .map(|opt| SelectionItem {
                name: opt.label.clone(),
                display_shortcut: opt
                    .display_shortcut
                    .or_else(|| opt.additional_shortcuts.first().copied()),
                dismiss_on_select: false,
                ..Default::default()
            })
            .collect();

        let params = SelectionViewParams {
            footer_hint: Some(approval_footer_hint(request)),
            items,
            header,
            ..Default::default()
        };

        (options, params)
    }

    fn apply_selection(&mut self, actual_idx: usize) {
        if self.current_complete {
            return;
        }
        let Some(option) = self.options.get(actual_idx).cloned() else {
            return;
        };
        if matches!(option.decision, ApprovalDecision::PatchRejectWithNotes) {
            self.open_patch_notes();
            return;
        }
        if let Some(request) = self.current_request.as_ref() {
            match (request, &option.decision) {
                (ApprovalRequest::Exec { id, command, .. }, ApprovalDecision::Review(decision)) => {
                    self.handle_exec_decision(id, command, decision.clone());
                }
                (ApprovalRequest::ApplyPatch { id, .. }, ApprovalDecision::Review(decision)) => {
                    self.handle_patch_decision(id, decision.clone());
                }
                (
                    ApprovalRequest::McpElicitation {
                        server_name,
                        request_id,
                        ..
                    },
                    ApprovalDecision::McpElicitation(decision),
                ) => {
                    self.handle_elicitation_decision(server_name, request_id, *decision);
                }
                _ => {}
            }
        }

        self.current_complete = true;
        self.advance_queue();
    }

    fn patch_state(&self) -> Option<&PatchOverlayState> {
        self.patch_state.as_ref()
    }

    fn patch_state_mut(&mut self) -> Option<&mut PatchOverlayState> {
        self.patch_state.as_mut()
    }

    fn patch_selected_index(&self) -> Option<usize> {
        self.patch_state()
            .and_then(|state| state.options_state.selected_idx)
    }

    fn patch_focus_is_notes(&self) -> bool {
        self.patch_state()
            .is_some_and(|state| matches!(state.focus, PatchFocus::Notes))
    }

    fn patch_note_text(&self) -> String {
        self.patch_state()
            .map(|state| state.composer.current_text_with_pending())
            .unwrap_or_default()
    }

    fn patch_notes_visible(&self) -> bool {
        let Some(state) = self.patch_state() else {
            return false;
        };
        state.options_state.selected_idx == Some(PATCH_REJECT_OPTION_INDEX)
            && (state.notes_visible
                || !state.composer.current_text_with_pending().trim().is_empty())
    }

    fn patch_note_error_visible(&self) -> bool {
        self.patch_notes_visible()
            && self
                .patch_state()
                .is_some_and(|state| state.note_submit_attempted)
            && self.patch_note_text().trim().is_empty()
    }

    fn patch_title_lines(&self, width: u16) -> Vec<Line<'static>> {
        let line = Line::from("Would you like to make the following edits?".bold());
        wrap_styled_line(&line, width.max(1))
            .into_iter()
            .map(line_to_owned)
            .collect()
    }

    fn patch_option_rows(&self) -> Vec<GenericDisplayRow> {
        let selected_idx = self.patch_selected_index();
        self.options
            .iter()
            .enumerate()
            .map(|(idx, option)| {
                let prefix = if selected_idx == Some(idx) {
                    '›'
                } else {
                    ' '
                };
                let prefix_label = format!("{prefix} {}. ", idx + 1);
                GenericDisplayRow {
                    name: format!("{prefix_label}{}", option.label),
                    display_shortcut: option
                        .display_shortcut
                        .or_else(|| option.additional_shortcuts.first().copied()),
                    wrap_indent: Some(UnicodeWidthStr::width(prefix_label.as_str())),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn patch_hint_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut hint = if self.patch_notes_visible() {
            if self.patch_focus_is_notes() {
                "enter to send | tab to go back | esc to interrupt".to_string()
            } else {
                "enter or tab to edit follow up | esc to interrupt".to_string()
            }
        } else if self.patch_selected_index() == Some(PATCH_REJECT_OPTION_INDEX) {
            "tab to follow up | esc to interrupt".to_string()
        } else {
            "esc to interrupt".to_string()
        };
        if self
            .current_request
            .as_ref()
            .and_then(ApprovalRequest::thread_label)
            .is_some()
        {
            hint.push_str(" | o to open thread");
        }
        let line = Line::from(hint).dim();
        wrap_styled_line(&line, width.max(1))
            .into_iter()
            .map(line_to_owned)
            .collect()
    }

    fn patch_validation_lines(&self, width: u16) -> Vec<Line<'static>> {
        if !self.patch_note_error_visible() {
            return Vec::new();
        }
        let line = Line::from(PATCH_EMPTY_NOTES_MESSAGE).red();
        wrap_styled_line(&line, width.max(1))
            .into_iter()
            .map(line_to_owned)
            .collect()
    }

    fn patch_notes_input_height(&self, width: u16) -> u16 {
        self.patch_state()
            .map(|state| {
                state
                    .composer
                    .desired_height(width.max(1))
                    .clamp(PATCH_MIN_COMPOSER_HEIGHT, PATCH_MAX_COMPOSER_HEIGHT)
            })
            .unwrap_or(PATCH_MIN_COMPOSER_HEIGHT)
    }

    fn patch_select_index(&mut self, idx: usize) {
        let len = self.options.len();
        let idx = idx.min(len.saturating_sub(1));
        if let Some(state) = self.patch_state_mut() {
            state.options_state.selected_idx = Some(idx);
            if idx != PATCH_REJECT_OPTION_INDEX {
                state.notes_visible = false;
            }
            state.note_submit_attempted = false;
        }
    }

    fn open_patch_notes(&mut self) {
        if self.options.len() <= PATCH_REJECT_OPTION_INDEX {
            return;
        }
        self.patch_select_index(PATCH_REJECT_OPTION_INDEX);
        if let Some(state) = self.patch_state_mut() {
            state.notes_visible = true;
            state.focus = PatchFocus::Notes;
            state.note_submit_attempted = false;
            state.composer.move_cursor_to_end();
        }
    }

    fn move_patch_selection(&mut self, move_down: bool) {
        let options_len = self.options.len();
        if let Some(state) = self.patch_state_mut() {
            if move_down {
                state.options_state.move_down_wrap(options_len);
            } else {
                state.options_state.move_up_wrap(options_len);
            }
            if state.options_state.selected_idx != Some(PATCH_REJECT_OPTION_INDEX) {
                state.notes_visible = false;
            }
            state.note_submit_attempted = false;
        }
    }

    fn handle_exec_decision(&self, id: &str, command: &[String], decision: ReviewDecision) {
        let Some(request) = self.current_request.as_ref() else {
            return;
        };
        if request.thread_label().is_none() {
            let cell = history_cell::new_approval_decision_cell(command.to_vec(), decision.clone());
            self.app_event_tx.send(AppEvent::InsertHistoryCell(cell));
        }
        let thread_id = request.thread_id();
        self.app_event_tx.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: Op::ExecApproval {
                id: id.to_string(),
                turn_id: None,
                decision,
            },
        });
    }

    fn handle_patch_decision(&self, id: &str, decision: ReviewDecision) {
        let Some(thread_id) = self
            .current_request
            .as_ref()
            .map(ApprovalRequest::thread_id)
        else {
            return;
        };
        self.app_event_tx.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: Op::PatchApproval {
                id: id.to_string(),
                decision,
            },
        });
    }

    fn handle_elicitation_decision(
        &self,
        server_name: &str,
        request_id: &RequestId,
        decision: ElicitationAction,
    ) {
        let Some(thread_id) = self
            .current_request
            .as_ref()
            .map(ApprovalRequest::thread_id)
        else {
            return;
        };
        self.app_event_tx.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: Op::ResolveElicitation {
                server_name: server_name.to_string(),
                request_id: request_id.clone(),
                decision,
            },
        });
    }

    fn advance_queue(&mut self) {
        if let Some(next) = self.queue.pop() {
            self.set_current(next);
        } else {
            self.done = true;
        }
    }

    fn try_handle_global_shortcut(&mut self, key_event: &KeyEvent) -> bool {
        match key_event {
            KeyEvent {
                kind: KeyEventKind::Press,
                code: KeyCode::Char('a'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(request) = self.current_request.as_ref() {
                    self.app_event_tx
                        .send(AppEvent::FullScreenApprovalRequest(request.clone()));
                    true
                } else {
                    false
                }
            }
            KeyEvent {
                kind: KeyEventKind::Press,
                code: KeyCode::Char('o'),
                ..
            } => {
                if let Some(request) = self.current_request.as_ref() {
                    if request.thread_label().is_some() {
                        self.app_event_tx
                            .send(AppEvent::SelectAgentThread(request.thread_id()));
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn try_handle_option_shortcut(&mut self, key_event: &KeyEvent) -> bool {
        self.options
            .iter()
            .position(|opt| {
                opt.shortcuts()
                    .any(|shortcut| shortcut.is_press(*key_event))
            })
            .map(|idx| {
                self.apply_selection(idx);
            })
            .is_some()
    }

    fn handle_patch_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Release {
            return;
        }
        if self.done || self.current_complete {
            return;
        }

        if self.try_handle_global_shortcut(&key_event) {
            return;
        }

        match self.patch_state().map(|state| state.focus) {
            Some(PatchFocus::Options) => match key_event {
                KeyEvent {
                    code: KeyCode::Char('y'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.apply_selection(0),
                KeyEvent {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.apply_selection(1),
                KeyEvent {
                    code: KeyCode::Char('n'),
                    modifiers: KeyModifiers::NONE,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Tab,
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.open_patch_notes(),
                KeyEvent {
                    code: KeyCode::Char('1'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.apply_selection(0),
                KeyEvent {
                    code: KeyCode::Char('2'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.apply_selection(1),
                KeyEvent {
                    code: KeyCode::Char('3'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.open_patch_notes(),
                KeyEvent {
                    code: KeyCode::Up | KeyCode::Char('k'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.move_patch_selection(false),
                KeyEvent {
                    code: KeyCode::Down | KeyCode::Char('j'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.move_patch_selection(true),
                KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                    ..
                } => match self.patch_selected_index() {
                    Some(PATCH_REJECT_OPTION_INDEX) => self.open_patch_notes(),
                    Some(idx) => self.apply_selection(idx),
                    None => {}
                },
                _ => {}
            },
            Some(PatchFocus::Notes) => {
                if matches!(
                    key_event,
                    KeyEvent {
                        code: KeyCode::Tab,
                        modifiers: KeyModifiers::NONE,
                        ..
                    }
                ) {
                    if let Some(state) = self.patch_state_mut() {
                        state.focus = PatchFocus::Options;
                        state.note_submit_attempted = false;
                    }
                    return;
                }
                if matches!(
                    key_event,
                    KeyEvent {
                        code: KeyCode::Enter,
                        modifiers: KeyModifiers::NONE,
                        ..
                    }
                ) {
                    let text = self.patch_note_text();
                    if text.trim().is_empty() {
                        if let Some(state) = self.patch_state_mut() {
                            state.note_submit_attempted = true;
                        }
                        return;
                    }
                    if let Some(ApprovalRequest::ApplyPatch { thread_id, id, .. }) =
                        self.current_request.as_ref()
                    {
                        self.app_event_tx
                            .send(AppEvent::RejectPatchApprovalWithNotes {
                                thread_id: *thread_id,
                                approval_id: id.clone(),
                                text,
                            });
                        self.current_complete = true;
                        self.advance_queue();
                    }
                    return;
                }
                if let Some(state) = self.patch_state_mut() {
                    let _ = state.composer.handle_key_event(key_event);
                    if !state.composer.current_text_with_pending().trim().is_empty() {
                        state.note_submit_attempted = false;
                    }
                }
            }
            None => {}
        }
    }
}

impl BottomPaneView for ApprovalOverlay {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if matches!(
            self.current_request,
            Some(ApprovalRequest::ApplyPatch { .. })
        ) {
            self.handle_patch_key_event(key_event);
            return;
        }
        if self.try_handle_global_shortcut(&key_event)
            || self.try_handle_option_shortcut(&key_event)
        {
            return;
        }
        self.list.handle_key_event(key_event);
        if let Some(idx) = self.list.take_last_selected_index() {
            self.apply_selection(idx);
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        if self.done {
            return CancellationEvent::Handled;
        }
        if !self.current_complete
            && let Some(request) = self.current_request.as_ref()
        {
            match request {
                ApprovalRequest::Exec { id, command, .. } => {
                    self.handle_exec_decision(id, command, ReviewDecision::Abort);
                }
                ApprovalRequest::ApplyPatch { id, .. } => {
                    self.handle_patch_decision(id, ReviewDecision::Abort);
                }
                ApprovalRequest::McpElicitation {
                    server_name,
                    request_id,
                    ..
                } => {
                    self.handle_elicitation_decision(
                        server_name,
                        request_id,
                        ElicitationAction::Cancel,
                    );
                }
            }
        }
        self.queue.clear();
        self.done = true;
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.done
    }

    fn try_consume_approval_request(
        &mut self,
        request: ApprovalRequest,
    ) -> Option<ApprovalRequest> {
        self.enqueue_request(request);
        None
    }

    fn handle_paste(&mut self, pasted: String) -> bool {
        if pasted.is_empty()
            || !matches!(
                self.current_request,
                Some(ApprovalRequest::ApplyPatch { .. })
            )
        {
            return false;
        }
        self.open_patch_notes();
        self.patch_state_mut()
            .map(|state| {
                state.note_submit_attempted = false;
                state.composer.handle_paste(pasted)
            })
            .unwrap_or(false)
    }

    fn flush_paste_burst_if_due(&mut self) -> bool {
        self.patch_state_mut()
            .map(|state| state.composer.flush_paste_burst_if_due())
            .unwrap_or(false)
    }

    fn is_in_paste_burst(&self) -> bool {
        self.patch_state()
            .is_some_and(|state| state.composer.is_in_paste_burst())
    }
}

impl Renderable for ApprovalOverlay {
    fn desired_height(&self, width: u16) -> u16 {
        let Some(request @ ApprovalRequest::ApplyPatch { .. }) = self.current_request.as_ref()
        else {
            return self.list.desired_height(width);
        };

        let outer = Rect::new(0, 0, width, u16::MAX);
        let inner = menu_surface_inset(outer);
        let inner_width = inner.width.max(1);
        let title_height = self.patch_title_lines(inner_width).len() as u16;
        let header = build_header(request);
        let header_height = header.desired_height(inner_width);
        let rows = self.patch_option_rows();
        let mut state = self
            .patch_state()
            .map(|patch_state| patch_state.options_state)
            .unwrap_or_default();
        if state.selected_idx.is_none() {
            state.selected_idx = Some(0);
        }
        let options_height =
            measure_rows_height(&rows, &state, rows.len().max(1), inner_width.max(1));
        let hint_height = self.patch_hint_lines(inner_width).len() as u16;
        let notes_height = if self.patch_notes_visible() {
            self.patch_notes_input_height(inner_width)
        } else {
            0
        };
        let validation_height = self.patch_validation_lines(inner_width).len() as u16;
        let height = title_height
            + 1
            + header_height
            + 1
            + options_height
            + hint_height
            + notes_height
            + validation_height
            + menu_surface_padding_height();
        height.max(PATCH_MIN_OVERLAY_HEIGHT)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let Some(request @ ApprovalRequest::ApplyPatch { .. }) = self.current_request.as_ref()
        else {
            self.list.render(area, buf);
            return;
        };
        let content_area = render_menu_surface(area, buf);
        if content_area.width == 0 || content_area.height == 0 {
            return;
        }

        let width = content_area.width.max(1);
        let title_lines = self.patch_title_lines(width);
        let header = build_header(request);
        let header_height = header.desired_height(width);
        let rows = self.patch_option_rows();
        let mut state = self
            .patch_state()
            .map(|patch_state| patch_state.options_state)
            .unwrap_or_default();
        if state.selected_idx.is_none() {
            state.selected_idx = Some(0);
        }
        let options_height = measure_rows_height(&rows, &state, rows.len().max(1), width.max(1));
        let hint_lines = self.patch_hint_lines(width);
        let validation_lines = self.patch_validation_lines(width);
        let validation_height = validation_lines.len() as u16;
        let notes_height = if self.patch_notes_visible() {
            self.patch_notes_input_height(width)
        } else {
            0
        };

        let mut cursor_y = content_area.y;
        for line in title_lines {
            Paragraph::new(line).render(
                Rect {
                    x: content_area.x,
                    y: cursor_y,
                    width: content_area.width,
                    height: 1,
                },
                buf,
            );
            cursor_y = cursor_y.saturating_add(1);
        }
        cursor_y = cursor_y.saturating_add(1);

        header.render(
            Rect {
                x: content_area.x,
                y: cursor_y,
                width: content_area.width,
                height: header_height.min(
                    content_area
                        .height
                        .saturating_sub(cursor_y - content_area.y),
                ),
            },
            buf,
        );
        cursor_y = cursor_y.saturating_add(header_height).saturating_add(1);

        let options_area = Rect {
            x: content_area.x,
            y: cursor_y,
            width: content_area.width,
            height: options_height,
        };
        render_rows(
            options_area,
            buf,
            &rows,
            &state,
            rows.len().max(1),
            "No options",
        );
        cursor_y = cursor_y.saturating_add(options_height);

        for line in hint_lines {
            Paragraph::new(line).render(
                Rect {
                    x: content_area.x,
                    y: cursor_y,
                    width: content_area.width,
                    height: 1,
                },
                buf,
            );
            cursor_y = cursor_y.saturating_add(1);
        }

        if self.patch_notes_visible() {
            let notes_area =
                Rect {
                    x: content_area.x,
                    y: cursor_y,
                    width: content_area.width,
                    height: notes_height.min(content_area.height.saturating_sub(
                        cursor_y.saturating_sub(content_area.y) + validation_height,
                    )),
                };
            if let Some(state) = self.patch_state() {
                state.composer.render(notes_area, buf);
            }
            cursor_y = cursor_y.saturating_add(notes_area.height);
        }

        for line in validation_lines {
            Paragraph::new(line).render(
                Rect {
                    x: content_area.x,
                    y: cursor_y,
                    width: content_area.width,
                    height: 1,
                },
                buf,
            );
            cursor_y = cursor_y.saturating_add(1);
        }
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.patch_focus_is_notes() || !self.patch_notes_visible() {
            return self.list.cursor_pos(area);
        }
        let content_area = menu_surface_inset(area);
        if content_area.width == 0 || content_area.height == 0 {
            return None;
        }
        let width = content_area.width.max(1);
        let title_height = self.patch_title_lines(width).len() as u16;
        let header_height = self
            .current_request
            .as_ref()
            .map(build_header)
            .map(|header| header.desired_height(width))
            .unwrap_or(0);
        let rows = self.patch_option_rows();
        let mut state = self
            .patch_state()
            .map(|patch_state| patch_state.options_state)
            .unwrap_or_default();
        if state.selected_idx.is_none() {
            state.selected_idx = Some(0);
        }
        let options_height = measure_rows_height(&rows, &state, rows.len().max(1), width.max(1));
        let hint_height = self.patch_hint_lines(width).len() as u16;
        let notes_area = Rect {
            x: content_area.x,
            y: content_area.y + title_height + 1 + header_height + 1 + options_height + hint_height,
            width: content_area.width,
            height: self.patch_notes_input_height(width),
        };
        self.patch_state()
            .and_then(|state| state.composer.cursor_pos(notes_area))
    }
}

fn approval_footer_hint(request: &ApprovalRequest) -> Line<'static> {
    let mut spans = vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Enter).into(),
        " to confirm or ".into(),
        key_hint::plain(KeyCode::Esc).into(),
        " to cancel".into(),
    ];
    if request.thread_label().is_some() {
        spans.extend([
            " or ".into(),
            key_hint::plain(KeyCode::Char('o')).into(),
            " to open thread".into(),
        ]);
    }
    Line::from(spans)
}

fn build_header(request: &ApprovalRequest) -> Box<dyn Renderable> {
    match request {
        ApprovalRequest::Exec {
            thread_label,
            reason,
            command,
            network_approval_context,
            additional_permissions,
            ..
        } => {
            let mut header: Vec<Line<'static>> = Vec::new();
            if let Some(thread_label) = thread_label {
                header.push(Line::from(vec![
                    "Thread: ".into(),
                    thread_label.clone().bold(),
                ]));
                header.push(Line::from(""));
            }
            if let Some(reason) = reason {
                header.push(Line::from(vec!["Reason: ".into(), reason.clone().italic()]));
                header.push(Line::from(""));
            }
            if let Some(additional_permissions) = additional_permissions
                && let Some(rule_line) = format_additional_permissions_rule(additional_permissions)
            {
                header.push(Line::from(vec![
                    "Permission rule: ".into(),
                    rule_line.cyan(),
                ]));
                header.push(Line::from(""));
            }
            let full_cmd = strip_bash_lc_and_escape(command);
            let mut full_cmd_lines = highlight_bash_to_lines(&full_cmd);
            if let Some(first) = full_cmd_lines.first_mut() {
                first.spans.insert(0, Span::from("$ "));
            }
            if network_approval_context.is_none() {
                header.extend(full_cmd_lines);
            }
            Box::new(Paragraph::new(header).wrap(Wrap { trim: false }))
        }
        ApprovalRequest::ApplyPatch {
            thread_label,
            reason,
            cwd,
            changes,
            ..
        } => {
            let mut header: Vec<Box<dyn Renderable>> = Vec::new();
            if let Some(thread_label) = thread_label {
                header.push(Box::new(Line::from(vec![
                    "Thread: ".into(),
                    thread_label.clone().bold(),
                ])));
                header.push(Box::new(Line::from("")));
            }
            if let Some(reason) = reason
                && !reason.is_empty()
            {
                header.push(Box::new(
                    Paragraph::new(Line::from_iter([
                        "Reason: ".into(),
                        reason.clone().italic(),
                    ]))
                    .wrap(Wrap { trim: false }),
                ));
                header.push(Box::new(Line::from("")));
            }
            header.push(DiffSummary::new(changes.clone(), cwd.clone()).into());
            Box::new(ColumnRenderable::with(header))
        }
        ApprovalRequest::McpElicitation {
            thread_label,
            server_name,
            message,
            ..
        } => {
            let mut lines = Vec::new();
            if let Some(thread_label) = thread_label {
                lines.push(Line::from(vec![
                    "Thread: ".into(),
                    thread_label.clone().bold(),
                ]));
                lines.push(Line::from(""));
            }
            lines.extend([
                Line::from(vec!["Server: ".into(), server_name.clone().bold()]),
                Line::from(""),
                Line::from(message.clone()),
            ]);
            let header = Paragraph::new(lines).wrap(Wrap { trim: false });
            Box::new(header)
        }
    }
}

#[derive(Clone)]
enum ApprovalDecision {
    Review(ReviewDecision),
    McpElicitation(ElicitationAction),
    PatchRejectWithNotes,
}

#[derive(Clone)]
struct ApprovalOption {
    label: String,
    decision: ApprovalDecision,
    display_shortcut: Option<KeyBinding>,
    additional_shortcuts: Vec<KeyBinding>,
}

impl ApprovalOption {
    fn shortcuts(&self) -> impl Iterator<Item = KeyBinding> + '_ {
        self.display_shortcut
            .into_iter()
            .chain(self.additional_shortcuts.iter().copied())
    }
}

fn exec_options(
    available_decisions: &[ReviewDecision],
    network_approval_context: Option<&NetworkApprovalContext>,
    additional_permissions: Option<&PermissionProfile>,
) -> Vec<ApprovalOption> {
    available_decisions
        .iter()
        .filter_map(|decision| match decision {
            ReviewDecision::Approved => Some(ApprovalOption {
                label: if network_approval_context.is_some() {
                    "Yes, just this once".to_string()
                } else {
                    "Yes, proceed".to_string()
                },
                decision: ApprovalDecision::Review(ReviewDecision::Approved),
                display_shortcut: None,
                additional_shortcuts: vec![key_hint::plain(KeyCode::Char('y'))],
            }),
            ReviewDecision::ApprovedExecpolicyAmendment {
                proposed_execpolicy_amendment,
            } => {
                let rendered_prefix =
                    strip_bash_lc_and_escape(proposed_execpolicy_amendment.command());
                if rendered_prefix.contains('\n') || rendered_prefix.contains('\r') {
                    return None;
                }

                Some(ApprovalOption {
                    label: format!(
                        "Yes, and don't ask again for commands that start with `{rendered_prefix}`"
                    ),
                    decision: ApprovalDecision::Review(
                        ReviewDecision::ApprovedExecpolicyAmendment {
                            proposed_execpolicy_amendment: proposed_execpolicy_amendment.clone(),
                        },
                    ),
                    display_shortcut: None,
                    additional_shortcuts: vec![key_hint::plain(KeyCode::Char('p'))],
                })
            }
            ReviewDecision::ApprovedForSession => Some(ApprovalOption {
                label: if network_approval_context.is_some() {
                    "Yes, and allow this host for this conversation".to_string()
                } else if additional_permissions.is_some() {
                    "Yes, and allow these permissions for this session".to_string()
                } else {
                    "Yes, and don't ask again for this command in this session".to_string()
                },
                decision: ApprovalDecision::Review(ReviewDecision::ApprovedForSession),
                display_shortcut: None,
                additional_shortcuts: vec![key_hint::plain(KeyCode::Char('a'))],
            }),
            ReviewDecision::NetworkPolicyAmendment {
                network_policy_amendment,
            } => {
                let (label, shortcut) = match network_policy_amendment.action {
                    NetworkPolicyRuleAction::Allow => (
                        "Yes, and allow this host in the future".to_string(),
                        KeyCode::Char('p'),
                    ),
                    NetworkPolicyRuleAction::Deny => (
                        "No, and block this host in the future".to_string(),
                        KeyCode::Char('d'),
                    ),
                };
                Some(ApprovalOption {
                    label,
                    decision: ApprovalDecision::Review(ReviewDecision::NetworkPolicyAmendment {
                        network_policy_amendment: network_policy_amendment.clone(),
                    }),
                    display_shortcut: None,
                    additional_shortcuts: vec![key_hint::plain(shortcut)],
                })
            }
            ReviewDecision::Denied => Some(ApprovalOption {
                label: "No, continue without running it".to_string(),
                decision: ApprovalDecision::Review(ReviewDecision::Denied),
                display_shortcut: None,
                additional_shortcuts: vec![key_hint::plain(KeyCode::Char('d'))],
            }),
            ReviewDecision::Abort => Some(ApprovalOption {
                label: "No, and tell Codex what to do differently".to_string(),
                decision: ApprovalDecision::Review(ReviewDecision::Abort),
                display_shortcut: Some(key_hint::plain(KeyCode::Esc)),
                additional_shortcuts: vec![key_hint::plain(KeyCode::Char('n'))],
            }),
        })
        .collect()
}

fn format_additional_permissions_rule(
    additional_permissions: &PermissionProfile,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(file_system) = additional_permissions.file_system.as_ref() {
        if let Some(read) = file_system.read.as_ref() {
            let reads = read
                .iter()
                .map(|path| format!("`{}`", path.display()))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("read {reads}"));
        }
        if let Some(write) = file_system.write.as_ref() {
            let writes = write
                .iter()
                .map(|path| format!("`{}`", path.display()))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("write {writes}"));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn patch_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Yes, proceed".to_string(),
            decision: ApprovalDecision::Review(ReviewDecision::Approved),
            display_shortcut: None,
            additional_shortcuts: vec![key_hint::plain(KeyCode::Char('y'))],
        },
        ApprovalOption {
            label: "Yes, and don't ask again for these files".to_string(),
            decision: ApprovalDecision::Review(ReviewDecision::ApprovedForSession),
            display_shortcut: None,
            additional_shortcuts: vec![key_hint::plain(KeyCode::Char('a'))],
        },
        ApprovalOption {
            label: "No, and tell Codex what to do differently".to_string(),
            decision: ApprovalDecision::PatchRejectWithNotes,
            display_shortcut: Some(key_hint::plain(KeyCode::Tab)),
            additional_shortcuts: vec![key_hint::plain(KeyCode::Char('n'))],
        },
    ]
}

fn elicitation_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Yes, provide the requested info".to_string(),
            decision: ApprovalDecision::McpElicitation(ElicitationAction::Accept),
            display_shortcut: None,
            additional_shortcuts: vec![key_hint::plain(KeyCode::Char('y'))],
        },
        ApprovalOption {
            label: "No, but continue without it".to_string(),
            decision: ApprovalDecision::McpElicitation(ElicitationAction::Decline),
            display_shortcut: None,
            additional_shortcuts: vec![key_hint::plain(KeyCode::Char('n'))],
        },
        ApprovalOption {
            label: "Cancel this request".to_string(),
            decision: ApprovalDecision::McpElicitation(ElicitationAction::Cancel),
            display_shortcut: Some(key_hint::plain(KeyCode::Esc)),
            additional_shortcuts: vec![key_hint::plain(KeyCode::Char('c'))],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use codex_protocol::models::FileSystemPermissions;
    use codex_protocol::protocol::ExecPolicyAmendment;
    use codex_protocol::protocol::NetworkApprovalProtocol;
    use codex_protocol::protocol::NetworkPolicyAmendment;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    fn absolute_path(path: &str) -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path(path).expect("absolute path")
    }

    fn render_overlay_lines(view: &ApprovalOverlay, width: u16) -> String {
        let height = view.desired_height(width);
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        view.render(Rect::new(0, 0, width, height), &mut buf);
        (0..buf.area.height)
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn normalize_snapshot_paths(rendered: String) -> String {
        [
            (absolute_path("/tmp/readme.txt"), "/tmp/readme.txt"),
            (absolute_path("/tmp/out.txt"), "/tmp/out.txt"),
        ]
        .into_iter()
        .fold(rendered, |rendered, (path, normalized)| {
            rendered.replace(&path.display().to_string(), normalized)
        })
    }

    fn make_exec_request() -> ApprovalRequest {
        ApprovalRequest::Exec {
            thread_id: ThreadId::new(),
            thread_label: None,
            id: "test".to_string(),
            command: vec!["echo".to_string(), "hi".to_string()],
            reason: Some("reason".to_string()),
            available_decisions: vec![ReviewDecision::Approved, ReviewDecision::Abort],
            network_approval_context: None,
            additional_permissions: None,
        }
    }

    fn make_patch_request() -> ApprovalRequest {
        let mut changes = HashMap::new();
        changes.insert(
            PathBuf::from("README.md"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        ApprovalRequest::ApplyPatch {
            thread_id: ThreadId::new(),
            thread_label: None,
            id: "patch-test".to_string(),
            reason: Some("review these edits".to_string()),
            cwd: PathBuf::from("/tmp"),
            changes,
        }
    }

    #[test]
    fn ctrl_c_aborts_and_clears_queue() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(make_exec_request(), tx, Features::with_defaults());
        view.enqueue_request(make_exec_request());
        assert_eq!(CancellationEvent::Handled, view.on_ctrl_c());
        assert!(view.queue.is_empty());
        assert!(view.is_complete());
    }

    #[test]
    fn shortcut_triggers_selection() {
        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(make_exec_request(), tx, Features::with_defaults());
        assert!(!view.is_complete());
        view.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        // We expect at least one thread-scoped approval op message in the queue.
        let mut saw_op = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, AppEvent::SubmitThreadOp { .. }) {
                saw_op = true;
                break;
            }
        }
        assert!(saw_op, "expected approval decision to emit an op");
    }

    #[test]
    fn o_opens_source_thread_for_cross_thread_approval() {
        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let thread_id = ThreadId::new();
        let mut view = ApprovalOverlay::new(
            ApprovalRequest::Exec {
                thread_id,
                thread_label: Some("Robie [explorer]".to_string()),
                id: "test".to_string(),
                command: vec!["echo".to_string(), "hi".to_string()],
                reason: None,
                available_decisions: vec![ReviewDecision::Approved, ReviewDecision::Abort],
                network_approval_context: None,
                additional_permissions: None,
            },
            tx,
            Features::with_defaults(),
        );

        view.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

        let event = rx.try_recv().expect("expected select-agent-thread event");
        assert_eq!(
            matches!(event, AppEvent::SelectAgentThread(id) if id == thread_id),
            true
        );
    }

    #[test]
    fn cross_thread_footer_hint_mentions_o_shortcut() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let view = ApprovalOverlay::new(
            ApprovalRequest::Exec {
                thread_id: ThreadId::new(),
                thread_label: Some("Robie [explorer]".to_string()),
                id: "test".to_string(),
                command: vec!["echo".to_string(), "hi".to_string()],
                reason: None,
                available_decisions: vec![ReviewDecision::Approved, ReviewDecision::Abort],
                network_approval_context: None,
                additional_permissions: None,
            },
            tx,
            Features::with_defaults(),
        );

        assert_snapshot!(
            "approval_overlay_cross_thread_prompt",
            render_overlay_lines(&view, 80)
        );
    }

    #[test]
    fn exec_prefix_option_emits_execpolicy_amendment() {
        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(
            ApprovalRequest::Exec {
                thread_id: ThreadId::new(),
                thread_label: None,
                id: "test".to_string(),
                command: vec!["echo".to_string()],
                reason: None,
                available_decisions: vec![
                    ReviewDecision::Approved,
                    ReviewDecision::ApprovedExecpolicyAmendment {
                        proposed_execpolicy_amendment: ExecPolicyAmendment::new(vec![
                            "echo".to_string(),
                        ]),
                    },
                    ReviewDecision::Abort,
                ],
                network_approval_context: None,
                additional_permissions: None,
            },
            tx,
            Features::with_defaults(),
        );
        view.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        let mut saw_op = false;
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::SubmitThreadOp {
                op: Op::ExecApproval { decision, .. },
                ..
            } = ev
            {
                assert_eq!(
                    decision,
                    ReviewDecision::ApprovedExecpolicyAmendment {
                        proposed_execpolicy_amendment: ExecPolicyAmendment::new(vec![
                            "echo".to_string()
                        ])
                    }
                );
                saw_op = true;
                break;
            }
        }
        assert!(
            saw_op,
            "expected approval decision to emit an op with command prefix"
        );
    }

    #[test]
    fn network_deny_forever_shortcut_is_not_bound() {
        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(
            ApprovalRequest::Exec {
                thread_id: ThreadId::new(),
                thread_label: None,
                id: "test".to_string(),
                command: vec!["curl".to_string(), "https://example.com".to_string()],
                reason: None,
                available_decisions: vec![
                    ReviewDecision::Approved,
                    ReviewDecision::ApprovedForSession,
                    ReviewDecision::NetworkPolicyAmendment {
                        network_policy_amendment: NetworkPolicyAmendment {
                            host: "example.com".to_string(),
                            action: NetworkPolicyRuleAction::Allow,
                        },
                    },
                    ReviewDecision::Abort,
                ],
                network_approval_context: Some(NetworkApprovalContext {
                    host: "example.com".to_string(),
                    protocol: NetworkApprovalProtocol::Https,
                }),
                additional_permissions: None,
            },
            tx,
            Features::with_defaults(),
        );
        view.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert!(
            rx.try_recv().is_err(),
            "unexpected approval event emitted for hidden network deny shortcut"
        );
    }

    #[test]
    fn header_includes_command_snippet() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let command = vec!["echo".into(), "hello".into(), "world".into()];
        let exec_request = ApprovalRequest::Exec {
            thread_id: ThreadId::new(),
            thread_label: None,
            id: "test".into(),
            command,
            reason: None,
            available_decisions: vec![ReviewDecision::Approved, ReviewDecision::Abort],
            network_approval_context: None,
            additional_permissions: None,
        };

        let view = ApprovalOverlay::new(exec_request, tx, Features::with_defaults());
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, view.desired_height(80)));
        view.render(Rect::new(0, 0, 80, view.desired_height(80)), &mut buf);

        let rendered: Vec<String> = (0..buf.area.height)
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect();
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("echo hello world")),
            "expected header to include command snippet, got {rendered:?}"
        );
    }

    #[test]
    fn network_exec_options_use_expected_labels_and_hide_execpolicy_amendment() {
        let network_context = NetworkApprovalContext {
            host: "example.com".to_string(),
            protocol: NetworkApprovalProtocol::Https,
        };
        let options = exec_options(
            &[
                ReviewDecision::Approved,
                ReviewDecision::ApprovedForSession,
                ReviewDecision::NetworkPolicyAmendment {
                    network_policy_amendment: NetworkPolicyAmendment {
                        host: "example.com".to_string(),
                        action: NetworkPolicyRuleAction::Allow,
                    },
                },
                ReviewDecision::Abort,
            ],
            Some(&network_context),
            None,
        );

        let labels: Vec<String> = options.into_iter().map(|option| option.label).collect();
        assert_eq!(
            labels,
            vec![
                "Yes, just this once".to_string(),
                "Yes, and allow this host for this conversation".to_string(),
                "Yes, and allow this host in the future".to_string(),
                "No, and tell Codex what to do differently".to_string(),
            ]
        );
    }

    #[test]
    fn generic_exec_options_can_offer_allow_for_session() {
        let options = exec_options(
            &[
                ReviewDecision::Approved,
                ReviewDecision::ApprovedForSession,
                ReviewDecision::Abort,
            ],
            None,
            None,
        );

        let labels: Vec<String> = options.into_iter().map(|option| option.label).collect();
        assert_eq!(
            labels,
            vec![
                "Yes, proceed".to_string(),
                "Yes, and don't ask again for this command in this session".to_string(),
                "No, and tell Codex what to do differently".to_string(),
            ]
        );
    }

    #[test]
    fn additional_permissions_exec_options_hide_execpolicy_amendment() {
        let additional_permissions = PermissionProfile {
            file_system: Some(FileSystemPermissions {
                read: Some(vec![absolute_path("/tmp/readme.txt")]),
                write: Some(vec![absolute_path("/tmp/out.txt")]),
            }),
            ..Default::default()
        };
        let options = exec_options(
            &[ReviewDecision::Approved, ReviewDecision::Abort],
            None,
            Some(&additional_permissions),
        );

        let labels: Vec<String> = options.into_iter().map(|option| option.label).collect();
        assert_eq!(
            labels,
            vec![
                "Yes, proceed".to_string(),
                "No, and tell Codex what to do differently".to_string(),
            ]
        );
    }

    #[test]
    fn additional_permissions_prompt_shows_permission_rule_line() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let exec_request = ApprovalRequest::Exec {
            thread_id: ThreadId::new(),
            thread_label: None,
            id: "test".into(),
            command: vec!["cat".into(), "/tmp/readme.txt".into()],
            reason: None,
            available_decisions: vec![ReviewDecision::Approved, ReviewDecision::Abort],
            network_approval_context: None,
            additional_permissions: Some(PermissionProfile {
                file_system: Some(FileSystemPermissions {
                    read: Some(vec![absolute_path("/tmp/readme.txt")]),
                    write: Some(vec![absolute_path("/tmp/out.txt")]),
                }),
                ..Default::default()
            }),
        };

        let view = ApprovalOverlay::new(exec_request, tx, Features::with_defaults());
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, view.desired_height(120)));
        view.render(Rect::new(0, 0, 120, view.desired_height(120)), &mut buf);

        let rendered: Vec<String> = (0..buf.area.height)
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("Permission rule:")),
            "expected permission-rule line, got {rendered:?}"
        );
    }

    #[test]
    fn additional_permissions_prompt_snapshot() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let exec_request = ApprovalRequest::Exec {
            thread_id: ThreadId::new(),
            thread_label: None,
            id: "test".into(),
            command: vec!["cat".into(), "/tmp/readme.txt".into()],
            reason: Some("need filesystem access".into()),
            available_decisions: vec![ReviewDecision::Approved, ReviewDecision::Abort],
            network_approval_context: None,
            additional_permissions: Some(PermissionProfile {
                file_system: Some(FileSystemPermissions {
                    read: Some(vec![absolute_path("/tmp/readme.txt")]),
                    write: Some(vec![absolute_path("/tmp/out.txt")]),
                }),
                ..Default::default()
            }),
        };

        let view = ApprovalOverlay::new(exec_request, tx, Features::with_defaults());
        assert_snapshot!(
            "approval_overlay_additional_permissions_prompt",
            normalize_snapshot_paths(render_overlay_lines(&view, 120))
        );
    }

    #[test]
    fn network_exec_prompt_title_includes_host() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let exec_request = ApprovalRequest::Exec {
            thread_id: ThreadId::new(),
            thread_label: None,
            id: "test".into(),
            command: vec!["curl".into(), "https://example.com".into()],
            reason: Some("network request blocked".into()),
            available_decisions: vec![
                ReviewDecision::Approved,
                ReviewDecision::ApprovedForSession,
                ReviewDecision::NetworkPolicyAmendment {
                    network_policy_amendment: NetworkPolicyAmendment {
                        host: "example.com".to_string(),
                        action: NetworkPolicyRuleAction::Allow,
                    },
                },
                ReviewDecision::Abort,
            ],
            network_approval_context: Some(NetworkApprovalContext {
                host: "example.com".to_string(),
                protocol: NetworkApprovalProtocol::Https,
            }),
            additional_permissions: None,
        };

        let view = ApprovalOverlay::new(exec_request, tx, Features::with_defaults());
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, view.desired_height(100)));
        view.render(Rect::new(0, 0, 100, view.desired_height(100)), &mut buf);
        assert_snapshot!("network_exec_prompt", format!("{buf:?}"));

        let rendered: Vec<String> = (0..buf.area.height)
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect();

        assert!(
            rendered.iter().any(|line| {
                line.contains("Do you want to approve network access to \"example.com\"?")
            }),
            "expected network title to include host, got {rendered:?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("$ curl")),
            "network prompt should not show command line, got {rendered:?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("don't ask again")),
            "network prompt should not show execpolicy option, got {rendered:?}"
        );
    }

    #[test]
    fn exec_history_cell_wraps_with_two_space_indent() {
        let command = vec![
            "/bin/zsh".into(),
            "-lc".into(),
            "git add tui/src/render/mod.rs tui/src/render/renderable.rs".into(),
        ];
        let cell = history_cell::new_approval_decision_cell(command, ReviewDecision::Approved);
        let lines = cell.display_lines(28);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let expected = vec![
            "✔ You approved codex to run".to_string(),
            "  git add tui/src/render/".to_string(),
            "  mod.rs tui/src/render/".to_string(),
            "  renderable.rs this time".to_string(),
        ];
        assert_eq!(rendered, expected);
    }

    #[test]
    fn enter_sets_last_selected_index_without_dismissing() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut view = ApprovalOverlay::new(make_exec_request(), tx, Features::with_defaults());
        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            view.is_complete(),
            "exec approval should complete without queued requests"
        );

        let mut decision = None;
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::SubmitThreadOp {
                op: Op::ExecApproval { decision: d, .. },
                ..
            } = ev
            {
                decision = Some(d);
                break;
            }
        }
        assert_eq!(decision, Some(ReviewDecision::Approved));
    }

    #[test]
    fn patch_reject_shortcuts_open_notes_and_show_expected_labels() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let view = ApprovalOverlay::new(make_patch_request(), tx, Features::with_defaults());
        let rendered = render_overlay_lines(&view, 80);

        assert!(
            rendered.contains("(y)"),
            "patch option should show y shortcut: {rendered}"
        );
        assert!(
            rendered.contains("(a)"),
            "patch option should show a shortcut: {rendered}"
        );
        assert!(
            rendered.contains("(tab)"),
            "patch option should show tab shortcut: {rendered}"
        );
        assert!(
            !rendered.contains("(esc)"),
            "patch option should not show esc shortcut: {rendered}"
        );
        assert!(
            rendered.contains("esc to interrupt"),
            "patch modal should show interrupt hint under options: {rendered}"
        );

        let assert_opens_notes = |events: &[KeyEvent]| {
            let (tx, _rx) = unbounded_channel::<AppEvent>();
            let tx = AppEventSender::new(tx);
            let mut view =
                ApprovalOverlay::new(make_patch_request(), tx, Features::with_defaults());
            for event in events {
                view.handle_key_event(*event);
            }

            let state = view.patch_state().expect("patch state");
            assert_eq!(
                state.options_state.selected_idx,
                Some(PATCH_REJECT_OPTION_INDEX)
            );
            assert_eq!(state.focus, PatchFocus::Notes);
            assert!(view.patch_notes_visible());
            assert!(!view.is_complete());
        };

        assert_opens_notes(&[KeyEvent::from(KeyCode::Tab)]);
        assert_opens_notes(&[KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)]);
        assert_opens_notes(&[KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE)]);
        assert_opens_notes(&[
            KeyEvent::from(KeyCode::Down),
            KeyEvent::from(KeyCode::Down),
            KeyEvent::from(KeyCode::Enter),
        ]);
    }

    #[test]
    fn patch_shortcuts_bind_expected_actions() {
        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(make_patch_request(), tx, Features::with_defaults());

        view.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        let mut decision = None;
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::SubmitThreadOp {
                op: Op::PatchApproval { decision: d, .. },
                ..
            } = event
            {
                decision = Some(d);
                break;
            }
        }
        assert_eq!(decision, Some(ReviewDecision::Approved));

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(make_patch_request(), tx, Features::with_defaults());

        view.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        let mut decision = None;
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::SubmitThreadOp {
                op: Op::PatchApproval { decision: d, .. },
                ..
            } = event
            {
                decision = Some(d);
                break;
            }
        }
        assert_eq!(decision, Some(ReviewDecision::ApprovedForSession));

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(make_patch_request(), tx, Features::with_defaults());
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));

        assert_eq!(view.on_ctrl_c(), CancellationEvent::Handled);

        let mut decision = None;
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::SubmitThreadOp {
                op: Op::PatchApproval { decision: d, .. },
                ..
            } = event
            {
                decision = Some(d);
                break;
            }
        }
        assert_eq!(decision, Some(ReviewDecision::Abort));
    }

    #[test]
    fn patch_notes_validate_before_submit_and_emit_event() {
        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let mut view = ApprovalOverlay::new(make_patch_request(), tx, Features::with_defaults());
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));

        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        assert!(
            rx.try_recv().is_err(),
            "unexpected event for empty note submit"
        );
        assert!(!view.is_complete());
        assert!(view.patch_note_error_visible());

        view.patch_state_mut()
            .expect("patch state")
            .composer
            .set_text_content("use smaller diffs".to_string(), Vec::new(), Vec::new());

        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        let event = rx.try_recv().expect("reject event");
        assert!(
            matches!(
                event,
                AppEvent::RejectPatchApprovalWithNotes { text, .. }
                    if text == "use smaller diffs"
            ),
            "expected reject-with-notes event"
        );
        assert!(view.is_complete());
    }
}
