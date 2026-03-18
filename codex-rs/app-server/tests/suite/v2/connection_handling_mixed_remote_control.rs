use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_initialize_request;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server_with_args;
use anyhow::Context;
use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use codex_core::auth::AuthCredentialsStoreMode;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::process::Child;
use tokio::process::Command;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

type BackendWebSocket = WebSocketStream<TcpStream>;

#[tokio::test]
async fn websocket_transport_with_remote_control_routes_connections_independently() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let remote_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("listener should bind")?;
    let remote_control_base_url = format!(
        "http://{}/backend-api",
        remote_listener
            .local_addr()
            .context("listener should have local addr")?
    );
    let codex_home = TempDir::new()?;
    create_config_toml_with_remote_control(
        codex_home.path(),
        &server.uri(),
        &remote_control_base_url,
        "never",
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("Access Token").account_id("account_id"),
        AuthCredentialsStoreMode::File,
    )?;

    let (mut process, bind_addr) =
        spawn_websocket_server_with_args(codex_home.path(), &["--with-remote-control"]).await?;

    let enroll_request = accept_http_request(&remote_listener).await?;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_mixed" })).await?;
    let (backend_request, mut backend_websocket) =
        accept_remote_control_backend_connection(&remote_listener).await?;
    assert_eq!(backend_request.path, "/backend-api/remote/control/server");

    let mut local_websocket = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut local_websocket, 11, "local_ws_client").await?;
    assert_eq!(
        read_response_for_id(&mut local_websocket, 11).await?.id,
        RequestId::Integer(11)
    );

    send_remote_request(
        &mut backend_websocket,
        "remote-client-1",
        "initialize",
        11,
        Some(serde_json::to_value(InitializeParams {
            client_info: ClientInfo {
                name: "remote_control_client".to_string(),
                title: Some("Remote Control Test Client".to_string()),
                version: "0.1.0".to_string(),
            },
            capabilities: None,
        })?),
    )
    .await?;
    let remote_initialize =
        read_remote_response_for_id(&mut backend_websocket, "remote-client-1", 11).await?;
    assert_eq!(remote_initialize["id"], json!(11));

    send_request(
        &mut local_websocket,
        "config/read",
        77,
        Some(json!({ "includeLayers": false })),
    )
    .await?;
    send_remote_request(
        &mut backend_websocket,
        "remote-client-1",
        "config/read",
        77,
        Some(json!({ "includeLayers": false })),
    )
    .await?;

    let local_response = read_response_for_id(&mut local_websocket, 77).await?;
    let remote_response =
        read_remote_response_for_id(&mut backend_websocket, "remote-client-1", 77).await?;

    assert_eq!(local_response.id, RequestId::Integer(77));
    assert!(local_response.result.get("config").is_some());
    assert_eq!(remote_response["id"], json!(77));
    assert!(remote_response["result"].get("config").is_some());

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn stdio_transport_with_remote_control_exits_when_stdio_closes() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let remote_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("listener should bind")?;
    let remote_control_base_url = format!(
        "http://{}/backend-api",
        remote_listener
            .local_addr()
            .context("listener should have local addr")?
    );
    let codex_home = TempDir::new()?;
    create_config_toml_with_remote_control(
        codex_home.path(),
        &server.uri(),
        &remote_control_base_url,
        "never",
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("Access Token").account_id("account_id"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut process = spawn_stdio_server_with_remote_control(codex_home.path()).await?;
    let enroll_request = accept_http_request(&remote_listener).await?;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_stdio" })).await?;
    let (_backend_request, mut backend_websocket) =
        accept_remote_control_backend_connection(&remote_listener).await?;

    drop(process.stdin.take());
    let exit_status = timeout(Duration::from_secs(10), process.wait())
        .await
        .context("timed out waiting for stdio app-server to exit")?
        .context("failed waiting for stdio app-server exit")?;
    assert!(exit_status.success());

    let close_frame = timeout(Duration::from_secs(5), backend_websocket.next())
        .await
        .context("timed out waiting for remote-control websocket to close")?;
    match close_frame {
        Some(Ok(WebSocketMessage::Close(_))) | Some(Err(_)) | None => {}
        Some(Ok(other)) => {
            anyhow::bail!("unexpected websocket frame while waiting for shutdown: {other:?}")
        }
    }
    Ok(())
}

struct CapturedHttpRequest {
    stream: TcpStream,
    request_line: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedWebSocketRequest {
    path: String,
}

async fn accept_http_request(listener: &TcpListener) -> Result<CapturedHttpRequest> {
    let (stream, _) = timeout(Duration::from_secs(10), listener.accept())
        .await
        .context("HTTP request should arrive in time")?
        .context("listener accept should succeed")?;
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .context("request line should read")?;
    let request_line = request_line.trim_end_matches("\r\n").to_string();

    let mut headers = std::collections::BTreeMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .context("header line should read")?;
        if line == "\r\n" {
            break;
        }
        let line = line.trim_end_matches("\r\n");
        let (name, value) = line
            .split_once(':')
            .context("header should contain colon")?;
        headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .await
        .context("request body should read")?;

    Ok(CapturedHttpRequest {
        stream: reader.into_inner(),
        request_line,
    })
}

async fn respond_with_json(mut stream: TcpStream, body: serde_json::Value) -> Result<()> {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .context("response should write")?;
    stream.flush().await.context("response should flush")?;
    Ok(())
}

async fn accept_remote_control_backend_connection(
    listener: &TcpListener,
) -> Result<(CapturedWebSocketRequest, BackendWebSocket)> {
    let (stream, _) = timeout(Duration::from_secs(10), listener.accept())
        .await
        .context("websocket request should arrive in time")?
        .context("listener accept should succeed")?;
    let captured_request = Arc::new(std::sync::Mutex::new(None::<CapturedWebSocketRequest>));
    let captured_request_for_callback = Arc::clone(&captured_request);
    let websocket = accept_hdr_async(
        stream,
        move |request: &tungstenite::handshake::server::Request,
              response: tungstenite::handshake::server::Response| {
            let mut guard = match captured_request_for_callback.lock() {
                Ok(guard) => guard,
                Err(err) => panic!("capture lock should acquire: {err}"),
            };
            *guard = Some(CapturedWebSocketRequest {
                path: request.uri().path().to_string(),
            });
            Ok(response)
        },
    )
    .await
    .context("websocket handshake should succeed")?;
    let captured_request = match captured_request.lock() {
        Ok(guard) => guard.clone(),
        Err(err) => panic!("capture lock should acquire: {err}"),
    }
    .context("websocket request should be captured")?;
    Ok((captured_request, websocket))
}

async fn send_remote_request(
    websocket: &mut BackendWebSocket,
    client_id: &str,
    method: &str,
    id: i64,
    params: Option<Value>,
) -> Result<()> {
    let message = serde_json::to_value(JSONRPCMessage::Request(JSONRPCRequest {
        id: RequestId::Integer(id),
        method: method.to_string(),
        params,
        trace: None,
    }))?;
    let payload = json!({
        "type": "client_message",
        "client_id": client_id,
        "message": message,
    });
    websocket
        .send(WebSocketMessage::Text(payload.to_string().into()))
        .await
        .context("client event should send")?;
    Ok(())
}

async fn read_remote_response_for_id(
    websocket: &mut BackendWebSocket,
    client_id: &str,
    id: i64,
) -> Result<Value> {
    loop {
        let event = read_remote_server_event(websocket).await?;
        if event["type"] == json!("server_message")
            && event["client_id"] == json!(client_id)
            && event["message"]["id"] == json!(id)
        {
            return Ok(event["message"].clone());
        }
    }
}

async fn read_remote_server_event(websocket: &mut BackendWebSocket) -> Result<Value> {
    loop {
        let frame = timeout(Duration::from_secs(5), websocket.next())
            .await
            .context("server event should arrive in time")?
            .context("websocket should stay open")?
            .context("websocket frame should be readable")?;
        match frame {
            WebSocketMessage::Text(text) => {
                return serde_json::from_str(text.as_ref())
                    .context("server event should deserialize");
            }
            WebSocketMessage::Ping(payload) => {
                websocket
                    .send(WebSocketMessage::Pong(payload))
                    .await
                    .context("websocket pong should send")?;
            }
            WebSocketMessage::Pong(_) => {}
            WebSocketMessage::Close(frame) => {
                anyhow::bail!("unexpected websocket close frame: {frame:?}");
            }
            WebSocketMessage::Binary(_) => anyhow::bail!("unexpected binary websocket frame"),
            WebSocketMessage::Frame(_) => {}
        }
    }
}

async fn spawn_stdio_server_with_remote_control(codex_home: &Path) -> Result<Child> {
    let program = codex_utils_cargo_bin::cargo_bin("codex-app-server")
        .context("should find app-server binary")?;
    let mut command = Command::new(program);
    command
        .arg("--with-remote-control")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("CODEX_HOME", codex_home)
        .env("RUST_LOG", "debug");
    let mut process = command
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn stdio app-server process")?;
    if let Some(stderr) = process.stderr.take() {
        let mut stderr_reader = BufReader::new(stderr).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = stderr_reader.next_line().await {
                eprintln!("[stdio app-server stderr] {line}");
            }
        });
    }
    Ok(process)
}

fn create_config_toml_with_remote_control(
    codex_home: &Path,
    server_uri: &str,
    remote_control_base_url: &str,
    approval_policy: &str,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "{approval_policy}"
sandbox_mode = "read-only"
chatgpt_base_url = "{remote_control_base_url}"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
