use std::path::PathBuf;

use codex_cloud_requirements::cloud_requirements_loader;
use codex_core::AuthManager;
use codex_core::auth::AuthCredentialsStoreMode;
use codex_core::config::Config;
use codex_core::config_loader::CloudRequirementsLoader;

pub fn shared_cloud_requirements_loader(
    codex_home: PathBuf,
    enable_codex_api_key_env: bool,
    credentials_store_mode: AuthCredentialsStoreMode,
    chatgpt_base_url: String,
) -> CloudRequirementsLoader {
    let auth_manager = AuthManager::shared(
        codex_home.clone(),
        enable_codex_api_key_env,
        credentials_store_mode,
    );
    cloud_requirements_loader(auth_manager, chatgpt_base_url, codex_home)
}

pub fn shared_cloud_requirements_loader_for_config(
    config: &Config,
    enable_codex_api_key_env: bool,
) -> CloudRequirementsLoader {
    shared_cloud_requirements_loader(
        config.codex_home.clone(),
        enable_codex_api_key_env,
        config.cli_auth_credentials_store_mode,
        config.chatgpt_base_url.clone(),
    )
}
