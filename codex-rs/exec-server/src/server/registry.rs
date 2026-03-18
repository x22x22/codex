use std::sync::Arc;

use tokio::sync::Mutex;

use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::rpc::RpcRouter;
use crate::server::ExecServerHandler;

pub(crate) fn build_router() -> RpcRouter<Mutex<ExecServerHandler>> {
    let mut router = RpcRouter::new();
    router.request(
        INITIALIZE_METHOD,
        |handler: Arc<Mutex<ExecServerHandler>>, _params: InitializeParams| async move {
            handler.lock().await.initialize()
        },
    );
    router.notification(
        INITIALIZED_METHOD,
        |handler: Arc<Mutex<ExecServerHandler>>, (): ()| async move {
            handler.lock().await.initialized()
        },
    );
    router
}
