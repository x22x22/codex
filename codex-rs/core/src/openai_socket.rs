use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

pub const CODEX_OPENAI_UNIX_SOCKET_ENV_VAR: &str = "CODEX_OPENAI_UNIX_SOCKET";
pub const CODEX_USE_AGENT_AUTH_PROXY_ENV_VAR: &str = "CODEX_USE_AGENT_AUTH_PROXY";

pub fn openai_unix_socket_path() -> Option<PathBuf> {
    std::env::var(CODEX_OPENAI_UNIX_SOCKET_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(default_android_socket_path)
}

pub fn should_route_via_codexd() -> bool {
    (openai_unix_socket_path().is_some()
        || should_use_agent_auth_proxy_env(std::env::var_os(CODEX_USE_AGENT_AUTH_PROXY_ENV_VAR)))
        && !is_codexd_process()
}

fn is_codexd_process() -> bool {
    std::env::args_os()
        .next()
        .and_then(|arg0| {
            Path::new(&arg0)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .is_some_and(|arg0| arg0.contains("codexd"))
}

#[cfg(target_os = "android")]
fn default_android_socket_path() -> Option<PathBuf> {
    const CANDIDATES: [&str; 2] = [
        "/data/data/com.openai.codexd/files/codexd.sock",
        "/data/user/0/com.openai.codexd/files/codexd.sock",
    ];

    CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}

#[cfg(not(target_os = "android"))]
fn default_android_socket_path() -> Option<PathBuf> {
    None
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
