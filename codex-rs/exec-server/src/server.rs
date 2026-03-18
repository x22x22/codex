mod filesystem;
mod handler;
mod jsonrpc;
mod processor;
mod transport;

pub(crate) use handler::ExecServerHandler;
pub(crate) use handler::ExecServerServerNotification;
pub(crate) use jsonrpc::internal_error;
pub(crate) use jsonrpc::invalid_params;
pub(crate) use jsonrpc::invalid_request;
pub(crate) use jsonrpc::unauthorized;
pub use transport::ExecServerTransport;
pub use transport::ExecServerTransportParseError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecServerConfig {
    pub auth_token: Option<String>,
}

pub async fn run_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_main_with_transport_and_config(ExecServerTransport::default(), ExecServerConfig::default())
        .await
}

pub async fn run_main_with_transport(
    transport: ExecServerTransport,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_main_with_transport_and_config(transport, ExecServerConfig::default()).await
}

pub async fn run_main_with_transport_and_config(
    transport: ExecServerTransport,
    config: ExecServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    transport::run_transport(transport, config).await
}
