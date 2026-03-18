//! Private error types for OAuth token exchange failures.
//!
//! This module keeps transport/provider failure modeling separate from the localhost callback
//! orchestration in `server.rs`. The main goal is to preserve useful reqwest transport detail for
//! users and support while stripping the attached URL field that may carry sensitive OAuth data.

use crate::progress::LoginFailureCategory;
use serde_json::Value as JsonValue;

/// Failure from exchanging an OAuth authorization code for tokens.
///
/// This type separates protocol failures from transport failures so the caller can choose a stable
/// user-facing bucket without throwing away the underlying detail. Variants that wrap
/// [`reqwest::Error`] are constructed through helpers that strip the attached URL first; callers
/// should not build those variants directly unless they preserve that redaction step.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TokenExchangeError {
    /// The HTTP client could not be constructed for the token request.
    #[error("failed to prepare HTTP client for {endpoint}: {message}")]
    HttpClientSetup {
        /// Sanitized token endpoint identity for the error message.
        endpoint: String,
        /// Human-readable setup failure.
        message: String,
    },

    /// The token request failed before a usable HTTP response was received.
    #[error("{source} (endpoint: {endpoint})")]
    Transport {
        /// Sanitized token endpoint identity for the error message.
        endpoint: String,
        /// Transport error with the attached URL removed.
        #[source]
        source: reqwest::Error,
    },

    /// The token endpoint returned a non-success HTTP status.
    #[error("token endpoint returned status {status} from {endpoint}: {detail}")]
    EndpointRejected {
        /// Sanitized token endpoint identity for the error message.
        endpoint: String,
        /// HTTP status returned by the token endpoint.
        status: reqwest::StatusCode,
        /// Parsed error detail from the response body.
        detail: TokenEndpointErrorDetail,
    },

    /// The token endpoint returned success, but the body was not the expected token payload.
    #[error("token response from {endpoint} could not be parsed: {source}")]
    ResponseMalformed {
        /// Sanitized token endpoint identity for the error message.
        endpoint: String,
        /// Body parse error with the attached URL removed.
        #[source]
        source: reqwest::Error,
    },
}

impl TokenExchangeError {
    /// Builds a setup failure for the token endpoint HTTP client.
    pub(crate) fn http_client_setup(endpoint: String, message: String) -> Self {
        Self::HttpClientSetup { endpoint, message }
    }

    /// Builds a transport failure and strips the URL from the wrapped reqwest error.
    ///
    /// This preserves lower-level proxy/TLS/connect detail while keeping the potentially sensitive
    /// URL out of the source error's `Display` output.
    pub(crate) fn transport(endpoint: String, source: reqwest::Error) -> Self {
        Self::Transport {
            endpoint,
            source: source.without_url(),
        }
    }

    /// Builds a rejected-endpoint failure from a non-success HTTP response.
    pub(crate) fn endpoint_rejected(
        endpoint: String,
        status: reqwest::StatusCode,
        detail: TokenEndpointErrorDetail,
    ) -> Self {
        Self::EndpointRejected {
            endpoint,
            status,
            detail,
        }
    }

    /// Builds a malformed-response failure and strips the URL from the wrapped reqwest error.
    pub(crate) fn response_malformed(endpoint: String, source: reqwest::Error) -> Self {
        Self::ResponseMalformed {
            endpoint,
            source: source.without_url(),
        }
    }

    /// Maps this error to the coarse support/UI category used by progress events.
    ///
    /// This intentionally collapses many low-level transport causes into a small stable enum.
    /// Callers that need exact diagnostics should inspect the source chain or logs instead of
    /// adding more categories for every transient network failure.
    pub(crate) fn failure_category(&self) -> LoginFailureCategory {
        match self {
            TokenExchangeError::HttpClientSetup { .. } => {
                LoginFailureCategory::TokenExchangeRequest
            }
            TokenExchangeError::Transport { source, .. } => {
                if source.is_timeout() {
                    LoginFailureCategory::TokenExchangeTimeout
                } else if source.is_connect() {
                    LoginFailureCategory::TokenExchangeConnect
                } else {
                    LoginFailureCategory::TokenExchangeRequest
                }
            }
            TokenExchangeError::EndpointRejected { .. } => {
                LoginFailureCategory::TokenEndpointRejected
            }
            TokenExchangeError::ResponseMalformed { .. } => {
                LoginFailureCategory::TokenResponseMalformed
            }
        }
    }

    /// Returns the wrapped reqwest error when one exists.
    ///
    /// This is only for structured logging and transport classification. A caller that formats the
    /// returned source directly into normal logs should remember that only the URL was stripped;
    /// other low-level error text is intentionally preserved.
    pub(crate) fn as_transport_error(&self) -> Option<&reqwest::Error> {
        match self {
            TokenExchangeError::Transport { source, .. }
            | TokenExchangeError::ResponseMalformed { source, .. } => Some(source),
            TokenExchangeError::HttpClientSetup { .. }
            | TokenExchangeError::EndpointRejected { .. } => None,
        }
    }
}

