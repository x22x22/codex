use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::FileType;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use codex_exec_server::ExecutorAttachment;
use codex_exec_server::ReadDirectoryEntry;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_string::take_bytes_at_char_boundary;
use serde::Deserialize;
use tokio::fs;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ListDirHandler {
    executor_attachment: Arc<ExecutorAttachment>,
}

impl ListDirHandler {
    pub fn new(executor_attachment: Arc<ExecutorAttachment>) -> Self {
        Self {
            executor_attachment,
        }
    }
}

const MAX_ENTRY_LENGTH: usize = 500;
const INDENTATION_SPACES: usize = 2;

fn default_offset() -> usize {
    1
}

fn default_limit() -> usize {
    25
}

fn default_depth() -> usize {
    2
}

#[derive(Deserialize)]
struct ListDirArgs {
    dir_path: String,
    #[serde(default = "default_offset")]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_depth")]
    depth: usize,
}

#[async_trait]
impl ToolHandler for ListDirHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation { payload, .. } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "list_dir handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ListDirArgs = parse_arguments(&arguments)?;

        let ListDirArgs {
            dir_path,
            offset,
            limit,
            depth,
        } = args;

        if offset == 0 {
            return Err(FunctionCallError::RespondToModel(
                "offset must be a 1-indexed entry number".to_string(),
            ));
        }

        if limit == 0 {
            return Err(FunctionCallError::RespondToModel(
                "limit must be greater than zero".to_string(),
            ));
        }

        if depth == 0 {
            return Err(FunctionCallError::RespondToModel(
                "depth must be greater than zero".to_string(),
            ));
        }

        let path = PathBuf::from(&dir_path);
        if !path.is_absolute() {
            return Err(FunctionCallError::RespondToModel(
                "dir_path must be an absolute path".to_string(),
            ));
        }

        let entries =
            list_dir_slice(&self.executor_attachment, &path, offset, limit, depth).await?;
        let mut output = Vec::with_capacity(entries.len() + 1);
        output.push(format!("Absolute path: {}", path.display()));
        output.extend(entries);
        Ok(FunctionToolOutput::from_text(output.join("\n"), Some(true)))
    }
}

async fn list_dir_slice(
    executor_attachment: &ExecutorAttachment,
    path: &Path,
    offset: usize,
    limit: usize,
    depth: usize,
) -> Result<Vec<String>, FunctionCallError> {
    let mut entries = Vec::new();
    collect_entries(
        executor_attachment,
        path,
        Path::new(""),
        depth,
        &mut entries,
    )
    .await?;

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    let start_index = offset - 1;
    if start_index >= entries.len() {
        return Err(FunctionCallError::RespondToModel(
            "offset exceeds directory entry count".to_string(),
        ));
    }

    let remaining_entries = entries.len() - start_index;
    let capped_limit = limit.min(remaining_entries);
    let end_index = start_index + capped_limit;
    let selected_entries = &entries[start_index..end_index];
    let mut formatted = Vec::with_capacity(selected_entries.len());

    for entry in selected_entries {
        formatted.push(format_entry_line(entry));
    }

    if end_index < entries.len() {
        formatted.push(format!("More than {capped_limit} entries found"));
    }

    Ok(formatted)
}

async fn collect_entries(
    executor_attachment: &ExecutorAttachment,
    dir_path: &Path,
    relative_prefix: &Path,
    depth: usize,
    entries: &mut Vec<DirEntry>,
) -> Result<(), FunctionCallError> {
    let mut queue = VecDeque::new();
    queue.push_back((dir_path.to_path_buf(), relative_prefix.to_path_buf(), depth));

    while let Some((current_dir, prefix, remaining_depth)) = queue.pop_front() {
        let mut dir_entries = Vec::new();

        if executor_attachment.exec_server_url().is_some() {
            collect_remote_dir_entries(
                executor_attachment,
                &current_dir,
                &prefix,
                &mut dir_entries,
            )
            .await?;
        } else {
            collect_local_dir_entries(&current_dir, &prefix, &mut dir_entries).await?;
        }

        dir_entries.sort_unstable_by(|a, b| a.3.name.cmp(&b.3.name));

        for (entry_path, relative_path, kind, dir_entry) in dir_entries {
            if kind == DirEntryKind::Directory && remaining_depth > 1 {
                queue.push_back((entry_path, relative_path, remaining_depth - 1));
            }
            entries.push(dir_entry);
        }
    }

    Ok(())
}

