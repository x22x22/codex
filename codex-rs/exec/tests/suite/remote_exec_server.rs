#![cfg(not(target_os = "windows"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::TcpListener;
use std::process::Stdio;
use std::time::Duration;

use core_test_support::responses;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exec_cli_can_route_exec_command_through_remote_exec_server() -> anyhow::Result<()> {
    let test = test_codex_exec();

    let websocket_listener = TcpListener::bind("127.0.0.1:0")?;
    let websocket_port = websocket_listener.local_addr()?.port();
    drop(websocket_listener);
    let websocket_url = format!("ws://127.0.0.1:{websocket_port}");

    let mut exec_server = Command::new(codex_utils_cargo_bin::cargo_bin("codex-exec-server")?)
        .arg("--listen")
        .arg(&websocket_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let generated_path = test.cwd_path().join("remote_exec_generated.txt");
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_function_call(
                    "call-exec",
                    "exec_command",
                    &serde_json::to_string(&json!({
                        "cmd": "/bin/sh -lc 'printf from-remote > remote_exec_generated.txt'",
                        "yield_time_ms": 500,
                    }))?,
                ),
                responses::ev_completed("resp-1"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-2"),
                responses::ev_assistant_message("msg-1", "done"),
                responses::ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.cmd_with_server(&server)
        .arg("--skip-git-repo-check")
        .arg("-s")
        .arg("danger-full-access")
        .arg("-c")
        .arg("experimental_use_unified_exec_tool=true")
        .arg("-c")
        .arg(format!(
            "experimental_unified_exec_exec_server_websocket_url={}",
            serde_json::to_string(&websocket_url)?
        ))
        .arg("-c")
        .arg(format!(
            "experimental_unified_exec_exec_server_workspace_root={}",
            serde_json::to_string(test.cwd_path().to_string_lossy().as_ref())?
        ))
        .arg("run remote exec-server command")
        .assert()
        .success();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !generated_path.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read_to_string(&generated_path)?, "from-remote");

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);

    exec_server.start_kill()?;
    let _ = exec_server.wait().await;
    Ok(())
}
