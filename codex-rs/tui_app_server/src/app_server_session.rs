use codex_app_server_client::AppServerClient;
use codex_app_server_client::AppServerEvent;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::Account;
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::DynamicToolSpec;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::Model as ApiModel;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ReviewDelivery;
use codex_app_server_protocol::ReviewStartParams;
use codex_app_server_protocol::ReviewStartResponse;
use codex_app_server_protocol::SkillsListParams;
use codex_app_server_protocol::SkillsListResponse;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadListParams;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadRealtimeAppendAudioParams;
use codex_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use codex_app_server_protocol::ThreadRealtimeAppendTextParams;
use codex_app_server_protocol::ThreadRealtimeAppendTextResponse;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartResponse;
use codex_app_server_protocol::ThreadRealtimeStopParams;
use codex_app_server_protocol::ThreadRealtimeStopResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadRollbackParams;
use codex_app_server_protocol::ThreadRollbackResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_core::config::Config;
use codex_otel::TelemetryAuthMode;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ModelAvailabilityNux;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::ReviewTarget as CoreReviewTarget;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use color_eyre::eyre::ContextCompat;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use crate::bottom_pane::FeedbackAudience;
use crate::dynamic_tools::DynamicToolExecutionContext;
use crate::dynamic_tools::DynamicToolRegistry;
use crate::status::StatusAccountDisplay;

pub(crate) struct AppServerBootstrap {
    pub(crate) account_auth_mode: Option<AuthMode>,
    pub(crate) account_email: Option<String>,
    pub(crate) auth_mode: Option<TelemetryAuthMode>,
    pub(crate) status_account_display: Option<StatusAccountDisplay>,
    pub(crate) plan_type: Option<codex_protocol::account::PlanType>,
    pub(crate) default_model: String,
    pub(crate) feedback_audience: FeedbackAudience,
    pub(crate) has_chatgpt_account: bool,
    pub(crate) available_models: Vec<ModelPreset>,
    pub(crate) rate_limit_snapshots: Vec<RateLimitSnapshot>,
}

pub(crate) struct AppServerSession {
    client: AppServerClient,
    dynamic_tools: Arc<DynamicToolRegistry>,
    thread_cwds: RwLock<HashMap<String, PathBuf>>,
    next_request_id: i64,
}

#[derive(Clone, Copy)]
enum ThreadParamsMode {
    Embedded,
    Remote,
}

impl ThreadParamsMode {
    fn model_provider_from_config(self, config: &Config) -> Option<String> {
        match self {
            Self::Embedded => Some(config.model_provider_id.clone()),
            Self::Remote => None,
        }
    }
}

/// Result of starting, resuming, or forking an app-server thread.
///
/// Carries the full `Thread` snapshot returned by the server alongside the
/// derived `SessionConfiguredEvent`. The snapshot's `turns` are used by
/// `App::restore_started_app_server_thread` to seed the event store and
/// replay transcript history — this is the only source of prior-turn data
/// for remote sessions, where historical websocket notifications are not
/// re-sent after the handshake.
pub(crate) struct AppServerStartedThread {
    pub(crate) thread: Thread,
    pub(crate) session_configured: SessionConfiguredEvent,
    pub(crate) show_raw_agent_reasoning: bool,
}

impl AppServerSession {
    pub(crate) fn new(client: AppServerClient) -> Self {
        Self::new_with_dynamic_tools(client, Arc::new(DynamicToolRegistry::tui_owned()))
    }

    pub(crate) fn new_with_dynamic_tools(
        client: AppServerClient,
        dynamic_tools: Arc<DynamicToolRegistry>,
    ) -> Self {
        Self {
            client,
            dynamic_tools,
            thread_cwds: RwLock::new(HashMap::new()),
            next_request_id: 1,
        }
    }

    pub(crate) fn is_remote(&self) -> bool {
        matches!(self.client, AppServerClient::Remote(_))
    }

    pub(crate) async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        let account_request_id = self.next_request_id();
        let account: GetAccountResponse = self
            .client
            .request_typed(ClientRequest::GetAccount {
                request_id: account_request_id,
                params: GetAccountParams {
                    refresh_token: false,
                },
            })
            .await
            .wrap_err("account/read failed during TUI bootstrap")?;
        let model_request_id = self.next_request_id();
        let models: ModelListResponse = self
            .client
            .request_typed(ClientRequest::ModelList {
                request_id: model_request_id,
                params: ModelListParams {
                    cursor: None,
                    limit: None,
                    include_hidden: Some(true),
                },
            })
            .await
            .wrap_err("model/list failed during TUI bootstrap")?;
        let rate_limit_request_id = self.next_request_id();
        let rate_limits: GetAccountRateLimitsResponse = self
            .client
            .request_typed(ClientRequest::GetAccountRateLimits {
                request_id: rate_limit_request_id,
                params: None,
            })
            .await
            .wrap_err("account/rateLimits/read failed during TUI bootstrap")?;

        let available_models = models
            .data
            .into_iter()
            .map(model_preset_from_api_model)
            .collect::<Vec<_>>();
        let default_model = config
            .model
            .clone()
            .or_else(|| {
                available_models
                    .iter()
                    .find(|model| model.is_default)
                    .map(|model| model.model.clone())
            })
            .or_else(|| available_models.first().map(|model| model.model.clone()))
            .wrap_err("model/list returned no models for TUI bootstrap")?;

