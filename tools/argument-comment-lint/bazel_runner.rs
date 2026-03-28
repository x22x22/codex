use std::env;
use std::path::Path;
use std::process::Command;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let manifest = env::var("ARGUMENT_COMMENT_LINT_MANIFEST")
        .map_err(|_| "ARGUMENT_COMMENT_LINT_MANIFEST must be set".to_string())?;
    let workspace_dir = env::current_dir().map_err(|err| format!("failed to get cwd: {err}"))?;
    let wrapper = find_repo_root(&workspace_dir)?
        .join("tools")
        .join("argument-comment-lint")
        .join("run-prebuilt-linter.py");

    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut command = Command::new(python);
    command.arg(&wrapper);
    command.arg("--manifest-path");
    command.arg(&manifest);

    // Keep Linux on the narrower target set for now to match the current CI
    // rollout, while macOS and Windows continue to exercise all targets.
    if cfg!(target_os = "linux") {
        command.args(["--", "--lib", "--bins"]);
    }

    if env::var_os("CARGO_TARGET_DIR").is_none()
        && let Some(test_tmpdir) = env::var_os("TEST_TMPDIR")
    {
        let sanitized_manifest = manifest.replace('/', "_").replace('\\', "_");
        let target_dir =
            Path::new(&test_tmpdir).join(format!("argument-comment-lint-{sanitized_manifest}"));
        command.env("CARGO_TARGET_DIR", target_dir);
    }

    if let Some(cargo) = cargo_binary() {
        let cargo_dir = cargo
            .parent()
            .ok_or_else(|| format!("failed to resolve cargo directory from {}", cargo.display()))?;
        let existing_path = env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![cargo_dir.to_path_buf()];
        paths.extend(env::split_paths(&existing_path));
        let joined_paths =
            env::join_paths(paths).map_err(|err| format!("failed to build PATH: {err}"))?;
        command.env("PATH", joined_paths);
        command.env("CARGO", cargo);
    }

    let status = command
        .status()
        .map_err(|err| format!("failed to execute {python}: {err}"))?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or_else(|| ExitCode::from(1), ExitCode::from))
}

fn find_repo_root(cwd: &Path) -> Result<&Path, String> {
    if cwd
        .join("tools")
        .join("argument-comment-lint")
        .join("run-prebuilt-linter.py")
        .is_file()
    {
        return Ok(cwd);
    }

    let Some(parent) = cwd.parent() else {
        return Err(format!(
            "argument-comment wrapper not found relative to {}",
            cwd.display()
        ));
    };
    if parent
        .join("tools")
        .join("argument-comment-lint")
        .join("run-prebuilt-linter.py")
        .is_file()
    {
        return Ok(parent);
    }

    Err(format!(
        "argument-comment wrapper not found relative to {}",
        cwd.display()
    ))
}

fn cargo_binary() -> Option<std::path::PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = env::var_os(var) {
            let cargo_bin = Path::new(&home).join(".cargo").join("bin");
            let candidate = cargo_bin.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if let Ok(cwd) = env::current_dir() {
        for ancestor in cwd.ancestors() {
            let candidate = ancestor.join(".cargo").join("bin").join(if cfg!(windows) {
                "cargo.exe"
            } else {
                "cargo"
            });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}
