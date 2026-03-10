use std::sync::Arc;

use crate::client_common::tools::ToolSpec;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::config::Config;
use crate::features::Feature;
use crate::function_tool::FunctionCallError;
use crate::tools::ToolRouter;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolPayload;
use crate::tools::router::ToolCall;
use crate::tools::router::ToolCallSource;
use codex_code_mode::EnabledTool;
use codex_code_mode::ToolKind as CodeModeToolKind;
use codex_code_mode::execute as execute_code_mode;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::runtime::Handle;
use tokio::runtime::RuntimeFlavor;

#[derive(Clone)]
struct ExecContext {
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    tracker: SharedTurnDiffTracker,
}

pub(crate) fn instructions(config: &Config) -> Option<String> {
    if !config.features.enabled(Feature::CodeMode) || !codex_code_mode::is_supported() {
        return None;
    }

    let mut section = String::from("## Code Mode\n");
    section.push_str("- Use `code_mode` for JavaScript execution in an embedded V8 runtime.\n");
    section.push_str("- `code_mode` is a freeform/custom tool. Direct `code_mode` calls must send raw JavaScript tool input. Do not wrap code in JSON, quotes, or markdown code fences.\n");
    section.push_str("- Direct tool calls remain available while `code_mode` is enabled.\n");
    section.push_str("- Import nested tools from `tools.js`, for example `import { exec_command } from \"tools.js\"` or `import { tools } from \"tools.js\"`. `tools[name]` and identifier wrappers like `await exec_command(args)` remain available for compatibility. Nested tool calls resolve to arrays of content items.\n");
    section.push_str(
        "- Function tools require JSON object arguments. Freeform tools require raw strings.\n",
    );
    section.push_str("- `add_content(value)` is synchronous. It accepts a string, a content item, or an array of content items. `add_content(await exec_command(...))` returns the same content items a direct tool call would expose to the model, and structured results can be converted to text first with `JSON.stringify(...)` when needed.\n");
    section
        .push_str("- Only content passed to `add_content(value)` is surfaced back to the model.");
    Some(section)
}

pub(crate) async fn execute(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    tracker: SharedTurnDiffTracker,
    code: String,
) -> Result<Vec<FunctionCallOutputContentItem>, FunctionCallError> {
    let exec = ExecContext {
        session,
        turn,
        tracker,
    };
    let enabled_tools = build_enabled_tools(&exec);
    execute_code_mode(
        code,
        enabled_tools,
        Box::new(move |tool_name, input| run_tool_call(&exec, tool_name, input)),
    )
    .and_then(output_content_items_from_json_values)
    .map_err(FunctionCallError::RespondToModel)
}

fn build_enabled_tools(exec: &ExecContext) -> Vec<EnabledTool> {
    let nested_tools_config = exec.turn.tools_config.for_code_mode_nested_tools();
    let router = ToolRouter::from_config(
        &nested_tools_config,
        None,
        None,
        exec.turn.dynamic_tools.as_slice(),
    );
    let mut out = router
        .specs()
        .into_iter()
        .map(|spec| EnabledTool {
            name: spec.name().to_string(),
            kind: tool_kind_for_spec(&spec),
        })
        .filter(|tool| tool.name != "code_mode")
        .collect::<Vec<_>>();
    out.sort_by(|left, right| left.name.cmp(&right.name));
    out.dedup_by(|left, right| left.name == right.name);
    out
}

fn run_tool_call(
    exec: &ExecContext,
    tool_name: String,
    input: Option<JsonValue>,
) -> Result<JsonValue, String> {
    let content_items = match Handle::current().runtime_flavor() {
        RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| {
            Handle::current().block_on(call_nested_tool(exec.clone(), tool_name, input))
        }),
        RuntimeFlavor::CurrentThread => {
            return Err("code_mode tool calls require a multi-thread Tokio runtime".to_string());
        }
        _ => {
            return Err("code_mode tool calls require a supported Tokio runtime".to_string());
        }
    };

    Ok(JsonValue::Array(content_items))
}

async fn call_nested_tool(
    exec: ExecContext,
    tool_name: String,
    input: Option<JsonValue>,
) -> Vec<JsonValue> {
    if tool_name == "code_mode" {
        return error_content_items_json("code_mode cannot invoke itself".to_string());
    }

    let nested_config = exec.turn.tools_config.for_code_mode_nested_tools();
    let router = ToolRouter::from_config(
        &nested_config,
        None,
        None,
        exec.turn.dynamic_tools.as_slice(),
    );

    let specs = router.specs();
    let payload = match build_nested_tool_payload(&specs, &tool_name, input) {
        Ok(payload) => payload,
        Err(error) => return error_content_items_json(error),
    };

    let call = ToolCall {
        tool_name: tool_name.clone(),
        call_id: format!("code_mode-{}", uuid::Uuid::new_v4()),
        payload,
    };
    let response = router
        .dispatch_tool_call(
            Arc::clone(&exec.session),
            Arc::clone(&exec.turn),
            Arc::clone(&exec.tracker),
            call,
            ToolCallSource::CodeMode,
        )
        .await;

    match response {
        Ok(response) => {
            json_values_from_output_content_items(content_items_from_response_input(response))
        }
        Err(error) => error_content_items_json(error.to_string()),
    }
}

