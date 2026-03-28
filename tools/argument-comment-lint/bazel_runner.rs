use std::env;
use std::ffi::OsString;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;

const TOOLCHAIN_CHANNEL: &str = "nightly-2025-09-18";
#[cfg(target_os = "linux")]
const TOOLCHAIN_TRIPLE: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const TOOLCHAIN_TRIPLE: &str = "aarch64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const TOOLCHAIN_TRIPLE: &str = "x86_64-apple-darwin";
#[cfg(target_os = "windows")]
const TOOLCHAIN_TRIPLE: &str = "x86_64-pc-windows-msvc";

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

    if env::var_os("CARGO_TARGET_DIR").is_none() {
        command.env("CARGO_TARGET_DIR", shared_target_dir(&workspace_dir));
    }

    let toolchain_bin_dir = toolchain_bin_dir();
    if let Some(bin_dir) = toolchain_bin_dir
        .clone()
        .or_else(|| fallback_cargo_binary().and_then(|cargo| cargo.parent().map(Path::to_path_buf)))
    {
        let existing_path = env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin_dir.clone()];
        paths.extend(env::split_paths(&existing_path));
        let joined_paths =
            env::join_paths(paths).map_err(|err| format!("failed to build PATH: {err}"))?;
        command.env("PATH", joined_paths);

        let cargo_name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        command.env("CARGO", bin_dir.join(cargo_name));

        if let Some(rustc) = executable_in_dir(&bin_dir, "rustc") {
            command.env("RUSTC", rustc);
        }
        if let Some(rustdoc) = executable_in_dir(&bin_dir, "rustdoc") {
            command.env("RUSTDOC", rustdoc);
        }
    }
    if toolchain_bin_dir.is_some() {
        command.env("CODEX_ARGUMENT_COMMENT_LINT_SKIP_RUSTUP_SHIMS", "1");
        command.env("RUSTUP_TOOLCHAIN", TOOLCHAIN_CHANNEL);
        command.env("RUSTUP_AUTO_INSTALL", "0");
    }
    if env::var_os("RUSTUP_HOME").is_none()
        && let Some(rustup_home) = infer_rustup_home()
    {
        command.env("RUSTUP_HOME", rustup_home);
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

fn fallback_cargo_binary() -> Option<PathBuf> {
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

fn toolchain_bin_dir() -> Option<PathBuf> {
    direct_toolchain_bin_dir().or_else(query_toolchain_bin_dir)
}

fn direct_toolchain_bin_dir() -> Option<PathBuf> {
    let rustup_home = infer_rustup_home()?;
    let bin_dir = PathBuf::from(rustup_home)
        .join("toolchains")
        .join(format!("{TOOLCHAIN_CHANNEL}-{TOOLCHAIN_TRIPLE}"))
        .join("bin");

    executable_in_dir(&bin_dir, "cargo").map(|_| bin_dir)
}

fn query_toolchain_bin_dir() -> Option<PathBuf> {
    let rustup = executable_on_path("rustup")?;
    let output = Command::new(rustup)
        .args(["which", "--toolchain", TOOLCHAIN_CHANNEL, "cargo"])
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .env_remove("RUSTUP_TOOLCHAIN")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let cargo_path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    cargo_path.parent().map(Path::to_path_buf)
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|entry| entry.join(&file_name))
            .find(|candidate| candidate.is_file())
    })
}

fn shared_target_dir(workspace_dir: &Path) -> PathBuf {
    let namespace = if let Some(run_id) = env::var_os("GITHUB_RUN_ID") {
        let mut namespace = run_id;
        if let Some(run_attempt) = env::var_os("GITHUB_RUN_ATTEMPT") {
            namespace.push("-");
            namespace.push(run_attempt);
        }
        namespace
    } else {
        let mut hasher = DefaultHasher::new();
        workspace_dir.hash(&mut hasher);
        OsString::from(format!("{:016x}", hasher.finish()))
    };

    env::temp_dir()
        .join("argument-comment-lint")
        .join(namespace)
        .join(format!("{TOOLCHAIN_CHANNEL}-{TOOLCHAIN_TRIPLE}"))
}

fn executable_in_dir(bin_dir: &Path, name: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let path = bin_dir.join(file_name);
    path.is_file().then_some(path)
}

fn infer_rustup_home() -> Option<OsString> {
    if let Some(rustup_home) = env::var_os("RUSTUP_HOME") {
        return Some(rustup_home);
    }

    if let Some(cargo_home) = env::var_os("CARGO_HOME")
        && let Some(home_dir) = Path::new(&cargo_home).parent()
    {
        let rustup_home = home_dir.join(".rustup");
        if rustup_home.is_dir() {
            return Some(rustup_home.into_os_string());
        }
    }

    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = env::var_os(var) {
            let rustup_home = Path::new(&home).join(".rustup");
            if rustup_home.is_dir() {
                return Some(rustup_home.into_os_string());
            }
        }
    }

    None
}
