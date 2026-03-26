use super::enroll::REMOTE_CONTROL_ACCOUNT_ID_HEADER;
use super::enroll::REMOTE_CONTROL_PROTOCOL_VERSION;
use super::enroll::RemoteControlConnectionAuth;
use super::enroll::RemoteControlEnrollment;
use super::enroll::connect_remote_control_websocket;
use super::enroll::enroll_remote_control_server;
use super::enroll::load_persisted_remote_control_enrollment;
use super::enroll::update_persisted_remote_control_enrollment;
use super::*;
use crate::AppServerTransport;
use crate::error_code::OVERLOADED_ERROR_CODE;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::QueuedOutgoingMessage;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::OutboundConnectionState;
use crate::transport::TransportEvent;
use crate::transport::enqueue_incoming_message;
use crate::transport::route_outgoing_envelope;
use base64::Engine;
use codex_app_server_protocol::CommandExecutionRequestApprovalSkillMetadata;
use codex_app_server_protocol::ConfigWarningNotification;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_core::AuthManager;
use codex_core::CodexAuth;
use codex_core::test_support::auth_manager_from_auth;
use codex_core::test_support::auth_manager_from_auth_with_home;
use codex_state::StateRuntime;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::SinkExt;
use futures::StreamExt;
use gethostname::gethostname;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite;
use tokio_util::sync::CancellationToken;

fn absolute_path(path: &str) -> AbsolutePathBuf {
    AbsolutePathBuf::from_absolute_path(path).expect("absolute path")
}

fn remote_control_auth_manager() -> Arc<AuthManager> {
    auth_manager_from_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
}

fn remote_control_auth_manager_with_home(codex_home: &TempDir) -> Arc<AuthManager> {
    auth_manager_from_auth_with_home(
        CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        codex_home.path().to_path_buf(),
    )
}

async fn remote_control_state_runtime(codex_home: &TempDir) -> Arc<StateRuntime> {
    StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string())
        .await
        .expect("state runtime should initialize")
}

#[test]
fn app_server_transport_parses_stdio_listen_url() {
    let transport = AppServerTransport::from_listen_url(AppServerTransport::DEFAULT_LISTEN_URL)
        .expect("stdio listen URL should parse");
    assert_eq!(transport, AppServerTransport::Stdio);
}

#[test]
fn app_server_transport_parses_websocket_listen_url() {
    let transport = AppServerTransport::from_listen_url("ws://127.0.0.1:1234")
        .expect("websocket listen URL should parse");
    assert_eq!(
        transport,
        AppServerTransport::WebSocket {
            bind_address: "127.0.0.1:1234".parse().expect("valid socket address"),
        }
    );
}

#[test]
fn app_server_transport_rejects_invalid_websocket_listen_url() {
    let err = AppServerTransport::from_listen_url("ws://localhost:1234")
        .expect_err("hostname bind address should be rejected");
    assert_eq!(
        err.to_string(),
        "invalid websocket --listen URL `ws://localhost:1234`; expected `ws://IP:PORT`"
    );
}

#[test]
fn app_server_transport_rejects_unsupported_listen_url() {
    let err = AppServerTransport::from_listen_url("http://127.0.0.1:1234")
        .expect_err("unsupported scheme should fail");
    assert_eq!(
        err.to_string(),
        "unsupported --listen URL `http://127.0.0.1:1234`; expected `stdio://` or `ws://IP:PORT`"
    );
}

#[tokio::test]
async fn validate_remote_control_auth_rejects_api_key_auth() {
    let auth_manager = auth_manager_from_auth(CodexAuth::from_api_key("sk-test"));

    let err = validate_remote_control_auth(auth_manager.as_ref())
        .await
        .expect_err("API key auth should be rejected");

    assert_eq!(
        err.to_string(),
        "remote control requires ChatGPT authentication; API key auth is not supported"
    );
}

#[tokio::test]
async fn enqueue_incoming_request_returns_overload_error_when_queue_is_full() {
    let connection_id = ConnectionId(42);
    let (transport_event_tx, mut transport_event_rx) = mpsc::channel(1);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);

    let first_message =
        JSONRPCMessage::Notification(codex_app_server_protocol::JSONRPCNotification {
            method: "initialized".to_string(),
            params: None,
        });
    transport_event_tx
        .send(TransportEvent::IncomingMessage {
            connection_id,
            message: first_message.clone(),
        })
        .await
        .expect("queue should accept first message");

    let request = JSONRPCMessage::Request(codex_app_server_protocol::JSONRPCRequest {
        id: codex_app_server_protocol::RequestId::Integer(7),
        method: "config/read".to_string(),
        params: Some(json!({ "includeLayers": false })),
        trace: None,
    });
    assert!(
        enqueue_incoming_message(&transport_event_tx, &writer_tx, connection_id, request).await
    );

    let queued_event = transport_event_rx
        .recv()
        .await
        .expect("first event should stay queued");
    match queued_event {
        TransportEvent::IncomingMessage {
            connection_id: queued_connection_id,
            message,
        } => {
            assert_eq!(queued_connection_id, connection_id);
            assert_eq!(message, first_message);
        }
        _ => panic!("expected queued incoming message"),
    }

    let overload = writer_rx
        .recv()
        .await
        .expect("request should receive overload error");
    let overload_json = serde_json::to_value(overload.message).expect("serialize overload error");
    assert_eq!(
        overload_json,
        json!({
            "id": 7,
            "error": {
                "code": OVERLOADED_ERROR_CODE,
                "message": "Server overloaded; retry later."
            }
        })
    );
}

