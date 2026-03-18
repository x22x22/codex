use std::collections::HashMap;
use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::time::timeout;

use super::ExecServerClient;
use super::ExecServerClientConnectOptions;
use super::ExecServerError;
use super::ExecServerOutput;
use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::EXEC_TERMINATE_METHOD;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecParams;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::ReadParams;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;

fn test_options() -> ExecServerClientConnectOptions {
    ExecServerClientConnectOptions {
        client_name: "test-client".to_string(),
        initialize_timeout: Duration::from_secs(1),
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
        None => panic!("server connection closed before JSON-RPC line arrived"),
    };
    match serde_json::from_str::<JSONRPCMessage>(&line) {
        Ok(message) => message,
        Err(err) => panic!("failed to parse JSON-RPC line: {err}"),
    }
}

async fn write_jsonrpc_line<W>(writer: &mut W, message: JSONRPCMessage)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let encoded = match serde_json::to_string(&message) {
        Ok(encoded) => encoded,
        Err(err) => panic!("failed to encode JSON-RPC message: {err}"),
    };
    if let Err(err) = writer.write_all(format!("{encoded}\n").as_bytes()).await {
        panic!("failed to write JSON-RPC line: {err}");
    }
}

#[tokio::test]
async fn connect_stdio_performs_initialize_handshake() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(request) = initialize else {
            panic!("expected initialize request");
        };
        assert_eq!(request.method, INITIALIZE_METHOD);
        assert_eq!(
            request.params,
            Some(serde_json::json!({ "clientName": "test-client" }))
        );
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(JSONRPCNotification { method, params }) = initialized
        else {
            panic!("expected initialized notification");
        };
        assert_eq!(method, INITIALIZED_METHOD);
        assert_eq!(params, Some(serde_json::json!({})));
    });

    let client = ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await;
    if let Err(err) = client {
        panic!("failed to connect test client: {err}");
    }

    if let Err(err) = server.await {
        panic!("server task failed: {err}");
    }
}

#[tokio::test]
async fn connect_in_process_starts_processes_without_jsonrpc_transport() {
    let client = match ExecServerClient::connect_in_process(test_options()).await {
        Ok(client) => client,
        Err(err) => panic!("failed to connect in-process client: {err}"),
    };

    let process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["printf".to_string(), "hello".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: false,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start in-process child: {err}"),
    };

    let mut output = process.output_receiver();
    let output = timeout(Duration::from_secs(1), output.recv())
        .await
        .unwrap_or_else(|err| panic!("timed out waiting for process output: {err}"))
        .unwrap_or_else(|err| panic!("failed to receive process output: {err}"));
    assert_eq!(
        output,
        ExecServerOutput {
            stream: crate::protocol::ExecOutputStream::Stdout,
            chunk: b"hello".to_vec(),
        }
    );
}

#[tokio::test]
async fn connect_in_process_read_returns_retained_output_and_exit_state() {
    let client = match ExecServerClient::connect_in_process(test_options()).await {
        Ok(client) => client,
        Err(err) => panic!("failed to connect in-process client: {err}"),
    };

    let response = match client
        .exec(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["printf".to_string(), "hello".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: false,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(response) => response,
        Err(err) => panic!("failed to start in-process child: {err}"),
    };

    let read = match client
        .read(ReadParams {
            process_id: response.process_id,
            after_seq: None,
            max_bytes: None,
            wait_ms: Some(1000),
        })
        .await
    {
        Ok(read) => read,
        Err(err) => panic!("failed to read in-process child output: {err}"),
    };

    assert_eq!(read.chunks.len(), 1);
    assert_eq!(read.chunks[0].seq, 1);
    assert_eq!(read.chunks[0].stream, ExecOutputStream::Stdout);
    assert_eq!(read.chunks[0].chunk.clone().into_inner(), b"hello".to_vec());
    assert_eq!(read.next_seq, 2);
    assert!(read.exited);
    assert_eq!(read.exit_code, Some(0));
}

#[tokio::test]
async fn connect_in_process_rejects_invalid_exec_params_from_handler() {
    let client = match ExecServerClient::connect_in_process(test_options()).await {
        Ok(client) => client,
        Err(err) => panic!("failed to connect in-process client: {err}"),
    };

    let result = client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: Vec::new(),
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: false,
            arg0: None,
            sandbox: None,
        })
        .await;

    match result {
        Err(ExecServerError::Server { code, message }) => {
            assert_eq!(code, -32602);
            assert_eq!(message, "argv must not be empty");
        }
        Err(err) => panic!("unexpected in-process exec failure: {err}"),
        Ok(_) => panic!("expected invalid params error"),
    }
}

