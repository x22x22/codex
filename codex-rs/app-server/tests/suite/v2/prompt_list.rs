use std::time::Duration;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::PromptListParams;
use codex_app_server_protocol::PromptListResponse;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn write_prompt(root: &TempDir, name: &str, contents: &str) -> Result<()> {
    let prompts_dir = root.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir)?;
    std::fs::write(prompts_dir.join(format!("{name}.md")), contents)?;
    Ok(())
}

#[tokio::test]
async fn prompt_list_returns_environment_bound_custom_prompts() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_prompt(
        &codex_home,
        "draft-release-notes",
        "---\ndescription: Release notes helper\nargument_hint: version\n---\n\nBody",
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_prompt_list_request(PromptListParams {
            environment_id: Some("local".to_string()),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let PromptListResponse {
        environment_id,
        prompts,
    } = to_response(response)?;
    assert_eq!(environment_id, "local");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "draft-release-notes");
    assert_eq!(
        prompts[0].description.as_deref(),
        Some("Release notes helper")
    );
    assert_eq!(prompts[0].argument_hint.as_deref(), Some("version"));
    assert_eq!(prompts[0].content.trim(), "Body");
    Ok(())
}

#[tokio::test]
async fn prompt_list_rejects_unsupported_environment_id() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_prompt_list_request(PromptListParams {
            environment_id: Some("remote".to_string()),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        err.error
            .message
            .contains("unsupported environmentId `remote`"),
        "unexpected error: {}",
        err.error.message
    );
    Ok(())
}
