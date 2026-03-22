use std::sync::Arc;

use async_trait::async_trait;
use codex_api::TransportError;
use codex_app_server_protocol::ResponseSendParams;
use codex_app_server_protocol::ResponseSendResponse;
use codex_app_server_protocol::ServerRequestPayload;
use codex_core::http_transport::AppServerResponsesBridge;
use codex_core::http_transport::ResponsesBridgeHttpResponse;
use codex_core::http_transport::install_app_server_responses_bridge;
use codex_core::openai_socket::should_use_app_server_responses_bridge;

use crate::outgoing_message::OutgoingMessageSender;

#[derive(Clone)]
pub(crate) struct ResponsesProxyBridge {
    outgoing: Arc<OutgoingMessageSender>,
}

impl ResponsesProxyBridge {
    pub(crate) fn maybe_install(
        outgoing: Arc<OutgoingMessageSender>,
    ) -> Option<codex_core::http_transport::AppServerResponsesBridgeGuard> {
        should_use_app_server_responses_bridge()
            .then(|| install_app_server_responses_bridge(Arc::new(Self { outgoing })))
    }
}

#[async_trait]
impl AppServerResponsesBridge for ResponsesProxyBridge {
    async fn send_responses_request(
        &self,
        request_body: String,
    ) -> Result<ResponsesBridgeHttpResponse, TransportError> {
        let (request_id, rx) = self
            .outgoing
            .send_request(ServerRequestPayload::ResponseSend(ResponseSendParams {
                request_body,
            }))
            .await;
        let result = rx.await.map_err(|err| {
            TransportError::Network(format!("responses bridge request canceled: {err}"))
        })?;
        let result = result.map_err(|err| {
            TransportError::Network(format!(
                "responses bridge request failed: code={} message={}",
                err.code, err.message
            ))
        })?;
        let response: ResponseSendResponse = serde_json::from_value(result)
            .map_err(|err| TransportError::Network(err.to_string()))?;
        tracing::debug!(
            request_id = ?request_id,
            status_code = response.status_code,
            "app-server responses bridge resolved"
        );
        Ok(ResponsesBridgeHttpResponse {
            status_code: response.status_code,
            body: response.body,
        })
    }
}
