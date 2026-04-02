use crate::terminal_wrappers;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use std::collections::BTreeSet;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub(crate) struct FieldFormatter {
    indent: &'static str,
    label_width: usize,
    value_offset: usize,
    value_indent: String,
}

impl FieldFormatter {
    pub(crate) const INDENT: &'static str = " ";

    pub(crate) fn from_labels<S>(labels: impl IntoIterator<Item = S>) -> Self
    where
        S: AsRef<str>,
    {
        let label_width = labels
            .into_iter()
            .map(|label| UnicodeWidthStr::width(label.as_ref()))
            .max()
            .unwrap_or(0);
        let indent_width = UnicodeWidthStr::width(Self::INDENT);
        let value_offset = indent_width + label_width + 1 + 3;

        Self {
            indent: Self::INDENT,
            label_width,
            value_offset,
            value_indent: " ".repeat(value_offset),
        }
    }

    pub(crate) fn line(
        &self,
        label: &'static str,
        value_spans: Vec<Span<'static>>,
    ) -> Line<'static> {
        Line::from(self.full_spans(label, value_spans))
    }

    pub(crate) fn continuation(&self, mut spans: Vec<Span<'static>>) -> Line<'static> {
        let mut all_spans = Vec::with_capacity(spans.len() + 1);
        all_spans.push(Span::from(self.value_indent.clone()).dim());
        all_spans.append(&mut spans);
        Line::from(all_spans)
    }

    pub(crate) fn value_width(&self, available_inner_width: usize) -> usize {
        available_inner_width.saturating_sub(self.value_offset)
    }

    pub(crate) fn full_spans(
        &self,
        label: &str,
        mut value_spans: Vec<Span<'static>>,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::with_capacity(value_spans.len() + 1);
        spans.push(self.label_span(label));
        spans.append(&mut value_spans);
        spans
    }

    fn label_span(&self, label: &str) -> Span<'static> {
        let mut buf = String::with_capacity(self.value_offset);
        buf.push_str(self.indent);

        buf.push_str(label);
        buf.push(':');

        let label_width = UnicodeWidthStr::width(label);
        let padding = 3 + self.label_width.saturating_sub(label_width);
        for _ in 0..padding {
            buf.push(' ');
        }

        Span::from(buf).dim()
    }
}

pub(crate) fn push_label(labels: &mut Vec<String>, seen: &mut BTreeSet<String>, label: &str) {
    if seen.contains(label) {
        return;
    }

    let owned = label.to_string();
    seen.insert(owned.clone());
    labels.push(owned);
}

pub(crate) fn line_display_width(line: &Line<'static>) -> usize {
    line.iter()
        .map(|span| terminal_wrappers::display_width(span.content.as_ref()))
        .sum()
}

