use codex_git_utils::collect_git_info;
use codex_git_utils::get_git_repo_root;
use codex_login::AuthManager;
use codex_login::default_client::create_client;
use codex_login::default_client::originator;
use codex_plugin::PluginTelemetryMetadata;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SkillScope;
use codex_protocol::protocol::SubAgentSource;
use serde::Serialize;
use sha1::Digest;
use sha1::Sha1;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct TrackEventsContext {
    pub model_slug: String,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Clone)]
pub struct CodexTurnEvent {
    pub submission_type: Option<TurnSubmissionType>,
    pub model_provider: String,
    pub sandbox_policy: SandboxPolicy,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<ReasoningSummary>,
    pub service_tier: Option<ServiceTier>,
    pub approval_policy: AskForApproval,
    pub approvals_reviewer: ApprovalsReviewer,
    pub sandbox_network_access: bool,
    pub collaboration_mode: ModeKind,
    pub personality: Option<Personality>,
    pub num_input_images: usize,
    pub is_first_turn: bool,
    pub status: Option<TurnStatus>,
    pub turn_error: Option<CodexErrorInfo>,
    pub steer_count: Option<usize>,
    pub total_tool_call_count: Option<usize>,
    pub shell_command_count: Option<usize>,
    pub file_change_count: Option<usize>,
    pub mcp_tool_call_count: Option<usize>,
    pub dynamic_tool_call_count: Option<usize>,
    pub subagent_tool_call_count: Option<usize>,
    pub web_search_count: Option<usize>,
    pub image_generation_count: Option<usize>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<u64>,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSubmissionType {
    Default,
    Queued,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone)]
pub struct CodexThreadInitializedInput {
    pub thread_id: String,
    pub model: String,
    pub product_client_id: String,
    pub created_at: u64,
    pub thread_context: CodexThreadContext,
}

