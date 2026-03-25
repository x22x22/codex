//! Browser-based OAuth flow for provisioning OpenAI project API keys.

use std::time::Duration;

use codex_client::build_reqwest_client_with_custom_ca;
use reqwest::Client;
use reqwest::Method;
use serde::Deserialize;
use url::Url;

use crate::pkce::PkceCodes;
use crate::server::AuthorizationCodeServer;
use crate::server::start_authorization_code_server;

const AUTH_ISSUER: &str = "https://auth.openai.com";
const PLATFORM_HYDRA_CLIENT_ID: &str = "app_2SKx67EdpoN0G6j64rFvigXD";
const PLATFORM_AUDIENCE: &str = "https://api.openai.com/v1";
const DEFAULT_API_BASE: &str = "https://api.openai.com";
const DEFAULT_CALLBACK_PORT: u16 = 5000;
const DEFAULT_CALLBACK_PATH: &str = "/auth/callback";
const DEFAULT_SCOPE: &str = "openid email profile offline_access";
const DEFAULT_APP: &str = "api";
const DEFAULT_USER_AGENT: &str = "OpenAI-Onboard-Auth-Script/1.0";
const DEFAULT_PROJECT_API_KEY_NAME: &str = "Codex CLI";
const DEFAULT_PROJECT_POLL_INTERVAL_SECONDS: u64 = 10;
const DEFAULT_PROJECT_POLL_TIMEOUT_SECONDS: u64 = 60;
const OAUTH_TIMEOUT_SECONDS: u64 = 15 * 60;
const HTTP_TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiProvisionOptions {
    pub issuer: String,
    pub client_id: String,
    pub audience: String,
    pub api_base: String,
    pub app: String,
    pub callback_port: u16,
    pub scope: String,
    pub api_key_name: String,
    pub project_poll_interval_seconds: u64,
    pub project_poll_timeout_seconds: u64,
}

impl Default for ApiProvisionOptions {
    fn default() -> Self {
        Self {
            issuer: AUTH_ISSUER.to_string(),
            client_id: PLATFORM_HYDRA_CLIENT_ID.to_string(),
            audience: PLATFORM_AUDIENCE.to_string(),
            api_base: DEFAULT_API_BASE.to_string(),
            app: DEFAULT_APP.to_string(),
            callback_port: DEFAULT_CALLBACK_PORT,
            scope: DEFAULT_SCOPE.to_string(),
            api_key_name: DEFAULT_PROJECT_API_KEY_NAME.to_string(),
            project_poll_interval_seconds: DEFAULT_PROJECT_POLL_INTERVAL_SECONDS,
            project_poll_timeout_seconds: DEFAULT_PROJECT_POLL_TIMEOUT_SECONDS,
        }
    }
}

pub struct PendingApiProvisioning {
    client: Client,
    options: ApiProvisionOptions,
    redirect_uri: String,
    code_verifier: String,
    callback_server: AuthorizationCodeServer,
}

impl PendingApiProvisioning {
    pub fn auth_url(&self) -> &str {
        &self.callback_server.auth_url
    }

    pub fn callback_port(&self) -> u16 {
        self.callback_server.actual_port
    }

    pub fn open_browser(&self) -> bool {
        self.callback_server.open_browser()
    }

    pub fn open_browser_or_print(&self) -> bool {
        self.callback_server.open_browser_or_print()
    }

