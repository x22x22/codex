use std::path::Path;

use codex_secrets::redact_secrets;

const MAX_PREVIEW_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SecurityRedactor;

impl SecurityRedactor {
    pub(crate) fn sanitize_command(self, command: &[String]) -> Option<String> {
        (!command.is_empty()).then(|| self.sanitize_text(&command.join(" ")))
    }

    pub(crate) fn sanitize_path(self, path: &Path) -> String {
        let display = dunce::simplified(path).display().to_string();
        self.sanitize_text(&display)
    }

    pub(crate) fn sanitize_text(self, text: &str) -> String {
        let redacted = redact_secrets(text.to_owned());
        self.truncate(redacted)
    }

    fn truncate(self, value: String) -> String {
        let mut chars = value.chars();
        let truncated: String = chars.by_ref().take(MAX_PREVIEW_CHARS).collect();
        if chars.next().is_some() {
            format!("{truncated}...")
        } else {
            value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SecurityRedactor;
    use pretty_assertions::assert_eq;

    #[test]
    fn redact_secrets_is_applied() {
        let redactor = SecurityRedactor;
        let value = redactor.sanitize_text("Bearer abcdefghijklmnopqrstuvwxyz123456");
        assert_eq!("Bearer [REDACTED_SECRET]", value);
    }

    #[test]
    fn values_are_truncated() {
        let redactor = SecurityRedactor;
        let value = redactor.sanitize_text(&"a".repeat(300));
        assert_eq!(259, value.len());
        assert!(value.ends_with("..."));
    }
}
