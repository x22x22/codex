use std::pin::pin;
use std::sync::Arc;
use std::sync::Once;

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
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use rusty_v8 as v8;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::runtime::Handle;
use tokio::runtime::RuntimeFlavor;

const CODE_MODE_BOOTSTRAP_SOURCE: &str = include_str!("code_mode_bridge.js");
const CODE_MODE_BOOTSTRAP_FILENAME: &str = "code_mode_bootstrap.js";
const CODE_MODE_MAIN_FILENAME: &str = "code_mode_main.mjs";
const CODE_MODE_TOOLS_MODULE_NAME: &str = "tools.js";

static CODE_MODE_V8_INIT: Once = Once::new();

#[derive(Clone)]
struct ExecContext {
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    tracker: SharedTurnDiffTracker,
}

struct CodeModeRuntimeState {
    exec: ExecContext,
    enabled_tools: Vec<EnabledTool>,
    tools_module: Option<v8::Global<v8::Module>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CodeModeToolKind {
    Function,
    Freeform,
}

#[derive(Clone, Debug, Serialize)]
struct EnabledTool {
    name: String,
    kind: CodeModeToolKind,
}

pub(crate) fn instructions(config: &Config) -> Option<String> {
    if !config.features.enabled(Feature::CodeMode) {
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
    section.push_str("- `add_content(value)` is synchronous. It accepts a content item or an array of content items, so `add_content(await exec_command(...))` returns the same content items a direct tool call would expose to the model.\n");
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
    let bootstrap_source =
        build_bootstrap_source(&enabled_tools).map_err(FunctionCallError::RespondToModel)?;
    execute_v8(exec, code, enabled_tools, bootstrap_source)
        .map_err(FunctionCallError::RespondToModel)
}

fn execute_v8(
    exec: ExecContext,
    code: String,
    enabled_tools: Vec<EnabledTool>,
    bootstrap_source: String,
) -> Result<Vec<FunctionCallOutputContentItem>, String> {
    init_v8();

    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    isolate.set_capture_stack_trace_for_uncaught_exceptions(true, 32);
    isolate.set_host_import_module_dynamically_callback(code_mode_dynamic_import_callback);
    isolate.set_slot(CodeModeRuntimeState {
        exec,
        enabled_tools: enabled_tools.clone(),
        tools_module: None,
    });

    let scope = pin!(v8::HandleScope::new(&mut isolate));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    install_tool_call_binding(scope)?;
    run_script(scope, CODE_MODE_BOOTSTRAP_FILENAME, &bootstrap_source)?;

    let tools_module = create_tools_module(scope, &enabled_tools)?;
    let tools_module = v8::Global::new(scope, tools_module);
    let Some(runtime_state) = scope.get_slot_mut::<CodeModeRuntimeState>() else {
        return Err("code_mode runtime state missing".to_string());
    };
    runtime_state.tools_module = Some(tools_module);

    let scope = pin!(v8::TryCatch::new(scope));
    let scope = &mut scope.init();
    let filename = v8_string(scope, CODE_MODE_MAIN_FILENAME)?;
    let source = v8_string(scope, &code)?;
    let origin = script_origin(scope, filename, true);
    let mut source = v8::script_compiler::Source::new(source, Some(&origin));
    let Some(module) = v8::script_compiler::compile_module(scope, &mut source) else {
        return Err(format_v8_exception(scope));
    };
    let Some(instantiated) = module.instantiate_module(scope, resolve_code_mode_module) else {
        return Err(format_v8_exception(scope));
    };
    if !instantiated {
        return Err("failed to instantiate code_mode module".to_string());
    }

    let Some(result) = module.evaluate(scope) else {
        return Err(format_v8_exception(scope));
    };
    if result.is_promise() {
        let promise = v8::Local::<v8::Promise>::try_from(result)
            .map_err(|_| "code_mode module evaluation did not return a promise".to_string())?;
        wait_for_module_promise(scope, module, promise)?;
    } else {
        scope.perform_microtask_checkpoint();
    }

    read_content_items(scope)
}

fn init_v8() {
    CODE_MODE_V8_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

fn install_tool_call_binding(scope: &mut v8::PinScope<'_, '_>) -> Result<(), String> {
    let function = v8::Function::new(scope, code_mode_tool_call_callback)
        .ok_or_else(|| "failed to install code_mode tool bridge".to_string())?;
    let key = v8_string(scope, "__codex_tool_call")?;
    let global = scope.get_current_context().global(scope);
    if global.set(scope, key.into(), function.into()).is_some() {
        Ok(())
    } else {
        Err("failed to bind __codex_tool_call".to_string())
    }
}

fn run_script(
    scope: &mut v8::PinScope<'_, '_>,
    filename: &str,
    source: &str,
) -> Result<(), String> {
    let scope = pin!(v8::TryCatch::new(scope));
    let scope = &mut scope.init();
    let source = v8_string(scope, source)?;
    let filename = v8_string(scope, filename)?;
    let origin = script_origin(scope, filename, false);
    let Some(script) = v8::Script::compile(scope, source, Some(&origin)) else {
        return Err(format_v8_exception(scope));
    };
    if script.run(scope).is_none() {
        return Err(format_v8_exception(scope));
    }
    Ok(())
}

fn script_origin<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    filename: v8::Local<'s, v8::String>,
    is_module: bool,
) -> v8::ScriptOrigin<'s> {
    v8::ScriptOrigin::new(
        scope,
        filename.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        is_module,
        None,
    )
}

fn create_tools_module<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    enabled_tools: &[EnabledTool],
) -> Result<v8::Local<'s, v8::Module>, String> {
    let mut export_names = vec![v8_string(scope, "tools")?];
    for tool in enabled_tools {
        if tool.name != "tools" && is_valid_identifier(&tool.name) {
            export_names.push(v8_string(scope, &tool.name)?);
        }
    }
    let module_name = v8_string(scope, CODE_MODE_TOOLS_MODULE_NAME)?;
    Ok(v8::Module::create_synthetic_module(
        scope,
        module_name,
        &export_names,
        evaluate_tools_module,
    ))
}

