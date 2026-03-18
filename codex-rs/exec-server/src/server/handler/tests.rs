use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::sync::Notify;
use tokio::time::timeout;

use super::ExecServerHandler;
use super::RetainedOutputChunk;
use super::RunningProcess;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecSandboxConfig;
use crate::protocol::ExecSandboxMode;
use crate::protocol::InitializeParams;
use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::ReadParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::server::routing::ExecServerClientNotification;
use crate::server::routing::ExecServerInboundMessage;
use crate::server::routing::ExecServerOutboundMessage;
use crate::server::routing::ExecServerRequest;
use crate::server::routing::ExecServerResponseMessage;
use codex_app_server_protocol::RequestId;

async fn recv_outbound(
    outgoing_rx: &mut tokio::sync::mpsc::Receiver<ExecServerOutboundMessage>,
) -> ExecServerOutboundMessage {
    let recv_result = timeout(Duration::from_secs(1), outgoing_rx.recv()).await;
    let maybe_message = match recv_result {
        Ok(maybe_message) => maybe_message,
        Err(err) => panic!("timed out waiting for handler output: {err}"),
    };
    match maybe_message {
        Some(message) => message,
        None => panic!("handler output channel closed unexpectedly"),
    }
}

#[tokio::test]
async fn initialize_response_reports_protocol_version() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }

    assert_eq!(
        recv_outbound(&mut outgoing_rx).await,
        ExecServerOutboundMessage::Response {
            request_id: RequestId::Integer(1),
            response: ExecServerResponseMessage::Initialize(InitializeResponse {
                protocol_version: PROTOCOL_VERSION.to_string(),
            }),
        }
    );
}

#[tokio::test]
async fn exec_methods_require_initialize() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id: RequestId::Integer(7),
            params: crate::protocol::ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().expect("cwd"),
                env: HashMap::new(),
                tty: true,
                arg0: None,
                sandbox: None,
            },
        }))
        .await
    {
        panic!("request handling should not fail the handler: {err}");
    }

    let ExecServerOutboundMessage::Error { request_id, error } =
        recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected invalid-request error");
    };
    assert_eq!(request_id, RequestId::Integer(7));
    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "client must call initialize before using exec methods"
    );
}

#[tokio::test]
async fn exec_methods_require_initialized_notification_after_initialize() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id: RequestId::Integer(2),
            params: crate::protocol::ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().expect("cwd"),
                env: HashMap::new(),
                tty: true,
                arg0: None,
                sandbox: None,
            },
        }))
        .await
    {
        panic!("request handling should not fail the handler: {err}");
    }

    let ExecServerOutboundMessage::Error { request_id, error } =
        recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected invalid-request error");
    };
    assert_eq!(request_id, RequestId::Integer(2));
    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "client must send initialized before using exec methods"
    );
}

#[tokio::test]
async fn initialized_before_initialize_is_a_protocol_error() {
    let (outgoing_tx, _outgoing_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    let result = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await;

    match result {
        Err(err) => {
            assert_eq!(
                err,
                "received `initialized` notification before `initialize`"
            );
        }
        Ok(()) => panic!("expected protocol error for early initialized notification"),
    }
}

#[tokio::test]
async fn initialize_may_only_be_sent_once_per_connection() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(2),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("duplicate initialize should not fail the handler: {err}");
    }

    let ExecServerOutboundMessage::Error { request_id, error } =
        recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected invalid-request error");
    };
    assert_eq!(request_id, RequestId::Integer(2));
    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "initialize may only be sent once per connection"
    );
}

#[tokio::test]
async fn host_default_sandbox_requests_are_rejected_until_supported() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(3);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await
    {
        panic!("initialized should succeed: {err}");
    }

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id: RequestId::Integer(2),
            params: crate::protocol::ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().expect("cwd"),
                env: HashMap::new(),
                tty: false,
                arg0: None,
                sandbox: Some(ExecSandboxConfig {
                    mode: ExecSandboxMode::HostDefault,
                }),
            },
        }))
        .await
    {
        panic!("request handling should not fail the handler: {err}");
    }

    let ExecServerOutboundMessage::Error { request_id, error } =
        recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected unsupported sandbox error");
    };
    assert_eq!(request_id, RequestId::Integer(2));
    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "sandbox mode `hostDefault` is not supported by exec-server yet"
    );
}