#[tokio::test]
async fn enqueue_incoming_response_waits_instead_of_dropping_when_queue_is_full() {
    let connection_id = ConnectionId(42);
    let (transport_event_tx, mut transport_event_rx) = mpsc::channel(1);
    let (writer_tx, _writer_rx) = mpsc::channel(1);

    let first_message =
        JSONRPCMessage::Notification(codex_app_server_protocol::JSONRPCNotification {
            method: "initialized".to_string(),
            params: None,
        });
    transport_event_tx
        .send(TransportEvent::IncomingMessage {
            connection_id,
            message: first_message.clone(),
        })
        .await
        .expect("queue should accept first message");

    let response = JSONRPCMessage::Response(codex_app_server_protocol::JSONRPCResponse {
        id: codex_app_server_protocol::RequestId::Integer(7),
        result: json!({"ok": true}),
    });
    let transport_event_tx_for_enqueue = transport_event_tx.clone();
    let writer_tx_for_enqueue = writer_tx.clone();
    let enqueue_handle = tokio::spawn(async move {
        enqueue_incoming_message(
            &transport_event_tx_for_enqueue,
            &writer_tx_for_enqueue,
            connection_id,
            response,
        )
        .await
    });

    let queued_event = transport_event_rx
        .recv()
        .await
        .expect("first event should be dequeued");
    match queued_event {
        TransportEvent::IncomingMessage {
            connection_id: queued_connection_id,
            message,
        } => {
            assert_eq!(queued_connection_id, connection_id);
            assert_eq!(message, first_message);
        }
        _ => panic!("expected queued incoming message"),
    }

    let enqueue_result = enqueue_handle.await.expect("enqueue task should not panic");
    assert!(enqueue_result);

    let forwarded_event = transport_event_rx
        .recv()
        .await
        .expect("response should be forwarded instead of dropped");
    match forwarded_event {
        TransportEvent::IncomingMessage {
            connection_id: queued_connection_id,
            message:
                JSONRPCMessage::Response(codex_app_server_protocol::JSONRPCResponse { id, result }),
        } => {
            assert_eq!(queued_connection_id, connection_id);
            assert_eq!(id, codex_app_server_protocol::RequestId::Integer(7));
            assert_eq!(result, json!({"ok": true}));
        }
        _ => panic!("expected forwarded response message"),
    }
}

#[tokio::test]
async fn enqueue_incoming_request_does_not_block_when_writer_queue_is_full() {
    let connection_id = ConnectionId(42);
    let (transport_event_tx, _transport_event_rx) = mpsc::channel(1);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);

    transport_event_tx
        .send(TransportEvent::IncomingMessage {
            connection_id,
            message: JSONRPCMessage::Notification(codex_app_server_protocol::JSONRPCNotification {
                method: "initialized".to_string(),
                params: None,
            }),
        })
        .await
        .expect("transport queue should accept first message");

    writer_tx
        .send(QueuedOutgoingMessage::new(
            OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "queued".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
        ))
        .await
        .expect("writer queue should accept first message");

    let request = JSONRPCMessage::Request(codex_app_server_protocol::JSONRPCRequest {
        id: codex_app_server_protocol::RequestId::Integer(7),
        method: "config/read".to_string(),
        params: Some(json!({ "includeLayers": false })),
        trace: None,
    });

    let enqueue_result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        enqueue_incoming_message(&transport_event_tx, &writer_tx, connection_id, request),
    )
    .await
    .expect("enqueue should not block while writer queue is full");
    assert!(enqueue_result);

    let queued_outgoing = writer_rx
        .recv()
        .await
        .expect("writer queue should still contain original message");
    let queued_json =
        serde_json::to_value(queued_outgoing.message).expect("serialize queued message");
    assert_eq!(
        queued_json,
        json!({
            "method": "configWarning",
            "params": {
                "summary": "queued",
                "details": null,
            },
        })
    );
}

#[tokio::test]
async fn to_connection_notification_respects_opt_out_filters() {
    let connection_id = ConnectionId(7);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);
    let initialized = Arc::new(AtomicBool::new(true));
    let opted_out_notification_methods =
        Arc::new(RwLock::new(HashSet::from(["configWarning".to_string()])));

    let mut connections = HashMap::new();
    connections.insert(
        connection_id,
        OutboundConnectionState::new(
            writer_tx,
            initialized,
            Arc::new(AtomicBool::new(true)),
            opted_out_notification_methods,
            None,
        ),
    );

    route_outgoing_envelope(
        &mut connections,
        OutgoingEnvelope::ToConnection {
            connection_id,
            message: OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "task_started".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
            write_complete_tx: None,
        },
    )
    .await;

    assert!(
        writer_rx.try_recv().is_err(),
        "opted-out notification should be dropped"
    );
}

