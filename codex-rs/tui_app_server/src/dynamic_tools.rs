use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallParams;
use codex_app_server_protocol::DynamicToolCallResponse;
use codex_app_server_protocol::DynamicToolSpec;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RequestId;
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

type DynamicToolFuture = Pin<
    Box<dyn Future<Output = Result<DynamicToolCallResponse, DynamicToolExecutionError>> + Send>,
>;
type DynamicToolExecutor =
    dyn Fn(DynamicToolExecutionContext, DynamicToolCallParams) -> DynamicToolFuture + Send + Sync;

pub(crate) struct DynamicToolRegistration {
    spec: DynamicToolSpec,
    executor: Arc<DynamicToolExecutor>,
}

impl DynamicToolRegistration {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new<F, Fut>(spec: DynamicToolSpec, executor: F) -> Self
    where
        F: Fn(DynamicToolExecutionContext, DynamicToolCallParams) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<DynamicToolCallResponse, DynamicToolExecutionError>>
            + Send
            + 'static,
    {
        let executor = Arc::new(move |context, params| Box::pin(executor(context, params)) as _);
        Self { spec, executor }
    }
}

#[derive(Default)]
pub(crate) struct DynamicToolRegistry {
    tools: HashMap<String, DynamicToolRegistration>,
}

impl DynamicToolRegistry {
    pub(crate) fn tui_owned() -> Self {
        Self::from_registrations(Vec::new())
    }

    pub(crate) fn from_registrations(registrations: Vec<DynamicToolRegistration>) -> Self {
        let tools = registrations
            .into_iter()
            .map(|registration| (registration.spec.name.clone(), registration))
            .collect();
        Self { tools }
    }

    pub(crate) fn specs(&self) -> Option<Vec<DynamicToolSpec>> {
        if self.tools.is_empty() {
            return None;
        }

        let mut specs = self
            .tools
            .values()
            .map(|registration| registration.spec.clone())
            .collect::<Vec<_>>();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        Some(specs)
    }

    pub(crate) async fn execute(
        &self,
        context: DynamicToolExecutionContext,
        params: DynamicToolCallParams,
    ) -> Result<DynamicToolCallResponse, DynamicToolExecutionError> {
        let executor = self
            .tools
            .get(&params.tool)
            .map(|registration| Arc::clone(&registration.executor))
            .ok_or_else(|| DynamicToolExecutionError::UnknownTool {
                tool: params.tool.clone(),
            })?;
        executor(context, params).await
    }
}

#[derive(Clone)]
pub(crate) struct DynamicToolExecutionContext {
    request_handle: Option<AppServerRequestHandle>,
    #[cfg_attr(not(test), allow(dead_code))]
    cwd: Option<PathBuf>,
}

impl DynamicToolExecutionContext {
    pub(crate) fn new(request_handle: AppServerRequestHandle, cwd: Option<PathBuf>) -> Self {
        Self {
            request_handle: Some(request_handle),
            cwd,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            request_handle: None,
            cwd: None,
        }
    }

