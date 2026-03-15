use codex_app_server_protocol::AgentMessageDeltaNotification;
use codex_app_server_protocol::CommandExecutionOutputDeltaNotification;
use codex_app_server_protocol::CommandExecutionRequestApprovalParams;
use codex_app_server_protocol::DeprecationNoticeNotification;
use codex_app_server_protocol::DynamicToolCallParams;
use codex_app_server_protocol::ErrorNotification;
use codex_app_server_protocol::FileChangeOutputDeltaNotification;
use codex_app_server_protocol::FileChangeRequestApprovalParams;
use codex_app_server_protocol::HookCompletedNotification;
use codex_app_server_protocol::HookStartedNotification;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemGuardianApprovalReviewCompletedNotification;
use codex_app_server_protocol::ItemGuardianApprovalReviewStartedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::McpServerElicitationRequestParams;
use codex_app_server_protocol::McpToolCallProgressNotification;
use codex_app_server_protocol::ModelReroutedNotification;
use codex_app_server_protocol::PermissionsRequestApprovalParams;
use codex_app_server_protocol::PlanDeltaNotification;
use codex_app_server_protocol::ReasoningSummaryPartAddedNotification;
use codex_app_server_protocol::ReasoningSummaryTextDeltaNotification;
use codex_app_server_protocol::ReasoningTextDeltaNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::TerminalInteractionNotification;
use codex_app_server_protocol::ThreadClosedNotification;
use codex_app_server_protocol::ThreadNameUpdatedNotification;
use codex_app_server_protocol::ThreadRealtimeClosedNotification;
use codex_app_server_protocol::ThreadRealtimeErrorNotification;
use codex_app_server_protocol::ThreadRealtimeItemAddedNotification;
use codex_app_server_protocol::ThreadRealtimeOutputAudioDeltaNotification;
use codex_app_server_protocol::ThreadRealtimeStartedNotification;
use codex_app_server_protocol::ThreadStartedNotification;
use codex_app_server_protocol::ThreadStatusChangedNotification;
use codex_app_server_protocol::ThreadTokenUsageUpdatedNotification;
use codex_app_server_protocol::ToolRequestUserInputParams;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnDiffUpdatedNotification;
use codex_app_server_protocol::TurnPlanUpdatedNotification;
use codex_app_server_protocol::TurnStartedNotification;
use codex_protocol::protocol::SessionConfiguredEvent;

#[derive(Debug, Clone)]
pub(crate) enum ThreadUpdate {
    SessionConfigured(SessionConfiguredEvent),
    ThreadStarted(ThreadStartedNotification),
    ThreadStatusChanged(ThreadStatusChangedNotification),
    ThreadClosed(ThreadClosedNotification),
    ThreadNameUpdated(ThreadNameUpdatedNotification),
    ThreadTokenUsageUpdated(ThreadTokenUsageUpdatedNotification),
    TurnStarted(TurnStartedNotification),
    TurnCompleted(TurnCompletedNotification),
    TurnDiffUpdated(TurnDiffUpdatedNotification),
    TurnPlanUpdated(TurnPlanUpdatedNotification),
    ItemStarted(ItemStartedNotification),
    ItemGuardianApprovalReviewStarted(ItemGuardianApprovalReviewStartedNotification),
    ItemGuardianApprovalReviewCompleted(ItemGuardianApprovalReviewCompletedNotification),
    ItemCompleted(ItemCompletedNotification),
    AgentMessageDelta(AgentMessageDeltaNotification),
    PlanDelta(PlanDeltaNotification),
    ReasoningSummaryTextDelta(ReasoningSummaryTextDeltaNotification),
    ReasoningSummaryPartAdded(ReasoningSummaryPartAddedNotification),
    ReasoningTextDelta(ReasoningTextDeltaNotification),
    TerminalInteraction(TerminalInteractionNotification),
    CommandExecutionOutputDelta(CommandExecutionOutputDeltaNotification),
    FileChangeOutputDelta(FileChangeOutputDeltaNotification),
    McpToolCallProgress(McpToolCallProgressNotification),
    HookStarted(HookStartedNotification),
    HookCompleted(HookCompletedNotification),
    Error(ErrorNotification),
    ModelRerouted(ModelReroutedNotification),
    DeprecationNotice(DeprecationNoticeNotification),
    ThreadRealtimeStarted(ThreadRealtimeStartedNotification),
    ThreadRealtimeItemAdded(ThreadRealtimeItemAddedNotification),
    ThreadRealtimeOutputAudioDelta(ThreadRealtimeOutputAudioDeltaNotification),
    ThreadRealtimeError(ThreadRealtimeErrorNotification),
    ThreadRealtimeClosed(ThreadRealtimeClosedNotification),
    CommandExecutionRequestApproval {
        _request_id: RequestId,
        params: CommandExecutionRequestApprovalParams,
    },
    FileChangeRequestApproval {
        _request_id: RequestId,
        params: FileChangeRequestApprovalParams,
    },
    McpServerElicitationRequest {
        request_id: RequestId,
        params: McpServerElicitationRequestParams,
    },
    PermissionsRequestApproval {
        _request_id: RequestId,
        params: PermissionsRequestApprovalParams,
    },
    ToolRequestUserInput {
        _request_id: RequestId,
        params: ToolRequestUserInputParams,
    },
    DynamicToolCall {
        _request_id: RequestId,
        params: DynamicToolCallParams,
    },
    ThreadRolledBack {
        thread_id: String,
        num_turns: u32,
    },
}