#[derive(Clone)]
pub struct CodexThreadContext {
    pub ephemeral: bool,
    pub session_source: SessionSource,
    pub initialization_mode: InitializationMode,
    pub subagent_source: Option<SubAgentSource>,
    pub parent_thread_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSteerResult {
    Accepted,
    Rejected,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSteerRejectionReason {
    NoActiveTurn,
    ExpectedTurnMismatch,
    NonSteerableReview,
    NonSteerableCompact,
    EmptyInput,
    InputTooLarge,
    InternalError,
}

#[derive(Clone)]
pub struct CodexTurnSteerEvent {
    pub expected_turn_id: String,
    pub accepted_turn_id: Option<String>,
    pub num_input_images: usize,
    pub result: TurnSteerResult,
    pub rejection_reason: Option<TurnSteerRejectionReason>,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationMode {
    New,
    Forked,
    Resumed,
}

pub fn build_track_events_context(
    model_slug: String,
    thread_id: String,
    turn_id: String,
) -> TrackEventsContext {
    TrackEventsContext {
        model_slug,
        thread_id,
        turn_id,
    }
}

#[derive(Clone, Debug)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub skill_scope: SkillScope,
    pub skill_path: PathBuf,
    pub invocation_type: InvocationType,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InvocationType {
    Explicit,
    Implicit,
}

pub struct AppInvocation {
    pub connector_id: Option<String>,
    pub app_name: Option<String>,
    pub invocation_type: Option<InvocationType>,
}

pub enum AnalyticsInput {
    CodexThreadInitialized(CodexThreadInitializedInput),
    TurnEvent(TurnEventInput),
    TurnSteer(TurnSteerInput),
    SkillInvoked(SkillInvokedInput),
    AppMentioned(AppMentionedInput),
    AppUsed(AppUsedInput),
    PluginUsed(PluginUsedInput),
    PluginStateChanged(PluginStateChangedInput),
}

pub struct TurnEventInput {
    pub tracking: TrackEventsContext,
    pub turn_event: CodexTurnEvent,
}

pub struct TurnSteerInput {
    pub tracking: TrackEventsContext,
    pub turn_steer: CodexTurnSteerEvent,
}

pub struct SkillInvokedInput {
    pub tracking: TrackEventsContext,
    pub invocations: Vec<SkillInvocation>,
}

pub struct AppMentionedInput {
    pub tracking: TrackEventsContext,
    pub mentions: Vec<AppInvocation>,
}

pub struct AppUsedInput {
    pub tracking: TrackEventsContext,
    pub app: AppInvocation,
}

pub struct PluginUsedInput {
    pub tracking: TrackEventsContext,
    pub plugin: PluginTelemetryMetadata,
}

pub struct PluginStateChangedInput {
    pub plugin: PluginTelemetryMetadata,
    pub state: PluginState,
}

#[derive(Clone, Copy)]
pub enum PluginState {
    Installed,
    Uninstalled,
    Enabled,
    Disabled,
}

#[derive(Default)]
pub struct AnalyticsReducer {
    threads: HashMap<String, ThreadState>,
}

struct ThreadState {
    _initialized_input: CodexThreadInitializedInput,
}

#[derive(Clone)]
pub(crate) struct AnalyticsEventsQueue {
    sender: mpsc::Sender<AnalyticsInput>,
    app_used_emitted_keys: Arc<Mutex<HashSet<(String, String)>>>,
    plugin_used_emitted_keys: Arc<Mutex<HashSet<(String, String)>>>,
}

#[derive(Clone)]
pub struct AnalyticsEventsClient {
    queue: AnalyticsEventsQueue,
    analytics_enabled: Option<bool>,
}

impl AnalyticsEventsQueue {
    pub(crate) fn new(auth_manager: Arc<AuthManager>, base_url: String) -> Self {
        let (sender, mut receiver) = mpsc::channel(ANALYTICS_EVENTS_QUEUE_SIZE);
        tokio::spawn(async move {
            let mut reducer = AnalyticsReducer::default();
            while let Some(job) = receiver.recv().await {
                let mut events = Vec::new();
                reducer.ingest(job, &mut events).await;
                send_track_events(&auth_manager, &base_url, events).await;
            }
        });
        Self {
            sender,
            app_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
            plugin_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn try_send(&self, input: AnalyticsInput) {
        if self.sender.try_send(input).is_err() {
            //TODO: add a metric for this
            tracing::warn!("dropping analytics events: queue is full");
        }
    }

    fn should_enqueue_app_used(&self, tracking: &TrackEventsContext, app: &AppInvocation) -> bool {
        let Some(connector_id) = app.connector_id.as_ref() else {
            return true;
        };
        let mut emitted = self
            .app_used_emitted_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if emitted.len() >= ANALYTICS_EVENT_DEDUPE_MAX_KEYS {
            emitted.clear();
        }
        emitted.insert((tracking.turn_id.clone(), connector_id.clone()))
    }

    fn should_enqueue_plugin_used(
        &self,
        tracking: &TrackEventsContext,
        plugin: &PluginTelemetryMetadata,
    ) -> bool {
        let mut emitted = self
            .plugin_used_emitted_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if emitted.len() >= ANALYTICS_EVENT_DEDUPE_MAX_KEYS {
            emitted.clear();
        }
        emitted.insert((tracking.turn_id.clone(), plugin.plugin_id.as_key()))
    }
}

impl AnalyticsEventsClient {
    pub fn new(
        auth_manager: Arc<AuthManager>,
        base_url: String,
        analytics_enabled: Option<bool>,
    ) -> Self {
        Self {
            queue: AnalyticsEventsQueue::new(Arc::clone(&auth_manager), base_url),
            analytics_enabled,
        }
    }

    pub fn track_skill_invocations(
        &self,
        tracking: TrackEventsContext,
        invocations: Vec<SkillInvocation>,
    ) {
        if invocations.is_empty() {
            return;
        }
        self.record(AnalyticsInput::SkillInvoked(SkillInvokedInput {
            tracking,
            invocations,
        }));
    }

    pub fn track_thread_initialized(&self, input: CodexThreadInitializedInput) {
        self.record(AnalyticsInput::CodexThreadInitialized(input));
    }

    pub fn track_app_mentioned(&self, tracking: TrackEventsContext, mentions: Vec<AppInvocation>) {
        if mentions.is_empty() {
            return;
        }
        self.record(AnalyticsInput::AppMentioned(AppMentionedInput {
            tracking,
            mentions,
        }));
    }

    pub fn track_app_used(&self, tracking: TrackEventsContext, app: AppInvocation) {
        if !self.queue.should_enqueue_app_used(&tracking, &app) {
            return;
        }
        self.record(AnalyticsInput::AppUsed(AppUsedInput { tracking, app }));
    }

    pub fn track_plugin_used(&self, tracking: TrackEventsContext, plugin: PluginTelemetryMetadata) {
        if !self.queue.should_enqueue_plugin_used(&tracking, &plugin) {
            return;
        }
        self.record(AnalyticsInput::PluginUsed(PluginUsedInput {
            tracking,
            plugin,
        }));
    }

    pub fn track_turn_event(&self, tracking: TrackEventsContext, turn_event: CodexTurnEvent) {
        self.record(AnalyticsInput::TurnEvent(TurnEventInput {
            tracking,
            turn_event,
        }));
    }

    pub fn track_turn_steer(&self, tracking: TrackEventsContext, turn_steer: CodexTurnSteerEvent) {
        self.record(AnalyticsInput::TurnSteer(TurnSteerInput {
            tracking,
            turn_steer,
        }));
    }

    pub fn track_plugin_installed(&self, plugin: PluginTelemetryMetadata) {
        self.record(AnalyticsInput::PluginStateChanged(
            PluginStateChangedInput {
                plugin,
                state: PluginState::Installed,
            },
        ));
    }

    pub fn track_plugin_uninstalled(&self, plugin: PluginTelemetryMetadata) {
        self.record(AnalyticsInput::PluginStateChanged(
            PluginStateChangedInput {
                plugin,
                state: PluginState::Uninstalled,
            },
        ));
    }

    pub fn track_plugin_enabled(&self, plugin: PluginTelemetryMetadata) {
        self.record(AnalyticsInput::PluginStateChanged(
            PluginStateChangedInput {
                plugin,
                state: PluginState::Enabled,
            },
        ));
    }

    pub fn track_plugin_disabled(&self, plugin: PluginTelemetryMetadata) {
        self.record(AnalyticsInput::PluginStateChanged(
            PluginStateChangedInput {
                plugin,
                state: PluginState::Disabled,
            },
        ));
    }

    pub fn record(&self, input: AnalyticsInput) {
        if self.analytics_enabled == Some(false) {
            return;
        }
        self.queue.try_send(input);
    }
}

const ANALYTICS_EVENTS_QUEUE_SIZE: usize = 256;
const ANALYTICS_EVENTS_TIMEOUT: Duration = Duration::from_secs(10);
const ANALYTICS_EVENT_DEDUPE_MAX_KEYS: usize = 4096;

#[derive(Serialize)]
struct TrackEventsRequest {
    events: Vec<TrackEventRequest>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum TrackEventRequest {
    SkillInvocation(SkillInvocationEventRequest),
    CodexThreadInitialized(CodexThreadInitializedEvent),
    AppMentioned(CodexAppMentionedEventRequest),
    AppUsed(CodexAppUsedEventRequest),
    TurnEvent(Box<CodexTurnEventRequest>),
    TurnSteer(CodexTurnSteerEventRequest),
    PluginUsed(CodexPluginUsedEventRequest),
    PluginInstalled(CodexPluginEventRequest),
    PluginUninstalled(CodexPluginEventRequest),
    PluginEnabled(CodexPluginEventRequest),
    PluginDisabled(CodexPluginEventRequest),
}

#[derive(Serialize)]
struct SkillInvocationEventRequest {
    event_type: &'static str,
    skill_id: String,
    skill_name: String,
    event_params: SkillInvocationEventParams,
}

#[derive(Serialize)]
struct SkillInvocationEventParams {
    product_client_id: Option<String>,
    skill_scope: Option<String>,
    repo_url: Option<String>,
    thread_id: Option<String>,
    invoke_type: Option<InvocationType>,
    model_slug: Option<String>,
}

#[derive(Serialize)]
struct CodexThreadInitializedEventParams {
    thread_id: String,
    product_client_id: String,
    model: String,
    ephemeral: bool,
    session_source: Option<&'static str>,
    initialization_mode: InitializationMode,
    subagent_source: Option<String>,
    parent_thread_id: Option<String>,
    created_at: u64,
}

#[derive(Serialize)]
struct CodexThreadInitializedEvent {
    event_type: &'static str,
    event_params: CodexThreadInitializedEventParams,
}

#[derive(Serialize)]
struct CodexAppMetadata {
    connector_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    app_name: Option<String>,
    product_client_id: Option<String>,
    invoke_type: Option<InvocationType>,
    model_slug: Option<String>,
}

#[derive(Serialize)]
struct CodexAppMentionedEventRequest {
    event_type: &'static str,
    event_params: CodexAppMetadata,
}

#[derive(Serialize)]
struct CodexAppUsedEventRequest {
    event_type: &'static str,
    event_params: CodexAppMetadata,
}

#[derive(Serialize)]
struct CodexTurnEventParams {
    thread_id: String,
    turn_id: String,
    product_client_id: Option<String>,
    submission_type: Option<TurnSubmissionType>,
    model: Option<String>,
    model_provider: String,
    sandbox_policy: Option<&'static str>,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    service_tier: String,
    approval_policy: String,
    approvals_reviewer: String,
    sandbox_network_access: bool,
    collaboration_mode: Option<&'static str>,
    personality: Option<String>,
    num_input_images: usize,
    is_first_turn: bool,
    status: Option<TurnStatus>,
    turn_error: Option<CodexErrorInfo>,
    steer_count: Option<usize>,
    total_tool_call_count: Option<usize>,
    shell_command_count: Option<usize>,
    file_change_count: Option<usize>,
    mcp_tool_call_count: Option<usize>,
    dynamic_tool_call_count: Option<usize>,
    subagent_tool_call_count: Option<usize>,
    web_search_count: Option<usize>,
    image_generation_count: Option<usize>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    duration_ms: Option<u64>,
    started_at: Option<u64>,
    completed_at: Option<u64>,
}

#[derive(Serialize)]
struct CodexTurnEventRequest {
    event_type: &'static str,
    event_params: CodexTurnEventParams,
}

#[derive(Serialize)]
struct CodexTurnSteerEventParams {
    thread_id: String,
    expected_turn_id: String,
    accepted_turn_id: Option<String>,
    product_client_id: Option<String>,
    num_input_images: usize,
    result: TurnSteerResult,
    rejection_reason: Option<TurnSteerRejectionReason>,
    created_at: u64,
}

#[derive(Serialize)]
struct CodexTurnSteerEventRequest {
    event_type: &'static str,
    event_params: CodexTurnSteerEventParams,
}

#[derive(Serialize)]
struct CodexPluginMetadata {
    plugin_id: Option<String>,
    plugin_name: Option<String>,
    marketplace_name: Option<String>,
    has_skills: Option<bool>,
    mcp_server_count: Option<usize>,
    connector_ids: Option<Vec<String>>,
    product_client_id: Option<String>,
}

#[derive(Serialize)]
struct CodexPluginUsedMetadata {
    #[serde(flatten)]
    plugin: CodexPluginMetadata,
    thread_id: Option<String>,
    turn_id: Option<String>,
    model_slug: Option<String>,
}

#[derive(Serialize)]
struct CodexPluginEventRequest {
    event_type: &'static str,
    event_params: CodexPluginMetadata,
}

#[derive(Serialize)]
struct CodexPluginUsedEventRequest {
    event_type: &'static str,
    event_params: CodexPluginUsedMetadata,
}

impl AnalyticsReducer {
    async fn ingest(&mut self, input: AnalyticsInput, out: &mut Vec<TrackEventRequest>) {
        match input {
            AnalyticsInput::CodexThreadInitialized(input) => {
                self.ingest_thread_initialized(input, out);
            }
            AnalyticsInput::TurnEvent(input) => {
                self.ingest_turn_event(input, out);
            }
            AnalyticsInput::TurnSteer(input) => {
                self.ingest_turn_steer(input, out);
            }
            AnalyticsInput::SkillInvoked(input) => {
                self.ingest_skill_invoked(input, out).await;
            }
            AnalyticsInput::AppMentioned(input) => {
                self.ingest_app_mentioned(input, out);
            }
            AnalyticsInput::AppUsed(input) => {
                self.ingest_app_used(input, out);
            }
            AnalyticsInput::PluginUsed(input) => {
                self.ingest_plugin_used(input, out);
            }
            AnalyticsInput::PluginStateChanged(input) => {
                self.ingest_plugin_state_changed(input, out);
            }
        }
    }

    fn ingest_thread_initialized(
        &mut self,
        input: CodexThreadInitializedInput,
        out: &mut Vec<TrackEventRequest>,
    ) {
        self.threads.insert(
            input.thread_id.clone(),
            ThreadState {
                _initialized_input: input.clone(),
            },
        );
        out.push(TrackEventRequest::CodexThreadInitialized(
            codex_thread_initialized_event_request(input),
        ));
    }

    fn ingest_turn_event(&mut self, input: TurnEventInput, out: &mut Vec<TrackEventRequest>) {
        let TurnEventInput {
            tracking,
            turn_event,
        } = input;
        out.push(TrackEventRequest::TurnEvent(CodexTurnEventRequest {
            event_type: "codex_turn_event",
            event_params: codex_turn_event_params(&tracking, turn_event),
        }));
    }

    fn ingest_turn_steer(&mut self, input: TurnSteerInput, out: &mut Vec<TrackEventRequest>) {
        let TurnSteerInput {
            tracking,
            turn_steer,
        } = input;
        out.push(TrackEventRequest::TurnSteer(CodexTurnSteerEventRequest {
            event_type: "codex_turn_steer_event",
            event_params: codex_turn_steer_event_params(&tracking, turn_steer),
        }));
    }

    fn ingest_turn_steer(&mut self, input: TurnSteerInput, out: &mut Vec<TrackEventRequest>) {
        let TurnSteerInput {
            tracking,
            turn_steer,
        } = input;
        out.push(TrackEventRequest::TurnSteer(CodexTurnSteerEventRequest {
            event_type: "codex_turn_steer_event",
            event_params: codex_turn_steer_event_params(&tracking, turn_steer),
        }));
    }

    async fn ingest_skill_invoked(
        &mut self,
        input: SkillInvokedInput,
        out: &mut Vec<TrackEventRequest>,
    ) {
        let SkillInvokedInput {
            tracking,
            invocations,
        } = input;
        for invocation in invocations {
            let skill_scope = match invocation.skill_scope {
                SkillScope::User => "user",
                SkillScope::Repo => "repo",
                SkillScope::System => "system",
                SkillScope::Admin => "admin",
            };
            let repo_root = get_git_repo_root(invocation.skill_path.as_path());
            let repo_url = if let Some(root) = repo_root.as_ref() {
                collect_git_info(root)
                    .await
                    .and_then(|info| info.repository_url)
            } else {
                None
            };
            let skill_id = skill_id_for_local_skill(
                repo_url.as_deref(),
                repo_root.as_deref(),
                invocation.skill_path.as_path(),
                invocation.skill_name.as_str(),
            );
            out.push(TrackEventRequest::SkillInvocation(
                SkillInvocationEventRequest {
                    event_type: "skill_invocation",
                    skill_id,
                    skill_name: invocation.skill_name.clone(),
                    event_params: SkillInvocationEventParams {
                        thread_id: Some(tracking.thread_id.clone()),
                        invoke_type: Some(invocation.invocation_type),
                        model_slug: Some(tracking.model_slug.clone()),
                        product_client_id: Some(originator().value),
                        repo_url,
                        skill_scope: Some(skill_scope.to_string()),
                    },
                },
            ));
        }
    }
    fn ingest_app_mentioned(&mut self, input: AppMentionedInput, out: &mut Vec<TrackEventRequest>) {
        let AppMentionedInput { tracking, mentions } = input;
        out.extend(mentions.into_iter().map(|mention| {
            let event_params = codex_app_metadata(&tracking, mention);
            TrackEventRequest::AppMentioned(CodexAppMentionedEventRequest {
                event_type: "codex_app_mentioned",
                event_params,
            })
        }));
    }

    fn ingest_app_used(&mut self, input: AppUsedInput, out: &mut Vec<TrackEventRequest>) {
        let AppUsedInput { tracking, app } = input;
        let event_params = codex_app_metadata(&tracking, app);
        out.push(TrackEventRequest::AppUsed(CodexAppUsedEventRequest {
            event_type: "codex_app_used",
            event_params,
        }));
    }

    fn ingest_plugin_used(&mut self, input: PluginUsedInput, out: &mut Vec<TrackEventRequest>) {
        let PluginUsedInput { tracking, plugin } = input;
        out.push(TrackEventRequest::PluginUsed(CodexPluginUsedEventRequest {
            event_type: "codex_plugin_used",
            event_params: codex_plugin_used_metadata(&tracking, plugin),
        }));
    }

    fn ingest_plugin_state_changed(
        &mut self,
        input: PluginStateChangedInput,
        out: &mut Vec<TrackEventRequest>,
    ) {
        let PluginStateChangedInput { plugin, state } = input;
        let event = CodexPluginEventRequest {
            event_type: plugin_state_event_type(state),
            event_params: codex_plugin_metadata(plugin),
        };
        out.push(match state {
            PluginState::Installed => TrackEventRequest::PluginInstalled(event),
            PluginState::Uninstalled => TrackEventRequest::PluginUninstalled(event),
            PluginState::Enabled => TrackEventRequest::PluginEnabled(event),
            PluginState::Disabled => TrackEventRequest::PluginDisabled(event),
        });
    }
}

fn plugin_state_event_type(state: PluginState) -> &'static str {
    match state {
        PluginState::Installed => "codex_plugin_installed",
        PluginState::Uninstalled => "codex_plugin_uninstalled",
        PluginState::Enabled => "codex_plugin_enabled",
        PluginState::Disabled => "codex_plugin_disabled",
    }
}

fn codex_app_metadata(tracking: &TrackEventsContext, app: AppInvocation) -> CodexAppMetadata {
    CodexAppMetadata {
        connector_id: app.connector_id,
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        app_name: app.app_name,
        product_client_id: Some(originator().value),
        invoke_type: app.invocation_type,
        model_slug: Some(tracking.model_slug.clone()),
    }
}

fn codex_turn_event_params(
    tracking: &TrackEventsContext,
    turn_event: CodexTurnEvent,
) -> CodexTurnEventParams {
    CodexTurnEventParams {
        thread_id: tracking.thread_id.clone(),
        turn_id: tracking.turn_id.clone(),
        product_client_id: Some(originator().value),
        submission_type: turn_event.submission_type,
        model: Some(tracking.model_slug.clone()),
        model_provider: turn_event.model_provider,
        sandbox_policy: Some(sandbox_policy_mode(&turn_event.sandbox_policy)),
        reasoning_effort: turn_event.reasoning_effort.map(|value| value.to_string()),
        reasoning_summary: reasoning_summary_mode(turn_event.reasoning_summary),
        service_tier: turn_event
            .service_tier
            .map(|value| value.to_string())
            .unwrap_or_else(|| "default".to_string()),
        approval_policy: turn_event.approval_policy.to_string(),
        approvals_reviewer: turn_event.approvals_reviewer.to_string(),
        sandbox_network_access: turn_event.sandbox_network_access,
        collaboration_mode: Some(collaboration_mode_mode(turn_event.collaboration_mode)),
        personality: personality_mode(turn_event.personality),
        num_input_images: turn_event.num_input_images,
        is_first_turn: turn_event.is_first_turn,
        status: turn_event.status,
        turn_error: turn_event.turn_error,
        steer_count: turn_event.steer_count,
        total_tool_call_count: turn_event.total_tool_call_count,
        shell_command_count: turn_event.shell_command_count,
        file_change_count: turn_event.file_change_count,
        mcp_tool_call_count: turn_event.mcp_tool_call_count,
        dynamic_tool_call_count: turn_event.dynamic_tool_call_count,
        subagent_tool_call_count: turn_event.subagent_tool_call_count,
        web_search_count: turn_event.web_search_count,
        image_generation_count: turn_event.image_generation_count,
        input_tokens: turn_event.input_tokens,
        cached_input_tokens: turn_event.cached_input_tokens,
        output_tokens: turn_event.output_tokens,
        reasoning_output_tokens: turn_event.reasoning_output_tokens,
        total_tokens: turn_event.total_tokens,
        duration_ms: turn_event.duration_ms,
        started_at: turn_event.started_at,
        completed_at: turn_event.completed_at,
    }
}

fn codex_turn_steer_event_params(
    tracking: &TrackEventsContext,
    turn_steer: CodexTurnSteerEvent,
) -> CodexTurnSteerEventParams {
    CodexTurnSteerEventParams {
        thread_id: tracking.thread_id.clone(),
        expected_turn_id: turn_steer.expected_turn_id,
        accepted_turn_id: turn_steer.accepted_turn_id,
        product_client_id: Some(originator().value),
        num_input_images: turn_steer.num_input_images,
        result: turn_steer.result,
        rejection_reason: turn_steer.rejection_reason,
        created_at: turn_steer.created_at,
    }
}

fn sandbox_policy_mode(sandbox_policy: &SandboxPolicy) -> &'static str {
    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => "full_access",
        SandboxPolicy::ReadOnly { .. } => "read_only",
        SandboxPolicy::WorkspaceWrite { .. } => "workspace_write",
        SandboxPolicy::ExternalSandbox { .. } => "external_sandbox",
    }
}

fn collaboration_mode_mode(mode: ModeKind) -> &'static str {
    match mode {
        ModeKind::Plan => "plan",
        ModeKind::Default | ModeKind::PairProgramming | ModeKind::Execute => "default",
    }
}

fn reasoning_summary_mode(summary: Option<ReasoningSummary>) -> Option<String> {
    match summary {
        Some(ReasoningSummary::None) | None => None,
        Some(summary) => Some(summary.to_string()),
    }
}

fn personality_mode(personality: Option<Personality>) -> Option<String> {
    match personality {
        Some(Personality::None) | None => None,
        Some(personality) => Some(personality.to_string()),
    }
}

fn codex_thread_initialized_event_request(
    input: CodexThreadInitializedInput,
) -> CodexThreadInitializedEvent {
    CodexThreadInitializedEvent {
        event_type: "codex_thread_initialized",
        event_params: codex_thread_initialized_event_params(input),
    }
}

fn codex_thread_initialized_event_params(
    input: CodexThreadInitializedInput,
) -> CodexThreadInitializedEventParams {
    CodexThreadInitializedEventParams {
        thread_id: input.thread_id,
        product_client_id: input.product_client_id,
        model: input.model,
        ephemeral: input.thread_context.ephemeral,
        session_source: session_source_name(&input.thread_context.session_source),
        initialization_mode: input.thread_context.initialization_mode,
        subagent_source: input
            .thread_context
            .subagent_source
            .map(subagent_source_name),
        parent_thread_id: input.thread_context.parent_thread_id,
        created_at: input.created_at,
    }
}

fn codex_plugin_metadata(plugin: PluginTelemetryMetadata) -> CodexPluginMetadata {
    let capability_summary = plugin.capability_summary;
    CodexPluginMetadata {
        plugin_id: Some(plugin.plugin_id.as_key()),
        plugin_name: Some(plugin.plugin_id.plugin_name),
        marketplace_name: Some(plugin.plugin_id.marketplace_name),
        has_skills: capability_summary
            .as_ref()
            .map(|summary| summary.has_skills),
        mcp_server_count: capability_summary
            .as_ref()
            .map(|summary| summary.mcp_server_names.len()),
        connector_ids: capability_summary.map(|summary| {
            summary
                .app_connector_ids
                .into_iter()
                .map(|connector_id| connector_id.0)
                .collect()
        }),
        product_client_id: Some(originator().value),
    }
}

fn codex_plugin_used_metadata(
    tracking: &TrackEventsContext,
    plugin: PluginTelemetryMetadata,
) -> CodexPluginUsedMetadata {
    CodexPluginUsedMetadata {
        plugin: codex_plugin_metadata(plugin),
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        model_slug: Some(tracking.model_slug.clone()),
    }
}

fn session_source_name(session_source: &SessionSource) -> Option<&'static str> {
    match session_source {
        SessionSource::Cli | SessionSource::VSCode | SessionSource::Exec => Some("user"),
        SessionSource::SubAgent(_) => Some("subagent"),
        SessionSource::Mcp | SessionSource::Custom(_) | SessionSource::Unknown => None,
    }
}

fn subagent_source_name(subagent_source: SubAgentSource) -> String {
    match subagent_source {
        SubAgentSource::Review => "review".to_string(),
        SubAgentSource::Compact => "compact".to_string(),
        SubAgentSource::ThreadSpawn { .. } => "thread_spawn".to_string(),
        SubAgentSource::MemoryConsolidation => "memory_consolidation".to_string(),
        SubAgentSource::Other(other) => other,
    }
}

async fn send_track_events(
    auth_manager: &AuthManager,
    base_url: &str,
    events: Vec<TrackEventRequest>,
) {
    if events.is_empty() {
        return;
    }
    let Some(auth) = auth_manager.auth().await else {
        return;
    };
    if !auth.is_chatgpt_auth() {
        return;
    }
    let access_token = match auth.get_token() {
        Ok(token) => token,
        Err(_) => return,
    };
    let Some(account_id) = auth.get_account_id() else {
        return;
    };

    let base_url = base_url.trim_end_matches('/');
    let url = format!("{base_url}/codex/analytics-events/events");
    let payload = TrackEventsRequest { events };

    let response = create_client()
        .post(&url)
        .timeout(ANALYTICS_EVENTS_TIMEOUT)
        .bearer_auth(&access_token)
        .header("chatgpt-account-id", &account_id)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!("events failed with status {status}: {body}");
        }
        Err(err) => {
            tracing::warn!("failed to send events request: {err}");
        }
    }
}

