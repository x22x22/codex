use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use codex_app_server_protocol::FsCopyParams;
use codex_app_server_protocol::FsCreateDirectoryParams;
use codex_app_server_protocol::FsGetMetadataParams;
use codex_app_server_protocol::FsReadDirectoryParams;
use codex_app_server_protocol::FsReadFileParams;
use codex_app_server_protocol::FsRemoveParams;
use codex_app_server_protocol::FsWriteFileParams;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::fmt;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::io;

use crate::ExecServerClient;
use crate::ExecServerError;

const MAX_READ_FILE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("filesystem operation failed: {0}")]
pub struct FsError(#[source] pub io::Error);

impl From<io::Error> for FsError {
    fn from(err: io::Error) -> Self {
        Self(err)
    }
}

pub type FileSystemResult<T> = Result<T, FsError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateDirectoryOptions {
    pub recursive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoveOptions {
    pub recursive: bool,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyOptions {
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    pub is_directory: bool,
    pub is_file: bool,
    pub created_at_ms: i64,
    pub modified_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadDirectoryEntry {
    pub file_name: String,
    pub is_directory: bool,
    pub is_file: bool,
}

#[async_trait]
pub trait ExecutorFileSystem: std::fmt::Debug + Send + Sync {
    async fn read_file(&self, path: &AbsolutePathBuf) -> FileSystemResult<Vec<u8>>;

    async fn write_file(&self, path: &AbsolutePathBuf, contents: Vec<u8>) -> FileSystemResult<()>;

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: CreateDirectoryOptions,
    ) -> FileSystemResult<()>;

    async fn get_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata>;

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>>;

    async fn remove(&self, path: &AbsolutePathBuf, options: RemoveOptions) -> FileSystemResult<()>;

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: CopyOptions,
    ) -> FileSystemResult<()>;

    async fn file_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        self.get_metadata(path).await
    }

    async fn create_dir_all(&self, path: &AbsolutePathBuf) -> FileSystemResult<()> {
        self.create_directory(path, CreateDirectoryOptions { recursive: true })
            .await
    }

    async fn remove_file(&self, path: &AbsolutePathBuf) -> FileSystemResult<()> {
        self.remove(
            path,
            RemoveOptions {
                recursive: false,
                force: false,
            },
        )
        .await
    }

    async fn remove_dir_all(&self, path: &AbsolutePathBuf) -> FileSystemResult<()> {
        self.remove(
            path,
            RemoveOptions {
                recursive: true,
                force: false,
            },
        )
        .await
    }

    async fn read_dir(&self, path: &AbsolutePathBuf) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        self.read_directory(path).await
    }

    async fn symlink_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        self.file_metadata(path).await
    }

    async fn rename(&self, _from: &AbsolutePathBuf, _to: &AbsolutePathBuf) -> FileSystemResult<()> {
        Err(FsError(io::Error::new(
            io::ErrorKind::Unsupported,
            "rename is not supported by this filesystem backend",
        )))
    }

    async fn read_link(&self, _path: &AbsolutePathBuf) -> FileSystemResult<AbsolutePathBuf> {
        Err(FsError(io::Error::new(
            io::ErrorKind::Unsupported,
            "read_link is not supported by this filesystem backend",
        )))
    }
}

#[derive(Clone, Debug, Default)]
pub struct LocalFileSystem;

