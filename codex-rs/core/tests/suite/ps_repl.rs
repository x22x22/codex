#![allow(clippy::expect_used, clippy::unwrap_used)]

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_core::features::Feature;
use codex_protocol::protocol::EventMsg;
use core_test_support::responses;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_custom_tool_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event_match;
use serde_json::Value;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::tempdir;
use wiremock::MockServer;

fn custom_tool_output_text_and_success(
    req: &ResponsesRequest,
    call_id: &str,
) -> (String, Option<bool>) {
    let (output, success) = req
        .custom_tool_call_output_content_and_success(call_id)
        .expect("custom tool output should be present");
    (output.unwrap_or_default(), success)
}

fn tool_names(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .expect("tools array should be present")
        .iter()
        .map(|tool| {
            tool.get("name")
                .and_then(|value| value.as_str())
                .or_else(|| tool.get("type").and_then(|value| value.as_str()))
                .expect("tool should have a name or type")
                .to_string()
        })
        .collect()
}

fn write_too_old_pwsh_script(dir: &Path) -> Result<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let path = dir.join("old-pwsh.cmd");
        fs::write(&path, "@echo off\r\necho PowerShell 5.1.0\r\n")?;
        Ok(path)
    }

    #[cfg(unix)]
    {
        let path = dir.join("old-pwsh.sh");
        fs::write(&path, "#!/bin/sh\necho PowerShell 5.1.0\n")?;
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions)?;
        Ok(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("unsupported platform for ps_repl test fixture");
    }
}

fn write_test_png(dir: &Path) -> Result<std::path::PathBuf> {
    let path = dir.join("dot.png");
    let png_bytes = BASE64_STANDARD.decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==",
    )?;
    fs::write(&path, png_bytes)?;
    Ok(path)
}

fn ps_single_quote(input: &Path) -> String {
    input.display().to_string().replace('\'', "''")
}

