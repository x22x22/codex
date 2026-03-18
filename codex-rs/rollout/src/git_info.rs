use codex_protocol::protocol::GitInfo;
use std::path::Path;
use tokio::process::Command;
use tokio::time::Duration as TokioDuration;
use tokio::time::timeout;

const GIT_COMMAND_TIMEOUT: TokioDuration = TokioDuration::from_secs(5);

pub(crate) async fn collect_git_info(cwd: &Path) -> Option<GitInfo> {
    let is_git_repo = run_git_command_with_timeout(&["rev-parse", "--git-dir"], cwd)
        .await?
        .status
        .success();
    if !is_git_repo {
        return None;
    }

    let (commit_result, branch_result, url_result) = tokio::join!(
        run_git_command_with_timeout(&["rev-parse", "HEAD"], cwd),
        run_git_command_with_timeout(&["rev-parse", "--abbrev-ref", "HEAD"], cwd),
        run_git_command_with_timeout(&["remote", "get-url", "origin"], cwd)
    );

    let mut git_info = GitInfo {
        commit_hash: None,
        branch: None,
        repository_url: None,
    };

    if let Some(output) = commit_result
        && output.status.success()
        && let Ok(hash) = String::from_utf8(output.stdout)
    {
        git_info.commit_hash = Some(hash.trim().to_string());
    }

    if let Some(output) = branch_result
        && output.status.success()
        && let Ok(branch) = String::from_utf8(output.stdout)
    {
        let branch = branch.trim();
        if branch != "HEAD" {
            git_info.branch = Some(branch.to_string());
        }
    }

    if let Some(output) = url_result
        && output.status.success()
        && let Ok(url) = String::from_utf8(output.stdout)
    {
        git_info.repository_url = Some(url.trim().to_string());
    }

    Some(git_info)
}

async fn run_git_command_with_timeout(args: &[&str], cwd: &Path) -> Option<std::process::Output> {
    timeout(
        GIT_COMMAND_TIMEOUT,
        Command::new("git").args(args).current_dir(cwd).output(),
    )
    .await
    .ok()?
    .ok()
}
