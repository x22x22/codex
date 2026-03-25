#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use codex_core::CodexAuth;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::apps_test_server::AppsTestServer;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use std::sync::Mutex;
use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_test::internal::MockWriter;

#[tokio::test(flavor = "current_thread")]
async fn codex_apps_mcp_span_records_connector_metadata() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let buffer: &'static Mutex<Vec<u8>> = Box::leak(Box::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_level(true)
        .with_ansi(false)
        .with_max_level(Level::TRACE)
        .with_span_events(FmtSpan::FULL)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let server = start_mock_server().await;
    let apps_server = AppsTestServer::mount(&server).await?;

    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call(
                "calendar-call-1",
                "mcp__codex_apps__calendar_create_event",
                r#"{"title":"Lunch","starts_at":"2026-03-10T12:00:00Z"}"#,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "calendar tool completed successfully."),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    let fixture = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(move |config| {
            config
                .features
                .enable(Feature::Apps)
                .expect("test config should allow feature update");
            config.chatgpt_base_url = apps_server.chatgpt_base_url;
            config.model = Some("gpt-5-codex".to_string());
        })
        .build(&server)
        .await?;

    fixture
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "create a calendar event".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;

    wait_for_event(&fixture.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("mcp.tools.call{otel.kind=\"client\"")
            && logs.contains("mcp.server.name=\"codex_apps\"")
            && logs.contains("mcp.connector.id=\"calendar\"")
            && logs.contains("mcp.connector.name=\"Calendar\"")
            && logs.contains("tool.name=\"calendar_create_event\""),
        "missing connector metadata on mcp.tools.call span\nlogs:\n{logs}"
    );

    Ok(())
}
