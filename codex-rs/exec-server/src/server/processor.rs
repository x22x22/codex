use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

use crate::connection::CHANNEL_CAPACITY;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
use crate::server::handler::ExecServerHandler;
use crate::server::routing::ExecServerOutboundMessage;
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
                    if let Err(err) = handler.handle_message(message).await {
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
