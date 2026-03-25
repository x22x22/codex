use crate::remote_browser_api::RemoteBrowserCommandOutcome;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallResponse;
use serde_json::json;

const CREATE_TAB_TOOL: &str = "create_tab";
const LIST_TABS_TOOL: &str = "list_tabs";
const SELECTED_TAB_TOOL: &str = "selected_tab";
const SELECT_TAB_TOOL: &str = "select_tab";
const NAVIGATE_TAB_URL_TOOL: &str = "navigate_tab_url";
const TABS_CONTENT_TOOL: &str = "tabs_content";
const WAIT_FOR_LOAD_STATE_TOOL: &str = "playwright_wait_for_load_state";
const SCREENSHOT_TOOL: &str = "playwright_screenshot";

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
