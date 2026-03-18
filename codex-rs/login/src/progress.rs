//! Progress, failure vocabulary, and default text rendering for OAuth login.
//!
//! This module describes what happened in the login flow in a UI-neutral form. The `codex-login`
//! crate emits phases and coarse failure categories so direct CLI, TUI, or app-server callers can
//! choose their own presentation while still sharing the same state machine and support-oriented
//! error buckets.
//!
//! The types here are intentionally coarse and redaction-friendly. Callback events report only the
//! presence or validity of sensitive query fields, never the raw authorization code, state value,
//! or provider error payload, and the failure category is a stable bucket rather than a complete
//! transport diagnosis.

const LOGIN_HELP_URL: &str = "https://developers.openai.com/codex/auth";

/// Coarse-grained progress phases for the OAuth login flow.
///
/// This is a flow contract, not a direct UI contract. Callers should treat variants as lifecycle
/// milestones and decide locally which ones are worth rendering to users; many phases exist only so
/// logs, tests, and future UIs can tell where a failure occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginPhase {
    /// The localhost callback server is attempting to bind its listening port.
    BindingLocalServer {
        /// One-based bind attempt count.
        attempt: u32,
    },
    /// A previous login server appears to still own the callback port, so this attempt is
    /// cancelling the stale session before retrying.
    PreviousLoginServerDetected {
        /// One-based bind attempt count that observed the stale server.
        attempt: u32,
    },
    /// The local callback listener is bound and has an assigned port.
    LocalServerBound {
        /// The bound localhost port for the OAuth redirect URI.
        port: u16,
    },
    /// Codex is launching the browser for the provider authorization page.
    OpeningBrowser,
    /// Codex is waiting for the browser to return to the localhost callback.
    WaitingForCallback,
    /// A callback request reached the local server.
    CallbackReceived {
        /// Whether the callback carried an authorization code parameter.
        has_code: bool,
        /// Whether the callback carried a state parameter.
        has_state: bool,
        /// Whether the callback carried a provider error parameter.
        has_error: bool,
        /// Whether the received state matched the one generated for this attempt.
        state_valid: bool,
    },
    /// Codex is exchanging the authorization code for tokens at the token endpoint.
    ExchangingToken,
    /// Codex is writing the resulting credentials to local storage.
    PersistingCredentials,
    /// The browser login flow finished successfully.
    Succeeded,
    /// The browser login flow failed.
    Failed {
        /// The stage of the login flow where the failure occurred.
        phase: LoginFailurePhase,
        /// The stable coarse-grained failure bucket for UI and support.
        category: LoginFailureCategory,
        /// Human-readable detail for the specific failure instance.
        message: String,
    },
}

impl LoginPhase {
    /// Returns whether this phase should be rendered by the default direct-CLI progress UI.
    ///
    /// Some phases are useful for logging and tests but too noisy for normal stderr output.
    pub fn is_user_visible(&self) -> bool {
        match self {
            LoginPhase::BindingLocalServer { attempt } => *attempt > 1,
            LoginPhase::PreviousLoginServerDetected { .. }
            | LoginPhase::OpeningBrowser
            | LoginPhase::PersistingCredentials
            | LoginPhase::Failed { .. } => true,
            LoginPhase::CallbackReceived {
                has_error,
                state_valid,
                ..
            } => !has_error && *state_valid,
            LoginPhase::LocalServerBound { .. }
            | LoginPhase::WaitingForCallback
            | LoginPhase::ExchangingToken
            | LoginPhase::Succeeded => false,
        }
    }
}

