use std::sync::Arc;

use crate::api_bridge::CoreAuthProvider;
use crate::api_bridge::auth_provider_from_resolved_provider_token;
use crate::auth::AuthManager;
use crate::auth::AuthMode;
use crate::auth::CodexAuth;
use crate::auth::RefreshTokenError;
use crate::auth::UnauthorizedRecovery;
use crate::error::CodexErr;
use crate::error::Result;
use crate::model_provider_info::ModelProviderInfo;
use crate::provider_auth::ProviderAuthResolver;

#[derive(Clone)]
pub(crate) struct ResolvedRequestAuth {
    pub(crate) auth: Option<CodexAuth>,
    pub(crate) auth_mode: Option<AuthMode>,
    pub(crate) api_provider: codex_api::Provider,
    pub(crate) api_auth: CoreAuthProvider,
}

pub(crate) async fn resolve_request_auth(
    auth_manager: Option<&Arc<AuthManager>>,
    provider: &ModelProviderInfo,
    provider_auth: &ProviderAuthResolver,
) -> Result<ResolvedRequestAuth> {
    let auth = match auth_manager {
        Some(manager) => manager.auth().await,
        None => None,
    };
    let auth_mode = auth.as_ref().map(CodexAuth::auth_mode);
    let provider_token = provider_auth.resolve_token().await?;
    let api_provider = provider.to_api_provider(auth_mode)?;
    let api_auth =
        auth_provider_from_resolved_provider_token(auth.clone(), provider, provider_token)?;
    Ok(ResolvedRequestAuth {
        auth,
        auth_mode,
        api_provider,
        api_auth,
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnauthorizedRecoveryExecution {
    pub(crate) mode: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) auth_state_changed: Option<bool>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnauthorizedRecoveryUnavailable {
    pub(crate) mode: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) reason: &'static str,
}

#[derive(Debug)]
pub(crate) enum UnauthorizedRecoveryOutcome {
    Recovered(UnauthorizedRecoveryExecution),
    Unavailable(UnauthorizedRecoveryUnavailable),
}

#[derive(Debug)]
pub(crate) enum UnauthorizedRecoveryError {
    Chatgpt {
        execution: UnauthorizedRecoveryExecution,
        error: RefreshTokenError,
    },
    Provider {
        execution: UnauthorizedRecoveryExecution,
        error: CodexErr,
    },
}

impl UnauthorizedRecoveryError {
    pub(crate) fn into_codex_err(self) -> CodexErr {
        match self {
            Self::Chatgpt { error, .. } => match error {
                RefreshTokenError::Permanent(failed) => CodexErr::RefreshTokenFailed(failed),
                RefreshTokenError::Transient(error) => CodexErr::Io(error),
            },
            Self::Provider { error, .. } => error,
        }
    }
}

pub(crate) struct RequestUnauthorizedRecovery {
    provider_auth: ProviderAuthResolver,
    provider_auth_retry_available: bool,
    auth_recovery: Option<UnauthorizedRecovery>,
}

impl RequestUnauthorizedRecovery {
    pub(crate) fn new(
        auth_manager: Option<&Arc<AuthManager>>,
        provider_auth: &ProviderAuthResolver,
    ) -> Self {
        Self {
            provider_auth: provider_auth.clone(),
            provider_auth_retry_available: provider_auth.is_configured(),
            auth_recovery: auth_manager.map(AuthManager::unauthorized_recovery),
        }
    }

    pub(crate) async fn next(
        &mut self,
    ) -> std::result::Result<UnauthorizedRecoveryOutcome, UnauthorizedRecoveryError> {
        if self.provider_auth_retry_available {
            self.provider_auth_retry_available = false;
            let execution = UnauthorizedRecoveryExecution {
                mode: "provider_exec",
                phase: "refresh",
                auth_state_changed: None,
            };
            return match self.provider_auth.refresh_after_unauthorized().await {
                Ok(auth_state_changed) => Ok(UnauthorizedRecoveryOutcome::Recovered(
                    UnauthorizedRecoveryExecution {
                        auth_state_changed,
                        ..execution
                    },
                )),
                Err(error) => Err(UnauthorizedRecoveryError::Provider { execution, error }),
            };
        }

        if let Some(recovery) = self.auth_recovery.as_mut()
            && recovery.has_next()
        {
            let execution = UnauthorizedRecoveryExecution {
                mode: recovery.mode_name(),
                phase: recovery.step_name(),
                auth_state_changed: None,
            };
            return match recovery.next().await {
                Ok(step_result) => Ok(UnauthorizedRecoveryOutcome::Recovered(
                    UnauthorizedRecoveryExecution {
                        auth_state_changed: step_result.auth_state_changed(),
                        ..execution
                    },
                )),
                Err(error) => Err(UnauthorizedRecoveryError::Chatgpt { execution, error }),
            };
        }

        let unavailable = match self.auth_recovery.as_ref() {
            Some(recovery) => UnauthorizedRecoveryUnavailable {
                mode: recovery.mode_name(),
                phase: recovery.step_name(),
                reason: recovery.unavailable_reason(),
            },
            None => UnauthorizedRecoveryUnavailable {
                mode: "none",
                phase: "none",
                reason: "auth_manager_missing",
            },
        };
        Ok(UnauthorizedRecoveryOutcome::Unavailable(unavailable))
    }
}
