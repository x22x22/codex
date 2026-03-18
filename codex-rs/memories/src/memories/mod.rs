//! Shared memory filesystem and artifact utilities.

pub mod citations;
pub mod control;
pub mod storage;

use std::path::Path;
use std::path::PathBuf;

mod artifacts {
    pub(super) const ROLLOUT_SUMMARIES_SUBDIR: &str = "rollout_summaries";
    pub(super) const RAW_MEMORIES_FILENAME: &str = "raw_memories.md";
}

pub fn memory_root(codex_home: &Path) -> PathBuf {
    codex_home.join("memories")
}

pub fn rollout_summaries_dir(root: &Path) -> PathBuf {
    root.join(artifacts::ROLLOUT_SUMMARIES_SUBDIR)
}

pub fn raw_memories_file(root: &Path) -> PathBuf {
    root.join(artifacts::RAW_MEMORIES_FILENAME)
}

pub async fn ensure_layout(root: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(rollout_summaries_dir(root)).await
}
