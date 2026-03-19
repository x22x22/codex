use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::client::ExecServerError;
use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteResponse;

/// Connection options for any exec-server client transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerClientConnectOptions {
    pub client_name: String,
    pub initialize_timeout: Duration,
}

/// WebSocket connection arguments for a remote exec-server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecServerConnectArgs {
    pub websocket_url: String,
    pub client_name: String,
    pub connect_timeout: Duration,
    pub initialize_timeout: Duration,
}

/// Connection-level server events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecServerEvent {
    OutputDelta(ExecOutputDeltaNotification),
    Exited(ExecExitedNotification),
}

/// Process lifecycle capability for an execution environment.
#[async_trait]
pub trait ExecProcess: Send + Sync {
    async fn start(&self, params: ExecParams) -> Result<ExecResponse, ExecServerError>;

    async fn read(&self, params: ReadParams) -> Result<ReadResponse, ExecServerError>;

    async fn write(
        &self,
        process_id: &str,
        chunk: Vec<u8>,
    ) -> Result<WriteResponse, ExecServerError>;

    async fn terminate(&self, process_id: &str) -> Result<TerminateResponse, ExecServerError>;

    fn subscribe_events(&self) -> broadcast::Receiver<ExecServerEvent>;
}

/// Capability bundle exposed by an execution environment.
pub trait ExecutorEnvironment: Send + Sync {
    fn get_executor(&self) -> &(dyn ExecProcess + '_);
}