/// Parsed token endpoint error detail for structured logs and caller-visible error text.
///
/// The parsed fields are the reviewed, structured subset that can be logged directly. The
/// `Display` form may preserve a raw non-JSON body for the returned error path, so callers should
/// avoid logging this type with `{}` unless they intend to surface backend text to a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenEndpointErrorDetail {
    /// Parsed OAuth error code, when present.
    pub(crate) error_code: Option<String>,
    /// Parsed provider message, when present.
    pub(crate) error_message: Option<String>,
    /// Best-effort text for the caller-visible error message.
    display_message: String,
}

impl std::fmt::Display for TokenEndpointErrorDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.display_message.fmt(f)
    }
}

/// Extracts token endpoint error detail for both structured logging and caller-visible errors.
///
/// Parsed JSON fields are safe to log individually. If the response is not JSON, the raw body is
/// preserved only for the returned error path so the CLI/browser can still surface the backend
/// detail, while the structured log path continues to use the explicitly parsed safe fields above.
pub(crate) fn parse_token_endpoint_error(body: &str) -> TokenEndpointErrorDetail {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return TokenEndpointErrorDetail {
            error_code: None,
            error_message: None,
            display_message: "unknown error".to_string(),
        };
    }

    let parsed = serde_json::from_str::<JsonValue>(trimmed).ok();
    if let Some(json) = parsed {
        let error_code = json
            .get("error")
            .and_then(JsonValue::as_str)
            .filter(|error_code| !error_code.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                json.get("error")
                    .and_then(JsonValue::as_object)
                    .and_then(|error_obj| error_obj.get("code"))
                    .and_then(JsonValue::as_str)
                    .filter(|code| !code.trim().is_empty())
                    .map(ToString::to_string)
            });
        if let Some(description) = json.get("error_description").and_then(JsonValue::as_str)
            && !description.trim().is_empty()
        {
            return TokenEndpointErrorDetail {
                error_code,
                error_message: Some(description.to_string()),
                display_message: description.to_string(),
            };
        }
        if let Some(error_obj) = json.get("error")
            && let Some(message) = error_obj.get("message").and_then(JsonValue::as_str)
            && !message.trim().is_empty()
        {
            return TokenEndpointErrorDetail {
                error_code,
                error_message: Some(message.to_string()),
                display_message: message.to_string(),
            };
        }
        if let Some(error_code) = error_code {
            return TokenEndpointErrorDetail {
                display_message: error_code.clone(),
                error_code: Some(error_code),
                error_message: None,
            };
        }
    }

    // Preserve non-JSON token-endpoint bodies for the returned error so CLI/browser flows still
    // surface the backend detail users and admins need, but keep that text out of structured logs
    // by only logging explicitly parsed fields above and avoiding `%err` logging at the callback
    // layer.
    TokenEndpointErrorDetail {
        error_code: None,
        error_message: None,
        display_message: trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::TokenEndpointErrorDetail;
    use super::parse_token_endpoint_error;

    #[test]
    fn parse_token_endpoint_error_prefers_error_description() {
        let detail = parse_token_endpoint_error(
            r#"{"error":"invalid_grant","error_description":"refresh token expired"}"#,
        );

        assert_eq!(
            detail,
            TokenEndpointErrorDetail {
                error_code: Some("invalid_grant".to_string()),
                error_message: Some("refresh token expired".to_string()),
                display_message: "refresh token expired".to_string(),
            }
        );
    }

    #[test]
    fn parse_token_endpoint_error_reads_nested_error_message_and_code() {
        let detail = parse_token_endpoint_error(
            r#"{"error":{"code":"proxy_auth_required","message":"proxy authentication required"}}"#,
        );

        assert_eq!(
            detail,
            TokenEndpointErrorDetail {
                error_code: Some("proxy_auth_required".to_string()),
                error_message: Some("proxy authentication required".to_string()),
                display_message: "proxy authentication required".to_string(),
            }
        );
    }

    #[test]
    fn parse_token_endpoint_error_falls_back_to_error_code() {
        let detail = parse_token_endpoint_error(r#"{"error":"temporarily_unavailable"}"#);

        assert_eq!(
            detail,
            TokenEndpointErrorDetail {
                error_code: Some("temporarily_unavailable".to_string()),
                error_message: None,
                display_message: "temporarily_unavailable".to_string(),
            }
        );
    }

    #[test]
    fn parse_token_endpoint_error_preserves_plain_text_for_display() {
        let detail = parse_token_endpoint_error("service unavailable");

        assert_eq!(
            detail,
            TokenEndpointErrorDetail {
                error_code: None,
                error_message: None,
                display_message: "service unavailable".to_string(),
            }
        );
    }
}
