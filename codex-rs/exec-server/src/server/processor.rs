use std::sync::Arc;

use codex_app_server_protocol::JSONRPCMessage;
use jsonrpsee::RpcModule;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

use crate::connection::CHANNEL_CAPACITY;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_READ_METHOD;
use crate::protocol::EXEC_TERMINATE_METHOD;
use crate::protocol::EXEC_WRITE_METHOD;
use crate::protocol::ExecParams;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::ReadParams;
use crate::protocol::TerminateParams;
use crate::protocol::WriteParams;
use crate::server::handler::ExecServerHandler;
use crate::server::routing::ExecServerOutboundMessage;
use crate::server::routing::encode_outbound_message;

const MAX_JSON_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn run_connection(connection: JsonRpcConnection) {
    let (json_outgoing_tx, mut incoming_rx) = connection.into_parts();
    let (outgoing_tx, mut outgoing_rx) =
        mpsc::channel::<ExecServerOutboundMessage>(CHANNEL_CAPACITY);
    let handler = Arc::new(Mutex::new(ExecServerHandler::new(outgoing_tx.clone())));
    let rpc_module = match build_rpc_module(Arc::clone(&handler)) {
        Ok(rpc_module) => rpc_module,
        Err(err) => {
            warn!("failed to build exec-server RPC module: {err}");
            return;
        }
    };

    let notification_outgoing_tx = json_outgoing_tx.clone();
    let outbound_task = tokio::spawn(async move {
        while let Some(message) = outgoing_rx.recv().await {
            let json_message = match encode_outbound_message(message) {
                Ok(json_message) => json_message,
                Err(err) => {
                    warn!("failed to serialize exec-server outbound notification: {err}");
                    break;
                }
            };
            if notification_outgoing_tx.send(json_message).await.is_err() {
                break;
            }
        }
    });

    while let Some(event) = incoming_rx.recv().await {
        match event {
            JsonRpcConnectionEvent::Message(message) => {
                if let Err(err) =
                    handle_incoming_message(&handler, &rpc_module, message, &json_outgoing_tx).await
                {
                    warn!("closing exec-server connection after protocol error: {err}");
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

    handler.lock().await.shutdown().await;
    drop(outgoing_tx);
    let _ = outbound_task.await;
}

fn build_rpc_module(
    handler: Arc<Mutex<ExecServerHandler>>,
) -> Result<RpcModule<()>, Box<dyn std::error::Error + Send + Sync>> {
    let mut rpc = RpcModule::new(());

    let initialize_handler = Arc::clone(&handler);
    rpc.register_async_method(INITIALIZE_METHOD, move |_, _, _| {
        let initialize_handler = Arc::clone(&initialize_handler);
        async move { handler_result_to_rpc(initialize_handler.lock().await.initialize()) }
    })?;

    let initialized_handler = Arc::clone(&handler);
    rpc.register_async_method(INITIALIZED_METHOD, move |_, _, _| {
        let initialized_handler = Arc::clone(&initialized_handler);
        async move {
            initialized_handler
                .lock()
                .await
                .initialized()
                .map_err(|message| ErrorObjectOwned::owned(-32600, message, None::<()>))?;
            Ok::<(), ErrorObjectOwned>(())
        }
    })?;

    let exec_handler = Arc::clone(&handler);
    rpc.register_async_method(EXEC_METHOD, move |params, _, _| {
        let exec_handler = Arc::clone(&exec_handler);
        async move {
            let params = params
                .parse::<ExecParams>()
                .map_err(|err| ErrorObjectOwned::owned(-32602, err.to_string(), None::<()>))?;
            handler_result_to_rpc(exec_handler.lock().await.exec(params).await)
        }
    })?;

    let read_handler = Arc::clone(&handler);
    rpc.register_async_method(EXEC_READ_METHOD, move |params, _, _| {
        let read_handler = Arc::clone(&read_handler);
        async move {
            let params = params
                .parse::<ReadParams>()
                .map_err(|err| ErrorObjectOwned::owned(-32602, err.to_string(), None::<()>))?;
            handler_result_to_rpc(read_handler.lock().await.read(params).await)
        }
    })?;

    let write_handler = Arc::clone(&handler);
    rpc.register_async_method(EXEC_WRITE_METHOD, move |params, _, _| {
        let write_handler = Arc::clone(&write_handler);
        async move {
            let params = params
                .parse::<WriteParams>()
                .map_err(|err| ErrorObjectOwned::owned(-32602, err.to_string(), None::<()>))?;
            handler_result_to_rpc(write_handler.lock().await.write(params).await)
        }
    })?;

    let terminate_handler = Arc::clone(&handler);
    rpc.register_async_method(EXEC_TERMINATE_METHOD, move |params, _, _| {
        let terminate_handler = Arc::clone(&terminate_handler);
        async move {
            let params = params
                .parse::<TerminateParams>()
                .map_err(|err| ErrorObjectOwned::owned(-32602, err.to_string(), None::<()>))?;
            handler_result_to_rpc(terminate_handler.lock().await.terminate(params).await)
        }
    })?;

    Ok(rpc)
}

fn handler_result_to_rpc<T>(
    result: Result<T, codex_app_server_protocol::JSONRPCErrorError>,
) -> RpcResult<T> {
    result.map_err(|error| {
        let code = i32::try_from(error.code).unwrap_or(-32603);
        ErrorObjectOwned::owned(code, error.message, error.data)
    })
}

async fn handle_incoming_message(
    handler: &Arc<Mutex<ExecServerHandler>>,
    rpc_module: &RpcModule<()>,
    message: JSONRPCMessage,
    json_outgoing_tx: &mpsc::Sender<JSONRPCMessage>,
) -> Result<(), String> {
    if let JSONRPCMessage::Notification(notification) = &message
        && notification.method == INITIALIZED_METHOD
    {
        return handler.lock().await.initialized();
    }

    let mut raw_request = serde_json::to_string(&message)
        .map_err(|err| format!("failed to encode request: {err}"))?;
    raw_request = inject_jsonrpc_version(&raw_request)?;

    let response = rpc_module
        .raw_json_request(&raw_request, MAX_JSON_RESPONSE_BYTES)
        .await
        .map_err(|err| err.to_string())?;
    let (response, _subscription_rx) = response;
    if response.get() == "null" {
        return Ok(());
    }

    let mut response: JSONRPCMessage =
        serde_json::from_str(response.get()).map_err(|err| err.to_string())?;
    strip_jsonrpc_version(&mut response);
    json_outgoing_tx
        .send(response)
        .await
        .map_err(|_| "JSON-RPC connection closed".to_string())
}

fn inject_jsonrpc_version(raw_request: &str) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(raw_request).map_err(|err| err.to_string())?;
    let Some(object) = value.as_object_mut() else {
        return Err("expected JSON-RPC object".to_string());
    };
    object
        .entry("jsonrpc".to_string())
        .or_insert_with(|| serde_json::Value::String("2.0".to_string()));
    serde_json::to_string(&value).map_err(|err| err.to_string())
}

fn strip_jsonrpc_version(message: &mut JSONRPCMessage) {
    let encoded = match serde_json::to_value(&*message) {
        Ok(encoded) => encoded,
        Err(_) => return,
    };
    let mut object = match encoded {
        serde_json::Value::Object(object) => object,
        _ => return,
    };
    object.remove("jsonrpc");
    if let Ok(decoded) = serde_json::from_value::<JSONRPCMessage>(serde_json::Value::Object(object))
    {
        *message = decoded;
    }
}
