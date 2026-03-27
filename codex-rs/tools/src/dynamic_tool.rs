use crate::JsonSchema;
use crate::parse_tool_input_schema;
use codex_protocol::dynamic_tools::DynamicToolSpec;

/// Parsed dynamic tool metadata and schemas that can be adapted into a
/// higher-level tool spec by downstream crates.
#[derive(Debug, PartialEq)]
pub struct ParsedDynamicTool {
    pub description: String,
    pub input_schema: JsonSchema,
    pub defer_loading: bool,
}

pub fn parse_dynamic_tool(tool: &DynamicToolSpec) -> Result<ParsedDynamicTool, serde_json::Error> {
    Ok(ParsedDynamicTool {
        description: tool.description.clone(),
        input_schema: parse_tool_input_schema(&tool.input_schema)?,
        defer_loading: tool.defer_loading,
    })
}

#[cfg(test)]
#[path = "dynamic_tool_tests.rs"]
mod tests;
