use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;
use ts_rs::TS;

/// Configuration for a custom agent loaded from a markdown file.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, TS)]
pub struct CustomAgent {
    /// The agent name (derived from filename stem).
    pub name: String,
    /// Full path to the markdown file.
    pub path: PathBuf,
    /// The agent's system prompt (markdown body without frontmatter).
    pub instructions: String,
    /// Optional description shown in UI.
    pub description: Option<String>,
    /// Optional model override for this agent.
    pub model: Option<String>,
    /// Optional sandbox policy setting (defaults to "read-only" if not specified).
    pub sandbox: Option<String>,
}
