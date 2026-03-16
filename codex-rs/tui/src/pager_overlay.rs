//! Overlay UIs rendered in an alternate screen.
//!
//! This module implements the pager-style overlays used by the TUI, including the transcript
//! overlay (`Ctrl+T`) that renders a full history view separate from the main viewport.
//!
//! The transcript overlay renders committed transcript cells plus an optional render-only live tail
//! derived from the current in-flight active cell. Because rebuilding wrapped `Line`s on every draw
//! can be expensive, that live tail is cached and only recomputed when its cache key changes, which
//! is derived from the terminal width (wrapping), an active-cell revision (in-place mutations), the
//! stream-continuation flag (spacing), and an animation tick (time-based spinner/shimmer output).
//!
//! The transcript overlay live tail is kept in sync by `App` during draws: `App` supplies an
//! `ActiveCellTranscriptKey` and a function to compute the active cell transcript lines, and
//! `TranscriptOverlay::sync_live_tail` uses the key to decide when the cached tail must be
//! recomputed. `ChatWidget` is responsible for producing a key that changes when the active cell
//! mutates in place or when its transcript output is time-dependent.

use std::io::Result;
use std::ops::Range;
use std::sync::Arc;

use crate::chatwidget::ActiveCellTranscriptKey;
use crate::history_cell::HistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::render::Insets;
use crate::render::renderable::InsetRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;
use crate::text_formatting::truncate_text;
use crate::tui;
use crate::tui::TuiEvent;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

pub(crate) enum Overlay {
    Transcript(TranscriptOverlay),
    Static(StaticOverlay),
}

impl Overlay {
    pub(crate) fn new_transcript(cells: Vec<Arc<dyn HistoryCell>>) -> Self {
        Self::Transcript(TranscriptOverlay::new(cells))
    }

    pub(crate) fn new_static_with_lines(lines: Vec<Line<'static>>, title: String) -> Self {
        Self::Static(StaticOverlay::with_title(lines, title))
    }

    pub(crate) fn new_static_with_renderables(
        renderables: Vec<Box<dyn Renderable>>,
        title: String,
    ) -> Self {
        Self::Static(StaticOverlay::with_renderables(renderables, title))
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match self {
            Overlay::Transcript(o) => o.handle_event(tui, event),
            Overlay::Static(o) => o.handle_event(tui, event),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Overlay::Transcript(o) => o.is_done(),
            Overlay::Static(o) => o.is_done(),
        }
    }
}

const KEY_UP: KeyBinding = key_hint::plain(KeyCode::Up);
const KEY_DOWN: KeyBinding = key_hint::plain(KeyCode::Down);
const KEY_K: KeyBinding = key_hint::plain(KeyCode::Char('k'));
const KEY_J: KeyBinding = key_hint::plain(KeyCode::Char('j'));
const KEY_PAGE_UP: KeyBinding = key_hint::plain(KeyCode::PageUp);
const KEY_PAGE_DOWN: KeyBinding = key_hint::plain(KeyCode::PageDown);
const KEY_SPACE: KeyBinding = key_hint::plain(KeyCode::Char(' '));
const KEY_SHIFT_SPACE: KeyBinding = key_hint::shift(KeyCode::Char(' '));
const KEY_HOME: KeyBinding = key_hint::plain(KeyCode::Home);
const KEY_END: KeyBinding = key_hint::plain(KeyCode::End);
const KEY_LEFT: KeyBinding = key_hint::plain(KeyCode::Left);
const KEY_RIGHT: KeyBinding = key_hint::plain(KeyCode::Right);
const KEY_CTRL_F: KeyBinding = key_hint::ctrl(KeyCode::Char('f'));
const KEY_CTRL_D: KeyBinding = key_hint::ctrl(KeyCode::Char('d'));
const KEY_CTRL_B: KeyBinding = key_hint::ctrl(KeyCode::Char('b'));
const KEY_CTRL_U: KeyBinding = key_hint::ctrl(KeyCode::Char('u'));
const KEY_Q: KeyBinding = key_hint::plain(KeyCode::Char('q'));
const KEY_ESC: KeyBinding = key_hint::plain(KeyCode::Esc);
const KEY_ENTER: KeyBinding = key_hint::plain(KeyCode::Enter);
const KEY_CTRL_T: KeyBinding = key_hint::ctrl(KeyCode::Char('t'));
const KEY_CTRL_C: KeyBinding = key_hint::ctrl(KeyCode::Char('c'));
const KEY_TAB: KeyBinding = key_hint::plain(KeyCode::Tab);
const KEY_A: KeyBinding = key_hint::plain(KeyCode::Char('a'));
const KEY_E: KeyBinding = key_hint::plain(KeyCode::Char('e'));
const MAX_ANCHOR_LINES: usize = 2;

// Common pager navigation hints rendered on the first line
const PAGER_KEY_HINTS: &[(&[KeyBinding], &str)] = &[
    (&[KEY_UP, KEY_DOWN], "to scroll"),
    (&[KEY_PAGE_UP, KEY_PAGE_DOWN], "to page"),
    (&[KEY_HOME, KEY_END], "to jump"),
];

// Render a single line of key hints from (key(s), description) pairs.
fn render_key_hints(area: Rect, buf: &mut Buffer, pairs: &[(&[KeyBinding], &str)]) {
    let mut spans: Vec<Span<'static>> = vec![" ".into()];
    let mut first = true;
    for (keys, desc) in pairs {
        if !first {
            spans.push("   ".into());
        }
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                spans.push("/".into());
            }
            spans.push("<".into());
            spans.push(Span::from(key));
            spans.push(">".into());
        }
        spans.push(" ".into());
        spans.push(Span::from(desc.to_string()));
        first = false;
    }
    Paragraph::new(vec![Line::from(spans).dim()]).render_ref(area, buf);
}

/// Generic widget for rendering a pager view.
struct PagerView {
    renderables: Vec<Box<dyn Renderable>>,
    scroll_offset: usize,
    title: String,
    last_content_height: Option<usize>,
    last_rendered_height: Option<usize>,
    /// If set, on next render scroll the target chunk according to the request.
    pending_scroll_chunk: Option<PendingChunkScroll>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingChunkScroll {
    chunk_index: usize,
    align_top: bool,
}

impl PagerView {
    fn new(renderables: Vec<Box<dyn Renderable>>, title: String, scroll_offset: usize) -> Self {
        Self {
            renderables,
            scroll_offset,
            title,
            last_content_height: None,
            last_rendered_height: None,
            pending_scroll_chunk: None,
        }
    }

