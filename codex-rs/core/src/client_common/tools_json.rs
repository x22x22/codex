use crate::error::Result;

use super::tools::ToolSpec;

/// Returns JSON values that are compatible with Function Calling in the
/// Responses API:
/// https://platform.openai.com/docs/guides/function-calling?api-mode=responses
///
/// This helper intentionally lives under `client_common` so the model client
/// can serialize tools without depending on the full tool registry builder in
/// `tools/spec.rs`.
pub(crate) fn create_tools_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Vec<serde_json::Value>> {
    let mut tools_json = Vec::new();

    for tool in tools {
        let json = serde_json::to_value(tool)?;
        tools_json.push(json);
    }

    Ok(tools_json)
}