#[async_trait]
impl ExecutorFileSystem for LocalFileSystem {
    async fn read_file(&self, path: &AbsolutePathBuf) -> FileSystemResult<Vec<u8>> {
        let metadata = tokio::fs::metadata(path.as_path()).await?;
        if metadata.len() > MAX_READ_FILE_BYTES {
            return Err(FsError(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("file is too large to read: limit is {MAX_READ_FILE_BYTES} bytes"),
            )));
        }
        Ok(tokio::fs::read(path.as_path()).await?)
    }

    async fn write_file(&self, path: &AbsolutePathBuf, contents: Vec<u8>) -> FileSystemResult<()> {
        Ok(tokio::fs::write(path.as_path(), contents).await?)
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: CreateDirectoryOptions,
    ) -> FileSystemResult<()> {
        if options.recursive {
            tokio::fs::create_dir_all(path.as_path()).await?;
        } else {
            tokio::fs::create_dir(path.as_path()).await?;
        }
        Ok(())
    }

    async fn get_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        let metadata = tokio::fs::metadata(path.as_path()).await?;
        Ok(FileMetadata {
            is_directory: metadata.is_dir(),
            is_file: metadata.is_file(),
            created_at_ms: metadata.created().ok().map_or(0, system_time_to_unix_ms),
            modified_at_ms: metadata.modified().ok().map_or(0, system_time_to_unix_ms),
        })
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(path.as_path()).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let metadata = tokio::fs::metadata(entry.path()).await?;
            entries.push(ReadDirectoryEntry {
                file_name: entry.file_name().to_string_lossy().into_owned(),
                is_directory: metadata.is_dir(),
                is_file: metadata.is_file(),
            });
        }
        Ok(entries)
    }

    async fn remove(&self, path: &AbsolutePathBuf, options: RemoveOptions) -> FileSystemResult<()> {
        match tokio::fs::symlink_metadata(path.as_path()).await {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_dir() {
                    if options.recursive {
                        tokio::fs::remove_dir_all(path.as_path()).await?;
                    } else {
                        tokio::fs::remove_dir(path.as_path()).await?;
                    }
                } else {
                    tokio::fs::remove_file(path.as_path()).await?;
                }
                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound && options.force => Ok(()),
            Err(err) => Err(FsError(err)),
        }
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: CopyOptions,
    ) -> FileSystemResult<()> {
        let source_path = source_path.to_path_buf();
        let destination_path = destination_path.to_path_buf();
        tokio::task::spawn_blocking(move || -> FileSystemResult<()> {
            let metadata = std::fs::symlink_metadata(source_path.as_path())?;
            let file_type = metadata.file_type();

            if file_type.is_dir() {
                if !options.recursive {
                    return Err(FsError(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "fs/copy requires recursive: true when sourcePath is a directory",
                    )));
                }
                if destination_is_same_or_descendant_of_source(
                    source_path.as_path(),
                    destination_path.as_path(),
                )? {
                    return Err(FsError(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "fs/copy cannot copy a directory to itself or one of its descendants",
                    )));
                }
                copy_dir_recursive(source_path.as_path(), destination_path.as_path())?;
                return Ok(());
            }

            if file_type.is_symlink() {
                copy_symlink(source_path.as_path(), destination_path.as_path())?;
                return Ok(());
            }

            if file_type.is_file() {
                std::fs::copy(source_path.as_path(), destination_path.as_path())?;
                return Ok(());
            }

            Err(FsError(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fs/copy only supports regular files, directories, and symlinks",
            )))
        })
        .await
        .map_err(|err| FsError(io::Error::other(format!("filesystem task failed: {err}"))))?
    }

    async fn file_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        let metadata = tokio::fs::symlink_metadata(path.as_path()).await?;
        Ok(FileMetadata {
            is_directory: metadata.is_dir(),
            is_file: metadata.is_file(),
            created_at_ms: metadata.created().ok().map_or(0, system_time_to_unix_ms),
            modified_at_ms: metadata.modified().ok().map_or(0, system_time_to_unix_ms),
        })
    }

    async fn symlink_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        self.file_metadata(path).await
    }

    async fn read_link(&self, path: &AbsolutePathBuf) -> FileSystemResult<AbsolutePathBuf> {
        let target = tokio::fs::read_link(path.as_path()).await?;
        AbsolutePathBuf::try_from(target)
            .map_err(io::Error::other)
            .map_err(Into::into)
    }

    async fn rename(&self, from: &AbsolutePathBuf, to: &AbsolutePathBuf) -> FileSystemResult<()> {
        tokio::fs::rename(from.as_path(), to.as_path()).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct RemoteFileSystem {
    client: ExecServerClient,
}

impl fmt::Debug for RemoteFileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteFileSystem")
            .field("client", &"redacted")
            .finish()
    }
}

