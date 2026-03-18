use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::analytics_client::AnalyticsEventsClient;
use crate::analytics_client::InvocationType;
use crate::analytics_client::SkillInvocation;
use crate::analytics_client::TrackEventsContext;
use crate::instructions::SkillInstructions;
use crate::mention_syntax::TOOL_MENTION_SIGIL;
use crate::mentions::build_skill_name_counts;
use crate::skills::SkillMetadata;
use codex_environment::Environment;
use codex_otel::SessionTelemetry;
use codex_protocol::models::ResponseItem;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio::fs;

#[derive(Debug, Default)]
pub(crate) struct SkillInjections {
    pub(crate) items: Vec<ResponseItem>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) async fn build_skill_injections(
    mentioned_skills: &[SkillMetadata],
    otel: Option<&SessionTelemetry>,
    analytics_client: &AnalyticsEventsClient,
    tracking: TrackEventsContext,
) -> SkillInjections {
    build_skill_injections_with_environment(
        mentioned_skills,
        None,
        otel,
        analytics_client,
        tracking,
    )
    .await
}

pub(crate) async fn build_skill_injections_with_environment(
    mentioned_skills: &[SkillMetadata],
    environment: Option<&Environment>,
    otel: Option<&SessionTelemetry>,
    analytics_client: &AnalyticsEventsClient,
    tracking: TrackEventsContext,
) -> SkillInjections {
    if mentioned_skills.is_empty() {
        return SkillInjections::default();
    }

    let mut result = SkillInjections {
        items: Vec::with_capacity(mentioned_skills.len()),
        warnings: Vec::new(),
    };
    let mut invocations = Vec::new();

    for skill in mentioned_skills {
        match read_skill_contents(skill, environment).await {
            Ok(contents) => {
                emit_skill_injected_metric(otel, skill, "ok");
                invocations.push(SkillInvocation {
                    skill_name: skill.name.clone(),
                    skill_scope: skill.scope,
                    skill_path: skill.path_to_skills_md.clone(),
                    invocation_type: InvocationType::Explicit,
                });
                result.items.push(ResponseItem::from(SkillInstructions {
                    name: skill.name.clone(),
                    path: skill.path_to_skills_md.to_string_lossy().into_owned(),
                    contents,
                }));
            }
            Err(err) => {
                emit_skill_injected_metric(otel, skill, "error");
                let message = format!(
                    "Failed to load skill {name} at {path}: {err:#}",
                    name = skill.name,
                    path = skill.path_to_skills_md.display()
                );
                result.warnings.push(message);
            }
        }
    }

    analytics_client.track_skill_invocations(tracking, invocations);

    result
}

async fn read_skill_contents(
    skill: &SkillMetadata,
    environment: Option<&Environment>,
) -> std::io::Result<String> {
    if skill.scope == codex_protocol::protocol::SkillScope::Repo
        && let Some(environment) = environment
    {
        let abs_path = AbsolutePathBuf::try_from(skill.path_to_skills_md.clone())
            .map_err(std::io::Error::other)?;
        let bytes = environment.get_filesystem().read_file(&abs_path).await?;
        return Ok(String::from_utf8_lossy(&bytes).to_string());
    }

    fs::read_to_string(&skill.path_to_skills_md).await
}

fn emit_skill_injected_metric(
    otel: Option<&SessionTelemetry>,
    skill: &SkillMetadata,
    status: &str,
) {
    let Some(otel) = otel else {
        return;
    };

    otel.counter(
        "codex.skill.injected",
        /*inc*/ 1,
        &[("status", status), ("skill", skill.name.as_str())],
    );
}

#[cfg(test)]
mod remote_environment_tests {
    use super::*;
    use crate::analytics_client::AnalyticsEventsClient;
    use crate::analytics_client::build_track_events_context;
    use crate::config::ConfigBuilder;
    use crate::test_support::auth_manager_from_auth;
    use crate::CodexAuth;
    use async_trait::async_trait;
    use codex_environment::CopyOptions;
    use codex_environment::CreateDirectoryOptions;
    use codex_environment::Environment;
    use codex_environment::ExecutorFileSystem;
    use codex_environment::FileMetadata;
    use codex_environment::FileSystemResult;
    use codex_environment::ReadDirectoryEntry;
    use codex_environment::RemoveOptions;
    use codex_protocol::protocol::SkillScope;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[derive(Clone)]
    struct RemappedFileSystem {
        local_root: AbsolutePathBuf,
        remote_root: AbsolutePathBuf,
    }

