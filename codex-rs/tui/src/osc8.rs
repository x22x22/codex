#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParsedOsc8<'a> {
    pub(crate) destination: &'a str,
    pub(crate) text: &'a str,
}

const OSC8_OPEN_PREFIX: &str = "\u{1b}]8;;";
const OSC8_CLOSE: &str = "\u{1b}]8;;\u{7}";

/// Strip bytes that could terminate or escape an OSC 8 destination early.
pub(crate) fn sanitize_osc8_destination(destination: &str) -> String {
    destination
        .chars()
        .filter(|&c| c != '\x1B' && c != '\x07')
        .collect()
}

/// Wrap visible text in a single OSC 8 hyperlink span.
pub(crate) fn osc8_hyperlink(destination: &str, text: &str) -> String {
    let safe_destination = sanitize_osc8_destination(destination);
    if safe_destination.is_empty() {
        return text.to_string();
    }

    format!("{OSC8_OPEN_PREFIX}{safe_destination}\u{7}{text}{OSC8_CLOSE}")
}

/// Parse a string that consists entirely of one OSC 8 hyperlink span.
pub(crate) fn parse_osc8_hyperlink(text: &str) -> Option<ParsedOsc8<'_>> {
    let after_open = text.strip_prefix(OSC8_OPEN_PREFIX)?;
    let destination_end = after_open.find('\x07')?;
    let destination = &after_open[..destination_end];
    let after_destination = &after_open[destination_end + 1..];
    let label = after_destination.strip_suffix(OSC8_CLOSE)?;
    Some(ParsedOsc8 {
        destination,
        text: label,
    })
}

/// Strip OSC 8 wrappers while preserving visible label text.
pub(crate) fn strip_osc8_hyperlinks(text: &str) -> String {
    let mut remaining = text;
    let mut rendered = String::new();

    while let Some(open_pos) = remaining.find(OSC8_OPEN_PREFIX) {
        rendered.push_str(&remaining[..open_pos]);
        let after_open = &remaining[open_pos + OSC8_OPEN_PREFIX.len()..];
        let Some(destination_end) = after_open.find('\x07') else {
            rendered.push_str(&remaining[open_pos..]);
            return rendered;
        };
        let after_destination = &after_open[destination_end + 1..];
        let Some(close_pos) = after_destination.find(OSC8_CLOSE) else {
            rendered.push_str(&remaining[open_pos..]);
            return rendered;
        };
        rendered.push_str(&after_destination[..close_pos]);
        remaining = &after_destination[close_pos + OSC8_CLOSE.len()..];
    }

    rendered.push_str(remaining);
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_wrapped_text() {
        let wrapped = osc8_hyperlink("https://example.com", "docs");
        let parsed = parse_osc8_hyperlink(&wrapped).expect("expected osc8 span");
        assert_eq!(parsed.destination, "https://example.com");
        assert_eq!(parsed.text, "docs");
    }

    #[test]
    fn parse_rejects_mixed_text() {
        let wrapped = format!("See {}", osc8_hyperlink("https://example.com", "docs"));
        assert_eq!(parse_osc8_hyperlink(&wrapped), None);
    }

    #[test]
    fn strips_wrapped_text() {
        let wrapped = format!("See {}", osc8_hyperlink("https://example.com", "docs"));
        assert_eq!(strip_osc8_hyperlinks(&wrapped), "See docs");
    }

    #[test]
    fn strips_multiple_wrapped_segments() {
        let wrapped = format!(
            "{} {}",
            osc8_hyperlink("https://example.com/docs", "docs"),
            osc8_hyperlink("https://example.com/api", "api")
        );
        assert_eq!(strip_osc8_hyperlinks(&wrapped), "docs api");
    }

    #[test]
    fn malformed_sequences_are_preserved() {
        let malformed = "\u{1b}]8;;https://example.com\u{7}docs";
        assert_eq!(strip_osc8_hyperlinks(malformed), malformed);
    }
}
