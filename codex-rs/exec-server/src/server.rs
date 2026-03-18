use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;

pub async fn run_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = stdin.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let message = serde_json::from_str::<JSONRPCMessage>(&line)?;
        match message {
            JSONRPCMessage::Request(request) => {
                handle_request(request, &mut stdout).await?;
            }
            JSONRPCMessage::Notification(notification) => {
                if notification.method != INITIALIZED_METHOD {
                    send_error(
                        &mut stdout,
                        RequestId::Integer(-1),
                        invalid_request(format!(
                            "unexpected notification method: {}",
                            notification.method
                        )),
                    )
                    .await?;
                }
            }
            JSONRPCMessage::Response(response) => {
                send_error(
                    &mut stdout,
                    response.id,
                    invalid_request("unexpected response from client".to_string()),
                )
                .await?;
            }
            JSONRPCMessage::Error(error) => {
                send_error(
                    &mut stdout,
                    error.id,
                    invalid_request("unexpected error from client".to_string()),
                )
                .await?;
            }
        }
    }

    Ok(())
}

async fn handle_request(
    request: JSONRPCRequest,
    stdout: &mut tokio::io::Stdout,
) -> Result<(), std::io::Error> {
    match request.method.as_str() {
        INITIALIZE_METHOD => {
            let result = serde_json::to_value(InitializeResponse {
                protocol_version: PROTOCOL_VERSION.to_string(),
            })
            .map_err(std::io::Error::other)?;

            send_response(
                stdout,
                JSONRPCResponse {
                    id: request.id,
                    result,
                },
            )
            .await
        }
        method => {
            send_error(
                stdout,
                request.id,
                method_not_implemented(format!(
                    "exec-server stub does not implement `{method}` yet"
                )),
            )
            .await
        }
    }
}

async fn send_response(
    stdout: &mut tokio::io::Stdout,
    response: JSONRPCResponse,
) -> Result<(), std::io::Error> {
    send_message(stdout, &JSONRPCMessage::Response(response)).await
}

async fn send_error(
    stdout: &mut tokio::io::Stdout,
    id: RequestId,
    error: JSONRPCErrorError,
) -> Result<(), std::io::Error> {
    send_message(stdout, &JSONRPCMessage::Error(JSONRPCError { id, error })).await
}

async fn send_message(
    stdout: &mut tokio::io::Stdout,
    message: &JSONRPCMessage,
) -> Result<(), std::io::Error> {
    let encoded = serde_json::to_vec(message).map_err(std::io::Error::other)?;
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