    pub(crate) fn request_handle(&self) -> Option<&AppServerRequestHandle> {
        self.request_handle.as_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DynamicToolExecutionError {
    UnknownTool { tool: String },
    ExecutionFailed { tool: String, message: String },
}

impl DynamicToolExecutionError {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn failed(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            tool: tool.into(),
            message: message.into(),
        }
    }
}

pub(crate) async fn handle_dynamic_tool_call_request(
    registry: Arc<DynamicToolRegistry>,
    context: DynamicToolExecutionContext,
    request_id: RequestId,
    params: DynamicToolCallParams,
) -> Result<(), String> {
    let Some(request_handle) = context.request_handle().cloned() else {
        return Err(
            "dynamic tool execution context is missing an app-server request handle".to_string(),
        );
    };

    match registry.execute(context, params.clone()).await {
        Ok(response) => {
            let result = serde_json::to_value(response).map_err(|err| {
                format!(
                    "failed to serialize dynamic tool response for `{}`: {err}",
                    params.tool
                )
            })?;
            request_handle
                .resolve_server_request(request_id, result)
                .await
                .map_err(|err| {
                    format!(
                        "failed to resolve dynamic tool request for `{}`: {err}",
                        params.tool
                    )
                })
        }
        Err(DynamicToolExecutionError::UnknownTool { tool }) => {
            let message = format!("unknown dynamic tool `{tool}` for this TUI client");
            request_handle
                .reject_server_request(
                    request_id,
                    JSONRPCErrorError {
                        code: -32000,
                        message,
                        data: None,
                    },
                )
                .await
                .map_err(|err| format!("failed to reject dynamic tool request for `{tool}`: {err}"))
        }
        Err(DynamicToolExecutionError::ExecutionFailed { tool, message }) => {
            tracing::warn!(tool, %message, "dynamic tool executor failed");
            let result =
                serde_json::to_value(dynamic_tool_failure_response(&message)).map_err(|err| {
                    format!("failed to serialize fallback response for `{tool}`: {err}")
                })?;
            request_handle
                .resolve_server_request(request_id, result)
                .await
                .map_err(|err| {
                    format!("failed to resolve fallback dynamic tool response for `{tool}`: {err}")
                })
        }
    }
}

pub(crate) fn dynamic_tool_failure_response(message: &str) -> DynamicToolCallResponse {
    DynamicToolCallResponse {
        content_items: vec![DynamicToolCallOutputContentItem::InputText {
            text: message.to_string(),
        }],
        success: false,
    }
}

#[cfg(test)]
mod tests {
    use super::DynamicToolExecutionContext;
    use super::DynamicToolExecutionError;
    use super::DynamicToolRegistration;
    use super::DynamicToolRegistry;
    use codex_app_server_protocol::DynamicToolCallOutputContentItem;
    use codex_app_server_protocol::DynamicToolCallParams;
    use codex_app_server_protocol::DynamicToolCallResponse;
    use codex_app_server_protocol::DynamicToolSpec;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn demo_spec(name: &str) -> DynamicToolSpec {
        DynamicToolSpec {
            name: name.to_string(),
            description: format!("dynamic tool {name}"),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
            }),
            defer_loading: false,
        }
    }

    #[tokio::test]
    async fn dispatches_registered_dynamic_tools() {
        let registry = DynamicToolRegistry::from_registrations(vec![DynamicToolRegistration::new(
            demo_spec("demo_tool"),
            |_context, params| async move {
                Ok(DynamicToolCallResponse {
                    content_items: vec![DynamicToolCallOutputContentItem::InputText {
                        text: params.arguments.to_string(),
                    }],
                    success: true,
                })
            },
        )]);

        let response = registry
            .execute(
                DynamicToolExecutionContext::for_tests(),
                DynamicToolCallParams {
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    call_id: "call-1".to_string(),
                    tool: "demo_tool".to_string(),
                    arguments: json!({ "city": "Paris" }),
                },
            )
            .await
            .expect("dynamic tool should execute");

        assert_eq!(
            response,
            DynamicToolCallResponse {
                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                    text: json!({ "city": "Paris" }).to_string(),
                }],
                success: true,
            }
        );
    }

    #[tokio::test]
    async fn rejects_unknown_dynamic_tools() {
        let registry = DynamicToolRegistry::default();

        let error = registry
            .execute(
                DynamicToolExecutionContext::for_tests(),
                DynamicToolCallParams {
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    call_id: "call-1".to_string(),
                    tool: "missing_tool".to_string(),
                    arguments: json!({}),
                },
            )
            .await
            .expect_err("unknown tool should be rejected");

        assert_eq!(
            error,
            DynamicToolExecutionError::UnknownTool {
                tool: "missing_tool".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn surfaces_executor_failures() {
        let registry = DynamicToolRegistry::from_registrations(vec![DynamicToolRegistration::new(
            demo_spec("demo_tool"),
            |_context, _params| async move {
                Err(DynamicToolExecutionError::failed(
                    "demo_tool",
                    "dynamic tool failed",
                ))
            },
        )]);

        let error = registry
            .execute(
                DynamicToolExecutionContext::for_tests(),
                DynamicToolCallParams {
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    call_id: "call-1".to_string(),
                    tool: "demo_tool".to_string(),
                    arguments: json!({}),
                },
            )
            .await
            .expect_err("executor failure should surface");

        assert_eq!(
            error,
            DynamicToolExecutionError::ExecutionFailed {
                tool: "demo_tool".to_string(),
                message: "dynamic tool failed".to_string(),
            }
        );
    }
}