#[tokio::test]
async fn to_connection_notifications_are_dropped_for_opted_out_clients() {
    let connection_id = ConnectionId(10);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);

    let mut connections = HashMap::new();
    connections.insert(
        connection_id,
        OutboundConnectionState::new(
            writer_tx,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            Arc::new(RwLock::new(HashSet::from(["configWarning".to_string()]))),
            None,
        ),
    );

    route_outgoing_envelope(
        &mut connections,
        OutgoingEnvelope::ToConnection {
            connection_id,
            message: OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "task_started".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
            write_complete_tx: None,
        },
    )
    .await;

    assert!(
        writer_rx.try_recv().is_err(),
        "opted-out notifications should not reach clients"
    );
}

#[tokio::test]
async fn to_connection_notifications_are_preserved_for_non_opted_out_clients() {
    let connection_id = ConnectionId(11);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);

    let mut connections = HashMap::new();
    connections.insert(
        connection_id,
        OutboundConnectionState::new(
            writer_tx,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            Arc::new(RwLock::new(HashSet::new())),
            None,
        ),
    );

    route_outgoing_envelope(
        &mut connections,
        OutgoingEnvelope::ToConnection {
            connection_id,
            message: OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "task_started".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
            write_complete_tx: None,
        },
    )
    .await;

    let message = writer_rx
        .recv()
        .await
        .expect("notification should reach non-opted-out clients");
    assert!(matches!(
        message.message,
        OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
            ConfigWarningNotification { summary, .. }
        )) if summary == "task_started"
    ));
}

#[tokio::test]
async fn command_execution_request_approval_strips_experimental_fields_without_capability() {
    let connection_id = ConnectionId(8);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);

    let mut connections = HashMap::new();
    connections.insert(
        connection_id,
        OutboundConnectionState::new(
            writer_tx,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(RwLock::new(HashSet::new())),
            None,
        ),
    );

    route_outgoing_envelope(
        &mut connections,
        OutgoingEnvelope::ToConnection {
            connection_id,
            message: OutgoingMessage::Request(ServerRequest::CommandExecutionRequestApproval {
                request_id: codex_app_server_protocol::RequestId::Integer(1),
                params: codex_app_server_protocol::CommandExecutionRequestApprovalParams {
                    thread_id: "thr_123".to_string(),
                    turn_id: "turn_123".to_string(),
                    item_id: "call_123".to_string(),
                    approval_id: None,
                    reason: Some("Need extra read access".to_string()),
                    network_approval_context: None,
                    command: Some("cat file".to_string()),
                    cwd: Some(PathBuf::from("/tmp")),
                    command_actions: None,
                    additional_permissions: Some(
                        codex_app_server_protocol::AdditionalPermissionProfile {
                            network: None,
                            file_system: Some(
                                codex_app_server_protocol::AdditionalFileSystemPermissions {
                                    read: Some(vec![absolute_path("/tmp/allowed")]),
                                    write: None,
                                },
                            ),
                            macos: None,
                        },
                    ),
                    skill_metadata: Some(CommandExecutionRequestApprovalSkillMetadata {
                        path_to_skills_md: PathBuf::from("/tmp/SKILLS.md"),
                    }),
                    proposed_execpolicy_amendment: None,
                    proposed_network_policy_amendments: None,
                    available_decisions: None,
                },
            }),
            write_complete_tx: None,
        },
    )
    .await;

    let message = writer_rx
        .recv()
        .await
        .expect("request should be delivered to the connection");
    let json = serde_json::to_value(message.message).expect("request should serialize");
    assert_eq!(json["params"].get("additionalPermissions"), None);
    assert_eq!(json["params"].get("skillMetadata"), None);
}

#[tokio::test]
async fn command_execution_request_approval_keeps_experimental_fields_with_capability() {
    let connection_id = ConnectionId(9);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);

    let mut connections = HashMap::new();
    connections.insert(
        connection_id,
        OutboundConnectionState::new(
            writer_tx,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            Arc::new(RwLock::new(HashSet::new())),
            None,
        ),
    );

    route_outgoing_envelope(
        &mut connections,
        OutgoingEnvelope::ToConnection {
            connection_id,
            message: OutgoingMessage::Request(ServerRequest::CommandExecutionRequestApproval {
                request_id: codex_app_server_protocol::RequestId::Integer(1),
                params: codex_app_server_protocol::CommandExecutionRequestApprovalParams {
                    thread_id: "thr_123".to_string(),
                    turn_id: "turn_123".to_string(),
                    item_id: "call_123".to_string(),
                    approval_id: None,
                    reason: Some("Need extra read access".to_string()),
                    network_approval_context: None,
                    command: Some("cat file".to_string()),
                    cwd: Some(PathBuf::from("/tmp")),
                    command_actions: None,
                    additional_permissions: Some(
                        codex_app_server_protocol::AdditionalPermissionProfile {
                            network: None,
                            file_system: Some(
                                codex_app_server_protocol::AdditionalFileSystemPermissions {
                                    read: Some(vec![absolute_path("/tmp/allowed")]),
                                    write: None,
                                },
                            ),
                            macos: None,
                        },
                    ),
                    skill_metadata: Some(CommandExecutionRequestApprovalSkillMetadata {
                        path_to_skills_md: PathBuf::from("/tmp/SKILLS.md"),
                    }),
                    proposed_execpolicy_amendment: None,
                    proposed_network_policy_amendments: None,
                    available_decisions: None,
                },
            }),
            write_complete_tx: None,
        },
    )
    .await;

    let message = writer_rx
        .recv()
        .await
        .expect("request should be delivered to the connection");
    let json = serde_json::to_value(message.message).expect("request should serialize");
    let allowed_path = absolute_path("/tmp/allowed").to_string_lossy().into_owned();
    assert_eq!(
        json["params"]["additionalPermissions"],
        json!({
            "network": null,
            "fileSystem": {
                "read": [allowed_path],
                "write": null,
            },
            "macos": null,
        })
    );
    assert_eq!(
        json["params"]["skillMetadata"],
        json!({
            "pathToSkillsMd": "/tmp/SKILLS.md",
        })
    );
}

