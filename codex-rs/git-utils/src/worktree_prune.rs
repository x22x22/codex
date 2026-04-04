use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::GitToolingError;
use crate::operations::run_git_for_status;
use crate::operations::run_git_for_stdout;
use crate::worktree::CODEX_MANAGED_WORKTREE_MARKER_FILE;
use crate::worktree::CODEX_MANAGED_WORKTREE_METADATA_FILE;
use crate::worktree::CodexManagedWorktreeMetadata;
use crate::worktree::read_or_backfill_worktree_metadata;
use crate::worktree::worktree_git_dir;

/// Filters and deletion behavior for pruning Codex-managed worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexWorktreePruneOptions {
    pub codex_home: PathBuf,
    pub source_repo_root: Option<PathBuf>,
    pub created_before: Option<u64>,
    pub last_used_before: Option<u64>,
    pub mode: CodexWorktreePruneMode,
}

/// Controls whether prune removes matching worktrees or only reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexWorktreePruneMode {
    DryRun,
    Delete,
}

/// One Codex-managed worktree selected by the prune scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexWorktreePruneCandidate {
    pub worktree_git_root: PathBuf,
    pub metadata_path: PathBuf,
    pub metadata: CodexManagedWorktreeMetadata,
}

/// Result of a prune scan over `$CODEX_HOME/worktrees`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexWorktreePruneReport {
    pub pruned: Vec<CodexWorktreePruneCandidate>,
    pub kept: Vec<CodexWorktreePruneCandidate>,
    pub skipped: Vec<CodexWorktreePruneSkipped>,
}

/// A managed worktree was discovered but not deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexWorktreePruneSkipped {
    pub candidate: CodexWorktreePruneCandidate,
    pub reason: CodexWorktreePruneSkipReason,
}

/// Why a managed worktree was not safe to prune.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexWorktreePruneSkipReason {
    DirtyWorktree,
    LocalCommits,
    MissingStartingRef,
}

/// Scans and optionally removes Codex-managed worktrees that match `options`.
pub fn prune_codex_managed_worktrees(
    options: &CodexWorktreePruneOptions,
) -> Result<CodexWorktreePruneReport, GitToolingError> {
    let mut report = CodexWorktreePruneReport::default();
    let candidates = discover_codex_managed_worktrees(options.codex_home.as_path())?;
    for candidate in candidates {
        if !worktree_matches_prune_filters(&candidate, options)? {
            report.kept.push(candidate);
            continue;
        }

        match classify_prune_candidate(&candidate) {
            Ok(None) => {
                if options.mode == CodexWorktreePruneMode::Delete {
                    delete_prune_candidate(&candidate)?;
                }
                report.pruned.push(candidate);
            }
            Ok(Some(reason)) => {
                report
                    .skipped
                    .push(CodexWorktreePruneSkipped { candidate, reason });
            }
            Err(_) => {
                report.skipped.push(CodexWorktreePruneSkipped {
                    candidate,
                    reason: CodexWorktreePruneSkipReason::MissingStartingRef,
                });
            }
        }
    }
    Ok(report)
}

fn discover_codex_managed_worktrees(
    codex_home: &Path,
) -> Result<Vec<CodexWorktreePruneCandidate>, GitToolingError> {
    let worktrees_root = codex_home.join("worktrees");
    let mut candidates = Vec::new();
    let Ok(bucket_entries) = fs::read_dir(&worktrees_root) else {
        return Ok(candidates);
    };

    for bucket_entry in bucket_entries {
        let bucket_entry = bucket_entry?;
        if !bucket_entry.file_type()?.is_dir() {
            continue;
        }
        for worktree_entry in fs::read_dir(bucket_entry.path())? {
            let worktree_entry = worktree_entry?;
            if !worktree_entry.file_type()?.is_dir() {
                continue;
            }
            let worktree_git_root = worktree_entry.path();
            let Ok(worktree_git_dir) = worktree_git_dir(&worktree_git_root) else {
                continue;
            };
            if !worktree_git_dir
                .join(CODEX_MANAGED_WORKTREE_MARKER_FILE)
                .exists()
            {
                continue;
            }
            let metadata_path = worktree_git_dir.join(CODEX_MANAGED_WORKTREE_METADATA_FILE);
            let metadata = read_or_backfill_worktree_metadata(&metadata_path, &worktree_git_root)?;
            candidates.push(CodexWorktreePruneCandidate {
                worktree_git_root,
                metadata_path,
                metadata,
            });
        }
    }
    Ok(candidates)
}

