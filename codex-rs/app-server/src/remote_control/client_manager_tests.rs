use super::super::ClientActivityState;
use super::super::ClientEvent;
use super::super::ClientId;
use super::super::start_remote_control;
use super::super::test_support::accept_http_request;
use super::super::test_support::accept_remote_control_backend_connection;
use super::super::test_support::read_server_event;
use super::super::test_support::remote_control_auth_manager;
use super::super::test_support::respond_with_json;
use super::super::test_support::send_client_event;
use super::*;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingNotification;
use crate::transport::AppServerTransport;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::ConnectionIdAllocator;
use crate::transport::TransportEvent;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn close_expired_remote_control_clients_closes_only_stale_connections() {
    let (transport_event_tx, mut transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let stale_disconnect_token = CancellationToken::new();
    let fresh_disconnect_token = CancellationToken::new();
    let stale_client_id = ClientId("stale-client".to_string());
    let fresh_client_id = ClientId("fresh-client".to_string());
    let now = tokio::time::Instant::now();
    let mut clients = HashMap::from([
        (
            stale_client_id.clone(),
            RemoteControlClientState {
                connection_id: ConnectionId(11),
                disconnect_token: stale_disconnect_token.clone(),
                last_activity_at: now - REMOTE_CONTROL_CLIENT_IDLE_TIMEOUT,
            },
        ),
        (
            fresh_client_id.clone(),
            RemoteControlClientState {
                connection_id: ConnectionId(12),
                disconnect_token: fresh_disconnect_token.clone(),
                last_activity_at: now,
            },
        ),
    ]);

    assert!(close_expired_remote_control_clients(&transport_event_tx, &mut clients).await);
    assert!(stale_disconnect_token.is_cancelled());
    assert!(!fresh_disconnect_token.is_cancelled());
    assert!(!clients.contains_key(&stale_client_id));
    assert!(clients.contains_key(&fresh_client_id));

    match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("stale client close should arrive in time")
        .expect("stale client close should exist")
    {
        TransportEvent::ConnectionClosed {
            connection_id,
            transport_kind,
        } => {
            assert_eq!(connection_id, ConnectionId(11));
            assert_eq!(transport_kind, AppServerTransport::RemoteControlled);
        }
        other => panic!("expected stale client close event, got {other:?}"),
    }
    assert!(
        timeout(Duration::from_millis(100), transport_event_rx.recv())
            .await
            .is_err(),
        "fresh clients should remain connected during stale sweep"
    );
}

#[tokio::test]
async fn remote_control_transport_manages_virtual_clients_and_routes_messages() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/backend-api/wham",
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
        codex_home.path().to_path_buf(),
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
        ConnectionIdAllocator::default(),
    )
    .await
    .expect("remote control should start");
    let enroll_request = accept_http_request(&listener).await;
    assert_eq!(
        enroll_request.request_line,
        "POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(
        enroll_request.stream,
        json!({ "server_id": "srv_e_client_manager" }),
    )
    .await;
    let (_handshake_request, mut websocket) =
        accept_remote_control_backend_connection(&listener, None).await;

    let client_id = ClientId("client-1".to_string());
    send_client_event(
        &mut websocket,
        ClientEvent::Ping {
            client_id: client_id.clone(),
            state: Some(ClientActivityState::Foreground),
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
        ClientEvent::ClientMessage {
            client_id: client_id.clone(),
            message: JSONRPCMessage::Notification(JSONRPCNotification {
                method: "initialized".to_string(),
                params: None,
            }),
        },
    )
    .await;
    assert!(
        timeout(Duration::from_millis(100), transport_event_rx.recv())
            .await
            .is_err(),
        "non-initialize client messages should be ignored before connection creation"
    );

    let initialize_message = JSONRPCMessage::Request(JSONRPCRequest {
        id: RequestId::Integer(1),
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
        ClientEvent::ClientMessage {
            client_id: client_id.clone(),
            message: initialize_message.clone(),
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
            transport_kind,
            writer,
            ..
        } => {
            assert_eq!(transport_kind, AppServerTransport::RemoteControlled);
            (connection_id, writer)
        }
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

    let followup_message = JSONRPCMessage::Notification(JSONRPCNotification {
        method: "initialized".to_string(),
        params: None,
    });
    send_client_event(
        &mut websocket,
        ClientEvent::ClientMessage {
            client_id: client_id.clone(),
            message: followup_message.clone(),
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
        ClientEvent::Ping {
            client_id: client_id.clone(),
            state: Some(ClientActivityState::Foreground),
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
        .send(OutgoingMessage::Notification(OutgoingNotification {
            method: "codex/event/test".to_string(),
            params: Some(json!({ "ok": true })),
        }))
        .await
        .expect("remote writer should accept outgoing message");
    assert_eq!(
        read_server_event(&mut websocket).await,
        json!({
            "type": "server_message",
            "client_id": "client-1",
            "message": {
                "method": "codex/event/test",
                "params": {
                    "ok": true,
                }
            }
        })
    );

    send_client_event(
        &mut websocket,
        ClientEvent::ClientClosed {
            client_id: client_id.clone(),
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
            transport_kind,
        } => {
            assert_eq!(closed_connection_id, connection_id);
            assert_eq!(transport_kind, AppServerTransport::RemoteControlled);
        }
        other => panic!("expected connection close event, got {other:?}"),
    }

    send_client_event(
        &mut websocket,
        ClientEvent::Ping {
            client_id,
            state: Some(ClientActivityState::Foreground),
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
