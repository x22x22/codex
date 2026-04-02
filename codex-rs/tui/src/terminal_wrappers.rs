use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Measure display width while treating terminal control sequences as zero-width wrappers.
pub(crate) fn display_width(text: &str) -> usize {
    let mut width = 0usize;
    let mut parser = TokenParser::new(text);

    while let Some(token) = parser.next_token() {
        if let Token::Visible(grapheme) = token {
            width = width.saturating_add(UnicodeWidthStr::width(grapheme));
        }
    }

    width
}

/// Strip terminal control sequences, keeping only visible text.
pub(crate) fn strip(text: &str) -> String {
    if !contains_control_intro(text) {
        return text.to_string();
    }

    let mut visible = String::new();
    let mut parser = TokenParser::new(text);
    while let Some(token) = parser.next_token() {
        if let Token::Visible(grapheme) = token {
            visible.push_str(grapheme);
        }
    }
    visible
}

/// Truncate by visible grapheme width while preserving wrapper open/close semantics.
pub(crate) fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 || text.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut wrappers = ActiveWrappers::default();
    let mut used_width = 0usize;
    let mut overflowed = false;
    let mut parser = TokenParser::new(text);

    while let Some(token) = parser.next_token() {
        match token {
            Token::Control(control) => {
                if overflowed {
                    wrappers.consume_trailing_close_control(&control, &mut out);
                } else {
                    wrappers.consume_control(&control, Some(&mut out));
                }
            }
            Token::Visible(grapheme) => {
                if overflowed {
                    continue;
                }

                let grapheme_width = UnicodeWidthStr::width(grapheme);
                if used_width.saturating_add(grapheme_width) > max_width {
                    overflowed = true;
                    continue;
                }

                out.push_str(grapheme);
                used_width = used_width.saturating_add(grapheme_width);
            }
        }
    }

    wrappers.append_closers(&mut out);
    out
}

/// Slice a visible-text byte range out of `text`, rewrapping any active zero-width wrappers.
pub(crate) fn slice_visible_range(text: &str, visible_range: Range<usize>) -> String {
    if visible_range.start >= visible_range.end || text.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut wrappers = ActiveWrappers::default();
    let mut visible_cursor = 0usize;
    let mut started = false;
    let mut parser = TokenParser::new(text);

    while let Some(token) = parser.next_token() {
        match token {
            Token::Control(control) => {
                if visible_cursor < visible_range.start {
                    wrappers.consume_control(&control, None);
                } else if visible_cursor < visible_range.end {
                    if started {
                        wrappers.consume_control(&control, Some(&mut out));
                    } else {
                        wrappers.consume_control(&control, None);
                    }
                } else if started && wrappers.consume_trailing_close_control(&control, &mut out) {
                    continue;
                } else {
                    break;
                }
            }
            Token::Visible(grapheme) => {
                let next_visible_cursor = visible_cursor.saturating_add(grapheme.len());
                if next_visible_cursor <= visible_range.start {
                    visible_cursor = next_visible_cursor;
                    continue;
                }
                if visible_cursor >= visible_range.end {
                    break;
                }
                if !started {
                    wrappers.append_openers(&mut out);
                    started = true;
                }
                out.push_str(grapheme);
                visible_cursor = next_visible_cursor;
            }
        }
    }

    if started {
        wrappers.append_closers(&mut out);
    }

    out
}

#[derive(Debug)]
struct ParsedControl<'a> {
    raw: &'a str,
    kind: ControlKind,
    closer: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ControlKind {
    Osc8Open,
    Osc8Close,
    SgrOpen,
    SgrClose,
    Other,
}

#[derive(Debug, Clone)]
struct ActiveWrapper {
    kind: ActiveWrapperKind,
    opener: String,
    closer: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ActiveWrapperKind {
    Osc8,
    Sgr,
}

#[derive(Debug, Default)]
struct ActiveWrappers {
    stack: Vec<ActiveWrapper>,
}

impl ActiveWrappers {
    fn append_openers(&self, out: &mut String) {
        for wrapper in &self.stack {
            out.push_str(&wrapper.opener);
        }
    }

