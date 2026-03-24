use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use codex_protocol::ThreadId;
use codex_protocol::protocol::USER_MESSAGE_BEGIN;
use codex_protocol::user_input::UserInput;
use rand::Rng;
use tracing::debug;
use tracing::error;

use crate::parse_command::shlex_join;

const AUTO_THREAD_NAME_MAX_WORDS: usize = 4;
const AUTO_THREAD_NAME_MAX_CHARS: usize = 40;
const INITIAL_DELAY_MS: u64 = 200;
const BACKOFF_FACTOR: f64 = 2.0;
const AUTO_THREAD_NAME_STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "could", "for", "from", "help", "how",
    "i", "in", "into", "is", "it", "me", "my", "need", "of", "on", "or", "please", "should",
    "tell", "that", "the", "this", "to", "us", "want", "we", "what", "why", "with", "would", "you",
    "your",
];

/// Emit structured feedback metadata as key/value pairs.
///
/// This logs a tracing event with `target: "feedback_tags"`. If
/// `codex_feedback::CodexFeedback::metadata_layer()` is installed, these fields are captured and
/// later attached as tags when feedback is uploaded.
///
/// Values are wrapped with [`tracing::field::DebugValue`], so the expression only needs to
/// implement [`std::fmt::Debug`].
///
/// Example:
///
/// ```rust
/// codex_core::feedback_tags!(model = "gpt-5", cached = true);
/// codex_core::feedback_tags!(provider = provider_id, request_id = request_id);
/// ```
#[macro_export]
macro_rules! feedback_tags {
    ($( $key:ident = $value:expr ),+ $(,)?) => {
        ::tracing::info!(
            target: "feedback_tags",
            $( $key = ::tracing::field::debug(&$value) ),+
        );
    };
}

pub fn backoff(attempt: u64) -> Duration {
    let exp = BACKOFF_FACTOR.powi(attempt.saturating_sub(1) as i32);
    let base = (INITIAL_DELAY_MS as f64 * exp) as u64;
    let jitter = rand::rng().random_range(0.9..1.1);
    Duration::from_millis((base as f64 * jitter) as u64)
}

pub(crate) fn error_or_panic(message: impl std::string::ToString) {
    if cfg!(debug_assertions) {
        panic!("{}", message.to_string());
    } else {
        error!("{}", message.to_string());
    }
}

pub(crate) fn try_parse_error_message(text: &str) -> String {
    debug!("Parsing server error response: {}", text);
    let json = serde_json::from_str::<serde_json::Value>(text).unwrap_or_default();
    if let Some(error) = json.get("error")
        && let Some(message) = error.get("message")
        && let Some(message_str) = message.as_str()
    {
        return message_str.to_string();
    }
    if text.is_empty() {
        return "Unknown error".to_string();
    }
    text.to_string()
}

pub fn resolve_path(base: &Path, path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        path.clone()
    } else {
        base.join(path)
    }
}

/// Trim a thread name and return `None` if it is empty after trimming.
pub fn normalize_thread_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Mirror the desktop app's preference for very short task titles while keeping
/// CLI naming deterministic and local.
pub fn auto_thread_name_from_user_input(input: &[UserInput]) -> Option<String> {
    let text = input
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    auto_thread_name_from_text(&text)
}

fn auto_thread_name_from_text(text: &str) -> Option<String> {
    let excerpt = strip_user_message_prefix(text)
        .split(['\n', '\r'])
        .next()
        .unwrap_or(text)
        .split(['.', '?', '!'])
        .next()
        .unwrap_or(text)
        .trim();
    if excerpt.is_empty() {
        return None;
    }

    let preferred = collect_title_words(excerpt, true);
    let words = if preferred.len() >= 2 {
        preferred
    } else {
        collect_title_words(excerpt, false)
    };
    if words.is_empty() {
        return None;
    }

    let mut title = words.join(" ");
    uppercase_first_letter(&mut title);
    normalize_thread_name(&title)
}

fn collect_title_words(text: &str, drop_stop_words: bool) -> Vec<String> {
    let mut words = Vec::new();
    let mut total_len = 0usize;

    for raw_word in text.split_whitespace() {
        let word = clean_title_word(raw_word);
        if word.is_empty() {
            continue;
        }
        if drop_stop_words && is_auto_thread_name_stop_word(&word) {
            continue;
        }

        let next_len = if words.is_empty() {
            word.len()
        } else {
            total_len + 1 + word.len()
        };
        if words.len() >= AUTO_THREAD_NAME_MAX_WORDS
            || (!words.is_empty() && next_len > AUTO_THREAD_NAME_MAX_CHARS)
        {
            break;
        }

        total_len = next_len;
        words.push(word);
    }

    words
}

fn clean_title_word(word: &str) -> String {
    word.trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_string()
}

fn is_auto_thread_name_stop_word(word: &str) -> bool {
    AUTO_THREAD_NAME_STOP_WORDS
        .iter()
        .any(|stop_word| word.eq_ignore_ascii_case(stop_word))
}

