pub mod manager;
pub mod model_info;
pub mod cache {
    pub use codex_models::models_manager::cache::*;
}
pub mod collaboration_mode_presets {
    pub use codex_models::models_manager::collaboration_mode_presets::*;
}
pub mod model_presets {
    pub use codex_models::models_manager::model_presets::*;
}

/// Convert the client version string to a whole version string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    codex_models::models_manager::client_version_to_whole()
}