    pub async fn finish(self) -> Result<ProvisionedApiKey, HelperError> {
        let code = self
            .callback_server
            .wait_for_code(Duration::from_secs(OAUTH_TIMEOUT_SECONDS))
            .await
            .map_err(|err| HelperError::message(err.to_string()))?;
        provision_from_authorization_code(
            &self.client,
            &self.options,
            &self.redirect_uri,
            &self.code_verifier,
            &code,
        )
        .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedApiKey {
    pub sensitive_id: String,
    pub organization_id: String,
    pub organization_title: Option<String>,
    pub default_project_id: String,
    pub default_project_title: Option<String>,
    pub project_api_key: String,
    pub access_token: String,
}

pub fn start_api_provisioning(
    options: ApiProvisionOptions,
) -> Result<PendingApiProvisioning, HelperError> {
    validate_api_provision_options(&options)?;
    let client = build_http_client()?;
    let callback_server = start_authorization_code_server(
        options.callback_port,
        DEFAULT_CALLBACK_PATH,
        /*force_state*/ None,
        |redirect_uri, pkce, state| {
            build_authorize_url(&options, redirect_uri, pkce, state)
                .map_err(|err| std::io::Error::other(err.to_string()))
        },
    )
    .map_err(|err| HelperError::message(err.to_string()))?;
    let redirect_uri = callback_server.redirect_uri.clone();

    Ok(PendingApiProvisioning {
        client,
        options,
        redirect_uri,
        code_verifier: callback_server.code_verifier().to_string(),
        callback_server,
    })
}

fn validate_api_provision_options(options: &ApiProvisionOptions) -> Result<(), HelperError> {
    if options.project_poll_interval_seconds == 0 {
        return Err(HelperError::message(
            "project_poll_interval_seconds must be greater than 0.".to_string(),
        ));
    }
    if options.project_poll_timeout_seconds == 0 {
        return Err(HelperError::message(
            "project_poll_timeout_seconds must be greater than 0.".to_string(),
        ));
    }
    if options.api_key_name.trim().is_empty() {
        return Err(HelperError::message(
            "api_key_name must not be empty.".to_string(),
        ));
    }
    Ok(())
}

fn build_authorize_url(
    options: &ApiProvisionOptions,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> Result<String, HelperError> {
    let mut url = Url::parse(&format!(
        "{}/oauth/authorize",
        options.issuer.trim_end_matches('/')
    ))
    .map_err(|err| HelperError::message(format!("invalid issuer URL: {err}")))?;
    url.query_pairs_mut()
        .append_pair("audience", &options.audience)
        .append_pair("client_id", &options.client_id)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &options.scope)
        .append_pair("state", state);
    Ok(url.to_string())
}

fn build_http_client() -> Result<Client, HelperError> {
    build_reqwest_client_with_custom_ca(
        reqwest::Client::builder().timeout(Duration::from_secs(HTTP_TIMEOUT_SECONDS)),
    )
    .map_err(|err| HelperError::message(format!("failed to build HTTP client: {err}")))
}

async fn provision_from_authorization_code(
    client: &Client,
    options: &ApiProvisionOptions,
    redirect_uri: &str,
    code_verifier: &str,
    code: &str,
) -> Result<ProvisionedApiKey, HelperError> {
    let tokens = exchange_authorization_code_for_tokens(
        client,
        &options.issuer,
        &options.client_id,
        redirect_uri,
        code_verifier,
        code,
    )
    .await?;
    let login = onboarding_login(
        client,
        &options.api_base,
        &options.app,
        &tokens.access_token,
    )
    .await?;
    let target = wait_for_default_project(
        client,
        &options.api_base,
        &login.user.session.sensitive_id,
        options.project_poll_interval_seconds,
        options.project_poll_timeout_seconds,
    )
    .await?;
    let api_key = create_project_api_key(
        client,
        &options.api_base,
        &login.user.session.sensitive_id,
        &target,
        &options.api_key_name,
    )
    .await?
    .key
    .sensitive_id;

    Ok(ProvisionedApiKey {
        sensitive_id: login.user.session.sensitive_id,
        organization_id: target.organization_id,
        organization_title: target.organization_title,
        default_project_id: target.project_id,
        default_project_title: target.project_title,
        project_api_key: api_key,
        access_token: tokens.access_token,
    })
}

async fn exchange_authorization_code_for_tokens(
    client: &Client,
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    code_verifier: &str,
    code: &str,
) -> Result<OAuthTokens, HelperError> {
    let url = format!("{}/oauth/token", issuer.trim_end_matches('/'));
    execute_json(
        client
            .request(Method::POST, &url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT)
            .json(&serde_json::json!({
                "client_id": client_id,
                "code_verifier": code_verifier,
                "code": code,
                "grant_type": "authorization_code",
                "redirect_uri": redirect_uri,
            })),
        "POST",
        &url,
    )
    .await
}

async fn onboarding_login(
    client: &Client,
    api_base: &str,
    app: &str,
    access_token: &str,
) -> Result<OnboardingLoginResponse, HelperError> {
    let url = format!(
        "{}/dashboard/onboarding/login",
        api_base.trim_end_matches('/')
    );
    execute_json(
        client
            .request(Method::POST, &url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT)
            .bearer_auth(access_token)
            .json(&serde_json::json!({ "app": app })),
        "POST",
        &url,
    )
    .await
}

async fn list_organizations(
    client: &Client,
    api_base: &str,
    session_key: &str,
) -> Result<Vec<Organization>, HelperError> {
    let url = format!("{}/v1/organizations", api_base.trim_end_matches('/'));
    let response: DataList<Organization> = execute_json(
        client
            .request(Method::GET, &url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT)
            .bearer_auth(session_key),
        "GET",
        &url,
    )
    .await?;
    Ok(response.data)
}

async fn list_projects(
    client: &Client,
    api_base: &str,
    session_key: &str,
    organization_id: &str,
) -> Result<Vec<Project>, HelperError> {
    let url = format!(
        "{}/dashboard/organizations/{}/projects?detail=basic&limit=100",
        api_base.trim_end_matches('/'),
        urlencoding::encode(organization_id)
    );
    let response: DataList<Project> = execute_json(
        client
            .request(Method::GET, &url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT)
            .header("openai-organization", organization_id)
            .bearer_auth(session_key),
        "GET",
        &url,
    )
    .await?;
    Ok(response.data)
}

async fn wait_for_default_project(
    client: &Client,
    api_base: &str,
    session_key: &str,
    poll_interval_seconds: u64,
    timeout_seconds: u64,
) -> Result<ProvisioningTarget, HelperError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_seconds);
    loop {
        let organizations = list_organizations(client, api_base, session_key).await?;
        let last_state = if let Some(organization) = select_active_organization(&organizations) {
            let projects = list_projects(client, api_base, session_key, &organization.id).await?;
            if let Some(project) = find_default_project(&projects) {
                return Ok(ProvisioningTarget {
                    organization_id: organization.id.clone(),
                    organization_title: organization.title.clone(),
                    project_id: project.id.clone(),
                    project_title: project.title.clone(),
                });
            }
            format!(
                "organization `{}` exists, but no default project is ready yet (saw {} projects).",
                organization.id,
                projects.len()
            )
        } else {
            "no organization found".to_string()
        };

        if std::time::Instant::now() >= deadline {
            return Err(HelperError::message(format!(
                "Timed out waiting for an organization and default project. Last observed state: {last_state}"
            )));
        }
        let remaining_seconds = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_secs();
        let sleep_seconds = poll_interval_seconds.min(remaining_seconds.max(1));
        std::thread::sleep(Duration::from_secs(sleep_seconds));
    }
}

