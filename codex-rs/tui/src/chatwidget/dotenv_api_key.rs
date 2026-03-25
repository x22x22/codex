use std::fs::OpenOptions;
use std::io;
use std::io::ErrorKind;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use codex_login::OPENAI_API_KEY_ENV_VAR;

pub(super) fn validate_dotenv_target(path: &Path) -> io::Result<()> {
    ensure_parent_dir(path)?;
    reject_symlink(path)?;

    if path.exists() {
        let mut options = OpenOptions::new();
        options.append(true);
        #[cfg(unix)]
        {
            options.custom_flags(libc::O_NOFOLLOW);
        }
        options.open(path)?;
        return Ok(());
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
        options.mode(0o600);
    }
    options.open(path)?;
    std::fs::remove_file(path)
}

pub(super) fn upsert_dotenv_api_key(path: &Path, api_key: &str) -> io::Result<()> {
    if api_key.contains(['\n', '\r']) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "OPENAI_API_KEY must not contain newlines",
        ));
    }

    ensure_parent_dir(path)?;
    reject_symlink(path)?;

    let existing = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let mut next = String::new();
    let mut wrote_api_key = false;

    for segment in split_lines_preserving_terminators(&existing) {
        if is_active_assignment_for(segment, OPENAI_API_KEY_ENV_VAR) {
            if !wrote_api_key {
                next.push_str(&format!("{OPENAI_API_KEY_ENV_VAR}={api_key}\n"));
                wrote_api_key = true;
            }
            continue;
        }

        next.push_str(segment);
    }

    if !wrote_api_key {
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&format!("{OPENAI_API_KEY_ENV_VAR}={api_key}\n"));
    }

    write_dotenv_file(path, &next)
}

fn write_dotenv_file(path: &Path, contents: &str) -> io::Result<()> {
    reject_symlink(path)?;

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.flush()?;

    #[cfg(unix)]
    {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            ".env.local must not be a symlink",
        ));
    }

    Ok(())
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn split_lines_preserving_terminators(contents: &str) -> Vec<&str> {
    if contents.is_empty() {
        return Vec::new();
    }

    contents.split_inclusive('\n').collect()
}

fn is_active_assignment_for(line: &str, key: &str) -> bool {
    let mut rest = line.trim_start();
    if rest.starts_with('#') {
        return false;
    }

    if let Some(stripped) = rest.strip_prefix("export") {
        rest = stripped.trim_start();
    }

    let Some(rest) = rest.strip_prefix(key) else {
        return false;
    };

    rest.trim_start().starts_with('=')
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn upsert_creates_dotenv_file_when_missing() {
        let temp_dir = tempdir().expect("tempdir");
        let dotenv_path = temp_dir.path().join(".env");

        upsert_dotenv_api_key(&dotenv_path, "sk-test-key").expect("write dotenv");

        let written = std::fs::read_to_string(&dotenv_path).expect("read dotenv");
        assert_eq!(written, "OPENAI_API_KEY=sk-test-key\n");
    }

    #[cfg(unix)]
    #[test]
    fn upsert_creates_dotenv_file_with_owner_only_permissions() {
        let temp_dir = tempdir().expect("tempdir");
        let dotenv_path = temp_dir.path().join(".env");

        upsert_dotenv_api_key(&dotenv_path, "sk-test-key").expect("write dotenv");

        let mode = std::fs::metadata(&dotenv_path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn upsert_rejects_symlink_target() {
        let temp_dir = tempdir().expect("tempdir");
        let dotenv_path = temp_dir.path().join(".env");
        let target_path = temp_dir.path().join("target.env");
        std::fs::write(&target_path, "OTHER=value\n").expect("seed target");
        std::os::unix::fs::symlink(&target_path, &dotenv_path).expect("symlink");

        let err = upsert_dotenv_api_key(&dotenv_path, "sk-test-key").expect_err("reject symlink");

        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        let target = std::fs::read_to_string(&target_path).expect("read target");
        assert_eq!(target, "OTHER=value\n");
    }

    #[test]
    fn upsert_replaces_existing_api_key_and_collapses_duplicates() {
        let temp_dir = tempdir().expect("tempdir");
        let dotenv_path = temp_dir.path().join(".env");
        std::fs::write(
            &dotenv_path,
            "# comment\nOPENAI_API_KEY=sk-old-1\nOTHER=value\nexport OPENAI_API_KEY = sk-old-2\n",
        )
        .expect("seed dotenv");

        upsert_dotenv_api_key(&dotenv_path, "sk-new-key").expect("update dotenv");

        let written = std::fs::read_to_string(&dotenv_path).expect("read dotenv");
        assert_eq!(
            written,
            "# comment\nOPENAI_API_KEY=sk-new-key\nOTHER=value\n"
        );
    }

    #[test]
    fn validate_dotenv_target_succeeds_for_missing_file() {
        let temp_dir = tempdir().expect("tempdir");
        let dotenv_path = temp_dir.path().join(".env");

        validate_dotenv_target(&dotenv_path).expect("validate dotenv");

        assert!(
            !dotenv_path.exists(),
            "validation should not leave behind a new file"
        );
    }
}
