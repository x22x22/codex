use clap::Parser;
use codex_core::config::find_codex_home;
use codex_git_utils::CodexWorktreePruneCandidate;
use codex_git_utils::CodexWorktreePruneMode;
use codex_git_utils::CodexWorktreePruneOptions;
use codex_git_utils::CodexWorktreePruneSkipReason;
use codex_git_utils::CodexWorktreePruneSkipped;
use codex_git_utils::prune_codex_managed_worktrees;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct WorktreeCli {
    #[command(subcommand)]
    pub(crate) subcommand: WorktreeSubcommand,
}

#[derive(Debug, Parser)]
pub(crate) enum WorktreeSubcommand {
    /// Remove old Codex-managed local git worktrees.
    Prune(WorktreePruneCommand),
}

#[derive(Debug, Parser)]
pub(crate) struct WorktreePruneCommand {
    /// Only prune worktrees created from this source repository.
    #[arg(long = "repo", value_name = "DIR")]
    repo: Option<PathBuf>,

    /// Only prune worktrees created before this Unix timestamp (seconds).
    #[arg(long = "created-before", value_name = "UNIX_SECONDS")]
    created_before: Option<u64>,

    /// Only prune worktrees last used before this Unix timestamp (seconds).
    #[arg(long = "last-used-before", value_name = "UNIX_SECONDS")]
    last_used_before: Option<u64>,

    /// Print matching worktrees without deleting them.
    #[arg(long = "dry-run", default_value_t = false)]
    dry_run: bool,
}

pub(crate) fn run_worktree_command(worktree_cli: WorktreeCli) -> anyhow::Result<()> {
    match worktree_cli.subcommand {
        WorktreeSubcommand::Prune(command) => run_worktree_prune_command(command),
    }
}

fn run_worktree_prune_command(command: WorktreePruneCommand) -> anyhow::Result<()> {
    let options = CodexWorktreePruneOptions {
        codex_home: find_codex_home()?,
        source_repo_root: command.repo,
        created_before: command.created_before,
        last_used_before: command.last_used_before,
        mode: if command.dry_run {
            CodexWorktreePruneMode::DryRun
        } else {
            CodexWorktreePruneMode::Delete
        },
    };

    let report = prune_codex_managed_worktrees(&options)?;
    for candidate in &report.pruned {
        print_candidate("pruned", candidate);
    }
    for skipped in &report.skipped {
        print_skipped_candidate(skipped);
    }
    for candidate in &report.kept {
        print_candidate("kept", candidate);
    }
    println!(
        "summary: pruned={} skipped={} kept={}",
        report.pruned.len(),
        report.skipped.len(),
        report.kept.len()
    );
    Ok(())
}

fn print_candidate(action: &str, candidate: &CodexWorktreePruneCandidate) {
    println!(
        "{action} {} repo={} created_at={} last_used_at={}",
        candidate.worktree_git_root.display(),
        candidate.metadata.source_repo_root.display(),
        candidate.metadata.created_at,
        candidate.metadata.last_used_at
    );
}

fn print_skipped_candidate(skipped: &CodexWorktreePruneSkipped) {
    let reason = match skipped.reason {
        CodexWorktreePruneSkipReason::DirtyWorktree => "dirty-worktree",
        CodexWorktreePruneSkipReason::LocalCommits => "local-commits",
        CodexWorktreePruneSkipReason::MissingStartingRef => "missing-starting-ref",
    };
    println!(
        "skipped {} reason={} repo={} created_at={} last_used_at={}",
        skipped.candidate.worktree_git_root.display(),
        reason,
        skipped.candidate.metadata.source_repo_root.display(),
        skipped.candidate.metadata.created_at,
        skipped.candidate.metadata.last_used_at
    );
}

#[cfg(test)]
mod tests {
    use super::WorktreeCli;
    use super::WorktreeSubcommand;
    use clap::Parser;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    #[test]
    fn parse_worktree_prune_filters() {
        let cli = WorktreeCli::parse_from([
            "codex-worktree",
            "prune",
            "--repo",
            "/tmp/repo",
            "--created-before",
            "123",
            "--last-used-before",
            "456",
            "--dry-run",
        ]);

        let WorktreeSubcommand::Prune(command) = cli.subcommand;
        assert_eq!(command.repo, Some(PathBuf::from("/tmp/repo")));
        assert_eq!(command.created_before, Some(123));
        assert_eq!(command.last_used_before, Some(456));
        assert!(command.dry_run);
    }
}