fn worktree_matches_prune_filters(
    candidate: &CodexWorktreePruneCandidate,
    options: &CodexWorktreePruneOptions,
) -> Result<bool, GitToolingError> {
    if let Some(source_repo_root) = options.source_repo_root.as_deref()
        && candidate.metadata.source_repo_root != source_repo_root.canonicalize()?
    {
        return Ok(false);
    }
    if let Some(created_before) = options.created_before
        && candidate.metadata.created_at >= created_before
    {
        return Ok(false);
    }
    if let Some(last_used_before) = options.last_used_before
        && candidate.metadata.last_used_at >= last_used_before
    {
        return Ok(false);
    }
    Ok(true)
}

fn classify_prune_candidate(
    candidate: &CodexWorktreePruneCandidate,
) -> Result<Option<CodexWorktreePruneSkipReason>, GitToolingError> {
    if candidate.metadata.starting_ref.is_empty() {
        return Ok(Some(CodexWorktreePruneSkipReason::MissingStartingRef));
    }

    let status = run_git_for_stdout(
        candidate.worktree_git_root.as_path(),
        vec![
            OsString::from("status"),
            OsString::from("--porcelain"),
            OsString::from("--untracked-files=all"),
        ],
        /*env*/ None,
    )?;
    if !status.is_empty() {
        return Ok(Some(CodexWorktreePruneSkipReason::DirtyWorktree));
    }

    let local_commits = run_git_for_stdout(
        candidate.worktree_git_root.as_path(),
        vec![
            OsString::from("rev-list"),
            OsString::from("--max-count=1"),
            OsString::from(format!("{}..HEAD", candidate.metadata.starting_ref)),
        ],
        /*env*/ None,
    )?;
    if !local_commits.is_empty() {
        return Ok(Some(CodexWorktreePruneSkipReason::LocalCommits));
    }

    Ok(None)
}

