use super::super::ClientEvent;
use super::super::ClientId;
use super::super::REMOTE_CONTROL_REQUEST_ID_HEADER;
use super::super::entrollment_manager::EnrollmentManager;
use super::super::normalize_remote_control_url;
use super::super::start_remote_control;
use super::super::test_support::accept_http_request;
use super::super::test_support::accept_remote_control_backend_connection;
use super::super::test_support::read_server_event;
use super::super::test_support::remote_control_auth_manager;
use super::super::test_support::respond_with_json;
use super::super::test_support::respond_with_status_and_headers;
use super::super::test_support::send_client_event;
use super::*;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingNotification;
use crate::transport::AppServerTransport;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::ConnectionIdAllocator;
use crate::transport::TransportEvent;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn connect_remote_control_websocket_captures_handshake_request_id() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/api/codex",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let accept_task = tokio::spawn(async move {
        let enroll_request = accept_http_request(&listener).await;
        assert_eq!(
            enroll_request.request_line,
            "POST /api/codex/remote/control/server/enroll HTTP/1.1"
        );
        assert_eq!(
            enroll_request.headers.get("authorization"),
            Some(&"Bearer Access Token".to_string())
        );
        assert_eq!(
            enroll_request.headers.get("chatgpt-account-id"),
            Some(&"account_id".to_string())
        );
        let enroll_body = serde_json::from_str::<serde_json::Value>(&enroll_request.body)
            .expect("enroll body should deserialize");
        assert_eq!(enroll_body["os"], json!(std::env::consts::OS));
        assert_eq!(enroll_body["arch"], json!(std::env::consts::ARCH));
        assert_eq!(
            enroll_body["app_server_version"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        assert!(enroll_body["name"].is_string());
        respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_test" })).await;
        accept_remote_control_backend_connection(&listener, Some("req-control-123")).await
    });
    let codex_home = TempDir::new().expect("temp dir should create");
    let auth_manager = remote_control_auth_manager();
    let mut enrollment_manager = EnrollmentManager::new(
        remote_control_target.clone(),
        codex_home.path().to_path_buf(),
    );

    let connection = connect_remote_control_websocket(
        auth_manager.as_ref(),
        &remote_control_target,
        &mut enrollment_manager,
    )
    .await
    .expect("websocket connection should succeed");

    assert_eq!(connection.request_id.as_deref(), Some("req-control-123"));

    let (_request, server_websocket) = accept_task.await.expect("accept task should succeed");
    drop(server_websocket);
    drop(connection.websocket_stream);
}

#[tokio::test]
async fn connect_remote_control_websocket_includes_http_error_details() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/api/codex",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let websocket_url = remote_control_target.websocket_url.clone();
    let expected_error = format!(
        "failed to connect app-server remote control websocket `{websocket_url}`: HTTP error: 503 Service Unavailable, request id: req-503, cf-ray: ray-503, body: upstream unavailable"
    );
    let server_task = tokio::spawn(async move {
        let enroll_request = accept_http_request(&listener).await;
        assert_eq!(
            enroll_request.request_line,
            "POST /api/codex/remote/control/server/enroll HTTP/1.1"
        );
        assert_eq!(
            enroll_request.headers.get("authorization"),
            Some(&"Bearer Access Token".to_string())
        );
        assert_eq!(
            enroll_request.headers.get("chatgpt-account-id"),
            Some(&"account_id".to_string())
        );
        let enroll_body = serde_json::from_str::<serde_json::Value>(&enroll_request.body)
            .expect("enroll body should deserialize");
        assert_eq!(enroll_body["os"], json!(std::env::consts::OS));
        assert_eq!(enroll_body["arch"], json!(std::env::consts::ARCH));
        assert_eq!(
            enroll_body["app_server_version"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        assert!(enroll_body["name"].is_string());
        respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_test" })).await;

        let request = accept_http_request(&listener).await;
        assert_eq!(
            request.request_line,
            "GET /api/codex/remote/control/server HTTP/1.1"
        );
        respond_with_status_and_headers(
            request.stream,
            "503 Service Unavailable",
            &[
                (REMOTE_CONTROL_REQUEST_ID_HEADER, "req-503"),
                (REMOTE_CONTROL_CF_RAY_HEADER, "ray-503"),
            ],
            "upstream unavailable",
        )
        .await;
    });
    let codex_home = TempDir::new().expect("temp dir should create");
    let auth_manager = remote_control_auth_manager();
    let mut enrollment_manager = EnrollmentManager::new(
        remote_control_target.clone(),
        codex_home.path().to_path_buf(),
    );

    let err = match connect_remote_control_websocket(
        auth_manager.as_ref(),
        &remote_control_target,
        &mut enrollment_manager,
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
async fn remote_control_transport_reconnects_and_keeps_virtual_client_writer_alive() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/api/codex",
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
        "POST /api/codex/remote/control/server/enroll HTTP/1.1"
    );
    respond_with_json(enroll_request.stream, json!({ "server_id": "srv_e_test" })).await;

    let (_first_request, mut first_websocket) =
        accept_remote_control_backend_connection(&listener, None).await;
    let client_id = ClientId("client-2".to_string());
    let initialize_message = JSONRPCMessage::Request(JSONRPCRequest {
        id: RequestId::Integer(2),
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
        &mut first_websocket,
        ClientEvent::ClientMessage {
            client_id: client_id.clone(),
            message: initialize_message.clone(),
        },
    )
    .await;
    let writer = match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("connection open should arrive in time")
        .expect("connection open should exist")
    {
        TransportEvent::ConnectionOpened {
            writer,
            transport_kind,
            ..
        } => {
            assert_eq!(transport_kind, AppServerTransport::RemoteControlled);
            writer
        }
        other => panic!("expected connection open before reconnect, got {other:?}"),
    };
    match timeout(Duration::from_secs(5), transport_event_rx.recv())
        .await
        .expect("initialize message should arrive in time")
        .expect("initialize message should exist")
    {
        TransportEvent::IncomingMessage { message, .. } => {
            assert_eq!(message, initialize_message);
        }
        other => panic!("expected initialize incoming message, got {other:?}"),
    }
    first_websocket
        .close(None)
        .await
        .expect("first websocket should close");
    drop(first_websocket);

    let (_second_request, mut second_websocket) =
        accept_remote_control_backend_connection(&listener, None).await;
    writer
        .send(OutgoingMessage::Notification(OutgoingNotification {
            method: "codex/event/reconnected".to_string(),
            params: Some(json!({ "replayed": true })),
        }))
        .await
        .expect("outgoing message should send after reconnect");
    assert_eq!(
        read_server_event(&mut second_websocket).await,
        json!({
            "type": "server_message",
            "client_id": client_id.0,
            "message": {
                "method": "codex/event/reconnected",
                "params": {
                    "replayed": true,
                }
            }
        })
    );

    shutdown_token.cancel();
    let _ = remote_handle.await;
}
