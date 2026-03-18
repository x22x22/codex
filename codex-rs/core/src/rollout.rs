use crate::config::Config;

pub use codex_rollout::ARCHIVED_SESSIONS_SUBDIR;
pub use codex_rollout::INTERACTIVE_SESSION_SOURCES;
pub use codex_rollout::RolloutRecorder;
pub use codex_rollout::RolloutRecorderParams;
pub use codex_rollout::SESSIONS_SUBDIR;
pub use codex_rollout::SessionMeta;
pub use codex_rollout::append_thread_name;
pub use codex_rollout::find_archived_thread_path_by_id_str;
#[allow(deprecated)]
pub use codex_rollout::find_conversation_path_by_id_str;
pub use codex_rollout::find_thread_name_by_id;
pub use codex_rollout::find_thread_path_by_id_str;
pub use codex_rollout::find_thread_path_by_name_str;
pub use codex_rollout::list;
pub use codex_rollout::metadata;
pub use codex_rollout::policy;
pub use codex_rollout::rollout_date_parts;
pub use codex_rollout::session_index;

mod error;
pub(crate) mod truncation;

pub(crate) use error::map_session_init_error;

impl codex_rollout::StateDbConfig for Config {
    fn codex_home(&self) -> &std::path::Path {
        self.codex_home.as_path()
    }

    fn sqlite_home(&self) -> &std::path::Path {
        self.sqlite_home.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }
}

impl codex_rollout::RolloutConfig for Config {
    fn cwd(&self) -> &std::path::Path {
        self.cwd.as_path()
    }

    fn generate_memories(&self) -> bool {
        self.memories.generate_memories
    }

    fn originator(&self) -> String {
        crate::default_client::originator().value
    }
}
