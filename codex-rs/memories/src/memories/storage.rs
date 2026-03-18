use crate::memories::ensure_layout;
use crate::memories::raw_memories_file;
use crate::memories::rollout_summaries_dir;
use codex_state::Stage1Output;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use tracing::warn;
use uuid::Uuid;

/// Rebuild `raw_memories.md` from DB-backed stage-1 outputs.
pub async fn rebuild_raw_memories_file_from_memories(
    root: &Path,
    memories: &[Stage1Output],
    max_raw_memories_for_consolidation: usize,
) -> std::io::Result<()> {
    ensure_layout(root).await?;
    rebuild_raw_memories_file(root, memories, max_raw_memories_for_consolidation).await
}

/// Syncs canonical rollout summary files from DB-backed stage-1 output rows.
pub async fn sync_rollout_summaries_from_memories(
    root: &Path,
    memories: &[Stage1Output],
    max_raw_memories_for_consolidation: usize,
) -> std::io::Result<()> {
    ensure_layout(root).await?;

    let retained = retained_memories(memories, max_raw_memories_for_consolidation);
    let keep = retained
        .iter()
        .map(rollout_summary_file_stem)
        .collect::<HashSet<_>>();
    prune_rollout_summaries(root, &keep).await?;

    for memory in retained {
        write_rollout_summary_for_thread(root, memory).await?;
    }

    if retained.is_empty() {
        for file_name in ["MEMORY.md", "memory_summary.md"] {
            let path = root.join(file_name);
            if let Err(err) = tokio::fs::remove_file(path).await
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err);
            }
        }

        let skills_dir = root.join("skills");
        if let Err(err) = tokio::fs::remove_dir_all(skills_dir).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(err);
        }
    }

    Ok(())
}

async fn rebuild_raw_memories_file(
    root: &Path,
    memories: &[Stage1Output],
    max_raw_memories_for_consolidation: usize,
) -> std::io::Result<()> {
    let retained = retained_memories(memories, max_raw_memories_for_consolidation);
    let mut body = String::from("# Raw Memories\n\n");

    if retained.is_empty() {
        body.push_str("No raw memories yet.\n");
        return tokio::fs::write(raw_memories_file(root), body).await;
    }

    body.push_str("Merged stage-1 raw memories (latest first):\n\n");
    for memory in retained {
        writeln!(body, "## Thread `{}`", memory.thread_id).map_err(raw_memories_format_error)?;
        writeln!(
            body,
            "updated_at: {}",
            memory.source_updated_at.to_rfc3339()
        )
        .map_err(raw_memories_format_error)?;
        writeln!(body, "cwd: {}", memory.cwd.display()).map_err(raw_memories_format_error)?;
        writeln!(body, "rollout_path: {}", memory.rollout_path.display())
            .map_err(raw_memories_format_error)?;
        let rollout_summary_file = format!("{}.md", rollout_summary_file_stem(memory));
        writeln!(body, "rollout_summary_file: {rollout_summary_file}")
            .map_err(raw_memories_format_error)?;
        writeln!(body).map_err(raw_memories_format_error)?;
        body.push_str(memory.raw_memory.trim());
        body.push_str("\n\n");
    }

    tokio::fs::write(raw_memories_file(root), body).await
}

async fn prune_rollout_summaries(root: &Path, keep: &HashSet<String>) -> std::io::Result<()> {
    let dir_path = rollout_summaries_dir(root);
    let mut dir = match tokio::fs::read_dir(&dir_path).await {
        Ok(dir) => dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(".md") else {
            continue;
        };
        if !keep.contains(stem)
            && let Err(err) = tokio::fs::remove_file(&path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                "failed pruning outdated rollout summary {}: {err}",
                path.display()
            );
        }
    }

    Ok(())
}