impl std::fmt::Display for LoginPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginPhase::BindingLocalServer { attempt } => {
                if *attempt > 1 {
                    write!(
                        f,
                        "Retrying the local sign-in listener... (attempt {attempt})"
                    )
                } else {
                    f.write_str("Starting local sign-in listener...")
                }
            }
            LoginPhase::PreviousLoginServerDetected { .. } => {
                f.write_str("Cleaning up an old login session first...")
            }
            LoginPhase::LocalServerBound { port } => {
                write!(f, "Local callback server ready on http://localhost:{port}")
            }
            LoginPhase::OpeningBrowser => f.write_str("Opening your browser to sign in..."),
            LoginPhase::WaitingForCallback => f.write_str("Waiting for browser sign-in..."),
            LoginPhase::CallbackReceived {
                has_error,
                state_valid,
                ..
            } => {
                if !has_error && *state_valid {
                    f.write_str("Browser sign-in received. Finishing up...")
                } else {
                    f.write_str("Browser sign-in callback received")
                }
            }
            LoginPhase::ExchangingToken => {
                f.write_str("Exchanging authorization code for tokens...")
            }
            LoginPhase::PersistingCredentials => f.write_str("Saving your Codex credentials..."),
            LoginPhase::Succeeded => f.write_str("Signed in. You're good to go."),
            LoginPhase::Failed {
                phase,
                category,
                message,
            } => write!(
                f,
                "{}\nCodex couldn't finish signing in while {phase}. {}\nHelp: {LOGIN_HELP_URL}\nDetails: {category} - {message}",
                category.title(),
                category.help()
            ),
        }
    }
}

/// Where in the login flow a failure happened.
///
/// This locates the failing stage independently from the failure category. For example, a token
/// exchange can fail because the server rejected the request or because the network path was
/// unavailable; both happen in `ExchangeToken`, but they produce different categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFailurePhase {
    /// Failed before or while binding the localhost callback server.
    BindLocalServer,
    /// Failed while launching the browser.
    OpenBrowser,
    /// Failed while waiting for the OAuth callback to arrive.
    WaitForCallback,
    /// Failed while validating callback parameters such as state or authorization code presence.
    ValidateCallback,
    /// Failed while exchanging the authorization code for tokens.
    ExchangeToken,
    /// Failed while persisting credentials after a successful token exchange.
    PersistCredentials,
    /// Failed while redirecting the browser back into Codex after the callback completed.
    RedirectBackToCodex,
}

impl std::fmt::Display for LoginFailurePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LoginFailurePhase::BindLocalServer => "binding local callback server",
            LoginFailurePhase::OpenBrowser => "opening browser",
            LoginFailurePhase::WaitForCallback => "waiting for OAuth callback",
            LoginFailurePhase::ValidateCallback => "validating OAuth callback",
            LoginFailurePhase::ExchangeToken => "exchanging authorization code for tokens",
            LoginFailurePhase::PersistCredentials => "saving credentials locally",
            LoginFailurePhase::RedirectBackToCodex => "redirecting back to Codex",
        })
    }
}

/// Stable high-level failure categories for UI and support messaging.
///
/// These names are intended to be durable enough for snapshots, support references, and future UI
/// branching. They are broader than the underlying transport or provider errors by design, so a
/// caller should not assume one category maps to exactly one root cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFailureCategory {
    /// The local callback server could not become available.
    LocalServerUnavailable,
    /// The system browser could not be launched.
    BrowserLaunchFailed,
    /// The login attempt was cancelled before completion.
    LoginCancelled,
    /// The callback state did not match the state generated for this attempt.
    CallbackStateMismatch,
    /// The provider redirected back with an OAuth error instead of a code.
    ProviderCallbackError,
    /// The callback did not include an authorization code.
    MissingAuthorizationCode,
    /// The token endpoint request timed out.
    TokenExchangeTimeout,
    /// The token endpoint could not be reached due to a connect-level failure.
    TokenExchangeConnect,
    /// The token endpoint request failed for a non-timeout, non-connect transport reason.
    TokenExchangeRequest,
    /// The token endpoint returned a non-success HTTP status.
    TokenEndpointRejected,
    /// The token endpoint returned a response body Codex could not parse.
    TokenResponseMalformed,
    /// The signed-in account is not allowed in the selected workspace context.
    WorkspaceRestriction,
    /// Codex could not save credentials locally.
    PersistFailed,
    /// Codex could not send the browser to the final post-login page.
    RedirectFailed,
}

