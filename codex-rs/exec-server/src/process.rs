use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::ExecServerError;
use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecParams;
use crate::protocol::ExecResizeParams;
use crate::protocol::ExecResizeResponse;
use crate::protocol::ExecResponse;
use crate::protocol::ExecWaitParams;
use crate::protocol::ExecWaitResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecServerEvent {
    OutputDelta(ExecOutputDeltaNotification),
    Exited(ExecExitedNotification),
}

#[async_trait]
pub trait ExecProcess: Send + Sync {
    async fn start(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError>;

    async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError>;

    async fn write(&self, params: WriteParams) -> Result<WriteResponse, ExecServerError>;

    async fn resize(&self, params: ExecResizeParams)
    -> Result<ExecResizeResponse, ExecServerError>;

    async fn wait(&self, params: ExecWaitParams) -> Result<ExecWaitResponse, ExecServerError>;

    async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError>;

    fn subscribe_events(&self) -> broadcast::Receiver<ExecServerEvent>;
}