#[tokio::test]
async fn broadcast_does_not_block_on_slow_connection() {
    let fast_connection_id = ConnectionId(1);
    let slow_connection_id = ConnectionId(2);

    let (fast_writer_tx, mut fast_writer_rx) = mpsc::channel(1);
    let (slow_writer_tx, mut slow_writer_rx) = mpsc::channel(1);
    let fast_disconnect_token = CancellationToken::new();
    let slow_disconnect_token = CancellationToken::new();

    let mut connections = HashMap::new();
    connections.insert(
        fast_connection_id,
        OutboundConnectionState::new(
            fast_writer_tx,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            Arc::new(RwLock::new(HashSet::new())),
            Some(fast_disconnect_token.clone()),
        ),
    );
    connections.insert(
        slow_connection_id,
        OutboundConnectionState::new(
            slow_writer_tx.clone(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            Arc::new(RwLock::new(HashSet::new())),
            Some(slow_disconnect_token.clone()),
        ),
    );

    let queued_message = OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
        ConfigWarningNotification {
            summary: "already-buffered".to_string(),
            details: None,
            path: None,
            range: None,
        },
    ));
    slow_writer_tx
        .try_send(QueuedOutgoingMessage::new(queued_message))
        .expect("channel should have room");

    let broadcast_message = OutgoingMessage::AppServerNotification(
        ServerNotification::ConfigWarning(ConfigWarningNotification {
            summary: "test".to_string(),
            details: None,
            path: None,
            range: None,
        }),
    );
    timeout(
        Duration::from_millis(100),
        route_outgoing_envelope(
            &mut connections,
            OutgoingEnvelope::Broadcast {
                message: broadcast_message,
            },
        ),
    )
    .await
    .expect("broadcast should return even when one connection is slow");
    assert!(!connections.contains_key(&slow_connection_id));
    assert!(slow_disconnect_token.is_cancelled());
    assert!(!fast_disconnect_token.is_cancelled());
    let fast_message = fast_writer_rx
        .try_recv()
        .expect("fast connection should receive the broadcast notification");
    assert!(matches!(
        fast_message.message,
        OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
            ConfigWarningNotification { summary, .. }
        )) if summary == "test"
    ));

    let slow_message = slow_writer_rx
        .try_recv()
        .expect("slow connection should retain its original buffered message");
    assert!(matches!(
        slow_message.message,
        OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
            ConfigWarningNotification { summary, .. }
        )) if summary == "already-buffered"
    ));
}

#[tokio::test]
async fn to_connection_stdio_waits_instead_of_disconnecting_when_writer_queue_is_full() {
    let connection_id = ConnectionId(3);
    let (writer_tx, mut writer_rx) = mpsc::channel(1);
    writer_tx
        .send(QueuedOutgoingMessage::new(
            OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "queued".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
        ))
        .await
        .expect("channel should accept the first queued message");

    let mut connections = HashMap::new();
    connections.insert(
        connection_id,
        OutboundConnectionState::new(
            writer_tx,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            Arc::new(RwLock::new(HashSet::new())),
            None,
        ),
    );

    let route_task = tokio::spawn(async move {
        route_outgoing_envelope(
            &mut connections,
            OutgoingEnvelope::ToConnection {
                connection_id,
                message: OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                    ConfigWarningNotification {
                        summary: "second".to_string(),
                        details: None,
                        path: None,
                        range: None,
                    },
                )),
                write_complete_tx: None,
            },
        )
        .await
    });

    let first = timeout(Duration::from_millis(100), writer_rx.recv())
        .await
        .expect("first queued message should be readable")
        .expect("first queued message should exist");
    timeout(Duration::from_millis(100), route_task)
        .await
        .expect("routing should finish after the first queued message is drained")
        .expect("routing task should succeed");

    assert!(matches!(
        first.message,
        OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
            ConfigWarningNotification { summary, .. }
        )) if summary == "queued"
    ));
    let second = writer_rx
        .try_recv()
        .expect("second notification should be delivered once the queue has room");
    assert!(matches!(
        second.message,
        OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
            ConfigWarningNotification { summary, .. }
        )) if summary == "second"
    ));
}

