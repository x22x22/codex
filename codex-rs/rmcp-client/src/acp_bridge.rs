use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::RwLock;

use anyhow::Result;
use futures::future::BoxFuture;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::model::ServerJsonRpcMessage;

pub trait AcpConnection: Send + Sync + 'static {
    fn send(&self, message: ClientJsonRpcMessage) -> BoxFuture<'static, Result<()>>;
    fn recv(&self) -> BoxFuture<'static, Result<Option<ServerJsonRpcMessage>>>;
    fn close(&self) -> BoxFuture<'static, Result<()>>;
}

pub trait AcpBridge: Send + Sync + 'static {
    fn connect(&self, acp_id: String) -> BoxFuture<'static, Result<Arc<dyn AcpConnection>>>;
}

static ACP_BRIDGE: LazyLock<RwLock<Option<Arc<dyn AcpBridge>>>> =
    LazyLock::new(|| RwLock::new(None));

pub fn set_acp_bridge(bridge: Option<Arc<dyn AcpBridge>>) {
    let mut slot = ACP_BRIDGE
        .write()
        .unwrap_or_else(|_| panic!("ACP bridge registry poisoned"));
    *slot = bridge;
}

pub(crate) fn get_acp_bridge() -> Result<Arc<dyn AcpBridge>> {
    ACP_BRIDGE
        .read()
        .unwrap_or_else(|_| panic!("ACP bridge registry poisoned"))
        .clone()
        .ok_or_else(|| anyhow::anyhow!("ACP MCP bridge is not installed"))
}
