use std::sync::Arc;

use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_READ_METHOD;
use crate::protocol::EXEC_TERMINATE_METHOD;
use crate::protocol::EXEC_WRITE_METHOD;
use crate::protocol::ExecParams;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::protocol::ReadParams;
use crate::protocol::TerminateParams;
use crate::protocol::WriteParams;
use crate::rpc::RpcRouter;
use crate::server::ExecServerHandler;

pub(crate) fn build_router() -> RpcRouter<ExecServerHandler> {
    let mut router = RpcRouter::new();
    router.request(
        INITIALIZE_METHOD,
        |handler: Arc<ExecServerHandler>, _params: InitializeParams| async move {
            handler.initialize()
        },
    );
    router.notification(
        INITIALIZED_METHOD,
        |handler: Arc<ExecServerHandler>, (): ()| async move { handler.initialized() },
    );
    router.request(
        EXEC_METHOD,
        |handler: Arc<ExecServerHandler>, params: ExecParams| async move { handler.exec(params).await },
    );
    router.request(
        EXEC_READ_METHOD,
        |handler: Arc<ExecServerHandler>, params: ReadParams| async move {
            handler.exec_read(params).await
        },
    );
    router.request(
        EXEC_WRITE_METHOD,
        |handler: Arc<ExecServerHandler>, params: WriteParams| async move {
            handler.exec_write(params).await
        },
    );
    router.request(
        EXEC_TERMINATE_METHOD,
        |handler: Arc<ExecServerHandler>, params: TerminateParams| async move {
            handler.terminate(params).await
        },
    );
    router
}