fn uppercase_first_letter(text: &mut String) {
    let Some((idx, ch)) = text.char_indices().find(|(_, ch)| ch.is_alphabetic()) else {
        return;
    };
    let upper = ch.to_uppercase().to_string();
    text.replace_range(idx..idx + ch.len_utf8(), &upper);
}

fn strip_user_message_prefix(text: &str) -> &str {
    match text.find(USER_MESSAGE_BEGIN) {
        Some(idx) => text[idx + USER_MESSAGE_BEGIN.len()..].trim(),
        None => text.trim(),
    }
}

pub fn resume_command(thread_name: Option<&str>, thread_id: Option<ThreadId>) -> Option<String> {
    let resume_target = thread_name
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| thread_id.map(|thread_id| thread_id.to_string()));
    resume_target.map(|target| {
        let needs_double_dash = target.starts_with('-');
        let escaped = shlex_join(&[target]);
        if needs_double_dash {
            format!("codex resume -- {escaped}")
        } else {
            format!("codex resume {escaped}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_parse_error_message() {
        let text = r#"{
  "error": {
    "message": "Your refresh token has already been used to generate a new access token. Please try signing in again.",
    "type": "invalid_request_error",
    "param": null,
    "code": "refresh_token_reused"
  }
}"#;
        let message = try_parse_error_message(text);
        assert_eq!(
            message,
            "Your refresh token has already been used to generate a new access token. Please try signing in again."
        );
    }

    #[test]
    fn test_try_parse_error_message_no_error() {
        let text = r#"{"message": "test"}"#;
        let message = try_parse_error_message(text);
        assert_eq!(message, r#"{"message": "test"}"#);
    }

    #[test]
    fn feedback_tags_macro_compiles() {
        #[derive(Debug)]
        struct OnlyDebug;

        feedback_tags!(model = "gpt-5", cached = true, debug_only = OnlyDebug);
    }

    #[test]
    fn normalize_thread_name_trims_and_rejects_empty() {
        assert_eq!(normalize_thread_name("   "), None);
        assert_eq!(
            normalize_thread_name("  my thread  "),
            Some("my thread".to_string())
        );
    }

    #[test]
    fn auto_thread_name_prefers_short_content_words() {
        let input = vec![UserInput::Text {
            text: "Fix CI on Android app".to_string(),
            text_elements: Vec::new(),
        }];

        assert_eq!(
            auto_thread_name_from_user_input(&input),
            Some("Fix CI Android app".to_string())
        );
    }

    #[test]
    fn auto_thread_name_strips_user_message_prefix() {
        let text = format!("{USER_MESSAGE_BEGIN} can you explain MCP OAuth login flow?");
        assert_eq!(
            auto_thread_name_from_text(&text),
            Some("Explain MCP OAuth login".to_string())
        );
    }

    #[test]
    fn auto_thread_name_preserves_word_boundaries_between_text_items() {
        let input = vec![
            UserInput::Text {
                text: "fix CI".to_string(),
                text_elements: Vec::new(),
            },
            UserInput::Text {
                text: "on Android".to_string(),
                text_elements: Vec::new(),
            },
        ];

        assert_eq!(
            auto_thread_name_from_user_input(&input),
            Some("Fix CI Android".to_string())
        );
    }

    #[test]
    fn auto_thread_name_ignores_non_text_input() {
        let input = vec![UserInput::Image {
            image_url: "https://example.com/image.png".to_string(),
        }];

        assert_eq!(auto_thread_name_from_user_input(&input), None);
    }

    #[test]
    fn resume_command_prefers_name_over_id() {
        let thread_id = ThreadId::from_string("123e4567-e89b-12d3-a456-426614174000").unwrap();
        let command = resume_command(Some("my-thread"), Some(thread_id));
        assert_eq!(command, Some("codex resume my-thread".to_string()));
    }

    #[test]
    fn resume_command_with_only_id() {
        let thread_id = ThreadId::from_string("123e4567-e89b-12d3-a456-426614174000").unwrap();
        let command = resume_command(None, Some(thread_id));
        assert_eq!(
            command,
            Some("codex resume 123e4567-e89b-12d3-a456-426614174000".to_string())
        );
    }

    #[test]
    fn resume_command_with_no_name_or_id() {
        let command = resume_command(None, None);
        assert_eq!(command, None);
    }

    #[test]
    fn resume_command_quotes_thread_name_when_needed() {
        let command = resume_command(Some("-starts-with-dash"), None);
        assert_eq!(
            command,
            Some("codex resume -- -starts-with-dash".to_string())
        );

        let command = resume_command(Some("two words"), None);
        assert_eq!(command, Some("codex resume 'two words'".to_string()));

        let command = resume_command(Some("quote'case"), None);
        assert_eq!(command, Some("codex resume \"quote'case\"".to_string()));
    }
}
