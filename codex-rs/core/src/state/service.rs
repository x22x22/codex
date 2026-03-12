use std::collections::HashMap;
use std::sync::Arc;

use crate::AuthManager;
use crate::RolloutRecorder;
use crate::agent::AgentControl;
use crate::analytics_client::AnalyticsEventsClient;
use crate::client::ModelClient;
use crate::config::StartedNetworkProxy;
use crate::exec_policy::ExecPolicyManager;
use crate::file_watcher::FileWatcher;
use crate::mcp::McpManager;
use crate::mcp_connection_manager::McpConnectionManager;
use crate::models_manager::manager::ModelsManager;
use crate::plugins::PluginsManager;
use crate::skills::SkillsManager;
use crate::state_db::StateDbHandle;
use crate::tools::code_mode::CodeModeProcess;
use crate::tools::code_mode::CodeModeYieldedSession;
use crate::tools::network_approval::NetworkApprovalService;
use crate::tools::runtimes::ExecveSessionApproval;
use crate::tools::sandboxing::ApprovalStore;
use crate::unified_exec::UnifiedExecProcessManager;
use codex_hooks::Hooks;
use codex_otel::SessionTelemetry;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub(crate) struct CodeModeStoreService {
    stored_values: Mutex<HashMap<String, JsonValue>>,
    process: Mutex<Option<Arc<Mutex<CodeModeProcess>>>>,
    yielded_sessions: Mutex<HashMap<i32, CodeModeYieldedSession>>,
    next_session_id: Mutex<i32>,
}

impl Default for CodeModeStoreService {
    fn default() -> Self {
        Self {
            stored_values: Mutex::new(HashMap::new()),
            process: Mutex::new(None),
            yielded_sessions: Mutex::new(HashMap::new()),
            next_session_id: Mutex::new(1),
        }
    }
}

impl CodeModeStoreService {
    pub(crate) async fn stored_values(&self) -> HashMap<String, JsonValue> {
        self.stored_values.lock().await.clone()
    }

    pub(crate) async fn replace_stored_values(&self, values: HashMap<String, JsonValue>) {
        *self.stored_values.lock().await = values;
    }

    pub(crate) async fn store_process(&self, process: Arc<Mutex<CodeModeProcess>>) {
        *self.process.lock().await = Some(process);
    }

    pub(crate) async fn process(&self) -> Option<Arc<Mutex<CodeModeProcess>>> {
        self.process.lock().await.clone()
    }

    pub(crate) async fn allocate_session_id(&self) -> i32 {
        let mut next_session_id = self.next_session_id.lock().await;
        let session_id = *next_session_id;
        *next_session_id = next_session_id.saturating_add(1);
        session_id
    }

    pub(crate) async fn store_yielded_session(&self, yielded_session: CodeModeYieldedSession) {
        self.yielded_sessions
            .lock()
            .await
            .insert(yielded_session.session_id, yielded_session);
    }

    pub(crate) async fn take_yielded_session(
        &self,
        session_id: i32,
    ) -> Option<CodeModeYieldedSession> {
        self.yielded_sessions.lock().await.remove(&session_id)
    }
}

pub(crate) struct SessionServices {
    pub(crate) mcp_connection_manager: Arc<RwLock<McpConnectionManager>>,
    pub(crate) mcp_startup_cancellation_token: Mutex<CancellationToken>,
    pub(crate) unified_exec_manager: UnifiedExecProcessManager,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) shell_zsh_path: Option<PathBuf>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) main_execve_wrapper_exe: Option<PathBuf>,
    pub(crate) analytics_events_client: AnalyticsEventsClient,
    pub(crate) hooks: Hooks,
    pub(crate) rollout: Mutex<Option<RolloutRecorder>>,
    pub(crate) user_shell: Arc<crate::shell::Shell>,
    pub(crate) shell_snapshot_tx: watch::Sender<Option<Arc<crate::shell_snapshot::ShellSnapshot>>>,
    pub(crate) show_raw_agent_reasoning: bool,
    pub(crate) exec_policy: ExecPolicyManager,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) models_manager: Arc<ModelsManager>,
    pub(crate) session_telemetry: SessionTelemetry,
    pub(crate) tool_approvals: Mutex<ApprovalStore>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) execve_session_approvals: RwLock<HashMap<AbsolutePathBuf, ExecveSessionApproval>>,
    pub(crate) skills_manager: Arc<SkillsManager>,
    pub(crate) plugins_manager: Arc<PluginsManager>,
    pub(crate) mcp_manager: Arc<McpManager>,
    pub(crate) file_watcher: Arc<FileWatcher>,
    pub(crate) agent_control: AgentControl,
    pub(crate) network_proxy: Option<StartedNetworkProxy>,
    pub(crate) network_approval: Arc<NetworkApprovalService>,
    pub(crate) state_db: Option<StateDbHandle>,
    /// Session-scoped model client shared across turns.
    pub(crate) model_client: ModelClient,
    pub(crate) code_mode_store: CodeModeStoreService,
}