pub(crate) fn truncate_line_to_width(line: Line<'static>, max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from(Vec::<Span<'static>>::new());
    }

    let mut used = 0usize;
    let mut spans_out: Vec<Span<'static>> = Vec::new();

    for span in line.spans {
        let text = span.content.into_owned();
        let style = span.style;
        let span_width = terminal_wrappers::display_width(text.as_str());

        if span_width == 0 {
            spans_out.push(Span::styled(text, style));
            continue;
        }

        if used >= max_width {
            break;
        }

        if used + span_width <= max_width {
            used += span_width;
            spans_out.push(Span::styled(text, style));
            continue;
        }

        let truncated = terminal_wrappers::truncate_to_width(&text, max_width - used);
        if !truncated.is_empty() {
            spans_out.push(Span::styled(truncated, style));
        }

        break;
    }

    Line::from(spans_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn concat_line(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn osc8_bel_hyperlink(destination: &str, text: &str) -> String {
        format!("\u{1b}]8;;{destination}\u{7}{text}\u{1b}]8;;\u{7}")
    }

    fn osc8_st_hyperlink(destination: &str, text: &str) -> String {
        format!("\u{1b}]8;;{destination}\u{1b}\\{text}\u{1b}]8;;\u{1b}\\")
    }

    fn osc8_bel_hyperlink_with_params(params: &str, destination: &str, text: &str) -> String {
        format!("\u{1b}]8;{params};{destination}\u{7}{text}\u{1b}]8;;\u{7}")
    }

    fn osc8_bel_open_with_params(params: &str, destination: &str) -> String {
        format!("\u{1b}]8;{params};{destination}\u{7}")
    }

    fn c1_osc8_hyperlink(destination: &str, text: &str) -> String {
        format!("\u{9d}8;;{destination}\u{9c}{text}\u{9d}8;;\u{9c}")
    }

    fn csi_red_text(text: &str) -> String {
        format!("\u{1b}[31m{text}\u{1b}[0m")
    }

    fn c1_csi_red_text(text: &str) -> String {
        format!("\u{9b}31m{text}\u{9b}0m")
    }

    // Status lines reuse the same truncation contract as generic rows: preserve wrapper state,
    // but size and cut by visible grapheme width only.
    #[test]
    fn line_display_width_counts_only_visible_text_for_osc8_and_csi_wrappers() {
        let line = Line::from(vec![
            osc8_bel_hyperlink("https://example.com/docs", "docs").into(),
            " ".into(),
            csi_red_text("tail").into(),
        ]);

        assert_eq!(line_display_width(&line), 9);
    }

    #[test]
    fn line_display_width_counts_only_visible_text_for_st_terminated_osc8_wrappers() {
        let line = Line::from(vec![
            osc8_st_hyperlink("https://example.com/docs", "docs").into(),
        ]);

        assert_eq!(line_display_width(&line), 4);
    }

    #[test]
    fn line_display_width_counts_only_visible_text_for_osc8_params_and_c1_wrappers() {
        let line = Line::from(vec![
            osc8_bel_hyperlink_with_params(
                "id=docs:target=_blank",
                "https://example.com/docs",
                "docs",
            )
            .into(),
            " ".into(),
            c1_osc8_hyperlink("https://example.com/logs", "log").into(),
            " ".into(),
            c1_csi_red_text("tail").into(),
        ]);

        assert_eq!(line_display_width(&line), 13);
    }

    #[test]
    fn line_display_width_ignores_unterminated_c1_osc8_payload_bytes() {
        let line = Line::from("\u{9d}8;;https://example.com/docs docs");

        assert_eq!(line_display_width(&line), 0);
    }

    #[test]
    fn truncate_line_to_width_preserves_osc8_wrapper_around_partial_visible_text() {
        let line = Line::from(vec![
            "go ".into(),
            osc8_bel_hyperlink("https://example.com/docs", "abcdef").into(),
        ]);

        let truncated = truncate_line_to_width(line, 5);

        assert_eq!(
            concat_line(&truncated),
            format!(
                "go {}",
                osc8_bel_hyperlink("https://example.com/docs", "ab")
            )
        );
    }

    #[test]
    fn truncate_line_to_width_preserves_csi_wrapper_around_partial_visible_text() {
        let line = Line::from(vec![csi_red_text("abcdef").into()]);

        let truncated = truncate_line_to_width(line, 3);

        assert_eq!(concat_line(&truncated), csi_red_text("abc"));
    }

    #[test]
    fn truncate_line_to_width_preserves_full_grapheme_clusters() {
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        let line = Line::from(format!("{family} docs"));

        let truncated = truncate_line_to_width(line, 2);

        assert_eq!(concat_line(&truncated), family);
    }

    #[test]
    fn truncate_line_to_width_preserves_adjacent_osc8_wrappers_as_distinct_runs() {
        let docs = "https://example.com/docs";
        let logs = "https://example.com/logs";
        let line = Line::from(vec![
            osc8_bel_hyperlink(docs, "ab").into(),
            osc8_bel_hyperlink(logs, "cd").into(),
        ]);

        let truncated = truncate_line_to_width(line, 3);

        assert_eq!(
            concat_line(&truncated),
            format!(
                "{}{}",
                osc8_bel_hyperlink(docs, "ab"),
                osc8_bel_hyperlink(logs, "c")
            )
        );
    }

    #[test]
    fn truncate_line_to_width_preserves_osc8_id_params_and_nested_csi_wrappers() {
        let docs = "https://example.com/docs";
        let line = Line::from(vec![
            format!(
                "\u{1b}[31m{}abcdef\u{1b}]8;;\u{7}\u{1b}[0m",
                osc8_bel_open_with_params("id=docs:target=_blank", docs),
            )
            .into(),
        ]);

        let truncated = truncate_line_to_width(line, 3);

        assert_eq!(
            concat_line(&truncated),
            format!(
                "\u{1b}[31m{}abc\u{1b}]8;;\u{7}\u{1b}[0m",
                osc8_bel_open_with_params("id=docs:target=_blank", docs),
            ),
        );
    }

    #[test]
    fn truncate_line_to_width_preserves_osc8_retarget_and_stray_close_state_transitions() {
        let docs = "https://example.com/docs";
        let logs = "https://example.com/logs";
        let line = Line::from(vec![
            format!(
                "{}ab{}cd\u{1b}]8;;\u{7}\u{1b}]8;;\u{7}",
                osc8_bel_open_with_params("id=docs", docs),
                osc8_bel_open_with_params("id=logs", logs),
            )
            .into(),
        ]);

        let truncated = truncate_line_to_width(line, 3);

        assert_eq!(
            concat_line(&truncated),
            format!(
                "{}ab\u{1b}]8;;\u{7}{}c\u{1b}]8;;\u{7}\u{1b}]8;;\u{7}",
                osc8_bel_open_with_params("id=docs", docs),
                osc8_bel_open_with_params("id=logs", logs),
            ),
        );
    }

    #[test]
    fn truncate_line_to_width_handles_unterminated_escape_sequences_without_panicking() {
        let line = Line::from(vec![
            "\u{1b}]8;;https://example.com/docs\u{7}docs".into(),
            "\u{1b}[31mtail".into(),
        ]);

        let _ = truncate_line_to_width(line, 4);
    }
}
