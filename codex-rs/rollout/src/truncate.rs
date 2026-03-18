use codex_protocol::protocol::TruncationPolicy as ProtocolTruncationPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TruncationPolicy {
    Bytes(usize),
    Tokens(usize),
}

impl From<TruncationPolicy> for ProtocolTruncationPolicy {
    fn from(value: TruncationPolicy) -> Self {
        match value {
            TruncationPolicy::Bytes(bytes) => Self::Bytes(bytes),
            TruncationPolicy::Tokens(tokens) => Self::Tokens(tokens),
        }
    }
}

pub(crate) fn truncate_text(content: &str, policy: TruncationPolicy) -> String {
    match policy {
        TruncationPolicy::Bytes(max_bytes) => truncate_with_byte_budget(content, max_bytes),
        TruncationPolicy::Tokens(tokens) => {
            truncate_with_byte_budget(content, tokens.saturating_mul(4))
        }
    }
}

fn truncate_with_byte_budget(s: &str, max_bytes: usize) -> String {
    if s.is_empty() {
        return String::new();
    }
    if max_bytes == 0 {
        return format!("…{} chars truncated…", s.chars().count());
    }
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let total_chars = s.chars().count();
    let left_budget = max_bytes / 2;
    let right_budget = max_bytes - left_budget;
    let (removed_chars, left, right) = split_string(s, left_budget, right_budget);
    let marker = format!(
        "…{} chars truncated…",
        total_chars.saturating_sub(left.chars().count() + right.chars().count() + removed_chars)
            + removed_chars
    );
    let mut out = String::with_capacity(left.len() + marker.len() + right.len());
    out.push_str(left);
    out.push_str(&marker);
    out.push_str(right);
    out
}

fn split_string(s: &str, beginning_bytes: usize, end_bytes: usize) -> (usize, &str, &str) {
    let len = s.len();
    let tail_start_target = len.saturating_sub(end_bytes);
    let mut prefix_end = 0usize;
    let mut suffix_start = len;
    let mut removed_chars = 0usize;
    let mut suffix_started = false;

    for (idx, ch) in s.char_indices() {
        let char_end = idx + ch.len_utf8();
        if char_end <= beginning_bytes {
            prefix_end = char_end;
            continue;
        }
        if idx >= tail_start_target {
            if !suffix_started {
                suffix_start = idx;
                suffix_started = true;
            }
            continue;
        }
        removed_chars = removed_chars.saturating_add(1);
    }

    if suffix_start < prefix_end {
        suffix_start = prefix_end;
    }

    (removed_chars, &s[..prefix_end], &s[suffix_start..])
}
