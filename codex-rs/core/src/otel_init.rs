use crate::AuthManager;
use crate::CodexAuth;
use crate::config::Config;
use crate::config::types::OtelExporterKind as Kind;
use crate::config::types::OtelHttpProtocol as Protocol;
use crate::default_client::originator;
use crate::features::Feature;
use codex_otel::config::OtelExporter;
use codex_otel::config::OtelHttpProtocol;
use codex_otel::config::OtelSettings;
use codex_otel::config::OtelTlsConfig as OtelTlsSettings;
use codex_otel::otel_provider::OtelProvider;
use std::collections::HashMap;
use std::error::Error;

#[derive(Default)]
pub struct EnterpriseAuditOtelProviderInit {
    pub provider: Option<OtelProvider>,
    pub warning: Option<String>,
}

/// Build an OpenTelemetry provider from the app Config.
///
/// Returns `None` when OTEL export is disabled.
pub fn build_provider(
    config: &Config,
    service_version: &str,
    service_name_override: Option<&str>,
    default_analytics_enabled: bool,
) -> Result<Option<OtelProvider>, Box<dyn Error>> {
    let to_otel_exporter = |kind: &Kind| match kind {
        Kind::None => OtelExporter::None,
        Kind::Statsig => OtelExporter::Statsig,
        Kind::OtlpHttp {
            endpoint,
            headers,
            protocol,
            tls,
        } => {
            let protocol = match protocol {
                Protocol::Json => OtelHttpProtocol::Json,
                Protocol::Binary => OtelHttpProtocol::Binary,
            };

            OtelExporter::OtlpHttp {
                endpoint: endpoint.clone(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                protocol,
                tls: tls.as_ref().map(|config| OtelTlsSettings {
                    ca_certificate: config.ca_certificate.clone(),
                    client_certificate: config.client_certificate.clone(),
                    client_private_key: config.client_private_key.clone(),
                }),
            }
        }
        Kind::OtlpGrpc {
            endpoint,
            headers,
            tls,
        } => OtelExporter::OtlpGrpc {
            endpoint: endpoint.clone(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            tls: tls.as_ref().map(|config| OtelTlsSettings {
                ca_certificate: config.ca_certificate.clone(),
                client_certificate: config.client_certificate.clone(),
                client_private_key: config.client_private_key.clone(),
            }),
        },
    };

    let exporter = to_otel_exporter(&config.otel.exporter);
    let trace_exporter = to_otel_exporter(&config.otel.trace_exporter);
    let metrics_exporter = if config
        .analytics_enabled
        .unwrap_or(default_analytics_enabled)
    {
        to_otel_exporter(&config.otel.metrics_exporter)
    } else {
        OtelExporter::None
    };

    let originator = originator();
    let service_name = service_name_override.unwrap_or(originator.value.as_str());
    let runtime_metrics = config.features.enabled(Feature::RuntimeMetrics);

    OtelProvider::from(&OtelSettings {
        service_name: service_name.to_string(),
        service_version: service_version.to_string(),
        codex_home: config.codex_home.clone(),
        environment: config.otel.environment.to_string(),
        exporter,
        trace_exporter,
        metrics_exporter,
        runtime_metrics,
    })
}

/// Build an additive logs-only OpenTelemetry provider for enterprise audit export.
///
/// Returns an empty result when the managed requirement or feature gate is
/// disabled. When enterprise audit is enabled but no auth token is available,
/// returns a warning and skips the audit exporter so normal startup can
/// continue.
pub fn build_enterprise_audit_provider(
    config: &Config,
    service_version: &str,
    service_name_override: Option<&str>,
) -> Result<EnterpriseAuditOtelProviderInit, Box<dyn Error>> {
    let auth_manager = AuthManager::new(
        config.codex_home.clone(),
        true,
        config.cli_auth_credentials_store_mode,
    );
    let auth = auth_manager.auth_cached();
    build_enterprise_audit_provider_for_auth(
        config,
        service_version,
        service_name_override,
        auth.as_ref(),
    )
}

fn build_enterprise_audit_provider_for_auth(
    config: &Config,
    service_version: &str,
    service_name_override: Option<&str>,
    auth: Option<&CodexAuth>,
) -> Result<EnterpriseAuditOtelProviderInit, Box<dyn Error>> {
    if !config.enterprise_audit_enabled() || !config.features.enabled(Feature::EnterpriseAudit) {
        return Ok(EnterpriseAuditOtelProviderInit::default());
    }

    let Some(headers) = enterprise_audit_http_headers(auth) else {
        return Ok(EnterpriseAuditOtelProviderInit {
            provider: None,
            warning: Some(
                "Enterprise audit log export is enabled, but no usable auth token is available; skipping the enterprise audit exporter."
                    .to_string(),
            ),
        });
    };

    let originator = originator();
    let service_name = service_name_override.unwrap_or(originator.value.as_str());
    let provider = OtelProvider::from(&OtelSettings {
        service_name: service_name.to_string(),
        service_version: service_version.to_string(),
        codex_home: config.codex_home.clone(),
        environment: config.otel.environment.to_string(),
        exporter: OtelExporter::OtlpHttp {
            endpoint: enterprise_audit_url(&config.chatgpt_base_url),
            headers,
            protocol: OtelHttpProtocol::Json,
            tls: None,
        },
        trace_exporter: OtelExporter::None,
        metrics_exporter: OtelExporter::None,
        runtime_metrics: false,
    })?;

    Ok(EnterpriseAuditOtelProviderInit {
        provider,
        warning: None,
    })
}

fn enterprise_audit_http_headers(auth: Option<&CodexAuth>) -> Option<HashMap<String, String>> {
    let token = auth.and_then(enterprise_audit_bearer_token)?;

    let mut headers = HashMap::from([("Authorization".to_string(), format!("Bearer {token}"))]);
    if let Some(account_id) = auth.and_then(CodexAuth::get_account_id) {
        headers.insert("ChatGPT-Account-Id".to_string(), account_id);
    }

    Some(headers)
}

fn enterprise_audit_bearer_token(auth: &CodexAuth) -> Option<String> {
    let token = auth.get_token().ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn normalize_enterprise_audit_base_url(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .trim_end_matches("/backend-api")
        .to_string()
}

fn enterprise_audit_url(base_url: &str) -> String {
    let base_url = normalize_enterprise_audit_base_url(base_url);
    if base_url.contains("/api/codex") {
        format!("{base_url}/audit")
    } else {
        format!("{base_url}/api/codex/audit")
    }
}

/// Filter predicate for exporting only Codex-owned events via OTEL.
/// Keeps events that originated from codex_otel module
pub fn codex_export_filter(meta: &tracing::Metadata<'_>) -> bool {
    meta.target().starts_with("codex_otel")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthCredentialsStoreMode;
    use crate::auth::login_with_api_key;
    use crate::config_loader::ConfigLayerStack;
    use crate::config_loader::ConfigRequirementsToml;
    use pretty_assertions::assert_eq;

    fn test_config_with_enterprise_audit(enabled: Option<bool>, feature_enabled: bool) -> Config {
        let mut config = crate::config::test_config();
        config
            .features
            .set_enabled(Feature::EnterpriseAudit, feature_enabled)
            .expect("toggle enterprise audit feature");
        config.config_layer_stack = ConfigLayerStack::new(
            Vec::new(),
            Default::default(),
            ConfigRequirementsToml {
                enterprise_audit_enabled: enabled,
                ..Default::default()
            },
        )
        .expect("config layer stack");
        config
    }

    #[test]
    fn enterprise_audit_url_maps_backend_api_bases() {
        assert_eq!(
            enterprise_audit_url("https://chatgpt.com/backend-api/"),
            "https://chatgpt.com/api/codex/audit"
        );
        assert_eq!(
            enterprise_audit_url("https://chatgpt.com"),
            "https://chatgpt.com/api/codex/audit"
        );
    }

    #[test]
    fn enterprise_audit_url_maps_codex_api_bases() {
        assert_eq!(
            enterprise_audit_url("https://example.com/api/codex"),
            "https://example.com/api/codex/audit"
        );
        assert_eq!(
            enterprise_audit_url("https://example.com"),
            "https://example.com/api/codex/audit"
        );
    }

    #[test]
    fn enterprise_audit_provider_is_disabled_when_requirement_is_unset() {
        let config = test_config_with_enterprise_audit(None, true);

        let init = build_enterprise_audit_provider_for_auth(&config, "test-version", None, None)
            .expect("enterprise audit init");

        assert!(init.provider.is_none());
        assert_eq!(init.warning, None);
    }

    #[test]
    fn enterprise_audit_provider_is_disabled_when_feature_flag_is_off() {
        let config = test_config_with_enterprise_audit(Some(true), false);

        let init = build_enterprise_audit_provider_for_auth(&config, "test-version", None, None)
            .expect("enterprise audit init");

        assert!(init.provider.is_none());
        assert_eq!(init.warning, None);
    }

    #[test]
    fn enterprise_audit_provider_uses_chatgpt_auth_when_available() {
        let config = test_config_with_enterprise_audit(Some(true), true);
        let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();

        let init =
            build_enterprise_audit_provider_for_auth(&config, "test-version", None, Some(&auth))
                .expect("enterprise audit init");

        assert!(init.provider.is_some());
        assert_eq!(init.warning, None);
    }

    #[test]
    fn enterprise_audit_provider_uses_api_key_when_that_is_the_active_auth() {
        let config = test_config_with_enterprise_audit(Some(true), true);
        let auth = CodexAuth::from_api_key("sk-test");

        let init =
            build_enterprise_audit_provider_for_auth(&config, "test-version", None, Some(&auth))
                .expect("enterprise audit init");

        assert!(init.provider.is_some());
        assert_eq!(init.warning, None);
    }

    #[test]
    fn enterprise_audit_provider_warns_when_no_auth_is_available() {
        let config = test_config_with_enterprise_audit(Some(true), true);

        let init = build_enterprise_audit_provider_for_auth(&config, "test-version", None, None)
            .expect("enterprise audit init");

        assert!(init.provider.is_none());
        assert_eq!(
            init.warning.as_deref(),
            Some(
                "Enterprise audit log export is enabled, but no usable auth token is available; skipping the enterprise audit exporter."
            )
        );
    }

    #[test]
    fn enterprise_audit_provider_loads_api_key_from_auth_storage() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        login_with_api_key(tempdir.path(), "sk-stored", AuthCredentialsStoreMode::File)
            .expect("persist api key");

        let mut config = test_config_with_enterprise_audit(Some(true), true);
        config.codex_home = tempdir.path().to_path_buf();
        config.cli_auth_credentials_store_mode = AuthCredentialsStoreMode::File;

        let init = build_enterprise_audit_provider(&config, "test-version", None)
            .expect("enterprise audit init");

        assert!(init.provider.is_some());
        assert_eq!(init.warning, None);
    }
}
