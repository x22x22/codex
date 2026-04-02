use std::collections::hash_map::DefaultHasher;
use std::ffi::OsString;
use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use crate::GitToolingError;
use crate::operations::ensure_git_repository;
use crate::operations::repo_subdir;
use crate::operations::resolve_head;
use crate::operations::resolve_repository_root;
use crate::operations::run_git_for_status;
use crate::operations::run_git_for_stdout;

static WORKTREE_BUCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const CODEX_MANAGED_WORKTREE_MARKER_FILE: &str = "codex-managed";

/// Metadata for a detached worktree created under `$CODEX_HOME/worktrees`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexManagedWorktree {
    pub source_cwd: PathBuf,
    pub source_repo_root: PathBuf,
    pub worktree_git_root: PathBuf,
    pub worktree_git_dir: PathBuf,
    pub worktree_workspace_root: PathBuf,
    pub starting_ref: String,
    pub marker_path: PathBuf,
}

/// Creates a detached worktree for `source_cwd` and returns the mapped cwd
/// inside the new checkout.
pub fn create_codex_managed_worktree(
    source_cwd: &Path,
    codex_home: &Path,
) -> Result<CodexManagedWorktree, GitToolingError> {
    ensure_git_repository(source_cwd)?;

    let source_repo_root = resolve_repository_root(source_cwd)?;
    let source_cwd = source_cwd.to_path_buf();
    let relative_cwd = repo_subdir(&source_repo_root, &source_cwd);

    let starting_ref = starting_ref_for_repo(source_repo_root.as_path())?;
    let worktree_git_root = allocate_worktree_root(codex_home, source_repo_root.as_path())?;

    create_worktree_checkout(
        source_repo_root.as_path(),
        &worktree_git_root,
        &starting_ref,
    )?;
    let setup_result = setup_worktree_checkout(&worktree_git_root);
    let (worktree_git_dir, marker_path) = match setup_result {
        Ok(setup) => setup,
        Err(err) => {
            cleanup_worktree_checkout(source_repo_root.as_path(), &worktree_git_root);
            return Err(err);
        }
    };

    let worktree_workspace_root = match relative_cwd {
        Some(relative) => worktree_git_root.join(relative),
        None => worktree_git_root.clone(),
    };

    Ok(CodexManagedWorktree {
        source_cwd,
        source_repo_root,
        worktree_git_root,
        worktree_git_dir,
        worktree_workspace_root,
        starting_ref,
        marker_path,
    })
}

fn starting_ref_for_repo(repo_root: &Path) -> Result<String, GitToolingError> {
    let branch = run_git_for_stdout(
        repo_root,
        vec![OsString::from("branch"), OsString::from("--show-current")],
        None,
    )?;
    if !branch.is_empty() {
        return Ok(branch);
    }

    match resolve_head(repo_root)? {
        Some(head) => Ok(head),
        None => Ok(String::from("HEAD")),
    }
}

fn allocate_worktree_root(
    codex_home: &Path,
    source_repo_root: &Path,
) -> Result<PathBuf, GitToolingError> {
    let repo_name = source_repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("repo");
    let worktrees_root = codex_home.join("worktrees");
    fs::create_dir_all(&worktrees_root)?;

    for _ in 0..64 {
        let bucket = next_worktree_bucket(source_repo_root);
        let candidate = worktrees_root.join(bucket).join(repo_name);
        if candidate.exists() {
            continue;
        }
        if let Some(parent) = candidate.parent() {
            fs::create_dir_all(parent)?;
        }
        return Ok(candidate);
    }

    Err(GitToolingError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to allocate a unique codex worktree path",
    )))
}

fn next_worktree_bucket(source_repo_root: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    source_repo_root.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    WORKTREE_BUCKET_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .hash(&mut hasher);
    format!("{:04x}", (hasher.finish() & 0xffff) as u16)
}

fn create_worktree_checkout(
    source_repo_root: &Path,
    worktree_git_root: &Path,
    starting_ref: &str,
) -> Result<(), GitToolingError> {
    let result = run_git_for_status(
        source_repo_root,
        vec![
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("--detach"),
            OsString::from(worktree_git_root.as_os_str()),
            OsString::from(starting_ref),
        ],
        None,
    );

    if let Err(err) = result {
        let _ = fs::remove_dir_all(worktree_git_root);
        return Err(err);
    }

    Ok(())
}

fn setup_worktree_checkout(
    worktree_git_root: &Path,
) -> Result<(PathBuf, PathBuf), GitToolingError> {
    let worktree_git_dir = worktree_git_dir(worktree_git_root)?;
    let marker_path = write_codex_managed_marker(&worktree_git_dir)?;
    Ok((worktree_git_dir, marker_path))
}

fn cleanup_worktree_checkout(source_repo_root: &Path, worktree_git_root: &Path) {
    let _ = run_git_for_status(
        source_repo_root,
        vec![
            OsString::from("worktree"),
            OsString::from("remove"),
            OsString::from("--force"),
            OsString::from(worktree_git_root.as_os_str()),
        ],
        /*env*/ None,
    );
    let _ = fs::remove_dir_all(worktree_git_root);
}