    impl RemappedFileSystem {
        fn new(local_root: &std::path::Path, remote_root: &std::path::Path) -> Self {
            Self {
                local_root: AbsolutePathBuf::try_from(local_root.to_path_buf()).unwrap(),
                remote_root: AbsolutePathBuf::try_from(remote_root.to_path_buf()).unwrap(),
            }
        }

        fn remap(&self, path: &AbsolutePathBuf) -> AbsolutePathBuf {
            let relative = path
                .as_path()
                .strip_prefix(self.local_root.as_path())
                .expect("path should stay under the local test root");
            AbsolutePathBuf::try_from(self.remote_root.as_path().join(relative)).unwrap()
        }
    }

    #[async_trait]
    impl ExecutorFileSystem for RemappedFileSystem {
        async fn read_file(&self, path: &AbsolutePathBuf) -> FileSystemResult<Vec<u8>> {
            tokio::fs::read(self.remap(path).as_path()).await
        }

        async fn write_file(
            &self,
            path: &AbsolutePathBuf,
            contents: Vec<u8>,
        ) -> FileSystemResult<()> {
            tokio::fs::write(self.remap(path).as_path(), contents).await
        }

        async fn create_directory(
            &self,
            path: &AbsolutePathBuf,
            options: CreateDirectoryOptions,
        ) -> FileSystemResult<()> {
            if options.recursive {
                tokio::fs::create_dir_all(self.remap(path).as_path()).await
            } else {
                tokio::fs::create_dir(self.remap(path).as_path()).await
            }
        }

        async fn get_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
            let metadata = tokio::fs::metadata(self.remap(path).as_path()).await?;
            Ok(FileMetadata {
                is_directory: metadata.is_dir(),
                is_file: metadata.is_file(),
                created_at_ms: 0,
                modified_at_ms: 0,
            })
        }

        async fn read_directory(
            &self,
            path: &AbsolutePathBuf,
        ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
            let mut entries = Vec::new();
            let mut read_dir = tokio::fs::read_dir(self.remap(path).as_path()).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
                entries.push(ReadDirectoryEntry {
                    file_name: entry.file_name().to_string_lossy().into_owned(),
                    is_directory: metadata.is_dir(),
                    is_file: metadata.is_file(),
                    is_symlink: metadata.file_type().is_symlink(),
                });
            }
            Ok(entries)
        }

        async fn remove(
            &self,
            path: &AbsolutePathBuf,
            options: RemoveOptions,
        ) -> FileSystemResult<()> {
            let remapped = self.remap(path);
            match tokio::fs::symlink_metadata(remapped.as_path()).await {
                Ok(metadata) => {
                    if metadata.is_dir() {
                        if options.recursive {
                            tokio::fs::remove_dir_all(remapped.as_path()).await
                        } else {
                            tokio::fs::remove_dir(remapped.as_path()).await
                        }
                    } else {
                        tokio::fs::remove_file(remapped.as_path()).await
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound && options.force => Ok(()),
                Err(err) => Err(err),
            }
        }

        async fn copy(
            &self,
            source_path: &AbsolutePathBuf,
            destination_path: &AbsolutePathBuf,
            _options: CopyOptions,
        ) -> FileSystemResult<()> {
            tokio::fs::copy(
                self.remap(source_path).as_path(),
                self.remap(destination_path).as_path(),
            )
            .await
            .map(|_| ())
        }
    }

    async fn analytics_client_for_test(codex_home: &TempDir) -> AnalyticsEventsClient {
        let config = Arc::new(
            ConfigBuilder::default()
                .codex_home(codex_home.path().to_path_buf())
                .build()
                .await
                .expect("config"),
        );
        let auth_manager = auth_manager_from_auth(CodexAuth::from_api_key("Test API Key"));
        AnalyticsEventsClient::new(config, auth_manager)
    }

    #[tokio::test]
    async fn build_skill_injections_with_environment_reads_remote_repo_skill_contents() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let local_root = tempfile::tempdir().expect("tempdir");
        let remote_root = tempfile::tempdir().expect("tempdir");

        fs::create_dir_all(local_root.path().join("repo/.agents/skills/demo")).unwrap();
        fs::create_dir_all(remote_root.path().join("repo/.agents/skills/demo")).unwrap();

        let local_skill_path = local_root.path().join("repo/.agents/skills/demo/SKILL.md");
        fs::write(
            &local_skill_path,
            "---\nname: demo\ndescription: local\n---\nLOCAL_SKILL_MARKER\n",
        )
        .unwrap();
        fs::write(
            remote_root.path().join("repo/.agents/skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: remote\n---\nREMOTE_SKILL_MARKER\n",
        )
        .unwrap();

        let mentioned_skills = vec![SkillMetadata {
            name: "demo".to_string(),
            description: "demo".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: local_skill_path,
            scope: SkillScope::Repo,
        }];
        let environment = Environment::new(Arc::new(RemappedFileSystem::new(
            local_root.path(),
            remote_root.path(),
        )));
        let analytics_client = analytics_client_for_test(&codex_home).await;

        let result = build_skill_injections_with_environment(
            &mentioned_skills,
            Some(&environment),
            None,
            &analytics_client,
            build_track_events_context(
                "gpt-test".to_string(),
                "thread".to_string(),
                "turn".to_string(),
            ),
        )
        .await;

        assert!(result.warnings.is_empty());
        assert_eq!(result.items.len(), 1);

        let serialized = serde_json::to_string(&result.items[0]).expect("serialize response item");
        assert!(serialized.contains("REMOTE_SKILL_MARKER"));
        assert!(!serialized.contains("LOCAL_SKILL_MARKER"));
    }
}