fn evaluate_tools_module<'s>(
    context: v8::Local<'s, v8::Context>,
    module: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Value>> {
    v8::callback_scope!(unsafe scope, context);
    let Some(runtime_state) = scope.get_slot::<CodeModeRuntimeState>() else {
        return throw_v8_exception(scope, "code_mode runtime state missing");
    };
    let enabled_tools = runtime_state.enabled_tools.clone();
    let global = context.global(scope);

    let tools_name = match v8::String::new(scope, "tools") {
        Some(name) => name,
        None => return throw_v8_exception(scope, "failed to allocate tools export name"),
    };
    let Some(tools_value) = global.get(scope, tools_name.into()) else {
        return throw_v8_exception(scope, "code_mode tools namespace missing");
    };
    let Ok(tools_object) = v8::Local::<v8::Object>::try_from(tools_value) else {
        return throw_v8_exception(scope, "code_mode tools namespace is not an object");
    };
    module.set_synthetic_module_export(scope, tools_name, tools_object.into())?;

    for tool in enabled_tools {
        if !is_valid_identifier(&tool.name) || tool.name == "tools" {
            continue;
        }
        let Some(export_name) = v8::String::new(scope, &tool.name) else {
            return throw_v8_exception(scope, "failed to allocate tool export name");
        };
        let Some(export_value) = tools_object.get(scope, export_name.into()) else {
            return throw_v8_exception(
                scope,
                &format!("code_mode tool export `{}` is unavailable", tool.name),
            );
        };
        module.set_synthetic_module_export(scope, export_name, export_value)?;
    }

    Some(v8::undefined(scope).into())
}

fn resolve_code_mode_module<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);
    match resolve_tools_module(scope, specifier) {
        Ok(module) => Some(module),
        Err(error) => throw_v8_exception(scope, &error),
    }
}

fn code_mode_dynamic_import_callback<'s, 'i>(
    scope: &mut v8::PinScope<'s, 'i>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    _resource_name: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);

    match resolve_tools_module(scope, specifier).and_then(|module| {
        instantiate_and_evaluate_tools_module(scope, module)?;
        Ok(module.get_module_namespace())
    }) {
        Ok(namespace) => {
            let _ = resolver.resolve(scope, namespace);
        }
        Err(error) => {
            let error = v8_string(scope, &error).ok()?;
            let _ = resolver.reject(scope, error.into());
        }
    }

    Some(promise)
}

fn resolve_tools_module<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    specifier: v8::Local<'s, v8::String>,
) -> Result<v8::Local<'s, v8::Module>, String> {
    let specifier = specifier.to_rust_string_lossy(scope);
    if specifier != CODE_MODE_TOOLS_MODULE_NAME {
        return Err(format!("Unsupported import in code_mode: {specifier}"));
    }

    let Some(runtime_state) = scope.get_slot::<CodeModeRuntimeState>() else {
        return Err("code_mode runtime state missing".to_string());
    };
    let Some(tools_module) = runtime_state.tools_module.as_ref() else {
        return Err("code_mode tools module missing".to_string());
    };
    Ok(v8::Local::new(scope, tools_module))
}

