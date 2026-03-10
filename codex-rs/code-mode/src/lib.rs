use serde::Serialize;
use serde_json::Value as JsonValue;

pub type ToolCallHandler =
    dyn FnMut(String, Option<JsonValue>) -> Result<JsonValue, String> + Send + 'static;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Function,
    Freeform,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnabledTool {
    pub name: String,
    pub kind: ToolKind,
}

const MUSL_UNSUPPORTED_REASON: &str = "code_mode is unavailable on musl Linux";

pub const fn is_supported() -> bool {
    !cfg!(all(target_os = "linux", target_env = "musl"))
}

pub fn unsupported_reason() -> Option<&'static str> {
    if is_supported() {
        None
    } else {
        Some(MUSL_UNSUPPORTED_REASON)
    }
}

#[cfg(not(all(target_os = "linux", target_env = "musl")))]
mod imp;

#[cfg(not(all(target_os = "linux", target_env = "musl")))]
pub use imp::execute;

#[cfg(all(target_os = "linux", target_env = "musl"))]
pub fn execute(
    code: String,
    enabled_tools: Vec<EnabledTool>,
    on_tool_call: Box<ToolCallHandler>,
) -> Result<Vec<JsonValue>, String> {
    let _ = code;
    let _ = enabled_tools;
    let _ = on_tool_call;
    Err(MUSL_UNSUPPORTED_REASON.to_string())
}
