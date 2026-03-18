use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;
use crate::rpc::RpcRouter;
use crate::rpc::RpcServerOutboundMessage;
use crate::rpc::encode_server_message;

pub async fn run_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = stdin.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let message = serde_json::from_str::<JSONRPCMessage>(&line)?;
        let mut router = RpcRouter::new();
        router.raw_request(INITIALIZE_METHOD, handle_request);
        router.notification(INITIALIZED_METHOD, |_| Ok(None));

        match router.route_message(message, unknown_request) {
            Ok(Some(outbound)) => {
                send_message(&mut stdout, outbound).await?;
            }
            Ok(None) => {}
            Err(message) => {
                send_error(
                    &mut stdout,
                    RequestId::Integer(-1),
                    invalid_request(message),
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn handle_request(request: JSONRPCRequest) -> Option<RpcServerOutboundMessage> {
    let result = match serde_json::to_value(InitializeResponse {
        protocol_version: PROTOCOL_VERSION.to_string(),
    }) {
        Ok(result) => result,
        Err(err) => {
            return Some(RpcServerOutboundMessage::Error {
                request_id: request.id,
                error: internal_error(err.to_string()),
            });
        }
    };
    Some(RpcServerOutboundMessage::Response {
        request_id: request.id,
        result,
    })
}

fn unknown_request(request: JSONRPCRequest) -> Option<RpcServerOutboundMessage> {
    Some(RpcServerOutboundMessage::Error {
        request_id: request.id,
        error: method_not_implemented(format!(
            "exec-server stub does not implement `{}` yet",
            request.method
        )),
    })
}

async fn send_error(
    stdout: &mut tokio::io::Stdout,
    id: RequestId,
    error: JSONRPCErrorError,
) -> Result<(), std::io::Error> {
    send_message(
        stdout,
        RpcServerOutboundMessage::Error {
            request_id: id,
            error,
        },
    )
    .await
}

async fn send_message(
    stdout: &mut tokio::io::Stdout,
    message: RpcServerOutboundMessage,
) -> Result<(), std::io::Error> {
    let message = encode_server_message(message).map_err(std::io::Error::other)?;
    let encoded = serde_json::to_vec(&message).map_err(std::io::Error::other)?;
    stdout.write_all(&encoded).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await
}

fn invalid_request(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32600,
        message,
        data: None,
    }
}

fn method_not_implemented(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32601,
        message,
        data: None,
    }
}

fn internal_error(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        message,
        data: None,
    }
}
