use super::AnalyticsEventsQueue;
use super::AppInvocation;
use super::CodexAppMentionedEventRequest;
use super::CodexAppUsedEventRequest;
use super::CodexPluginEventRequest;
use super::CodexPluginUsedEventRequest;
use super::InvocationType;
use super::TrackAppMentionedJob;
use super::TrackAppUsedJob;
use super::TrackEventRequest;
use super::TrackEventsContext;
use super::TrackEventsJob;
use super::TrackPluginManagementJob;
use super::TrackPluginUsedJob;
use super::TrackSkillInvocationsJob;
use super::codex_app_metadata;
use super::codex_plugin_metadata;
use super::codex_plugin_used_metadata;
use super::normalize_path_for_skill_id;
use crate::config::ConfigBuilder;
use crate::plugins::AppConnectorId;
use crate::plugins::PluginCapabilitySummary;
use crate::plugins::PluginId;
use crate::plugins::PluginTelemetryMetadata;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tempfile::TempDir;
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
                "product_client_id": crate::default_client::originator().value,
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
                "product_client_id": crate::default_client::originator().value,
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
                "product_client_id": crate::default_client::originator().value,
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
                "product_client_id": crate::default_client::originator().value
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

#[test]
fn track_events_job_type_uses_expected_tag_values() {
    let codex_home = TempDir::new().expect("tempdir should create");
    let config = Arc::new(load_test_config(codex_home.path()));
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    };
    let cases = vec![
        (
            TrackEventsJob::SkillInvocations(TrackSkillInvocationsJob {
                config: Arc::clone(&config),
                tracking: tracking.clone(),
                invocations: Vec::new(),
            }),
            "skill_invocations",
        ),
        (
            TrackEventsJob::AppMentioned(TrackAppMentionedJob {
                config: Arc::clone(&config),
                tracking: tracking.clone(),
                mentions: Vec::new(),
            }),
            "app_mentioned",
        ),
        (
            TrackEventsJob::AppUsed(TrackAppUsedJob {
                config: Arc::clone(&config),
                tracking: tracking.clone(),
                app: AppInvocation {
                    connector_id: None,
                    app_name: None,
                    invocation_type: None,
                },
            }),
            "app_used",
        ),
        (
            TrackEventsJob::PluginUsed(TrackPluginUsedJob {
                config: Arc::clone(&config),
                tracking: tracking.clone(),
                plugin: sample_plugin_metadata(),
            }),
            "plugin_used",
        ),
        (
            TrackEventsJob::PluginInstalled(TrackPluginManagementJob {
                config: Arc::clone(&config),
                plugin: sample_plugin_metadata(),
            }),
            "plugin_installed",
        ),
        (
            TrackEventsJob::PluginUninstalled(TrackPluginManagementJob {
                config: Arc::clone(&config),
                plugin: sample_plugin_metadata(),
            }),
            "plugin_uninstalled",
        ),
        (
            TrackEventsJob::PluginEnabled(TrackPluginManagementJob {
                config: Arc::clone(&config),
                plugin: sample_plugin_metadata(),
            }),
            "plugin_enabled",
        ),
        (
            TrackEventsJob::PluginDisabled(TrackPluginManagementJob {
                config,
                plugin: sample_plugin_metadata(),
            }),
            "plugin_disabled",
        ),
    ];

    for (job, expected) in cases {
        assert_eq!(job.job_type(), expected);
    }
}

#[test]
fn track_events_job_event_count_matches_underlying_payloads() {
    let codex_home = TempDir::new().expect("tempdir should create");
    let config = Arc::new(load_test_config(codex_home.path()));
    let tracking = TrackEventsContext {
        model_slug: "gpt-5".to_string(),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    };
    let skill_job = TrackEventsJob::SkillInvocations(TrackSkillInvocationsJob {
        config: Arc::clone(&config),
        tracking: tracking.clone(),
        invocations: vec![
            super::SkillInvocation {
                skill_name: "doc".to_string(),
                skill_scope: codex_protocol::protocol::SkillScope::Repo,
                skill_path: codex_home.path().join("SKILL.md"),
                invocation_type: InvocationType::Explicit,
            },
            super::SkillInvocation {
                skill_name: "doc-2".to_string(),
                skill_scope: codex_protocol::protocol::SkillScope::Repo,
                skill_path: codex_home.path().join("SKILL-2.md"),
                invocation_type: InvocationType::Implicit,
            },
        ],
    });
    let app_mentioned_job = TrackEventsJob::AppMentioned(TrackAppMentionedJob {
        config: Arc::clone(&config),
        tracking: tracking.clone(),
        mentions: vec![
            AppInvocation {
                connector_id: Some("drive".to_string()),
                app_name: Some("Drive".to_string()),
                invocation_type: Some(InvocationType::Explicit),
            },
            AppInvocation {
                connector_id: Some("calendar".to_string()),
                app_name: Some("Calendar".to_string()),
                invocation_type: Some(InvocationType::Implicit),
            },
            AppInvocation {
                connector_id: Some("gmail".to_string()),
                app_name: Some("Gmail".to_string()),
                invocation_type: None,
            },
        ],
    });
    let plugin_job = TrackEventsJob::PluginUsed(TrackPluginUsedJob {
        config,
        tracking,
        plugin: sample_plugin_metadata(),
    });

    assert_eq!(skill_job.event_count(), 2);
    assert_eq!(app_mentioned_job.event_count(), 3);
    assert_eq!(plugin_job.event_count(), 1);
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

fn load_test_config(codex_home: &std::path::Path) -> crate::config::Config {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build")
        .block_on(
            ConfigBuilder::default()
                .codex_home(codex_home.to_path_buf())
                .fallback_cwd(Some(codex_home.to_path_buf()))
                .build(),
        )
        .expect("config should load")
}
