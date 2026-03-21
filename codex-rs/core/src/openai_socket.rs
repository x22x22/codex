use std::ffi::OsString;
use std::path::PathBuf;

pub const CODEX_OPENAI_UNIX_SOCKET_ENV_VAR: &str = "CODEX_OPENAI_UNIX_SOCKET";
pub const CODEX_USE_AGENT_AUTH_PROXY_ENV_VAR: &str = "CODEX_USE_AGENT_AUTH_PROXY";

pub fn openai_unix_socket_path() -> Option<PathBuf> {
    std::env::var(CODEX_OPENAI_UNIX_SOCKET_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn should_route_via_openai_socket_proxy() -> bool {
    openai_unix_socket_path().is_some()
        || should_use_agent_auth_proxy_env(std::env::var_os(CODEX_USE_AGENT_AUTH_PROXY_ENV_VAR))
}

fn should_use_agent_auth_proxy_env(value: Option<OsString>) -> bool {
    value
        .and_then(|raw| raw.into_string().ok())
        .map(|raw| raw.trim().to_ascii_lowercase())
        .is_some_and(|value| value == "1" || value == "true" || value == "yes")
}

#[cfg(test)]
mod tests {
    use super::should_use_agent_auth_proxy_env;
    use std::ffi::OsString;

    #[test]
    fn agent_auth_proxy_env_accepts_truthy_values() {
        assert!(should_use_agent_auth_proxy_env(Some(OsString::from("1"))));
        assert!(should_use_agent_auth_proxy_env(Some(OsString::from(
            "true"
        ))));
        assert!(should_use_agent_auth_proxy_env(Some(OsString::from("YES"))));
    }

    #[test]
    fn agent_auth_proxy_env_rejects_missing_or_falsey_values() {
        assert!(!should_use_agent_auth_proxy_env(None));
        assert!(!should_use_agent_auth_proxy_env(Some(OsString::from(""))));
        assert!(!should_use_agent_auth_proxy_env(Some(OsString::from("0"))));
        assert!(!should_use_agent_auth_proxy_env(Some(OsString::from(
            "false"
        ))));
    }
}