impl ThreadUpdate {
    pub(crate) fn thread_id(&self) -> Option<String> {
        match self {
            Self::SessionConfigured(session) => Some(session.session_id.to_string()),
            Self::ThreadStarted(notification) => Some(notification.thread.id.clone()),
            Self::ThreadStatusChanged(notification) => Some(notification.thread_id.clone()),
            Self::ThreadClosed(notification) => Some(notification.thread_id.clone()),
            Self::ThreadNameUpdated(notification) => Some(notification.thread_id.clone()),
            Self::ThreadTokenUsageUpdated(notification) => Some(notification.thread_id.clone()),
            Self::TurnStarted(notification) => Some(notification.thread_id.clone()),
            Self::TurnCompleted(notification) => Some(notification.thread_id.clone()),
            Self::TurnDiffUpdated(notification) => Some(notification.thread_id.clone()),
            Self::TurnPlanUpdated(notification) => Some(notification.thread_id.clone()),
            Self::ItemStarted(notification) => Some(notification.thread_id.clone()),
            Self::ItemGuardianApprovalReviewStarted(notification) => {
                Some(notification.thread_id.clone())
            }
            Self::ItemGuardianApprovalReviewCompleted(notification) => {
                Some(notification.thread_id.clone())
            }
            Self::ItemCompleted(notification) => Some(notification.thread_id.clone()),
            Self::AgentMessageDelta(notification) => Some(notification.thread_id.clone()),
            Self::PlanDelta(notification) => Some(notification.thread_id.clone()),
            Self::ReasoningSummaryTextDelta(notification) => Some(notification.thread_id.clone()),
            Self::ReasoningSummaryPartAdded(notification) => Some(notification.thread_id.clone()),
            Self::ReasoningTextDelta(notification) => Some(notification.thread_id.clone()),
            Self::TerminalInteraction(notification) => Some(notification.thread_id.clone()),
            Self::CommandExecutionOutputDelta(notification) => Some(notification.thread_id.clone()),
            Self::FileChangeOutputDelta(notification) => Some(notification.thread_id.clone()),
            Self::McpToolCallProgress(notification) => Some(notification.thread_id.clone()),
            Self::HookStarted(notification) => Some(notification.thread_id.clone()),
            Self::HookCompleted(notification) => Some(notification.thread_id.clone()),
            Self::Error(notification) => Some(notification.thread_id.clone()),
            Self::ModelRerouted(notification) => Some(notification.thread_id.clone()),
            Self::ThreadRealtimeStarted(notification) => Some(notification.thread_id.clone()),
            Self::ThreadRealtimeItemAdded(notification) => Some(notification.thread_id.clone()),
            Self::ThreadRealtimeOutputAudioDelta(notification) => {
                Some(notification.thread_id.clone())
            }
            Self::ThreadRealtimeError(notification) => Some(notification.thread_id.clone()),
            Self::ThreadRealtimeClosed(notification) => Some(notification.thread_id.clone()),
            Self::CommandExecutionRequestApproval { params, .. } => Some(params.thread_id.clone()),
            Self::FileChangeRequestApproval { params, .. } => Some(params.thread_id.clone()),
            Self::McpServerElicitationRequest { params, .. } => Some(params.thread_id.clone()),
            Self::PermissionsRequestApproval { params, .. } => Some(params.thread_id.clone()),
            Self::ToolRequestUserInput { params, .. } => Some(params.thread_id.clone()),
            Self::DynamicToolCall { params, .. } => Some(params.thread_id.clone()),
            Self::ThreadRolledBack { thread_id, .. } => Some(thread_id.clone()),
            Self::DeprecationNotice(_) => None,
        }
    }

    pub(crate) fn is_status_refresh_update(&self) -> bool {
        matches!(
            self,
            Self::SessionConfigured(_)
                | Self::TurnStarted(_)
                | Self::ThreadTokenUsageUpdated(_)
                | Self::TurnCompleted(_)
        )
    }

    pub(crate) fn is_thread_closed(&self) -> bool {
        matches!(self, Self::ThreadClosed(_))
    }
}