    fn content_height(&self, width: u16) -> usize {
        self.renderables
            .iter()
            .map(|c| c.desired_height(width) as usize)
            .sum()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        self.render_header(area, buf);
        let content_area = self.content_area(area);
        self.update_last_content_height(content_area.height);
        let content_height = self.content_height(content_area.width);
        self.last_rendered_height = Some(content_height);
        // If there is a pending request to scroll a specific chunk into view,
        // satisfy it now that wrapping is up to date for this width.
        if let Some(target) = self.pending_scroll_chunk.take() {
            if target.align_top {
                self.scroll_chunk_to_top(target.chunk_index, content_area);
            } else {
                self.ensure_chunk_visible(target.chunk_index, content_area);
            }
        }
        self.scroll_offset = self
            .scroll_offset
            .min(content_height.saturating_sub(content_area.height as usize));

        self.render_content(content_area, buf);

        self.render_bottom_bar(area, content_area, buf, content_height);
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render_ref(area, buf);
        let header = format!("/ {}", self.title);
        header.dim().render_ref(area, buf);
    }

    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let mut y = -(self.scroll_offset as isize);
        let mut drawn_bottom = area.y;
        for renderable in &self.renderables {
            let top = y;
            let height = renderable.desired_height(area.width) as isize;
            y += height;
            let bottom = y;
            if bottom < area.y as isize {
                continue;
            }
            if top > area.y as isize + area.height as isize {
                break;
            }
            if top < 0 {
                let drawn = render_offset_content(area, buf, &**renderable, (-top) as u16);
                drawn_bottom = drawn_bottom.max(area.y + drawn);
            } else {
                let draw_height = (height as u16).min(area.height.saturating_sub(top as u16));
                let draw_area = Rect::new(area.x, area.y + top as u16, area.width, draw_height);
                renderable.render(draw_area, buf);
                drawn_bottom = drawn_bottom.max(draw_area.y.saturating_add(draw_area.height));
            }
        }

        for y in drawn_bottom..area.bottom() {
            if area.width == 0 {
                break;
            }
            buf[(area.x, y)] = Cell::from('~');
            for x in area.x + 1..area.right() {
                buf[(x, y)] = Cell::from(' ');
            }
        }
    }

    fn render_bottom_bar(
        &self,
        full_area: Rect,
        content_area: Rect,
        buf: &mut Buffer,
        total_len: usize,
    ) {
        let sep_y = content_area.bottom();
        let sep_rect = Rect::new(full_area.x, sep_y, full_area.width, 1);

        Span::from("─".repeat(sep_rect.width as usize))
            .dim()
            .render_ref(sep_rect, buf);
        let percent = if total_len == 0 {
            100
        } else {
            let max_scroll = total_len.saturating_sub(content_area.height as usize);
            if max_scroll == 0 {
                100
            } else {
                (((self.scroll_offset.min(max_scroll)) as f32 / max_scroll as f32) * 100.0).round()
                    as u8
            }
        };
        let pct_text = format!(" {percent}% ");
        let pct_w = pct_text.chars().count() as u16;
        let pct_x = sep_rect.x + sep_rect.width - pct_w - 1;
        Span::from(pct_text)
            .dim()
            .render_ref(Rect::new(pct_x, sep_rect.y, pct_w, 1), buf);
    }

    fn handle_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) -> Result<()> {
        match key_event {
            e if KEY_UP.is_press(e) || KEY_K.is_press(e) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            e if KEY_DOWN.is_press(e) || KEY_J.is_press(e) => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            e if KEY_PAGE_UP.is_press(e)
                || KEY_SHIFT_SPACE.is_press(e)
                || KEY_CTRL_B.is_press(e) =>
            {
                let page_height = self.page_height(tui.terminal.viewport_area);
                self.scroll_offset = self.scroll_offset.saturating_sub(page_height);
            }
            e if KEY_PAGE_DOWN.is_press(e) || KEY_SPACE.is_press(e) || KEY_CTRL_F.is_press(e) => {
                let page_height = self.page_height(tui.terminal.viewport_area);
                self.scroll_offset = self.scroll_offset.saturating_add(page_height);
            }
            e if KEY_CTRL_D.is_press(e) => {
                let area = self.content_area(tui.terminal.viewport_area);
                let half_page = (area.height as usize).saturating_add(1) / 2;
                self.scroll_offset = self.scroll_offset.saturating_add(half_page);
            }
            e if KEY_CTRL_U.is_press(e) => {
                let area = self.content_area(tui.terminal.viewport_area);
                let half_page = (area.height as usize).saturating_add(1) / 2;
                self.scroll_offset = self.scroll_offset.saturating_sub(half_page);
            }
            e if KEY_HOME.is_press(e) => {
                self.scroll_offset = 0;
            }
            e if KEY_END.is_press(e) => {
                self.scroll_offset = usize::MAX;
            }
            _ => {
                return Ok(());
            }
        }
        tui.frame_requester()
            .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
        Ok(())
    }

    /// Returns the height of one page in content rows.
    ///
    /// Prefers the last rendered content height (excluding header/footer chrome);
    /// if no render has occurred yet, falls back to the content area height
    /// computed from the given viewport.
    fn page_height(&self, viewport_area: Rect) -> usize {
        self.last_content_height
            .unwrap_or_else(|| self.content_area(viewport_area).height as usize)
    }

    fn update_last_content_height(&mut self, height: u16) {
        self.last_content_height = Some(height as usize);
    }

    fn content_area(&self, area: Rect) -> Rect {
        let mut area = area;
        area.y = area.y.saturating_add(1);
        area.height = area.height.saturating_sub(2);
        area
    }
}

impl PagerView {
    fn is_scrolled_to_bottom(&self) -> bool {
        if self.scroll_offset == usize::MAX {
            return true;
        }
        let Some(height) = self.last_content_height else {
            return false;
        };
        if self.renderables.is_empty() {
            return true;
        }
        let Some(total_height) = self.last_rendered_height else {
            return false;
        };
        if total_height <= height {
            return true;
        }
        let max_scroll = total_height.saturating_sub(height);
        self.scroll_offset >= max_scroll
    }

    /// Request that the given text chunk index be scrolled into view on next render.
    fn scroll_chunk_into_view(&mut self, chunk_index: usize) {
        self.pending_scroll_chunk = Some(PendingChunkScroll {
            chunk_index,
            align_top: false,
        });
    }

    fn scroll_chunk_to_top_on_next_render(&mut self, chunk_index: usize) {
        self.pending_scroll_chunk = Some(PendingChunkScroll {
            chunk_index,
            align_top: true,
        });
    }

    fn ensure_chunk_visible(&mut self, idx: usize, area: Rect) {
        if area.height == 0 || idx >= self.renderables.len() {
            return;
        }
        let first = self
            .renderables
            .iter()
            .take(idx)
            .map(|r| r.desired_height(area.width) as usize)
            .sum();
        let last = first + self.renderables[idx].desired_height(area.width) as usize;
        let current_top = self.scroll_offset;
        let current_bottom = current_top.saturating_add(area.height.saturating_sub(1) as usize);
        if first < current_top {
            self.scroll_offset = first;
        } else if last > current_bottom {
            self.scroll_offset = last.saturating_sub(area.height.saturating_sub(1) as usize);
        }
    }

