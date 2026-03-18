use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use serde::Serialize;
use tokio::sync::mpsc;

use super::ExecServerError;

pub(super) struct JsonRpcBackend {
    write_tx: mpsc::Sender<JSONRPCMessage>,
}

impl JsonRpcBackend {
    pub(super) fn new(write_tx: mpsc::Sender<JSONRPCMessage>) -> Self {
        Self { write_tx }
    }

    pub(super) async fn notify<P: Serialize>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<(), ExecServerError> {
        let params = serde_json::to_value(params)?;
        self.write_tx
            .send(JSONRPCMessage::Notification(JSONRPCNotification {
                method: method.to_string(),
                params: Some(params),
            }))
            .await
            .map_err(|_| ExecServerError::Closed)
    }

    pub(super) async fn send_request<P: Serialize>(
        &self,
        request_id: RequestId,
        method: &str,
        params: &P,
    ) -> Result<(), ExecServerError> {
        let params = serde_json::to_value(params)?;
        self.write_tx
            .send(JSONRPCMessage::Request(JSONRPCRequest {
                id: request_id,
                method: method.to_string(),
                params: Some(params),
                trace: None,
            }))
            .await
            .map_err(|_| ExecServerError::Closed)
    }
}