        let (
            account_auth_mode,
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            feedback_audience,
            has_chatgpt_account,
        ) = match account.account {
            Some(Account::ApiKey {}) => (
                Some(AuthMode::ApiKey),
                None,
                Some(TelemetryAuthMode::ApiKey),
                Some(StatusAccountDisplay::ApiKey),
                None,
                FeedbackAudience::External,
                false,
            ),
            Some(Account::Chatgpt { email, plan_type }) => {
                let feedback_audience = if email.ends_with("@openai.com") {
                    FeedbackAudience::OpenAiEmployee
                } else {
                    FeedbackAudience::External
                };
                (
                    Some(AuthMode::Chatgpt),
                    Some(email.clone()),
                    Some(TelemetryAuthMode::Chatgpt),
                    Some(StatusAccountDisplay::ChatGpt {
                        email: Some(email),
                        plan: Some(title_case(format!("{plan_type:?}").as_str())),
                    }),
                    Some(plan_type),
                    feedback_audience,
                    true,
                )
            }
            None => (
                None,
                None,
                None,
                None,
                None,
                FeedbackAudience::External,
                false,
            ),
        };

        Ok(AppServerBootstrap {
            account_auth_mode,
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            default_model,
            feedback_audience,
            has_chatgpt_account,
            available_models,
            rate_limit_snapshots: app_server_rate_limit_snapshots_to_core(rate_limits),
        })
    }

    pub(crate) async fn next_event(&mut self) -> Option<AppServerEvent> {
        self.client.next_event().await
    }

    pub(crate) async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadStart {
                request_id,
                params: thread_start_params_from_config(
                    config,
                    self.thread_params_mode(),
                    self.dynamic_tools.specs(),
                ),
            })
            .await
            .wrap_err("thread/start failed during TUI bootstrap")?;
        let thread_id = response.thread.id.clone();
        let cwd = response.cwd.clone();
        let started = started_thread_from_start_response(response)?;
        self.remember_thread_cwd(thread_id, cwd);
        Ok(started)
    }

    pub(crate) async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let show_raw_agent_reasoning = config.show_raw_agent_reasoning;
        let request_id = self.next_request_id();
        let response: ThreadResumeResponse = self
            .client
            .request_typed(ClientRequest::ThreadResume {
                request_id,
                params: thread_resume_params_from_config(
                    config,
                    thread_id,
                    self.thread_params_mode(),
                ),
            })
            .await
            .wrap_err("thread/resume failed during TUI bootstrap")?;
        let thread_id = response.thread.id.clone();
        let cwd = response.cwd.clone();
        let started = started_thread_from_resume_response(response, show_raw_agent_reasoning)?;
        self.remember_thread_cwd(thread_id, cwd);
        Ok(started)
    }

    pub(crate) async fn fork_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let show_raw_agent_reasoning = config.show_raw_agent_reasoning;
        let request_id = self.next_request_id();
        let response: ThreadForkResponse = self
            .client
            .request_typed(ClientRequest::ThreadFork {
                request_id,
                params: thread_fork_params_from_config(
                    config,
                    thread_id,
                    self.thread_params_mode(),
                ),
            })
            .await
            .wrap_err("thread/fork failed during TUI bootstrap")?;
        let thread_id = response.thread.id.clone();
        let cwd = response.cwd.clone();
        let started = started_thread_from_fork_response(response, show_raw_agent_reasoning)?;
        self.remember_thread_cwd(thread_id, cwd);
        Ok(started)
    }

    fn thread_params_mode(&self) -> ThreadParamsMode {
        match &self.client {
            AppServerClient::InProcess(_) => ThreadParamsMode::Embedded,
            AppServerClient::Remote(_) => ThreadParamsMode::Remote,
        }
    }

    pub(crate) async fn thread_list(
        &mut self,
        params: ThreadListParams,
    ) -> Result<ThreadListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadList { request_id, params })
            .await
            .wrap_err("thread/list failed during TUI session lookup")
    }

    pub(crate) async fn thread_read(
        &mut self,
        thread_id: ThreadId,
        include_turns: bool,
    ) -> Result<Thread> {
        let request_id = self.next_request_id();
        let response: ThreadReadResponse = self
            .client
            .request_typed(ClientRequest::ThreadRead {
                request_id,
                params: ThreadReadParams {
                    thread_id: thread_id.to_string(),
                    include_turns,
                },
            })
            .await
            .wrap_err("thread/read failed during TUI session lookup")?;
        Ok(response.thread)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn turn_start(
        &mut self,
        thread_id: ThreadId,
        items: Vec<codex_protocol::user_input::UserInput>,
        cwd: PathBuf,
        approval_policy: AskForApproval,
        approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
        sandbox_policy: SandboxPolicy,
        model: String,
        effort: Option<codex_protocol::openai_models::ReasoningEffort>,
        summary: Option<codex_protocol::config_types::ReasoningSummary>,
        service_tier: Option<Option<codex_protocol::config_types::ServiceTier>>,
        collaboration_mode: Option<codex_protocol::config_types::CollaborationMode>,
        personality: Option<codex_protocol::config_types::Personality>,
        output_schema: Option<serde_json::Value>,
    ) -> Result<TurnStartResponse> {
        let request_id = self.next_request_id();
        let request_cwd = cwd.clone();
        let response = self
            .client
            .request_typed(ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id: thread_id.to_string(),
                    input: items.into_iter().map(Into::into).collect(),
                    cwd: Some(request_cwd),
                    approval_policy: Some(approval_policy.into()),
                    approvals_reviewer: Some(approvals_reviewer.into()),
                    sandbox_policy: Some(sandbox_policy.into()),
                    model: Some(model),
                    service_tier,
                    effort,
                    summary,
                    personality,
                    output_schema,
                    collaboration_mode,
                },
            })
            .await
            .wrap_err("turn/start failed in app-server TUI")?;
        self.remember_thread_cwd(thread_id.to_string(), cwd);
        Ok(response)
    }

    pub(crate) async fn turn_interrupt(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: TurnInterruptResponse = self
            .client
            .request_typed(ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id: thread_id.to_string(),
                    turn_id,
                },
            })
            .await
            .wrap_err("turn/interrupt failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn turn_steer(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
        items: Vec<codex_protocol::user_input::UserInput>,
    ) -> Result<TurnSteerResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::TurnSteer {
                request_id,
                params: TurnSteerParams {
                    thread_id: thread_id.to_string(),
                    input: items.into_iter().map(Into::into).collect(),
                    expected_turn_id: turn_id,
                },
            })
            .await
            .wrap_err("turn/steer failed in app-server TUI")
    }

    pub(crate) async fn thread_set_name(
        &mut self,
        thread_id: ThreadId,
        name: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadSetNameResponse = self
            .client
            .request_typed(ClientRequest::ThreadSetName {
                request_id,
                params: ThreadSetNameParams {
                    thread_id: thread_id.to_string(),
                    name,
                },
            })
            .await
            .wrap_err("thread/name/set failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_unsubscribe(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadUnsubscribeResponse = self
            .client
            .request_typed(ClientRequest::ThreadUnsubscribe {
                request_id,
                params: ThreadUnsubscribeParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/unsubscribe failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_compact_start(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadCompactStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadCompactStart {
                request_id,
                params: ThreadCompactStartParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/compact/start failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_background_terminals_clean(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadBackgroundTerminalsCleanResponse = self
            .client
            .request_typed(ClientRequest::ThreadBackgroundTerminalsClean {
                request_id,
                params: ThreadBackgroundTerminalsCleanParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/backgroundTerminals/clean failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_rollback(
        &mut self,
        thread_id: ThreadId,
        num_turns: u32,
    ) -> Result<ThreadRollbackResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadRollback {
                request_id,
                params: ThreadRollbackParams {
                    thread_id: thread_id.to_string(),
                    num_turns,
                },
            })
            .await
            .wrap_err("thread/rollback failed in app-server TUI")
    }

    pub(crate) async fn review_start(
        &mut self,
        thread_id: ThreadId,
        review_request: ReviewRequest,
    ) -> Result<ReviewStartResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ReviewStart {
                request_id,
                params: ReviewStartParams {
                    thread_id: thread_id.to_string(),
                    target: review_target_to_app_server(review_request.target),
                    delivery: Some(ReviewDelivery::Inline),
                },
            })
            .await
            .wrap_err("review/start failed in app-server TUI")
    }

    pub(crate) async fn skills_list(
        &mut self,
        params: SkillsListParams,
    ) -> Result<SkillsListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::SkillsList { request_id, params })
            .await
            .wrap_err("skills/list failed in app-server TUI")
    }

    pub(crate) async fn thread_realtime_start(
        &mut self,
        thread_id: ThreadId,
        params: ConversationStartParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStart {
                request_id,
                params: ThreadRealtimeStartParams {
                    thread_id: thread_id.to_string(),
                    prompt: params.prompt,
                    session_id: params.session_id,
                },
            })
            .await
            .wrap_err("thread/realtime/start failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_audio(
        &mut self,
        thread_id: ThreadId,
        params: ConversationAudioParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendAudioResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendAudio {
                request_id,
                params: ThreadRealtimeAppendAudioParams {
                    thread_id: thread_id.to_string(),
                    audio: params.frame.into(),
                },
            })
            .await
            .wrap_err("thread/realtime/appendAudio failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_text(
        &mut self,
        thread_id: ThreadId,
        params: ConversationTextParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendTextResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendText {
                request_id,
                params: ThreadRealtimeAppendTextParams {
                    thread_id: thread_id.to_string(),
                    text: params.text,
                },
            })
            .await
            .wrap_err("thread/realtime/appendText failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_stop(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStopResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStop {
                request_id,
                params: ThreadRealtimeStopParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/realtime/stop failed in app-server TUI")?;
        Ok(())
    }

    pub(crate) async fn reject_server_request(
        &self,
        request_id: RequestId,
        error: JSONRPCErrorError,
    ) -> std::io::Result<()> {
        self.client.reject_server_request(request_id, error).await
    }

    pub(crate) async fn resolve_server_request(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> std::io::Result<()> {
        self.client.resolve_server_request(request_id, result).await
    }

    pub(crate) async fn shutdown(self) -> std::io::Result<()> {
        self.client.shutdown().await
    }

    pub(crate) fn request_handle(&self) -> AppServerRequestHandle {
        self.client.request_handle()
    }

    pub(crate) fn dynamic_tool_registry(&self) -> Arc<DynamicToolRegistry> {
        Arc::clone(&self.dynamic_tools)
    }

    pub(crate) fn dynamic_tool_execution_context(
        &self,
        thread_id: &str,
    ) -> DynamicToolExecutionContext {
        DynamicToolExecutionContext::new(self.request_handle(), self.thread_cwd(thread_id))
    }

    fn next_request_id(&mut self) -> RequestId {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        RequestId::Integer(request_id)
    }

    fn remember_thread_cwd(&self, thread_id: String, cwd: PathBuf) {
        match self.thread_cwds.write() {
            Ok(mut thread_cwds) => {
                thread_cwds.insert(thread_id, cwd);
            }
            Err(err) => panic!("thread cwd map lock should not be poisoned: {err}"),
        }
    }

    fn thread_cwd(&self, thread_id: &str) -> Option<PathBuf> {
        match self.thread_cwds.read() {
            Ok(thread_cwds) => thread_cwds.get(thread_id).cloned(),
            Err(err) => panic!("thread cwd map lock should not be poisoned: {err}"),
        }
    }
}

fn title_case(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest = chars.as_str().to_ascii_lowercase();
    first.to_uppercase().collect::<String>() + &rest
}

pub(crate) fn status_account_display_from_auth_mode(
    auth_mode: Option<AuthMode>,
    plan_type: Option<codex_protocol::account::PlanType>,
) -> Option<StatusAccountDisplay> {
    match auth_mode {
        Some(AuthMode::ApiKey) => Some(StatusAccountDisplay::ApiKey),
        Some(AuthMode::Chatgpt) | Some(AuthMode::ChatgptAuthTokens) => {
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: plan_type.map(|plan_type| title_case(format!("{plan_type:?}").as_str())),
            })
        }
        None => None,
    }
}

#[allow(dead_code)]
pub(crate) fn feedback_audience_from_account_email(
    account_email: Option<&str>,
) -> FeedbackAudience {
    match account_email {
        Some(email) if email.ends_with("@openai.com") => FeedbackAudience::OpenAiEmployee,
        Some(_) | None => FeedbackAudience::External,
    }
}

fn model_preset_from_api_model(model: ApiModel) -> ModelPreset {
    let upgrade = model.upgrade.map(|upgrade_id| {
        let upgrade_info = model.upgrade_info.clone();
        ModelUpgrade {
            id: upgrade_id,
            reasoning_effort_mapping: None,
            migration_config_key: model.model.clone(),
            model_link: upgrade_info
                .as_ref()
                .and_then(|info| info.model_link.clone()),
            upgrade_copy: upgrade_info
                .as_ref()
                .and_then(|info| info.upgrade_copy.clone()),
            migration_markdown: upgrade_info.and_then(|info| info.migration_markdown),
        }
    });

    ModelPreset {
        id: model.id,
        model: model.model,
        display_name: model.display_name,
        description: model.description,
        default_reasoning_effort: model.default_reasoning_effort,
        supported_reasoning_efforts: model
            .supported_reasoning_efforts
            .into_iter()
            .map(|effort| ReasoningEffortPreset {
                effort: effort.reasoning_effort,
                description: effort.description,
            })
            .collect(),
        supports_personality: model.supports_personality,
        is_default: model.is_default,
        upgrade,
        show_in_picker: !model.hidden,
        availability_nux: model.availability_nux.map(|nux| ModelAvailabilityNux {
            message: nux.message,
        }),
        // `model/list` already returns models filtered for the active client/auth context.
        supported_in_api: true,
        input_modalities: model.input_modalities,
    }
}

fn approvals_reviewer_override_from_config(
    config: &Config,
) -> Option<codex_app_server_protocol::ApprovalsReviewer> {
    Some(config.approvals_reviewer.into())
}

fn config_request_overrides_from_config(
    config: &Config,
) -> Option<HashMap<String, serde_json::Value>> {
    config.active_profile.as_ref().map(|profile| {
        HashMap::from([(
            "profile".to_string(),
            serde_json::Value::String(profile.clone()),
        )])
    })
}

fn sandbox_mode_from_policy(
    policy: SandboxPolicy,
) -> Option<codex_app_server_protocol::SandboxMode> {
    match policy {
        SandboxPolicy::DangerFullAccess => {
            Some(codex_app_server_protocol::SandboxMode::DangerFullAccess)
        }
        SandboxPolicy::ReadOnly { .. } => Some(codex_app_server_protocol::SandboxMode::ReadOnly),
        SandboxPolicy::WorkspaceWrite { .. } => {
            Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite)
        }
        SandboxPolicy::ExternalSandbox { .. } => None,
    }
}