    fn scroll_chunk_to_top(&mut self, idx: usize, area: Rect) {
        if idx >= self.renderables.len() {
            return;
        }
        self.scroll_offset = self
            .renderables
            .iter()
            .take(idx)
            .map(|r| r.desired_height(area.width) as usize)
            .sum::<usize>();
    }
}

/// A renderable that caches its desired height.
struct CachedRenderable {
    renderable: Box<dyn Renderable>,
    height: std::cell::Cell<Option<u16>>,
    last_width: std::cell::Cell<Option<u16>>,
}

impl CachedRenderable {
    fn new(renderable: impl Into<Box<dyn Renderable>>) -> Self {
        Self {
            renderable: renderable.into(),
            height: std::cell::Cell::new(None),
            last_width: std::cell::Cell::new(None),
        }
    }
}

impl Renderable for CachedRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.renderable.render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        if self.last_width.get() != Some(width) {
            let height = self.renderable.desired_height(width);
            self.height.set(Some(height));
            self.last_width.set(Some(width));
        }
        self.height.get().unwrap_or(0)
    }
}

struct CellRenderable {
    cell: Arc<dyn HistoryCell>,
    highlight_style: Option<Style>,
}

impl Renderable for CellRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = self.cell.transcript_lines(area.width);
        if let Some(style) = self.highlight_style
            && let Some(line) = lines.iter_mut().find(|line| {
                line.spans
                    .iter()
                    .any(|span| !span.content.trim().is_empty())
            })
        {
            line.spans = line
                .spans
                .clone()
                .into_iter()
                .map(|span| span.patch_style(style))
                .collect();
        }
        let p = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        p.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.cell.desired_transcript_height(width)
    }
}

struct CollapsedTurnRenderable {
    prompt: String,
    response: String,
}

impl CollapsedTurnRenderable {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let budget = width.saturating_sub(4).max(8) as usize;
        vec![
            Line::from(vec![
                Span::styled("› ".to_string(), user_message_style().bold().dim()),
                Span::styled(truncate_preview_text(&self.prompt, budget), user_message_style()),
            ]),
            Line::from(vec![
                Span::styled("• ".to_string(), Style::default().dim()),
                Span::styled(
                    truncate_preview_text(&self.response, budget),
                    Style::default().dim(),
                ),
            ]),
        ]
    }
}

impl Renderable for CollapsedTurnRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines(area.width)))
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        Paragraph::new(Text::from(self.lines(width)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }
}

fn truncate_preview_text(text: &str, max_graphemes: usize) -> String {
    let truncated = truncate_text(text, max_graphemes);
    let Some(base) = truncated.strip_suffix("...") else {
        return truncated;
    };
    let base = base.trim_end();
    let Some(last_whitespace) = base.rfind(char::is_whitespace) else {
        return truncated;
    };
    let word_safe = base[..last_whitespace].trim_end();
    if word_safe.len() >= 8 {
        format!("{word_safe}...")
    } else {
        truncated
    }
}

pub(crate) struct TranscriptOverlay {
    /// Pager UI state and the renderables currently displayed.
    ///
    /// The invariant is that `view.renderables` is `render_cells(cells)` plus an optional trailing
    /// live-tail renderable appended after the committed cells.
    view: PagerView,
    /// Committed transcript cells (does not include the live tail).
    cells: Vec<Arc<dyn HistoryCell>>,
    highlight_cell: Option<usize>,
    /// Cache key for the render-only live tail appended after committed cells.
    live_tail_key: Option<LiveTailKey>,
    has_live_tail_renderable: bool,
    anchors: Vec<TranscriptAnchor>,
    selected_anchor: Option<usize>,
    anchors_visible: bool,
    expand_all: bool,
    mode: TranscriptBrowseMode,
    is_done: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TranscriptBrowseMode {
    FreeForm,
    ByPrompt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TranscriptAnchor {
    cell_idx: usize,
    label: String,
}

/// Cache key for the active-cell "live tail" appended to the transcript overlay.
///
/// Changing any field implies a different rendered tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LiveTailKey {
    /// Current terminal width, which affects wrapping.
    width: u16,
    /// Revision that changes on in-place active cell transcript updates.
    revision: u64,
    /// Whether the tail should be treated as a continuation for spacing.
    is_stream_continuation: bool,
    /// Optional animation tick to refresh spinners/progress indicators.
    animation_tick: Option<u64>,
}

impl TranscriptOverlay {
    /// Creates a transcript overlay for a fixed set of committed cells.
    ///
    /// This overlay does not own the "active cell"; callers may optionally append a live tail via
    /// `sync_live_tail` during draws to reflect in-flight activity.
    pub(crate) fn new(transcript_cells: Vec<Arc<dyn HistoryCell>>) -> Self {
        let anchors = Self::build_anchors(&transcript_cells);
        let selected_anchor = anchors.len().checked_sub(1);
        let mode = if anchors.is_empty() {
            TranscriptBrowseMode::FreeForm
        } else {
            TranscriptBrowseMode::ByPrompt
        };
        let mut overlay = Self {
            view: PagerView::new(
                Self::render_cells(
                    &transcript_cells,
                    &anchors,
                    selected_anchor,
                    false,
                    selected_anchor.and_then(|idx| anchors.get(idx).map(|anchor| anchor.cell_idx)),
                ),
                "T R A N S C R I P T".to_string(),
                usize::MAX,
            ),
            cells: transcript_cells,
            highlight_cell: None,
            live_tail_key: None,
            has_live_tail_renderable: false,
            anchors,
            selected_anchor,
            anchors_visible: true,
            expand_all: false,
            mode,
            is_done: false,
        };
        if let Some(anchor_idx) = overlay.selected_anchor {
            overlay.select_anchor(anchor_idx);
        }
        overlay
    }

    fn anchors_are_effectively_visible(&self, area: Rect) -> bool {
        self.anchors_visible && area.width >= 80
    }

    fn anchor_pane_width(&self, area: Rect) -> u16 {
        area.width.clamp(24, 36)
    }

    fn build_anchors(cells: &[Arc<dyn HistoryCell>]) -> Vec<TranscriptAnchor> {
        let mut anchors = Vec::new();
        for (cell_idx, cell) in cells.iter().enumerate() {
            let Some(user_cell) = cell.as_any().downcast_ref::<UserHistoryCell>() else {
                continue;
            };
            let turn_number = anchors.len() + 1;
            let message_preview = Self::summarize_user_prompt(user_cell);
            anchors.push(TranscriptAnchor {
                cell_idx,
                label: format!("{turn_number}. {}", truncate_preview_text(&message_preview, 48)),
            });
        }
        anchors
    }

