mod handler;
mod processor;
mod routing;
mod transport;

pub(crate) use handler::ExecServerHandler;
pub(crate) use routing::ExecServerClientNotification;
pub(crate) use routing::ExecServerInboundMessage;
pub(crate) use routing::ExecServerOutboundMessage;
pub(crate) use routing::ExecServerRequest;
pub(crate) use routing::ExecServerResponseMessage;
pub(crate) use routing::ExecServerServerNotification;
pub use transport::ExecServerTransport;
pub use transport::ExecServerTransportParseError;

pub async fn run_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_main_with_transport(ExecServerTransport::Stdio).await
}

pub async fn run_main_with_transport(
    transport: ExecServerTransport,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    transport::run_transport(transport).await
}
