use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use codex_exec_server::ExecutorFileSystem;
use codex_utils_absolute_path::AbsolutePathBuf;
use regex_lite::Regex;
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::timeout;
use wildmatch::WildMatch;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct GrepFilesHandler;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 2000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[derive(Deserialize)]
struct GrepFilesArgs {
    pattern: String,
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[async_trait]
impl ToolHandler for GrepFilesHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation { payload, turn, .. } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "grep_files handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: GrepFilesArgs = parse_arguments(&arguments)?;

        let pattern = args.pattern.trim();
        if pattern.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "pattern must not be empty".to_string(),
            ));
        }

        if args.limit == 0 {
            return Err(FunctionCallError::RespondToModel(
                "limit must be greater than zero".to_string(),
            ));
        }

        let limit = args.limit.min(MAX_LIMIT);
        let search_path = turn.resolve_path(args.path.clone());

        let include = args.include.as_deref().map(str::trim).and_then(|val| {
            if val.is_empty() {
                None
            } else {
                Some(val.to_string())
            }
        });

        let search_results = if turn.environment.experimental_exec_server_url().is_some() {
            let absolute_path = AbsolutePathBuf::try_from(search_path.clone()).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to normalize search path `{}`: {err}",
                    search_path.display()
                ))
            })?;
            let filesystem = turn.environment.get_filesystem();
            verify_path_exists_with_filesystem(&filesystem, &absolute_path).await?;
            run_search_with_filesystem(
                pattern,
                include.as_deref(),
                &absolute_path,
                limit,
                &filesystem,
            )
            .await?
        } else {
            verify_path_exists(&search_path).await?;
            run_rg_search(pattern, include.as_deref(), &search_path, limit, &turn.cwd).await?
        };

        if search_results.is_empty() {
            Ok(FunctionToolOutput::from_text(
                "No matches found.".to_string(),
                Some(false),
            ))
        } else {
            Ok(FunctionToolOutput::from_text(
                search_results.join("\n"),
                Some(true),
            ))
        }
    }
}

async fn verify_path_exists(path: &Path) -> Result<(), FunctionCallError> {
    tokio::fs::metadata(path).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("unable to access `{}`: {err}", path.display()))
    })?;
    Ok(())
}

async fn verify_path_exists_with_filesystem<F>(
    filesystem: &F,
    path: &AbsolutePathBuf,
) -> Result<(), FunctionCallError>
where
    F: ExecutorFileSystem + ?Sized,
{
    filesystem.get_metadata(path).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("unable to access `{}`: {err}", path.display()))
    })?;
    Ok(())
}

async fn run_rg_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<String>, FunctionCallError> {
    let mut command = Command::new("rg");
    command
        .current_dir(cwd)
        .arg("--files-with-matches")
        .arg("--sortr=modified")
        .arg("--regexp")
        .arg(pattern)
        .arg("--no-messages");

    if let Some(glob) = include {
        command.arg("--glob").arg(glob);
    }

    command.arg("--").arg(search_path);

    let output = timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            FunctionCallError::RespondToModel("rg timed out after 30 seconds".to_string())
        })?
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to launch rg: {err}. Ensure ripgrep is installed and on PATH."
            ))
        })?;

    match output.status.code() {
        Some(0) => Ok(parse_results(&output.stdout, limit)),
        Some(1) => Ok(Vec::new()),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(FunctionCallError::RespondToModel(format!(
                "rg failed: {stderr}"
            )))
        }
    }
}

async fn run_search_with_filesystem<F>(
    pattern: &str,
    include: Option<&str>,
    search_path: &AbsolutePathBuf,
    limit: usize,
    filesystem: &F,
) -> Result<Vec<String>, FunctionCallError>
where
    F: ExecutorFileSystem + ?Sized,
{
    let regex = Regex::new(pattern).map_err(|err| {
        FunctionCallError::RespondToModel(format!("invalid regex pattern `{pattern}`: {err}"))
    })?;
    let include = include.map(WildMatch::new);
    let search_metadata = filesystem.get_metadata(search_path).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to access `{}`: {err}",
            search_path.display()
        ))
    })?;

    let mut matches = Vec::new();
    if search_metadata.is_file {
        if file_matches_regex(filesystem, search_path, include.as_ref(), &regex).await {
            let modified_at_ms = filesystem
                .get_metadata(search_path)
                .await
                .map(|metadata| metadata.modified_at_ms)
                .unwrap_or_default();
            matches.push((modified_at_ms, search_path.to_string_lossy().to_string()));
        }
    } else {
        let mut pending = vec![search_path.clone()];
        while let Some(current_dir) = pending.pop() {
            let entries = match filesystem.read_directory(&current_dir).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries {
                let Ok(entry_path) =
                    AbsolutePathBuf::try_from(current_dir.as_path().join(&entry.file_name))
                else {
                    continue;
                };
                if entry.is_directory {
                    pending.push(entry_path);
                    continue;
                }
                if !entry.is_file {
                    continue;
                }
                if !file_matches_regex(filesystem, &entry_path, include.as_ref(), &regex).await {
                    continue;
                }
                let modified_at_ms = filesystem
                    .get_metadata(&entry_path)
                    .await
                    .map(|metadata| metadata.modified_at_ms)
                    .unwrap_or_default();
                matches.push((modified_at_ms, entry_path.to_string_lossy().to_string()));
            }
        }
    }

    matches.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(matches
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect())
}

async fn file_matches_regex<F>(
    filesystem: &F,
    path: &AbsolutePathBuf,
    include: Option<&WildMatch>,
    regex: &Regex,
) -> bool
where
    F: ExecutorFileSystem + ?Sized,
{
    if let Some(glob) = include {
        let full_path = path.to_string_lossy();
        let Some(file_name) = path.as_path().file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if !glob.matches(&full_path) && !glob.matches(file_name) {
            return false;
        }
    }

    let Ok(bytes) = filesystem.read_file(path).await else {
        return false;
    };
    let contents = String::from_utf8_lossy(&bytes);
    regex.is_match(&contents)
}

fn parse_results(stdout: &[u8], limit: usize) -> Vec<String> {
    let mut results = Vec::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(line) {
            if text.is_empty() {
                continue;
            }
            results.push(text.to_string());
            if results.len() == limit {
                break;
            }
        }
    }
    results
}

#[cfg(test)]
#[path = "grep_files_tests.rs"]
mod tests;
