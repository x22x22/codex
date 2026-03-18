pub mod fs;

pub use fs::CopyOptions;
pub use fs::CreateDirectoryOptions;
pub use fs::ExecutorFileSystem;
pub use fs::FileMetadata;
pub use fs::FileSystemResult;
pub use fs::LocalFileSystem;
pub use fs::ReadDirectoryEntry;
pub use fs::RemoveOptions;
use std::sync::Arc;

#[derive(Clone)]
pub struct Environment {
    file_system: Arc<dyn ExecutorFileSystem>,
}

impl Environment {
    pub fn new(file_system: Arc<dyn ExecutorFileSystem>) -> Self {
        Self { file_system }
    }

    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.file_system)
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new(Arc::new(fs::LocalFileSystem))
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment").finish_non_exhaustive()
    }
}