async fn run_ps_repl_turn(
    server: &MockServer,
    prompt: &str,
    calls: &[(&str, &str)],
) -> Result<ResponseMock> {
    let test = test_codex()
        .with_config(|config| {
            config.features.enable(Feature::PsRepl);
        })
        .build(server)
        .await?;

    let mut first_events = vec![ev_response_created("resp-1")];
    for (call_id, ps_input) in calls {
        first_events.push(ev_custom_tool_call(call_id, "ps_repl", ps_input));
    }
    first_events.push(ev_completed("resp-1"));
    responses::mount_sse_once(server, sse(first_events)).await;

    let second_mock = responses::mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    test.submit_turn(prompt).await?;
    Ok(second_mock)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_is_not_advertised_when_startup_pwsh_is_incompatible() -> Result<()> {
    skip_if_no_network!(Ok(()));
    if std::env::var_os("CODEX_PS_REPL_PATH").is_some() {
        return Ok(());
    }

    let server = responses::start_mock_server().await;
    let temp = tempdir()?;
    let old_pwsh = write_too_old_pwsh_script(temp.path())?;

    let test = test_codex()
        .with_config(move |config| {
            config.features.enable(Feature::PsRepl);
            config.ps_repl_path = Some(old_pwsh);
        })
        .build(&server)
        .await?;
    let warning = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Warning(ev) if ev.message.contains("Disabled `ps_repl` for this session") => {
            Some(ev.message.clone())
        }
        _ => None,
    })
    .await;
    assert!(
        warning.contains("PowerShell runtime"),
        "warning should explain the PowerShell compatibility issue: {warning}"
    );

    let request_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    test.submit_turn("hello").await?;

    let body = request_mock.single_request().body_json();
    let tools = tool_names(&body);
    assert!(
        !tools.iter().any(|tool| tool == "ps_repl"),
        "ps_repl should be omitted when startup validation fails: {tools:?}"
    );
    assert!(
        !tools.iter().any(|tool| tool == "ps_repl_reset"),
        "ps_repl_reset should be omitted when startup validation fails: {tools:?}"
    );
    let instructions = body["instructions"].as_str().unwrap_or_default();
    assert!(
        !instructions.contains("## PowerShell REPL (pwsh)"),
        "startup instructions should not mention ps_repl when it is disabled: {instructions}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_persists_variables_functions_and_modules() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.features.enable(Feature::PsRepl);
        })
        .build(&server)
        .await?;

    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_custom_tool_call(
                "call-1",
                "ps_repl",
                r#"
$x = 41
New-Module -Name CodexPsTestModule -ScriptBlock {
    function Get-CodexValue { 42 }
} | Import-Module
function Add-One {
    param([int]$Value)
    $Value + 1
}
Write-Output "state-ready"
"#,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let second_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-2"),
            ev_custom_tool_call(
                "call-2",
                "ps_repl",
                r#"
Write-Output ($x + 1)
Write-Output (Add-One -Value 1)
Write-Output (Get-CodexValue)
"#,
            ),
            ev_completed("resp-2"),
        ]),
    )
    .await;
    let third_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-3"),
        ]),
    )
    .await;

    test.submit_turn("run ps_repl twice").await?;

    let req2 = second_mock.single_request();
    let (first_output, first_success) = custom_tool_output_text_and_success(&req2, "call-1");
    assert_ne!(
        first_success,
        Some(false),
        "first ps_repl call failed unexpectedly: {first_output}"
    );
    assert!(first_output.contains("state-ready"));

    let req3 = third_mock.single_request();
    let (second_output, second_success) = custom_tool_output_text_and_success(&req3, "call-2");
    assert_ne!(
        second_success,
        Some(false),
        "second ps_repl call failed unexpectedly: {second_output}"
    );
    let lines = second_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert!(
        lines.iter().filter(|line| **line == "42").count() >= 2,
        "expected persisted variable and module output, got: {second_output}"
    );
    assert!(
        lines.contains(&"2"),
        "expected persisted function output, got: {second_output}"
    );
    assert!(
        !second_output.contains("Get-CodexValue"),
        "unexpected formatting leak: {second_output}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_can_invoke_builtin_tools() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let mock = run_ps_repl_turn(
        &server,
        "use ps_repl to call a tool",
        &[(
            "call-1",
            "$toolOut = Invoke-CodexTool -Name list_mcp_resources -Arguments @{}; Write-Output $toolOut.type",
        )],
    )
    .await?;

    let req = mock.single_request();
    let (output, success) = custom_tool_output_text_and_success(&req, "call-1");
    assert_ne!(
        success,
        Some(false),
        "ps_repl call failed unexpectedly: {output}"
    );
    assert!(output.contains("function_call_output"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_tool_call_rejects_recursive_ps_repl_invocation() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let mock = run_ps_repl_turn(
        &server,
        "use ps_repl recursively",
        &[(
            "call-1",
            r#"
try {
    Invoke-CodexTool -Name ps_repl -Arguments "Write-Output 'recursive'" | Out-Null
    Write-Output "unexpected-success"
} catch {
    Write-Output $_.Exception.Message
}
"#,
        )],
    )
    .await?;

    let req = mock.single_request();
    let (output, success) = custom_tool_output_text_and_success(&req, "call-1");
    assert_ne!(
        success,
        Some(false),
        "ps_repl call failed unexpectedly: {output}"
    );
    assert!(
        output.contains("ps_repl cannot invoke itself"),
        "expected recursion guard message, got output: {output}"
    );
    assert!(
        !output.contains("unexpected-success"),
        "recursive ps_repl call unexpectedly succeeded: {output}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_resets_after_timeout_and_accepts_followup_execution() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.features.enable(Feature::PsRepl);
        })
        .build(&server)
        .await?;

    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_custom_tool_call(
                "call-1",
                "ps_repl",
                "# codex-ps-repl: timeout_ms=50\nStart-Sleep -Milliseconds 500",
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let second_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-2"),
            ev_custom_tool_call("call-2", "ps_repl", "Write-Output 'healthy'"),
            ev_completed("resp-2"),
        ]),
    )
    .await;
    let third_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-3"),
        ]),
    )
    .await;

    test.submit_turn("run ps_repl after timeout").await?;

    let req2 = second_mock.single_request();
    let (first_output, first_success) = custom_tool_output_text_and_success(&req2, "call-1");
    assert_ne!(
        first_success,
        Some(true),
        "timeout should not report success: {first_output}"
    );
    assert!(
        first_output.contains("ps_repl execution timed out"),
        "expected timeout output, got: {first_output}"
    );

    let req3 = third_mock.single_request();
    let (second_output, second_success) = custom_tool_output_text_and_success(&req3, "call-2");
    assert_ne!(
        second_success,
        Some(false),
        "ps_repl follow-up execution failed unexpectedly: {second_output}"
    );
    assert!(second_output.contains("healthy"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_captures_standard_powershell_output_streams() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let mock = run_ps_repl_turn(
        &server,
        "capture powershell output",
        &[(
            "call-1",
            "Write-Output 'stdout'; Write-Warning 'warn-stream'",
        )],
    )
    .await?;

    let req = mock.single_request();
    let (output, success) = custom_tool_output_text_and_success(&req, "call-1");
    assert_ne!(
        success,
        Some(false),
        "ps_repl call failed unexpectedly: {output}"
    );
    assert!(output.contains("stdout"));
    assert!(output.contains("warn-stream"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ps_repl_view_image_propagates_content_items() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let temp = tempdir()?;
    let png_path = write_test_png(temp.path())?;
    let png_path = ps_single_quote(&png_path);
    let script =
        format!("$null = Invoke-CodexTool -Name view_image -Arguments @{{ path = '{png_path}' }}");

    let mock = run_ps_repl_turn(
        &server,
        "render an image via ps_repl",
        &[("call-1", &script)],
    )
    .await?;

    let req = mock.single_request();
    let custom_output = req.custom_tool_call_output("call-1");
    let output_items = custom_output
        .get("output")
        .and_then(Value::as_array)
        .expect("custom_tool_call_output should be a content item array");
    let image_url = output_items
        .iter()
        .find_map(|item| {
            (item.get("type").and_then(Value::as_str) == Some("input_image"))
                .then(|| item.get("image_url").and_then(Value::as_str))
                .flatten()
        })
        .expect("image_url present in ps_repl custom tool output");
    assert!(
        image_url.starts_with("data:image/png;base64,"),
        "expected png data URL, got {image_url}"
    );

    Ok(())
}
