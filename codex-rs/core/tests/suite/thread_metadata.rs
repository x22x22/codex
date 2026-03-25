use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::config::Constrained;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use std::time::Duration;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_start_tracks_thread_started_analytics() -> Result<()> {
    let server = start_mock_server().await;
    let chatgpt_base_url = server.uri();

    let test = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(move |config| {
            config.chatgpt_base_url = chatgpt_base_url;
            config.model = Some("gpt-5".to_string());
            config.model_reasoning_effort = Some(ReasoningEffort::High);
            config.model_reasoning_summary = Some(ReasoningSummary::Detailed);
            config.service_tier = Some(ServiceTier::Flex);
            config.approvals_reviewer = ApprovalsReviewer::GuardianSubagent;
            config.permissions.sandbox_policy = Constrained::allow_any(
                codex_protocol::protocol::SandboxPolicy::new_workspace_write_policy(),
            );
            config.personality = Some(Personality::Friendly);
            config.ephemeral = true;
        })
        .build(&server)
        .await?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let analytics_request = loop {
        let requests = server.received_requests().await.unwrap_or_default();
        if let Some(request) = requests
            .into_iter()
            .find(|request| request.url.path() == "/codex/analytics-events/events")
        {
            break request;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for thread analytics request");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let payload: serde_json::Value =
        serde_json::from_slice(&analytics_request.body).expect("analytics payload");
    let event = payload["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|event| event["event_type"] == "codex_thread_started")
        .expect("codex_thread_started event should be present");

    assert_eq!(
        event["event_params"]["thread_id"],
        serde_json::json!(test.session_configured.session_id.to_string())
    );
    assert_eq!(
        event["event_params"]["product_client_id"],
        serde_json::json!(codex_core::default_client::originator().value)
    );
    assert_eq!(event["event_params"]["model"], "gpt-5");
    assert_eq!(event["event_params"]["model_provider"], "openai");
    assert_eq!(event["event_params"]["reasoning_effort"], "high");
    assert_eq!(event["event_params"]["reasoning_summary"], "detailed");
    assert_eq!(event["event_params"]["service_tier"], "flex");
    assert_eq!(event["event_params"]["approval_policy"], "on-request");
    assert_eq!(
        event["event_params"]["approvals_reviewer"],
        "guardian_subagent"
    );
    assert_eq!(event["event_params"]["sandbox_policy"], "workspace_write");
    assert_eq!(event["event_params"]["sandbox_network_access"], false);
    assert_eq!(event["event_params"]["collaboration_mode"], "default");
    assert_eq!(event["event_params"]["personality"], "friendly");
    assert_eq!(event["event_params"]["ephemeral"], true);
    assert_eq!(event["event_params"]["session_source"], "user");
    assert_eq!(event["event_params"]["initialization_mode"], "new");
    assert_eq!(
        event["event_params"]["subagent_source"],
        serde_json::Value::Null
    );
    assert_eq!(
        event["event_params"]["parent_thread_id"],
        serde_json::Value::Null
    );
    assert!(
        event["event_params"]["created_at"]
            .as_u64()
            .is_some_and(|timestamp| timestamp > 0)
    );

    Ok(())
}