/// Collect explicitly mentioned skills from structured and text mentions.
///
/// Structured `UserInput::Skill` selections are resolved first by path against
/// enabled skills. Text inputs are then scanned to extract `$skill-name` tokens, and we
/// iterate `skills` in their existing order to preserve prior ordering semantics.
/// Explicit links are resolved by path and plain names are only used when the match
/// is unambiguous.
///
/// Complexity: `O(T + (N_s + N_t) * S)` time, `O(S + M)` space, where:
/// `S` = number of skills, `T` = total text length, `N_s` = number of structured skill inputs,
/// `N_t` = number of text inputs, `M` = max mentions parsed from a single text input.
pub(crate) fn collect_explicit_skill_mentions(
    inputs: &[UserInput],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<PathBuf>,
    connector_slug_counts: &HashMap<String, usize>,
) -> Vec<SkillMetadata> {
    let skill_name_counts = build_skill_name_counts(skills, disabled_paths).0;

    let selection_context = SkillSelectionContext {
        skills,
        disabled_paths,
        skill_name_counts: &skill_name_counts,
        connector_slug_counts,
    };
    let mut selected: Vec<SkillMetadata> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut blocked_plain_names: HashSet<String> = HashSet::new();

    for input in inputs {
        if let UserInput::Skill { name, path } = input {
            blocked_plain_names.insert(name.clone());
            if selection_context.disabled_paths.contains(path) || seen_paths.contains(path) {
                continue;
            }

            if let Some(skill) = selection_context
                .skills
                .iter()
                .find(|skill| skill.path_to_skills_md.as_path() == path.as_path())
            {
                seen_paths.insert(skill.path_to_skills_md.clone());
                seen_names.insert(skill.name.clone());
                selected.push(skill.clone());
            }
        }
    }

    for input in inputs {
        if let UserInput::Text { text, .. } = input {
            let mentioned_names = extract_tool_mentions(text);
            select_skills_from_mentions(
                &selection_context,
                &blocked_plain_names,
                &mentioned_names,
                &mut seen_names,
                &mut seen_paths,
                &mut selected,
            );
        }
    }

    selected
}

struct SkillSelectionContext<'a> {
    skills: &'a [SkillMetadata],
    disabled_paths: &'a HashSet<PathBuf>,
    skill_name_counts: &'a HashMap<String, usize>,
    connector_slug_counts: &'a HashMap<String, usize>,
}

pub(crate) struct ToolMentions<'a> {
    names: HashSet<&'a str>,
    paths: HashSet<&'a str>,
    plain_names: HashSet<&'a str>,
}