fn thread_start_params_from_config(
    config: &Config,
    thread_params_mode: ThreadParamsMode,
    dynamic_tools: Option<Vec<DynamicToolSpec>>,
) -> ThreadStartParams {
    ThreadStartParams {
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(config),
        cwd: thread_cwd_from_config(config, thread_params_mode),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(config),
        dynamic_tools,
        ephemeral: Some(config.ephemeral),
        persist_extended_history: true,
        ..ThreadStartParams::default()
    }
}

fn thread_resume_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id: thread_id.to_string(),
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(&config),
        cwd: thread_cwd_from_config(&config, thread_params_mode),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(&config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(&config),
        persist_extended_history: true,
        ..ThreadResumeParams::default()
    }
}

fn thread_fork_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
) -> ThreadForkParams {
    ThreadForkParams {
        thread_id: thread_id.to_string(),
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(&config),
        cwd: thread_cwd_from_config(&config, thread_params_mode),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(&config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(&config),
        ephemeral: config.ephemeral,
        persist_extended_history: true,
        ..ThreadForkParams::default()
    }
}

fn thread_cwd_from_config(config: &Config, thread_params_mode: ThreadParamsMode) -> Option<String> {
    match thread_params_mode {
        ThreadParamsMode::Embedded => Some(config.cwd.to_string_lossy().to_string()),
        ThreadParamsMode::Remote => None,
    }
}

fn started_thread_from_start_response(
    response: ThreadStartResponse,
) -> Result<AppServerStartedThread> {
    let session_configured = session_configured_from_thread_start_response(&response)
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        thread: response.thread,
        session_configured,
        show_raw_agent_reasoning: false,
    })
}