    fn append_closers(&self, out: &mut String) {
        for wrapper in self.stack.iter().rev() {
            out.push_str(&wrapper.closer);
        }
    }

    fn consume_trailing_close_control(
        &mut self,
        control: &ParsedControl<'_>,
        out: &mut String,
    ) -> bool {
        match control.kind {
            ControlKind::Osc8Close | ControlKind::SgrClose => {
                self.consume_control(control, Some(out));
                true
            }
            ControlKind::Osc8Open | ControlKind::SgrOpen | ControlKind::Other => false,
        }
    }

    fn consume_control(&mut self, control: &ParsedControl<'_>, mut out: Option<&mut String>) {
        match control.kind {
            ControlKind::Osc8Open => {
                if let Some(wrapper_index) = self
                    .stack
                    .iter()
                    .rposition(|wrapper| wrapper.kind == ActiveWrapperKind::Osc8)
                {
                    let prior = self.stack.remove(wrapper_index);
                    if let Some(out) = out.as_mut() {
                        out.push_str(&prior.closer);
                    }
                }

                if let Some(out) = out.as_mut() {
                    out.push_str(control.raw);
                }
                if let Some(closer) = control.closer.clone() {
                    self.stack.push(ActiveWrapper {
                        kind: ActiveWrapperKind::Osc8,
                        opener: control.raw.to_string(),
                        closer,
                    });
                }
            }
            ControlKind::Osc8Close => {
                if let Some(out) = out.as_mut() {
                    out.push_str(control.raw);
                }
                if let Some(wrapper_index) = self
                    .stack
                    .iter()
                    .rposition(|wrapper| wrapper.kind == ActiveWrapperKind::Osc8)
                {
                    self.stack.remove(wrapper_index);
                }
            }
            ControlKind::SgrOpen => {
                if let Some(out) = out.as_mut() {
                    out.push_str(control.raw);
                }
                if let Some(closer) = control.closer.clone() {
                    self.stack.push(ActiveWrapper {
                        kind: ActiveWrapperKind::Sgr,
                        opener: control.raw.to_string(),
                        closer,
                    });
                }
            }
            ControlKind::SgrClose => {
                if let Some(out) = out.as_mut() {
                    out.push_str(control.raw);
                }
                self.stack
                    .retain(|wrapper| wrapper.kind != ActiveWrapperKind::Sgr);
            }
            ControlKind::Other => {
                if let Some(out) = out.as_mut() {
                    out.push_str(control.raw);
                }
            }
        }
    }
}

enum Token<'a> {
    Control(ParsedControl<'a>),
    Visible(&'a str),
}

struct TokenParser<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> TokenParser<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, position: 0 }
    }

    fn next_token(&mut self) -> Option<Token<'a>> {
        if self.position >= self.text.len() {
            return None;
        }

        if let Some((control, next_position)) = parse_control(self.text, self.position) {
            self.position = next_position;
            return Some(Token::Control(control));
        }

        let grapheme = self.text[self.position..].graphemes(true).next()?;
        self.position += grapheme.len();
        Some(Token::Visible(grapheme))
    }
}

fn parse_control(text: &str, position: usize) -> Option<(ParsedControl<'_>, usize)> {
    let tail = text.get(position..)?;
    if tail.starts_with("\u{1b}]") {
        return Some(parse_osc_control(text, position, "\u{1b}]", "\u{1b}\\"));
    }
    if tail.starts_with("\u{9d}") {
        return Some(parse_osc_control(text, position, "\u{9d}", "\u{9c}"));
    }
    if tail.starts_with("\u{1b}[") {
        return Some(parse_csi_control(text, position, "\u{1b}[", "\u{1b}[0m"));
    }
    if tail.starts_with("\u{9b}") {
        return Some(parse_csi_control(text, position, "\u{9b}", "\u{9b}0m"));
    }
    if tail.starts_with("\u{1b}\\") {
        return Some((
            ParsedControl {
                raw: &text[position..position + 2],
                kind: ControlKind::Other,
                closer: None,
            },
            position + 2,
        ));
    }
    if tail.starts_with("\u{9c}") {
        return Some((
            ParsedControl {
                raw: &text[position..position + "\u{9c}".len()],
                kind: ControlKind::Other,
                closer: None,
            },
            position + "\u{9c}".len(),
        ));
    }
    if tail.starts_with("\u{1b}") {
        return Some((
            ParsedControl {
                raw: &text[position..position + 1],
                kind: ControlKind::Other,
                closer: None,
            },
            position + 1,
        ));
    }
    None
}