impl<'a> ToolMentions<'a> {
    fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }

    pub(crate) fn plain_names(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.plain_names.iter().copied()
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.paths.iter().copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolMentionKind {
    App,
    Mcp,
    Plugin,
    Skill,
    Other,
}

const APP_PATH_PREFIX: &str = "app://";
const MCP_PATH_PREFIX: &str = "mcp://";
const PLUGIN_PATH_PREFIX: &str = "plugin://";
const SKILL_PATH_PREFIX: &str = "skill://";
const SKILL_FILENAME: &str = "SKILL.md";

pub(crate) fn tool_kind_for_path(path: &str) -> ToolMentionKind {
    if path.starts_with(APP_PATH_PREFIX) {
        ToolMentionKind::App
    } else if path.starts_with(MCP_PATH_PREFIX) {
        ToolMentionKind::Mcp
    } else if path.starts_with(PLUGIN_PATH_PREFIX) {
        ToolMentionKind::Plugin
    } else if path.starts_with(SKILL_PATH_PREFIX) || is_skill_filename(path) {
        ToolMentionKind::Skill
    } else {
        ToolMentionKind::Other
    }
}

fn is_skill_filename(path: &str) -> bool {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file_name.eq_ignore_ascii_case(SKILL_FILENAME)
}

pub(crate) fn app_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix(APP_PATH_PREFIX)
        .filter(|value| !value.is_empty())
}

pub(crate) fn plugin_config_name_from_path(path: &str) -> Option<&str> {
    path.strip_prefix(PLUGIN_PATH_PREFIX)
        .filter(|value| !value.is_empty())
}

pub(crate) fn normalize_skill_path(path: &str) -> &str {
    path.strip_prefix(SKILL_PATH_PREFIX).unwrap_or(path)
}

/// Extract `$tool-name` mentions from a single text input.
///
/// Supports explicit resource links in the form `[$tool-name](resource path)`. When a
/// resource path is present, it is captured for exact path matching while also tracking
/// the name for fallback matching.
pub(crate) fn extract_tool_mentions(text: &str) -> ToolMentions<'_> {
    extract_tool_mentions_with_sigil(text, TOOL_MENTION_SIGIL)
}

pub(crate) fn extract_tool_mentions_with_sigil(text: &str, sigil: char) -> ToolMentions<'_> {
    let text_bytes = text.as_bytes();
    let mut mentioned_names: HashSet<&str> = HashSet::new();
    let mut mentioned_paths: HashSet<&str> = HashSet::new();
    let mut plain_names: HashSet<&str> = HashSet::new();

    let mut index = 0;
    while index < text_bytes.len() {
        let byte = text_bytes[index];
        if byte == b'['
            && let Some((name, path, end_index)) =
                parse_linked_tool_mention(text, text_bytes, index, sigil)
        {
            if !is_common_env_var(name) {
                if !matches!(
                    tool_kind_for_path(path),
                    ToolMentionKind::App | ToolMentionKind::Mcp | ToolMentionKind::Plugin
                ) {
                    mentioned_names.insert(name);
                }
                mentioned_paths.insert(path);
            }
            index = end_index;
            continue;
        }

        if byte != sigil as u8 {
            index += 1;
            continue;
        }

        let name_start = index + 1;
        let Some(first_name_byte) = text_bytes.get(name_start) else {
            index += 1;
            continue;
        };
        if !is_mention_name_char(*first_name_byte) {
            index += 1;
            continue;
        }

        let mut name_end = name_start + 1;
        while let Some(next_byte) = text_bytes.get(name_end)
            && is_mention_name_char(*next_byte)
        {
            name_end += 1;
        }

        let name = &text[name_start..name_end];
        if !is_common_env_var(name) {
            mentioned_names.insert(name);
            plain_names.insert(name);
        }
        index = name_end;
    }

    ToolMentions {
        names: mentioned_names,
        paths: mentioned_paths,
        plain_names,
    }
}