    fn summarize_user_prompt(user_cell: &UserHistoryCell) -> String {
        let mut lines = user_cell
            .message
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        let first_line = lines
            .next()
            .map(ToOwned::to_owned)
            .or_else(|| {
                (!user_cell.remote_image_urls.is_empty()).then(|| "[image attachment]".to_string())
            })
            .unwrap_or_else(|| "(empty prompt)".to_string());
        let has_more = lines.next().is_some()
            || user_cell.remote_image_urls.len() > 1
            || (!user_cell.remote_image_urls.is_empty() && !user_cell.message.trim().is_empty());
        if has_more && !first_line.ends_with("...") {
            format!("{first_line}...")
        } else {
            first_line
        }
    }

    fn selected_transcript_cell(&self) -> Option<usize> {
        self.highlight_cell.or_else(|| {
            self.selected_anchor
                .and_then(|idx| self.anchors.get(idx))
                .map(|anchor| anchor.cell_idx)
        })
    }

    fn refresh_anchors(&mut self) {
        let previously_selected_cell = self
            .selected_anchor
            .and_then(|idx| self.anchors.get(idx))
            .map(|anchor| anchor.cell_idx);
        self.anchors = Self::build_anchors(&self.cells);
        self.selected_anchor = previously_selected_cell
            .and_then(|cell_idx| {
                self.anchors
                    .iter()
                    .position(|anchor| anchor.cell_idx == cell_idx)
            })
            .or_else(|| self.anchors.len().checked_sub(1));
    }

    fn select_anchor(&mut self, anchor_idx: usize) {
        let Some(cell_idx) = self.anchors.get(anchor_idx).map(|anchor| anchor.cell_idx) else {
            return;
        };
        self.selected_anchor = Some(anchor_idx);
        self.rebuild_renderables();
        if let Some(chunk_idx) = self.chunk_index_for_cell(cell_idx) {
            self.view.scroll_chunk_to_top_on_next_render(chunk_idx);
        }
    }

    fn move_selected_anchor_to(&mut self, anchor_idx: usize, tui: &mut tui::Tui) {
        if self.selected_anchor == Some(anchor_idx) {
            return;
        }
        self.select_anchor(anchor_idx);
        tui.frame_requester()
            .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
    }

    fn handle_anchor_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) -> Result<()> {
        if self.anchors.is_empty() {
            return Ok(());
        }

        let last_anchor = self.anchors.len().saturating_sub(1);
        let current = self.selected_anchor.unwrap_or(last_anchor).min(last_anchor);
        let next = match key_event {
            e if KEY_UP.is_press(e) || KEY_K.is_press(e) => current.saturating_sub(1),
            e if KEY_DOWN.is_press(e) || KEY_J.is_press(e) => current.saturating_add(1).min(last_anchor),
            e if KEY_HOME.is_press(e) => 0,
            e if KEY_END.is_press(e) => last_anchor,
            _ => return Ok(()),
        };
        self.move_selected_anchor_to(next, tui);
        Ok(())
    }

    fn render_cells(
        cells: &[Arc<dyn HistoryCell>],
        anchors: &[TranscriptAnchor],
        selected_anchor: Option<usize>,
        expand_all: bool,
        highlight_cell: Option<usize>,
    ) -> Vec<Box<dyn Renderable>> {
        let mut renderables: Vec<Box<dyn Renderable>> = Vec::new();
        let turn_ranges = Self::turn_ranges(cells, anchors);
        let selected_anchor = selected_anchor.unwrap_or_else(|| anchors.len().saturating_sub(1));
        let first_anchor_idx = anchors.first().map(|anchor| anchor.cell_idx).unwrap_or(cells.len());

        for cell_idx in 0..first_anchor_idx {
            let cell = &cells[cell_idx];
            let renderable = Self::cell_renderable(cell.clone(), cell_idx, highlight_cell);
            renderables.push(Self::with_spacing(renderable, !cell.is_stream_continuation(), renderables.len()));
        }

        for (anchor_idx, turn_range) in turn_ranges.iter().enumerate() {
            if expand_all || anchor_idx == selected_anchor {
                for cell_idx in turn_range.clone() {
                    let cell = &cells[cell_idx];
                    let renderable = Self::cell_renderable(cell.clone(), cell_idx, highlight_cell);
                    renderables.push(Self::with_spacing(renderable, !cell.is_stream_continuation(), renderables.len()));
                }
            } else if let Some(anchor) = anchors.get(anchor_idx) {
                let collapsed = Box::new(CachedRenderable::new(CollapsedTurnRenderable {
                    prompt: anchor.label.clone(),
                    response: Self::collapsed_response_summary(cells, turn_range.clone()),
                })) as Box<dyn Renderable>;
                renderables.push(Self::with_spacing(collapsed, true, renderables.len()));
            }
        }

        renderables
    }

    fn cell_renderable(
        cell: Arc<dyn HistoryCell>,
        cell_idx: usize,
        highlight_cell: Option<usize>,
    ) -> Box<dyn Renderable> {
        if cell.as_any().is::<UserHistoryCell>() {
            Box::new(CachedRenderable::new(CellRenderable {
                cell,
                highlight_style: (highlight_cell == Some(cell_idx))
                    .then(|| user_message_style().reversed()),
            })) as Box<dyn Renderable>
        } else {
            Box::new(CachedRenderable::new(CellRenderable {
                cell,
                highlight_style: None,
            })) as Box<dyn Renderable>
        }
    }

    fn with_spacing(
        renderable: Box<dyn Renderable>,
        add_spacing: bool,
        display_idx: usize,
    ) -> Box<dyn Renderable> {
        if add_spacing && display_idx > 0 {
            Box::new(InsetRenderable::new(renderable, Insets::tlbr(1, 0, 0, 0)))
        } else {
            renderable
        }
    }

    fn turn_ranges(cells: &[Arc<dyn HistoryCell>], anchors: &[TranscriptAnchor]) -> Vec<Range<usize>> {
        anchors
            .iter()
            .enumerate()
            .map(|(idx, anchor)| {
                let end = anchors
                    .get(idx + 1)
                    .map(|next| next.cell_idx)
                    .unwrap_or(cells.len());
                anchor.cell_idx..end
            })
            .collect()
    }