#[tokio::test]
async fn connect_remote_control_websocket_includes_http_error_details() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let expected_error = format!(
        "failed to connect app-server remote control websocket `{}`: HTTP error: 503 Service Unavailable, headers: {{connection: close, content-length: 20, content-type: text/plain, x-region: us-east-1, x-trace-id: trace-503}}, body: upstream unavailable",
        remote_control_target.websocket_url
    );
    let server_task = tokio::spawn(async move {
        let request = accept_http_request(&listener).await;
        assert_eq!(
            request.request_line,
            "GET /backend-api/wham/remote/control/server HTTP/1.1"
        );
        respond_with_status_and_headers(
            request.stream,
            "503 Service Unavailable",
            &[("x-trace-id", "trace-503"), ("x-region", "us-east-1")],
            "upstream unavailable",
        )
        .await;
    });
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&codex_home).await;
    let auth_manager = remote_control_auth_manager();
    let mut enrollment = Some(RemoteControlEnrollment {
        server_id: "srv_e_test".to_string(),
        server_name: "test-server".to_string(),
    });

    let err = match connect_remote_control_websocket(
        &remote_control_target,
        Some(state_db.as_ref()),
        auth_manager.as_ref(),
        &mut enrollment,
        None,
    )
    .await
    {
        Ok(_) => panic!("http error response should fail the websocket connect"),
        Err(err) => err,
    };

    server_task.await.expect("server task should succeed");
    assert_eq!(err.to_string(), expected_error);
}

#[tokio::test]
async fn persisted_remote_control_enrollment_round_trips_by_target_and_account() {
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&codex_home).await;
    let first_target = normalize_remote_control_url("http://example.com/remote/control")
        .expect("first target should parse");
    let second_target = normalize_remote_control_url("http://example.com/other/control")
        .expect("second target should parse");
    let first_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_first".to_string(),
        server_name: "first-server".to_string(),
    };
    let second_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_second".to_string(),
        server_name: "second-server".to_string(),
    };

    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &first_target,
        Some("account-a"),
        Some(&first_enrollment),
    )
    .await
    .expect("first enrollment should persist");
    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &second_target,
        Some("account-a"),
        Some(&second_enrollment),
    )
    .await
    .expect("second enrollment should persist");

    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &first_target,
            Some("account-a"),
        )
        .await,
        Some(first_enrollment.clone())
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &first_target,
            Some("account-b"),
        )
        .await,
        None
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &second_target,
            Some("account-a"),
        )
        .await,
        Some(second_enrollment)
    );
}

#[tokio::test]
async fn clearing_persisted_remote_control_enrollment_removes_only_matching_entry() {
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&codex_home).await;
    let first_target = normalize_remote_control_url("http://example.com/remote/control")
        .expect("first target should parse");
    let second_target = normalize_remote_control_url("http://example.com/other/control")
        .expect("second target should parse");
    let first_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_first".to_string(),
        server_name: "first-server".to_string(),
    };
    let second_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_second".to_string(),
        server_name: "second-server".to_string(),
    };

    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &first_target,
        Some("account-a"),
        Some(&first_enrollment),
    )
    .await
    .expect("first enrollment should persist");
    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &second_target,
        Some("account-a"),
        Some(&second_enrollment),
    )
    .await
    .expect("second enrollment should persist");

    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &first_target,
        Some("account-a"),
        None,
    )
    .await
    .expect("matching enrollment should clear");

    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &first_target,
            Some("account-a"),
        )
        .await,
        None
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &second_target,
            Some("account-a"),
        )
        .await,
        Some(second_enrollment)
    );
}

#[test]
fn remote_control_client_is_alive_respects_activity_timeout() {
    let base = Instant::now();
    let client = RemoteControlClientState {
        connection_id: ConnectionId(11),
        disconnect_token: CancellationToken::new(),
        last_activity_at: base,
        last_inbound_seq_id: None,
    };

    assert!(remote_control_client_is_alive(
        &client,
        base + REMOTE_CONTROL_CLIENT_IDLE_TIMEOUT - Duration::from_millis(1)
    ));
    assert!(!remote_control_client_is_alive(
        &client,
        base + REMOTE_CONTROL_CLIENT_IDLE_TIMEOUT
    ));
}