/// Select mentioned skills while preserving the order of `skills`.
fn select_skills_from_mentions(
    selection_context: &SkillSelectionContext<'_>,
    blocked_plain_names: &HashSet<String>,
    mentions: &ToolMentions<'_>,
    seen_names: &mut HashSet<String>,
    seen_paths: &mut HashSet<PathBuf>,
    selected: &mut Vec<SkillMetadata>,
) {
    if mentions.is_empty() {
        return;
    }

    let mention_skill_paths: HashSet<&str> = mentions
        .paths()
        .filter(|path| {
            !matches!(
                tool_kind_for_path(path),
                ToolMentionKind::App | ToolMentionKind::Mcp | ToolMentionKind::Plugin
            )
        })
        .map(normalize_skill_path)
        .collect();

    for skill in selection_context.skills {
        if selection_context
            .disabled_paths
            .contains(&skill.path_to_skills_md)
            || seen_paths.contains(&skill.path_to_skills_md)
        {
            continue;
        }

        let path_str = skill.path_to_skills_md.to_string_lossy();
        if mention_skill_paths.contains(path_str.as_ref()) {
            seen_paths.insert(skill.path_to_skills_md.clone());
            seen_names.insert(skill.name.clone());
            selected.push(skill.clone());
        }
    }

    for skill in selection_context.skills {
        if selection_context
            .disabled_paths
            .contains(&skill.path_to_skills_md)
            || seen_paths.contains(&skill.path_to_skills_md)
        {
            continue;
        }

        if blocked_plain_names.contains(skill.name.as_str()) {
            continue;
        }
        if !mentions.plain_names.contains(skill.name.as_str()) {
            continue;
        }

        let skill_count = selection_context
            .skill_name_counts
            .get(skill.name.as_str())
            .copied()
            .unwrap_or(0);
        let connector_count = selection_context
            .connector_slug_counts
            .get(&skill.name.to_ascii_lowercase())
            .copied()
            .unwrap_or(0);
        if skill_count != 1 || connector_count != 0 {
            continue;
        }

        if seen_names.insert(skill.name.clone()) {
            seen_paths.insert(skill.path_to_skills_md.clone());
            selected.push(skill.clone());
        }
    }
}

fn parse_linked_tool_mention<'a>(
    text: &'a str,
    text_bytes: &[u8],
    start: usize,
    sigil: char,
) -> Option<(&'a str, &'a str, usize)> {
    let sigil_index = start + 1;
    if text_bytes.get(sigil_index) != Some(&(sigil as u8)) {
        return None;
    }

    let name_start = sigil_index + 1;
    let first_name_byte = text_bytes.get(name_start)?;
    if !is_mention_name_char(*first_name_byte) {
        return None;
    }

    let mut name_end = name_start + 1;
    while let Some(next_byte) = text_bytes.get(name_end)
        && is_mention_name_char(*next_byte)
    {
        name_end += 1;
    }

    if text_bytes.get(name_end) != Some(&b']') {
        return None;
    }

    let mut path_start = name_end + 1;
    while let Some(next_byte) = text_bytes.get(path_start)
        && next_byte.is_ascii_whitespace()
    {
        path_start += 1;
    }
    if text_bytes.get(path_start) != Some(&b'(') {
        return None;
    }

    let mut path_end = path_start + 1;
    while let Some(next_byte) = text_bytes.get(path_end)
        && *next_byte != b')'
    {
        path_end += 1;
    }
    if text_bytes.get(path_end) != Some(&b')') {
        return None;
    }

    let path = text[path_start + 1..path_end].trim();
    if path.is_empty() {
        return None;
    }

    let name = &text[name_start..name_end];
    Some((name, path, path_end + 1))
}

fn is_common_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "PATH"
            | "HOME"
            | "USER"
            | "SHELL"
            | "PWD"
            | "TMPDIR"
            | "TEMP"
            | "TMP"
            | "LANG"
            | "TERM"
            | "XDG_CONFIG_HOME"
    )
}

#[cfg(test)]
fn text_mentions_skill(text: &str, skill_name: &str) -> bool {
    if skill_name.is_empty() {
        return false;
    }

    let text_bytes = text.as_bytes();
    let skill_bytes = skill_name.as_bytes();

    for (index, byte) in text_bytes.iter().copied().enumerate() {
        if byte != b'$' {
            continue;
        }

        let name_start = index + 1;
        let Some(rest) = text_bytes.get(name_start..) else {
            continue;
        };
        if !rest.starts_with(skill_bytes) {
            continue;
        }

        let after_index = name_start + skill_bytes.len();
        let after = text_bytes.get(after_index).copied();
        if after.is_none_or(|b| !is_mention_name_char(b)) {
            return true;
        }
    }

    false
}

fn is_mention_name_char(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b':')
}

#[cfg(test)]
#[path = "injection_tests.rs"]
mod tests;
