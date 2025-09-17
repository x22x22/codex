use std::fs as std_fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_protocol::mcp_protocol::ConversationId;
use tokio::fs;
use tokio::process::Command;
use tracing::info;
use tracing::warn;

/// Represents a linked git worktree managed by Codex.
///
/// The handle tracks whether Codex created the worktree for the current
/// conversation. It leaves the checkout in place until [`remove`] is invoked.
pub struct WorktreeHandle {
    repo_root: PathBuf,
    path: PathBuf,
}

impl WorktreeHandle {
    /// Create (or reuse) a worktree rooted at
    /// `<repo_root>/codex/<conversation_id>`.
    pub async fn create(repo_root: &Path, conversation_id: &ConversationId) -> Result<Self> {
        if !repo_root.exists() {
            return Err(anyhow!(
                "git worktree root `{}` does not exist",
                repo_root.display()
            ));
        }

        let repo_root = repo_root.to_path_buf();
        let codex_dir = repo_root.join("codex");
        fs::create_dir_all(&codex_dir).await.with_context(|| {
            format!(
                "failed to create codex worktree directory at `{}`",
                codex_dir.display()
            )
        })?;

        let path = codex_dir.join(conversation_id.to_string());
        let is_registered = worktree_registered(&repo_root, &path).await?;

        if is_registered {
            info!(
                worktree = %path.display(),
                "reusing existing git worktree for conversation"
            );
            return Ok(Self { repo_root, path });
        }

        if path.exists() {
            return Err(anyhow!(
                "git worktree path `{}` already exists but is not registered; remove it manually",
                path.display()
            ));
        }

        run_git_command(
            &repo_root,
            [
                "worktree",
                "add",
                "--detach",
                path.to_str().ok_or_else(|| {
                    anyhow!(
                        "failed to convert worktree path `{}` to UTF-8",
                        path.display()
                    )
                })?,
                "HEAD",
            ],
        )
        .await
        .with_context(|| format!("failed to create git worktree at `{}`", path.display()))?;

        info!(
            worktree = %path.display(),
            "created git worktree for conversation"
        );

        Ok(Self { repo_root, path })
    }

    /// Absolute path to the worktree checkout on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the worktree and prune metadata from the repository.
    pub async fn remove(self) -> Result<()> {
        let path = self.path.clone();

        // `git worktree remove` fails if refs are missing or the checkout is dirty.
        // Use --force to ensure best effort removal; the user explicitly requested it.
        run_git_command(
            &self.repo_root,
            [
                "worktree",
                "remove",
                "--force",
                path.to_str().ok_or_else(|| {
                    anyhow!(
                        "failed to convert worktree path `{}` to UTF-8",
                        path.display()
                    )
                })?,
            ],
        )
        .await
        .with_context(|| format!("failed to remove git worktree `{}`", path.display()))?;

        // Prune dangling metadata so repeated sessions do not accumulate entries.
        if let Err(err) =
            run_git_command(&self.repo_root, ["worktree", "prune", "--expire", "now"]).await
        {
            warn!("failed to prune git worktrees: {err:#}");
        }

        Ok(())
    }
}

async fn worktree_registered(repo_root: &Path, target: &Path) -> Result<bool> {
    let output = run_git_command(repo_root, ["worktree", "list", "--porcelain"]).await?;
    let stdout = String::from_utf8(output.stdout)?;

    let target_canon = std_fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let candidate = Path::new(path);
            let candidate_canon =
                std_fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
            if candidate_canon == target_canon {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn run_git_command<'a>(
    repo_root: &Path,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<std::process::Output> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    cmd.current_dir(repo_root);
    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute git command in `{}`", repo_root.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        return Err(anyhow!("git command exited with status {status}: {stderr}",));
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    const GIT_ENV: [(&str, &str); 2] = [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ];

    async fn init_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().expect("tempdir");
        let repo_path = temp.path().join("repo");
        fs::create_dir_all(&repo_path)
            .await
            .expect("create repo dir");

        run_git_with_env(&repo_path, ["init"], &GIT_ENV)
            .await
            .expect("git init");
        run_git_with_env(&repo_path, ["config", "user.name", "Test User"], &GIT_ENV)
            .await
            .expect("config user.name");
        run_git_with_env(
            &repo_path,
            ["config", "user.email", "test@example.com"],
            &GIT_ENV,
        )
        .await
        .expect("config user.email");

        fs::write(repo_path.join("README.md"), b"hello world")
            .await
            .expect("write file");
        run_git_with_env(&repo_path, ["add", "README.md"], &GIT_ENV)
            .await
            .expect("git add");
        run_git_with_env(&repo_path, ["commit", "-m", "init"], &GIT_ENV)
            .await
            .expect("git commit");

        (temp, repo_path)
    }

    async fn run_git_with_env<'a>(
        cwd: &Path,
        args: impl IntoIterator<Item = &'a str>,
        envs: &[(&str, &str)],
    ) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(cwd);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let status = cmd.status().await.context("failed to spawn git command")?;
        if !status.success() {
            return Err(anyhow!(
                "git command exited with status {status} (cwd: {})",
                cwd.display()
            ));
        }
        Ok(())
    }

    async fn is_registered(repo_root: &Path, path: &Path) -> bool {
        worktree_registered(repo_root, path).await.unwrap_or(false)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn creates_and_removes_worktree() {
        let (_temp, repo) = init_repo().await;
        let conversation_id = ConversationId::new();

        let handle = WorktreeHandle::create(&repo, &conversation_id)
            .await
            .expect("create worktree");
        let path = handle.path().to_path_buf();
        assert!(path.exists(), "worktree path should exist on disk");
        assert!(
            is_registered(&repo, &path).await,
            "worktree should be registered"
        );

        handle.remove().await.expect("remove worktree");
        assert!(
            !is_registered(&repo, &path).await,
            "worktree should be removed from registration"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reuses_existing_worktree() {
        let (_temp, repo) = init_repo().await;
        let conversation_id = ConversationId::new();

        let first = WorktreeHandle::create(&repo, &conversation_id)
            .await
            .expect("create worktree");
        let path = first.path().to_path_buf();
        drop(first);

        let second = WorktreeHandle::create(&repo, &conversation_id)
            .await
            .expect("reuse worktree");
        assert_eq!(path, second.path());
        assert!(is_registered(&repo, second.path()).await);

        second.remove().await.expect("remove worktree");
    }
}
