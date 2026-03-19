use super::*;

use std::collections::BTreeMap;

use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Command;

#[tokio::test]
async fn build_turn_metadata_header_includes_has_changes_for_clean_repo() {
    let temp_dir = TempDir::new().expect("temp dir");
    let repo_path = temp_dir.path().join("repo");
    std::fs::create_dir_all(&repo_path).expect("create repo");

    Command::new("git")
        .args(["init"])
        .current_dir(&repo_path)
        .output()
        .await
        .expect("git init");
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&repo_path)
        .output()
        .await
        .expect("git config user.name");
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&repo_path)
        .output()
        .await
        .expect("git config user.email");

    std::fs::write(repo_path.join("README.md"), "hello").expect("write file");
    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_path)
        .output()
        .await
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_path)
        .output()
        .await
        .expect("git commit");

    let header = build_turn_metadata_header(&repo_path, Some("none"))
        .await
        .expect("header");
    let parsed: Value = serde_json::from_str(&header).expect("valid json");
    let workspace = parsed
        .get("workspaces")
        .and_then(Value::as_object)
        .and_then(|workspaces| workspaces.values().next())
        .cloned()
        .expect("workspace");

    assert_eq!(
        workspace.get("has_changes").and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn turn_metadata_state_uses_platform_sandbox_tag() {
    let temp_dir = TempDir::new().expect("temp dir");
    let cwd = temp_dir.path().to_path_buf();
    let sandbox_policy = SandboxPolicy::new_read_only_policy();

    let state = TurnMetadataState::new(
        "session-a".to_string(),
        "turn-a".to_string(),
        cwd,
        &sandbox_policy,
        WindowsSandboxLevel::Disabled,
        BTreeMap::new(),
    );

    let header = state.current_header_value().expect("header");
    let json: Value = serde_json::from_str(&header).expect("json");
    let sandbox_name = json.get("sandbox").and_then(Value::as_str);
    let session_id = json.get("session_id").and_then(Value::as_str);

    let expected_sandbox = sandbox_tag(&sandbox_policy, WindowsSandboxLevel::Disabled);
    assert_eq!(sandbox_name, Some(expected_sandbox));
    assert_eq!(session_id, Some("session-a"));
}

#[test]
fn extend_known_request_headers_reads_parent_ids_from_turn_metadata() {
    let mut headers = http::HeaderMap::new();
    let turn_metadata = serde_json::json!({
        "turn_id": "turn-a",
        "metadata": {
            PARENT_CONVERSATION_ID_METADATA_KEY: "conv-123",
            PARENT_MESSAGE_ID_METADATA_KEY: "msg-123",
            PARENT_TURN_ID_METADATA_KEY: "turn-123",
        },
    });
    let turn_metadata = serde_json::to_string(&turn_metadata).expect("turn metadata json");

    extend_known_request_headers(&mut headers, Some(turn_metadata.as_str()));

    assert_eq!(
        headers
            .get("x-openai-parent-conversation-id")
            .and_then(|value| value.to_str().ok()),
        Some("conv-123")
    );
    assert_eq!(
        headers
            .get("x-openai-parent-message-id")
            .and_then(|value| value.to_str().ok()),
        Some("msg-123")
    );
    assert_eq!(
        headers
            .get("x-openai-parent-turn-id")
            .and_then(|value| value.to_str().ok()),
        Some("turn-123")
    );
}