    fn collapsed_response_summary(
        cells: &[Arc<dyn HistoryCell>],
        turn_range: Range<usize>,
    ) -> String {
        for cell_idx in turn_range.start.saturating_add(1)..turn_range.end {
            let mut line_iter = cells[cell_idx].transcript_lines(200).into_iter();
            while let Some(line) = line_iter.next() {
                let text = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let has_more = line_iter.any(|extra_line| {
                        extra_line
                            .spans
                            .iter()
                            .any(|span| !span.content.trim().is_empty())
                    }) || (cell_idx + 1 < turn_range.end);
                    return if has_more && !trimmed.ends_with("...") {
                        format!("{trimmed}...")
                    } else {
                        trimmed.to_string()
                    };
                }
            }
        }
        "(no response yet)".to_string()
    }

    fn chunk_index_for_cell(&self, cell_idx: usize) -> Option<usize> {
        let turn_ranges = Self::turn_ranges(&self.cells, &self.anchors);
        let selected_anchor = self
            .selected_anchor
            .unwrap_or_else(|| self.anchors.len().saturating_sub(1));
        let first_anchor_idx = self
            .anchors
            .first()
            .map(|anchor| anchor.cell_idx)
            .unwrap_or(self.cells.len());

        if cell_idx < first_anchor_idx {
            return Some(cell_idx);
        }

        let mut chunk_idx = first_anchor_idx;
        for (anchor_idx, turn_range) in turn_ranges.iter().enumerate() {
            if anchor_idx == selected_anchor || self.expand_all {
                if turn_range.contains(&cell_idx) {
                    return Some(chunk_idx + cell_idx.saturating_sub(turn_range.start));
                }
                chunk_idx += turn_range.end.saturating_sub(turn_range.start);
            } else {
                if turn_range.start == cell_idx {
                    return Some(chunk_idx);
                }
                chunk_idx += 1;
            }
        }
        None
    }

    /// Insert a committed history cell while keeping any cached live tail.
    ///
    /// The live tail is temporarily removed, the committed cells are rebuilt,
    /// then the tail is reattached. If the tail previously had no leading
    /// spacing because it was the only renderable, we add the missing inset
    /// when the first committed cell arrives.
    ///
    /// This expects `cell` to be a committed transcript cell (not the in-flight active cell). If
    /// the overlay was scrolled to bottom before insertion, it remains pinned to bottom after the
    /// insertion to preserve the "follow along" behavior.
    pub(crate) fn insert_cell(&mut self, cell: Arc<dyn HistoryCell>) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        let had_prior_cells = !self.cells.is_empty();
        let tail_renderable = self.take_live_tail_renderable();
        self.cells.push(cell);
        self.refresh_anchors();
        self.view.renderables = Self::render_cells(
            &self.cells,
            &self.anchors,
            self.selected_anchor,
            self.expand_all,
            self.selected_transcript_cell(),
        );
        if let Some(tail) = tail_renderable {
            let tail = if !had_prior_cells
                && self
                    .live_tail_key
                    .is_some_and(|key| !key.is_stream_continuation)
            {
                // The tail was rendered as the only entry, so it lacks a top
                // inset; add one now that it follows a committed cell.
                Box::new(InsetRenderable::new(tail, Insets::tlbr(1, 0, 0, 0)))
                    as Box<dyn Renderable>
            } else {
                tail
            };
            self.view.renderables.push(tail);
            self.has_live_tail_renderable = true;
        }
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    /// Replace committed transcript cells while keeping any cached in-progress output that is
    /// currently shown at the end of the overlay.
    ///
    /// This is used when existing history is trimmed (for example after rollback) so the
    /// transcript overlay immediately reflects the same committed cells as the main transcript.
    pub(crate) fn replace_cells(&mut self, cells: Vec<Arc<dyn HistoryCell>>) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        self.cells = cells;
        if self
            .highlight_cell
            .is_some_and(|idx| idx >= self.cells.len())
        {
            self.highlight_cell = None;
        }
        self.refresh_anchors();
        self.rebuild_renderables();
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    /// Sync the active-cell live tail with the current width and cell state.
    ///
    /// Recomputes the tail only when the cache key changes, preserving scroll
    /// position and dropping the tail if there is nothing to render.
    ///
    /// The overlay owns committed transcript cells while the live tail is derived from the current
    /// active cell, which can mutate in place while streaming. `App` calls this during
    /// `TuiEvent::Draw` for `Overlay::Transcript`, passing a key that changes when the active cell
    /// mutates or animates so the cached tail stays fresh.
    ///
    /// Passing a key that does not change on in-place active-cell mutations will freeze the tail in
    /// `Ctrl+T` while the main viewport continues to update.
    pub(crate) fn sync_live_tail(
        &mut self,
        width: u16,
        active_key: Option<ActiveCellTranscriptKey>,
        compute_lines: impl FnOnce(u16) -> Option<Vec<Line<'static>>>,
    ) {
        let next_key = active_key.map(|key| LiveTailKey {
            width,
            revision: key.revision,
            is_stream_continuation: key.is_stream_continuation,
            animation_tick: key.animation_tick,
        });

        if self.live_tail_key == next_key {
            return;
        }
        let follow_bottom = self.view.is_scrolled_to_bottom();

        self.take_live_tail_renderable();
        self.live_tail_key = next_key;
        self.has_live_tail_renderable = false;

        if let Some(key) = next_key {
            let lines = compute_lines(width).unwrap_or_default();
            if !lines.is_empty() {
                self.view.renderables.push(Self::live_tail_renderable(
                    lines,
                    !self.cells.is_empty(),
                    key.is_stream_continuation,
                ));
                self.has_live_tail_renderable = true;
            }
        }
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    pub(crate) fn set_highlight_cell(&mut self, cell: Option<usize>) {
        self.highlight_cell = cell;
        if let Some(cell_idx) = cell
            && let Some(anchor_idx) = self
                .anchors
                .iter()
                .position(|anchor| anchor.cell_idx == cell_idx)
        {
            self.selected_anchor = Some(anchor_idx);
        }
        self.rebuild_renderables();
        if let Some(idx) = self.selected_transcript_cell() {
            self.view.scroll_chunk_into_view(idx);
        }
    }

    /// Returns whether the underlying pager view is currently pinned to the bottom.
    ///
    /// The `App` draw loop uses this to decide whether to schedule animation frames for the live
    /// tail; if the user has scrolled up, we avoid driving animation work that they cannot see.
    pub(crate) fn is_scrolled_to_bottom(&self) -> bool {
        self.view.is_scrolled_to_bottom()
    }

    fn rebuild_renderables(&mut self) {
        let tail_renderable = self.take_live_tail_renderable();
        self.view.renderables = Self::render_cells(
            &self.cells,
            &self.anchors,
            self.selected_anchor,
            self.expand_all,
            self.selected_transcript_cell(),
        );
        if let Some(tail) = tail_renderable {
            self.view.renderables.push(tail);
            self.has_live_tail_renderable = true;
        }
    }

    /// Removes and returns the cached live-tail renderable, if present.
    ///
    /// The live tail is represented as a single optional renderable appended after the committed
    /// cell renderables, so this relies on the live tail always being the final entry in
    /// `view.renderables` when present.
    fn take_live_tail_renderable(&mut self) -> Option<Box<dyn Renderable>> {
        if self.has_live_tail_renderable {
            self.has_live_tail_renderable = false;
            self.view.renderables.pop()
        } else {
            None
        }
    }

    fn live_tail_renderable(
        lines: Vec<Line<'static>>,
        has_prior_cells: bool,
        is_stream_continuation: bool,
    ) -> Box<dyn Renderable> {
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        let mut renderable: Box<dyn Renderable> = Box::new(CachedRenderable::new(paragraph));
        if has_prior_cells && !is_stream_continuation {
            renderable = Box::new(InsetRenderable::new(renderable, Insets::tlbr(1, 0, 0, 0)));
        }
        renderable
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(line1, buf, PAGER_KEY_HINTS);

        let mut pairs: Vec<(&[KeyBinding], &str)> = vec![
            (&[KEY_Q], "to quit"),
            (
                &[KEY_A],
                if self.anchors_visible {
                    "hide side panel"
                } else {
                    "show side panel"
                },
            ),
            (&[KEY_E], if self.expand_all { "collapse all" } else { "expand all" }),
        ];
        if self.anchors_visible {
            pairs.push((
                &[KEY_TAB],
                if self.mode == TranscriptBrowseMode::ByPrompt {
                    "main panel"
                } else {
                    "side panel"
                },
            ));
        }
        if self.highlight_cell.is_some() {
            pairs.push((&[KEY_ESC, KEY_LEFT], "to edit prev"));
            pairs.push((&[KEY_RIGHT], "to edit next"));
            pairs.push((&[KEY_ENTER], "to edit message"));
        } else {
            pairs.push((&[KEY_ESC], "to edit prev"));
        }
        render_key_hints(line2, buf, &pairs);
    }

    fn render_anchor_shell(&self, area: Rect, buf: &mut Buffer) {
        let border_style = if self.mode == TranscriptBrowseMode::ByPrompt {
            Style::default().cyan().bold()
        } else {
            Style::default().dim()
        };
        let block = Block::default()
            .title("User Prompts")
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.is_empty() {
            return;
        }

        if self.anchors.is_empty() {
            Paragraph::new(Text::from(vec![
                Line::from("No user prompts yet."),
                Line::from(""),
                Line::from("Add another turn"),
                Line::from("and reopen Ctrl+T."),
            ]))
            .render(inner, buf);
            return;
        }

        let available_height = inner.height.max(1) as usize;
        let selected = self
            .selected_anchor
            .unwrap_or_else(|| self.anchors.len().saturating_sub(1));
        let heights: Vec<usize> = self
            .anchors
            .iter()
            .enumerate()
            .map(|(idx, anchor)| {
                self.anchor_paragraph(
                    anchor,
                    inner.width,
                    Some(idx) == self.selected_anchor,
                )
                .line_count(inner.width)
                .min(MAX_ANCHOR_LINES)
            })
            .collect();

        let mut start = selected;
        let mut used_before = 0usize;
        let before_budget = available_height / 2;
        while start > 0 {
            let next_height = heights[start - 1];
            if used_before + next_height > before_budget {
                break;
            }
            used_before += next_height;
            start -= 1;
        }

        let mut end = start;
        let mut used_height = 0usize;
        while end < self.anchors.len() && used_height + heights[end] <= available_height {
            used_height += heights[end];
            end += 1;
        }
        while end < self.anchors.len() && start < selected {
            used_height = used_height.saturating_sub(heights[start]);
            start += 1;
            while end < self.anchors.len() && used_height + heights[end] <= available_height {
                used_height += heights[end];
                end += 1;
            }
        }

        let mut y = inner.y;
        for idx in start..end {
            let height = heights[idx] as u16;
            let row_area = Rect::new(inner.x, y, inner.width, height);
            self.anchor_paragraph(&self.anchors[idx], inner.width, Some(idx) == self.selected_anchor)
                .render(row_area, buf);
            y = y.saturating_add(height);
            if y >= inner.bottom() {
                break;
            }
        }
    }

    fn anchor_paragraph(
        &self,
        anchor: &TranscriptAnchor,
        width: u16,
        is_selected: bool,
    ) -> Paragraph<'static> {
        let prefix = if is_selected { "› " } else { "  " };
        let label_budget = width
            .saturating_mul(MAX_ANCHOR_LINES as u16)
            .saturating_sub(prefix.len() as u16)
            .max(8) as usize;
        let style = if is_selected {
            Style::default().cyan().bold()
        } else {
            Style::default()
        };
        Paragraph::new(format!(
            "{prefix}{}",
            truncate_preview_text(&anchor.label, label_budget)
        ))
        .style(style)
        .wrap(Wrap { trim: false })
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);
        if self.anchors_are_effectively_visible(top) {
            let anchor_width = self.anchor_pane_width(top);
            let [transcript_area, anchors_area] = Layout::horizontal([
                Constraint::Min(top.width.saturating_sub(anchor_width)),
                Constraint::Length(anchor_width),
            ])
            .areas(top);
            self.view.render(transcript_area, buf);
            self.render_anchor_shell(anchors_area, buf);
        } else {
            self.view.render(top, buf);
        }
        self.render_hints(bottom, buf);
    }
}

