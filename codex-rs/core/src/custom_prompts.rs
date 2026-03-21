use codex_exec_server::ExecutorFileSystem;
#[cfg(test)]
use codex_exec_server::LocalFileSystem;
use codex_protocol::custom_prompts::CustomPrompt;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

/// Return the default prompts directory: `$CODEX_HOME/prompts`.
/// If `CODEX_HOME` cannot be resolved, returns `None`.
pub fn default_prompts_dir() -> Option<PathBuf> {
    crate::config::find_codex_home()
        .ok()
        .map(|home| prompts_dir(&home))
}

pub fn prompts_dir(codex_home: &Path) -> PathBuf {
    codex_home.join("prompts")
}

/// Discover prompt files in the given directory, returning entries sorted by name.
/// Non-files are ignored. If the directory does not exist or cannot be read, returns empty.
#[cfg(test)]
pub async fn discover_prompts_in(dir: &Path) -> Vec<CustomPrompt> {
    discover_prompts_in_excluding(dir, &HashSet::new()).await
}

/// Discover prompt files in the given directory, excluding any with names in `exclude`.
/// Returns entries sorted by name. Non-files are ignored. Missing/unreadable dir yields empty.
#[cfg(test)]
pub async fn discover_prompts_in_excluding(
    dir: &Path,
    exclude: &HashSet<String>,
) -> Vec<CustomPrompt> {
    let Ok(dir) = AbsolutePathBuf::from_absolute_path(dir) else {
        return Vec::new();
    };
    discover_prompts_in_excluding_with_filesystem(&dir, exclude, &LocalFileSystem).await
}

pub async fn discover_prompts_in_with_filesystem<F>(
    dir: &AbsolutePathBuf,
    filesystem: &F,
) -> Vec<CustomPrompt>
where
    F: ExecutorFileSystem + ?Sized,
{
    discover_prompts_in_excluding_with_filesystem(dir, &HashSet::new(), filesystem).await
}

pub async fn discover_prompts_in_excluding_with_filesystem<F>(
    dir: &AbsolutePathBuf,
    exclude: &HashSet<String>,
    filesystem: &F,
) -> Vec<CustomPrompt>
where
    F: ExecutorFileSystem + ?Sized,
{
    let mut out: Vec<CustomPrompt> = Vec::new();
    let entries = match filesystem.read_directory(dir).await {
        Ok(entries) => entries,
        Err(_) => return out,
    };

    for entry in entries {
        if !entry.is_file {
            continue;
        }
        let path = dir.as_path().join(&entry.file_name);
        // Only include Markdown files with a .md extension.
        let is_md = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !is_md {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if exclude.contains(&name) {
            continue;
        }
        let Ok(path) = AbsolutePathBuf::try_from(path) else {
            continue;
        };
        let content = match filesystem.read_file(&path).await {
            Ok(contents) => match String::from_utf8(contents) {
                Ok(contents) => contents,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        let (description, argument_hint, body) = parse_frontmatter(&content);
        out.push(CustomPrompt {
            name,
            path: path.to_path_buf(),
            content: body,
            description,
            argument_hint,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse optional YAML-like frontmatter at the beginning of `content`.
/// Supported keys:
/// - `description`: short description shown in the slash popup
/// - `argument-hint` or `argument_hint`: brief hint string shown after the description
///   Returns (description, argument_hint, body_without_frontmatter).
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>, String) {
    let mut segments = content.split_inclusive('\n');
    let Some(first_segment) = segments.next() else {
        return (None, None, String::new());
    };
    let first_line = first_segment.trim_end_matches(['\r', '\n']);
    if first_line.trim() != "---" {
        return (None, None, content.to_string());
    }

    let mut desc: Option<String> = None;
    let mut hint: Option<String> = None;
    let mut frontmatter_closed = false;
    let mut consumed = first_segment.len();

    for segment in segments {
        let line = segment.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();

        if trimmed == "---" {
            frontmatter_closed = true;
            consumed += segment.len();
            break;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            consumed += segment.len();
            continue;
        }

        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let mut val = v.trim().to_string();
            if val.len() >= 2 {
                let bytes = val.as_bytes();
                let first = bytes[0];
                let last = bytes[bytes.len() - 1];
                if (first == b'\"' && last == b'\"') || (first == b'\'' && last == b'\'') {
                    val = val[1..val.len().saturating_sub(1)].to_string();
                }
            }
            match key.as_str() {
                "description" => desc = Some(val),
                "argument-hint" | "argument_hint" => hint = Some(val),
                _ => {}
            }
        }

        consumed += segment.len();
    }

    if !frontmatter_closed {
        // Unterminated frontmatter: treat input as-is.
        return (None, None, content.to_string());
    }

    let body = if consumed >= content.len() {
        String::new()
    } else {
        content[consumed..].to_string()
    };
    (desc, hint, body)
}

#[cfg(test)]
#[path = "custom_prompts_tests.rs"]
mod tests;
