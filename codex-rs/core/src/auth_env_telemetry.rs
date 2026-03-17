use crate::auth::CODEX_API_KEY_ENV_VAR;
use crate::auth::OPENAI_API_KEY_ENV_VAR;
use crate::auth::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;
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

pub(crate) fn collect_auth_env_telemetry(
    provider: &ModelProviderInfo,
    codex_api_key_env_enabled: bool,
) -> AuthEnvTelemetry {
    AuthEnvTelemetry {
        openai_api_key_env_present: env_var_present(OPENAI_API_KEY_ENV_VAR),
        codex_api_key_env_present: env_var_present(CODEX_API_KEY_ENV_VAR),
        codex_api_key_env_enabled,
        provider_env_key_name: provider.env_key.clone(),
        provider_env_key_present: provider.env_key.as_deref().map(env_var_present),
        refresh_token_url_override_present: env_var_present(REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR),
    }
}

fn env_var_present(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => !value.trim().is_empty(),
        Err(std::env::VarError::NotUnicode(_)) => true,
        Err(std::env::VarError::NotPresent) => false,
    }
}
