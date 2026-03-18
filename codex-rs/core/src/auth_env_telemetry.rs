use codex_otel::AuthEnvTelemetryMetadata;

use crate::model_provider_info::ModelProviderInfo;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AuthEnvTelemetry {
    pub(crate) openai_api_key_env_present: bool,
    pub(crate) codex_api_key_env_present: bool,
    pub(crate) codex_api_key_env_enabled: bool,
    pub(crate) provider_env_key_name: Option<String>,
    pub(crate) provider_env_key_present: Option<bool>,
    pub(crate) refresh_token_url_override_present: bool,
}

impl AuthEnvTelemetry {
    pub(crate) fn to_otel_metadata(&self) -> AuthEnvTelemetryMetadata {
        AuthEnvTelemetryMetadata {
            openai_api_key_env_present: self.openai_api_key_env_present,
            codex_api_key_env_present: self.codex_api_key_env_present,
            codex_api_key_env_enabled: self.codex_api_key_env_enabled,
            provider_env_key_name: self.provider_env_key_name.clone(),
            provider_env_key_present: self.provider_env_key_present,
            refresh_token_url_override_present: self.refresh_token_url_override_present,
        }
    }
}

pub(crate) fn collect_auth_env_telemetry(
    provider: &ModelProviderInfo,
    codex_api_key_env_enabled: bool,
) -> AuthEnvTelemetry {
    let telemetry = codex_auth::collect_auth_env_telemetry(
        provider.env_key.is_some(),
        provider.env_key.as_deref(),
        codex_api_key_env_enabled,
    );
    AuthEnvTelemetry {
        openai_api_key_env_present: telemetry.openai_api_key_env_present,
        codex_api_key_env_present: telemetry.codex_api_key_env_present,
        codex_api_key_env_enabled: telemetry.codex_api_key_env_enabled,
        provider_env_key_name: telemetry.provider_env_key_name,
        provider_env_key_present: telemetry.provider_env_key_present,
        refresh_token_url_override_present: telemetry.refresh_token_url_override_present,
    }
}