async fn write_rollout_summary_for_thread(
    root: &Path,
    memory: &Stage1Output,
) -> std::io::Result<()> {
    let file_stem = rollout_summary_file_stem(memory);
    let path = rollout_summaries_dir(root).join(format!("{file_stem}.md"));

    let mut body = String::new();
    writeln!(body, "thread_id: {}", memory.thread_id).map_err(rollout_summary_format_error)?;
    writeln!(
        body,
        "updated_at: {}",
        memory.source_updated_at.to_rfc3339()
    )
    .map_err(rollout_summary_format_error)?;
    writeln!(body, "rollout_path: {}", memory.rollout_path.display())
        .map_err(rollout_summary_format_error)?;
    writeln!(body, "cwd: {}", memory.cwd.display()).map_err(rollout_summary_format_error)?;
    if let Some(git_branch) = memory.git_branch.as_deref() {
        writeln!(body, "git_branch: {git_branch}").map_err(rollout_summary_format_error)?;
    }
    writeln!(body).map_err(rollout_summary_format_error)?;
    body.push_str(&memory.rollout_summary);
    body.push('\n');

    tokio::fs::write(path, body).await
}

fn retained_memories(
    memories: &[Stage1Output],
    max_raw_memories_for_consolidation: usize,
) -> &[Stage1Output] {
    &memories[..memories.len().min(max_raw_memories_for_consolidation)]
}

fn raw_memories_format_error(err: std::fmt::Error) -> std::io::Error {
    std::io::Error::other(format!("format raw memories: {err}"))
}

fn rollout_summary_format_error(err: std::fmt::Error) -> std::io::Error {
    std::io::Error::other(format!("format rollout summary: {err}"))
}

pub fn rollout_summary_file_stem(memory: &Stage1Output) -> String {
    rollout_summary_file_stem_from_parts(
        memory.thread_id,
        memory.source_updated_at,
        memory.rollout_slug.as_deref(),
    )
}