fn tool_kind_for_spec(spec: &ToolSpec) -> CodeModeToolKind {
    if matches!(spec, ToolSpec::Freeform(_)) {
        CodeModeToolKind::Freeform
    } else {
        CodeModeToolKind::Function
    }
}

fn tool_kind_for_name(specs: &[ToolSpec], tool_name: &str) -> Result<CodeModeToolKind, String> {
    specs
        .iter()
        .find(|spec| spec.name() == tool_name)
        .map(tool_kind_for_spec)
        .ok_or_else(|| format!("tool `{tool_name}` is not enabled in code_mode"))
}

fn build_nested_tool_payload(
    specs: &[ToolSpec],
    tool_name: &str,
    input: Option<JsonValue>,
) -> Result<ToolPayload, String> {
    let actual_kind = tool_kind_for_name(specs, tool_name)?;
    match actual_kind {
        CodeModeToolKind::Function => build_function_tool_payload(tool_name, input),
        CodeModeToolKind::Freeform => build_freeform_tool_payload(tool_name, input),
    }
}

fn build_function_tool_payload(
    tool_name: &str,
    input: Option<JsonValue>,
) -> Result<ToolPayload, String> {
    let arguments = match input {
        None => "{}".to_string(),
        Some(JsonValue::Object(map)) => serde_json::to_string(&JsonValue::Object(map))
            .map_err(|err| format!("failed to serialize tool `{tool_name}` arguments: {err}"))?,
        Some(_) => {
            return Err(format!(
                "tool `{tool_name}` expects a JSON object for arguments"
            ));
        }
    };
    Ok(ToolPayload::Function { arguments })
}

fn build_freeform_tool_payload(
    tool_name: &str,
    input: Option<JsonValue>,
) -> Result<ToolPayload, String> {
    match input {
        Some(JsonValue::String(input)) => Ok(ToolPayload::Custom { input }),
        _ => Err(format!("tool `{tool_name}` expects a string input")),
    }
}

fn content_items_from_response_input(
    response: ResponseInputItem,
) -> Vec<FunctionCallOutputContentItem> {
    match response {
        ResponseInputItem::Message { content, .. } => content
            .into_iter()
            .map(function_output_content_item_from_content_item)
            .collect(),
        ResponseInputItem::FunctionCallOutput { output, .. } => {
            content_items_from_function_output(output)
        }
        ResponseInputItem::CustomToolCallOutput { output, .. } => {
            content_items_from_function_output(output)
        }
        ResponseInputItem::McpToolCallOutput { output, .. } => {
            content_items_from_function_output(output.into_function_call_output_payload())
        }
    }
}

fn content_items_from_function_output(
    output: FunctionCallOutputPayload,
) -> Vec<FunctionCallOutputContentItem> {
    match output.body {
        FunctionCallOutputBody::Text(text) => {
            vec![FunctionCallOutputContentItem::InputText { text }]
        }
        FunctionCallOutputBody::ContentItems(items) => items,
    }
}

fn function_output_content_item_from_content_item(
    item: ContentItem,
) -> FunctionCallOutputContentItem {
    match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
            FunctionCallOutputContentItem::InputText { text }
        }
        ContentItem::InputImage { image_url } => FunctionCallOutputContentItem::InputImage {
            image_url,
            detail: None,
        },
    }
}

fn json_values_from_output_content_items(
    content_items: Vec<FunctionCallOutputContentItem>,
) -> Vec<JsonValue> {
    content_items
        .into_iter()
        .map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => {
                json!({ "type": "input_text", "text": text })
            }
            FunctionCallOutputContentItem::InputImage { image_url, detail } => {
                json!({ "type": "input_image", "image_url": image_url, "detail": detail })
            }
        })
        .collect()
}

fn output_content_items_from_json_values(
    content_items: Vec<JsonValue>,
) -> Result<Vec<FunctionCallOutputContentItem>, String> {
    content_items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            serde_json::from_value(item)
                .map_err(|err| format!("invalid code_mode content item at index {index}: {err}"))
        })
        .collect()
}

fn error_content_items_json(message: String) -> Vec<JsonValue> {
    vec![json!({ "type": "input_text", "text": message })]
}