fn started_thread_from_resume_response(
    response: ThreadResumeResponse,
    show_raw_agent_reasoning: bool,
) -> Result<AppServerStartedThread> {
    let session_configured = session_configured_from_thread_resume_response(&response)
        .map_err(color_eyre::eyre::Report::msg)?;
    let thread = response.thread;
    Ok(AppServerStartedThread {
        thread,
        session_configured,
        show_raw_agent_reasoning,
    })
}

fn started_thread_from_fork_response(
    response: ThreadForkResponse,
    show_raw_agent_reasoning: bool,
) -> Result<AppServerStartedThread> {
    let session_configured = session_configured_from_thread_fork_response(&response)
        .map_err(color_eyre::eyre::Report::msg)?;
    let thread = response.thread;
    Ok(AppServerStartedThread {
        thread,
        session_configured,
        show_raw_agent_reasoning,
    })
}

fn session_configured_from_thread_start_response(
    response: &ThreadStartResponse,
) -> Result<SessionConfiguredEvent, String> {
    session_configured_from_thread_response(
        &response.thread.id,
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
    )
}

fn session_configured_from_thread_resume_response(
    response: &ThreadResumeResponse,
) -> Result<SessionConfiguredEvent, String> {
    session_configured_from_thread_response(
        &response.thread.id,
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
    )
}

