use crate::mcp::CODEX_APPS_MCP_SERVER_NAME;
use crate::mcp_tool_call::MCP_TOOL_ARGS_CODEX_KEY;
use crate::mcp_tool_call::MCP_TOOL_ARGS_ELICITATION_DESCRIPTION_KEY;
use crate::mcp_tool_call::MCP_TOOL_ARGS_META_KEY;

pub(crate) fn render_apps_section() -> String {
    format!(
        "## Apps\nApps are mentioned in user messages in the format `[$app-name](app://{{connector_id}})`.\nAn app is equivalent to a set of MCP tools within the `{CODEX_APPS_MCP_SERVER_NAME}` MCP.\nWhen you see an app mention, the app's MCP tools are either available tools in the `{CODEX_APPS_MCP_SERVER_NAME}` MCP server, or the tools do not exist because the user has not installed the app.\nFor consequential `{CODEX_APPS_MCP_SERVER_NAME}` tool calls that requires approval, you need to include user-facing approval copy in the tool arguments under:\n```json\n{{\n  \"{MCP_TOOL_ARGS_META_KEY}\": {{\n    \"{MCP_TOOL_ARGS_CODEX_KEY}\": {{\n      \"{MCP_TOOL_ARGS_ELICITATION_DESCRIPTION_KEY}\": \"Allow Calendar to create this event?\"\n    }}\n  }}\n}}\n```\nUse `{MCP_TOOL_ARGS_META_KEY}.{MCP_TOOL_ARGS_CODEX_KEY}.{MCP_TOOL_ARGS_ELICITATION_DESCRIPTION_KEY}` only for consequential app tools that may require approval. Make it a short user-facing approval sentence, not execution data, and do not rely on it being forwarded to the app.\nDo not additionally call list_mcp_resources for apps that are already mentioned."
    )
}

#[cfg(test)]
mod tests {
    use super::render_apps_section;

    #[test]
    fn render_apps_section_mentions_elicitation_description_contract() {
        let rendered = render_apps_section();

        assert!(rendered.contains("`_meta._codex.elicitation_description`"));
        assert!(
            rendered
                .contains("\"elicitation_description\": \"Allow Calendar to create this event?\"")
        );
        assert!(rendered.contains("not execution data"));
        assert!(rendered.contains("do not rely on it being forwarded to the app"));
    }
}
