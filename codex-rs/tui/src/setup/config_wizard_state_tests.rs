use super::*;
use codex_core::config::ConfigBuilder;
use insta::assert_snapshot;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn preview_toml_writes_legacy_workspace_write_config() {
    let state = ConfigWizardState {
        access_mode: ConfigWizardAccessMode::WorkspaceWrite,
        writable_roots: vec![
            AbsolutePathBuf::from_absolute_path("/tmp/pip-cache").expect("absolute path"),
            AbsolutePathBuf::from_absolute_path("/tmp/ollama").expect("absolute path"),
        ],
        network_access: true,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: false,
        recommendation: None,
    };

    assert_snapshot!(state.preview_toml());
}

#[test]
fn build_apply_request_clears_permissions_profiles() {
    let state = ConfigWizardState {
        access_mode: ConfigWizardAccessMode::ReadOnly,
        writable_roots: Vec::new(),
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
        recommendation: None,
    };

    let request = state
        .build_apply_request()
        .expect("expected apply request to build");
    assert_eq!(request.edits.len(), 5);
    assert!(matches!(
        &request.edits[0],
        CoreConfigEdit::ClearPath { segments }
            if segments == &vec!["default_permissions".to_string()]
    ));
    assert!(matches!(
        &request.edits[1],
        CoreConfigEdit::ClearPath { segments }
            if segments == &vec!["permissions".to_string()]
    ));
    assert!(matches!(
        &request.edits[2],
        CoreConfigEdit::ClearPath { segments }
            if segments == &vec!["sandbox_workspace_write".to_string()]
    ));
    match &request.edits[3] {
        CoreConfigEdit::SetPath { segments, value } => {
            assert_eq!(segments, &vec!["approval_policy".to_string()]);
            assert_eq!(value.to_string().trim(), "\"on-request\"");
        }
        edit => panic!("unexpected edit: {edit:?}"),
    }
    match &request.edits[4] {
        CoreConfigEdit::SetPath { segments, value } => {
            assert_eq!(segments, &vec!["sandbox_mode".to_string()]);
            assert_eq!(value.to_string().trim(), "\"read-only\"");
        }
        edit => panic!("unexpected edit: {edit:?}"),
    }
}

#[test]
fn parse_writable_roots_rejects_relative_paths() {
    let err = parse_writable_roots("./cache").expect_err("relative paths should be rejected");
    assert_eq!(
        err.to_string(),
        "directory `./cache` must be an absolute path"
    );
}

#[test]
fn access_mode_subtitle_includes_recommendation_summary() {
    let state = ConfigWizardState {
        access_mode: ConfigWizardAccessMode::WorkspaceWrite,
        writable_roots: vec![
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/openai")
                .expect("absolute path"),
        ],
        network_access: true,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
        recommendation: Some(ConfigWizardRecommendation {
            access_mode: SandboxMode::WorkspaceWrite,
            network_access: true,
            directories: vec![
                AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/openai")
                    .expect("absolute path"),
            ],
            sampled_sessions: 12,
            workspace_write_sessions: 8,
            workspace_write_network_sessions: 6,
        }),
    };

    assert_snapshot!(state.access_mode_subtitle());
}

#[tokio::test]
async fn detect_with_config_uses_current_workspace_write_settings() {
    let tempdir = TempDir::new().expect("tempdir");
    let codex_home = tempdir.path().join("codex-home");
    let workspace = tempdir.path().join("workspace");
    let extra_dir = tempdir.path().join("shared");
    std::fs::create_dir_all(&codex_home).expect("codex home");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&extra_dir).expect("extra dir");
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            r#"
sandbox_mode = "workspace-write"

[sandbox_workspace_write]
writable_roots = ["{}"]
network_access = true
exclude_tmpdir_env_var = true
exclude_slash_tmp = true
"#,
            extra_dir.display()
        ),
    )
    .expect("config");

    let config = ConfigBuilder::default()
        .codex_home(codex_home)
        .fallback_cwd(Some(workspace))
        .build()
        .await
        .expect("config");

    let state = ConfigWizardState::detect_with_config(&config);

    assert_eq!(state.access_mode, ConfigWizardAccessMode::WorkspaceWrite);
    assert_eq!(
        state.writable_roots,
        vec![AbsolutePathBuf::try_from(extra_dir).expect("absolute path")]
    );
    assert!(state.network_access);
    assert!(state.exclude_tmpdir_env_var);
    assert!(state.exclude_slash_tmp);
}
