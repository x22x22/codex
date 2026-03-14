use std::collections::HashMap;

use color_eyre::eyre::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Widget;

use crate::app::App;
use crate::chatwidget::ExternalEditorState;
use crate::custom_terminal::Frame;
use crate::insert_history::insert_history_lines;
use crate::render::renderable::Renderable;
use crate::terminal_palette::PARENT_BG_RGB_ENV_VAR;
use crate::tui;
use crate::tui::TuiEvent;
use crate::vt100_backend::VT100Backend;
use crate::vt100_render::render_screen;

use super::fork_session_terminal::ForkSessionTerminal;

const DEFAULT_POPUP_WIDTH_NUMERATOR: u16 = 2;
const DEFAULT_POPUP_WIDTH_DENOMINATOR: u16 = 3;
const DEFAULT_POPUP_HEIGHT_NUMERATOR: u16 = 3;
const DEFAULT_POPUP_HEIGHT_DENOMINATOR: u16 = 5;
const POPUP_MIN_WIDTH: u16 = 44;
const POPUP_MIN_HEIGHT: u16 = 10;
const POPUP_HORIZONTAL_MARGIN: u16 = 2;
const POPUP_VERTICAL_MARGIN: u16 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum OverlayCommandState {
    #[default]
    PassThrough,
    AwaitingPrefix,
    Move,
    Resize,
}

pub(crate) struct ForkSessionOverlayState {
    pub(crate) terminal: ForkSessionTerminal,
    popup: Rect,
    command_state: OverlayCommandState,
}

fn popup_size_bounds(area: Rect) -> Rect {
    let horizontal_margin = POPUP_HORIZONTAL_MARGIN.min(area.width.saturating_sub(1) / 2);
    let vertical_margin = POPUP_VERTICAL_MARGIN.min(area.height.saturating_sub(1) / 2);
    Rect::new(
        area.x + horizontal_margin,
        area.y + vertical_margin,
        area.width.saturating_sub(horizontal_margin * 2).max(1),
        area.height.saturating_sub(vertical_margin * 2).max(1),
    )
}

fn popup_width_bounds(area: Rect) -> (u16, u16) {
    let bounds = popup_size_bounds(area);
    let min_width = POPUP_MIN_WIDTH.min(bounds.width).max(1);
    let max_width = bounds.width.max(min_width);
    (min_width, max_width)
}

fn popup_height_bounds(area: Rect) -> (u16, u16) {
    let bounds = popup_size_bounds(area);
    let min_height = POPUP_MIN_HEIGHT.min(bounds.height).max(1);
    let max_height = bounds.height.max(min_height);
    (min_height, max_height)
}

