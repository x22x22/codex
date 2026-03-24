use super::*;
use pretty_assertions::assert_eq;

#[test]
fn build_state_with_audit_metadata_threads_metadata_to_state() {
    let spec = NetworkProxySpec {
        config: NetworkProxyConfig::default(),
        constraints: NetworkProxyConstraints::default(),
        hard_deny_allowlist_misses: false,
    };
    let metadata = NetworkProxyAuditMetadata {
        conversation_id: Some("conversation-1".to_string()),
        app_version: Some("1.2.3".to_string()),
        user_account_id: Some("acct-1".to_string()),
        ..NetworkProxyAuditMetadata::default()
    };

    let state = spec
        .build_state_with_audit_metadata(metadata.clone())
        .expect("state should build");
    assert_eq!(state.audit_metadata(), &metadata);
}

#[test]
fn requirements_allowed_domains_are_a_baseline_for_user_allowlist() {
    let mut config = NetworkProxyConfig::default();
    config.network.allowed_domains = vec!["api.example.com".to_string()];
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["*.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::new_read_only_policy(),
    )
    .expect("config should stay within the managed allowlist");

    assert_eq!(
        spec.config.network.allowed_domains,
        vec!["*.example.com".to_string(), "api.example.com".to_string()]
    );
    assert_eq!(
        spec.constraints.allowed_domains,
        Some(vec!["*.example.com".to_string()])
    );
    assert_eq!(spec.constraints.allowlist_expansion_enabled, Some(true));
}

#[test]
fn danger_full_access_keeps_managed_allowlist_and_denylist_fixed() {
    let mut config = NetworkProxyConfig::default();
    config.network.allowed_domains = vec!["evil.com".to_string()];
    config.network.denied_domains = vec!["more-blocked.example.com".to_string()];
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["*.example.com".to_string()]),
        denied_domains: Some(vec!["blocked.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::DangerFullAccess,
    )
    .expect("yolo mode should pin the effective policy to the managed baseline");

    assert_eq!(
        spec.config.network.allowed_domains,
        vec!["*.example.com".to_string()]
    );
    assert_eq!(
        spec.config.network.denied_domains,
        vec!["blocked.example.com".to_string()]
    );
    assert_eq!(spec.constraints.allowlist_expansion_enabled, Some(false));
    assert_eq!(spec.constraints.denylist_expansion_enabled, Some(false));
}

#[test]
fn managed_allowed_domains_only_disables_default_mode_allowlist_expansion() {
    let mut config = NetworkProxyConfig::default();
    config.network.allowed_domains = vec!["api.example.com".to_string()];
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["*.example.com".to_string()]),
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("managed baseline should still load");

    assert_eq!(
        spec.config.network.allowed_domains,
        vec!["*.example.com".to_string()]
    );
    assert_eq!(spec.constraints.allowlist_expansion_enabled, Some(false));
}

#[test]
fn managed_allowed_domains_only_ignores_user_allowlist_and_hard_denies_misses() {
    let mut config = NetworkProxyConfig::default();
    config.network.allowed_domains = vec!["api.example.com".to_string()];
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["managed.example.com".to_string()]),
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("managed-only allowlist should still load");

    assert_eq!(
        spec.config.network.allowed_domains,
        vec!["managed.example.com".to_string()]
    );
    assert_eq!(
        spec.constraints.allowed_domains,
        Some(vec!["managed.example.com".to_string()])
    );
    assert_eq!(spec.constraints.allowlist_expansion_enabled, Some(false));
    assert!(spec.hard_deny_allowlist_misses);
}

#[test]
fn managed_allowed_domains_only_without_managed_allowlist_blocks_all_user_domains() {
    let mut config = NetworkProxyConfig::default();
    config.network.allowed_domains = vec!["api.example.com".to_string()];
    let requirements = NetworkConstraints {
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("managed-only mode should treat missing managed allowlist as empty");

    assert!(spec.config.network.allowed_domains.is_empty());
    assert_eq!(spec.constraints.allowed_domains, Some(Vec::new()));
    assert_eq!(spec.constraints.allowlist_expansion_enabled, Some(false));
    assert!(spec.hard_deny_allowlist_misses);
}

#[test]
fn managed_allowed_domains_only_blocks_all_user_domains_in_full_access_without_managed_list() {
    let mut config = NetworkProxyConfig::default();
    config.network.allowed_domains = vec!["api.example.com".to_string()];
    let requirements = NetworkConstraints {
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::DangerFullAccess,
    )
    .expect("managed-only mode should treat missing managed allowlist as empty");

    assert!(spec.config.network.allowed_domains.is_empty());
    assert_eq!(spec.constraints.allowed_domains, Some(Vec::new()));
    assert_eq!(spec.constraints.allowlist_expansion_enabled, Some(false));
    assert!(spec.hard_deny_allowlist_misses);
}

#[test]
fn yolo_only_enforce_blocklist_is_ignored_outside_full_access() {
    let mut config = NetworkProxyConfig::default();
    config.network.yolo_only_enforce_blocklist = true;

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        None,
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("non-yolo modes should load with blocklist-only disabled");

    assert!(!spec.config.network.yolo_only_enforce_blocklist);
}

#[test]
fn requirements_can_enable_yolo_only_enforce_blocklist_in_full_access() {
    let requirements = NetworkConstraints {
        yolo_only_enforce_blocklist: Some(true),
        denied_domains: Some(vec!["blocked.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        NetworkProxyConfig::default(),
        Some(requirements),
        &SandboxPolicy::DangerFullAccess,
    )
    .expect("requirements blocklist-only mode should load in yolo");

    assert!(spec.config.network.yolo_only_enforce_blocklist);
    assert_eq!(spec.constraints.yolo_only_enforce_blocklist, Some(true));
    assert_eq!(
        spec.config.network.denied_domains,
        vec!["blocked.example.com".to_string()]
    );
}

#[test]
fn requirements_yolo_only_enforce_blocklist_is_ignored_outside_full_access() {
    let requirements = NetworkConstraints {
        yolo_only_enforce_blocklist: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        NetworkProxyConfig::default(),
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("requirements blocklist-only should be disabled outside yolo");

    assert!(!spec.config.network.yolo_only_enforce_blocklist);
}

#[test]
fn managed_allowed_domains_only_disables_user_blocklist_only_yolo_without_explicit_requirement() {
    let mut config = NetworkProxyConfig::default();
    config.network.yolo_only_enforce_blocklist = true;
    config.network.allowed_domains = vec!["user.example.com".to_string()];

    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["managed.example.com".to_string()]),
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::DangerFullAccess,
    )
    .expect("managed allowlist-only should disable user blocklist-only yolo");

    assert!(!spec.config.network.yolo_only_enforce_blocklist);
    assert_eq!(spec.constraints.yolo_only_enforce_blocklist, Some(false));
    assert_eq!(
        spec.config.network.allowed_domains,
        vec!["managed.example.com".to_string()]
    );
    assert!(spec.hard_deny_allowlist_misses);
}

#[test]
fn requirements_denied_domains_are_a_baseline_for_default_mode() {
    let mut config = NetworkProxyConfig::default();
    config.network.denied_domains = vec!["blocked.example.com".to_string()];
    let requirements = NetworkConstraints {
        denied_domains: Some(vec!["managed-blocked.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("default mode should merge managed and user deny entries");

    assert_eq!(
        spec.config.network.denied_domains,
        vec![
            "managed-blocked.example.com".to_string(),
            "blocked.example.com".to_string()
        ]
    );
    assert_eq!(spec.constraints.denylist_expansion_enabled, Some(true));
}