impl TranscriptOverlay {
    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => match key_event {
                e if KEY_Q.is_press(e) || KEY_CTRL_C.is_press(e) || KEY_CTRL_T.is_press(e) => {
                    self.is_done = true;
                    Ok(())
                }
                e if KEY_A.is_press(e) => {
                    self.anchors_visible = !self.anchors_visible;
                    if !self.anchors_visible {
                        self.mode = TranscriptBrowseMode::FreeForm;
                    }
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                    Ok(())
                }
                e if KEY_TAB.is_press(e) && self.anchors_visible => {
                    self.mode = match self.mode {
                        TranscriptBrowseMode::FreeForm => {
                            if let Some(anchor_idx) = self.selected_anchor {
                                self.select_anchor(anchor_idx);
                            }
                            TranscriptBrowseMode::ByPrompt
                        }
                        TranscriptBrowseMode::ByPrompt => TranscriptBrowseMode::FreeForm,
                    };
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                    Ok(())
                }
                e if KEY_E.is_press(e) => {
                    self.expand_all = !self.expand_all;
                    self.rebuild_renderables();
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                    Ok(())
                }
                other => match self.mode {
                    TranscriptBrowseMode::FreeForm => self.view.handle_key_event(tui, other),
                    TranscriptBrowseMode::ByPrompt => self.handle_anchor_key_event(tui, other),
                },
            },
            TuiEvent::Draw => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    #[cfg(test)]
    pub(crate) fn committed_cell_count(&self) -> usize {
        self.cells.len()
    }
}

