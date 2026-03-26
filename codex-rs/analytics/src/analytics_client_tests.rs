use super::AnalyticsEventsQueue;
use super::AnalyticsInput;
use super::AnalyticsReducer;
use super::AppInvocation;
use super::CodexAppMentionedEventRequest;
use super::CodexAppUsedEventRequest;
use super::CodexPluginEventRequest;
use super::CodexPluginUsedEventRequest;
use super::CodexThreadContext;
use super::CodexThreadInitializedInput;
use super::CodexTurnEvent;
use super::CodexTurnEventRequest;
use super::CodexTurnSteerEvent;
use super::CodexTurnSteerEventRequest;
use super::InitializationMode;
use super::InvocationType;
use super::TrackEventRequest;
use super::TrackEventsContext;
use super::TurnCompletedInput;
use super::TurnStartedInput;
use super::TurnSteerResult;
use super::codex_app_metadata;
use super::codex_plugin_metadata;
use super::codex_plugin_used_metadata;
use super::codex_thread_initialized_event_request;
use super::codex_turn_event_params;
use super::codex_turn_steer_event_params;
use super::normalize_path_for_skill_id;
use codex_login::default_client::originator;
use codex_plugin::AppConnectorId;
use codex_plugin::PluginCapabilitySummary;
use codex_plugin::PluginId;
use codex_plugin::PluginTelemetryMetadata;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;

fn expected_absolute_path(path: &PathBuf) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn normalize_path_for_skill_id_repo_scoped_uses_relative_path() {
    let repo_root = PathBuf::from("/repo/root");
    let skill_path = PathBuf::from("/repo/root/.codex/skills/doc/SKILL.md");

    let path = normalize_path_for_skill_id(
        Some("https://example.com/repo.git"),
        Some(repo_root.as_path()),
        skill_path.as_path(),
    );

    assert_eq!(path, ".codex/skills/doc/SKILL.md");
}

#[test]
fn normalize_path_for_skill_id_user_scoped_uses_absolute_path() {
    let skill_path = PathBuf::from("/Users/abc/.codex/skills/doc/SKILL.md");

    let path = normalize_path_for_skill_id(None, None, skill_path.as_path());
    let expected = expected_absolute_path(&skill_path);

    assert_eq!(path, expected);
}

#[test]
fn normalize_path_for_skill_id_admin_scoped_uses_absolute_path() {
    let skill_path = PathBuf::from("/etc/codex/skills/doc/SKILL.md");

    let path = normalize_path_for_skill_id(None, None, skill_path.as_path());
    let expected = expected_absolute_path(&skill_path);

    assert_eq!(path, expected);
}

#[test]
fn normalize_path_for_skill_id_repo_root_not_in_skill_path_uses_absolute_path() {
    let repo_root = PathBuf::from("/repo/root");
    let skill_path = PathBuf::from("/other/path/.codex/skills/doc/SKILL.md");

    let path = normalize_path_for_skill_id(
        Some("https://example.com/repo.git"),
        Some(repo_root.as_path()),
        skill_path.as_path(),
    );
    let expected = expected_absolute_path(&skill_path);

    assert_eq!(path, expected);
}

#[test]
fn app_mentioned_event_serializes_expected_shape() {
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    };
    let event = TrackEventRequest::AppMentioned(CodexAppMentionedEventRequest {
        event_type: "codex_app_mentioned",
        event_params: codex_app_metadata(
            &tracking,
            AppInvocation {
                connector_id: Some("calendar".to_string()),
                app_name: Some("Calendar".to_string()),
                invocation_type: Some(InvocationType::Explicit),
            },
        ),
    });

    let payload = serde_json::to_value(&event).expect("serialize app mentioned event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_app_mentioned",
            "event_params": {
                "connector_id": "calendar",
                "thread_id": "thread-1",
                "turn_id": "turn-1",
                "app_name": "Calendar",
                "product_client_id": originator().value,
                "invoke_type": "explicit",
                "model_slug": "gpt-5"
            }
        })
    );
}

