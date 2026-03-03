#![cfg(not(target_os = "windows"))]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_core::CodexAuth;
use codex_core::features::Feature;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_models_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use image::GenericImageView;
use image::load_from_memory;
use pretty_assertions::assert_eq;
use serde_json::Value;
use wiremock::BodyPrintLimit;
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presentation_artifact_render_preview_returns_inline_image() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = test_codex()
        .with_config(|config| {
            config.features.enable(Feature::Artifact);
        })
        .build(&server)
        .await?;

    let call_id = "presentation-render-preview";
    let arguments = serde_json::json!({
        "actions": [
            {
                "action": "create",
                "args": {
                    "name": "Preview",
                    "theme": {
                        "color_scheme": {
                            "bg1": "F6F2E8",
                            "tx1": "1B1B1B"
                        }
                    }
                }
            },
            {
                "action": "add_slide",
                "args": {
                    "background_fill": "#F1E6D6"
                }
            },
            {
                "action": "add_shape",
                "args": {
                    "slide_index": 0,
                    "geometry": "rectangle",
                    "position": { "left": 72, "top": 120, "width": 160, "height": 80 },
                    "fill": "#C95A3D"
                }
            },
            {
                "action": "render_preview",
                "args": {}
            }
        ]
    })
    .to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "presentation_artifact", &arguments),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "done"),
        ev_completed("resp-2"),
    ]);
    let mock = responses::mount_sse_once(&server, second_response).await;

    codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "render the deck preview".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let function_output = mock.single_request().function_call_output(call_id);
    let output_items = function_output
        .get("output")
        .and_then(Value::as_array)
        .expect("render_preview output should be content items");
    assert_eq!(output_items.len(), 2);
    assert_eq!(
        output_items[0].get("type").and_then(Value::as_str),
        Some("input_text")
    );
    assert_eq!(
        output_items[1].get("type").and_then(Value::as_str),
        Some("input_image")
    );
    let image_url = output_items[1]
        .get("image_url")
        .and_then(Value::as_str)
        .expect("preview image_url present");
    let (prefix, payload) = image_url
        .split_once(',')
        .expect("preview image contains data prefix");
    assert_eq!(prefix, "data:image/png;base64");

    let decoded = BASE64_STANDARD.decode(payload)?;
    let rendered = load_from_memory(&decoded)?;
    assert_eq!(rendered.dimensions(), (720, 540));
    assert_eq!(rendered.get_pixel(20, 20).0, [0xF1, 0xE6, 0xD6, 0xFF]);
    assert_eq!(rendered.get_pixel(100, 150).0, [0xC9, 0x5A, 0x3D, 0xFF]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presentation_artifact_render_preview_fails_for_text_only_model() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::builder()
        .body_print_limit(BodyPrintLimit::Limited(80_000))
        .start()
        .await;
    let model_slug = "text-only-presentation-preview-test-model";
    let text_only_model = ModelInfo {
        slug: model_slug.to_string(),
        display_name: "Text-only presentation preview test model".to_string(),
        description: Some(
            "Remote model for presentation preview unsupported-path coverage".to_string(),
        ),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: vec![ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: ReasoningEffort::Medium.to_string(),
        }],
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        input_modalities: vec![InputModality::Text],
        prefer_websockets: false,
        used_fallback_model_metadata: false,
        priority: 1,
        upgrade: None,
        base_instructions: "base instructions".to_string(),
        model_messages: None,
        supports_reasoning_summaries: false,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        availability_nux: None,
        apply_patch_tool_type: None,
        truncation_policy: TruncationPolicyConfig::bytes(10_000),
        supports_parallel_tool_calls: false,
        context_window: Some(272_000),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
    };
    mount_models_once(
        &server,
        ModelsResponse {
            models: vec![text_only_model],
        },
    )
    .await;

    let TestCodex { codex, cwd, .. } = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(|config| {
            config.features.enable(Feature::Artifact);
            config.model = Some(model_slug.to_string());
        })
        .build(&server)
        .await?;

    let call_id = "presentation-render-preview-unsupported";
    let arguments = serde_json::json!({
        "actions": [
            {
                "action": "create",
                "args": { "name": "Preview" }
            },
            {
                "action": "add_slide",
                "args": {}
            },
            {
                "action": "render_preview",
                "args": {}
            }
        ]
    })
    .to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "presentation_artifact", &arguments),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "done"),
        ev_completed("resp-2"),
    ]);
    let mock = responses::mount_sse_once(&server, second_response).await;

    codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "render the deck preview".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: model_slug.to_string(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let output_text = mock
        .single_request()
        .function_call_output_content_and_success(call_id)
        .and_then(|(content, _)| content)
        .expect("output text present");
    assert_eq!(
        output_text,
        "render_preview is not allowed because you do not support image inputs"
    );

    Ok(())
}
