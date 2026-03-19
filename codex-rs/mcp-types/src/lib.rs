//! Small, stable MCP data helpers split out of `codex-core`.

use std::collections::HashMap;
use std::path::PathBuf;

use codex_protocol::mcp::Tool;
use codex_protocol::protocol::SandboxPolicy;
use serde::Deserialize;
use serde::Serialize;

const MCP_TOOL_NAME_PREFIX: &str = "mcp";
const MCP_TOOL_NAME_DELIMITER: &str = "__";

/// State needed by MCP servers to align their own sandboxing decisions with Codex.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxState {
    pub sandbox_policy: SandboxPolicy,
    pub codex_linux_sandbox_exe: Option<PathBuf>,
    pub sandbox_cwd: PathBuf,
    #[serde(default)]
    pub use_legacy_landlock: bool,
}

pub fn split_qualified_tool_name(qualified_name: &str) -> Option<(String, String)> {
    let mut parts = qualified_name.split(MCP_TOOL_NAME_DELIMITER);
    let prefix = parts.next()?;
    if prefix != MCP_TOOL_NAME_PREFIX {
        return None;
    }
    let server_name = parts.next()?;
    let tool_name: String = parts.collect::<Vec<_>>().join(MCP_TOOL_NAME_DELIMITER);
    if tool_name.is_empty() {
        None
    } else {
        Some((server_name.to_string(), tool_name))
    }
}

pub fn group_tools_by_server(
    tools: &HashMap<String, Tool>,
) -> HashMap<String, HashMap<String, Tool>> {
    let mut grouped = HashMap::new();
    for (qualified_name, tool) in tools {
        if let Some((server_name, tool_name)) = split_qualified_tool_name(qualified_name) {
            grouped
                .entry(server_name)
                .or_insert_with(HashMap::new)
                .insert(tool_name, tool.clone());
        }
    }
    grouped
}