fn delete_prune_candidate(candidate: &CodexWorktreePruneCandidate) -> Result<(), GitToolingError> {
    run_git_for_status(
        candidate.metadata.source_repo_root.as_path(),
        vec![
            OsString::from("worktree"),
            OsString::from("remove"),
            OsString::from("--force"),
            OsString::from(candidate.worktree_git_root.as_os_str()),
        ],
        /*env*/ None,
    )?;
    run_git_for_status(
        candidate.metadata.source_repo_root.as_path(),
        vec![OsString::from("worktree"), OsString::from("prune")],
        /*env*/ None,
    )?;
    let _ = fs::remove_dir_all(&candidate.worktree_git_root);
    if let Some(bucket_path) = candidate.worktree_git_root.parent() {
        let _ = fs::remove_dir(bucket_path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CodexWorktreePruneCandidate;
    use super::CodexWorktreePruneMode;
    use super::CodexWorktreePruneOptions;
    use super::CodexWorktreePruneSkipReason;
    use super::prune_codex_managed_worktrees;
    use crate::CodexManagedWorktreeMetadata;
    use crate::create_codex_managed_worktree;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn prune_codex_managed_worktrees_filters_by_repo_and_timestamps_and_skips_dirty_worktrees() {
        let (_first_temp, first_repo, first_nested) = create_repo_with_nested_cwd();
        let (_second_temp, _second_repo, second_nested) = create_repo_with_nested_cwd();
        let codex_home = tempdir().expect("codex home");

        let old_clean = create_codex_managed_worktree(&first_nested, codex_home.path())
            .expect("old clean worktree");
        let old_dirty = create_codex_managed_worktree(&first_nested, codex_home.path())
            .expect("old dirty worktree");
        let new_clean = create_codex_managed_worktree(&first_nested, codex_home.path())
            .expect("new clean worktree");
        let other_repo =
            create_codex_managed_worktree(&second_nested, codex_home.path()).expect("other repo");

        rewrite_metadata_timestamp(&old_clean.metadata_path, 10, 20);
        rewrite_metadata_timestamp(&old_dirty.metadata_path, 10, 20);
        rewrite_metadata_timestamp(&new_clean.metadata_path, 100, 200);
        rewrite_metadata_timestamp(&other_repo.metadata_path, 10, 20);
        fs::write(old_dirty.worktree_git_root.join("dirty.txt"), "dirty\n")
            .expect("dirty worktree");

        let report = prune_codex_managed_worktrees(&CodexWorktreePruneOptions {
            codex_home: codex_home.path().to_path_buf(),
            source_repo_root: Some(first_repo),
            created_before: Some(50),
            last_used_before: Some(50),
            mode: CodexWorktreePruneMode::Delete,
        })
        .expect("prune worktrees");

        assert_eq!(
            sorted_worktree_paths(&report.pruned),
            vec![old_clean.worktree_git_root.clone()]
        );
        assert_eq!(
            report
                .skipped
                .iter()
                .map(|skipped| (skipped.candidate.worktree_git_root.clone(), skipped.reason))
                .collect::<Vec<_>>(),
            vec![(
                old_dirty.worktree_git_root.clone(),
                CodexWorktreePruneSkipReason::DirtyWorktree
            )]
        );
        assert_eq!(
            sorted_worktree_paths(&report.kept),
            sorted_paths(vec![
                new_clean.worktree_git_root.clone(),
                other_repo.worktree_git_root.clone()
            ])
        );
        assert!(!old_clean.worktree_git_root.exists());
        assert!(old_dirty.worktree_git_root.exists());
        assert!(new_clean.worktree_git_root.exists());
        assert!(other_repo.worktree_git_root.exists());
    }

    #[test]
    fn prune_codex_managed_worktrees_skips_marker_only_legacy_worktrees() {
        let (_temp, _repo, nested) = create_repo_with_nested_cwd();
        let codex_home = tempdir().expect("codex home");
        let legacy =
            create_codex_managed_worktree(&nested, codex_home.path()).expect("legacy worktree");
        fs::remove_file(&legacy.metadata_path).expect("remove metadata sidecar");

        let report = prune_codex_managed_worktrees(&CodexWorktreePruneOptions {
            codex_home: codex_home.path().to_path_buf(),
            source_repo_root: None,
            created_before: None,
            last_used_before: None,
            mode: CodexWorktreePruneMode::Delete,
        })
        .expect("prune worktrees");

        assert_eq!(report.pruned, Vec::new());
        assert_eq!(report.kept, Vec::new());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            report.skipped[0].candidate.worktree_git_root,
            legacy.worktree_git_root
        );
        assert_eq!(
            report.skipped[0].reason,
            CodexWorktreePruneSkipReason::MissingStartingRef
        );
        assert!(legacy.worktree_git_root.exists());
    }

    fn create_repo_with_nested_cwd() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested = repo.join("nested").join("path");
        fs::create_dir_all(&nested).expect("nested dir");
        init_test_repo(&repo);
        fs::write(repo.join("README.md"), "hello\n").expect("write file");
        fs::write(nested.join("marker.txt"), "nested\n").expect("write nested file");
        commit(&repo, "initial");
        (temp, repo, nested)
    }

    fn init_test_repo(repo_path: &Path) {
        run_git_in(repo_path, &["init", "--initial-branch=main"]);
        run_git_in(repo_path, &["config", "core.autocrlf", "false"]);
        run_git_in(repo_path, &["config", "user.name", "Tester"]);
        run_git_in(repo_path, &["config", "user.email", "test@example.com"]);
    }

    fn commit(repo_path: &Path, message: &str) {
        run_git_in(repo_path, &["add", "."]);
        run_git_in(
            repo_path,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                message,
            ],
        );
    }

    fn run_git_in(repo_path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git command failed: {args:?}");
    }

    fn rewrite_metadata_timestamp(metadata_path: &Path, created_at: u64, last_used_at: u64) {
        let mut metadata: CodexManagedWorktreeMetadata =
            serde_json::from_slice(&fs::read(metadata_path).expect("read metadata"))
                .expect("parse metadata");
        metadata.created_at = created_at;
        metadata.last_used_at = last_used_at;
        fs::write(
            metadata_path,
            serde_json::to_vec_pretty(&metadata).expect("serialize metadata"),
        )
        .expect("write metadata");
    }

    fn sorted_worktree_paths(candidates: &[CodexWorktreePruneCandidate]) -> Vec<PathBuf> {
        sorted_paths(
            candidates
                .iter()
                .map(|candidate| candidate.worktree_git_root.clone())
                .collect(),
        )
    }

    fn sorted_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort();
        paths
    }
}