fn session_configured_from_thread_fork_response(
    response: &ThreadForkResponse,
) -> Result<SessionConfiguredEvent, String> {
    session_configured_from_thread_response(
        &response.thread.id,
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
    )
}

fn review_target_to_app_server(
    target: CoreReviewTarget,
) -> codex_app_server_protocol::ReviewTarget {
    match target {
        CoreReviewTarget::UncommittedChanges => {
            codex_app_server_protocol::ReviewTarget::UncommittedChanges
        }
        CoreReviewTarget::BaseBranch { branch } => {
            codex_app_server_protocol::ReviewTarget::BaseBranch { branch }
        }
        CoreReviewTarget::Commit { sha, title } => {
            codex_app_server_protocol::ReviewTarget::Commit { sha, title }
        }
        CoreReviewTarget::Custom { instructions } => {
            codex_app_server_protocol::ReviewTarget::Custom { instructions }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "session mapping keeps explicit fields"
)]
fn session_configured_from_thread_response(
    thread_id: &str,
    thread_name: Option<String>,
    rollout_path: Option<PathBuf>,
    model: String,
    model_provider_id: String,
    service_tier: Option<codex_protocol::config_types::ServiceTier>,
    approval_policy: AskForApproval,
    approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
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

fn app_server_rate_limit_snapshots_to_core(
    response: GetAccountRateLimitsResponse,
) -> Vec<RateLimitSnapshot> {
    let mut snapshots = Vec::new();
    snapshots.push(app_server_rate_limit_snapshot_to_core(response.rate_limits));
    if let Some(by_limit_id) = response.rate_limits_by_limit_id {
        snapshots.extend(
            by_limit_id
                .into_values()
                .map(app_server_rate_limit_snapshot_to_core),
        );
    }
    snapshots
}

pub(crate) fn app_server_rate_limit_snapshot_to_core(
    snapshot: codex_app_server_protocol::RateLimitSnapshot,
) -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: snapshot.limit_id,
        limit_name: snapshot.limit_name,
        primary: snapshot.primary.map(app_server_rate_limit_window_to_core),
        secondary: snapshot.secondary.map(app_server_rate_limit_window_to_core),
        credits: snapshot.credits.map(app_server_credits_snapshot_to_core),
        plan_type: snapshot.plan_type,
    }
}

