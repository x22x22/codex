use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

use crate::connection::CHANNEL_CAPACITY;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
use crate::protocol::EXEC_EXITED_METHOD;
use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::EXEC_READ_METHOD;
use crate::protocol::EXEC_TERMINATE_METHOD;
use crate::protocol::EXEC_WRITE_METHOD;
use crate::protocol::FS_COPY_METHOD;
use crate::protocol::FS_CREATE_DIRECTORY_METHOD;
use crate::protocol::FS_GET_METADATA_METHOD;
use crate::protocol::FS_READ_DIRECTORY_METHOD;
use crate::protocol::FS_READ_FILE_METHOD;
use crate::protocol::FS_REMOVE_METHOD;
use crate::protocol::FS_WRITE_FILE_METHOD;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::server::ExecServerConfig;
use crate::server::ExecServerHandler;
use crate::server::ExecServerServerNotification;
use crate::server::internal_error;
use crate::server::invalid_params;
use crate::server::invalid_request;

pub(crate) async fn run_connection(connection: JsonRpcConnection, config: ExecServerConfig) {
    let (json_outgoing_tx, mut incoming_rx, _connection_tasks) = connection.into_parts();
    let json_outgoing_tx_for_notifications = json_outgoing_tx.clone();
    let (notification_tx, mut notification_rx) =
        mpsc::channel::<ExecServerServerNotification>(CHANNEL_CAPACITY);
    let mut handler = ExecServerHandler::new(notification_tx, config.auth_token);

    let outbound_task = tokio::spawn(async move {
        while let Some(notification) = notification_rx.recv().await {
            let json_message = match notification_message(notification) {
                Ok(json_message) => json_message,
                Err(err) => {
                    warn!("failed to serialize exec-server notification: {err}");
                    break;
                }
            };
            if json_outgoing_tx_for_notifications
                .send(json_message)
                .await
                .is_err()
            {
                break;
            }
        }
    });

    while let Some(event) = incoming_rx.recv().await {
        match event {
            JsonRpcConnectionEvent::Message(message) => {
                let maybe_response = match handle_connection_message(&mut handler, message).await {
                    Ok(maybe_response) => maybe_response,
                    Err(err) => {
                        warn!("closing exec-server connection after protocol error: {err}");
                        break;
                    }
                };
                if let Some(response) = maybe_response
                    && json_outgoing_tx.send(response).await.is_err()
                {
                    break;
                }
            }
            JsonRpcConnectionEvent::Disconnected { reason } => {
                if let Some(reason) = reason {
                    debug!("exec-server connection disconnected: {reason}");
                }
                break;
            }
        }
    }

    handler.shutdown().await;
    drop(handler);
    let _ = outbound_task.await;
}

async fn handle_connection_message(
    handler: &mut ExecServerHandler,
    message: JSONRPCMessage,
) -> Result<Option<JSONRPCMessage>, String> {
    match message {
        JSONRPCMessage::Request(request) => Ok(Some(dispatch_request(handler, request).await)),
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

async fn dispatch_request(
    handler: &mut ExecServerHandler,
    request: JSONRPCRequest,
) -> JSONRPCMessage {
    let JSONRPCRequest {
        id,
        method,
        params,
        trace: _,
    } = request;
    let params = params.unwrap_or(serde_json::Value::Null);

    match method.as_str() {
        INITIALIZE_METHOD => request_response(
            id,
            parse_params(params).and_then(|params| handler.initialize(params)),
        ),
        EXEC_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.exec(params)).await,
        ),
        EXEC_READ_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.read(params)).await,
        ),
        EXEC_WRITE_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.write(params)).await,
        ),
        EXEC_TERMINATE_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.terminate(params)).await,
        ),
        FS_READ_FILE_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_read_file(params)).await,
        ),
        FS_WRITE_FILE_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_write_file(params)).await,
        ),
        FS_CREATE_DIRECTORY_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_create_directory(params)).await,
        ),
        FS_GET_METADATA_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_get_metadata(params)).await,
        ),
        FS_READ_DIRECTORY_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_read_directory(params)).await,
        ),
        FS_REMOVE_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_remove(params)).await,
        ),
        FS_COPY_METHOD => request_response(
            id,
            dispatch_async_request(params, |params| handler.fs_copy(params)).await,
        ),
        other => jsonrpc_error_response(id, invalid_request(format!("unknown method: {other}"))),
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

fn parse_params<P>(params: serde_json::Value) -> Result<P, JSONRPCErrorError>
where
    P: DeserializeOwned,
{
    serde_json::from_value(params).map_err(|err| invalid_params(err.to_string()))
}

async fn dispatch_async_request<P, T, F, Fut>(
    params: serde_json::Value,
    f: F,
) -> Result<T, JSONRPCErrorError>
where
    P: DeserializeOwned,
    F: FnOnce(P) -> Fut,
    Fut: std::future::Future<Output = Result<T, JSONRPCErrorError>>,
{
    match parse_params(params) {
        Ok(params) => f(params).await,
        Err(err) => Err(err),
    }
}