#[tokio::test]
async fn connect_in_process_rejects_writes_to_unknown_processes() {
    let client = match ExecServerClient::connect_in_process(test_options()).await {
        Ok(client) => client,
        Err(err) => panic!("failed to connect in-process client: {err}"),
    };

    let result = client
        .write_process(crate::protocol::WriteParams {
            process_id: "missing".to_string(),
            chunk: b"input".to_vec().into(),
        })
        .await;

    match result {
        Err(ExecServerError::Server { code, message }) => {
            assert_eq!(code, -32600);
            assert_eq!(message, "unknown process id missing");
        }
        Err(err) => panic!("unexpected in-process write failure: {err}"),
        Ok(_) => panic!("expected unknown process error"),
    }
}

#[tokio::test]
async fn connect_in_process_terminate_marks_process_exited() {
    let client = match ExecServerClient::connect_in_process(test_options()).await {
        Ok(client) => client,
        Err(err) => panic!("failed to connect in-process client: {err}"),
    };

    let process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["sleep".to_string(), "30".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: false,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start in-process child: {err}"),
    };

    if let Err(err) = client.terminate_session(&process.process_id).await {
        panic!("failed to terminate in-process child: {err}");
    }

    timeout(Duration::from_secs(2), async {
        loop {
            if process.has_exited() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|err| panic!("timed out waiting for in-process child to exit: {err}"));

    assert!(process.has_exited());
}

#[tokio::test]
async fn dropping_in_process_client_terminates_running_processes() {
    let marker_path = std::env::temp_dir().join(format!(
        "codex-exec-server-inprocess-drop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&marker_path);

    {
        let client = match ExecServerClient::connect_in_process(test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect in-process client: {err}"),
        };

        let _ = client
            .exec(ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    format!("sleep 2; printf dropped > {}", marker_path.display()),
                ],
                cwd: std::env::current_dir().expect("cwd"),
                env: HashMap::new(),
                tty: false,
                arg0: None,
                sandbox: None,
            })
            .await
            .unwrap_or_else(|err| panic!("failed to start in-process child: {err}"));
    }

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !marker_path.exists(),
        "dropping the in-process client should terminate managed children"
    );
    let _ = std::fs::remove_file(&marker_path);
}

#[tokio::test]
async fn connect_stdio_returns_initialize_errors() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Error(JSONRPCError {
                id: request.id,
                error: JSONRPCErrorError {
                    code: -32600,
                    message: "rejected".to_string(),
                    data: None,
                },
            }),
        )
        .await;
    });

    let result = ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await;

    match result {
        Err(ExecServerError::Server { code, message }) => {
            assert_eq!(code, -32600);
            assert_eq!(message, "rejected");
        }
        Err(err) => panic!("unexpected initialize failure: {err}"),
        Ok(_) => panic!("expected initialize failure"),
    }
}

#[tokio::test]
async fn start_process_cleans_up_registered_process_after_request_error() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(initialize_request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize_request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(notification) = initialized else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);

        let exec_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
            panic!("expected exec request");
        };
        assert_eq!(method, EXEC_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Error(JSONRPCError {
                id,
                error: JSONRPCErrorError {
                    code: -32600,
                    message: "duplicate process".to_string(),
                    data: None,
                },
            }),
        )
        .await;
    });

    let client =
        match ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

    let result = client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: true,
            arg0: None,
            sandbox: None,
        })
        .await;

    match result {
        Err(ExecServerError::Server { code, message }) => {
            assert_eq!(code, -32600);
            assert_eq!(message, "duplicate process");
        }
        Err(err) => panic!("unexpected start_process failure: {err}"),
        Ok(_) => panic!("expected start_process failure"),
    }

    assert!(
        client.inner.pending.lock().await.is_empty(),
        "failed requests should not leave pending request state behind"
    );
}

#[tokio::test]
async fn connect_stdio_times_out_during_initialize_handshake() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (_server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let _ = read_jsonrpc_line(&mut lines).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let result = ExecServerClient::connect_stdio(
        client_stdin,
        client_stdout,
        ExecServerClientConnectOptions {
            client_name: "test-client".to_string(),
            initialize_timeout: Duration::from_millis(25),
        },
    )
    .await;

    match result {
        Err(ExecServerError::InitializeTimedOut { timeout }) => {
            assert_eq!(timeout, Duration::from_millis(25));
        }
        Err(err) => panic!("unexpected initialize timeout failure: {err}"),
        Ok(_) => panic!("expected initialize timeout"),
    }
}

