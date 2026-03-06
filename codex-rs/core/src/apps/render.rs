use crate::mcp::CODEX_APPS_MCP_SERVER_NAME;

pub(crate) fn render_apps_section() -> String {
    format!(
        "## Apps\nApps are mentioned in the prompt in the format `[$app-name](app://{{connector_id}})`.\nAn app is equivalent to a set of MCP tools within the `{CODEX_APPS_MCP_SERVER_NAME}` MCP.\nWhen you see an app mention, its tools may already be available, or the app may need to be installed first.\nUse `search_tool_bm25` with `mode: \"available\"` to search for installed tools, before trying to call hidden app tools. If the user clearly wants a specific app and it is not available, use `search_tool_bm25` with `mode: \"installable\"`, then call `tool_suggest` to prompt the user to install it.\nDo not additionally call list_mcp_resources for apps that are already mentioned."
    )
}
