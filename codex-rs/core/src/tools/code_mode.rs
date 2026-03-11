use std::collections::HashMap;
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
use crate::truncate::TruncationPolicy;
use crate::truncate::formatted_truncate_text_content_items_with_policy;
use crate::truncate::truncate_function_output_items_with_policy;
use crate::unified_exec::resolve_max_tokens;
use codex_code_mode::EnabledTool;
use codex_code_mode::ToolKind as CodeModeToolKind;
use codex_code_mode::execute as execute_code_mode;
use codex_protocol::models::FunctionCallOutputContentItem;
use serde_json::Value as JsonValue;
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
    section.push_str("- Import nested tools from `tools.js`, for example `import { exec_command } from \"tools.js\"` or `import { tools } from \"tools.js\"`. Namespaced tools are also available from `tools/<namespace...>.js`; MCP tools use `tools/mcp/<server>.js`, for example `import { append_notebook_logs_chart } from \"tools/mcp/ologs.js\"`. `tools[name]` and identifier wrappers like `await exec_command(args)` remain available for compatibility. Nested tool calls resolve to their code-mode result values.\n");
    section.push_str("- Import `{ output_text, output_image, set_max_output_tokens_per_exec_call, store, load }` from `@openai/code_mode` (or `\"openai/code_mode\"`). `output_text(value)` surfaces text back to the model and stringifies non-string objects with `JSON.stringify(...)` when possible. `output_image(imageUrl)` appends an `input_image` content item for `http(s)` or `data:` URLs. `store(key, value)` persists JSON-serializable values across `code_mode` calls in the current session, and `load(key)` returns a cloned stored value or `undefined`. `set_max_output_tokens_per_exec_call(value)` sets the token budget used to truncate the final Rust-side result of the current `code_mode` execution; the default is `10000`. This guards the overall `code_mode` output, not individual nested tool invocations. When truncation happens, the final text uses the unified-exec style `Original token count:` / `Output:` wrapper and the usual `…N tokens truncated…` marker.\n");
    section.push_str(
        "- Function tools require JSON object arguments. Freeform tools require raw strings.\n",
    );
    section.push_str("- `add_content(value)` remains available for compatibility. It is synchronous and accepts a content item, an array of content items, or a string. Structured nested-tool results should be converted to text first, for example with `JSON.stringify(...)`.\n");
    section
        .push_str("- Only content passed to `output_text(...)`, `output_image(...)`, or `add_content(value)` is surfaced back to the model.");
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
    let enabled_tools = build_enabled_tools(&exec).await;
    let stored_values = exec.session.services.code_mode_store.stored_values().await;
    let callback_exec = exec.clone();
    let result = execute_code_mode(
        code,
        enabled_tools,
        stored_values,
        Box::new(move |tool_name, input| run_tool_call(&callback_exec, tool_name, input)),
    )
    .map_err(FunctionCallError::RespondToModel)?;
    exec.session
        .services
        .code_mode_store
        .replace_stored_values(result.stored_values)
        .await;
    let items = output_content_items_from_json_values(result.content_items)
        .map_err(FunctionCallError::RespondToModel)?;
    Ok(truncate_code_mode_result(
        items,
        Some(result.max_output_tokens_per_exec_call),
    ))
}

fn truncate_code_mode_result(
    items: Vec<FunctionCallOutputContentItem>,
    max_output_tokens_per_exec_call: Option<usize>,
) -> Vec<FunctionCallOutputContentItem> {
    let max_output_tokens = resolve_max_tokens(max_output_tokens_per_exec_call);
    if items
        .iter()
        .all(|item| matches!(item, FunctionCallOutputContentItem::InputText { .. }))
    {
        let (mut truncated_items, original_token_count) =
            formatted_truncate_text_content_items_with_policy(
                &items,
                TruncationPolicy::Tokens(max_output_tokens),
            );
        if let Some(original_token_count) = original_token_count
            && let Some(FunctionCallOutputContentItem::InputText { text }) =
                truncated_items.first_mut()
        {
            *text = format!("Original token count: {original_token_count}\nOutput:\n{text}");
        }
        return truncated_items;
    }

    truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(max_output_tokens))
}