impl std::fmt::Display for LoginFailureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LoginFailureCategory::LocalServerUnavailable => "local_server_unavailable",
            LoginFailureCategory::BrowserLaunchFailed => "browser_launch_failed",
            LoginFailureCategory::LoginCancelled => "login_cancelled",
            LoginFailureCategory::CallbackStateMismatch => "callback_state_mismatch",
            LoginFailureCategory::ProviderCallbackError => "provider_callback_error",
            LoginFailureCategory::MissingAuthorizationCode => "missing_authorization_code",
            LoginFailureCategory::TokenExchangeTimeout => "token_exchange_timeout",
            LoginFailureCategory::TokenExchangeConnect => "token_exchange_connect",
            LoginFailureCategory::TokenExchangeRequest => "token_exchange_request",
            LoginFailureCategory::TokenEndpointRejected => "token_endpoint_rejected",
            LoginFailureCategory::TokenResponseMalformed => "token_response_malformed",
            LoginFailureCategory::WorkspaceRestriction => "workspace_restriction",
            LoginFailureCategory::PersistFailed => "persist_failed",
            LoginFailureCategory::RedirectFailed => "redirect_failed",
        })
    }
}

impl LoginFailureCategory {
    /// Returns the short user-facing title for this failure category.
    pub fn title(self) -> &'static str {
        match self {
            LoginFailureCategory::BrowserLaunchFailed => "Couldn't open your browser",
            LoginFailureCategory::LocalServerUnavailable => "Couldn't start local sign-in",
            LoginFailureCategory::LoginCancelled => "Sign-in was cancelled",
            LoginFailureCategory::CallbackStateMismatch => "Couldn't verify this sign-in attempt",
            LoginFailureCategory::ProviderCallbackError => "The sign-in page reported a problem",
            LoginFailureCategory::MissingAuthorizationCode => "Didn't get a sign-in code back",
            LoginFailureCategory::TokenExchangeTimeout => {
                "The auth server took too long to respond"
            }
            LoginFailureCategory::TokenExchangeConnect
            | LoginFailureCategory::TokenExchangeRequest => "Couldn't reach the auth server",
            LoginFailureCategory::TokenEndpointRejected => "The auth server rejected the sign-in",
            LoginFailureCategory::TokenResponseMalformed => {
                "The auth server sent an unexpected response"
            }
            LoginFailureCategory::WorkspaceRestriction => "This account can't be used here",
            LoginFailureCategory::PersistFailed => "Couldn't save your Codex credentials",
            LoginFailureCategory::RedirectFailed => "Couldn't return to Codex after sign-in",
        }
    }

    /// Returns one concrete recovery hint for this failure category.
    pub fn help(self) -> &'static str {
        match self {
            LoginFailureCategory::BrowserLaunchFailed => {
                "Use the sign-in link printed above, or run `codex login --device-auth` on a remote machine."
            }
            LoginFailureCategory::LocalServerUnavailable => {
                "Retry in a moment. If it keeps happening, another process may be holding the local callback port."
            }
            LoginFailureCategory::LoginCancelled => "Run `codex login` to try again.",
            LoginFailureCategory::CallbackStateMismatch => {
                "Retry sign-in from the same terminal, and avoid reusing an old browser tab."
            }
            LoginFailureCategory::ProviderCallbackError => {
                "Try again, switch accounts, or contact your workspace admin if access is restricted."
            }
            LoginFailureCategory::MissingAuthorizationCode => {
                "Try again. If it keeps happening, restart the login flow from Codex."
            }
            LoginFailureCategory::TokenExchangeTimeout => {
                "Check your network connection or proxy, then try again."
            }
            LoginFailureCategory::TokenExchangeConnect
            | LoginFailureCategory::TokenExchangeRequest => {
                "Check your network, proxy, or custom CA setup, then try again."
            }
            LoginFailureCategory::TokenEndpointRejected => {
                "Try again. If this repeats, your account or workspace may not be allowed to use Codex."
            }
            LoginFailureCategory::TokenResponseMalformed => {
                "Try again. If this repeats, contact support with the details below."
            }
            LoginFailureCategory::WorkspaceRestriction => {
                "Switch to an allowed account or contact your workspace admin."
            }
            LoginFailureCategory::PersistFailed => {
                "Check permissions for your Codex home directory, then try again."
            }
            LoginFailureCategory::RedirectFailed => "Return to Codex and retry sign-in.",
        }
    }
}

/// Channel sender used to publish browser-login progress events.
///
/// The sender carries structured phases rather than formatted text so the login crate does not bake
/// in one presentation style. A caller that needs human-facing copy should render it at the UI
/// boundary instead of matching on these variants inside the auth flow.
pub type LoginProgressSender = tokio::sync::mpsc::UnboundedSender<LoginPhase>;

