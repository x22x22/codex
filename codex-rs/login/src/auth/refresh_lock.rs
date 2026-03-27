use fd_lock::RwLock;
use std::fs::File;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const AUTH_REFRESH_LOCK_FILE: &str = "auth-refresh.lock";

pub(super) fn auth_refresh_lock_path(codex_home: &Path) -> PathBuf {
    codex_home.join(AUTH_REFRESH_LOCK_FILE)
}

pub(super) fn open_auth_refresh_lock(codex_home: &Path) -> std::io::Result<RwLock<File>> {
    std::fs::create_dir_all(codex_home)?;
    let lock_path = auth_refresh_lock_path(codex_home);
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let file = options.open(lock_path)?;
    Ok(RwLock::new(file))
}

#[derive(Debug)]
pub(super) struct AuthRefreshLockGuard {
    release_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<std::io::Result<()>>,
}

impl AuthRefreshLockGuard {
    pub(super) async fn acquire(codex_home: &Path) -> std::io::Result<Self> {
        let codex_home = codex_home.to_path_buf();
        let (acquired_tx, acquired_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let join_handle = tokio::task::spawn_blocking(move || {
            let mut lock = open_auth_refresh_lock(&codex_home)?;
            let _guard = lock.write()?;
            let _ = acquired_tx.send(Ok(()));
            let _ = release_rx.blocking_recv();
            Ok(())
        });

        match acquired_rx.await {
            Ok(Ok(())) => Ok(Self {
                release_tx: Some(release_tx),
                join_handle,
            }),
            Ok(Err(err)) => {
                let _ = join_handle.await;
                Err(err)
            }
            Err(_) => Err(std::io::Error::other(
                "auth refresh lock acquisition task ended unexpectedly",
            )),
        }
    }

    pub(super) async fn release(mut self) -> std::io::Result<()> {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
        self.join_handle
            .await
            .map_err(|err| std::io::Error::other(format!("auth refresh lock task failed: {err}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn auth_refresh_lock_path_is_under_codex_home() {
        let codex_home = PathBuf::from("/tmp/example-codex-home");

        assert_eq!(
            auth_refresh_lock_path(&codex_home),
            codex_home.join(AUTH_REFRESH_LOCK_FILE)
        );
    }

    #[test]
    fn open_auth_refresh_lock_creates_lock_file() {
        let codex_home = tempdir().expect("tempdir");

        open_auth_refresh_lock(codex_home.path()).expect("open lock");

        assert!(auth_refresh_lock_path(codex_home.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn open_auth_refresh_lock_creates_unix_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let codex_home = tempdir().expect("tempdir");
        let lock_path = auth_refresh_lock_path(codex_home.path());

        open_auth_refresh_lock(codex_home.path()).expect("open lock");

        let mode = std::fs::metadata(lock_path)
            .expect("lock metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
