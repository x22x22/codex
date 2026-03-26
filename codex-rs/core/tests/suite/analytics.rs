use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::SteerInputError;
use codex_core::config::Constrained;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use std::time::Duration;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_initialization_tracks_thread_initialized_analytics() -> Result<()> {
    let server = start_mock_server().await;
    let chatgpt_base_url = server.uri();

    let test = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(move |config| {
            config.chatgpt_base_url = chatgpt_base_url;
            config.model = Some("gpt-5".to_string());
            config.permissions.sandbox_policy = Constrained::allow_any(
                codex_protocol::protocol::SandboxPolicy::new_workspace_write_policy(),
            );
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
        .find(|event| event["event_type"] == "codex_thread_initialized")
        .expect("codex_thread_initialized event should be present");

    assert_eq!(
        event["event_params"]["thread_id"],
        serde_json::json!(test.session_configured.session_id.to_string())
    );
    assert_eq!(
        event["event_params"]["product_client_id"],
        serde_json::json!(codex_core::default_client::originator().value)
    );
    assert_eq!(event["event_params"]["model"], "gpt-5");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_steer_tracks_turn_steer_analytics() -> Result<()> {
    let server = start_mock_server().await;
    let chatgpt_base_url = server.uri();

    let test = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(move |config| {
            config.chatgpt_base_url = chatgpt_base_url;
            config.model = Some("gpt-5".to_string());
            config.permissions.sandbox_policy = Constrained::allow_any(
                codex_protocol::protocol::SandboxPolicy::new_workspace_write_policy(),
            );
            config.ephemeral = true;
        })
        .build(&server)
        .await?;

    let err = test
        .codex
        .steer_input(
            vec![UserInput::Text {
                text: "reject this steer".to_string(),
                text_elements: Vec::new(),
            }],
            None,
        )
        .await
        .expect_err("steer should be rejected without an active turn");

    assert!(matches!(err, SteerInputError::NoActiveTurn(_)));

    let deadline = Instant::now() + Duration::from_secs(10);
    let event = loop {
        let requests = server.received_requests().await.unwrap_or_default();
        if let Some(event) = requests
            .into_iter()
            .filter(|request| request.url.path() == "/codex/analytics-events/events")
            .filter_map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).ok())
            .flat_map(|payload| {
                payload["events"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
            })
            .find(|event| event["event_type"] == "codex_turn_steer_event")
        {
            break event;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for steer analytics event");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(
        event["event_params"]["thread_id"],
        serde_json::json!(test.session_configured.session_id.to_string())
    );
    assert_eq!(
        event["event_params"]["expected_turn_id"],
        serde_json::Value::Null
    );
    assert_eq!(
        event["event_params"]["accepted_turn_id"],
        serde_json::Value::Null
    );
    assert_eq!(
        event["event_params"]["product_client_id"],
        serde_json::json!(codex_core::default_client::originator().value)
    );
    assert_eq!(event["event_params"]["num_input_images"], 0);
    assert_eq!(event["event_params"]["result"], "rejected");
    assert_eq!(event["event_params"]["rejection_reason"], "no_active_turn");
    assert!(
        event["event_params"]["created_at"]
            .as_u64()
            .is_some_and(|timestamp| timestamp > 0)
    );

    Ok(())
}
