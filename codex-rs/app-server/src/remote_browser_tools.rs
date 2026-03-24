use crate::remote_browser_api::RemoteBrowserCommandOutcome;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallResponse;
use codex_app_server_protocol::DynamicToolSpec;
use serde_json::json;
use std::collections::HashSet;

const CREATE_TAB_TOOL: &str = "create_tab";
const LIST_TABS_TOOL: &str = "list_tabs";
const SELECTED_TAB_TOOL: &str = "selected_tab";
const SELECT_TAB_TOOL: &str = "select_tab";
const NAVIGATE_TAB_URL_TOOL: &str = "navigate_tab_url";
const TABS_CONTENT_TOOL: &str = "tabs_content";
const WAIT_FOR_LOAD_STATE_TOOL: &str = "playwright_wait_for_load_state";
const SCREENSHOT_TOOL: &str = "playwright_screenshot";

pub(crate) fn merge_remote_browser_dynamic_tools(
    mut tools: Vec<DynamicToolSpec>,
) -> Vec<DynamicToolSpec> {
    let existing_names = tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<HashSet<_>>();

    for tool in remote_browser_dynamic_tools() {
        if !existing_names.contains(&tool.name) {
            tools.push(tool);
        }
    }

    tools
}

pub(crate) fn is_remote_browser_dynamic_tool(name: &str) -> bool {
    matches!(
        name,
        CREATE_TAB_TOOL
            | LIST_TABS_TOOL
            | SELECTED_TAB_TOOL
            | SELECT_TAB_TOOL
            | NAVIGATE_TAB_URL_TOOL
            | TABS_CONTENT_TOOL
            | WAIT_FOR_LOAD_STATE_TOOL
            | SCREENSHOT_TOOL
    )
}

pub(crate) fn build_dynamic_tool_response(
    outcome: RemoteBrowserCommandOutcome,
) -> DynamicToolCallResponse {
    let mut content_items = vec![DynamicToolCallOutputContentItem::InputText {
        text: format_remote_browser_text_result(&outcome),
    }];

    if let Some(image_url) = outcome.screenshot_data_url() {
        content_items.push(DynamicToolCallOutputContentItem::InputImage { image_url });
    }
    if let Some(image_url) = outcome.replay_gif_data_url() {
        content_items.push(DynamicToolCallOutputContentItem::InputImage { image_url });
    }

    DynamicToolCallResponse {
        content_items,
        success: true,
    }
}

fn format_remote_browser_text_result(outcome: &RemoteBrowserCommandOutcome) -> String {
    serde_json::to_string_pretty(&json!({
        "result": outcome.result,
        "browser_state": outcome.browser_state_json(),
    }))
    .unwrap_or_else(|_| "{\"result\":\"remote browser output unavailable\"}".to_string())
}

fn remote_browser_dynamic_tools() -> Vec<DynamicToolSpec> {
    vec![
        DynamicToolSpec {
            name: CREATE_TAB_TOOL.to_string(),
            description: "Create a browser tab in the shared remote browser session. Prefer this for the first navigation by passing a URL.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "select": { "type": "boolean" },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: LIST_TABS_TOOL.to_string(),
            description: "List the current tabs in the shared remote browser session.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: SELECTED_TAB_TOOL.to_string(),
            description: "Get the currently selected tab in the shared remote browser session.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: SELECT_TAB_TOOL.to_string(),
            description: "Select an existing browser tab by tab_id.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" }
                },
                "required": ["tab_id"],
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: NAVIGATE_TAB_URL_TOOL.to_string(),
            description: "Navigate an existing tab, or the selected tab, to a URL.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "tab_id": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: TABS_CONTENT_TOOL.to_string(),
            description: "Read sanitized text content from one or more tabs. Use this to inspect a page before summarizing it.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tab_ids": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "max_chars_per_tab": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: WAIT_FOR_LOAD_STATE_TOOL.to_string(),
            description: "Wait for the selected tab, or a specific tab, to reach a Playwright load state.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" },
                    "state": {
                        "type": "string",
                        "enum": ["domcontentloaded", "load", "networkidle"]
                    },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            name: SCREENSHOT_TOOL.to_string(),
            description: "Capture a screenshot from the selected tab, or a specific tab, and return it to the model.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" },
                    "full_page": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
            defer_loading: false,
        },
    ]
}
