use std::fmt;
use std::io;
use std::sync::Arc;

use crate::api_bridge::CoreAuthProvider;
use crate::api_bridge::auth_provider_from_resolved_provider_token;
use crate::auth::AuthManager;
use crate::auth::AuthMode;
use crate::auth::CodexAuth;
use crate::auth::ExternalAuthRefreshContext;
use crate::auth::ExternalAuthRefreshReason;
use crate::auth::ExternalAuthRefresher;
use crate::auth::RefreshTokenError;
use crate::auth::UnauthorizedRecovery;
use crate::error::CodexErr;
use crate::error::Result;
use crate::model_provider_info::ModelProviderInfo;

#[derive(Clone, Default)]
pub(crate) struct RequestAuthContext {
    auth_manager: Option<Arc<AuthManager>>,
    external_request_auth_source: Option<Arc<dyn ExternalAuthRefresher>>,
}

impl RequestAuthContext {
    pub(crate) fn new(
        auth_manager: Option<Arc<AuthManager>>,
        external_request_auth_source: Option<Arc<dyn ExternalAuthRefresher>>,
    ) -> Self {
        Self {
            auth_manager,
            external_request_auth_source,
        }
    }

    pub(crate) async fn resolve(
        &self,
        provider: &ModelProviderInfo,
    ) -> Result<ResolvedRequestAuth> {
        let auth = match self.auth_manager.as_ref() {
            Some(manager) => manager.auth().await,
            None => None,
        };
        let auth_mode = auth.as_ref().map(CodexAuth::auth_mode);
        let request_auth_access_token = self.resolve_external_request_auth_access_token().await?;
        let api_provider = provider.to_api_provider(auth_mode)?;
        let api_auth = auth_provider_from_resolved_provider_token(
            auth.clone(),
            provider,
            request_auth_access_token.clone(),
        )?;
        Ok(ResolvedRequestAuth {
            auth,
            auth_mode,
            api_provider,
            api_auth,
            request_auth_access_token,
        })
    }

    pub(crate) fn unauthorized_recovery(&self) -> RequestUnauthorizedRecovery {
        RequestUnauthorizedRecovery::new(self)
    }

    pub(crate) fn auth_manager(&self) -> Option<&Arc<AuthManager>> {
        self.auth_manager.as_ref()
    }

    pub(crate) async fn resolve_external_request_auth_access_token(
        &self,
    ) -> Result<Option<String>> {
        let Some(source) = self.external_request_auth_source.as_ref() else {
            return Ok(None);
        };

        let tokens = source
            .resolve()
            .await
            .map_err(map_external_request_auth_error)?;
        Ok(tokens.map(|tokens| tokens.access_token))
    }
}

impl fmt::Debug for RequestAuthContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestAuthContext")
            .field("has_auth_manager", &self.auth_manager.is_some())
            .field(
                "has_external_request_auth_source",
                &self.external_request_auth_source.is_some(),
            )
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct ResolvedRequestAuth {
    pub(crate) auth: Option<CodexAuth>,
    pub(crate) auth_mode: Option<AuthMode>,
    pub(crate) api_provider: codex_api::Provider,
    pub(crate) api_auth: CoreAuthProvider,
    pub(crate) request_auth_access_token: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnauthorizedRecoveryExecution {
    pub(crate) mode: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) auth_state_changed: Option<bool>,
    pub(crate) refreshes_request_auth: bool,
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
    RequestAuthSource {
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
            Self::RequestAuthSource { error, .. } => error,
        }
    }
}

/// Tracks which auth-recovery steps are still available after a request fails with `401`.
///
/// The request loops reuse the same instance across retries so this type can enforce the
/// recovery ordering and make sure each one-shot step only runs once.
pub(crate) struct RequestUnauthorizedRecovery {
    external_request_auth_source: Option<Arc<dyn ExternalAuthRefresher>>,
    external_request_auth_retry_remaining: bool,
    current_request_auth_access_token: Option<String>,
    auth_recovery: Option<UnauthorizedRecovery>,
}

impl RequestUnauthorizedRecovery {
    pub(crate) fn new(request_auth: &RequestAuthContext) -> Self {
        Self {
            external_request_auth_source: request_auth.external_request_auth_source.clone(),
            external_request_auth_retry_remaining: request_auth
                .external_request_auth_source
                .is_some(),
            current_request_auth_access_token: None,
            auth_recovery: request_auth
                .auth_manager
                .as_ref()
                .map(AuthManager::unauthorized_recovery),
        }
    }

    pub(crate) fn set_current_request_auth_access_token(&mut self, access_token: Option<String>) {
        self.current_request_auth_access_token = access_token;
    }

    /// Runs the next available auth-recovery step, if any.
    ///
    /// Request-auth refresh is attempted first because it repairs the bearer token attached to the
    /// failing request directly. If no request-auth source is configured for the request, or the
    /// retry has already been consumed, recovery falls back to the ChatGPT auth-manager flow.
    /// Once all recovery steps have been consumed, this returns
    /// `UnauthorizedRecoveryOutcome::Unavailable` so the caller can stop retrying.
    pub(crate) async fn next(
        &mut self,
    ) -> std::result::Result<UnauthorizedRecoveryOutcome, UnauthorizedRecoveryError> {
        if self.external_request_auth_retry_remaining
            && let Some(source) = self.external_request_auth_source.as_ref()
            && self.current_request_auth_access_token.is_some()
        {
            self.external_request_auth_retry_remaining = false;
            let execution = UnauthorizedRecoveryExecution {
                mode: "external_request_auth",
                phase: "refresh",
                auth_state_changed: None,
                refreshes_request_auth: true,
            };
            return match source
                .refresh_after_unauthorized(
                    ExternalAuthRefreshContext {
                        reason: ExternalAuthRefreshReason::Unauthorized,
                        previous_account_id: None,
                    },
                    self.current_request_auth_access_token.as_deref(),
                )
                .await
            {
                Ok(auth_state_changed) => Ok(UnauthorizedRecoveryOutcome::Recovered(
                    UnauthorizedRecoveryExecution {
                        auth_state_changed,
                        ..execution
                    },
                )),
                Err(error) => Err(UnauthorizedRecoveryError::RequestAuthSource {
                    execution,
                    error: map_external_request_auth_error(error),
                }),
            };
        }

        if let Some(recovery) = self.auth_recovery.as_mut()
            && recovery.has_next()
        {
            let execution = UnauthorizedRecoveryExecution {
                mode: recovery.mode_name(),
                phase: recovery.step_name(),
                auth_state_changed: None,
                refreshes_request_auth: false,
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

        if self.external_request_auth_source.is_some()
            && self.current_request_auth_access_token.is_none()
        {
            return Ok(UnauthorizedRecoveryOutcome::Unavailable(
                UnauthorizedRecoveryUnavailable {
                    mode: "external_request_auth",
                    phase: "refresh",
                    reason: "request_auth_not_attached",
                },
            ));
        }

        if self.external_request_auth_source.is_some()
            && !self.external_request_auth_retry_remaining
            && self.auth_recovery.is_none()
        {
            return Ok(UnauthorizedRecoveryOutcome::Unavailable(
                UnauthorizedRecoveryUnavailable {
                    mode: "external_request_auth",
                    phase: "refresh",
                    reason: "request_auth_recovery_exhausted",
                },
            ));
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

fn map_external_request_auth_error(error: io::Error) -> CodexErr {
    CodexErr::InvalidRequest(error.to_string())
}