fn app_server_rate_limit_window_to_core(
    window: codex_app_server_protocol::RateLimitWindow,
) -> RateLimitWindow {
    RateLimitWindow {
        used_percent: window.used_percent as f64,
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn app_server_credits_snapshot_to_core(
    snapshot: codex_app_server_protocol::CreditsSnapshot,
) -> CreditsSnapshot {
    CreditsSnapshot {
        has_credits: snapshot.has_credits,
        unlimited: snapshot.unlimited,
        balance: snapshot.balance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_test_support::create_final_assistant_message_sse_response;
    use app_test_support::create_mock_responses_server_sequence_unchecked;
    use codex_app_server_client::AppServerClient;
    use codex_app_server_client::AppServerEvent;
    use codex_app_server_protocol::ClientRequest;
    use codex_app_server_protocol::DynamicToolCallOutputContentItem;
    use codex_app_server_protocol::DynamicToolCallParams;
    use codex_app_server_protocol::DynamicToolCallResponse;
    use codex_app_server_protocol::DynamicToolSpec;
    use codex_app_server_protocol::ServerNotification;
    use codex_app_server_protocol::ServerRequest;
    use codex_app_server_protocol::ThreadResumeResponse;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus;
    use codex_arg0::Arg0DispatchPaths;
    use codex_core::config::ConfigBuilder;
    use codex_core::config_loader::CloudRequirementsLoader;
    use codex_core::config_loader::LoaderOverrides;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::user_input::UserInput;
    use core_test_support::responses;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time::timeout;
    use wiremock::MockServer;

    use crate::dynamic_tools::DynamicToolExecutionError;
    use crate::dynamic_tools::DynamicToolRegistration;
    use crate::dynamic_tools::DynamicToolRegistry;
    use crate::dynamic_tools::handle_dynamic_tool_call_request;

    const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

    fn mock_provider_cli_overrides(server_uri: &str) -> Vec<(String, toml::Value)> {
        vec![
            (
                "model".to_string(),
                toml::Value::String("mock-model".to_string()),
            ),
            (
                "model_provider".to_string(),
                toml::Value::String("mock_provider".to_string()),
            ),
            (
                "approval_policy".to_string(),
                toml::Value::String("never".to_string()),
            ),
            (
                "sandbox_mode".to_string(),
                toml::Value::String("read-only".to_string()),
            ),
            (
                "model_providers.mock_provider.name".to_string(),
                toml::Value::String("Mock provider for test".to_string()),
            ),
            (
                "model_providers.mock_provider.base_url".to_string(),
                toml::Value::String(format!("{server_uri}/v1")),
            ),
            (
                "model_providers.mock_provider.wire_api".to_string(),
                toml::Value::String("responses".to_string()),
            ),
            (
                "model_providers.mock_provider.request_max_retries".to_string(),
                toml::Value::Integer(0),
            ),
            (
                "model_providers.mock_provider.stream_max_retries".to_string(),
                toml::Value::Integer(0),
            ),
        ]
    }

    async fn build_config(temp_dir: &TempDir, cli_overrides: Vec<(String, toml::Value)>) -> Config {
        ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .cli_overrides(cli_overrides)
            .build()
            .await
            .expect("config should build")
    }

    async fn start_test_session(
        config: Config,
        cli_overrides: Vec<(String, toml::Value)>,
        dynamic_tools: Arc<DynamicToolRegistry>,
    ) -> Result<AppServerSession> {
        let client = crate::start_embedded_app_server(
            Arg0DispatchPaths::default(),
            config,
            cli_overrides,
            LoaderOverrides::default(),
            CloudRequirementsLoader::default(),
            codex_feedback::CodexFeedback::new(),
        )
        .await?;
        Ok(AppServerSession::new_with_dynamic_tools(
            AppServerClient::InProcess(client),
            dynamic_tools,
        ))
    }

    fn demo_tool_spec(name: &str) -> DynamicToolSpec {
        DynamicToolSpec {
            name: name.to_string(),
            description: format!("dynamic tool {name}"),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"],
                "additionalProperties": false,
            }),
            defer_loading: false,
        }
    }

    async fn responses_bodies(server: &MockServer) -> Result<Vec<Value>> {
        let mut bodies = Vec::new();
        for request in server
            .received_requests()
            .await
            .expect("requests should be readable")
        {
            if request.url.path().ends_with("/responses") {
                bodies.push(
                    request
                        .body_json::<Value>()
                        .expect("request body should be json"),
                );
            }
        }
        Ok(bodies)
    }

    fn function_call_output_payload(
        body: &Value,
        call_id: &str,
    ) -> Option<FunctionCallOutputPayload> {
        body.get("input")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("type").and_then(Value::as_str) == Some("function_call_output")
                        && item.get("call_id").and_then(Value::as_str) == Some(call_id)
                })
            })
            .and_then(|item| item.get("output"))
            .cloned()
            .and_then(|output| serde_json::from_value(output).ok())
    }

    async fn run_dynamic_tool_turn(
        session: &mut AppServerSession,
        config: &Config,
        thread_id: ThreadId,
        prompt: &str,
    ) -> Result<()> {
        session
            .turn_start(
                thread_id,
                vec![UserInput::Text {
                    text: prompt.to_string(),
                    text_elements: Vec::new(),
                }],
                config.cwd.clone(),
                config.permissions.approval_policy.value(),
                config.approvals_reviewer,
                config.permissions.sandbox_policy.get().clone(),
                config
                    .model
                    .clone()
                    .expect("mock model should be configured"),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await?;

        loop {
            let event = timeout(DEFAULT_READ_TIMEOUT, session.next_event())
                .await?
                .expect("app-server event stream should stay open");
            match event {
                AppServerEvent::ServerRequest(ServerRequest::DynamicToolCall {
                    request_id,
                    params,
                }) => {
                    handle_dynamic_tool_call_request(
                        session.dynamic_tool_registry(),
                        session.dynamic_tool_execution_context(&params.thread_id),
                        request_id,
                        params,
                    )
                    .await
                    .map_err(|err| color_eyre::eyre::eyre!(err))?;
                }
                AppServerEvent::ServerNotification(ServerNotification::TurnCompleted(_)) => {
                    return Ok(());
                }
                AppServerEvent::LegacyNotification(notification)
                    if notification.method.ends_with("task_complete") =>
                {
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn thread_start_params_include_cwd_for_embedded_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir, Vec::new()).await;

        let params = thread_start_params_from_config(&config, ThreadParamsMode::Embedded, None);

        assert_eq!(params.cwd, Some(config.cwd.to_string_lossy().to_string()));
        assert_eq!(params.model_provider, Some(config.model_provider_id));
    }

    #[tokio::test]
    async fn thread_start_params_include_registered_dynamic_tools() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir, Vec::new()).await;
        let spec = demo_tool_spec("demo_tool");

        let params = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Embedded,
            Some(vec![spec.clone()]),
        );

        assert_eq!(params.dynamic_tools, Some(vec![spec]));
    }

    #[tokio::test]
    async fn thread_lifecycle_params_omit_local_overrides_for_remote_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir, Vec::new()).await;
        let thread_id = ThreadId::new();

        let start = thread_start_params_from_config(&config, ThreadParamsMode::Remote, None);
        let resume =
            thread_resume_params_from_config(config.clone(), thread_id, ThreadParamsMode::Remote);
        let fork = thread_fork_params_from_config(config, thread_id, ThreadParamsMode::Remote);

        assert_eq!(start.cwd, None);
        assert_eq!(resume.cwd, None);
        assert_eq!(fork.cwd, None);
        assert_eq!(start.model_provider, None);
        assert_eq!(resume.model_provider, None);
        assert_eq!(fork.model_provider, None);
    }

    #[test]
    fn resume_response_relies_on_snapshot_replay_not_initial_messages() {
        let thread_id = ThreadId::new();
        let response = ThreadResumeResponse {
            thread: codex_app_server_protocol::Thread {
                id: thread_id.to_string(),
                preview: "hello".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 1,
                updated_at: 2,
                status: ThreadStatus::Idle,
                path: None,
                cwd: PathBuf::from("/tmp/project"),
                cli_version: "0.0.0".to_string(),
                source: codex_protocol::protocol::SessionSource::Cli.into(),
                agent_nickname: None,
                agent_role: None,
                git_info: None,
                name: None,
                turns: vec![Turn {
                    id: "turn-1".to_string(),
                    items: vec![
                        codex_app_server_protocol::ThreadItem::UserMessage {
                            id: "user-1".to_string(),
                            content: vec![codex_app_server_protocol::UserInput::Text {
                                text: "hello from history".to_string(),
                                text_elements: Vec::new(),
                            }],
                        },
                        codex_app_server_protocol::ThreadItem::AgentMessage {
                            id: "assistant-1".to_string(),
                            text: "assistant reply".to_string(),
                            phase: None,
                        },
                    ],
                    status: TurnStatus::Completed,
                    error: None,
                }],
            },
            model: "gpt-5.4".to_string(),
            model_provider: "openai".to_string(),
            service_tier: None,
            cwd: PathBuf::from("/tmp/project"),
            approval_policy: codex_protocol::protocol::AskForApproval::Never.into(),
            approvals_reviewer: codex_app_server_protocol::ApprovalsReviewer::User,
            sandbox: codex_protocol::protocol::SandboxPolicy::new_read_only_policy().into(),
            reasoning_effort: None,
        };

        let started =
            started_thread_from_resume_response(response, /*show_raw_agent_reasoning*/ false)
                .expect("resume response should map");
        assert!(started.session_configured.initial_messages.is_none());
        assert!(!started.show_raw_agent_reasoning);
        assert_eq!(started.thread.turns.len(), 1);
        assert_eq!(started.thread.turns[0].items.len(), 2);
    }

    #[tokio::test]
    async fn dynamic_tool_turn_round_trip_resolves_registered_tools() -> Result<()> {
        let call_id = "dyn-call-1";
        let tool_name = "demo_tool";
        let tool_args = json!({ "city": "Paris" });
        let tool_call_arguments = serde_json::to_string(&tool_args)?;
        let responses = vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_function_call(call_id, tool_name, &tool_call_arguments),
                responses::ev_completed("resp-1"),
            ]),
            create_final_assistant_message_sse_response("Done")
                .map_err(|err| color_eyre::eyre::eyre!(err))?,
        ];
        let server = create_mock_responses_server_sequence_unchecked(responses).await;

        let codex_home = TempDir::new()?;
        let cli_overrides = mock_provider_cli_overrides(&server.uri());
        let config = build_config(&codex_home, cli_overrides.clone()).await;
        let registry = Arc::new(DynamicToolRegistry::from_registrations(vec![
            DynamicToolRegistration::new(
                demo_tool_spec(tool_name),
                |context, _params| async move {
                    Ok(DynamicToolCallResponse {
                        content_items: vec![DynamicToolCallOutputContentItem::InputText {
                            text: context
                                .cwd()
                                .expect("thread cwd should be present")
                                .display()
                                .to_string(),
                        }],
                        success: true,
                    })
                },
            ),
        ]));
        let mut session = start_test_session(config.clone(), cli_overrides, registry).await?;
        let started = session.start_thread(&config).await?;
        let thread_id = started.session_configured.session_id;

        run_dynamic_tool_turn(&mut session, &config, thread_id, "Run the tool").await?;

        let bodies = responses_bodies(&server).await?;
        let payload = bodies
            .iter()
            .find_map(|body| function_call_output_payload(body, call_id))
            .expect("expected function_call_output payload");
        assert_eq!(
            payload,
            FunctionCallOutputPayload::from_text(config.cwd.display().to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn resumed_threads_can_reuse_registered_dynamic_tools() -> Result<()> {
        let first_call_id = "dyn-call-1";
        let second_call_id = "dyn-call-2";
        let tool_name = "demo_tool";
        let tool_args = json!({ "city": "Paris" });
        let tool_call_arguments = serde_json::to_string(&tool_args)?;
        let responses = vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_function_call(first_call_id, tool_name, &tool_call_arguments),
                responses::ev_completed("resp-1"),
            ]),
            create_final_assistant_message_sse_response("Done")
                .map_err(|err| color_eyre::eyre::eyre!(err))?,
            responses::sse(vec![
                responses::ev_response_created("resp-2"),
                responses::ev_function_call(second_call_id, tool_name, &tool_call_arguments),
                responses::ev_completed("resp-2"),
            ]),
            create_final_assistant_message_sse_response("Done again")
                .map_err(|err| color_eyre::eyre::eyre!(err))?,
        ];
        let server = create_mock_responses_server_sequence_unchecked(responses).await;

        let codex_home = TempDir::new()?;
        let cli_overrides = mock_provider_cli_overrides(&server.uri());
        let config = build_config(&codex_home, cli_overrides.clone()).await;
        let registry = Arc::new(DynamicToolRegistry::from_registrations(vec![
            DynamicToolRegistration::new(
                demo_tool_spec(tool_name),
                |context, _params| async move {
                    Ok(DynamicToolCallResponse {
                        content_items: vec![DynamicToolCallOutputContentItem::InputText {
                            text: context
                                .cwd()
                                .expect("thread cwd should be present")
                                .display()
                                .to_string(),
                        }],
                        success: true,
                    })
                },
            ),
        ]));

        let mut first_session =
            start_test_session(config.clone(), cli_overrides.clone(), Arc::clone(&registry))
                .await?;
        let started = first_session.start_thread(&config).await?;
        let thread_id = started.session_configured.session_id;
        run_dynamic_tool_turn(&mut first_session, &config, thread_id, "Run the tool").await?;
        let rollout_path = first_session
            .thread_read(thread_id, /* include_turns */ false)
            .await?
            .path
            .expect("thread should expose a persisted rollout path");
        first_session.shutdown().await?;

        let resumed_config = build_config(&codex_home, cli_overrides.clone()).await;
        let mut resumed_session =
            start_test_session(resumed_config.clone(), cli_overrides, Arc::clone(&registry))
                .await?;
        let request_id = resumed_session.next_request_id();
        let mut params = thread_resume_params_from_config(
            resumed_config.clone(),
            thread_id,
            ThreadParamsMode::Embedded,
        );
        params.path = Some(rollout_path);
        let _: ThreadResumeResponse = resumed_session
            .client
            .request_typed(ClientRequest::ThreadResume { request_id, params })
            .await?;
        run_dynamic_tool_turn(
            &mut resumed_session,
            &resumed_config,
            thread_id,
            "Run the tool again",
        )
        .await?;

        let bodies = responses_bodies(&server).await?;
        let first_payload = bodies
            .iter()
            .find_map(|body| function_call_output_payload(body, first_call_id))
            .expect("expected first function_call_output payload");
        let second_payload = bodies
            .iter()
            .find_map(|body| function_call_output_payload(body, second_call_id))
            .expect("expected second function_call_output payload");
        assert_eq!(
            first_payload,
            FunctionCallOutputPayload::from_text(config.cwd.display().to_string())
        );
        assert_eq!(
            second_payload,
            FunctionCallOutputPayload::from_text(resumed_config.cwd.display().to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn unknown_dynamic_tool_requests_are_rejected_cleanly() -> Result<()> {
        let registry = Arc::new(DynamicToolRegistry::default());
        let error = handle_dynamic_tool_call_request(
            registry,
            DynamicToolExecutionContext::for_tests(),
            RequestId::Integer(1),
            DynamicToolCallParams {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                call_id: "call-1".to_string(),
                tool: "missing_tool".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect_err("missing request handle should fail before dispatch");

        assert_eq!(
            error,
            "dynamic tool execution context is missing an app-server request handle"
        );
        Ok(())
    }

    #[tokio::test]
    async fn failing_dynamic_tools_return_failed_responses() -> Result<()> {
        let call_id = "dyn-call-1";
        let tool_name = "demo_tool";
        let tool_args = json!({ "city": "Paris" });
        let tool_call_arguments = serde_json::to_string(&tool_args)?;
        let responses = vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_function_call(call_id, tool_name, &tool_call_arguments),
                responses::ev_completed("resp-1"),
            ]),
            create_final_assistant_message_sse_response("Done")
                .map_err(|err| color_eyre::eyre::eyre!(err))?,
        ];
        let server = create_mock_responses_server_sequence_unchecked(responses).await;

        let codex_home = TempDir::new()?;
        let cli_overrides = mock_provider_cli_overrides(&server.uri());
        let config = build_config(&codex_home, cli_overrides.clone()).await;
        let registry = Arc::new(DynamicToolRegistry::from_registrations(vec![
            DynamicToolRegistration::new(
                demo_tool_spec(tool_name),
                move |_context, _params| async move {
                    Err(DynamicToolExecutionError::failed(
                        tool_name,
                        "dynamic tool failed",
                    ))
                },
            ),
        ]));
        let mut session = start_test_session(config.clone(), cli_overrides, registry).await?;
        let started = session.start_thread(&config).await?;

        run_dynamic_tool_turn(
            &mut session,
            &config,
            started.session_configured.session_id,
            "Run the tool",
        )
        .await?;

        let bodies = responses_bodies(&server).await?;
        let payload = bodies
            .iter()
            .find_map(|body| function_call_output_payload(body, call_id))
            .expect("expected function_call_output payload");
        assert_eq!(
            payload,
            FunctionCallOutputPayload::from_text("dynamic tool failed".to_string())
        );
        Ok(())
    }
}