fn default_popup_rect(area: Rect) -> Rect {
    let bounds = popup_size_bounds(area);
    let width = bounds
        .width
        .saturating_mul(DEFAULT_POPUP_WIDTH_NUMERATOR)
        / DEFAULT_POPUP_WIDTH_DENOMINATOR;
    let width = width.min(bounds.width).max(POPUP_MIN_WIDTH.min(bounds.width).max(1));
    let height = bounds
        .height
        .saturating_mul(DEFAULT_POPUP_HEIGHT_NUMERATOR)
        / DEFAULT_POPUP_HEIGHT_DENOMINATOR;
    let height = height
        .min(bounds.height)
        .max(POPUP_MIN_HEIGHT.min(bounds.height).max(1));

    Rect::new(
        bounds.x + bounds.width.saturating_sub(width) / 2,
        bounds.y + bounds.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn clamp_popup_rect(area: Rect, popup: Rect) -> Rect {
    let (min_width, max_width) = popup_width_bounds(area);
    let (min_height, max_height) = popup_height_bounds(area);
    let width = popup.width.clamp(min_width, max_width);
    let height = popup.height.clamp(min_height, max_height);
    let max_x = area.right().saturating_sub(width);
    let max_y = area.bottom().saturating_sub(height);
    let x = popup.x.clamp(area.x, max_x);
    let y = popup.y.clamp(area.y, max_y);
    Rect::new(x, y, width, height)
}

fn move_popup_rect(area: Rect, popup: Rect, dx: i32, dy: i32) -> Rect {
    let popup = clamp_popup_rect(area, popup);
    let max_x = i32::from(area.right().saturating_sub(popup.width));
    let max_y = i32::from(area.bottom().saturating_sub(popup.height));
    let next_x = (i32::from(popup.x) + dx).clamp(i32::from(area.x), max_x);
    let next_y = (i32::from(popup.y) + dy).clamp(i32::from(area.y), max_y);
    Rect::new(next_x as u16, next_y as u16, popup.width, popup.height)
}

fn move_popup_delta(key_event: KeyEvent) -> Option<(i32, i32)> {
    let step = if key_event.modifiers.contains(KeyModifiers::SHIFT) {
        5
    } else {
        1
    };
    match key_event.code {
        KeyCode::Left | KeyCode::Char('h') => Some((-step, 0)),
        KeyCode::Right | KeyCode::Char('l') => Some((step, 0)),
        KeyCode::Up | KeyCode::Char('k') => Some((0, -step)),
        KeyCode::Down | KeyCode::Char('j') => Some((0, step)),
        _ => None,
    }
}

fn resize_left_edge(area: Rect, popup: Rect, delta: i32) -> Rect {
    let popup = clamp_popup_rect(area, popup);
    let (min_width, max_width) = popup_width_bounds(area);
    let right = i32::from(popup.right());
    let min_left = (right - i32::from(max_width)).max(i32::from(area.x));
    let max_left = right - i32::from(min_width);
    let next_left = (i32::from(popup.x) + delta).clamp(min_left, max_left);
    Rect::new(next_left as u16, popup.y, (right - next_left) as u16, popup.height)
}

fn resize_right_edge(area: Rect, popup: Rect, delta: i32) -> Rect {
    let popup = clamp_popup_rect(area, popup);
    let (min_width, max_width) = popup_width_bounds(area);
    let left = i32::from(popup.x);
    let min_right = left + i32::from(min_width);
    let max_right = (left + i32::from(max_width)).min(i32::from(area.right()));
    let next_right = (i32::from(popup.right()) + delta).clamp(min_right, max_right);
    Rect::new(popup.x, popup.y, (next_right - left) as u16, popup.height)
}

fn resize_top_edge(area: Rect, popup: Rect, delta: i32) -> Rect {
    let popup = clamp_popup_rect(area, popup);
    let (min_height, max_height) = popup_height_bounds(area);
    let bottom = i32::from(popup.bottom());
    let min_top = (bottom - i32::from(max_height)).max(i32::from(area.y));
    let max_top = bottom - i32::from(min_height);
    let next_top = (i32::from(popup.y) + delta).clamp(min_top, max_top);
    Rect::new(popup.x, next_top as u16, popup.width, (bottom - next_top) as u16)
}

fn resize_bottom_edge(area: Rect, popup: Rect, delta: i32) -> Rect {
    let popup = clamp_popup_rect(area, popup);
    let (min_height, max_height) = popup_height_bounds(area);
    let top = i32::from(popup.y);
    let min_bottom = top + i32::from(min_height);
    let max_bottom = (top + i32::from(max_height)).min(i32::from(area.bottom()));
    let next_bottom = (i32::from(popup.bottom()) + delta).clamp(min_bottom, max_bottom);
    Rect::new(popup.x, popup.y, popup.width, (next_bottom - top) as u16)
}

fn resize_all_edges(area: Rect, popup: Rect, delta: i32) -> Rect {
    let popup = resize_left_edge(area, popup, -delta);
    let popup = resize_right_edge(area, popup, delta);
    let popup = resize_top_edge(area, popup, -delta);
    resize_bottom_edge(area, popup, delta)
}

fn popup_hint(command_state: OverlayCommandState) -> Vec<Span<'static>> {
    match command_state {
        OverlayCommandState::PassThrough => vec!["ctrl+] prefix".dim()],
        OverlayCommandState::AwaitingPrefix => {
            vec![
                "m move".yellow(),
                "  ".into(),
                "r resize".yellow(),
                "  ".into(),
                "q close".yellow(),
                "  ".into(),
                "] send ^]".dim(),
            ]
        }
        OverlayCommandState::Move => {
            vec![
                "move".yellow().bold(),
                "  ".into(),
                "hjkl/arrows".dim(),
                "  ".into(),
                "shift faster".dim(),
                "  ".into(),
                "enter done".dim(),
            ]
        }
        OverlayCommandState::Resize => {
            vec![
                "resize".yellow().bold(),
                "  ".into(),
                "hjkl HJKL +/-".dim(),
                "  ".into(),
                "arrows too".dim(),
                "  ".into(),
                "enter done".dim(),
            ]
        }
    }
}

fn popup_block(exit_code: Option<i32>, command_state: OverlayCommandState) -> Block<'static> {
    let status = match exit_code {
        Some(code) => format!("exited {code}").red().bold(),
        None => "running".green().bold(),
    };
    let mut title = vec![
        " fork session ".bold().cyan(),
        "  ".into(),
        status,
        "  ".into(),
    ];
    title.extend(popup_hint(command_state));
    let title = Line::from(title);

    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title)
}