fn request_response<T>(
    request_id: RequestId,
    result: Result<T, JSONRPCErrorError>,
) -> JSONRPCMessage
where
    T: Serialize,
{
    match result.and_then(serialize_response) {
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

fn serialize_response<T>(response: T) -> Result<serde_json::Value, JSONRPCErrorError>
where
    T: Serialize,
{
    serde_json::to_value(response).map_err(|err| internal_error(err.to_string()))
}

fn jsonrpc_error_response(request_id: RequestId, error: JSONRPCErrorError) -> JSONRPCMessage {
    JSONRPCMessage::Error(JSONRPCError {
        id: request_id,
        error,
    })
}

fn notification_message(
    notification: ExecServerServerNotification,
) -> Result<JSONRPCMessage, serde_json::Error> {
    match notification {
        ExecServerServerNotification::OutputDelta(params) => {
            typed_notification(EXEC_OUTPUT_DELTA_METHOD, params)
        }
        ExecServerServerNotification::Exited(params) => {
            typed_notification(EXEC_EXITED_METHOD, params)
        }
    }
}

fn typed_notification<T>(method: &str, params: T) -> Result<JSONRPCMessage, serde_json::Error>
where
    T: Serialize,
{
    Ok(JSONRPCMessage::Notification(JSONRPCNotification {
        method: method.to_string(),
        params: Some(serde_json::to_value(params)?),
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pretty_assertions::assert_eq;

    use super::dispatch_request;
    use super::handle_connection_message;
    use super::notification_message;
    use crate::protocol::EXEC_METHOD;
    use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
    use crate::protocol::ExecOutputDeltaNotification;
    use crate::protocol::ExecOutputStream;
    use crate::protocol::INITIALIZE_METHOD;
    use crate::protocol::INITIALIZED_METHOD;
    use crate::protocol::InitializeParams;
    use crate::protocol::PROTOCOL_VERSION;
    use crate::server::ExecServerHandler;
    use crate::server::ExecServerServerNotification;
    use codex_app_server_protocol::JSONRPCError;
    use codex_app_server_protocol::JSONRPCMessage;
    use codex_app_server_protocol::JSONRPCNotification;
    use codex_app_server_protocol::JSONRPCRequest;
    use codex_app_server_protocol::JSONRPCResponse;
    use codex_app_server_protocol::RequestId;

    #[tokio::test]
    async fn dispatch_initialize_returns_initialize_response() {
        let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
        let mut handler = ExecServerHandler::new(notification_tx, None);

        let message = dispatch_request(
            &mut handler,
            JSONRPCRequest {
                id: RequestId::Integer(1),
                method: INITIALIZE_METHOD.to_string(),
                params: Some(
                    serde_json::to_value(InitializeParams {
                        client_name: "test".to_string(),
                        auth_token: None,
                    })
                    .expect("serialize initialize params"),
                ),
                trace: None,
            },
        )
        .await;

        assert_eq!(
            message,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: RequestId::Integer(1),
                result: serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                }),
            })
        );
    }

    #[tokio::test]
    async fn dispatch_exec_returns_invalid_request_before_initialize() {
        let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
        let mut handler = ExecServerHandler::new(notification_tx, None);

        let message = dispatch_request(
            &mut handler,
            JSONRPCRequest {
                id: RequestId::Integer(7),
                method: EXEC_METHOD.to_string(),
                params: Some(serde_json::json!({
                    "processId": "proc-1",
                    "argv": ["bash", "-lc", "true"],
                    "cwd": std::env::current_dir().expect("cwd"),
                    "env": HashMap::<String, String>::new(),
                    "tty": true,
                    "arg0": null,
                    "sandbox": null,
                })),
                trace: None,
            },
        )
        .await;

        let JSONRPCMessage::Error(JSONRPCError { id, error }) = message else {
            panic!("expected invalid-request error");
        };
        assert_eq!(id, RequestId::Integer(7));
        assert_eq!(error.code, -32600);
        assert_eq!(
            error.message,
            "client must call initialize before using exec methods"
        );
    }

    #[tokio::test]
    async fn initialized_notification_before_initialize_is_protocol_error() {
        let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
        let mut handler = ExecServerHandler::new(notification_tx, None);

        let err = handle_connection_message(
            &mut handler,
            JSONRPCMessage::Notification(JSONRPCNotification {
                method: INITIALIZED_METHOD.to_string(),
                params: Some(serde_json::json!({})),
            }),
        )
        .await
        .expect_err("expected early initialized to fail");

        assert_eq!(
            err,
            "received `initialized` notification before `initialize`"
        );
    }

    #[test]
    fn notification_message_serializes_process_output() {
        let message = notification_message(ExecServerServerNotification::OutputDelta(
            ExecOutputDeltaNotification {
                process_id: "proc-1".to_string(),
                stream: ExecOutputStream::Stdout,
                chunk: b"hello".to_vec().into(),
            },
        ))
        .expect("serialize notification");

        assert_eq!(
            message,
            JSONRPCMessage::Notification(JSONRPCNotification {
                method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                params: Some(serde_json::json!({
                    "processId": "proc-1",
                    "stream": "stdout",
                    "chunk": "aGVsbG8=",
                })),
            })
        );
    }
}