fn write_codex_managed_marker(worktree_git_dir: &Path) -> Result<PathBuf, GitToolingError> {
    let marker_path = worktree_git_dir.join(CODEX_MANAGED_WORKTREE_MARKER_FILE);
    let mut marker = fs::File::create(&marker_path)?;
    marker.write_all(b"codex-managed\n")?;
    Ok(marker_path)
}

fn worktree_git_dir(worktree_git_root: &Path) -> Result<PathBuf, GitToolingError> {
    let git_dir = run_git_for_stdout(
        worktree_git_root,
        vec![OsString::from("rev-parse"), OsString::from("--git-dir")],
        None,
    )?;
    let git_dir = PathBuf::from(git_dir);
    if git_dir.is_absolute() {
        Ok(git_dir)
    } else {
        Ok(worktree_git_root.join(git_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::CODEX_MANAGED_WORKTREE_MARKER_FILE;
    use super::CodexManagedWorktree;
    use super::allocate_worktree_root;
    use super::cleanup_worktree_checkout;
    use super::create_codex_managed_worktree;
    use super::create_worktree_checkout;
    use super::starting_ref_for_repo;
    use crate::GitToolingError;
    #[cfg(unix)]
    use crate::platform::create_symlink;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::tempdir;

    fn run_git_in(repo_path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git command failed: {args:?}");
    }

    fn git_stdout_in(repo_path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .output()
            .expect("git command");
        assert!(output.status.success(), "git command failed: {args:?}");
        String::from_utf8(output.stdout).expect("git stdout utf8")
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

    fn assert_worktree_result(
        result: &CodexManagedWorktree,
        codex_home: &Path,
        repo: &Path,
        nested: &Path,
    ) {
        let expected_repo_root = repo.canonicalize().expect("repo canonicalized");
        assert_eq!(result.source_repo_root, expected_repo_root);
        assert_eq!(
            result.worktree_workspace_root,
            result.worktree_git_root.join("nested/path")
        );
        assert_eq!(result.source_cwd, nested);
        assert!(
            result
                .worktree_git_root
                .starts_with(codex_home.join("worktrees"))
        );
        assert!(result.worktree_git_dir.exists());
        assert_eq!(
            result.marker_path,
            result
                .worktree_git_dir
                .join(CODEX_MANAGED_WORKTREE_MARKER_FILE)
        );
    }

    #[test]
    fn create_codex_managed_worktree_preserves_nested_cwd_mapping() -> Result<(), GitToolingError> {
        let (_temp, repo, nested) = create_repo_with_nested_cwd();
        let codex_home = tempdir().expect("codex home");

        let result = create_codex_managed_worktree(&nested, codex_home.path())?;

        assert_worktree_result(&result, codex_home.path(), &repo, &nested);
        assert!(result.worktree_workspace_root.exists());
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn create_codex_managed_worktree_preserves_nested_cwd_mapping_from_symlink()
    -> Result<(), GitToolingError> {
        let (temp, repo, _nested) = create_repo_with_nested_cwd();
        let repo_symlink = temp.path().join("repo-symlink");
        create_symlink(&repo, &repo, &repo_symlink)?;
        let symlinked_nested = repo_symlink.join("nested/path");
        let codex_home = tempdir().expect("codex home");

        let result = create_codex_managed_worktree(&symlinked_nested, codex_home.path())?;

        assert_eq!(
            result.worktree_workspace_root,
            result.worktree_git_root.join("nested/path")
        );
        assert_eq!(result.source_cwd, symlinked_nested);
        Ok(())
    }

    #[test]
    fn create_codex_managed_worktree_writes_marker_file() -> Result<(), GitToolingError> {
        let (_temp, repo, nested) = create_repo_with_nested_cwd();
        let codex_home = tempdir().expect("codex home");

        let result = create_codex_managed_worktree(&nested, codex_home.path())?;

        let marker = fs::read_to_string(&result.marker_path)?;
        assert_eq!(marker, "codex-managed\n");
        assert_eq!(
            result.marker_path,
            result
                .worktree_git_dir
                .join(CODEX_MANAGED_WORKTREE_MARKER_FILE)
        );
        assert!(repo.exists());
        Ok(())
    }

    #[test]
    fn cleanup_worktree_checkout_removes_worktree_registration() -> Result<(), GitToolingError> {
        let (_temp, repo, _nested) = create_repo_with_nested_cwd();
        let codex_home = tempdir().expect("codex home");
        let starting_ref = starting_ref_for_repo(&repo)?;
        let worktree_git_root = allocate_worktree_root(codex_home.path(), &repo)?;
        create_worktree_checkout(&repo, &worktree_git_root, &starting_ref)?;
        assert!(worktree_git_root.exists());
        assert!(
            git_stdout_in(&repo, &["worktree", "list", "--porcelain"])
                .contains(&worktree_git_root.to_string_lossy().to_string())
        );

        cleanup_worktree_checkout(&repo, &worktree_git_root);

        assert!(!worktree_git_root.exists());
        assert!(
            !git_stdout_in(&repo, &["worktree", "list", "--porcelain"])
                .contains(&worktree_git_root.to_string_lossy().to_string())
        );
        Ok(())
    }
}