async fn build_enabled_tools(exec: &ExecContext) -> Vec<EnabledTool> {
    let router = build_nested_router(exec).await;
    let mcp_tool_names = exec
        .session
        .services
        .mcp_connection_manager
        .read()
        .await
        .list_all_tools()
        .await
        .into_iter()
        .map(|(qualified_name, tool_info)| {
            (
                qualified_name,
                (
                    vec!["mcp".to_string(), tool_info.server_name],
                    tool_info.tool_name,
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    for spec in router.specs() {
        let tool_name = spec.name().to_string();
        if tool_name == "code_mode" {
            continue;
        }

        let (namespace, name) = if let Some((namespace, name)) = mcp_tool_names.get(&tool_name) {
            (namespace.clone(), name.clone())
        } else {
            (Vec::new(), tool_name.clone())
        };

        out.push(EnabledTool {
            tool_name,
            namespace,
            name,
            kind: tool_kind_for_spec(&spec),
        });
    }
    out.sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
    out.dedup_by(|left, right| left.tool_name == right.tool_name);
    out
}

async fn build_nested_router(exec: &ExecContext) -> ToolRouter {
    let nested_tools_config = exec.turn.tools_config.for_code_mode_nested_tools();
    let mcp_tools = exec
        .session
        .services
        .mcp_connection_manager
        .read()
        .await
        .list_all_tools()
        .await
        .into_iter()
        .map(|(name, tool_info)| (name, tool_info.tool))
        .collect();

    ToolRouter::from_config(
        &nested_tools_config,
        Some(mcp_tools),
        None,
        exec.turn.dynamic_tools.as_slice(),
    )
}

fn run_tool_call(
    exec: &ExecContext,
    tool_name: String,
    input: Option<JsonValue>,
) -> Result<JsonValue, String> {
    match Handle::current().runtime_flavor() {
        RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| {
            Handle::current().block_on(call_nested_tool(exec.clone(), tool_name, input))
        }),
        RuntimeFlavor::CurrentThread => {
            Err("code_mode tool calls require a multi-thread Tokio runtime".to_string())
        }
        _ => Err("code_mode tool calls require a supported Tokio runtime".to_string()),
    }
}

async fn call_nested_tool(
    exec: ExecContext,
    tool_name: String,
    input: Option<JsonValue>,
) -> Result<JsonValue, String> {
    if tool_name == "code_mode" {
        return Ok(JsonValue::String("code_mode cannot invoke itself".to_string()));
    }

    let router = build_nested_router(&exec).await;
    let specs = router.specs();
    let payload = if let Some((server, tool)) = exec.session.parse_mcp_tool_name(&tool_name).await {
        ToolPayload::Mcp {
            server,
            tool,
            raw_arguments: serialize_function_tool_arguments(&tool_name, input)?,
        }
    } else {
        build_nested_tool_payload(&specs, &tool_name, input)?
    };

    let call = ToolCall {
        tool_name: tool_name.clone(),
        call_id: format!("code_mode-{}", uuid::Uuid::new_v4()),
        payload,
    };
    let result = router
        .dispatch_tool_call_with_code_mode_result(
            Arc::clone(&exec.session),
            Arc::clone(&exec.turn),
            Arc::clone(&exec.tracker),
            call,
            ToolCallSource::CodeMode,
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(result.code_mode_result())
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
    match tool_kind_for_name(specs, tool_name)? {
        CodeModeToolKind::Function => build_function_tool_payload(tool_name, input),
        CodeModeToolKind::Freeform => build_freeform_tool_payload(tool_name, input),
    }
}

fn build_function_tool_payload(
    tool_name: &str,
    input: Option<JsonValue>,
) -> Result<ToolPayload, String> {
    let arguments = serialize_function_tool_arguments(tool_name, input)?;
    Ok(ToolPayload::Function { arguments })
}

fn serialize_function_tool_arguments(
    tool_name: &str,
    input: Option<JsonValue>,
) -> Result<String, String> {
    match input {
        None => Ok("{}".to_string()),
        Some(JsonValue::Object(map)) => serde_json::to_string(&JsonValue::Object(map))
            .map_err(|err| format!("failed to serialize tool `{tool_name}` arguments: {err}")),
        Some(_) => Err(format!(
            "tool `{tool_name}` expects a JSON object for arguments"
        )),
    }
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
