use std::path::PathBuf;

pub const CODEX_OPENAI_UNIX_SOCKET_ENV_VAR: &str = "CODEX_OPENAI_UNIX_SOCKET";

pub fn openai_unix_socket_path() -> Option<PathBuf> {
    std::env::var(CODEX_OPENAI_UNIX_SOCKET_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