fn popup_terminal_size(popup: Rect) -> codex_utils_pty::TerminalSize {
    let inner = popup_block(None, OverlayCommandState::PassThrough).inner(popup);
    codex_utils_pty::TerminalSize {
        rows: inner.height.max(1),
        cols: inner.width.max(1),
    }
}

fn append_config_override(args: &mut Vec<String>, key: &str, value: impl std::fmt::Display) {
    args.push("-c".to_string());
    args.push(format!("{key}={value}"));
}

fn parent_bg_rgb_env_value(bg: (u8, u8, u8)) -> String {
    let (red, green, blue) = bg;
    format!("{red},{green},{blue}")
}

fn child_overlay_env(mut env: HashMap<String, String>) -> HashMap<String, String> {
    for key in [
        "TMUX",
        "TMUX_PANE",
        "ZELLIJ",
        "ZELLIJ_SESSION_NAME",
        "ZELLIJ_VERSION",
    ] {
        env.remove(key);
    }
    if let Some(bg) = crate::terminal_palette::default_bg() {
        env.insert(PARENT_BG_RGB_ENV_VAR.to_string(), parent_bg_rgb_env_value(bg));
    }
    env
}

fn sandbox_mode_override(policy: &codex_protocol::protocol::SandboxPolicy) -> &'static str {
    match policy {
        codex_protocol::protocol::SandboxPolicy::ReadOnly { .. } => "read-only",
        codex_protocol::protocol::SandboxPolicy::WorkspaceWrite { .. } => "workspace-write",
        codex_protocol::protocol::SandboxPolicy::DangerFullAccess
        | codex_protocol::protocol::SandboxPolicy::ExternalSandbox { .. } => "danger-full-access",
    }
}

