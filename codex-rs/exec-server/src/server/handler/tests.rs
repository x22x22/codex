use std::collections::HashMap;

use pretty_assertions::assert_eq;

use super::ExecServerHandler;
use crate::protocol::ExecParams;
use crate::protocol::ExecSandboxConfig;
use crate::protocol::ExecSandboxMode;
use crate::protocol::InitializeParams;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::TerminateParams;
use crate::protocol::WriteParams;

fn exec_params(process_id: &str) -> ExecParams {
    ExecParams {
        process_id: process_id.to_string(),
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
    }
}

async fn initialized_handler() -> ExecServerHandler {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(8);
    let mut handler = ExecServerHandler::new(notification_tx, None);
    let response = handler
        .initialize(InitializeParams {
            client_name: "test".to_string(),
            auth_token: None,
        })
        .expect("initialize should succeed");
    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
    handler
        .initialized()
        .expect("initialized notification should succeed");
    handler
}

#[tokio::test]
async fn initialize_reports_protocol_version() {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(notification_tx, None);

    let response = handler
        .initialize(InitializeParams {
            client_name: "test".to_string(),
            auth_token: None,
        })
        .expect("initialize should succeed");

    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
}

#[tokio::test]
async fn exec_methods_require_initialize() {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
    let handler = ExecServerHandler::new(notification_tx, None);

    let error = handler
        .exec(exec_params("proc-1"))
        .await
        .expect_err("exec should fail before initialize");

    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "client must call initialize before using exec methods"
    );
}

#[tokio::test]
async fn exec_methods_require_initialized_notification_after_initialize() {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(notification_tx, None);
    let _ = handler
        .initialize(InitializeParams {
            client_name: "test".to_string(),
            auth_token: None,
        })
        .expect("initialize should succeed");

    let error = handler
        .exec(exec_params("proc-1"))
        .await
        .expect_err("exec should fail before initialized notification");

    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "client must send initialized before using exec methods"
    );
}

#[tokio::test]
async fn initialized_before_initialize_is_a_protocol_error() {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(notification_tx, None);

    let error = handler
        .initialized()
        .expect_err("expected protocol error for early initialized notification");

    assert_eq!(
        error,
        "received `initialized` notification before `initialize`"
    );
}

#[tokio::test]
async fn initialize_may_only_be_sent_once_per_connection() {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(notification_tx, None);
    let _ = handler
        .initialize(InitializeParams {
            client_name: "test".to_string(),
            auth_token: None,
        })
        .expect("first initialize should succeed");

    let error = handler
        .initialize(InitializeParams {
            client_name: "test".to_string(),
            auth_token: None,
        })
        .expect_err("duplicate initialize should fail");

    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "initialize may only be sent once per connection"
    );
}

#[tokio::test]
async fn initialize_rejects_invalid_auth_token() {
    let (notification_tx, _notification_rx) = tokio::sync::mpsc::channel(1);
    let mut handler = ExecServerHandler::new(notification_tx, Some("secret-token".to_string()));

    let error = handler
        .initialize(InitializeParams {
            client_name: "test".to_string(),
            auth_token: Some("wrong-token".to_string()),
        })
        .expect_err("invalid auth token should fail");

    assert_eq!(error.code, -32001);
    assert_eq!(error.message, "invalid exec-server auth token");
}

#[tokio::test]
async fn exec_rejects_host_default_sandbox_mode() {
    let handler = initialized_handler().await;

    let error = handler
        .exec(ExecParams {
            sandbox: Some(ExecSandboxConfig {
                mode: ExecSandboxMode::HostDefault,
            }),
            ..exec_params("proc-1")
        })
        .await
        .expect_err("hostDefault sandbox should be rejected");

    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        "sandbox mode `hostDefault` is not supported by exec-server yet"
    );
}

#[tokio::test]
async fn exec_rejects_duplicate_process_ids() {
    let handler = initialized_handler().await;
    let first = handler
        .exec(exec_params("proc-1"))
        .await
        .expect("first exec should succeed");
    assert_eq!(first.process_id, "proc-1");

    let error = handler
        .exec(exec_params("proc-1"))
        .await
        .expect_err("duplicate process id should fail");

    assert_eq!(error.code, -32600);
    assert_eq!(error.message, "process proc-1 already exists");

    handler.shutdown().await;
}

#[tokio::test]
async fn write_rejects_unknown_process_ids() {
    let handler = initialized_handler().await;

    let error = handler
        .write(WriteParams {
            process_id: "missing".to_string(),
            chunk: b"input".to_vec().into(),
        })
        .await
        .expect_err("writing to an unknown process should fail");

    assert_eq!(error.code, -32600);
    assert_eq!(error.message, "unknown process id missing");
}

#[tokio::test]
async fn terminate_reports_missing_processes_as_not_running() {
    let handler = initialized_handler().await;

    let response = handler
        .terminate(TerminateParams {
            process_id: "missing".to_string(),
        })
        .await
        .expect("terminate should succeed");

    assert_eq!(response.running, false);
}
