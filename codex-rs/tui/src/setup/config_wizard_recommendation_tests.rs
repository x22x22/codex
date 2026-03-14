use super::*;
use codex_protocol::ThreadId;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::TurnContextItem;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn detects_recent_workspace_write_defaults_and_common_directories() {
    let tempdir = TempDir::new().expect("tempdir");
    let sessions_root = tempdir.path().join("sessions");
    let current_cwd = PathBuf::from("/Users/viyatb/.codex/worktrees/d003/codex");

    write_rollout(
        &sessions_root,
        "2026/03/13/rollout-2026-03-13T18-03-05-00000000-0000-0000-0000-000000000001.jsonl",
        SessionSource::VSCode,
        PathBuf::from("/Users/viyatb/code/openai"),
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![
                AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/infra")
                    .expect("absolute path"),
            ],
            read_only_access: Default::default(),
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    );
    write_rollout(
        &sessions_root,
        "2026/03/13/rollout-2026-03-13T17-03-05-00000000-0000-0000-0000-000000000002.jsonl",
        SessionSource::Cli,
        PathBuf::from("/Users/viyatb/code/openai"),
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![
                AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/infra")
                    .expect("absolute path"),
                AbsolutePathBuf::from_absolute_path("/tmp/should-not-appear")
                    .expect("absolute path"),
            ],
            read_only_access: Default::default(),
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    );
    write_rollout(
        &sessions_root,
        "2026/03/13/rollout-2026-03-13T16-03-05-00000000-0000-0000-0000-000000000003.jsonl",
        SessionSource::Exec,
        PathBuf::from("/Users/viyatb/code/ignored"),
        SandboxPolicy::DangerFullAccess,
    );

    let recommendation =
        detect_config_wizard_recommendation_in_root(sessions_root, Some(&current_cwd))
            .expect("recommendation");

    assert_eq!(recommendation.access_mode, SandboxMode::WorkspaceWrite);
    assert!(recommendation.network_access);
    assert_eq!(recommendation.sampled_sessions, 2);
    assert_eq!(recommendation.workspace_write_sessions, 2);
    assert_eq!(recommendation.workspace_write_network_sessions, 2);
    assert_eq!(
        recommendation.directories,
        vec![
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/infra").expect("absolute path"),
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/openai")
                .expect("absolute path"),
        ]
    );
}

#[test]
fn summary_mentions_network_and_directories() {
    let recommendation = ConfigWizardRecommendation {
        access_mode: SandboxMode::WorkspaceWrite,
        network_access: true,
        directories: vec![
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/openai")
                .expect("absolute path"),
        ],
        sampled_sessions: 12,
        workspace_write_sessions: 8,
        workspace_write_network_sessions: 6,
    };

    assert_eq!(
        recommendation.summary(),
        "Recommended from 12 recent interactive sessions: workspace-write access; network enabled in 6/8 workspace-write sessions; common directories: /Users/viyatb/code/openai."
    );
}

#[test]
fn filters_codex_worktree_directories_from_recommendations() {
    let tempdir = TempDir::new().expect("tempdir");
    let sessions_root = tempdir.path().join("sessions");
    let current_cwd = tempdir.path().join("worktrees/d003/codex");

    write_rollout(
        &sessions_root,
        "2026/03/13/rollout-2026-03-13T18-03-05-00000000-0000-0000-0000-000000000004.jsonl",
        SessionSource::Cli,
        tempdir.path().join("worktrees/bb2d/codex"),
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![
                AbsolutePathBuf::try_from(tempdir.path().join("worktrees/c84e/codex"))
                    .expect("absolute path"),
                AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/codex")
                    .expect("absolute path"),
            ],
            read_only_access: Default::default(),
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    );

    let recommendation =
        detect_config_wizard_recommendation_in_root(sessions_root, Some(&current_cwd))
            .expect("recommendation");

    assert_eq!(
        recommendation.directories,
        vec![
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/codex").expect("absolute path"),
        ]
    );
}

fn write_rollout(
    sessions_root: &Path,
    relative_path: &str,
    source: SessionSource,
    cwd: PathBuf,
    sandbox_policy: SandboxPolicy,
) {
    let path = sessions_root.join(relative_path);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");

    let session_meta = RolloutLine {
        timestamp: "2026-03-13T18:03:05Z".to_string(),
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: ThreadId::default(),
                timestamp: "2026-03-13T18:03:05Z".to_string(),
                cwd: cwd.clone(),
                originator: "Codex Desktop".to_string(),
                cli_version: "0.1.0".to_string(),
                source,
                ..Default::default()
            },
            git: None,
        }),
    };
    let turn_context = RolloutLine {
        timestamp: "2026-03-13T18:03:06Z".to_string(),
        item: RolloutItem::TurnContext(TurnContextItem {
            turn_id: None,
            trace_id: None,
            cwd,
            current_date: None,
            timezone: None,
            approval_policy: AskForApproval::OnRequest,
            sandbox_policy,
            network: None,
            model: "gpt-5.4".to_string(),
            personality: None,
            collaboration_mode: None,
            realtime_active: None,
            effort: None,
            summary: Default::default(),
            user_instructions: None,
            developer_instructions: None,
            final_output_json_schema: None,
            truncation_policy: None,
        }),
    };

    let payload = format!(
        "{}\n{}\n",
        serde_json::to_string(&session_meta).expect("serialize session meta"),
        serde_json::to_string(&turn_context).expect("serialize turn context")
    );
    fs::write(path, payload).expect("write rollout");
}
