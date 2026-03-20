#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParsedOsc8<'a> {
    pub(crate) destination: &'a str,
    pub(crate) text: &'a str,
}

const OSC8_OPEN_PREFIX: &str = "\u{1b}]8;;";
const OSC8_CLOSE: &str = "\u{1b}]8;;\u{7}";

pub(crate) fn sanitize_osc8_url(destination: &str) -> String {
    destination
        .chars()
        .filter(|&c| c != '\x1B' && c != '\x07')
        .collect()
}

pub(crate) fn osc8_hyperlink<S: AsRef<str>>(destination: &str, text: S) -> String {
    let safe_destination = sanitize_osc8_url(destination);
    if safe_destination.is_empty() {
        return text.as_ref().to_string();
    }

    format!(
        "{OSC8_OPEN_PREFIX}{safe_destination}\u{7}{}{OSC8_CLOSE}",
        text.as_ref()
    )
}

pub(crate) fn parse_osc8_hyperlink(text: &str) -> Option<ParsedOsc8<'_>> {
    let after_open = text.strip_prefix(OSC8_OPEN_PREFIX)?;
    let url_end = after_open.find('\x07')?;
    let destination = &after_open[..url_end];
    let after_destination = &after_open[url_end + 1..];
    let label = after_destination.strip_suffix(OSC8_CLOSE)?;
    Some(ParsedOsc8 {
        destination,
        text: label,
    })
}

pub(crate) fn strip_osc8_hyperlinks(text: &str) -> String {
    let mut remaining = text;
    let mut rendered = String::new();

    while let Some(open_pos) = remaining.find(OSC8_OPEN_PREFIX) {
        rendered.push_str(&remaining[..open_pos]);
        let after_open = &remaining[open_pos + OSC8_OPEN_PREFIX.len()..];
        let Some(url_end) = after_open.find('\x07') else {
            rendered.push_str(&remaining[open_pos..]);
            return rendered;
        };
        let after_url = &after_open[url_end + 1..];
        let Some(close_pos) = after_url.find(OSC8_CLOSE) else {
            rendered.push_str(&remaining[open_pos..]);
            return rendered;
        };
        rendered.push_str(&after_url[..close_pos]);
        remaining = &after_url[close_pos + OSC8_CLOSE.len()..];
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
    fn strips_wrapped_text() {
        let wrapped = format!("See {}", osc8_hyperlink("https://example.com", "docs"));
        assert_eq!(strip_osc8_hyperlinks(&wrapped), "See docs");
    }
}
