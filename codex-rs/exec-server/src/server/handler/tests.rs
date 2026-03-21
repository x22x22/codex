use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::ExecServerHandler;
use crate::protocol::EnvironmentCapabilitiesParams;
use crate::protocol::EnvironmentGetParams;
use crate::protocol::EnvironmentListParams;
use crate::protocol::ExecParams;
use crate::protocol::ExecResizeParams;
use crate::protocol::ExecTerminalSize;
use crate::protocol::ExecWaitParams;
use crate::protocol::InitializeResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::rpc::RpcNotificationSender;

fn exec_params(process_id: &str) -> ExecParams {
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
    ExecParams {
        process_id: process_id.to_string(),
        argv: vec![
            "/bin/bash".to_string(),
            "-lc".to_string(),
            "sleep 0.1".to_string(),
        ],
        cwd: std::env::current_dir().expect("cwd"),
        env,
        tty: false,
        arg0: None,
    }
}

fn tty_exec_params(process_id: &str) -> ExecParams {
    let mut params = exec_params(process_id);
    params.tty = true;
    params
}

async fn initialized_handler() -> Arc<ExecServerHandler> {
    let (outgoing_tx, _outgoing_rx) = mpsc::channel(16);
    let handler = Arc::new(ExecServerHandler::new(RpcNotificationSender::new(
        outgoing_tx,
    )));
    assert_eq!(
        handler.initialize().expect("initialize"),
        InitializeResponse {}
    );
    handler.initialized().expect("initialized");
    handler
}

#[tokio::test]
async fn duplicate_process_ids_allow_only_one_successful_start() {
    let handler = initialized_handler().await;
    let first_handler = Arc::clone(&handler);
    let second_handler = Arc::clone(&handler);

    let (first, second) = tokio::join!(
        first_handler.exec(exec_params("proc-1")),
        second_handler.exec(exec_params("proc-1")),
    );

    let (successes, failures): (Vec<_>, Vec<_>) =
        [first, second].into_iter().partition(Result::is_ok);
    assert_eq!(successes.len(), 1);
    assert_eq!(failures.len(), 1);

    let error = failures
        .into_iter()
        .next()
        .expect("one failed request")
        .expect_err("expected duplicate process error");
    assert_eq!(error.code, -32600);
    assert_eq!(error.message, "process proc-1 already exists");

    tokio::time::sleep(Duration::from_millis(150)).await;
    handler.shutdown().await;
}

#[tokio::test]
async fn terminate_reports_false_after_process_exit() {
    let handler = initialized_handler().await;
    handler
        .exec(exec_params("proc-1"))
        .await
        .expect("start process");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let response = handler
            .terminate(TerminateParams {
                process_id: "proc-1".to_string(),
            })
            .await
            .expect("terminate response");
        if response == (TerminateResponse { running: false }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "process should have exited within 1s"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    handler.shutdown().await;
}

#[tokio::test]
async fn environment_list_returns_local_environment() {
    let handler = initialized_handler().await;

    let response = handler
        .environment_list(EnvironmentListParams {})
        .await
        .expect("environment list");

    assert_eq!(response.environments.len(), 1);
    assert_eq!(response.environments[0].environment_id, "local");
    assert_eq!(response.environments[0].experimental_exec_server_url, None);
    assert_eq!(
        response.environments[0].capabilities,
        crate::protocol::EnvironmentCapabilities::default()
    );

    handler.shutdown().await;
}

#[tokio::test]
async fn environment_get_returns_local_environment() {
    let handler = initialized_handler().await;

    let response = handler
        .environment_get(EnvironmentGetParams {
            environment_id: "local".to_string(),
        })
        .await
        .expect("environment get");

    assert_eq!(response.environment.environment_id, "local");

    handler.shutdown().await;
}

#[tokio::test]
async fn environment_capabilities_returns_local_capabilities() {
    let handler = initialized_handler().await;

    let response = handler
        .environment_capabilities(EnvironmentCapabilitiesParams {
            environment_id: "local".to_string(),
        })
        .await
        .expect("environment capabilities");

    assert_eq!(response.environment_id, "local");
    assert_eq!(
        response.capabilities,
        crate::protocol::EnvironmentCapabilities::default()
    );

    handler.shutdown().await;
}

#[tokio::test]
async fn resize_and_wait_are_routed_for_running_processes() {
    let handler = initialized_handler().await;
    handler
        .exec(tty_exec_params("proc-tty"))
        .await
        .expect("start tty process");

    handler
        .resize(ExecResizeParams {
            process_id: "proc-tty".to_string(),
            size: ExecTerminalSize { rows: 24, cols: 80 },
        })
        .await
        .expect("resize tty process");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let response = handler
            .wait(ExecWaitParams {
                process_id: "proc-tty".to_string(),
                wait_ms: Some(50),
            })
            .await
            .expect("wait response");
        if response.exited {
            assert_eq!(response.exit_code, Some(0));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "process should have exited within 1s"
        );
    }

    handler.shutdown().await;
}
