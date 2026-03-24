//! Config-resolution and child-session spawn helpers for guardian reviews.
//!
//! The manager/orchestration layer decides whether a trunk or fork is needed. This module answers
//! two narrower questions:
//! - what config/model should the guardian child session use?
//! - how do we spawn that child session under the caller's deadline/cancel policy?

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use codex_features::Feature;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::SubAgentSource;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::codex_delegate::run_codex_thread_interactive;
use crate::config::Config;
use crate::config::Constrained;
use crate::config::ManagedFeatures;
use crate::config::NetworkProxySpec;
use crate::config::Permissions;
use crate::config::types::McpServerConfig;
use crate::model_provider_info::ModelProviderInfo;
use crate::models_manager::manager::RefreshStrategy;
use crate::protocol::SandboxPolicy;

use super::GuardianReviewSession;
use super::GuardianReviewSessionOutcome;
use super::execution::run_before_review_deadline_with_cancel;

/// Resolved guardian configuration for one parent turn.
///
/// The child-session config and the per-review `model` / `reasoning_effort` submission fields must
/// stay in sync, so they are resolved together once here.
pub(crate) struct GuardianResolvedReviewConfig {
    pub(crate) spawn_config: Config,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffortConfig>,
}

#[derive(Debug)]
pub(super) enum GuardianReviewSessionSpawnOutcome {
    Failed(anyhow::Error),
    TimedOut,
    Aborted,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GuardianReviewSessionReuseKey {
    // Only include settings that affect spawned-session behavior so reuse
    // invalidation remains explicit and does not depend on unrelated config
    // bookkeeping.
    model: Option<String>,
    model_provider_id: String,
    model_provider: ModelProviderInfo,
    model_context_window: Option<i64>,
    model_auto_compact_token_limit: Option<i64>,
    model_reasoning_effort: Option<ReasoningEffortConfig>,
    permissions: Permissions,
    developer_instructions: Option<String>,
    base_instructions: Option<String>,
    user_instructions: Option<String>,
    compact_prompt: Option<String>,
    cwd: PathBuf,
    mcp_servers: Constrained<HashMap<String, McpServerConfig>>,
    codex_linux_sandbox_exe: Option<PathBuf>,
    main_execve_wrapper_exe: Option<PathBuf>,
    js_repl_node_path: Option<PathBuf>,
    js_repl_node_module_dirs: Vec<PathBuf>,
    zsh_path: Option<PathBuf>,
    features: ManagedFeatures,
    include_apply_patch_tool: bool,
    use_experimental_unified_exec_tool: bool,
}

impl GuardianReviewSessionReuseKey {
    pub(super) fn from_spawn_config(spawn_config: &Config) -> Self {
        Self {
            model: spawn_config.model.clone(),
            model_provider_id: spawn_config.model_provider_id.clone(),
            model_provider: spawn_config.model_provider.clone(),
            model_context_window: spawn_config.model_context_window,
            model_auto_compact_token_limit: spawn_config.model_auto_compact_token_limit,
            model_reasoning_effort: spawn_config.model_reasoning_effort,
            permissions: spawn_config.permissions.clone(),
            developer_instructions: spawn_config.developer_instructions.clone(),
            base_instructions: spawn_config.base_instructions.clone(),
            user_instructions: spawn_config.user_instructions.clone(),
            compact_prompt: spawn_config.compact_prompt.clone(),
            cwd: spawn_config.cwd.clone(),
            mcp_servers: spawn_config.mcp_servers.clone(),
            codex_linux_sandbox_exe: spawn_config.codex_linux_sandbox_exe.clone(),
            main_execve_wrapper_exe: spawn_config.main_execve_wrapper_exe.clone(),
            js_repl_node_path: spawn_config.js_repl_node_path.clone(),
            js_repl_node_module_dirs: spawn_config.js_repl_node_module_dirs.clone(),
            zsh_path: spawn_config.zsh_path.clone(),
            features: spawn_config.features.clone(),
            include_apply_patch_tool: spawn_config.include_apply_patch_tool,
            use_experimental_unified_exec_tool: spawn_config.use_experimental_unified_exec_tool,
        }
    }
}

/// Spawns a guardian child session and maps deadline/cancel outcomes into a small internal enum.
///
/// Trunk creation and fork creation both use this helper so they do not duplicate the same
/// cancellation/timeout plumbing.
pub(super) async fn spawn_review_session_before_deadline(
    deadline: tokio::time::Instant,
    external_cancel: Option<&tokio_util::sync::CancellationToken>,
    parent_session: &Arc<Session>,
    parent_turn: &Arc<TurnContext>,
    spawn_config: Config,
    reuse_key: GuardianReviewSessionReuseKey,
    initial_history: Option<InitialHistory>,
) -> Result<Arc<GuardianReviewSession>, GuardianReviewSessionSpawnOutcome> {
    let spawn_cancel_token = tokio_util::sync::CancellationToken::new();
    match run_before_review_deadline_with_cancel(
        deadline,
        external_cancel,
        &spawn_cancel_token,
        Box::pin(spawn_guardian_review_session(
            Arc::clone(parent_session),
            Arc::clone(parent_turn),
            spawn_config,
            reuse_key,
            spawn_cancel_token.clone(),
            initial_history,
        )),
    )
    .await
    {
        Ok(Ok(review_session)) => Ok(Arc::new(review_session)),
        Ok(Err(err)) => Err(GuardianReviewSessionSpawnOutcome::Failed(err)),
        Err(GuardianReviewSessionOutcome::TimedOut) => {
            Err(GuardianReviewSessionSpawnOutcome::TimedOut)
        }
        Err(GuardianReviewSessionOutcome::Aborted) => {
            Err(GuardianReviewSessionSpawnOutcome::Aborted)
        }
        Err(GuardianReviewSessionOutcome::Completed(result)) => Err(
            GuardianReviewSessionSpawnOutcome::Failed(result.err().unwrap_or_else(|| {
                anyhow::anyhow!("guardian session spawn completed without returning a session")
            })),
        ),
    }
}

/// Converts shared child-session spawn failures back into the public review outcome shape.
pub(super) fn review_outcome_from_spawn_outcome(
    outcome: GuardianReviewSessionSpawnOutcome,
) -> GuardianReviewSessionOutcome {
    match outcome {
        GuardianReviewSessionSpawnOutcome::Failed(err) => {
            GuardianReviewSessionOutcome::Completed(Err(err))
        }
        GuardianReviewSessionSpawnOutcome::TimedOut => GuardianReviewSessionOutcome::TimedOut,
        GuardianReviewSessionSpawnOutcome::Aborted => GuardianReviewSessionOutcome::Aborted,
    }
}

async fn spawn_guardian_review_session(
    parent_session: Arc<Session>,
    parent_turn: Arc<TurnContext>,
    spawn_config: Config,
    reuse_key: GuardianReviewSessionReuseKey,
    cancel_token: tokio_util::sync::CancellationToken,
    initial_history: Option<InitialHistory>,
) -> anyhow::Result<GuardianReviewSession> {
    let has_prior_review = initial_history.is_some();
    // Guardian runs as an ordinary child Codex thread with a different config and source label.
    let codex = run_codex_thread_interactive(
        spawn_config,
        parent_session.services.auth_manager.clone(),
        parent_session.services.models_manager.clone(),
        Arc::clone(&parent_session),
        Arc::clone(&parent_turn),
        cancel_token.clone(),
        SubAgentSource::Other(super::super::GUARDIAN_REVIEWER_NAME.to_string()),
        initial_history,
    )
    .await?;

    Ok(GuardianReviewSession::new(
        codex,
        cancel_token,
        reuse_key,
        has_prior_review,
    ))
}

pub(crate) async fn resolve_guardian_review_config(
    session: &Session,
    turn: &TurnContext,
) -> anyhow::Result<GuardianResolvedReviewConfig> {
    // Prefer the dedicated guardian model when it exists locally; otherwise fall back to the
    // parent turn's active model while still biasing toward low reasoning effort when supported.
    let live_network_config = match session.services.network_proxy.as_ref() {
        Some(network_proxy) => Some(network_proxy.proxy().current_cfg().await?),
        None => None,
    };
    let available_models = session
        .services
        .models_manager
        .list_models(RefreshStrategy::Offline)
        .await;
    let preferred_reasoning_effort = |supports_low: bool, fallback| {
        if supports_low {
            Some(codex_protocol::openai_models::ReasoningEffort::Low)
        } else {
            fallback
        }
    };
    let preferred_model = available_models
        .iter()
        .find(|preset| preset.model == super::super::GUARDIAN_PREFERRED_MODEL);
    let (guardian_model, guardian_reasoning_effort) = if let Some(preset) = preferred_model {
        let reasoning_effort = preferred_reasoning_effort(
            preset
                .supported_reasoning_efforts
                .iter()
                .any(|effort| effort.effort == codex_protocol::openai_models::ReasoningEffort::Low),
            Some(preset.default_reasoning_effort),
        );
        (
            super::super::GUARDIAN_PREFERRED_MODEL.to_string(),
            reasoning_effort,
        )
    } else {
        let reasoning_effort = preferred_reasoning_effort(
            turn.model_info
                .supported_reasoning_levels
                .iter()
                .any(|preset| preset.effort == codex_protocol::openai_models::ReasoningEffort::Low),
            turn.reasoning_effort
                .or(turn.model_info.default_reasoning_level),
        );
        (turn.model_info.slug.clone(), reasoning_effort)
    };
    let spawn_config = build_guardian_review_session_config(
        turn.config.as_ref(),
        live_network_config,
        guardian_model.as_str(),
        guardian_reasoning_effort,
    )?;

    Ok(GuardianResolvedReviewConfig {
        spawn_config,
        model: guardian_model,
        reasoning_effort: guardian_reasoning_effort,
    })
}

pub(crate) fn build_guardian_review_session_config(
    parent_config: &Config,
    live_network_config: Option<codex_network_proxy::NetworkProxyConfig>,
    active_model: &str,
    reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
) -> anyhow::Result<Config> {
    // Guardian inherits the parent config as a base, then tightens it into a locked-down
    // read-only child session. That keeps managed-network and instruction context aligned while
    // ensuring guardian itself cannot mutate workspace state.
    let mut guardian_config = parent_config.clone();
    guardian_config.model = Some(active_model.to_string());
    guardian_config.model_reasoning_effort = reasoning_effort;
    guardian_config.model_reasoning_summary = Some(ReasoningSummaryConfig::None);
    guardian_config.personality = None;
    guardian_config.developer_instructions = Some(
        parent_config
            .guardian_developer_instructions
            .clone()
            .unwrap_or_else(super::super::prompt::guardian_policy_prompt),
    );
    guardian_config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
    guardian_config.permissions.sandbox_policy =
        Constrained::allow_only(SandboxPolicy::new_read_only_policy());
    if let Some(live_network_config) = live_network_config
        && guardian_config.permissions.network.is_some()
    {
        let network_constraints = guardian_config
            .config_layer_stack
            .requirements()
            .network
            .as_ref()
            .map(|network| network.value.clone());
        guardian_config.permissions.network = Some(NetworkProxySpec::from_config_and_constraints(
            live_network_config,
            network_constraints,
            &SandboxPolicy::new_read_only_policy(),
        )?);
    }
    for feature in [
        Feature::SpawnCsv,
        Feature::Collab,
        Feature::WebSearchRequest,
        Feature::WebSearchCached,
    ] {
        guardian_config.features.disable(feature).map_err(|err| {
            anyhow::anyhow!(
                "guardian review session could not disable `features.{}`: {err}",
                feature.key()
            )
        })?;
        if guardian_config.features.enabled(feature) {
            anyhow::bail!(
                "guardian review session requires `features.{}` to be disabled",
                feature.key()
            );
        }
    }
    Ok(guardian_config)
}
