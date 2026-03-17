#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_exec_server::ExecParams;
use codex_exec_server::ExecServerClient;
use codex_exec_server::ExecServerClientConnectOptions;
use codex_exec_server::ExecServerLaunchCommand;
use codex_exec_server::InitializeParams;
use codex_exec_server::InitializeResponse;
use codex_exec_server::RemoteExecServerConnectArgs;
use codex_exec_server::spawn_local_exec_server;
use codex_utils_cargo_bin::cargo_bin;
use pretty_assertions::assert_eq;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::broadcast;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_server_accepts_initialize_over_stdio() -> anyhow::Result<()> {
    let binary = cargo_bin("codex-exec-server")?;
    let mut child = Command::new(binary);
    child.stdin(Stdio::piped());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::inherit());
    let mut child = child.spawn()?;

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdout = BufReader::new(stdout).lines();

    let initialize = JSONRPCMessage::Request(JSONRPCRequest {
        id: RequestId::Integer(1),
        method: "initialize".to_string(),
        params: Some(serde_json::to_value(InitializeParams {
            client_name: "exec-server-test".to_string(),
        })?),
        trace: None,
    });
    stdin
        .write_all(format!("{}\n", serde_json::to_string(&initialize)?).as_bytes())
        .await?;

    let response_line = timeout(Duration::from_secs(5), stdout.next_line()).await??;
    let response_line = response_line.expect("response line");
    let response: JSONRPCMessage = serde_json::from_str(&response_line)?;
    let JSONRPCMessage::Response(JSONRPCResponse { id, result }) = response else {
        panic!("expected initialize response");
    };
    assert_eq!(id, RequestId::Integer(1));
    let initialize_response: InitializeResponse = serde_json::from_value(result)?;
    assert_eq!(initialize_response.protocol_version, "exec-server.v0");

    let initialized = JSONRPCMessage::Notification(JSONRPCNotification {
        method: "initialized".to_string(),
        params: Some(serde_json::json!({})),
    });
    stdin
        .write_all(format!("{}\n", serde_json::to_string(&initialized)?).as_bytes())
        .await?;

    child.start_kill()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_server_client_streams_output_and_accepts_writes() -> anyhow::Result<()> {
    let mut env = std::collections::HashMap::new();
    if let Some(path) = std::env::var_os("PATH") {
        env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    }

    let server = spawn_local_exec_server(
        ExecServerLaunchCommand {
            program: cargo_bin("codex-exec-server")?,
            args: Vec::new(),
        },
        ExecServerClientConnectOptions {
            client_name: "exec-server-test".to_string(),
        },
    )
    .await?;

    let process = server
        .client()
        .start_process(ExecParams {
            process_id: "2001".to_string(),
            argv: vec![
                "bash".to_string(),
                "-lc".to_string(),
                "printf 'ready\\n'; while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done"
                    .to_string(),
            ],
            cwd: std::env::current_dir()?,
            env,
            tty: true,
            output_bytes_cap: 4096,
            arg0: None,
        })
        .await?;

    let mut output = process.output_receiver();
    assert!(
        recv_until_contains(&mut output, "ready")
            .await?
            .contains("ready"),
        "expected initial ready output"
    );

    process
        .writer_sender()
        .send(b"hello\n".to_vec())
        .await
        .expect("write should succeed");

    assert!(
        recv_until_contains(&mut output, "echo:hello")
            .await?
            .contains("echo:hello"),
        "expected echoed output"
    );

    process.terminate();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_server_client_connects_over_websocket() -> anyhow::Result<()> {
    let mut env = std::collections::HashMap::new();
    if let Some(path) = std::env::var_os("PATH") {
        env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    }

    let binary = cargo_bin("codex-exec-server")?;
    let mut child = Command::new(binary);
    child.args(["--listen", "ws://127.0.0.1:0"]);
    child.stdin(Stdio::null());
    child.stdout(Stdio::null());
    child.stderr(Stdio::piped());
    let mut child = child.spawn()?;
    let stderr = child.stderr.take().expect("stderr");
    let mut stderr_lines = BufReader::new(stderr).lines();
    let websocket_url = read_websocket_url(&mut stderr_lines).await?;

    let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs {
        websocket_url,
        client_name: "exec-server-test".to_string(),
    })
    .await?;

    let process = client
        .start_process(ExecParams {
            process_id: "2002".to_string(),
            argv: vec![
                "bash".to_string(),
                "-lc".to_string(),
                "printf 'ready\\n'; while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done"
                    .to_string(),
            ],
            cwd: std::env::current_dir()?,
            env,
            tty: true,
            output_bytes_cap: 4096,
            arg0: None,
        })
        .await?;

    let mut output = process.output_receiver();
    assert!(
        recv_until_contains(&mut output, "ready")
            .await?
            .contains("ready"),
        "expected initial ready output"
    );

    process
        .writer_sender()
        .send(b"hello\n".to_vec())
        .await
        .expect("write should succeed");

    assert!(
        recv_until_contains(&mut output, "echo:hello")
            .await?
            .contains("echo:hello"),
        "expected echoed output"
    );

    process.terminate();
    child.start_kill()?;
    Ok(())
}

async fn read_websocket_url<R>(lines: &mut tokio::io::Lines<BufReader<R>>) -> anyhow::Result<String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let line = timeout(Duration::from_secs(5), lines.next_line()).await??;
    let line = line.context("missing websocket startup banner")?;
    let websocket_url = line
        .split_whitespace()
        .find(|part| part.starts_with("ws://"))
        .context("missing websocket URL in startup banner")?;
    Ok(websocket_url.to_string())
}

async fn recv_until_contains(
    output: &mut broadcast::Receiver<Vec<u8>>,
    needle: &str,
) -> anyhow::Result<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut collected = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let chunk = timeout(remaining, output.recv()).await??;
        collected.push_str(&String::from_utf8_lossy(&chunk));
        if collected.contains(needle) {
            return Ok(collected);
        }
    }
}