#[test]
fn app_used_event_serializes_expected_shape() {
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-2".to_string(),
        turn_id: "turn-2".to_string(),
    };
    let event = TrackEventRequest::AppUsed(CodexAppUsedEventRequest {
        event_type: "codex_app_used",
        event_params: codex_app_metadata(
            &tracking,
            AppInvocation {
                connector_id: Some("drive".to_string()),
                app_name: Some("Google Drive".to_string()),
                invocation_type: Some(InvocationType::Implicit),
            },
        ),
    });

    let payload = serde_json::to_value(&event).expect("serialize app used event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_app_used",
            "event_params": {
                "connector_id": "drive",
                "thread_id": "thread-2",
                "turn_id": "turn-2",
                "app_name": "Google Drive",
                "product_client_id": originator().value,
                "invoke_type": "implicit",
                "model_slug": "gpt-5"
            }
        })
    );
}

#[test]
fn app_used_dedupe_is_keyed_by_turn_and_connector() {
    let (sender, _receiver) = mpsc::channel(1);
    let queue = AnalyticsEventsQueue {
        sender,
        app_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
        plugin_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
    };
    let app = AppInvocation {
        connector_id: Some("calendar".to_string()),
        app_name: Some("Calendar".to_string()),
        invocation_type: Some(InvocationType::Implicit),
    };

    let turn_1 = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    };
    let turn_2 = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-2".to_string(),
    };

    assert_eq!(queue.should_enqueue_app_used(&turn_1, &app), true);
    assert_eq!(queue.should_enqueue_app_used(&turn_1, &app), false);
    assert_eq!(queue.should_enqueue_app_used(&turn_2, &app), true);
}

#[test]
fn turn_event_serializes_expected_shape() {
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-2".to_string(),
        turn_id: "turn-2".to_string(),
    };
    let event = TrackEventRequest::TurnEvent(Box::new(CodexTurnEventRequest {
        event_type: "codex_turn_event",
        event_params: codex_turn_event_params(
            &tracking,
            CodexTurnEvent {
                submission_type: None,
                model_provider: "openai".to_string(),
                sandbox_policy: SandboxPolicy::new_read_only_policy(),
                reasoning_effort: Some(ReasoningEffort::High),
                reasoning_summary: Some(ReasoningSummary::Detailed),
                service_tier: Some(ServiceTier::Flex),
                approval_policy: AskForApproval::OnRequest,
                approvals_reviewer: ApprovalsReviewer::GuardianSubagent,
                sandbox_network_access: true,
                collaboration_mode: ModeKind::Plan,
                personality: Some(Personality::Pragmatic),
                num_input_images: 2,
                is_first_turn: true,
                status: None,
                turn_error: None,
                steer_count: None,
                total_tool_call_count: None,
                shell_command_count: None,
                file_change_count: None,
                mcp_tool_call_count: None,
                dynamic_tool_call_count: None,
                subagent_tool_call_count: None,
                web_search_count: None,
                image_generation_count: None,
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            },
        ),
    }));

    let payload = serde_json::to_value(&event).expect("serialize turn event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_turn_event",
            "event_params": {
                "thread_id": "thread-2",
                "turn_id": "turn-2",
                "product_client_id": originator().value,
                "submission_type": null,
                "model": "gpt-5",
                "model_provider": "openai",
                "sandbox_policy": "read_only",
                "reasoning_effort": "high",
                "reasoning_summary": "detailed",
                "service_tier": "flex",
                "approval_policy": "on-request",
                "approvals_reviewer": "guardian_subagent",
                "sandbox_network_access": true,
                "collaboration_mode": "plan",
                "personality": "pragmatic",
                "num_input_images": 2,
                "is_first_turn": true,
                "status": null,
                "turn_error": null,
                "steer_count": null,
                "total_tool_call_count": null,
                "shell_command_count": null,
                "file_change_count": null,
                "mcp_tool_call_count": null,
                "dynamic_tool_call_count": null,
                "subagent_tool_call_count": null,
                "web_search_count": null,
                "image_generation_count": null,
                "input_tokens": null,
                "cached_input_tokens": null,
                "output_tokens": null,
                "reasoning_output_tokens": null,
                "total_tokens": null,
                "duration_ms": null,
                "started_at": null,
                "completed_at": null
            }
        })
    );
}