fn select_active_organization(organizations: &[Organization]) -> Option<&Organization> {
    organizations
        .iter()
        .find(|organization| organization.is_default)
        .or_else(|| {
            organizations
                .iter()
                .find(|organization| organization.personal)
        })
        .or_else(|| organizations.first())
}

fn find_default_project(projects: &[Project]) -> Option<&Project> {
    projects.iter().find(|project| project.is_initial)
}

async fn create_project_api_key(
    client: &Client,
    api_base: &str,
    session_key: &str,
    target: &ProvisioningTarget,
    key_name: &str,
) -> Result<CreateApiKeyResponse, HelperError> {
    let url = format!(
        "{}/dashboard/organizations/{}/projects/{}/api_keys",
        api_base.trim_end_matches('/'),
        urlencoding::encode(&target.organization_id),
        urlencoding::encode(&target.project_id)
    );
    execute_json(
        client
            .request(Method::POST, &url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT)
            .bearer_auth(session_key)
            .json(&serde_json::json!({
                "action": "create",
                "name": key_name,
            })),
        "POST",
        &url,
    )
    .await
}

async fn execute_json<T>(
    request: reqwest::RequestBuilder,
    method: &str,
    url: &str,
) -> Result<T, HelperError>
where
    T: for<'de> Deserialize<'de>,
{
    let response = request
        .send()
        .await
        .map_err(|err| HelperError::message(format!("Network error calling {url}: {err}")))?;
    let status = response.status();
    let body = response.bytes().await.map_err(|err| {
        HelperError::message(format!("Failed reading response from {url}: {err}"))
    })?;
    if !status.is_success() {
        return Err(HelperError::api(
            format!("{method} {url} failed with HTTP {status}"),
            String::from_utf8_lossy(&body).into_owned(),
        ));
    }
    serde_json::from_slice(&body)
        .map_err(|err| HelperError::message(format!("{url} returned invalid JSON: {err}")))
}

#[derive(Debug, Deserialize)]
struct OAuthTokens {
    #[serde(rename = "id_token")]
    _id_token: String,
    access_token: String,
    #[serde(rename = "refresh_token")]
    _refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct OnboardingLoginResponse {
    user: OnboardingUser,
}

#[derive(Debug, Deserialize)]
struct OnboardingUser {
    session: OnboardingSession,
}

#[derive(Debug, Deserialize)]
struct OnboardingSession {
    sensitive_id: String,
}

#[derive(Debug, Deserialize)]
struct DataList<T> {
    data: Vec<T>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct Organization {
    id: String,
    title: Option<String>,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    personal: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct Project {
    id: String,
    title: Option<String>,
    #[serde(default)]
    is_initial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProvisioningTarget {
    organization_id: String,
    organization_title: Option<String>,
    project_id: String,
    project_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateApiKeyResponse {
    key: CreatedApiKey,
}

#[derive(Debug, Deserialize)]
struct CreatedApiKey {
    sensitive_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperError {
    message: String,
    body: Option<String>,
}

impl HelperError {
    fn message(message: String) -> Self {
        Self {
            message,
            body: None,
        }
    }

    fn api(message: String, body: String) -> Self {
        Self {
            message,
            body: Some(body),
        }
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HelperError {}

#[cfg(test)]
#[path = "api_provision_tests.rs"]
mod tests;