pub(crate) struct StaticOverlay {
    view: PagerView,
    is_done: bool,
}

impl StaticOverlay {
    pub(crate) fn with_title(lines: Vec<Line<'static>>, title: String) -> Self {
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        Self::with_renderables(vec![Box::new(CachedRenderable::new(paragraph))], title)
    }

    pub(crate) fn with_renderables(renderables: Vec<Box<dyn Renderable>>, title: String) -> Self {
        Self {
            view: PagerView::new(renderables, title, 0),
            is_done: false,
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(line1, buf, PAGER_KEY_HINTS);
        let pairs: Vec<(&[KeyBinding], &str)> = vec![(&[KEY_Q], "to quit")];
        render_key_hints(line2, buf, &pairs);
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);
        self.view.render(top, buf);
        self.render_hints(bottom, buf);
    }
}

impl StaticOverlay {
    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => match key_event {
                e if KEY_Q.is_press(e) || KEY_CTRL_C.is_press(e) => {
                    self.is_done = true;
                    Ok(())
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Draw => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }
}

fn render_offset_content(
    area: Rect,
    buf: &mut Buffer,
    renderable: &dyn Renderable,
    scroll_offset: u16,
) -> u16 {
    let height = renderable.desired_height(area.width);
    let mut tall_buf = Buffer::empty(Rect::new(
        0,
        0,
        area.width,
        height.min(area.height + scroll_offset),
    ));
    renderable.render(*tall_buf.area(), &mut tall_buf);
    let copy_height = area
        .height
        .min(tall_buf.area().height.saturating_sub(scroll_offset));
    for y in 0..copy_height {
        let src_y = y + scroll_offset;
        for x in 0..area.width {
            buf[(area.x + x, area.y + y)] = tall_buf[(x, src_y)].clone();
        }
    }

    copy_height
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::ExecCommandSource;
    use codex_protocol::protocol::ReviewDecision;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::exec_cell::CommandOutput;
    use crate::history_cell;
    use crate::history_cell::HistoryCell;
    use crate::history_cell::new_patch_event;
    use codex_protocol::parse_command::ParsedCommand;
    use codex_protocol::protocol::FileChange;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Text;

    #[derive(Debug)]
    struct TestCell {
        lines: Vec<Line<'static>>,
    }

    impl crate::history_cell::HistoryCell for TestCell {
        fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines.clone()
        }

        fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines.clone()
        }
    }

    fn paragraph_block(label: &str, lines: usize) -> Box<dyn Renderable> {
        let text = Text::from(
            (0..lines)
                .map(|i| Line::from(format!("{label}{i}")))
                .collect::<Vec<_>>(),
        );
        Box::new(Paragraph::new(text)) as Box<dyn Renderable>
    }

    #[test]
    fn edit_prev_hint_is_visible() {
        let mut overlay = TranscriptOverlay::new(vec![Arc::new(TestCell {
            lines: vec![Line::from("hello")],
        })]);

        // Render into a wide buffer so the footer hints aren't truncated.
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let s = buffer_to_text(&buf, area);
        assert!(
            s.contains("edit prev"),
            "expected 'edit prev' hint in overlay footer, got: {s:?}"
        );
    }

    #[test]
    fn edit_next_hint_is_visible_when_highlighted() {
        let mut overlay = TranscriptOverlay::new(vec![Arc::new(TestCell {
            lines: vec![Line::from("hello")],
        })]);
        overlay.set_highlight_cell(Some(0));

        // Render into a wide buffer so the footer hints aren't truncated.
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let s = buffer_to_text(&buf, area);
        assert!(
            s.contains("edit next"),
            "expected 'edit next' hint in overlay footer, got: {s:?}"
        );
    }

    #[test]
    fn transcript_overlay_snapshot_basic() {
        // Prepare a transcript overlay with a few lines
        let mut overlay = TranscriptOverlay::new(vec![
            Arc::new(TestCell {
                lines: vec![Line::from("alpha")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("beta")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("gamma")],
            }),
        ]);
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn transcript_overlay_renders_live_tail() {
        let mut overlay = TranscriptOverlay::new(vec![Arc::new(TestCell {
            lines: vec![Line::from("alpha")],
        })]);
        overlay.sync_live_tail(
            40,
            Some(ActiveCellTranscriptKey {
                revision: 1,
                is_stream_continuation: false,
                animation_tick: None,
            }),
            |_| Some(vec![Line::from("tail")]),
        );

        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn transcript_overlay_sync_live_tail_is_noop_for_identical_key() {
        let mut overlay = TranscriptOverlay::new(vec![Arc::new(TestCell {
            lines: vec![Line::from("alpha")],
        })]);

        let calls = std::cell::Cell::new(0usize);
        let key = ActiveCellTranscriptKey {
            revision: 1,
            is_stream_continuation: false,
            animation_tick: None,
        };

        overlay.sync_live_tail(40, Some(key), |_| {
            calls.set(calls.get() + 1);
            Some(vec![Line::from("tail")])
        });
        overlay.sync_live_tail(40, Some(key), |_| {
            calls.set(calls.get() + 1);
            Some(vec![Line::from("tail2")])
        });

        assert_eq!(calls.get(), 1);
    }

    fn buffer_to_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let symbol = buf[(x, y)].symbol();
                if symbol.is_empty() {
                    out.push(' ');
                } else {
                    out.push(symbol.chars().next().unwrap_or(' '));
                }
            }
            // Trim trailing spaces for stability.
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn transcript_overlay_apply_patch_scroll_vt100_clears_previous_page() {
        let cwd = PathBuf::from("/repo");
        let mut cells: Vec<Arc<dyn HistoryCell>> = Vec::new();

        let mut approval_changes = HashMap::new();
        approval_changes.insert(
            PathBuf::from("foo.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        let approval_cell: Arc<dyn HistoryCell> = Arc::new(new_patch_event(approval_changes, &cwd));
        cells.push(approval_cell);

        let mut apply_changes = HashMap::new();
        apply_changes.insert(
            PathBuf::from("foo.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        let apply_begin_cell: Arc<dyn HistoryCell> = Arc::new(new_patch_event(apply_changes, &cwd));
        cells.push(apply_begin_cell);

        let apply_end_cell: Arc<dyn HistoryCell> = history_cell::new_approval_decision_cell(
            vec!["ls".into()],
            ReviewDecision::Approved,
            history_cell::ApprovalDecisionActor::User,
        )
        .into();
        cells.push(apply_end_cell);

        let mut exec_cell = crate::exec_cell::new_active_exec_command(
            "exec-1".into(),
            vec!["bash".into(), "-lc".into(), "ls".into()],
            vec![ParsedCommand::Unknown { cmd: "ls".into() }],
            ExecCommandSource::Agent,
            None,
            true,
        );
        exec_cell.complete_call(
            "exec-1",
            CommandOutput {
                exit_code: 0,
                aggregated_output: "src\nREADME.md\n".into(),
                formatted_output: "src\nREADME.md\n".into(),
            },
            Duration::from_millis(420),
        );
        let exec_cell: Arc<dyn HistoryCell> = Arc::new(exec_cell);
        cells.push(exec_cell);

        let mut overlay = TranscriptOverlay::new(cells);
        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);

        overlay.render(area, &mut buf);
        overlay.view.scroll_offset = 0;
        overlay.render(area, &mut buf);

        let snapshot = buffer_to_text(&buf, area);
        assert_snapshot!("transcript_overlay_apply_patch_scroll_vt100", snapshot);
    }

    #[test]
    fn transcript_overlay_keeps_scroll_pinned_at_bottom() {
        let mut overlay = TranscriptOverlay::new(
            (0..20)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        assert!(
            overlay.view.is_scrolled_to_bottom(),
            "expected initial render to leave view at bottom"
        );

        overlay.insert_cell(Arc::new(TestCell {
            lines: vec!["tail".into()],
        }));

        assert_eq!(overlay.view.scroll_offset, usize::MAX);
    }

    #[test]
    fn transcript_overlay_preserves_manual_scroll_position() {
        let mut overlay = TranscriptOverlay::new(
            (0..20)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        overlay.view.scroll_offset = 0;

        overlay.insert_cell(Arc::new(TestCell {
            lines: vec!["tail".into()],
        }));

        assert_eq!(overlay.view.scroll_offset, 0);
    }

    #[test]
    fn static_overlay_snapshot_basic() {
        // Prepare a static overlay with a few lines and a title
        let mut overlay = StaticOverlay::with_title(
            vec!["one".into(), "two".into(), "three".into()],
            "S T A T I C".to_string(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    /// Render transcript overlay and return visible line numbers (`line-NN`) in order.
    fn transcript_line_numbers(overlay: &mut TranscriptOverlay, area: Rect) -> Vec<usize> {
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let content_area = overlay.view.content_area(top);

        let mut nums = Vec::new();
        for y in content_area.y..content_area.bottom() {
            let mut line = String::new();
            for x in content_area.x..content_area.right() {
                line.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            if let Some(n) = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("line-"))
                .and_then(|s| s.parse().ok())
            {
                nums.push(n);
            }
        }
        nums
    }

    #[test]
    fn transcript_overlay_paging_is_continuous_and_round_trips() {
        let mut overlay = TranscriptOverlay::new(
            (0..50)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line-{i:02}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let area = Rect::new(0, 0, 40, 15);

        // Prime layout so last_content_height is populated and paging uses the real content height.
        let mut buf = Buffer::empty(area);
        overlay.view.scroll_offset = 0;
        overlay.render(area, &mut buf);
        let page_height = overlay.view.page_height(area);

        // Scenario 1: starting from the top, PageDown should show the next page of content.
        overlay.view.scroll_offset = 0;
        let page1 = transcript_line_numbers(&mut overlay, area);
        let page1_len = page1.len();
        let expected_page1: Vec<usize> = (0..page1_len).collect();
        assert_eq!(
            page1, expected_page1,
            "first page should start at line-00 and show a full page of content"
        );

        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let page2 = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            page2.len(),
            page1_len,
            "second page should have the same number of visible lines as the first page"
        );
        let expected_page2_first = *page1.last().unwrap() + 1;
        assert_eq!(
            page2[0], expected_page2_first,
            "second page after PageDown should immediately follow the first page"
        );

        // Scenario 2: from an interior offset (start=3), PageDown then PageUp should round-trip.
        let interior_offset = 3usize;
        overlay.view.scroll_offset = interior_offset;
        let before = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let _ = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_sub(page_height);
        let after = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            before, after,
            "PageDown+PageUp from interior offset ({interior_offset}) should round-trip"
        );

        // Scenario 3: from the top of the second page, PageUp then PageDown should round-trip.
        overlay.view.scroll_offset = page_height;
        let before2 = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_sub(page_height);
        let _ = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let after2 = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            before2, after2,
            "PageUp+PageDown from the top of the second page should round-trip"
        );
    }

    #[test]
    fn static_overlay_wraps_long_lines() {
        let mut overlay = StaticOverlay::with_title(
            vec!["a very long line that should wrap when rendered within a narrow pager overlay width".into()],
            "S T A T I C".to_string(),
        );
        let mut term = Terminal::new(TestBackend::new(24, 8)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn pager_view_content_height_counts_renderables() {
        let pv = PagerView::new(
            vec![paragraph_block("a", 2), paragraph_block("b", 3)],
            "T".to_string(),
            0,
        );

        assert_eq!(pv.content_height(80), 5);
    }

    #[test]
    fn pager_view_ensure_chunk_visible_scrolls_down_when_needed() {
        let mut pv = PagerView::new(
            vec![
                paragraph_block("a", 1),
                paragraph_block("b", 3),
                paragraph_block("c", 3),
            ],
            "T".to_string(),
            0,
        );
        let area = Rect::new(0, 0, 20, 8);

        pv.scroll_offset = 0;
        let content_area = pv.content_area(area);
        pv.ensure_chunk_visible(2, content_area);

        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);
        let rendered = buffer_to_text(&buf, area);

        assert!(
            rendered.contains("c0"),
            "expected chunk top in view: {rendered:?}"
        );
        assert!(
            rendered.contains("c1"),
            "expected chunk middle in view: {rendered:?}"
        );
        assert!(
            rendered.contains("c2"),
            "expected chunk bottom in view: {rendered:?}"
        );
    }

    #[test]
    fn pager_view_ensure_chunk_visible_scrolls_up_when_needed() {
        let mut pv = PagerView::new(
            vec![
                paragraph_block("a", 2),
                paragraph_block("b", 3),
                paragraph_block("c", 3),
            ],
            "T".to_string(),
            0,
        );
        let area = Rect::new(0, 0, 20, 3);

        pv.scroll_offset = 6;
        pv.ensure_chunk_visible(0, area);

        assert_eq!(pv.scroll_offset, 0);
    }

    #[test]
    fn pager_view_is_scrolled_to_bottom_accounts_for_wrapped_height() {
        let mut pv = PagerView::new(vec![paragraph_block("a", 10)], "T".to_string(), 0);
        let area = Rect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);

        pv.render(area, &mut buf);

        assert!(
            !pv.is_scrolled_to_bottom(),
            "expected view to report not at bottom when offset < max"
        );

        pv.scroll_offset = usize::MAX;
        pv.render(area, &mut buf);

        assert!(
            pv.is_scrolled_to_bottom(),
            "expected view to report at bottom after scrolling to end"
        );
    }
}