#[cfg(test)]
mod tests {
    use super::LoginFailureCategory;
    use super::LoginFailurePhase;
    use super::LoginPhase;

    #[test]
    fn login_progress_snapshots() {
        let samples = [
            (
                "progress_retry_local_listener",
                LoginPhase::BindingLocalServer { attempt: 2 },
            ),
            (
                "progress_cleanup_old_login_session",
                LoginPhase::PreviousLoginServerDetected { attempt: 1 },
            ),
            ("progress_opening_browser", LoginPhase::OpeningBrowser),
            (
                "progress_callback_received",
                LoginPhase::CallbackReceived {
                    has_code: true,
                    has_state: true,
                    has_error: false,
                    state_valid: true,
                },
            ),
            (
                "progress_saving_credentials",
                LoginPhase::PersistingCredentials,
            ),
        ];

        for (name, phase) in samples {
            insta::assert_snapshot!(name, phase.to_string());
        }
    }

    #[test]
    fn login_failure_snapshots() {
        let samples = [
            (
                "failure_local_server_unavailable",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::BindLocalServer,
                    category: LoginFailureCategory::LocalServerUnavailable,
                    message: "Port 127.0.0.1:1455 is already in use after 2000 ms".to_string(),
                },
            ),
            (
                "failure_browser_launch_failed",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::OpenBrowser,
                    category: LoginFailureCategory::BrowserLaunchFailed,
                    message: "No browser found".to_string(),
                },
            ),
            (
                "failure_login_cancelled",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::WaitForCallback,
                    category: LoginFailureCategory::LoginCancelled,
                    message: "Login was not completed".to_string(),
                },
            ),
            (
                "failure_callback_state_mismatch",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ValidateCallback,
                    category: LoginFailureCategory::CallbackStateMismatch,
                    message: "State mismatch".to_string(),
                },
            ),
            (
                "failure_provider_callback_error",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ValidateCallback,
                    category: LoginFailureCategory::ProviderCallbackError,
                    message: "Sign-in failed: access_denied".to_string(),
                },
            ),
            (
                "failure_missing_authorization_code",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ValidateCallback,
                    category: LoginFailureCategory::MissingAuthorizationCode,
                    message: "Missing authorization code. Sign-in could not be completed."
                        .to_string(),
                },
            ),
            (
                "failure_token_exchange_timeout",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ExchangeToken,
                    category: LoginFailureCategory::TokenExchangeTimeout,
                    message: "operation timed out".to_string(),
                },
            ),
            (
                "failure_token_exchange_connect",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ExchangeToken,
                    category: LoginFailureCategory::TokenExchangeConnect,
                    message:
                        "error sending request (endpoint: https://auth.openai.com/oauth/token)"
                            .to_string(),
                },
            ),
            (
                "failure_token_exchange_request",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ExchangeToken,
                    category: LoginFailureCategory::TokenExchangeRequest,
                    message: "request failed".to_string(),
                },
            ),
            (
                "failure_token_endpoint_rejected",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ExchangeToken,
                    category: LoginFailureCategory::TokenEndpointRejected,
                    message: "token endpoint returned status 403 Forbidden: denied".to_string(),
                },
            ),
            (
                "failure_token_response_malformed",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ExchangeToken,
                    category: LoginFailureCategory::TokenResponseMalformed,
                    message: "expected value at line 1 column 1".to_string(),
                },
            ),
            (
                "failure_workspace_restriction",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::ValidateCallback,
                    category: LoginFailureCategory::WorkspaceRestriction,
                    message: "Login is restricted to workspace id org-required".to_string(),
                },
            ),
            (
                "failure_persist_failed",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::PersistCredentials,
                    category: LoginFailureCategory::PersistFailed,
                    message: "permission denied".to_string(),
                },
            ),
            (
                "failure_redirect_failed",
                LoginPhase::Failed {
                    phase: LoginFailurePhase::RedirectBackToCodex,
                    category: LoginFailureCategory::RedirectFailed,
                    message: "invalid redirect header".to_string(),
                },
            ),
        ];

        for (name, phase) in samples {
            insta::assert_snapshot!(name, phase.to_string());
        }
    }
}