#[tokio::test]
async fn exec_echoes_client_process_ids() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(4);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await
    {
        panic!("initialized should succeed: {err}");
    }

    let params = crate::protocol::ExecParams {
        process_id: "proc-1".to_string(),
        argv: vec![
            "bash".to_string(),
            "-lc".to_string(),
            "sleep 30".to_string(),
        ],
        cwd: std::env::current_dir().expect("cwd"),
        env: HashMap::new(),
        tty: false,
        arg0: None,
        sandbox: None,
    };
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id: RequestId::Integer(2),
            params: params.clone(),
        }))
        .await
    {
        panic!("first exec should succeed: {err}");
    }
    let ExecServerOutboundMessage::Response {
        request_id,
        response: ExecServerResponseMessage::Exec(first_exec),
    } = recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected first exec response");
    };
    assert_eq!(request_id, RequestId::Integer(2));
    assert_eq!(first_exec.process_id, "proc-1");

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id: RequestId::Integer(3),
            params: crate::protocol::ExecParams {
                process_id: "proc-2".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                ..params
            },
        }))
        .await
    {
        panic!("second exec should succeed: {err}");
    }

    let ExecServerOutboundMessage::Response {
        request_id,
        response: ExecServerResponseMessage::Exec(second_exec),
    } = recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected second exec response");
    };
    assert_eq!(request_id, RequestId::Integer(3));
    assert_eq!(second_exec.process_id, "proc-2");

    handler.shutdown().await;
}

#[tokio::test]
async fn writes_to_pipe_backed_processes_are_rejected() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(4);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await
    {
        panic!("initialized should succeed: {err}");
    }

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
            request_id: RequestId::Integer(2),
            params: crate::protocol::ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec![
                    "bash".to_string(),
                    "-lc".to_string(),
                    "sleep 30".to_string(),
                ],
                cwd: std::env::current_dir().expect("cwd"),
                env: HashMap::new(),
                tty: false,
                arg0: None,
                sandbox: None,
            },
        }))
        .await
    {
        panic!("exec should succeed: {err}");
    }
    let ExecServerOutboundMessage::Response {
        response: ExecServerResponseMessage::Exec(exec_response),
        ..
    } = recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected exec response");
    };

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Write {
                request_id: RequestId::Integer(3),
                params: WriteParams {
                    process_id: exec_response.process_id,
                    chunk: b"hello\n".to_vec().into(),
                },
            },
        ))
        .await
    {
        panic!("write should not fail the handler: {err}");
    }

    let ExecServerOutboundMessage::Error { request_id, error } =
        recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected stdin-closed error");
    };
    assert_eq!(request_id, RequestId::Integer(3));
    assert_eq!(error.code, -32600);
    assert_eq!(error.message, "stdin is closed for process proc-1");

    handler.shutdown().await;
}

#[tokio::test]
async fn writes_to_unknown_processes_are_rejected() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await
    {
        panic!("initialized should succeed: {err}");
    }

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Write {
                request_id: RequestId::Integer(2),
                params: WriteParams {
                    process_id: "missing".to_string(),
                    chunk: b"hello\n".to_vec().into(),
                },
            },
        ))
        .await
    {
        panic!("write should not fail the handler: {err}");
    }

    let ExecServerOutboundMessage::Error { request_id, error } =
        recv_outbound(&mut outgoing_rx).await
    else {
        panic!("expected unknown-process error");
    };
    assert_eq!(request_id, RequestId::Integer(2));
    assert_eq!(error.code, -32600);
    assert_eq!(error.message, "unknown process id missing");
}

#[tokio::test]
async fn terminate_unknown_processes_report_running_false() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await
    {
        panic!("initialized should succeed: {err}");
    }

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Terminate {
                request_id: RequestId::Integer(2),
                params: crate::protocol::TerminateParams {
                    process_id: "missing".to_string(),
                },
            },
        ))
        .await
    {
        panic!("terminate should not fail the handler: {err}");
    }

    assert_eq!(
        recv_outbound(&mut outgoing_rx).await,
        ExecServerOutboundMessage::Response {
            request_id: RequestId::Integer(2),
            response: ExecServerResponseMessage::Terminate(TerminateResponse { running: false }),
        }
    );
}