impl RemoteFileSystem {
    pub(crate) fn new(client: ExecServerClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ExecutorFileSystem for RemoteFileSystem {
    async fn read_file(&self, path: &AbsolutePathBuf) -> FileSystemResult<Vec<u8>> {
        let response = self
            .client
            .fs_read_file(FsReadFileParams { path: path.clone() })
            .await
            .map_err(map_exec_server_error)?;
        STANDARD
            .decode(response.data_base64)
            .map_err(|err| FsError(io::Error::new(io::ErrorKind::InvalidData, err)))
    }

    async fn write_file(&self, path: &AbsolutePathBuf, contents: Vec<u8>) -> FileSystemResult<()> {
        self.client
            .fs_write_file(FsWriteFileParams {
                path: path.clone(),
                data_base64: STANDARD.encode(contents),
            })
            .await
            .map_err(map_exec_server_error)?;
        Ok(())
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: CreateDirectoryOptions,
    ) -> FileSystemResult<()> {
        self.client
            .fs_create_directory(FsCreateDirectoryParams {
                path: path.clone(),
                recursive: Some(options.recursive),
            })
            .await
            .map_err(map_exec_server_error)?;
        Ok(())
    }

    async fn get_metadata(&self, path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        let response = self
            .client
            .fs_get_metadata(FsGetMetadataParams { path: path.clone() })
            .await
            .map_err(map_exec_server_error)?;
        Ok(FileMetadata {
            is_directory: response.is_directory,
            is_file: response.is_file,
            created_at_ms: response.created_at_ms,
            modified_at_ms: response.modified_at_ms,
        })
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        let response = self
            .client
            .fs_read_directory(FsReadDirectoryParams { path: path.clone() })
            .await
            .map_err(map_exec_server_error)?;
        Ok(response
            .entries
            .into_iter()
            .map(|entry| ReadDirectoryEntry {
                file_name: entry.file_name,
                is_directory: entry.is_directory,
                is_file: entry.is_file,
            })
            .collect())
    }

    async fn remove(&self, path: &AbsolutePathBuf, options: RemoveOptions) -> FileSystemResult<()> {
        self.client
            .fs_remove(FsRemoveParams {
                path: path.clone(),
                recursive: Some(options.recursive),
                force: Some(options.force),
            })
            .await
            .map_err(map_exec_server_error)?;
        Ok(())
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: CopyOptions,
    ) -> FileSystemResult<()> {
        self.client
            .fs_copy(FsCopyParams {
                source_path: source_path.clone(),
                destination_path: destination_path.clone(),
                recursive: options.recursive,
            })
            .await
            .map_err(map_exec_server_error)?;
        Ok(())
    }

    async fn read_link(&self, _path: &AbsolutePathBuf) -> FileSystemResult<AbsolutePathBuf> {
        Err(FsError(io::Error::new(
            io::ErrorKind::Unsupported,
            "read_link is not supported by remote exec-server filesystem",
        )))
    }

    async fn symlink_metadata(&self, _path: &AbsolutePathBuf) -> FileSystemResult<FileMetadata> {
        Err(FsError(io::Error::new(
            io::ErrorKind::Unsupported,
            "symlink_metadata is not supported by remote exec-server filesystem",
        )))
    }

    async fn rename(&self, _from: &AbsolutePathBuf, _to: &AbsolutePathBuf) -> FileSystemResult<()> {
        Err(FsError(io::Error::new(
            io::ErrorKind::Unsupported,
            "rename is not supported by remote exec-server filesystem",
        )))
    }
}

fn map_exec_server_error(err: ExecServerError) -> FsError {
    match err {
        ExecServerError::Server {
            code: -32600 | -32602,
            message,
        } => io::Error::new(io::ErrorKind::InvalidInput, message).into(),
        other => io::Error::other(other.to_string()).into(),
    }
}

fn copy_dir_recursive(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &target_path)?;
        } else if file_type.is_symlink() {
            copy_symlink(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn destination_is_same_or_descendant_of_source(
    source: &Path,
    destination: &Path,
) -> io::Result<bool> {
    let source = std::fs::canonicalize(source)?;
    let destination = resolve_copy_destination_path(destination)?;
    Ok(destination.starts_with(&source))
}

fn resolve_copy_destination_path(path: &Path) -> io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    let mut unresolved_suffix = Vec::new();
    let mut existing_path = normalized.as_path();
    while !existing_path.exists() {
        let Some(file_name) = existing_path.file_name() else {
            break;
        };
        unresolved_suffix.push(file_name.to_os_string());
        let Some(parent) = existing_path.parent() else {
            break;
        };
        existing_path = parent;
    }

    let mut resolved = std::fs::canonicalize(existing_path)?;
    for file_name in unresolved_suffix.iter().rev() {
        resolved.push(file_name);
    }
    Ok(resolved)
}

fn copy_symlink(source: &Path, target: &Path) -> io::Result<()> {
    let link_target = std::fs::read_link(source)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link_target, target)
    }
    #[cfg(windows)]
    {
        if symlink_points_to_directory(source)? {
            std::os::windows::fs::symlink_dir(&link_target, target)
        } else {
            std::os::windows::fs::symlink_file(&link_target, target)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = link_target;
        let _ = target;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "copying symlinks is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn symlink_points_to_directory(source: &Path) -> io::Result<bool> {
    use std::os::windows::fs::FileTypeExt;

    Ok(std::fs::symlink_metadata(source)?
        .file_type()
        .is_symlink_dir())
}

fn system_time_to_unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn symlink_points_to_directory_handles_dangling_directory_symlinks() -> io::Result<()> {
        use std::os::windows::fs::symlink_dir;

        let temp_dir = tempfile::TempDir::new()?;
        let source_dir = temp_dir.path().join("source");
        let link_path = temp_dir.path().join("source-link");
        std::fs::create_dir(&source_dir)?;

        if symlink_dir(&source_dir, &link_path).is_err() {
            return Ok(());
        }

        std::fs::remove_dir(&source_dir)?;

        assert_eq!(symlink_points_to_directory(&link_path)?, true);
        Ok(())
    }
}