impl App {
    pub(crate) async fn open_fork_session_overlay(
        &mut self,
        tui: &mut tui::Tui,
        thread_id: codex_protocol::ThreadId,
    ) -> Result<()> {
        let size = tui.terminal.size()?;
        let popup = default_popup_rect(Rect::new(0, 0, size.width, size.height));
        let terminal_size = popup_terminal_size(popup);
        let program = std::env::current_exe()?.to_string_lossy().into_owned();
        let env = child_overlay_env(std::env::vars().collect::<HashMap<_, _>>());
        let args = self.build_fork_session_overlay_args(thread_id);
        let terminal = ForkSessionTerminal::spawn(
            &program,
            &args,
            &self.config.cwd,
            env,
            terminal_size,
            tui.frame_requester(),
        )
        .await?;

        self.fork_session_overlay = Some(ForkSessionOverlayState {
            terminal,
            popup,
            command_state: OverlayCommandState::PassThrough,
        });
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(crate) async fn close_fork_session_overlay(&mut self, tui: &mut tui::Tui) -> Result<()> {
        self.fork_session_overlay = None;
        self.restore_inline_view_after_fork_overlay_close(tui)?;
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(crate) async fn handle_fork_session_overlay_tui_event(
        &mut self,
        tui: &mut tui::Tui,
        event: TuiEvent,
    ) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => {
                let mut close_overlay = false;
                let mut forward_key = None;
                let viewport = tui.terminal.size()?;
                let area = Rect::new(0, 0, viewport.width, viewport.height);
                if let Some(state) = self.fork_session_overlay.as_mut() {
                    let is_ctrl_prefix =
                        matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && matches!(key_event.code, KeyCode::Char(']'))
                        && key_event.modifiers.contains(KeyModifiers::CONTROL);
                    match state.command_state {
                        OverlayCommandState::PassThrough => {
                            if is_ctrl_prefix {
                                state.command_state = OverlayCommandState::AwaitingPrefix;
                                tui.frame_requester().schedule_frame();
                            } else {
                                forward_key = Some(key_event);
                            }
                        }
                        OverlayCommandState::AwaitingPrefix => {
                            if matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                            {
                                if matches!(
                                    key_event.code,
                                    KeyCode::Left
                                        | KeyCode::Right
                                        | KeyCode::Up
                                        | KeyCode::Down
                                ) {
                                    if let Some((dx, dy)) = move_popup_delta(key_event) {
                                        state.command_state = OverlayCommandState::Move;
                                        state.popup =
                                            move_popup_rect(area, state.popup, dx, dy);
                                    }
                                } else if matches!(
                                    key_event.code,
                                    KeyCode::Char('=') | KeyCode::Char('+') | KeyCode::Char('-')
                                ) {
                                    state.command_state = OverlayCommandState::Resize;
                                    let delta = match key_event.code {
                                        KeyCode::Char('=') | KeyCode::Char('+') => 1,
                                        KeyCode::Char('-') => -1,
                                        _ => unreachable!(),
                                    };
                                    state.popup = resize_all_edges(area, state.popup, delta);
                                } else {
                                    match key_event.code {
                                    KeyCode::Char('m') | KeyCode::Char('M') => {
                                        state.command_state = OverlayCommandState::Move;
                                    }
                                    KeyCode::Char('r') | KeyCode::Char('R') => {
                                        state.command_state = OverlayCommandState::Resize;
                                    }
                                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                                        close_overlay = true;
                                    }
                                    KeyCode::Char('d')
                                        if key_event.modifiers.contains(KeyModifiers::CONTROL) =>
                                    {
                                        close_overlay = true;
                                    }
                                    KeyCode::Char(']') => {
                                        if is_ctrl_prefix {
                                            state.command_state = OverlayCommandState::PassThrough;
                                        } else {
                                            forward_key = Some(KeyEvent::new(
                                                KeyCode::Char(']'),
                                                KeyModifiers::CONTROL,
                                            ));
                                            state.command_state = OverlayCommandState::PassThrough;
                                        }
                                    }
                                    KeyCode::Esc | KeyCode::Enter => {
                                        state.command_state = OverlayCommandState::PassThrough;
                                    }
                                    _ => {
                                        if is_ctrl_prefix {
                                            forward_key = Some(KeyEvent::new(
                                                KeyCode::Char(']'),
                                                KeyModifiers::CONTROL,
                                            ));
                                        }
                                        state.command_state = OverlayCommandState::PassThrough;
                                    }
                                }
                                }
                                tui.frame_requester().schedule_frame();
                            }
                        }
                        OverlayCommandState::Move => {
                            if matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                            {
                                if is_ctrl_prefix {
                                    state.command_state = OverlayCommandState::PassThrough;
                                } else if let Some((dx, dy)) = move_popup_delta(key_event) {
                                    state.popup = move_popup_rect(area, state.popup, dx, dy);
                                } else {
                                    match key_event.code {
                                        KeyCode::Esc | KeyCode::Enter => {
                                            state.command_state = OverlayCommandState::PassThrough;
                                        }
                                        _ => {}
                                    }
                                }
                                tui.frame_requester().schedule_frame();
                            }
                        }
                        OverlayCommandState::Resize => {
                            if matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                            {
                                if is_ctrl_prefix {
                                    state.command_state = OverlayCommandState::PassThrough;
                                } else {
                                    match key_event.code {
                                    KeyCode::Left => {
                                        let delta = if key_event.modifiers.contains(KeyModifiers::SHIFT) {
                                            1
                                        } else {
                                            -1
                                        };
                                        state.popup = resize_left_edge(area, state.popup, delta);
                                    }
                                    KeyCode::Right => {
                                        let delta = if key_event.modifiers.contains(KeyModifiers::SHIFT) {
                                            -1
                                        } else {
                                            1
                                        };
                                        state.popup = resize_right_edge(area, state.popup, delta);
                                    }
                                    KeyCode::Up => {
                                        let delta = if key_event.modifiers.contains(KeyModifiers::SHIFT) {
                                            1
                                        } else {
                                            -1
                                        };
                                        state.popup = resize_top_edge(area, state.popup, delta);
                                    }
                                    KeyCode::Down => {
                                        let delta = if key_event.modifiers.contains(KeyModifiers::SHIFT) {
                                            -1
                                        } else {
                                            1
                                        };
                                        state.popup = resize_bottom_edge(area, state.popup, delta);
                                    }
                                    KeyCode::Char('h') => {
                                        state.popup = resize_left_edge(area, state.popup, -1);
                                    }
                                    KeyCode::Char('H') => {
                                        state.popup = resize_left_edge(area, state.popup, 1);
                                    }
                                    KeyCode::Char('j') => {
                                        state.popup = resize_bottom_edge(area, state.popup, 1);
                                    }
                                    KeyCode::Char('J') => {
                                        state.popup = resize_bottom_edge(area, state.popup, -1);
                                    }
                                    KeyCode::Char('k') => {
                                        state.popup = resize_top_edge(area, state.popup, -1);
                                    }
                                    KeyCode::Char('K') => {
                                        state.popup = resize_top_edge(area, state.popup, 1);
                                    }
                                    KeyCode::Char('l') => {
                                        state.popup = resize_right_edge(area, state.popup, 1);
                                    }
                                    KeyCode::Char('L') => {
                                        state.popup = resize_right_edge(area, state.popup, -1);
                                    }
                                    KeyCode::Char('=') | KeyCode::Char('+') => {
                                        state.popup = resize_all_edges(area, state.popup, 1);
                                    }
                                    KeyCode::Char('-') => {
                                        state.popup = resize_all_edges(area, state.popup, -1);
                                    }
                                    KeyCode::Esc | KeyCode::Enter => {
                                        state.command_state = OverlayCommandState::PassThrough;
                                    }
                                    _ => {}
                                }
                                }
                                tui.frame_requester().schedule_frame();
                            }
                        }
                    }
                }
                if close_overlay {
                    self.close_fork_session_overlay(tui).await?;
                } else if let Some(key_event) = forward_key
                    && let Some(state) = self.fork_session_overlay.as_ref()
                {
                    let _ = state.terminal.handle_key_event(key_event).await;
                }
            }
            TuiEvent::Paste(pasted) => {
                if let Some(state) = self.fork_session_overlay.as_ref() {
                    let pasted = pasted.replace("\r", "\n");
                    let _ = state.terminal.handle_paste(&pasted).await;
                }
            }
            TuiEvent::Draw => {
                if self
                    .fork_session_overlay
                    .as_ref()
                    .is_some_and(|state| state.terminal.exit_code().is_some())
                {
                    self.close_fork_session_overlay(tui).await?;
                    return Ok(());
                }
                if self.backtrack_render_pending {
                    self.backtrack_render_pending = false;
                    self.render_transcript_once(tui);
                }
                self.chat_widget.maybe_post_pending_notification(tui);
                self.chat_widget.pre_draw_tick();
                let terminal_height = tui.terminal.size()?.height;
                tui.draw(terminal_height, |frame| {
                    self.render_fork_session_background(frame);
                    if let Some((x, y)) = self.render_fork_session_overlay_frame(frame) {
                        frame.set_cursor_position((x, y));
                    }
                })?;
                if self.chat_widget.external_editor_state() == ExternalEditorState::Requested {
                    self.chat_widget
                        .set_external_editor_state(ExternalEditorState::Active);
                    self.app_event_tx
                        .send(crate::app_event::AppEvent::LaunchExternalEditor);
                }
            }
        }
        Ok(())
    }

    fn restore_inline_view_after_fork_overlay_close(&mut self, tui: &mut tui::Tui) -> Result<()> {
        let size = tui.terminal.size()?;
        let viewport_height = self.chat_widget.desired_height(size.width).min(size.height);
        tui.clear_pending_history_lines();
        tui.terminal
            .set_viewport_area(Rect::new(0, 0, size.width, viewport_height));
        tui.terminal.clear_visible_screen()?;
        self.has_emitted_history_lines = false;
        self.render_transcript_once(tui);
        self.has_emitted_history_lines = !self.transcript_cells.is_empty();
        Ok(())
    }

    fn visible_history_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut has_lines = false;

        for cell in &self.transcript_cells {
            let mut display = cell.display_lines(width);
            if display.is_empty() {
                continue;
            }
            if !cell.is_stream_continuation() {
                if has_lines {
                    lines.push(Line::from(""));
                } else {
                    has_lines = true;
                }
            }
            lines.append(&mut display);
        }

        lines.extend(self.deferred_history_lines.iter().cloned());
        lines
    }

    fn background_history_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let header_lines = self.clear_ui_header_lines(width);
        let transcript_has_session_header = self
            .transcript_cells
            .first()
            .map(|cell| cell.display_lines(width))
            .is_some_and(|display_lines| display_lines.starts_with(&header_lines));
        let active_cell_has_session_header = self
            .chat_widget
            .active_cell_transcript_lines(width)
            .is_some_and(|active_lines| active_lines.starts_with(&header_lines));
        if !transcript_has_session_header && !active_cell_has_session_header {
            lines.extend(header_lines);
        }

        let mut history_lines = self.visible_history_lines(width);
        if !lines.is_empty() && !history_lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.append(&mut history_lines);
        lines
    }

    fn render_fork_session_background(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        Clear.render(area, frame.buffer);
        let height = self.chat_widget.desired_height(area.width).min(area.height).max(1);
        let background_viewport = Rect::new(area.x, area.y, area.width, height);

        let Ok(mut terminal) = crate::custom_terminal::Terminal::with_options(VT100Backend::new(
            area.width,
            area.height,
        )) else {
            self.chat_widget.render(background_viewport, frame.buffer);
            return;
        };
        terminal.set_viewport_area(background_viewport);

        let history_lines = self.background_history_lines(area.width);
        if !history_lines.is_empty() {
            let _ = insert_history_lines(&mut terminal, history_lines);
        }

        let _ = terminal.draw(|offscreen_frame| {
            self.chat_widget.render(offscreen_frame.area(), offscreen_frame.buffer);
            if let Some((x, y)) = self.chat_widget.cursor_pos(offscreen_frame.area()) {
                offscreen_frame.set_cursor_position((x, y));
            }
        });

        let _ = render_screen(terminal.backend().vt100().screen(), area, frame.buffer);
    }

    pub(crate) fn render_fork_session_overlay_frame(
        &mut self,
        frame: &mut Frame<'_>,
    ) -> Option<(u16, u16)> {
        let state = self.fork_session_overlay.as_mut()?;
        state.popup = clamp_popup_rect(frame.area(), state.popup);
        let popup = state.popup;
        Clear.render(popup, frame.buffer);

        let exit_code = state.terminal.exit_code();
        let block = popup_block(exit_code, state.command_state);
        let inner = block.inner(popup);
        block.render(popup, frame.buffer);

        if inner.is_empty() {
            return None;
        }

        state.terminal.resize(codex_utils_pty::TerminalSize {
            rows: inner.height.max(1),
            cols: inner.width.max(1),
        });
        state.terminal.render(inner, frame.buffer)
    }

    fn build_fork_session_overlay_args(&self, thread_id: codex_protocol::ThreadId) -> Vec<String> {
        let mut args = vec!["fork".to_string(), thread_id.to_string()];

        for (key, value) in &self.cli_kv_overrides {
            append_config_override(&mut args, key, value);
        }
        if let Some(profile) = self.active_profile.as_ref() {
            args.push("-p".to_string());
            args.push(profile.clone());
        }

        args.push("-C".to_string());
        args.push(self.config.cwd.display().to_string());
        args.push("-m".to_string());
        args.push(self.chat_widget.current_model().to_string());

        if let Some(effort) = self.config.model_reasoning_effort {
            append_config_override(&mut args, "model_reasoning_effort", effort);
        }
        if let Some(policy) = self.runtime_approval_policy_override.as_ref()
            && let Ok(value) = toml::Value::try_from(*policy)
        {
            append_config_override(&mut args, "approval_policy", value);
        }
        if let Some(policy) = self.runtime_sandbox_policy_override.as_ref() {
            append_config_override(&mut args, "sandbox_mode", sandbox_mode_override(policy));
        }

        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app_backtrack::BacktrackState;
    use crate::chatwidget::tests::make_chatwidget_manual_with_sender;
    use crate::file_search::FileSearchManager;
    use codex_core::CodexAuth;
    use codex_core::config::ConfigOverrides;
    use codex_otel::SessionTelemetry;
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::SessionSource;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    use super::super::FeedbackAudience;
    use super::super::WindowsSandboxState;
    use super::super::agent_navigation::AgentNavigationState;

    fn snapshot_buffer(buf: &Buffer) -> String {
        let mut lines = Vec::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            while line.ends_with(' ') {
                line.pop();
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    async fn make_test_app() -> App {
        let (chat_widget, app_event_tx, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
        let config = chat_widget.config_ref().clone();
        let server = Arc::new(
            codex_core::test_support::thread_manager_with_models_provider(
                CodexAuth::from_api_key("Test API Key"),
                config.model_provider.clone(),
            ),
        );
        let auth_manager = codex_core::test_support::auth_manager_from_auth(
            CodexAuth::from_api_key("Test API Key"),
        );
        let file_search = FileSearchManager::new(config.cwd.clone(), app_event_tx.clone());
        let model = codex_core::test_support::get_model_offline(config.model.as_deref());
        let session_telemetry = SessionTelemetry::new(
            ThreadId::new(),
            model.as_str(),
            model.as_str(),
            None,
            None,
            None,
            "test_originator".to_string(),
            false,
            "test".to_string(),
            SessionSource::Cli,
        );

        App {
            server,
            session_telemetry,
            app_event_tx,
            chat_widget,
            auth_manager,
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
            backtrack: BacktrackState::default(),
            backtrack_render_pending: false,
            feedback: codex_feedback::CodexFeedback::new(),
            feedback_audience: FeedbackAudience::External,
            pending_update_action: None,
            suppress_shutdown_complete: false,
            pending_shutdown_exit_thread_id: None,
            windows_sandbox: WindowsSandboxState::default(),
            thread_event_channels: HashMap::new(),
            thread_event_listener_tasks: HashMap::new(),
            agent_navigation: AgentNavigationState::default(),
            active_thread_id: None,
            active_thread_rx: None,
            primary_thread_id: None,
            primary_session_configured: None,
            pending_primary_events: VecDeque::new(),
        }
    }

    #[tokio::test]
    async fn build_fork_overlay_args_include_live_model_and_runtime_overrides() {
        let temp_dir = tempdir().expect("tempdir");
        let mut app = make_test_app().await;
        app.active_profile = Some("dev".to_string());
        app.config.cwd = temp_dir.path().join("project");
        app.chat_widget.set_model("gpt-5.4");
        app.on_update_reasoning_effort(Some(codex_protocol::openai_models::ReasoningEffort::High));
        app.runtime_approval_policy_override =
            Some(codex_protocol::protocol::AskForApproval::Never);
        app.runtime_sandbox_policy_override =
            Some(codex_protocol::protocol::SandboxPolicy::DangerFullAccess);

        let args = app.build_fork_session_overlay_args(ThreadId::new());

        assert_eq!(args[0], "fork");
        assert!(args.iter().any(|arg| arg == "-p"));
        assert!(args.iter().any(|arg| arg == "dev"));
        assert!(args.iter().any(|arg| arg == "-m"));
        assert!(args.iter().any(|arg| arg == "gpt-5.4"));
        assert!(args.iter().any(|arg| arg == "approval_policy=\"never\""));
        assert!(
            args.iter()
                .any(|arg| arg == "sandbox_mode=danger-full-access")
        );
    }

    #[test]
    fn child_overlay_env_strips_terminal_multiplexer_markers() {
        let env = child_overlay_env(HashMap::from([
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("TMUX".to_string(), "1".to_string()),
            ("TMUX_PANE".to_string(), "%1".to_string()),
            ("ZELLIJ".to_string(), "1".to_string()),
            ("ZELLIJ_SESSION_NAME".to_string(), "codex".to_string()),
            ("ZELLIJ_VERSION".to_string(), "0.44.0".to_string()),
        ]));

        assert_eq!(env.get("PATH"), Some(&"/usr/bin".to_string()));
        assert_eq!(env.get("TMUX"), None);
        assert_eq!(env.get("TMUX_PANE"), None);
        assert_eq!(env.get("ZELLIJ"), None);
        assert_eq!(env.get("ZELLIJ_SESSION_NAME"), None);
        assert_eq!(env.get("ZELLIJ_VERSION"), None);
    }

    #[test]
    fn parent_bg_rgb_env_value_formats_rgb_triplet() {
        assert_eq!(parent_bg_rgb_env_value((12, 34, 56)), "12,34,56");
    }

    #[test]
    fn move_popup_rect_clamps_within_viewport() {
        let area = Rect::new(0, 0, 100, 28);
        let popup = default_popup_rect(area);

        let moved = move_popup_rect(area, popup, -100, 100);

        assert_eq!(moved, Rect::new(0, 13, 64, 15));
    }

    #[test]
    fn move_popup_delta_uses_shift_for_faster_steps() {
        assert_eq!(
            move_popup_delta(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Some((-1, 0))
        );
        assert_eq!(
            move_popup_delta(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
            Some((0, 5))
        );
        assert_eq!(
            move_popup_delta(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn resize_popup_rect_respects_min_and_max_bounds() {
        let area = Rect::new(0, 0, 100, 28);
        let popup = default_popup_rect(area);

        let shrunk = resize_all_edges(area, popup, -100);
        let grown = resize_all_edges(area, popup, 100);

        assert_eq!(shrunk, Rect::new(38, 11, 44, 10));
        assert_eq!(grown, Rect::new(0, 0, 96, 26));
    }

    #[test]
    fn resize_popup_rect_can_grow_beyond_default_cap_on_large_viewports() {
        let area = Rect::new(0, 0, 140, 40);
        let popup = default_popup_rect(area);

        let grown = resize_all_edges(area, popup, 100);

        assert_eq!(grown, Rect::new(0, 0, 136, 38));
    }

    #[test]
    fn default_popup_rect_scales_with_large_viewports() {
        let area = Rect::new(0, 0, 180, 50);

        assert_eq!(default_popup_rect(area), Rect::new(31, 11, 117, 28));
    }

    #[tokio::test]
    async fn fork_session_overlay_popup_snapshot() {
        let mut app = make_test_app().await;
        app.transcript_cells = vec![Arc::new(crate::history_cell::new_user_prompt(
            "background session".to_string(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ))];

        let mut parser = vt100::Parser::new(18, 74, 0);
        parser.process(
            b"\x1b[32mIndependent Codex session\x1b[0m\r\n\
\r\n\
> /tmp/worktree\r\n\
\r\n\
ready for a fresh turn\r\n",
        );
        app.fork_session_overlay = Some(ForkSessionOverlayState {
            terminal: ForkSessionTerminal::for_test(parser, None),
            popup: default_popup_rect(Rect::new(0, 0, 100, 28)),
            command_state: OverlayCommandState::PassThrough,
        });

        let area = Rect::new(0, 0, 100, 28);
        let mut buf = Buffer::empty(area);
        let mut frame = Frame {
            cursor_position: None,
            viewport_area: area,
            buffer: &mut buf,
        };

        app.render_fork_session_background(&mut frame);
        let _ = app.render_fork_session_overlay_frame(&mut frame);

        insta::assert_snapshot!("fork_session_overlay_popup", snapshot_buffer(&buf));
    }

    #[tokio::test]
    async fn fork_session_overlay_background_does_not_duplicate_live_header_snapshot() {
        let app = make_test_app().await;

        let area = Rect::new(0, 0, 100, 16);
        let mut buf = Buffer::empty(area);
        let mut frame = Frame {
            cursor_position: None,
            viewport_area: area,
            buffer: &mut buf,
        };

        app.render_fork_session_background(&mut frame);

        insta::assert_snapshot!(
            "fork_session_overlay_background_live_header",
            snapshot_buffer(&buf)
        );
    }
}

#[cfg(test)]
#[path = "fork_session_overlay_vt100_tests.rs"]
mod vt100_tests;
