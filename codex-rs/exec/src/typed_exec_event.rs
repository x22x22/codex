use codex_app_server_protocol::ConfigWarningNotification;
use codex_app_server_protocol::DeprecationNoticeNotification;
use codex_app_server_protocol::ErrorNotification;
use codex_app_server_protocol::HookCompletedNotification;
use codex_app_server_protocol::HookStartedNotification;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::ModelReroutedNotification;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadTokenUsageUpdatedNotification;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnDiffUpdatedNotification;
use codex_app_server_protocol::TurnPlanUpdatedNotification;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) enum TypedExecEvent {
    Warning(String),
    ConfigWarning(ConfigWarningNotification),
    Error(ErrorNotification),
    DeprecationNotice(DeprecationNoticeNotification),
    HookStarted(HookStartedNotification),
    HookCompleted(HookCompletedNotification),
    ItemStarted(ItemStartedNotification),
    ItemCompleted(ItemCompletedNotification),
    ModelRerouted(ModelReroutedNotification),
    ThreadTokenUsageUpdated(ThreadTokenUsageUpdatedNotification),
    TurnCompleted(TurnCompletedNotification),
    TurnDiffUpdated(TurnDiffUpdatedNotification),
    TurnPlanUpdated(TurnPlanUpdatedNotification),
    TurnStarted,
}

impl TypedExecEvent {
    pub(crate) fn from_server_notification(
        notification: ServerNotification,
    ) -> Option<TypedExecEvent> {
        match notification {
            ServerNotification::ConfigWarning(notification) => {
                Some(TypedExecEvent::ConfigWarning(notification))
            }
            ServerNotification::DeprecationNotice(notification) => {
                Some(TypedExecEvent::DeprecationNotice(notification))
            }
            ServerNotification::Error(notification) => Some(TypedExecEvent::Error(notification)),
            ServerNotification::HookCompleted(notification) => {
                Some(TypedExecEvent::HookCompleted(notification))
            }
            ServerNotification::HookStarted(notification) => {
                Some(TypedExecEvent::HookStarted(notification))
            }
            ServerNotification::ItemCompleted(notification) => {
                Some(TypedExecEvent::ItemCompleted(notification))
            }
            ServerNotification::ItemStarted(notification) => {
                Some(TypedExecEvent::ItemStarted(notification))
            }
            ServerNotification::ModelRerouted(notification) => {
                Some(TypedExecEvent::ModelRerouted(notification))
            }
            ServerNotification::ThreadTokenUsageUpdated(notification) => {
                Some(TypedExecEvent::ThreadTokenUsageUpdated(notification))
            }
            ServerNotification::TurnCompleted(notification) => {
                Some(TypedExecEvent::TurnCompleted(notification))
            }
            ServerNotification::TurnDiffUpdated(notification) => {
                Some(TypedExecEvent::TurnDiffUpdated(notification))
            }
            ServerNotification::TurnPlanUpdated(notification) => {
                Some(TypedExecEvent::TurnPlanUpdated(notification))
            }
            ServerNotification::TurnStarted(_) => Some(TypedExecEvent::TurnStarted),
            _ => None,
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "session mapping keeps explicit fields"
)]
pub(crate) fn session_configured_from_thread_response(
    thread_id: &str,
    thread_name: Option<String>,
    rollout_path: Option<PathBuf>,
    model: String,
    model_provider_id: String,
    service_tier: Option<ServiceTier>,
    approval_policy: AskForApproval,
    approvals_reviewer: ApprovalsReviewer,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    reasoning_effort: Option<ReasoningEffort>,
) -> Result<SessionConfiguredEvent, String> {
    let session_id = ThreadId::from_string(thread_id)
        .map_err(|err| format!("thread id `{thread_id}` is invalid: {err}"))?;

    Ok(SessionConfiguredEvent {
        session_id,
        forked_from_id: None,
        thread_name,
        model,
        model_provider_id,
        service_tier,
        approval_policy,
        approvals_reviewer,
        sandbox_policy,
        cwd,
        reasoning_effort,
        history_log_id: 0,
        history_entry_count: 0,
        initial_messages: None,
        network_proxy: None,
        rollout_path,
    })
}