fn instantiate_and_evaluate_tools_module(
    scope: &mut v8::PinScope<'_, '_>,
    module: v8::Local<'_, v8::Module>,
) -> Result<(), String> {
    match module.get_status() {
        v8::ModuleStatus::Uninstantiated => {
            let Some(instantiated) = module.instantiate_module(scope, resolve_code_mode_module)
            else {
                return Err("failed to instantiate code_mode tools module".to_string());
            };
            if !instantiated {
                return Err("failed to instantiate code_mode tools module".to_string());
            }
        }
        v8::ModuleStatus::Instantiating => {
            return Err("code_mode tools module is already instantiating".to_string());
        }
        v8::ModuleStatus::Instantiated
        | v8::ModuleStatus::Evaluating
        | v8::ModuleStatus::Evaluated => {}
        v8::ModuleStatus::Errored => {
            return Err(format_v8_value(scope, module.get_exception()));
        }
    }

    match module.get_status() {
        v8::ModuleStatus::Instantiated => {
            let Some(result) = module.evaluate(scope) else {
                return Err("failed to evaluate code_mode tools module".to_string());
            };
            if result.is_promise() {
                let promise = v8::Local::<v8::Promise>::try_from(result).map_err(|_| {
                    "code_mode tools module evaluation did not return a promise".to_string()
                })?;
                scope.perform_microtask_checkpoint();
                match promise.state() {
                    v8::PromiseState::Fulfilled => {}
                    v8::PromiseState::Rejected => {
                        return Err(format_v8_value(scope, promise.result(scope)));
                    }
                    v8::PromiseState::Pending => {
                        return Err("code_mode tools module evaluation did not settle".to_string());
                    }
                }
            }
        }
        v8::ModuleStatus::Evaluated => {}
        v8::ModuleStatus::Evaluating => {
            return Err("code_mode tools module is already evaluating".to_string());
        }
        v8::ModuleStatus::Errored => {
            return Err(format_v8_value(scope, module.get_exception()));
        }
        v8::ModuleStatus::Uninstantiated | v8::ModuleStatus::Instantiating => {}
    }

    Ok(())
}

fn code_mode_tool_call_callback(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    let promise = resolver.get_promise(scope);
    rv.set(promise.into());

    let result = run_tool_call(scope, &args).and_then(|value| json_to_v8(scope, &value));
    match result {
        Ok(value) => {
            let _ = resolver.resolve(scope, value);
        }
        Err(error) => {
            if let Some(error) = v8::String::new(scope, &error) {
                let _ = resolver.reject(scope, error.into());
            }
        }
    }
}

