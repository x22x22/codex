use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::Value as JsonValue;
use tempfile::TempDir;

fn write_json_line(stdin: &mut std::process::ChildStdin, value: JsonValue) -> Result<()> {
    writeln!(stdin, "{}", serde_json::to_string(&value)?)?;
    stdin.flush()?;
    Ok(())
}

fn read_json_line(stdout: &mut BufReader<std::process::ChildStdout>) -> Result<JsonValue> {
    let mut line = String::new();
    stdout.read_line(&mut line)?;
    serde_json::from_str(&line).context("parse ACP JSON-RPC response")
}

fn read_response(stdout: &mut BufReader<std::process::ChildStdout>, id: i64) -> Result<JsonValue> {
    loop {
        let message = read_json_line(stdout)?;
        if message["id"] == id {
            return Ok(message);
        }
    }
}

fn read_session_update_kinds(
    stdout: &mut BufReader<std::process::ChildStdout>,
    count: usize,
) -> Result<Vec<String>> {
    let mut kinds = Vec::new();
    while kinds.len() < count {
        let message = read_json_line(stdout)?;
        if message["method"] == "session/update" {
            kinds.push(
                message["params"]["update"]["sessionUpdate"]
                    .as_str()
                    .context("session update kind missing")?
                    .to_string(),
            );
        }
    }
    Ok(kinds)
}

#[test]
fn acp_server_supports_session_setup_and_mode_switching() -> Result<()> {
    let codex_home = TempDir::new()?;
    let workspace = TempDir::new()?;
    let mut child = Command::new(codex_utils_cargo_bin::cargo_bin("codex-acp-server")?)
        .env("CODEX_HOME", codex_home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn codex-acp-server")?;

    let mut stdin = child.stdin.take().context("take stdin")?;
    let stdout = child.stdout.take().context("take stdout")?;
    let mut stdout = BufReader::new(stdout);

    write_json_line(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "acp-test",
                    "version": "0.0.0"
                }
            }
        }),
    )?;

    let initialize = read_response(&mut stdout, 1)?;
    assert_eq!(initialize["id"], 1);
    assert_eq!(initialize["result"]["protocolVersion"], 1);
    assert_eq!(
        initialize["result"]["agentCapabilities"]["promptCapabilities"]["embeddedContext"],
        JsonValue::Bool(true)
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["promptCapabilities"]["image"],
        JsonValue::Bool(true)
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["mcpCapabilities"]["acp"],
        JsonValue::Bool(true)
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["sessionCapabilities"]["list"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["sessionCapabilities"]["fork"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["sessionCapabilities"]["resume"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["sessionCapabilities"]["close"],
        serde_json::json!({})
    );

    write_json_line(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": {
                "cwd": workspace.path(),
                "mcpServers": []
            }
        }),
    )?;

    let session_new = read_response(&mut stdout, 2)?;
    assert_eq!(session_new["id"], 2);
    let session_id = session_new["result"]["sessionId"]
        .as_str()
        .context("sessionId missing")?
        .to_string();
    let modes = session_new["result"]["modes"]["availableModes"]
        .as_array()
        .context("availableModes missing")?;
    assert!(!modes.is_empty(), "expected at least one ACP mode");
    let first_mode_id = modes[0]["id"]
        .as_str()
        .context("mode id missing")?
        .to_string();
    assert_eq!(
        session_new["result"]["configOptions"][0]["id"],
        JsonValue::String("mode".to_string())
    );

    write_json_line(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/set_mode",
            "params": {
                "sessionId": session_id,
                "modeId": first_mode_id
            }
        }),
    )?;

    let set_mode = read_response(&mut stdout, 3)?;
    assert_eq!(set_mode["id"], 3);
    assert_eq!(set_mode["result"], serde_json::json!({}));

    let updates = read_session_update_kinds(&mut stdout, 2)?;
    assert!(updates.contains(&"current_mode_update".to_string()));
    assert!(updates.contains(&"config_options_update".to_string()));

    drop(stdin);
    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}
