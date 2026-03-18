use super::ClientEvent;
use super::REMOTE_CONTROL_REQUEST_ID_HEADER;
use codex_core::AuthManager;
use codex_core::CodexAuth;
use codex_core::test_support::auth_manager_from_auth;
use futures::SinkExt;
use futures::StreamExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::http::HeaderValue;

pub(super) fn remote_control_auth_manager() -> Arc<AuthManager> {
    auth_manager_from_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
}

#[derive(Debug)]
pub(super) struct CapturedHttpRequest {
    pub(super) stream: TcpStream,
    pub(super) request_line: String,
    pub(super) headers: BTreeMap<String, String>,
    pub(super) body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CapturedWebSocketRequest {
    pub(super) path: String,
    pub(super) headers: BTreeMap<String, String>,
}

pub(super) async fn accept_http_request(listener: &TcpListener) -> CapturedHttpRequest {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("HTTP request should arrive in time")
        .expect("listener accept should succeed");
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .expect("request line should read");
    let request_line = request_line.trim_end_matches("\r\n").to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("header line should read");
        if line == "\r\n" {
            break;
        }
        let line = line.trim_end_matches("\r\n");
        let (name, value) = line.split_once(':').expect("header should contain colon");
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
        .expect("request body should read");

    CapturedHttpRequest {
        stream: reader.into_inner(),
        request_line,
        headers,
        body: String::from_utf8(body).expect("body should be utf-8"),
    }
}

pub(super) async fn respond_with_json(mut stream: TcpStream, body: serde_json::Value) {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("response should write");
    stream.flush().await.expect("response should flush");
}

pub(super) async fn respond_with_status_and_headers(
    mut stream: TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) {
    let extra_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n{extra_headers}\r\n{body}",
        body.len(),
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("response should write");
    stream.flush().await.expect("response should flush");
}

pub(super) async fn accept_remote_control_backend_connection(
    listener: &TcpListener,
    request_id: Option<&str>,
) -> (CapturedWebSocketRequest, WebSocketStream<TcpStream>) {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("websocket request should arrive in time")
        .expect("listener accept should succeed");
    let captured_request = Arc::new(std::sync::Mutex::new(None::<CapturedWebSocketRequest>));
    let captured_request_for_callback = captured_request.clone();
    let request_id = request_id.map(str::to_owned);
    let websocket = accept_hdr_async(
        stream,
        move |request: &tungstenite::handshake::server::Request,
              mut response: tungstenite::handshake::server::Response| {
            let headers = request
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_ascii_lowercase(),
                        value
                            .to_str()
                            .expect("header should be valid utf-8")
                            .to_string(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            *captured_request_for_callback
                .lock()
                .expect("capture lock should acquire") = Some(CapturedWebSocketRequest {
                path: request.uri().path().to_string(),
                headers,
            });
            if let Some(request_id) = request_id.as_deref() {
                response.headers_mut().insert(
                    REMOTE_CONTROL_REQUEST_ID_HEADER,
                    HeaderValue::from_str(request_id)
                        .expect("request id should be a valid header value"),
                );
            }
            Ok(response)
        },
    )
    .await
    .expect("websocket handshake should succeed");
    let captured_request = captured_request
        .lock()
        .expect("capture lock should acquire")
        .clone()
        .expect("websocket request should be captured");
    (captured_request, websocket)
}

pub(super) async fn send_client_event(
    websocket: &mut WebSocketStream<TcpStream>,
    client_event: ClientEvent,
) {
    let payload = serde_json::to_string(&client_event).expect("client event should serialize");
    websocket
        .send(TungsteniteMessage::Text(payload.into()))
        .await
        .expect("client event should send");
}

pub(super) async fn read_server_event(
    websocket: &mut WebSocketStream<TcpStream>,
) -> serde_json::Value {
    loop {
        let frame = timeout(Duration::from_secs(5), websocket.next())
            .await
            .expect("server event should arrive in time")
            .expect("websocket should stay open")
            .expect("websocket frame should be readable");
        match frame {
            TungsteniteMessage::Text(text) => {
                return serde_json::from_str(text.as_ref())
                    .expect("server event should deserialize");
            }
            TungsteniteMessage::Ping(payload) => {
                websocket
                    .send(TungsteniteMessage::Pong(payload))
                    .await
                    .expect("websocket pong should send");
            }
            TungsteniteMessage::Pong(_) => {}
            TungsteniteMessage::Close(frame) => {
                panic!("unexpected websocket close frame: {frame:?}");
            }
            TungsteniteMessage::Binary(_) => {
                panic!("unexpected binary websocket frame");
            }
            TungsteniteMessage::Frame(_) => {}
        }
    }
}