#[tokio::test]
async fn terminate_keeps_process_ids_reserved() {
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
    let mut handler = ExecServerHandler::new(outgoing_tx);

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Initialize {
                request_id: RequestId::Integer(1),
                params: InitializeParams {
                    client_name: "test".to_string(),
                },
            },
        ))
        .await
    {
        panic!("initialize should succeed: {err}");
    }
    let _ = recv_outbound(&mut outgoing_rx).await;
    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Notification(
            ExecServerClientNotification::Initialized,
        ))
        .await
    {
        panic!("initialized should succeed: {err}");
    }

    let spawned = codex_utils_pty::spawn_pipe_process_no_stdin(
        "bash",
        &["-lc".to_string(), "sleep 30".to_string()],
        std::env::current_dir().expect("cwd").as_path(),
        &HashMap::new(),
        &None,
    )
    .await
    .expect("spawn test process");
    {
        let mut process_map = handler.processes.lock().await;
        process_map.insert(
            "proc-1".to_string(),
            super::RunningProcess {
                session: spawned.session,
                tty: false,
                output: std::collections::VecDeque::new(),
                retained_bytes: 0,
                next_seq: 1,
                exit_code: None,
                output_notify: Arc::new(Notify::new()),
            },
        );
    }

    if let Err(err) = handler
        .handle_message(ExecServerInboundMessage::Request(
            ExecServerRequest::Terminate {
                request_id: RequestId::Integer(2),
                params: crate::protocol::TerminateParams {
                    process_id: "proc-1".to_string(),
                },
            },
        ))
        .await
    {
        panic!("terminate should not fail the handler: {err}");
    }

    assert_eq!(
        recv_outbound(&mut outgoing_rx).await,
        ExecServerOutboundMessage::Response {
            request_id: RequestId::Integer(2),
            response: ExecServerResponseMessage::Terminate(TerminateResponse { running: true }),
        }
    );

    assert!(
        handler.processes.lock().await.contains_key("proc-1"),
        "terminated ids should stay reserved until exit cleanup removes them"
    );

    handler.shutdown().await;
}

#[tokio::test]
async fn read_paginates_retained_output_without_skipping_omitted_chunks() {
    let (outgoing_tx, _outgoing_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(outgoing_tx);
    let _ = handler.initialize().expect("initialize should succeed");
    handler.initialized().expect("initialized should succeed");

    let spawned = codex_utils_pty::spawn_pipe_process_no_stdin(
        "bash",
        &["-lc".to_string(), "true".to_string()],
        std::env::current_dir().expect("cwd").as_path(),
        &HashMap::new(),
        &None,
    )
    .await
    .expect("spawn test process");
    {
        let mut process_map = handler.processes.lock().await;
        process_map.insert(
            "proc-1".to_string(),
            RunningProcess {
                session: spawned.session,
                tty: false,
                output: VecDeque::from([
                    RetainedOutputChunk {
                        seq: 1,
                        stream: ExecOutputStream::Stdout,
                        chunk: b"abc".to_vec(),
                    },
                    RetainedOutputChunk {
                        seq: 2,
                        stream: ExecOutputStream::Stderr,
                        chunk: b"def".to_vec(),
                    },
                ]),
                retained_bytes: 6,
                next_seq: 3,
                exit_code: None,
                output_notify: Arc::new(Notify::new()),
            },
        );
    }

    let first = handler
        .read(ReadParams {
            process_id: "proc-1".to_string(),
            after_seq: Some(0),
            max_bytes: Some(3),
            wait_ms: Some(0),
        })
        .await
        .expect("first read should succeed");

    assert_eq!(first.chunks.len(), 1);
    assert_eq!(first.chunks[0].seq, 1);
    assert_eq!(first.chunks[0].stream, ExecOutputStream::Stdout);
    assert_eq!(first.chunks[0].chunk.clone().into_inner(), b"abc".to_vec());
    assert_eq!(first.next_seq, 2);

    let second = handler
        .read(ReadParams {
            process_id: "proc-1".to_string(),
            after_seq: Some(first.next_seq - 1),
            max_bytes: Some(3),
            wait_ms: Some(0),
        })
        .await
        .expect("second read should succeed");

    assert_eq!(second.chunks.len(), 1);
    assert_eq!(second.chunks[0].seq, 2);
    assert_eq!(second.chunks[0].stream, ExecOutputStream::Stderr);
    assert_eq!(second.chunks[0].chunk.clone().into_inner(), b"def".to_vec());
    assert_eq!(second.next_seq, 3);

    handler.shutdown().await;
}
