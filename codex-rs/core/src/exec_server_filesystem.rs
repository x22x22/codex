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
use codex_environment::CopyOptions;
use codex_environment::CreateDirectoryOptions;
use codex_environment::ExecutorFileSystem;
use codex_environment::FileMetadata;
use codex_environment::FileSystemResult;
use codex_environment::ReadDirectoryEntry;
use codex_environment::RemoveOptions;
use codex_exec_server::ExecServerClient;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::io;

use crate::exec_server_path_mapper::RemoteWorkspacePathMapper;

#[derive(Clone)]
pub(crate) struct ExecServerFileSystem {
    client: ExecServerClient,
    path_mapper: Option<RemoteWorkspacePathMapper>,
}

impl ExecServerFileSystem {
    pub(crate) fn new(
        client: ExecServerClient,
        path_mapper: Option<RemoteWorkspacePathMapper>,
    ) -> Self {
        Self {
            client,
            path_mapper,
        }
    }

    fn map_path(&self, path: &AbsolutePathBuf) -> AbsolutePathBuf {
        self.path_mapper
            .as_ref()
            .map_or_else(|| path.clone(), |mapper| mapper.map_path(path))
    }
}

#[async_trait]
impl ExecutorFileSystem for ExecServerFileSystem {
    async fn read_file(&self, path: &AbsolutePathBuf) -> FileSystemResult<Vec<u8>> {
        let path = self.map_path(path);
        let response = self
            .client
            .fs_read_file(FsReadFileParams { path: path.clone() })
            .await
            .map_err(map_exec_server_error)?;
        STANDARD
            .decode(response.data_base64)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    async fn write_file(&self, path: &AbsolutePathBuf, contents: Vec<u8>) -> FileSystemResult<()> {
        let path = self.map_path(path);
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
        let path = self.map_path(path);
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
        let path = self.map_path(path);
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
        let path = self.map_path(path);
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
                is_symlink: entry.is_symlink,
            })
            .collect())
    }

    async fn remove(&self, path: &AbsolutePathBuf, options: RemoveOptions) -> FileSystemResult<()> {
        let path = self.map_path(path);
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
        let source_path = self.map_path(source_path);
        let destination_path = self.map_path(destination_path);
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
}

fn map_exec_server_error(error: codex_exec_server::ExecServerError) -> io::Error {
    match error {
        codex_exec_server::ExecServerError::Server { code: _, message } => {
            io::Error::new(io::ErrorKind::InvalidInput, message)
        }
        other => io::Error::other(other.to_string()),
    }
}
