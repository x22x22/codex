use super::*;
use futures::future::BoxFuture;
use pretty_assertions::assert_eq;

struct TestHandler;

impl ToolHandler for TestHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(
        &self,
        _invocation: ToolInvocation,
    ) -> BoxFuture<'_, Result<AnyToolResult, FunctionCallError>> {
        Box::pin(async move { unreachable!("test handler should not be invoked") })
    }
}

#[test]
fn handler_looks_up_namespaced_aliases_explicitly() {
    let plain_handler = Arc::new(TestHandler) as Arc<dyn ToolHandler>;
    let namespaced_handler = Arc::new(TestHandler) as Arc<dyn ToolHandler>;
    let namespace = "mcp__codex_apps__gmail";
    let tool_name = "gmail_get_recent_emails";
    let namespaced_name = tool_handler_key(tool_name, Some(namespace));
    let registry = ToolRegistry::new(HashMap::from([
        (tool_name.to_string(), Arc::clone(&plain_handler)),
        (namespaced_name, Arc::clone(&namespaced_handler)),
    ]));

    let plain = registry.handler(tool_name, /*namespace*/ None);
    let namespaced = registry.handler(tool_name, Some(namespace));
    let missing_namespaced = registry.handler(tool_name, Some("mcp__codex_apps__calendar"));

    assert_eq!(plain.is_some(), true);
    assert_eq!(namespaced.is_some(), true);
    assert_eq!(missing_namespaced.is_none(), true);
    assert!(
        plain
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &plain_handler))
    );
    assert!(
        namespaced
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &namespaced_handler))
    );
}
