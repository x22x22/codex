use codex_api::AnyTransport;
use codex_api::ReqwestTransport;

use crate::default_client::build_reqwest_client;
use crate::openai_socket::openai_unix_socket_path;

#[cfg(unix)]
use codex_api::UdsTransport;

pub fn build_api_transport() -> AnyTransport {
    if let Some(socket_path) = openai_unix_socket_path() {
        #[cfg(unix)]
        {
            return AnyTransport::Uds(UdsTransport::new(socket_path));
        }
        #[cfg(not(unix))]
        {
            tracing::warn!(
                "Ignoring CODEX_OPENAI_UNIX_SOCKET because unix sockets are unsupported"
            );
        }
    }

    AnyTransport::Reqwest(ReqwestTransport::new(build_reqwest_client()))
}