#[tokio::test]
async fn remote_control_transport_manages_virtual_clients_and_routes_messages() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let codex_home = TempDir::new().expect("temp dir should create");
    let (transport_event_tx, mut transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let remote_handle = start_remote_control(
        remote_control_url,
        Some(remote_control_state_runtime(&codex_home).await),
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
    )
    .await
    .expect("remote control should start");
    let enroll_request = accept_http_request(&listener).await;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_test" })).await;
    let mut websocket = accept_remote_control_connection(&listener).await;

    let client_id = ClientId("client-1".to_string());
    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::Ping,
            client_id: client_id.clone(),
            seq_id: None,
            cursor: None,
        },
    )
    .await;
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "pong",
            "client_id": "client-1",
            "status": "unknown",
        })
    );

    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::ClientMessage {
                message: JSONRPCMessage::Notification(
                    codex_app_server_protocol::JSONRPCNotification {
                        method: "initialized".to_string(),
                        params: None,
                    },
                ),
            },
            client_id: client_id.clone(),
            seq_id: Some(0),
            cursor: None,
        },
    )
    .await;
    assert!(
        timeout(Duration::from_millis(100), transport_event_rx.recv())
            .await
            .is_err(),
        "non-initialize client messages should be ignored before connection creation"
    );

    let initialize_message = JSONRPCMessage::Request(codex_app_server_protocol::JSONRPCRequest {
        id: codex_app_server_protocol::RequestId::Integer(1),
        method: "initialize".to_string(),
        params: Some(json!({
            "clientInfo": {
                "name": "remote-test-client",
                "version": "0.1.0"
            }
        })),
        trace: None,
    });
    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::ClientMessage {
                message: initialize_message.clone(),
            },
            client_id: client_id.clone(),
            seq_id: Some(1),
            cursor: None,
        },
    )
    .await;

    let (connection_id, writer) = match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("connection open should arrive in time")
        .expect("connection open should exist")
    {
        TransportEvent::ConnectionOpened {
            connection_id,
            writer,
            ..
        } => (connection_id, writer),
        other => panic!("expected connection open event, got {other:?}"),
    };

    match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("initialize message should arrive in time")
        .expect("initialize message should exist")
    {
        TransportEvent::IncomingMessage {
            connection_id: incoming_connection_id,
            message,
        } => {
            assert_eq!(incoming_connection_id, connection_id);
            assert_eq!(message, initialize_message);
        }
        other => panic!("expected initialize incoming message, got {other:?}"),
    }

    let followup_message =
        JSONRPCMessage::Notification(codex_app_server_protocol::JSONRPCNotification {
            method: "initialized".to_string(),
            params: None,
        });
    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::ClientMessage {
                message: followup_message.clone(),
            },
            client_id: client_id.clone(),
            seq_id: Some(2),
            cursor: None,
        },
    )
    .await;
    match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("followup message should arrive in time")
        .expect("followup message should exist")
    {
        TransportEvent::IncomingMessage {
            connection_id: incoming_connection_id,
            message,
        } => {
            assert_eq!(incoming_connection_id, connection_id);
            assert_eq!(message, followup_message);
        }
        other => panic!("expected followup incoming message, got {other:?}"),
    }

    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::Ping,
            client_id: client_id.clone(),
            seq_id: None,
            cursor: None,
        },
    )
    .await;
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "pong",
            "client_id": "client-1",
            "status": "active",
        })
    );

    writer
        .send(QueuedOutgoingMessage::new(
            OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "test".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
        ))
        .await
        .expect("remote writer should accept outgoing message");
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "server_message",
            "client_id": "client-1",
            "seq_id": 0,
            "message": {
                "method": "configWarning",
                "params": {
                    "summary": "test",
                    "details": null,
                }
            }
        })
    );

    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::ClientClosed,
            client_id: client_id.clone(),
            seq_id: None,
            cursor: None,
        },
    )
    .await;
    match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("connection close should arrive in time")
        .expect("connection close should exist")
    {
        TransportEvent::ConnectionClosed {
            connection_id: closed_connection_id,
        } => {
            assert_eq!(closed_connection_id, connection_id);
        }
        other => panic!("expected connection close event, got {other:?}"),
    }

    send_client_event(
        &mut websocket,
        ClientEnvelope {
            event: ClientEvent::Ping,
            client_id,
            seq_id: None,
            cursor: None,
        },
    )
    .await;
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "pong",
            "client_id": "client-1",
            "status": "unknown",
        })
    );

    shutdown_token.cancel();
    let _ = remote_handle.await;
}

#[tokio::test]
async fn remote_control_transport_reconnects_after_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let codex_home = TempDir::new().expect("temp dir should create");
    let (transport_event_tx, mut transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let remote_handle = start_remote_control(
        remote_control_url,
        Some(remote_control_state_runtime(&codex_home).await),
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
    )
    .await
    .expect("remote control should start");

    let enroll_request = accept_http_request(&listener).await;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_test" })).await;
    let mut first_websocket = accept_remote_control_connection(&listener).await;
    first_websocket
        .close(None)
        .await
        .expect("first websocket should close");
    drop(first_websocket);

    let mut second_websocket = accept_remote_control_connection(&listener).await;
    send_client_event(
        &mut second_websocket,
        ClientEnvelope {
            event: ClientEvent::ClientMessage {
                message: JSONRPCMessage::Request(codex_app_server_protocol::JSONRPCRequest {
                    id: codex_app_server_protocol::RequestId::Integer(2),
                    method: "initialize".to_string(),
                    params: Some(json!({
                        "clientInfo": {
                            "name": "remote-test-client",
                            "version": "0.1.0"
                        }
                    })),
                    trace: None,
                }),
            },
            client_id: ClientId("client-2".to_string()),
            seq_id: Some(0),
            cursor: None,
        },
    )
    .await;

    match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("reconnected initialize should arrive in time")
        .expect("reconnected initialize should exist")
    {
        TransportEvent::ConnectionOpened { .. } => {}
        other => panic!("expected connection open after reconnect, got {other:?}"),
    }

    shutdown_token.cancel();
    let _ = remote_handle.await;
}

