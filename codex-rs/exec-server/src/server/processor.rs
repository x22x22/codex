use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

use crate::connection::CHANNEL_CAPACITY;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
use crate::server::handler::ExecServerHandler;
use crate::server::routing::ExecServerClientNotification;
use crate::server::routing::ExecServerInboundMessage;
use crate::server::routing::ExecServerOutboundMessage;
use crate::server::routing::ExecServerRequest;
use crate::server::routing::ExecServerResponseMessage;
use crate::server::routing::RoutedExecServerMessage;
use crate::server::routing::encode_outbound_message;
use crate::server::routing::route_jsonrpc_message;

pub(crate) async fn run_connection(connection: JsonRpcConnection) {
    let (json_outgoing_tx, mut incoming_rx, _connection_tasks) = connection.into_parts();
    let (outgoing_tx, mut outgoing_rx) =
        mpsc::channel::<ExecServerOutboundMessage>(CHANNEL_CAPACITY);
    let mut handler = ExecServerHandler::new(outgoing_tx.clone());

    let outbound_task = tokio::spawn(async move {
        while let Some(message) = outgoing_rx.recv().await {
            let json_message = match encode_outbound_message(message) {
                Ok(json_message) => json_message,
                Err(err) => {
                    warn!("failed to serialize exec-server outbound message: {err}");
                    break;
                }
            };
            if json_outgoing_tx.send(json_message).await.is_err() {
                break;
            }
        }
    });

    while let Some(event) = incoming_rx.recv().await {
        match event {
            JsonRpcConnectionEvent::Message(message) => match route_jsonrpc_message(message) {
                Ok(RoutedExecServerMessage::Inbound(message)) => {
                    if let Err(err) = dispatch_to_handler(&mut handler, message, &outgoing_tx).await
                    {
                        warn!("closing exec-server connection after protocol error: {err}");
                        break;
                    }
                }
                Ok(RoutedExecServerMessage::ImmediateOutbound(message)) => {
                    if outgoing_tx.send(message).await.is_err() {
                        break;
                    }
                }
                Err(err) => {
                    warn!("closing exec-server connection after protocol error: {err}");
                    break;
                }
            },
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
    drop(outgoing_tx);
    let _ = outbound_task.await;
}

async fn dispatch_to_handler(
    handler: &mut ExecServerHandler,
    message: ExecServerInboundMessage,
    outgoing_tx: &mpsc::Sender<ExecServerOutboundMessage>,
) -> Result<(), String> {
    match message {
        ExecServerInboundMessage::Request(request) => {
            let outbound = match request {
                ExecServerRequest::Initialize { request_id, .. } => request_outbound(
                    request_id,
                    handler
                        .initialize()
                        .map(ExecServerResponseMessage::Initialize),
                ),
                ExecServerRequest::Exec { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .exec(params)
                        .await
                        .map(ExecServerResponseMessage::Exec),
                ),
                ExecServerRequest::Read { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .read(params)
                        .await
                        .map(ExecServerResponseMessage::Read),
                ),
                ExecServerRequest::Write { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .write(params)
                        .await
                        .map(ExecServerResponseMessage::Write),
                ),
                ExecServerRequest::Terminate { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .terminate(params)
                        .await
                        .map(ExecServerResponseMessage::Terminate),
                ),
                ExecServerRequest::FsReadFile { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_read_file(params)
                        .await
                        .map(ExecServerResponseMessage::FsReadFile),
                ),
                ExecServerRequest::FsWriteFile { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_write_file(params)
                        .await
                        .map(ExecServerResponseMessage::FsWriteFile),
                ),
                ExecServerRequest::FsCreateDirectory { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_create_directory(params)
                        .await
                        .map(ExecServerResponseMessage::FsCreateDirectory),
                ),
                ExecServerRequest::FsGetMetadata { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_get_metadata(params)
                        .await
                        .map(ExecServerResponseMessage::FsGetMetadata),
                ),
                ExecServerRequest::FsReadDirectory { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_read_directory(params)
                        .await
                        .map(ExecServerResponseMessage::FsReadDirectory),
                ),
                ExecServerRequest::FsRemove { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_remove(params)
                        .await
                        .map(ExecServerResponseMessage::FsRemove),
                ),
                ExecServerRequest::FsCopy { request_id, params } => request_outbound(
                    request_id,
                    handler
                        .fs_copy(params)
                        .await
                        .map(ExecServerResponseMessage::FsCopy),
                ),
            };
            outgoing_tx
                .send(outbound)
                .await
                .map_err(|_| "outbound channel closed".to_string())
        }
        ExecServerInboundMessage::Notification(ExecServerClientNotification::Initialized) => {
            handler.initialized()
        }
    }
}

fn request_outbound(
    request_id: codex_app_server_protocol::RequestId,
    result: Result<ExecServerResponseMessage, codex_app_server_protocol::JSONRPCErrorError>,
) -> ExecServerOutboundMessage {
    match result {
        Ok(response) => ExecServerOutboundMessage::Response {
            request_id,
            response,
        },
        Err(error) => ExecServerOutboundMessage::Error { request_id, error },
    }
}
