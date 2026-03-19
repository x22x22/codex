use std::path::PathBuf;

use codex_protocol::protocol::SandboxPolicy;
use serde::Deserialize;
use serde::Serialize;

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