pub fn rollout_summary_file_stem_from_parts(
    thread_id: codex_protocol::ThreadId,
    source_updated_at: chrono::DateTime<chrono::Utc>,
    rollout_slug: Option<&str>,
) -> String {
    const ROLLOUT_SLUG_MAX_LEN: usize = 60;
    const SHORT_HASH_ALPHABET: &[u8; 62] =
        b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const SHORT_HASH_SPACE: u32 = 14_776_336;

    let thread_id = thread_id.to_string();
    let (timestamp_fragment, short_hash_seed) = match Uuid::parse_str(&thread_id) {
        Ok(thread_uuid) => {
            let timestamp = thread_uuid
                .get_timestamp()
                .and_then(|uuid_timestamp| {
                    let (seconds, nanos) = uuid_timestamp.to_unix();
                    i64::try_from(seconds).ok().and_then(|secs| {
                        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
                    })
                })
                .unwrap_or(source_updated_at);
            let short_hash_seed = (thread_uuid.as_u128() & 0xFFFF_FFFF) as u32;
            (
                timestamp.format("%Y-%m-%dT%H-%M-%S").to_string(),
                short_hash_seed,
            )
        }
        Err(_) => {
            let mut short_hash_seed = 0u32;
            for byte in thread_id.bytes() {
                short_hash_seed = short_hash_seed
                    .wrapping_mul(31)
                    .wrapping_add(u32::from(byte));
            }
            (
                source_updated_at.format("%Y-%m-%dT%H-%M-%S").to_string(),
                short_hash_seed,
            )
        }
    };
    let mut short_hash_value = short_hash_seed % SHORT_HASH_SPACE;
    let mut short_hash = ['0'; 4];
    for digit in short_hash.iter_mut().rev() {
        let idx = usize::try_from(short_hash_value % 62).unwrap_or(0);
        *digit = char::from(SHORT_HASH_ALPHABET[idx]);
        short_hash_value /= 62;
    }

    let mut stem = format!(
        "{timestamp_fragment}-{}",
        short_hash.iter().collect::<String>()
    );
    let sanitized_slug = rollout_slug
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
        .map(|slug| {
            slug.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() {
                        ch.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .map(|slug| slug.chars().take(ROLLOUT_SLUG_MAX_LEN).collect::<String>())
        .filter(|slug| !slug.is_empty());
    if let Some(slug) = sanitized_slug {
        stem.push('-');
        stem.push_str(&slug);
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::rebuild_raw_memories_file_from_memories;
    use super::rollout_summary_file_stem;
    use super::rollout_summary_file_stem_from_parts;
    use super::sync_rollout_summaries_from_memories;
    use crate::memories::control::clear_memory_root_contents;
    use crate::memories::ensure_layout;
    use crate::memories::memory_root;
    use crate::memories::raw_memories_file;
    use crate::memories::rollout_summaries_dir;
    use chrono::TimeZone;
    use chrono::Utc;
    use codex_protocol::ThreadId;
    use codex_state::Stage1Output;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use tempfile::tempdir;

    const FIXED_PREFIX: &str = "2025-02-11T15-35-19-jqmb";

    fn stage1_output_with_slug(thread_id: ThreadId, rollout_slug: Option<&str>) -> Stage1Output {
        Stage1Output {
            thread_id,
            source_updated_at: Utc.timestamp_opt(123, 0).single().expect("timestamp"),
            raw_memory: "raw memory".to_string(),
            rollout_summary: "summary".to_string(),
            rollout_slug: rollout_slug.map(ToString::to_string),
            rollout_path: PathBuf::from("/tmp/rollout.jsonl"),
            cwd: PathBuf::from("/tmp/workspace"),
            git_branch: None,
            generated_at: Utc.timestamp_opt(124, 0).single().expect("timestamp"),
        }
    }

    fn fixed_thread_id() -> ThreadId {
        ThreadId::try_from("0194f5a6-89ab-7cde-8123-456789abcdef").expect("valid thread id")
    }

    #[test]
    fn memory_root_uses_shared_global_path() {
        let dir = tempdir().expect("tempdir");
        let codex_home = dir.path().join("codex");
        assert_eq!(memory_root(&codex_home), codex_home.join("memories"));
    }

    #[tokio::test]
    async fn clear_memory_root_contents_preserves_root_directory() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("memory");
        let nested_dir = root.join("rollout_summaries");
        tokio::fs::create_dir_all(&nested_dir)
            .await
            .expect("create rollout summaries dir");
        tokio::fs::write(root.join("MEMORY.md"), "stale memory index\n")
            .await
            .expect("write memory index");
        tokio::fs::write(nested_dir.join("rollout.md"), "stale rollout\n")
            .await
            .expect("write rollout summary");

        clear_memory_root_contents(&root)
            .await
            .expect("clear memory root contents");

        assert!(
            tokio::fs::try_exists(&root)
                .await
                .expect("check memory root existence"),
            "memory root should still exist after clearing contents"
        );
        let mut entries = tokio::fs::read_dir(&root)
            .await
            .expect("read memory root after clear");
        assert!(
            entries
                .next_entry()
                .await
                .expect("read next entry")
                .is_none(),
            "memory root should be empty after clearing contents"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clear_memory_root_contents_rejects_symlinked_root() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("outside");
        tokio::fs::create_dir_all(&target)
            .await
            .expect("create symlink target dir");
        let target_file = target.join("keep.txt");
        tokio::fs::write(&target_file, "keep\n")
            .await
            .expect("write target file");

        let root = dir.path().join("memory");
        std::os::unix::fs::symlink(&target, &root).expect("create memory root symlink");

        let err = clear_memory_root_contents(&root)
            .await
            .expect_err("symlinked memory root should be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            tokio::fs::try_exists(&target_file)
                .await
                .expect("check target file existence"),
            "rejecting a symlinked memory root should not delete the symlink target"
        );
    }

    #[tokio::test]
    async fn sync_rollout_summaries_and_raw_memories_file_keeps_latest_memories_only() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("memory");
        ensure_layout(&root).await.expect("ensure layout");

        let keep_id = ThreadId::default().to_string();
        let drop_id = ThreadId::default().to_string();
        let keep_path = rollout_summaries_dir(&root).join(format!("{keep_id}.md"));
        let drop_path = rollout_summaries_dir(&root).join(format!("{drop_id}.md"));
        tokio::fs::write(&keep_path, "keep")
            .await
            .expect("write keep");
        tokio::fs::write(&drop_path, "drop")
            .await
            .expect("write drop");

        let memories = vec![Stage1Output {
            thread_id: ThreadId::try_from(keep_id.clone()).expect("thread id"),
            source_updated_at: Utc.timestamp_opt(100, 0).single().expect("timestamp"),
            raw_memory: "raw memory".to_string(),
            rollout_summary: "short summary".to_string(),
            rollout_slug: None,
            rollout_path: PathBuf::from("/tmp/rollout-100.jsonl"),
            cwd: PathBuf::from("/tmp/workspace"),
            git_branch: None,
            generated_at: Utc.timestamp_opt(101, 0).single().expect("timestamp"),
        }];

        sync_rollout_summaries_from_memories(&root, &memories, 100)
            .await
            .expect("sync rollout summaries");
        rebuild_raw_memories_file_from_memories(&root, &memories, 100)
            .await
            .expect("rebuild raw memories");

        assert!(
            !tokio::fs::try_exists(&keep_path)
                .await
                .expect("check stale keep path"),
            "sync should prune stale filename that used thread id only"
        );
        assert!(
            !tokio::fs::try_exists(&drop_path)
                .await
                .expect("check stale drop path"),
            "sync should prune stale filename for dropped thread"
        );

        let mut dir = tokio::fs::read_dir(rollout_summaries_dir(&root))
            .await
            .expect("open rollout summaries dir");
        let mut files = Vec::new();
        while let Some(entry) = dir.next_entry().await.expect("read dir entry") {
            files.push(entry.file_name().to_string_lossy().to_string());
        }
        files.sort_unstable();
        assert_eq!(files.len(), 1);
        let canonical_rollout_summary_file = &files[0];

        let raw_memories = tokio::fs::read_to_string(raw_memories_file(&root))
            .await
            .expect("read raw memories");
        assert!(raw_memories.contains("raw memory"));
        assert!(raw_memories.contains(&keep_id));
        assert!(raw_memories.contains("cwd: /tmp/workspace"));
        assert!(raw_memories.contains("rollout_path: /tmp/rollout-100.jsonl"));
        assert!(raw_memories.contains(&format!(
            "rollout_summary_file: {canonical_rollout_summary_file}"
        )));
    }

    #[test]
    fn rollout_summary_file_stem_uses_uuid_timestamp_and_hash_when_slug_missing() {
        let thread_id = fixed_thread_id();
        let memory = stage1_output_with_slug(thread_id, None);

        assert_eq!(rollout_summary_file_stem(&memory), FIXED_PREFIX);
        assert_eq!(
            rollout_summary_file_stem_from_parts(
                memory.thread_id,
                memory.source_updated_at,
                memory.rollout_slug.as_deref(),
            ),
            FIXED_PREFIX
        );
    }

    #[test]
    fn rollout_summary_file_stem_sanitizes_and_truncates_slug() {
        let thread_id = fixed_thread_id();
        let memory = stage1_output_with_slug(
            thread_id,
            Some("Unsafe Slug/With Spaces & Symbols + EXTRA_LONG_12345_67890_ABCDE_fghij_klmno"),
        );

        let stem = rollout_summary_file_stem(&memory);
        let slug = stem
            .strip_prefix(&format!("{FIXED_PREFIX}-"))
            .expect("slug suffix should be present");
        assert_eq!(slug.len(), 60);
        assert_eq!(
            slug,
            "unsafe_slug_with_spaces___symbols___extra_long_12345_67890_a"
        );
    }

    #[test]
    fn rollout_summary_file_stem_uses_uuid_timestamp_and_hash_when_slug_is_empty() {
        let thread_id = fixed_thread_id();
        let memory = stage1_output_with_slug(thread_id, Some(""));

        assert_eq!(rollout_summary_file_stem(&memory), FIXED_PREFIX);
    }
}
