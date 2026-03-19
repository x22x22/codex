use crate::CodexAuth;
use crate::config::Config;
use crate::default_client::create_client;
use crate::error::CodexErr;
use crate::error::Result as CodexResult;
use codex_api::RealtimeSessionConfig;
use codex_api::realtime_client_secret_request_body;
use serde::Deserialize;

#[derive(Deserialize)]
struct RealtimeClientSecretResponse {
    value: String,
}

pub(crate) async fn fetch_realtime_client_secret(
    auth: &CodexAuth,
    config: &Config,
    session_config: &RealtimeSessionConfig,
) -> CodexResult<String> {
    let bearer_token = auth.get_token().map_err(|err| {
        CodexErr::InvalidRequest(format!("failed to read ChatGPT auth token: {err}"))
    })?;
    let body = realtime_client_secret_request_body(session_config)
        .map_err(|err| CodexErr::InvalidRequest(err.to_string()))?;
    let endpoint = format!(
        "{}/codex/realtime/client_secrets",
        normalized_chatgpt_base_url(&config.chatgpt_base_url)
    );

    let mut request = create_client().post(&endpoint).bearer_auth(bearer_token);
    if let Some(account_id) = auth.get_account_id() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request.json(&body).send().await.map_err(|err| {
        CodexErr::InvalidRequest(format!("failed to request realtime client secret: {err}"))
    })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CodexErr::InvalidRequest(format!(
            "failed to request realtime client secret: {status} {body}"
        )));
    }

    let payload: RealtimeClientSecretResponse = response.json().await.map_err(|err| {
        CodexErr::InvalidRequest(format!(
            "failed to parse realtime client secret response: {err}"
        ))
    })?;
    if payload.value.trim().is_empty() {
        return Err(CodexErr::InvalidRequest(
            "realtime client secret response was missing a value".to_string(),
        ));
    }
    Ok(payload.value)
}

fn normalized_chatgpt_base_url(input: &str) -> String {
    let mut base_url = input.trim_end_matches('/').to_string();
    if (base_url.starts_with("https://chatgpt.com")
        || base_url.starts_with("https://chat.openai.com"))
        && !base_url.contains("/backend-api")
    {
        base_url = format!("{base_url}/backend-api");
    }
    base_url
}
