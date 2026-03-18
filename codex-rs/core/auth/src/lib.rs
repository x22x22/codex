//! Auth storage backend for Codex CLI credentials.
//!
//! This crate provides the storage layer for auth.json (file, keyring, auto, ephemeral)
//! and the AuthDotJson / AuthCredentialsStoreMode types. The higher-level auth logic
//! (CodexAuth, AuthManager, token refresh) lives in codex-core.

pub mod storage;

pub use storage::AuthCredentialsStoreMode;
pub use storage::AuthDotJson;
pub use storage::AuthStorageBackend;
pub use storage::FileAuthStorage;
pub use storage::create_auth_storage;
pub use storage::get_auth_file;