async fn collect_local_dir_entries(
    current_dir: &Path,
    prefix: &Path,
    dir_entries: &mut Vec<(PathBuf, PathBuf, DirEntryKind, DirEntry)>,
) -> Result<(), FunctionCallError> {
    let mut read_dir = fs::read_dir(current_dir).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read directory: {err}"))
    })?;

    while let Some(entry) = read_dir.next_entry().await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read directory: {err}"))
    })? {
        let file_type = entry.file_type().await.map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to inspect entry: {err}"))
        })?;
        let file_name = entry.file_name();
        let relative_path = relative_child_path(prefix, &file_name);
        let kind = DirEntryKind::from(&file_type);
        dir_entries.push((
            entry.path(),
            relative_path.clone(),
            kind,
            to_dir_entry(relative_path, &file_name, kind),
        ));
    }

    Ok(())
}

async fn collect_remote_dir_entries(
    executor_attachment: &ExecutorAttachment,
    current_dir: &Path,
    prefix: &Path,
    dir_entries: &mut Vec<(PathBuf, PathBuf, DirEntryKind, DirEntry)>,
) -> Result<(), FunctionCallError> {
    let current_dir = AbsolutePathBuf::from_absolute_path(current_dir).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to inspect directory path: {err}"))
    })?;
    let entries = executor_attachment
        .get_filesystem()
        .read_directory(&current_dir)
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read directory: {err}"))
        })?;

    for entry in entries {
        let file_name = OsStr::new(entry.file_name.as_str());
        let relative_path = relative_child_path(prefix, file_name);
        let kind = DirEntryKind::from(&entry);
        dir_entries.push((
            current_dir
                .join(entry.file_name.as_str())
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to inspect entry path: {err}"
                    ))
                })?
                .into_path_buf(),
            relative_path.clone(),
            kind,
            to_dir_entry(relative_path, file_name, kind),
        ));
    }

    Ok(())
}

fn relative_child_path(prefix: &Path, file_name: &OsStr) -> PathBuf {
    if prefix.as_os_str().is_empty() {
        PathBuf::from(file_name)
    } else {
        prefix.join(file_name)
    }
}

fn to_dir_entry(relative_path: PathBuf, file_name: &OsStr, kind: DirEntryKind) -> DirEntry {
    let display_name = format_entry_component(file_name);
    let display_depth = relative_path
        .parent()
        .map_or(0, |prefix| prefix.components().count());
    let sort_key = format_entry_name(&relative_path);
    DirEntry {
        name: sort_key,
        display_name,
        depth: display_depth,
        kind,
    }
}

fn format_entry_name(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace("\\", "/");
    if normalized.len() > MAX_ENTRY_LENGTH {
        take_bytes_at_char_boundary(&normalized, MAX_ENTRY_LENGTH).to_string()
    } else {
        normalized
    }
}

fn format_entry_component(name: &OsStr) -> String {
    let normalized = name.to_string_lossy();
    if normalized.len() > MAX_ENTRY_LENGTH {
        take_bytes_at_char_boundary(&normalized, MAX_ENTRY_LENGTH).to_string()
    } else {
        normalized.to_string()
    }
}

fn format_entry_line(entry: &DirEntry) -> String {
    let indent = " ".repeat(entry.depth * INDENTATION_SPACES);
    let mut name = entry.display_name.clone();
    match entry.kind {
        DirEntryKind::Directory => name.push('/'),
        DirEntryKind::Symlink => name.push('@'),
        DirEntryKind::Other => name.push('?'),
        DirEntryKind::File => {}
    }
    format!("{indent}{name}")
}

#[derive(Clone)]
struct DirEntry {
    name: String,
    display_name: String,
    depth: usize,
    kind: DirEntryKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl From<&FileType> for DirEntryKind {
    fn from(file_type: &FileType) -> Self {
        if file_type.is_symlink() {
            DirEntryKind::Symlink
        } else if file_type.is_dir() {
            DirEntryKind::Directory
        } else if file_type.is_file() {
            DirEntryKind::File
        } else {
            DirEntryKind::Other
        }
    }
}

impl From<&ReadDirectoryEntry> for DirEntryKind {
    fn from(entry: &ReadDirectoryEntry) -> Self {
        // The remote directory API currently exposes directory/file booleans, but not a symlink
        // bit, so remote listings preserve entries but cannot render the local "@" suffix.
        if entry.is_directory {
            DirEntryKind::Directory
        } else if entry.is_file {
            DirEntryKind::File
        } else {
            DirEntryKind::Other
        }
    }
}

#[cfg(test)]
#[path = "list_dir_tests.rs"]
mod tests;