#[tokio::test]
async fn remote_control_http_mode_enrolls_before_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let codex_home = TempDir::new().expect("temp dir should create");
    let (transport_event_tx, mut transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let expected_server_name = gethostname().to_string_lossy().trim().to_string();
    let shutdown_token = CancellationToken::new();
    let remote_handle = start_remote_control(
        remote_control_url,
        Some(remote_control_state_runtime(&codex_home).await),
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
    )
    .await
    .expect("remote control should start");

    let enroll_request = accept_http_request(&listener).await;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"
    );
    assert_eq!(
        enroll_request.headers.get("authorization"),
        Some(&"Bearer Access Token".to_string())
    );
    assert_eq!(
        enroll_request.headers.get(REMOTE_CONTROL_ACCOUNT_ID_HEADER),
        Some(&"account_id".to_string())
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&enroll_request.body)
            .expect("enroll body should deserialize"),
        json!({
            "name": expected_server_name,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "app_server_version": env!("CARGO_PKG_VERSION"),
        })
    );
    respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_test" })).await;

    let (handshake_request, mut websocket) =
        accept_remote_control_backend_connection(&listener).await;
    assert_eq!(
        handshake_request.path,
        "/backend-api/wham/remote/control/server"
    );
    assert_eq!(
        handshake_request.headers.get("authorization"),
        Some(&"Bearer Access Token".to_string())
    );
    assert_eq!(
        handshake_request
            .headers
            .get(REMOTE_CONTROL_ACCOUNT_ID_HEADER),
        Some(&"account_id".to_string())
    );
    assert_eq!(
        handshake_request.headers.get("x-codex-server-id"),
        Some(&"srv_e_test".to_string())
    );
    assert_eq!(
        handshake_request.headers.get("x-codex-name"),
        Some(&base64::engine::general_purpose::STANDARD.encode(&expected_server_name))
    );
    assert_eq!(
        handshake_request.headers.get("x-codex-protocol-version"),
        Some(&REMOTE_CONTROL_PROTOCOL_VERSION.to_string())
    );

    let backend_client_id = ClientId("backend-test-client".to_string());
    let writer = {
        let initialize_message =
            JSONRPCMessage::Request(codex_app_server_protocol::JSONRPCRequest {
                id: codex_app_server_protocol::RequestId::Integer(11),
                method: "initialize".to_string(),
                params: Some(json!({
                    "clientInfo": {
                        "name": "remote-backend-client",
                        "version": "0.1.0"
                    }
                })),
                trace: None,
            });
        send_client_event(
            &mut websocket,
            ClientEnvelope {
                event: ClientEvent::ClientMessage {
                    message: initialize_message.clone(),
                },
                client_id: backend_client_id.clone(),
                seq_id: Some(0),
                cursor: None,
            },
        )
        .await;

        let (connection_id, writer) =
            match timeout(Duration::from_secs(5), transport_event_rx.recv())
                .await
                .expect("connection open should arrive in time")
                .expect("connection open should exist")
            {
                TransportEvent::ConnectionOpened {
                    connection_id,
                    writer,
                    ..
                } => (connection_id, writer),
                other => panic!("expected connection open event, got {other:?}"),
            };

        match timeout(Duration::from_secs(5), transport_event_rx.recv())
            .await
            .expect("initialize message should arrive in time")
            .expect("initialize message should exist")
        {
            TransportEvent::IncomingMessage {
                connection_id: incoming_connection_id,
                message,
            } => {
                assert_eq!(incoming_connection_id, connection_id);
                assert_eq!(message, initialize_message);
            }
            other => panic!("expected initialize incoming message, got {other:?}"),
        }
        writer
    };

    writer
        .send(QueuedOutgoingMessage::new(OutgoingMessage::Response(
            crate::outgoing_message::OutgoingResponse {
                id: codex_app_server_protocol::RequestId::Integer(11),
                result: json!({
                    "userAgent": "codex-test-agent"
                }),
            },
        )))
        .await
        .expect("remote writer should accept initialize response");
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "server_message",
            "client_id": backend_client_id.0.clone(),
            "seq_id": 0,
            "message": {
                "id": 11,
                "result": {
                    "userAgent": "codex-test-agent",
                }
            }
        })
    );

    writer
        .send(QueuedOutgoingMessage::new(
            OutgoingMessage::AppServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "backend".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
        ))
        .await
        .expect("remote writer should accept outgoing message");
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "server_message",
            "client_id": backend_client_id.0.clone(),
            "seq_id": 1,
            "message": {
                "method": "configWarning",
                "params": {
                    "summary": "backend",
                    "details": null,
                }
            }
        })
    );

    shutdown_token.cancel();
    let _ = remote_handle.await;
}

#[tokio::test]
async fn enroll_remote_control_server_parse_failure_includes_response_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let enroll_url = remote_control_target.enroll_url.clone();
    let response_body = json!({
        "error": "not enrolled",
    });
    let expected_body = response_body.to_string();
    let server_task = tokio::spawn(async move {
        let enroll_request = accept_http_request(&listener).await;
        respond_with_json(enroll_request.stream, response_body).await;
    });

    let err = enroll_remote_control_server(
        &remote_control_target,
        &RemoteControlConnectionAuth {
            bearer_token: "Access Token".to_string(),
            account_id: Some("account_id".to_string()),
        },
    )
    .await
    .expect_err("invalid response should fail to parse");

    server_task.await.expect("server task should succeed");
    assert_eq!(
        err.to_string(),
        format!(
            "failed to parse remote control enrollment response from `{enroll_url}`: HTTP 200 OK, headers: {{connection: close, content-length: 24, content-type: application/json}}, body: {expected_body}, decode error: missing field `server_id` at line 1 column {}",
            expected_body.len()
        )
    );
}