#[test]
fn turn_steer_event_serializes_expected_shape() {
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-2".to_string(),
        turn_id: "turn-2".to_string(),
    };
    let event = TrackEventRequest::TurnSteer(CodexTurnSteerEventRequest {
        event_type: "codex_turn_steer_event",
        event_params: codex_turn_steer_event_params(
            &tracking,
            CodexTurnSteerEvent {
                expected_turn_id: "turn-2".to_string(),
                accepted_turn_id: Some("turn-2".to_string()),
                num_input_images: 2,
                result: TurnSteerResult::Accepted,
                rejection_reason: None,
                created_at: 1_716_000_123,
            },
        ),
    });

    let payload = serde_json::to_value(&event).expect("serialize turn steer event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_turn_steer_event",
            "event_params": {
                "thread_id": "thread-2",
                "expected_turn_id": "turn-2",
                "accepted_turn_id": "turn-2",
                "product_client_id": originator().value,
                "num_input_images": 2,
                "result": "accepted",
                "rejection_reason": null,
                "created_at": 1_716_000_123
            }
        })
    );
}

#[tokio::test]
async fn turn_started_then_completed_emits_turn_event() {
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-2".to_string(),
        turn_id: "turn-2".to_string(),
    };
    let mut reducer = AnalyticsReducer::default();
    let mut out = Vec::new();

    reducer
        .ingest(
            AnalyticsInput::TurnStarted(TurnStartedInput {
                tracking: tracking.clone(),
                turn_event: CodexTurnEvent {
                    submission_type: None,
                    model_provider: "openai".to_string(),
                    sandbox_policy: SandboxPolicy::new_read_only_policy(),
                    reasoning_effort: Some(ReasoningEffort::High),
                    reasoning_summary: Some(ReasoningSummary::Detailed),
                    service_tier: Some(ServiceTier::Flex),
                    approval_policy: AskForApproval::OnRequest,
                    approvals_reviewer: ApprovalsReviewer::GuardianSubagent,
                    sandbox_network_access: true,
                    collaboration_mode: ModeKind::Plan,
                    personality: Some(Personality::Pragmatic),
                    num_input_images: 2,
                    is_first_turn: true,
                    status: None,
                    turn_error: None,
                    steer_count: None,
                    total_tool_call_count: None,
                    shell_command_count: None,
                    file_change_count: None,
                    mcp_tool_call_count: None,
                    dynamic_tool_call_count: None,
                    subagent_tool_call_count: None,
                    web_search_count: None,
                    image_generation_count: None,
                    input_tokens: None,
                    cached_input_tokens: None,
                    output_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: None,
                    duration_ms: None,
                    started_at: None,
                    completed_at: None,
                },
            }),
            &mut out,
        )
        .await;

    assert!(out.is_empty());

    reducer
        .ingest(
            AnalyticsInput::TurnCompleted(TurnCompletedInput {
                turn_id: tracking.turn_id.clone(),
            }),
            &mut out,
        )
        .await;

    assert_eq!(out.len(), 1);
    let payload = serde_json::to_value(&out[0]).expect("serialize turn event");
    assert_eq!(payload["event_type"], json!("codex_turn_event"));
    assert_eq!(payload["event_params"]["thread_id"], json!("thread-2"));
    assert_eq!(payload["event_params"]["turn_id"], json!("turn-2"));
}

#[test]
fn thread_initialized_event_serializes_expected_shape() {
    let event = TrackEventRequest::CodexThreadInitialized(codex_thread_initialized_event_request(
        CodexThreadInitializedInput {
            thread_id: "thread-0".to_string(),
            model: "gpt-5".to_string(),
            product_client_id: originator().value,
            created_at: 1_716_000_000,
            thread_context: CodexThreadContext {
                ephemeral: true,
                session_source: SessionSource::Exec,
                initialization_mode: InitializationMode::New,
                subagent_source: None,
                parent_thread_id: None,
            },
        },
    ));

    let payload = serde_json::to_value(&event).expect("serialize thread initialized event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_thread_initialized",
            "event_params": {
                "thread_id": "thread-0",
                "product_client_id": originator().value,
                "model": "gpt-5",
                "ephemeral": true,
                "session_source": "user",
                "initialization_mode": "new",
                "subagent_source": null,
                "parent_thread_id": null,
                "created_at": 1716000000
            }
        })
    );
}

