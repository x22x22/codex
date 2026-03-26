use super::*;
use codex_network_proxy::NetworkMode;
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
    let user_network = crate::config::permissions::NetworkToml {
        allowed_domains: Some(vec!["api.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["*.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
        Some(requirements),
        &SandboxPolicy::new_read_only_policy(),
    )
    .expect("config should stay within the managed allowlist");

    assert_eq!(
        spec.config.network.allowed_domains,
        vec!["api.example.com".to_string()]
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
    let user_network = crate::config::permissions::NetworkToml {
        allowed_domains: Some(vec!["evil.com".to_string()]),
        denied_domains: Some(vec!["more-blocked.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["*.example.com".to_string()]),
        denied_domains: Some(vec!["blocked.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
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
    let user_network = crate::config::permissions::NetworkToml {
        allowed_domains: Some(vec!["api.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["*.example.com".to_string()]),
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
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
    let user_network = crate::config::permissions::NetworkToml {
        allowed_domains: Some(vec!["api.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        allowed_domains: Some(vec!["managed.example.com".to_string()]),
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
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
    let user_network = crate::config::permissions::NetworkToml {
        allowed_domains: Some(vec!["api.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
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
    let user_network = crate::config::permissions::NetworkToml {
        allowed_domains: Some(vec!["api.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        managed_allowed_domains_only: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
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
fn requirements_denied_domains_are_a_baseline_for_default_mode() {
    let mut config = NetworkProxyConfig::default();
    config.network.denied_domains = vec!["blocked.example.com".to_string()];
    let user_network = crate::config::permissions::NetworkToml {
        denied_domains: Some(vec!["blocked.example.com".to_string()]),
        ..Default::default()
    };
    let requirements = NetworkConstraints {
        denied_domains: Some(vec!["managed-blocked.example.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        config,
        Some(&user_network),
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )
    .expect("default mode should merge managed and user deny entries");

    assert_eq!(
        spec.config.network.denied_domains,
        vec![
            "blocked.example.com".to_string(),
            "managed-blocked.example.com".to_string()
        ]
    );
    assert_eq!(spec.constraints.denylist_expansion_enabled, Some(true));
}

#[test]
fn managed_requirements_preserve_more_restrictive_user_network_settings() -> std::io::Result<()> {
    let mut user_config = NetworkProxyConfig::default();
    user_config.network.enabled = false;
    user_config.network.mode = NetworkMode::Limited;
    user_config.network.allow_upstream_proxy = false;
    user_config.network.dangerously_allow_non_loopback_proxy = false;
    user_config.network.allowed_domains = vec!["api.openai.com".to_string()];
    user_config.network.denied_domains = vec!["tracker.com".to_string(), "evil.com".to_string()];
    user_config.network.allow_unix_sockets = vec!["/tmp/a.sock".to_string()];
    user_config.network.allow_local_binding = false;

    let requirements = NetworkConstraints {
        enabled: Some(true),
        http_port: Some(43128),
        socks_port: Some(43129),
        allow_upstream_proxy: Some(true),
        dangerously_allow_non_loopback_proxy: Some(true),
        dangerously_allow_all_unix_sockets: None,
        allowed_domains: Some(vec!["*.openai.com".to_string()]),
        managed_allowed_domains_only: None,
        denied_domains: Some(vec!["evil.com".to_string()]),
        allow_unix_sockets: Some(vec!["/tmp/a.sock".to_string(), "/tmp/b.sock".to_string()]),
        allow_local_binding: Some(true),
    };

    let user_network = crate::config::permissions::NetworkToml {
        enabled: Some(false),
        allow_upstream_proxy: Some(false),
        dangerously_allow_non_loopback_proxy: Some(false),
        mode: Some(NetworkMode::Limited),
        allowed_domains: Some(vec!["api.openai.com".to_string()]),
        denied_domains: Some(vec!["tracker.com".to_string(), "evil.com".to_string()]),
        allow_unix_sockets: Some(vec!["/tmp/a.sock".to_string()]),
        allow_local_binding: Some(false),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        user_config,
        Some(&user_network),
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )?;

    let mut expected_config = NetworkProxyConfig::default();
    expected_config.network.enabled = false;
    expected_config.network.proxy_url = "http://127.0.0.1:43128".to_string();
    expected_config.network.mode = NetworkMode::Limited;
    expected_config.network.allow_upstream_proxy = false;
    expected_config.network.dangerously_allow_non_loopback_proxy = false;
    expected_config.network.enable_socks5 = true;
    expected_config.network.socks_url = "http://127.0.0.1:43129".to_string();
    expected_config.network.enable_socks5_udp = true;
    expected_config.network.allowed_domains = vec!["api.openai.com".to_string()];
    expected_config.network.denied_domains =
        vec!["tracker.com".to_string(), "evil.com".to_string()];
    expected_config.network.allow_unix_sockets = vec!["/tmp/a.sock".to_string()];
    expected_config.network.allow_local_binding = false;

    assert_eq!(spec.config, expected_config);
    assert_eq!(
        spec.constraints,
        NetworkProxyConstraints {
            enabled: Some(true),
            mode: None,
            allow_upstream_proxy: Some(true),
            dangerously_allow_non_loopback_proxy: Some(true),
            dangerously_allow_all_unix_sockets: None,
            allowed_domains: Some(vec!["*.openai.com".to_string()]),
            allowlist_expansion_enabled: Some(true),
            denied_domains: Some(vec!["evil.com".to_string()]),
            denylist_expansion_enabled: Some(true),
            allow_unix_sockets: Some(vec!["/tmp/a.sock".to_string(), "/tmp/b.sock".to_string()]),
            allow_local_binding: Some(true),
        }
    );

    Ok(())
}

#[test]
fn managed_requirements_clamp_and_union_user_network_settings() -> std::io::Result<()> {
    let mut user_config = NetworkProxyConfig::default();
    user_config.network.enabled = true;
    user_config.network.mode = NetworkMode::Full;
    user_config.network.allow_upstream_proxy = true;
    user_config.network.dangerously_allow_non_loopback_proxy = true;
    user_config.network.denied_domains = vec!["tracker.com".to_string()];
    user_config.network.allow_local_binding = true;

    let requirements = NetworkConstraints {
        enabled: Some(false),
        http_port: None,
        socks_port: None,
        allow_upstream_proxy: Some(false),
        dangerously_allow_non_loopback_proxy: Some(false),
        dangerously_allow_all_unix_sockets: None,
        allowed_domains: None,
        managed_allowed_domains_only: None,
        denied_domains: Some(vec!["evil.com".to_string()]),
        allow_unix_sockets: None,
        allow_local_binding: Some(false),
    };

    let user_network = crate::config::permissions::NetworkToml {
        enabled: Some(true),
        allow_upstream_proxy: Some(true),
        dangerously_allow_non_loopback_proxy: Some(true),
        mode: Some(NetworkMode::Full),
        denied_domains: Some(vec!["tracker.com".to_string()]),
        allow_local_binding: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        user_config,
        Some(&user_network),
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )?;

    let mut expected_config = NetworkProxyConfig::default();
    expected_config.network.enabled = false;
    expected_config.network.mode = NetworkMode::Full;
    expected_config.network.allow_upstream_proxy = false;
    expected_config.network.dangerously_allow_non_loopback_proxy = false;
    expected_config.network.denied_domains =
        vec!["tracker.com".to_string(), "evil.com".to_string()];
    expected_config.network.allow_local_binding = false;

    assert_eq!(spec.config, expected_config);
    assert_eq!(
        spec.constraints,
        NetworkProxyConstraints {
            enabled: Some(false),
            mode: None,
            allow_upstream_proxy: Some(false),
            dangerously_allow_non_loopback_proxy: Some(false),
            dangerously_allow_all_unix_sockets: None,
            allowed_domains: None,
            allowlist_expansion_enabled: None,
            denied_domains: Some(vec!["evil.com".to_string()]),
            denylist_expansion_enabled: Some(true),
            allow_unix_sockets: None,
            allow_local_binding: Some(false),
        }
    );

    Ok(())
}

#[test]
fn managed_requirements_enable_proxy_when_user_omits_network_enabled() -> std::io::Result<()> {
    let user_config = NetworkProxyConfig::default();
    let requirements = NetworkConstraints {
        enabled: Some(true),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        user_config,
        None,
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )?;

    let mut expected_config = NetworkProxyConfig::default();
    expected_config.network.enabled = true;

    assert_eq!(spec.config, expected_config);
    assert_eq!(
        spec.constraints,
        NetworkProxyConstraints {
            enabled: Some(true),
            ..NetworkProxyConstraints::default()
        }
    );

    Ok(())
}

#[test]
fn managed_requirements_seed_allowlist_when_user_omits_it() -> std::io::Result<()> {
    let user_config = NetworkProxyConfig::default();
    let requirements = NetworkConstraints {
        enabled: Some(true),
        allowed_domains: Some(vec!["*.openai.com".to_string()]),
        ..Default::default()
    };

    let spec = NetworkProxySpec::from_config_and_constraints(
        user_config,
        None,
        Some(requirements),
        &SandboxPolicy::new_workspace_write_policy(),
    )?;

    let mut expected_config = NetworkProxyConfig::default();
    expected_config.network.enabled = true;
    expected_config.network.allowed_domains = vec!["*.openai.com".to_string()];

    assert_eq!(spec.config, expected_config);
    assert_eq!(
        spec.constraints,
        NetworkProxyConstraints {
            enabled: Some(true),
            allowed_domains: Some(vec!["*.openai.com".to_string()]),
            allowlist_expansion_enabled: Some(true),
            ..NetworkProxyConstraints::default()
        }
    );

    Ok(())
}
