use crate::terminal_wrappers;
use ratatui::text::Line;
use ratatui::text::Span;

pub(crate) fn line_width(line: &Line<'_>) -> usize {
    line.iter()
        .map(|span| terminal_wrappers::display_width(span.content.as_ref()))
        .sum()
}

pub(crate) fn truncate_line_to_width(line: Line<'static>, max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from(Vec::<Span<'static>>::new());
    }

    let Line {
        style,
        alignment,
        spans,
    } = line;
    let mut used = 0usize;
    let mut spans_out: Vec<Span<'static>> = Vec::with_capacity(spans.len());

    for span in spans {
        let span_width = terminal_wrappers::display_width(span.content.as_ref());

        if span_width == 0 {
            spans_out.push(span);
            continue;
        }

        if used >= max_width {
            break;
        }

        if used + span_width <= max_width {
            used += span_width;
            spans_out.push(span);
            continue;
        }

        let style = span.style;
        let text = terminal_wrappers::truncate_to_width(span.content.as_ref(), max_width - used);
        if !text.is_empty() {
            spans_out.push(Span::styled(text, style));
        }

        break;
    }

    Line {
        style,
        alignment,
        spans: spans_out,
    }
}

/// Truncate a styled line to `max_width` and append an ellipsis on overflow.
///
/// Intended for short UI rows. This preserves a fast no-overflow path (width
/// pre-scan + return original line unchanged) and uses `truncate_line_to_width`
/// for the overflow case.
/// Performance should be reevaluated if using this method in loops/over larger content in the future.
pub(crate) fn truncate_line_with_ellipsis_if_overflow(
    line: Line<'static>,
    max_width: usize,
) -> Line<'static> {
    if max_width == 0 {
        return Line::from(Vec::<Span<'static>>::new());
    }

    if line_width(&line) <= max_width {
        return line;
    }

    let truncated = truncate_line_to_width(line, max_width.saturating_sub(1));
    let Line {
        style,
        alignment,
        mut spans,
    } = truncated;
    let ellipsis_style = spans.last().map(|span| span.style).unwrap_or_default();
    spans.push(Span::styled("…", ellipsis_style));
    Line {
        style,
        alignment,
        spans,
    }
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

    // These rows are sliced for tight status/UI slots, so wrapper payload bytes must not count
    // toward width and truncation must preserve the active link/color run around visible text.
    #[test]
    fn line_width_counts_only_visible_text_for_osc8_and_csi_wrappers() {
        let line = Line::from(vec![
            osc8_bel_hyperlink("https://example.com/docs", "docs").into(),
            " ".into(),
            csi_red_text("tail").into(),
        ]);

        assert_eq!(line_width(&line), 9);
    }

    #[test]
    fn line_width_counts_only_visible_text_for_st_terminated_osc8_wrappers() {
        let line = Line::from(vec![
            osc8_st_hyperlink("https://example.com/docs", "docs").into(),
        ]);

        assert_eq!(line_width(&line), 4);
    }

    #[test]
    fn line_width_counts_only_visible_text_for_osc8_params_and_c1_wrappers() {
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

        assert_eq!(line_width(&line), 13);
    }

    #[test]
    fn line_width_ignores_unterminated_c1_osc8_payload_bytes() {
        let line = Line::from("\u{9d}8;;https://example.com/docs docs");

        assert_eq!(line_width(&line), 0);
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
                "{}{}{}{}",
                "\u{1b}[31m",
                osc8_bel_open_with_params("id=docs:target=_blank", docs),
                "abcdef",
                "\u{1b}]8;;\u{7}\u{1b}[0m",
            )
            .into(),
        ]);

        let truncated = truncate_line_to_width(line, 3);

        assert_eq!(
            concat_line(&truncated),
            format!(
                "\u{1b}[31m{}\u{1b}]8;;\u{7}\u{1b}[0m",
                osc8_bel_open_with_params("id=docs:target=_blank", docs) + "abc"
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
