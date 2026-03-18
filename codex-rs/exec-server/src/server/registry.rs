use std::sync::Arc;

use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::rpc::RpcRouter;
use crate::server::ExecServerHandler;

pub(crate) fn build_router() -> RpcRouter<ExecServerHandler> {
    let mut router = RpcRouter::new();
    router.request(
        INITIALIZE_METHOD,
        |handler: Arc<ExecServerHandler>, _params: InitializeParams| async move { handler.initialize() },
    );
    router.notification(
        INITIALIZED_METHOD,
        |handler: Arc<ExecServerHandler>, (): ()| async move { handler.initialized() },
    );
    router
}
