pub mod cache;
pub mod collaboration_mode_presets;
pub mod model_presets;

/// Convert the client version string to a whole version string.
pub fn client_version_to_whole() -> String {
    format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    )
}
