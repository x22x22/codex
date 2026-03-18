use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;

use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::server::ExecServerHandler;
use crate::server::invalid_request;

pub(crate) async fn handle_connection_message(
    handler: &mut ExecServerHandler,
    message: JSONRPCMessage,
) -> Result<Option<JSONRPCMessage>, String> {
    match message {
        JSONRPCMessage::Request(request) => Ok(Some(dispatch_request(handler, request))),
        JSONRPCMessage::Notification(notification) => {
            handle_notification(handler, notification)?;
            Ok(None)
        }
        JSONRPCMessage::Response(response) => Err(format!(
            "unexpected client response for request id {:?}",
            response.id
        )),
        JSONRPCMessage::Error(error) => Err(format!(
            "unexpected client error for request id {:?}",
            error.id
        )),
    }
}

fn dispatch_request(handler: &mut ExecServerHandler, request: JSONRPCRequest) -> JSONRPCMessage {
    let JSONRPCRequest {
        id,
        method,
        params,
        trace: _,
    } = request;

    match method.as_str() {
        INITIALIZE_METHOD => {
            let result = serde_json::from_value::<InitializeParams>(
                params.unwrap_or(serde_json::Value::Null),
            )
            .map_err(|err| codex_app_server_protocol::JSONRPCErrorError {
                code: -32602,
                data: None,
                message: err.to_string(),
            })
            .and_then(|params| handler.initialize(params))
            .and_then(|response| {
                serde_json::to_value(response).map_err(|err| {
                    codex_app_server_protocol::JSONRPCErrorError {
                        code: -32603,
                        data: None,
                        message: err.to_string(),
                    }
                })
            });
            response_message(id, result)
        }
        other => response_message(id, Err(invalid_request(format!("unknown method: {other}")))),
    }
}

fn handle_notification(
    handler: &mut ExecServerHandler,
    notification: JSONRPCNotification,
) -> Result<(), String> {
    match notification.method.as_str() {
        INITIALIZED_METHOD => handler.initialized(),
        other => Err(format!("unexpected notification method: {other}")),
    }
}

fn response_message(
    request_id: RequestId,
    result: Result<serde_json::Value, codex_app_server_protocol::JSONRPCErrorError>,
) -> JSONRPCMessage {
    match result {
        Ok(result) => JSONRPCMessage::Response(JSONRPCResponse {
            id: request_id,
            result,
        }),
        Err(error) => JSONRPCMessage::Error(JSONRPCError {
            id: request_id,
            error,
        }),
    }
}
