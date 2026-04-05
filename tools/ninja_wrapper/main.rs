use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let Some(ninja) = find_ninja() else {
        eprintln!("unable to locate ninja");
        return ExitCode::from(127);
    };

    let status = match Command::new(&ninja).args(&args).status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("failed to launch {}: {err}", ninja.display());
            return ExitCode::from(127);
        }
    };

    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from)
}

fn find_ninja() -> Option<PathBuf> {
    path_ninja().or_else(common_ninja)
}

fn path_ninja() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for binary in binary_names() {
            let candidate = directory.join(binary);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn common_ninja() -> Option<PathBuf> {
    for env_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Some(root) = env::var_os(env_var) else {
            continue;
        };
        for edition in ["Enterprise", "Professional", "BuildTools"] {
            let candidate = PathBuf::from(&root)
                .join("Microsoft Visual Studio")
                .join("2022")
                .join(edition)
                .join("Common7")
                .join("IDE")
                .join("CommonExtensions")
                .join("Microsoft")
                .join("CMake")
                .join("Ninja")
                .join("ninja.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn common_ninja() -> Option<PathBuf> {
    for candidate in [
        PathBuf::from("/usr/bin/ninja"),
        PathBuf::from("/bin/ninja"),
        PathBuf::from("/usr/local/bin/ninja"),
        PathBuf::from("/opt/homebrew/bin/ninja"),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let output = Command::new("xcrun").args(["-f", "ninja"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let resolved = String::from_utf8(output.stdout).ok()?;
    let candidate = PathBuf::from(resolved.trim());
    candidate.is_file().then_some(candidate)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn common_ninja() -> Option<PathBuf> {
    for candidate in [
        PathBuf::from("/usr/bin/ninja"),
        PathBuf::from("/bin/ninja"),
        PathBuf::from("/usr/local/bin/ninja"),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn binary_names() -> &'static [&'static str] {
    &["ninja.exe"]
}

#[cfg(not(windows))]
fn binary_names() -> &'static [&'static str] {
    &["ninja"]
}