#[test]
fn thread_initialized_event_serializes_subagent_source() {
    let event = TrackEventRequest::CodexThreadInitialized(codex_thread_initialized_event_request(
        CodexThreadInitializedInput {
            thread_id: "thread-1".to_string(),
            model: "gpt-5".to_string(),
            product_client_id: originator().value,
            created_at: 1,
            thread_context: CodexThreadContext {
                ephemeral: false,
                session_source: SessionSource::SubAgent(SubAgentSource::Review),
                initialization_mode: InitializationMode::New,
                subagent_source: Some(SubAgentSource::Review),
                parent_thread_id: None,
            },
        },
    ));

    let payload =
        serde_json::to_value(&event).expect("serialize subagent thread initialized event");
    assert_eq!(payload["event_params"]["session_source"], "subagent");
    assert_eq!(payload["event_params"]["subagent_source"], "review");
}

#[test]
fn thread_initialized_event_omits_non_user_non_subagent_session_source() {
    let event = TrackEventRequest::CodexThreadInitialized(codex_thread_initialized_event_request(
        CodexThreadInitializedInput {
            thread_id: "thread-2".to_string(),
            model: "gpt-5".to_string(),
            product_client_id: originator().value,
            created_at: 1,
            thread_context: CodexThreadContext {
                ephemeral: false,
                session_source: SessionSource::Mcp,
                initialization_mode: InitializationMode::New,
                subagent_source: None,
                parent_thread_id: None,
            },
        },
    ));

    let payload = serde_json::to_value(&event).expect("serialize mcp thread initialized event");
    assert_eq!(
        payload["event_params"]["session_source"],
        serde_json::Value::Null
    );
}

#[test]
fn plugin_used_event_serializes_expected_shape() {
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-3".to_string(),
        turn_id: "turn-3".to_string(),
    };
    let event = TrackEventRequest::PluginUsed(CodexPluginUsedEventRequest {
        event_type: "codex_plugin_used",
        event_params: codex_plugin_used_metadata(&tracking, sample_plugin_metadata()),
    });

    let payload = serde_json::to_value(&event).expect("serialize plugin used event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_plugin_used",
            "event_params": {
                "plugin_id": "sample@test",
                "plugin_name": "sample",
                "marketplace_name": "test",
                "has_skills": true,
                "mcp_server_count": 2,
                "connector_ids": ["calendar", "drive"],
                "product_client_id": originator().value,
                "thread_id": "thread-3",
                "turn_id": "turn-3",
                "model_slug": "gpt-5"
            }
        })
    );
}

#[test]
fn plugin_management_event_serializes_expected_shape() {
    let event = TrackEventRequest::PluginInstalled(CodexPluginEventRequest {
        event_type: "codex_plugin_installed",
        event_params: codex_plugin_metadata(sample_plugin_metadata()),
    });

    let payload = serde_json::to_value(&event).expect("serialize plugin installed event");

    assert_eq!(
        payload,
        json!({
            "event_type": "codex_plugin_installed",
            "event_params": {
                "plugin_id": "sample@test",
                "plugin_name": "sample",
                "marketplace_name": "test",
                "has_skills": true,
                "mcp_server_count": 2,
                "connector_ids": ["calendar", "drive"],
                "product_client_id": originator().value
            }
        })
    );
}

#[test]
fn plugin_used_dedupe_is_keyed_by_turn_and_plugin() {
    let (sender, _receiver) = mpsc::channel(1);
    let queue = AnalyticsEventsQueue {
        sender,
        app_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
        plugin_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
    };
    let plugin = sample_plugin_metadata();

    let turn_1 = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    };
    let turn_2 = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-2".to_string(),
    };

    assert_eq!(queue.should_enqueue_plugin_used(&turn_1, &plugin), true);
    assert_eq!(queue.should_enqueue_plugin_used(&turn_1, &plugin), false);
    assert_eq!(queue.should_enqueue_plugin_used(&turn_2, &plugin), true);
}

fn sample_plugin_metadata() -> PluginTelemetryMetadata {
    PluginTelemetryMetadata {
        plugin_id: PluginId::parse("sample@test").expect("valid plugin id"),
        capability_summary: Some(PluginCapabilitySummary {
            config_name: "sample@test".to_string(),
            display_name: "sample".to_string(),
            description: None,
            has_skills: true,
            mcp_server_names: vec!["mcp-1".to_string(), "mcp-2".to_string()],
            app_connector_ids: vec![
                AppConnectorId("calendar".to_string()),
                AppConnectorId("drive".to_string()),
            ],
        }),
    }
}
