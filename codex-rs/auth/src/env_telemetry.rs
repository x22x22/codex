use crate::CODEX_API_KEY_ENV_VAR;
use crate::OPENAI_API_KEY_ENV_VAR;
use crate::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthEnvTelemetry {
    pub openai_api_key_env_present: bool,
    pub codex_api_key_env_present: bool,
    pub codex_api_key_env_enabled: bool,
    pub provider_env_key_name: Option<String>,
    pub provider_env_key_present: Option<bool>,
    pub refresh_token_url_override_present: bool,
}

pub fn collect_auth_env_telemetry(
    provider_env_key_configured: bool,
    provider_env_key: Option<&str>,
    codex_api_key_env_enabled: bool,
) -> AuthEnvTelemetry {
    AuthEnvTelemetry {
        openai_api_key_env_present: env_var_present(OPENAI_API_KEY_ENV_VAR),
        codex_api_key_env_present: env_var_present(CODEX_API_KEY_ENV_VAR),
        codex_api_key_env_enabled,
        provider_env_key_name: provider_env_key_configured.then(|| "configured".to_string()),
        provider_env_key_present: provider_env_key.map(env_var_present),
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn collect_auth_env_telemetry_buckets_provider_env_key_name() {
        let telemetry = collect_auth_env_telemetry(true, Some("sk-should-not-leak"), false);

        assert_eq!(
            telemetry.provider_env_key_name,
            Some("configured".to_string())
        );
    }
}
