pub const MCP_TOOL_APPROVAL_QUESTION_ID_PREFIX: &str = "mcp_tool_call_approval";
pub const MCP_TOOL_APPROVAL_ACCEPT: &str = "Allow";
pub const MCP_TOOL_APPROVAL_ACCEPT_FOR_SESSION: &str = "Allow for this session";
pub const MCP_TOOL_APPROVAL_DECLINE_SYNTHETIC: &str = "__codex_mcp_decline__";

pub fn is_mcp_tool_approval_question_id(question_id: &str) -> bool {
    question_id
        .strip_prefix(MCP_TOOL_APPROVAL_QUESTION_ID_PREFIX)
        .is_some_and(|suffix| suffix.starts_with('_'))
}
