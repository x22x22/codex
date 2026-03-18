mod handler;
mod jsonrpc;
mod processor;
mod transport;

pub(crate) use handler::ExecServerHandler;
pub(crate) use jsonrpc::invalid_request;
pub(crate) use jsonrpc::unauthorized;
pub use transport::ExecServerTransport;
pub use transport::ExecServerTransportParseError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecServerConfig {
    pub auth_token: Option<String>,
}