#[tokio::test]
async fn remote_control_http_mode_reuses_persisted_enrollment_before_reenrolling() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&codex_home).await;
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let persisted_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_persisted".to_string(),
        server_name: "persisted-server".to_string(),
    };
    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &remote_control_target,
        Some("account_id"),
        Some(&persisted_enrollment),
    )
    .await
    .expect("persisted enrollment should save");

    let (transport_event_tx, _transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let remote_handle = start_remote_control(
        remote_control_url,
        Some(state_db.clone()),
        remote_control_auth_manager_with_home(&codex_home),
        transport_event_tx,
        shutdown_token.clone(),
    )
    .await
    .expect("remote control should start");

    let (handshake_request, _websocket) = accept_remote_control_backend_connection(&listener).await;
    assert_eq!(
        handshake_request.path,
        "/backend-api/wham/remote/control/server"
    );
    assert_eq!(
        handshake_request.headers.get("x-codex-server-id"),
        Some(&persisted_enrollment.server_id)
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &remote_control_target,
            Some("account_id"),
        )
        .await,
        Some(persisted_enrollment)
    );

    shutdown_token.cancel();
    let _ = remote_handle.await;
}

#[tokio::test]
async fn remote_control_http_mode_clears_stale_persisted_enrollment_after_404() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&codex_home).await;
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let expected_server_name = gethostname().to_string_lossy().trim().to_string();
    let stale_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_stale".to_string(),
        server_name: "stale-server".to_string(),
    };
    let refreshed_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_refreshed".to_string(),
        server_name: expected_server_name,
    };
    update_persisted_remote_control_enrollment(
        Some(state_db.as_ref()),
        &remote_control_target,
        Some("account_id"),
        Some(&stale_enrollment),
    )
    .await
    .expect("stale enrollment should save");

    let (transport_event_tx, _transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let remote_handle = start_remote_control(
        remote_control_url,
        Some(state_db.clone()),
        remote_control_auth_manager_with_home(&codex_home),
        transport_event_tx,
        shutdown_token.clone(),
    )
    .await
    .expect("remote control should start");

    let websocket_request = accept_http_request(&listener).await;
    assert_eq!(
        websocket_request.request_line,
        "GET /backend-api/wham/remote/control/server HTTP/1.1"
    );
    assert_eq!(
        websocket_request.headers.get("x-codex-server-id"),
        Some(&stale_enrollment.server_id)
    );
    respond_with_status(websocket_request.stream, "404 Not Found", "").await;

    let enroll_request = accept_http_request(&listener).await;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(
        enroll_request.stream,
        json!({ "server_id": refreshed_enrollment.server_id }),
    )
    .await;

    let (handshake_request, _websocket) = accept_remote_control_backend_connection(&listener).await;
    assert_eq!(
        handshake_request.headers.get("x-codex-server-id"),
        Some(&refreshed_enrollment.server_id)
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            Some(state_db.as_ref()),
            &remote_control_target,
            Some("account_id"),
        )
        .await,
        Some(refreshed_enrollment)
    );

    shutdown_token.cancel();
    let _ = remote_handle.await;
}

#[derive(Debug)]
struct CapturedHttpRequest {
    stream: TcpStream,
    request_line: String,
    headers: BTreeMap<String, String>,
    body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedWebSocketRequest {
    path: String,
    headers: BTreeMap<String, String>,
}

async fn accept_remote_control_connection(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("remote control should connect in time")
        .expect("listener accept should succeed");
    accept_async(stream)
        .await
        .expect("websocket handshake should succeed")
}

async fn accept_http_request(listener: &TcpListener) -> CapturedHttpRequest {
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

async fn respond_with_json(mut stream: TcpStream, body: serde_json::Value) {
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

async fn respond_with_status(stream: TcpStream, status: &str, body: &str) {
    respond_with_status_and_headers(stream, status, &[], body).await;
}

async fn respond_with_status_and_headers(
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

async fn accept_remote_control_backend_connection(
    listener: &TcpListener,
) -> (CapturedWebSocketRequest, WebSocketStream<TcpStream>) {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("websocket request should arrive in time")
        .expect("listener accept should succeed");
    let captured_request = Arc::new(std::sync::Mutex::new(None::<CapturedWebSocketRequest>));
    let captured_request_for_callback = captured_request.clone();
    let websocket = accept_hdr_async(
        stream,
        move |request: &tungstenite::handshake::server::Request,
              response: tungstenite::handshake::server::Response| {
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

async fn send_client_event(
    websocket: &mut WebSocketStream<TcpStream>,
    client_envelope: ClientEnvelope,
) {
    let payload = serde_json::to_string(&client_envelope).expect("client event should serialize");
    websocket
        .send(tungstenite::Message::Text(payload.into()))
        .await
        .expect("client event should send");
}

async fn read_server_event(websocket: &mut WebSocketStream<TcpStream>) -> serde_json::Value {
    loop {
        let frame = timeout(Duration::from_secs(5), websocket.next())
            .await
            .expect("server event should arrive in time")
            .expect("websocket should stay open")
            .expect("websocket frame should be readable");
        match frame {
            tungstenite::Message::Text(text) => {
                return serde_json::from_str(text.as_ref())
                    .expect("server event should deserialize");
            }
            tungstenite::Message::Ping(payload) => {
                websocket
                    .send(tungstenite::Message::Pong(payload))
                    .await
                    .expect("websocket pong should send");
            }
            tungstenite::Message::Pong(_) => {}
            tungstenite::Message::Close(frame) => {
                panic!("unexpected websocket close frame: {frame:?}");
            }
            tungstenite::Message::Binary(_) => {
                panic!("unexpected binary websocket frame");
            }
            tungstenite::Message::Frame(_) => {}
        }
    }
}
