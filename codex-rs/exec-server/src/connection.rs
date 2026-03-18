use codex_app_server_protocol::JSONRPCMessage;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

pub(crate) const CHANNEL_CAPACITY: usize = 128;

#[derive(Debug)]
pub(crate) enum JsonRpcConnectionEvent {
    Message(JSONRPCMessage),
    Disconnected { reason: Option<String> },
}

pub(crate) struct JsonRpcConnection {
    outgoing_tx: mpsc::Sender<JSONRPCMessage>,
    incoming_rx: mpsc::Receiver<JsonRpcConnectionEvent>,
}

impl JsonRpcConnection {
    pub(crate) fn from_stdio<R, W>(reader: R, writer: W, connection_label: String) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_CAPACITY);

        let reader_label = connection_label.clone();
        let incoming_tx_for_reader = incoming_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JSONRPCMessage>(&line) {
                            Ok(message) => {
                                if incoming_tx_for_reader
                                    .send(JsonRpcConnectionEvent::Message(message))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                send_disconnected(
                                    &incoming_tx_for_reader,
                                    Some(format!(
                                        "failed to parse JSON-RPC message from {reader_label}: {err}"
                                    )),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        send_disconnected(&incoming_tx_for_reader, None).await;
                        break;
                    }
                    Err(err) => {
                        send_disconnected(
                            &incoming_tx_for_reader,
                            Some(format!(
                                "failed to read JSON-RPC message from {reader_label}: {err}"
                            )),
                        )
                        .await;
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut writer = BufWriter::new(writer);
            while let Some(message) = outgoing_rx.recv().await {
                if let Err(err) = write_jsonrpc_line_message(&mut writer, &message).await {
                    send_disconnected(
                        &incoming_tx,
                        Some(format!(
                            "failed to write JSON-RPC message to {connection_label}: {err}"
                        )),
                    )
                    .await;
                    break;
                }
            }
        });

        Self {
            outgoing_tx,
            incoming_rx,
        }
    }

    pub(crate) fn from_websocket<S>(stream: WebSocketStream<S>, connection_label: String) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (mut websocket_writer, mut websocket_reader) = stream.split();

        let reader_label = connection_label.clone();
        let incoming_tx_for_reader = incoming_tx.clone();
        tokio::spawn(async move {
            loop {
                match websocket_reader.next().await {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<JSONRPCMessage>(text.as_ref()) {
                            Ok(message) => {
                                if incoming_tx_for_reader
                                    .send(JsonRpcConnectionEvent::Message(message))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                send_disconnected(
                                    &incoming_tx_for_reader,
                                    Some(format!(
                                        "failed to parse websocket JSON-RPC message from {reader_label}: {err}"
                                    )),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        match serde_json::from_slice::<JSONRPCMessage>(bytes.as_ref()) {
                            Ok(message) => {
                                if incoming_tx_for_reader
                                    .send(JsonRpcConnectionEvent::Message(message))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                send_disconnected(
                                    &incoming_tx_for_reader,
                                    Some(format!(
                                        "failed to parse websocket JSON-RPC message from {reader_label}: {err}"
                                    )),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        send_disconnected(&incoming_tx_for_reader, None).await;
                        break;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        send_disconnected(
                            &incoming_tx_for_reader,
                            Some(format!(
                                "failed to read websocket JSON-RPC message from {reader_label}: {err}"
                            )),
                        )
                        .await;
                        break;
                    }
                    None => {
                        send_disconnected(&incoming_tx_for_reader, None).await;
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            while let Some(message) = outgoing_rx.recv().await {
                match serialize_jsonrpc_message(&message) {
                    Ok(encoded) => {
                        if let Err(err) = websocket_writer.send(Message::Text(encoded.into())).await
                        {
                            send_disconnected(
                                &incoming_tx,
                                Some(format!(
                                    "failed to write websocket JSON-RPC message to {connection_label}: {err}"
                                )),
                            )
                            .await;
                            break;
                        }
                    }
                    Err(err) => {
                        send_disconnected(
                            &incoming_tx,
                            Some(format!(
                                "failed to serialize JSON-RPC message for {connection_label}: {err}"
                            )),
                        )
                        .await;
                        break;
                    }
                }
            }
        });

        Self {
            outgoing_tx,
            incoming_rx,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        mpsc::Sender<JSONRPCMessage>,
        mpsc::Receiver<JsonRpcConnectionEvent>,
    ) {
        (self.outgoing_tx, self.incoming_rx)
    }
}

async fn send_disconnected(
    incoming_tx: &mpsc::Sender<JsonRpcConnectionEvent>,
    reason: Option<String>,
) {
    let _ = incoming_tx
        .send(JsonRpcConnectionEvent::Disconnected { reason })
        .await;
}

async fn write_jsonrpc_line_message<W>(
    writer: &mut BufWriter<W>,
    message: &JSONRPCMessage,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded =
        serialize_jsonrpc_message(message).map_err(|err| std::io::Error::other(err.to_string()))?;
    writer.write_all(encoded.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

fn serialize_jsonrpc_message(message: &JSONRPCMessage) -> Result<String, serde_json::Error> {
    serde_json::to_string(message)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use codex_app_server_protocol::JSONRPCMessage;
    use codex_app_server_protocol::JSONRPCRequest;
    use codex_app_server_protocol::JSONRPCResponse;
    use codex_app_server_protocol::RequestId;
    use pretty_assertions::assert_eq;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    use super::JsonRpcConnection;
    use super::JsonRpcConnectionEvent;
    use super::serialize_jsonrpc_message;

    async fn recv_event(
        incoming_rx: &mut mpsc::Receiver<JsonRpcConnectionEvent>,
    ) -> JsonRpcConnectionEvent {
        let recv_result = timeout(Duration::from_secs(1), incoming_rx.recv()).await;
        let maybe_event = match recv_result {
            Ok(maybe_event) => maybe_event,
            Err(err) => panic!("timed out waiting for connection event: {err}"),
        };
        match maybe_event {
            Some(event) => event,
            None => panic!("connection event stream ended unexpectedly"),
        }
    }

    async fn read_jsonrpc_line<R>(lines: &mut tokio::io::Lines<BufReader<R>>) -> JSONRPCMessage
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let next_line = timeout(Duration::from_secs(1), lines.next_line()).await;
        let line_result = match next_line {
            Ok(line_result) => line_result,
            Err(err) => panic!("timed out waiting for JSON-RPC line: {err}"),
        };
        let maybe_line = match line_result {
            Ok(maybe_line) => maybe_line,
            Err(err) => panic!("failed to read JSON-RPC line: {err}"),
        };
        let line = match maybe_line {
            Some(line) => line,
            None => panic!("connection closed before JSON-RPC line arrived"),
        };
        match serde_json::from_str::<JSONRPCMessage>(&line) {
            Ok(message) => message,
            Err(err) => panic!("failed to parse JSON-RPC line: {err}"),
        }
    }

    #[tokio::test]
    async fn stdio_connection_reads_and_writes_jsonrpc_messages() {
        let (mut writer_to_connection, connection_reader) = tokio::io::duplex(1024);
        let (connection_writer, reader_from_connection) = tokio::io::duplex(1024);
        let connection =
            JsonRpcConnection::from_stdio(connection_reader, connection_writer, "test".to_string());
        let (outgoing_tx, mut incoming_rx) = connection.into_parts();

        let incoming_message = JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(7),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({ "clientName": "test-client" })),
            trace: None,
        });
        let encoded = match serialize_jsonrpc_message(&incoming_message) {
            Ok(encoded) => encoded,
            Err(err) => panic!("failed to serialize incoming message: {err}"),
        };
        if let Err(err) = writer_to_connection
            .write_all(format!("{encoded}\n").as_bytes())
            .await
        {
            panic!("failed to write to connection: {err}");
        }

        let event = recv_event(&mut incoming_rx).await;
        match event {
            JsonRpcConnectionEvent::Message(message) => {
                assert_eq!(message, incoming_message);
            }
            JsonRpcConnectionEvent::Disconnected { reason } => {
                panic!("unexpected disconnect event: {reason:?}");
            }
        }

        let outgoing_message = JSONRPCMessage::Response(JSONRPCResponse {
            id: RequestId::Integer(7),
            result: serde_json::json!({ "protocolVersion": "exec-server.v0" }),
        });
        if let Err(err) = outgoing_tx.send(outgoing_message.clone()).await {
            panic!("failed to queue outgoing message: {err}");
        }

        let mut lines = BufReader::new(reader_from_connection).lines();
        let message = read_jsonrpc_line(&mut lines).await;
        assert_eq!(message, outgoing_message);
    }

    #[tokio::test]
    async fn stdio_connection_reports_parse_errors() {
        let (mut writer_to_connection, connection_reader) = tokio::io::duplex(1024);
        let (connection_writer, _reader_from_connection) = tokio::io::duplex(1024);
        let connection =
            JsonRpcConnection::from_stdio(connection_reader, connection_writer, "test".to_string());
        let (_outgoing_tx, mut incoming_rx) = connection.into_parts();

        if let Err(err) = writer_to_connection.write_all(b"not-json\n").await {
            panic!("failed to write invalid JSON: {err}");
        }

        let event = recv_event(&mut incoming_rx).await;
        match event {
            JsonRpcConnectionEvent::Disconnected { reason } => {
                let reason = match reason {
                    Some(reason) => reason,
                    None => panic!("expected a parse error reason"),
                };
                assert!(
                    reason.contains("failed to parse JSON-RPC message from test"),
                    "unexpected disconnect reason: {reason}"
                );
            }
            JsonRpcConnectionEvent::Message(message) => {
                panic!("unexpected JSON-RPC message: {message:?}");
            }
        }
    }

    #[tokio::test]
    async fn stdio_connection_reports_clean_disconnect() {
        let (writer_to_connection, connection_reader) = tokio::io::duplex(1024);
        let (connection_writer, _reader_from_connection) = tokio::io::duplex(1024);
        let connection =
            JsonRpcConnection::from_stdio(connection_reader, connection_writer, "test".to_string());
        let (_outgoing_tx, mut incoming_rx) = connection.into_parts();
        drop(writer_to_connection);

        let event = recv_event(&mut incoming_rx).await;
        match event {
            JsonRpcConnectionEvent::Disconnected { reason } => {
                assert_eq!(reason, None);
            }
            JsonRpcConnectionEvent::Message(message) => {
                panic!("unexpected JSON-RPC message: {message:?}");
            }
        }
    }
}
