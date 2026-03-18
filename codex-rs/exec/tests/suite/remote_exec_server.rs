#![cfg(not(target_os = "windows"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use core_test_support::responses;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::process::Command;

fn extract_output_text(item: &serde_json::Value) -> String {
    item.get("output")
        .and_then(|value| match value {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Object(obj) => obj
                .get("content")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .expect("function call output should include text content")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exec_cli_can_route_remote_exec_and_read_file_through_exec_server() -> anyhow::Result<()> {
    let test = test_codex_exec();
    let external_websocket_url = std::env::var("CODEX_EXEC_SERVER_TEST_WS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let external_remote_root = std::env::var("CODEX_EXEC_SERVER_TEST_REMOTE_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let websocket_url = if let Some(websocket_url) = external_websocket_url {
        websocket_url
    } else {
        let websocket_listener = TcpListener::bind("127.0.0.1:0")?;
        let websocket_port = websocket_listener.local_addr()?.port();
        drop(websocket_listener);
        format!("ws://127.0.0.1:{websocket_port}")
    };

    let mut exec_server = if std::env::var("CODEX_EXEC_SERVER_TEST_WS_URL").is_ok() {
        None
    } else {
        let child = Command::new(codex_utils_cargo_bin::cargo_bin("codex-exec-server")?)
            .arg("--listen")
            .arg(&websocket_url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        tokio::time::sleep(Duration::from_millis(250)).await;
        Some(child)
    };

    let local_workspace_root = test.cwd_path().to_path_buf();
    let remote_workspace_root = external_remote_root
        .clone()
        .unwrap_or_else(|| local_workspace_root.clone());
    let seed_path = local_workspace_root.join("remote_exec_seed.txt");
    if external_remote_root.is_none() {
        std::fs::write(&seed_path, "remote-fs-seed\n")?;
    }

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
                        "cmd": if external_remote_root.is_some() {
                            "printf remote-fs-seed > remote_exec_seed.txt && printf from-remote > remote_exec_generated.txt"
                        } else {
                            "printf from-remote > remote_exec_generated.txt"
                        },
                        "yield_time_ms": 500,
                    }))?,
                ),
                responses::ev_completed("resp-1"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-2"),
                responses::ev_function_call(
                    "call-read",
                    "read_file",
                    &serde_json::to_string(&json!({
                        "file_path": seed_path,
                    }))?,
                ),
                responses::ev_completed("resp-2"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-3"),
                responses::ev_function_call(
                    "call-list",
                    "list_dir",
                    &serde_json::to_string(&json!({
                        "dir_path": local_workspace_root,
                        "offset": 1,
                        "limit": 20,
                        "depth": 1,
                    }))?,
                ),
                responses::ev_completed("resp-3"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-4"),
                responses::ev_assistant_message("msg-1", "done"),
                responses::ev_completed("resp-4"),
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
        .arg("zsh_path=\"/usr/bin/zsh\"")
        .arg("-c")
        .arg("experimental_unified_exec_use_exec_server=true")
        .arg("-c")
        .arg(format!(
            "experimental_unified_exec_exec_server_websocket_url={}",
            serde_json::to_string(&websocket_url)?
        ))
        .arg("-c")
        .arg(format!(
            "experimental_unified_exec_exec_server_workspace_root={}",
            serde_json::to_string(&remote_workspace_root)?
        ))
        .arg("-c")
        .arg("experimental_supported_tools=[\"read_file\",\"list_dir\"]")
        .arg("run remote exec-server tools")
        .assert()
        .success();

    if external_remote_root.is_none() {
        let generated_path = test.cwd_path().join("remote_exec_generated.txt");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && !generated_path.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(std::fs::read_to_string(&generated_path)?, "from-remote");
    }

    let requests = response_mock.requests();
    let read_output = extract_output_text(&requests[2].function_call_output("call-read"));
    assert!(
        read_output.contains("remote-fs-seed"),
        "expected read_file tool output to include remote file contents, got {read_output:?}"
    );
    let list_output = extract_output_text(&requests[3].function_call_output("call-list"));
    assert!(
        list_output.contains("remote_exec_seed.txt"),
        "expected list_dir output to include remote_exec_seed.txt, got {list_output:?}"
    );
    assert!(
        list_output.contains("remote_exec_generated.txt"),
        "expected list_dir output to include remote_exec_generated.txt, got {list_output:?}"
    );

    if let Some(exec_server) = exec_server.as_mut() {
        exec_server.start_kill()?;
        let _ = exec_server.wait().await;
    }
    Ok(())
}