fn parse_osc_control<'a>(
    text: &'a str,
    position: usize,
    introducer: &str,
    default_terminator: &str,
) -> (ParsedControl<'a>, usize) {
    let payload_start = position + introducer.len();
    let mut scan_position = payload_start;
    let mut payload_end = text.len();
    let mut sequence_end = text.len();
    let mut terminator = default_terminator;

    while scan_position < text.len() {
        let tail = &text[scan_position..];
        if tail.starts_with('\u{7}') {
            payload_end = scan_position;
            sequence_end = scan_position + '\u{7}'.len_utf8();
            terminator = "\u{7}";
            break;
        }
        if tail.starts_with("\u{9c}") {
            payload_end = scan_position;
            sequence_end = scan_position + "\u{9c}".len();
            terminator = "\u{9c}";
            break;
        }
        if tail.starts_with("\u{1b}\\") {
            payload_end = scan_position;
            sequence_end = scan_position + 2;
            terminator = "\u{1b}\\";
            break;
        }

        let Some(ch) = tail.chars().next() else {
            break;
        };
        scan_position += ch.len_utf8();
    }

    let raw = &text[position..sequence_end];
    let payload = &text[payload_start..payload_end];
    let (kind, closer) = classify_osc_control(payload, introducer, terminator);
    (ParsedControl { raw, kind, closer }, sequence_end)
}

fn classify_osc_control(
    payload: &str,
    introducer: &str,
    terminator: &str,
) -> (ControlKind, Option<String>) {
    let Some(rest) = payload.strip_prefix("8;") else {
        return (ControlKind::Other, None);
    };
    let Some((_, destination)) = rest.split_once(';') else {
        return (ControlKind::Other, None);
    };

    if destination.is_empty() {
        (ControlKind::Osc8Close, None)
    } else {
        (
            ControlKind::Osc8Open,
            Some(format!("{introducer}8;;{terminator}")),
        )
    }
}

fn parse_csi_control<'a>(
    text: &'a str,
    position: usize,
    introducer: &str,
    closer: &str,
) -> (ParsedControl<'a>, usize) {
    let payload_start = position + introducer.len();
    let mut scan_position = payload_start;
    let mut payload_end = text.len();
    let mut sequence_end = text.len();
    let mut final_char = None;

    while scan_position < text.len() {
        let Some(ch) = text[scan_position..].chars().next() else {
            break;
        };
        if ('\u{40}'..='\u{7e}').contains(&ch) {
            payload_end = scan_position;
            sequence_end = scan_position + ch.len_utf8();
            final_char = Some(ch);
            break;
        }
        scan_position += ch.len_utf8();
    }

    let raw = &text[position..sequence_end];
    let kind = if final_char == Some('m') {
        let payload = &text[payload_start..payload_end];
        if is_sgr_reset(payload) {
            ControlKind::SgrClose
        } else {
            ControlKind::SgrOpen
        }
    } else {
        ControlKind::Other
    };
    let closer = (kind == ControlKind::SgrOpen).then(|| closer.to_string());
    (ParsedControl { raw, kind, closer }, sequence_end)
}

fn is_sgr_reset(payload: &str) -> bool {
    payload
        .split(';')
        .all(|param| param.is_empty() || param == "0")
}

fn contains_control_intro(text: &str) -> bool {
    text.contains('\u{1b}')
        || text.contains('\u{9b}')
        || text.contains('\u{9c}')
        || text.contains('\u{9d}')
}