pub(crate) fn skill_id_for_local_skill(
    repo_url: Option<&str>,
    repo_root: Option<&Path>,
    skill_path: &Path,
    skill_name: &str,
) -> String {
    let path = normalize_path_for_skill_id(repo_url, repo_root, skill_path);
    let prefix = if let Some(url) = repo_url {
        format!("repo_{url}")
    } else {
        "personal".to_string()
    };
    let raw_id = format!("{prefix}_{path}_{skill_name}");
    let mut hasher = Sha1::new();
    hasher.update(raw_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Returns a normalized path for skill ID construction.
///
/// - Repo-scoped skills use a path relative to the repo root.
/// - User/admin/system skills use an absolute path.
fn normalize_path_for_skill_id(
    repo_url: Option<&str>,
    repo_root: Option<&Path>,
    skill_path: &Path,
) -> String {
    let resolved_path =
        std::fs::canonicalize(skill_path).unwrap_or_else(|_| skill_path.to_path_buf());
    match (repo_url, repo_root) {
        (Some(_), Some(root)) => {
            let resolved_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            resolved_path
                .strip_prefix(&resolved_root)
                .unwrap_or(resolved_path.as_path())
                .to_string_lossy()
                .replace('\\', "/")
        }
        _ => resolved_path.to_string_lossy().replace('\\', "/"),
    }
}

#[cfg(test)]
#[path = "analytics_client_tests.rs"]
mod tests;