#[tokio::test]
async fn start_process_preserves_output_stream_metadata() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(initialize_request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize_request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(notification) = initialized else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);

        let exec_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
            panic!("expected exec request");
        };
        assert_eq!(method, EXEC_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result: serde_json::json!({ "processId": "proc-1" }),
            }),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Notification(JSONRPCNotification {
                method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                params: Some(serde_json::json!({
                    "processId": "proc-1",
                    "stream": "stderr",
                    "chunk": "ZXJyb3IK"
                })),
            }),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let client =
        match ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

    let process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: true,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start process: {err}"),
    };

    let mut output = process.output_receiver();
    let output = timeout(Duration::from_secs(1), output.recv())
        .await
        .unwrap_or_else(|err| panic!("timed out waiting for process output: {err}"))
        .unwrap_or_else(|err| panic!("failed to receive process output: {err}"));
    assert_eq!(output.stream, ExecOutputStream::Stderr);
    assert_eq!(output.chunk, b"error\n".to_vec());
}

#[tokio::test]
async fn terminate_does_not_mark_process_exited_before_exit_notification() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(initialize_request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize_request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(notification) = initialized else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);

        let exec_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
            panic!("expected exec request");
        };
        assert_eq!(method, EXEC_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result: serde_json::json!({ "processId": "proc-1" }),
            }),
        )
        .await;

        let terminate_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = terminate_request else {
            panic!("expected terminate request");
        };
        assert_eq!(method, EXEC_TERMINATE_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result: serde_json::json!({ "running": true }),
            }),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let client =
        match ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

    let process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: true,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start process: {err}"),
    };

    process.terminate();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!process.has_exited(), "terminate should not imply exit");
    assert_eq!(process.exit_code(), None);
}

#[tokio::test]
async fn start_process_uses_protocol_process_ids() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(initialize_request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize_request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(notification) = initialized else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);

        let exec_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
            panic!("expected exec request");
        };
        assert_eq!(method, EXEC_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result: serde_json::json!({ "processId": "other-proc" }),
            }),
        )
        .await;
    });

    let client =
        match ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

    let process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: true,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start process: {err}"),
    };

    assert_eq!(process.process_id, "other-proc");
}

#[tokio::test]
async fn start_process_routes_output_for_protocol_process_ids() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(initialize_request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize_request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(notification) = initialized else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);

        let exec_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
            panic!("expected exec request");
        };
        assert_eq!(method, EXEC_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result: serde_json::json!({ "processId": "proc-1" }),
            }),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Notification(JSONRPCNotification {
                method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
                params: Some(serde_json::json!({
                    "processId": "proc-1",
                    "stream": "stdout",
                    "chunk": "YWxpdmUK"
                })),
            }),
        )
        .await;
    });

    let client =
        match ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

    let first_process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: true,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start first process: {err}"),
    };

    let mut output = first_process.output_receiver();
    let output = timeout(Duration::from_secs(1), output.recv())
        .await
        .unwrap_or_else(|err| panic!("timed out waiting for process output: {err}"))
        .unwrap_or_else(|err| panic!("failed to receive process output: {err}"));
    assert_eq!(output.stream, ExecOutputStream::Stdout);
    assert_eq!(output.chunk, b"alive\n".to_vec());
}

#[tokio::test]
async fn transport_shutdown_marks_processes_exited_without_exit_codes() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (mut server_writer, client_stdout) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();

        let initialize = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(initialize_request) = initialize else {
            panic!("expected initialize request");
        };
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize_request.id,
                result: serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            }),
        )
        .await;

        let initialized = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Notification(notification) = initialized else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);

        let exec_request = read_jsonrpc_line(&mut lines).await;
        let JSONRPCMessage::Request(JSONRPCRequest { id, method, .. }) = exec_request else {
            panic!("expected exec request");
        };
        assert_eq!(method, EXEC_METHOD);
        write_jsonrpc_line(
            &mut server_writer,
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result: serde_json::json!({ "processId": "proc-1" }),
            }),
        )
        .await;
        drop(server_writer);
    });

    let client =
        match ExecServerClient::connect_stdio(client_stdin, client_stdout, test_options()).await {
            Ok(client) => client,
            Err(err) => panic!("failed to connect test client: {err}"),
        };

    let process = match client
        .start_process(ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
            cwd: std::env::current_dir().unwrap_or_else(|err| panic!("missing cwd: {err}")),
            env: HashMap::new(),
            tty: true,
            arg0: None,
            sandbox: None,
        })
        .await
    {
        Ok(process) => process,
        Err(err) => panic!("failed to start process: {err}"),
    };

    let _ = process;
}
