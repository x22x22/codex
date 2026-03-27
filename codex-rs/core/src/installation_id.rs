use std::fs::OpenOptions;
use std::io::Result;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tokio::fs;
use uuid::Uuid;

pub(crate) const INSTALLATION_ID_FILENAME: &str = "installation_id";

enum InstallationIdWriteMode {
    CreateNew,
    Overwrite,
}

pub(crate) async fn resolve_installation_id(codex_home: &Path) -> Result<String> {
    let path = codex_home.join(INSTALLATION_ID_FILENAME);
    if let Some(existing) = read_installation_id(&path).await? {
        return Ok(existing);
    }

    fs::create_dir_all(codex_home).await?;

    loop {
        let installation_id = Uuid::new_v4().to_string();
        match write_installation_id(
            path.clone(),
            installation_id.clone(),
            InstallationIdWriteMode::CreateNew,
        )
        .await
        {
            Ok(()) => return Ok(installation_id),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(existing) = read_installation_id(&path).await? {
                    return Ok(existing);
                }

                write_installation_id(
                    path.clone(),
                    installation_id.clone(),
                    InstallationIdWriteMode::Overwrite,
                )
                .await?;
                return Ok(installation_id);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(codex_home).await?;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn read_installation_id(path: &Path) -> Result<Option<String>> {
    let contents = match fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    Ok(Uuid::parse_str(trimmed).ok().map(|uuid| uuid.to_string()))
}

async fn write_installation_id(
    path: PathBuf,
    installation_id: String,
    mode: InstallationIdWriteMode,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options.write(true);

        match mode {
            InstallationIdWriteMode::CreateNew => {
                options.create_new(true);
            }
            InstallationIdWriteMode::Overwrite => {
                options.create(true).truncate(true);
            }
        }

        #[cfg(unix)]
        {
            options.mode(0o600);
        }

        let mut file = options.open(&path)?;
        file.write_all(installation_id.as_bytes())?;
        file.flush()?;

        #[cfg(unix)]
        {
            let metadata = file.metadata()?;
            let current_mode = metadata.permissions().mode() & 0o777;
            if current_mode != 0o600 {
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o600);
                file.set_permissions(permissions)?;
            }
        }

        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::INSTALLATION_ID_FILENAME;
    use super::resolve_installation_id;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn resolve_installation_id_generates_and_persists_uuid() {
        let codex_home = TempDir::new().expect("create temp dir");
        let persisted_path = codex_home.path().join(INSTALLATION_ID_FILENAME);

        let installation_id = resolve_installation_id(codex_home.path())
            .await
            .expect("resolve installation id");

        assert_eq!(
            std::fs::read_to_string(&persisted_path).expect("read persisted installation id"),
            installation_id
        );
        assert!(Uuid::parse_str(&installation_id).is_ok());

        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&persisted_path)
                .expect("read installation id metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn resolve_installation_id_reuses_existing_uuid() {
        let codex_home = TempDir::new().expect("create temp dir");
        let existing = Uuid::new_v4().to_string().to_uppercase();
        std::fs::write(
            codex_home.path().join(INSTALLATION_ID_FILENAME),
            existing.clone(),
        )
        .expect("write installation id");

        let resolved = resolve_installation_id(codex_home.path())
            .await
            .expect("resolve installation id");

        assert_eq!(
            resolved,
            Uuid::parse_str(existing.as_str())
                .expect("parse existing installation id")
                .to_string()
        );
    }

    #[tokio::test]
    async fn resolve_installation_id_rewrites_invalid_file_contents() {
        let codex_home = TempDir::new().expect("create temp dir");
        std::fs::write(
            codex_home.path().join(INSTALLATION_ID_FILENAME),
            "not-a-uuid",
        )
        .expect("write invalid installation id");

        let resolved = resolve_installation_id(codex_home.path())
            .await
            .expect("resolve installation id");

        assert!(Uuid::parse_str(&resolved).is_ok());
        assert_eq!(
            std::fs::read_to_string(codex_home.path().join(INSTALLATION_ID_FILENAME))
                .expect("read rewritten installation id"),
            resolved
        );
    }
}