fn run_tool_call(
    scope: &mut v8::PinScope<'_, '_>,
    args: &v8::FunctionCallbackArguments,
) -> Result<JsonValue, String> {
    let tool_name = args
        .get(0)
        .to_string(scope)
        .ok_or_else(|| "code_mode tool call requires a tool name".to_string())?
        .to_rust_string_lossy(scope);
    let input = json_from_v8(scope, args.get(1))?;

    let Some(exec) = scope
        .get_slot::<CodeModeRuntimeState>()
        .map(|runtime_state| runtime_state.exec.clone())
    else {
        return Err("code_mode runtime state missing".to_string());
    };

    let content_items = match Handle::current().runtime_flavor() {
        RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| {
            Handle::current().block_on(call_nested_tool(exec, tool_name, input))
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

fn wait_for_module_promise(
    scope: &mut v8::PinScope<'_, '_>,
    module: v8::Local<'_, v8::Module>,
    promise: v8::Local<'_, v8::Promise>,
) -> Result<(), String> {
    for _ in 0..32 {
        match promise.state() {
            v8::PromiseState::Fulfilled => return Ok(()),
            v8::PromiseState::Rejected => {
                return Err(format_v8_value(scope, promise.result(scope)));
            }
            v8::PromiseState::Pending => {
                scope.perform_microtask_checkpoint();
            }
        }
    }

    let stalled = module.get_stalled_top_level_await_message(scope);
    if let Some((_module, message)) = stalled.into_iter().next() {
        let pending = message.get(scope).to_rust_string_lossy(scope);
        let filename = message
            .get_script_resource_name(scope)
            .map(|name| name.to_rust_string_lossy(scope))
            .unwrap_or_else(|| CODE_MODE_MAIN_FILENAME.to_string());
        let line = message.get_line_number(scope).unwrap_or_default();
        return Err(format!("{filename}:{line}: {pending}"));
    }

    Err("code_mode top-level await did not settle".to_string())
}

fn read_content_items(
    scope: &mut v8::PinScope<'_, '_>,
) -> Result<Vec<FunctionCallOutputContentItem>, String> {
    let source = v8_string(
        scope,
        "JSON.stringify(globalThis.__codexContentItems ?? [])",
    )?;
    let script = v8::Script::compile(scope, source, None)
        .ok_or_else(|| "failed to read code_mode content items".to_string())?;
    let value = script
        .run(scope)
        .ok_or_else(|| "failed to evaluate code_mode content items".to_string())?;
    let serialized = value
        .to_string(scope)
        .ok_or_else(|| "failed to serialize code_mode content items".to_string())?
        .to_rust_string_lossy(scope);
    let content_items = serde_json::from_str::<Vec<JsonValue>>(&serialized)
        .map_err(|err| format!("invalid code_mode content items: {err}"))?;
    output_content_items_from_json_values(content_items)
}

fn build_bootstrap_source(enabled_tools: &[EnabledTool]) -> Result<String, String> {
    let enabled_tools_json = serde_json::to_string(enabled_tools)
        .map_err(|err| format!("failed to serialize enabled tools: {err}"))?;
    Ok(CODE_MODE_BOOTSTRAP_SOURCE.replace(
        "__CODE_MODE_ENABLED_TOOLS_PLACEHOLDER__",
        &enabled_tools_json,
    ))
}

fn v8_string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    text: &str,
) -> Result<v8::Local<'s, v8::String>, String> {
    v8::String::new(scope, text).ok_or_else(|| "failed to allocate V8 string".to_string())
}

fn json_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> Result<Option<JsonValue>, String> {
    if value.is_undefined() {
        return Ok(None);
    }

    let Some(serialized) = v8::json::stringify(scope, value) else {
        return Err("code_mode tool arguments must be JSON-serializable".to_string());
    };
    let serialized = serialized.to_rust_string_lossy(scope);
    serde_json::from_str(&serialized)
        .map(Some)
        .map_err(|err| format!("invalid code_mode tool arguments: {err}"))
}

fn json_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &JsonValue,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let serialized = serde_json::to_string(value)
        .map_err(|err| format!("failed to serialize code_mode tool response: {err}"))?;
    let serialized = v8_string(scope, &serialized)?;
    v8::json::parse(scope, serialized)
        .ok_or_else(|| "failed to deserialize code_mode tool response into V8".to_string())
}

fn throw_v8_exception<'s, T>(
    scope: &mut v8::PinScope<'s, '_>,
    message: &str,
) -> Option<v8::Local<'s, T>> {
    if let Some(message) = v8::String::new(scope, message) {
        scope.throw_exception(message.into());
    }
    None
}

fn format_v8_exception(try_catch: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>) -> String {
    let Some(exception) = try_catch.exception() else {
        return "JavaScript execution failed".to_string();
    };

    if let Some(stack_trace) = try_catch.stack_trace()
        && let Some(stack_trace) = stack_trace.to_string(try_catch)
    {
        let stack_trace = stack_trace.to_rust_string_lossy(try_catch);
        if !stack_trace.trim().is_empty() {
            return stack_trace;
        }
    }

    let exception_string = exception
        .to_string(try_catch)
        .map(|value| value.to_rust_string_lossy(try_catch))
        .unwrap_or_else(|| "JavaScript execution failed".to_string());
    let Some(message) = try_catch.message() else {
        return exception_string;
    };

    let filename = message
        .get_script_resource_name(try_catch)
        .and_then(|value| value.to_string(try_catch))
        .map(|value| value.to_rust_string_lossy(try_catch))
        .unwrap_or_else(|| "(unknown)".to_string());
    let line = message.get_line_number(try_catch).unwrap_or_default();
    format!("{filename}:{line}: {exception_string}")
}

fn format_v8_value(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<'_, v8::Value>) -> String {
    if value.is_object()
        && let Ok(object) = v8::Local::<v8::Object>::try_from(value)
        && let Some(stack_key) = v8::String::new(scope, "stack")
        && let Some(stack_value) = object.get(scope, stack_key.into())
        && let Some(stack_value) = stack_value.to_string(scope)
    {
        let stack_value = stack_value.to_rust_string_lossy(scope);
        if !stack_value.trim().is_empty() {
            return stack_value;
        }
    }

    value
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "JavaScript execution failed".to_string())
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
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
        ResponseInputItem::McpToolCallOutput { result, .. } => match result {
            Ok(result) => {
                content_items_from_function_output(FunctionCallOutputPayload::from(&result))
            }
            Err(error) => vec![FunctionCallOutputContentItem::InputText { text: error }],
        },
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
