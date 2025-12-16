use codex_protocol::protocol::SandboxPolicy;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use tokio::fs;

/// Configuration for a custom agent loaded from a markdown file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAgent {
    /// The agent name (derived from filename stem).
    pub name: String,
    /// Full path to the markdown file.
    pub path: PathBuf,
    /// The agent's system prompt (markdown body without frontmatter).
    pub instructions: String,
    /// Optional description shown in UI.
    pub description: Option<String>,
    /// Optional model override for this agent.
    pub model: Option<String>,
    /// Optional sandbox policy setting (defaults to "read-only" if not specified).
    pub sandbox: Option<String>,
}

/// Return the default agents directory: `$CODEX_HOME/agents`.
/// If `CODEX_HOME` cannot be resolved, returns `None`.
pub fn default_agents_dir() -> Option<PathBuf> {
    crate::config::find_codex_home()
        .ok()
        .map(|home| home.join("agents"))
}

/// Discover agent files in the given directory, returning entries sorted by name.
/// Non-files are ignored. If the directory does not exist or cannot be read, returns empty.
pub async fn discover_agents_in(dir: &Path) -> Vec<CustomAgent> {
    discover_agents_in_excluding(dir, &HashSet::new()).await
}

/// Discover agent files in the given directory, excluding any with names in `exclude`.
/// Returns entries sorted by name. Non-files are ignored. Missing/unreadable dir yields empty.
pub async fn discover_agents_in_excluding(
    dir: &Path,
    exclude: &HashSet<String>,
) -> Vec<CustomAgent> {
    let mut out: Vec<CustomAgent> = Vec::new();
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return out,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let is_file_like = fs::metadata(&path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false);
        if !is_file_like {
            continue;
        }
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
        let content = match fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (description, model, sandbox, body) = parse_agent_frontmatter(&content);
        out.push(CustomAgent {
            name,
            path,
            instructions: body,
            description,
            model,
            sandbox,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse optional YAML-like frontmatter at the beginning of `content`.
/// Supported keys:
/// - `description`: short description shown in UI
/// - `model`: model to use for this agent (e.g., "claude-3-5-sonnet-20241022")
/// - `sandbox`: sandbox policy ("read-only", "workspace-write", etc.)
/// Returns (description, model, sandbox, body_without_frontmatter).
fn parse_agent_frontmatter(
    content: &str,
) -> (Option<String>, Option<String>, Option<String>, String) {
    let mut segments = content.split_inclusive('\n');
    let Some(first_segment) = segments.next() else {
        return (None, None, None, String::new());
    };
    let first_line = first_segment.trim_end_matches(['\r', '\n']);
    if first_line.trim() != "---" {
        return (None, None, None, content.to_string());
    }

    let mut desc: Option<String> = None;
    let mut model: Option<String> = None;
    let mut sandbox: Option<String> = None;
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
                "model" => model = Some(val),
                "sandbox" => sandbox = Some(val),
                _ => {}
            }
        }

        consumed += segment.len();
    }

    if !frontmatter_closed {
        // Unterminated frontmatter: treat input as-is.
        return (None, None, None, content.to_string());
    }

    let body = if consumed >= content.len() {
        String::new()
    } else {
        content[consumed..].to_string()
    };
    (desc, model, sandbox, body)
}

/// Parse a sandbox policy string into a SandboxPolicy enum.
/// Supported values: "read-only", "workspace-write", "danger-full-access"
/// Returns None if the string doesn't match any known policy.
pub fn parse_sandbox_policy(s: &str) -> Option<SandboxPolicy> {
    match s.trim().to_ascii_lowercase().as_str() {
        "read-only" => Some(SandboxPolicy::ReadOnly),
        "workspace-write" => Some(SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }),
        "danger-full-access" => Some(SandboxPolicy::DangerFullAccess),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn empty_when_dir_missing() {
        let tmp = tempdir().expect("create TempDir");
        let missing = tmp.path().join("nope");
        let found = discover_agents_in(&missing).await;
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn discovers_and_sorts_files() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        fs::write(dir.join("reviewer.md"), b"Review the code").unwrap();
        fs::write(dir.join("analyzer.md"), b"Analyze the code").unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        let found = discover_agents_in(dir).await;
        let names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["analyzer", "reviewer"]);
    }

    #[tokio::test]
    async fn excludes_builtins() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        fs::write(dir.join("review.md"), b"ignored").unwrap();
        fs::write(dir.join("custom.md"), b"ok").unwrap();
        let mut exclude = HashSet::new();
        exclude.insert("review".to_string());
        let found = discover_agents_in_excluding(dir, &exclude).await;
        let names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["custom"]);
    }

    #[tokio::test]
    async fn skips_non_utf8_files() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        // Valid UTF-8 file
        fs::write(dir.join("good.md"), b"hello").unwrap();
        // Invalid UTF-8 content in .md file
        fs::write(dir.join("bad.md"), vec![0xFF, 0xFE, b'\n']).unwrap();
        let found = discover_agents_in(dir).await;
        let names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["good"]);
    }

    #[tokio::test]
    async fn parses_frontmatter_and_strips_from_body() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        let file = dir.join("specialist.md");
        let text = "---\ndescription: \"Code review specialist\"\nmodel: \"claude-3-5-sonnet-20241022\"\nsandbox: \"read-only\"\n---\nYou are a code review expert.";
        fs::write(&file, text).unwrap();

        let found = discover_agents_in(dir).await;
        assert_eq!(found.len(), 1);
        let agent = &found[0];
        assert_eq!(agent.name, "specialist");
        assert_eq!(agent.description.as_deref(), Some("Code review specialist"));
        assert_eq!(agent.model.as_deref(), Some("claude-3-5-sonnet-20241022"));
        assert_eq!(agent.sandbox.as_deref(), Some("read-only"));
        assert_eq!(agent.instructions, "You are a code review expert.");
    }

    #[test]
    fn parse_frontmatter_preserves_body_newlines() {
        let content = "---\r\ndescription: \"Test agent\"\r\nmodel: \"gpt-4\"\r\n---\r\nFirst line\r\nSecond line\r\n";
        let (desc, model, _sandbox, body) = parse_agent_frontmatter(content);
        assert_eq!(desc.as_deref(), Some("Test agent"));
        assert_eq!(model.as_deref(), Some("gpt-4"));
        assert_eq!(body, "First line\r\nSecond line\r\n");
    }

    #[test]
    fn test_parse_sandbox_policy() {
        assert!(matches!(
            parse_sandbox_policy("read-only"),
            Some(SandboxPolicy::ReadOnly)
        ));
        assert!(matches!(
            parse_sandbox_policy("workspace-write"),
            Some(SandboxPolicy::WorkspaceWrite { .. })
        ));
        assert!(matches!(
            parse_sandbox_policy("danger-full-access"),
            Some(SandboxPolicy::DangerFullAccess)
        ));
        assert!(parse_sandbox_policy("invalid").is_none());
    }
}
