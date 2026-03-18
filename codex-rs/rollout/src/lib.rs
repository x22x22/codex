#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::path::Path;
use std::path::PathBuf;

mod git_info;
mod path_utils;
pub mod rollout;
pub mod state_db;
mod truncate;

pub(crate) use codex_protocol::protocol;

pub trait StateDbConfig {
    fn codex_home(&self) -> &Path;
    fn sqlite_home(&self) -> &Path;
    fn model_provider_id(&self) -> &str;
}

pub trait RolloutConfig: StateDbConfig {
    fn cwd(&self) -> &Path;
    fn generate_memories(&self) -> bool;
    fn originator(&self) -> String;
}

#[derive(Clone, Debug)]
pub struct StateDbConfigSnapshot {
    codex_home: PathBuf,
    sqlite_home: PathBuf,
    model_provider_id: String,
}

impl StateDbConfigSnapshot {
    pub fn new(config: &(impl StateDbConfig + ?Sized)) -> Self {
        Self {
            codex_home: config.codex_home().to_path_buf(),
            sqlite_home: config.sqlite_home().to_path_buf(),
            model_provider_id: config.model_provider_id().to_string(),
        }
    }

    pub fn from_parts(
        codex_home: PathBuf,
        sqlite_home: PathBuf,
        model_provider_id: String,
    ) -> Self {
        Self {
            codex_home,
            sqlite_home,
            model_provider_id,
        }
    }
}

impl StateDbConfig for StateDbConfigSnapshot {
    fn codex_home(&self) -> &Path {
        self.codex_home.as_path()
    }

    fn sqlite_home(&self) -> &Path {
        self.sqlite_home.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }
}

#[derive(Clone, Debug)]
pub struct RolloutConfigSnapshot {
    state_db: StateDbConfigSnapshot,
    cwd: PathBuf,
    generate_memories: bool,
    originator: String,
}

impl RolloutConfigSnapshot {
    pub fn new(config: &(impl RolloutConfig + ?Sized)) -> Self {
        Self {
            state_db: StateDbConfigSnapshot::new(config),
            cwd: config.cwd().to_path_buf(),
            generate_memories: config.generate_memories(),
            originator: config.originator(),
        }
    }

    pub fn from_parts(
        codex_home: PathBuf,
        sqlite_home: PathBuf,
        cwd: PathBuf,
        model_provider_id: String,
        generate_memories: bool,
        originator: String,
    ) -> Self {
        Self {
            state_db: StateDbConfigSnapshot::from_parts(codex_home, sqlite_home, model_provider_id),
            cwd,
            generate_memories,
            originator,
        }
    }
}

impl StateDbConfig for RolloutConfigSnapshot {
    fn codex_home(&self) -> &Path {
        self.state_db.codex_home()
    }

    fn sqlite_home(&self) -> &Path {
        self.state_db.sqlite_home()
    }

    fn model_provider_id(&self) -> &str {
        self.state_db.model_provider_id()
    }
}

impl RolloutConfig for RolloutConfigSnapshot {
    fn cwd(&self) -> &Path {
        self.cwd.as_path()
    }

    fn generate_memories(&self) -> bool {
        self.generate_memories
    }

    fn originator(&self) -> String {
        self.originator.clone()
    }
}

pub use rollout::ARCHIVED_SESSIONS_SUBDIR;
pub use rollout::INTERACTIVE_SESSION_SOURCES;
pub use rollout::RolloutRecorder;
pub use rollout::RolloutRecorderParams;
pub use rollout::SESSIONS_SUBDIR;
pub use rollout::SessionMeta;
pub use rollout::append_thread_name;
pub use rollout::find_archived_thread_path_by_id_str;
#[allow(deprecated)]
pub use rollout::find_conversation_path_by_id_str;
pub use rollout::find_thread_name_by_id;
pub use rollout::find_thread_path_by_id_str;
pub use rollout::find_thread_path_by_name_str;
pub use rollout::list::Cursor;
pub use rollout::list::ThreadItem;
pub use rollout::list::ThreadSortKey;
pub use rollout::list::ThreadsPage;
pub use rollout::list::parse_cursor;
pub use rollout::list::read_head_for_summary;
pub use rollout::list::read_session_meta_line;
pub use rollout::policy::EventPersistenceMode;
pub use rollout::rollout_date_parts;
pub use rollout::session_index::find_thread_names_by_ids;
