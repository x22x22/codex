#![cfg(not(target_os = "windows"))]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_core::CodexAuth;
use codex_core::X_OPENAI_INTERNAL_CODEX_TASK_ID_HEADER;
use codex_core::auth::AuthCredentialsStoreMode;
use codex_core::auth::AuthDotJson;
use codex_core::auth::save_auth;
use codex_core::features::Feature;
use codex_core::token_data::IdTokenInfo;
use codex_core::token_data::TokenData;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::Request;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::path_regex;

#[derive(Clone)]
struct JsonSequenceResponder {
    bodies: Arc<Mutex<VecDeque<Value>>>,
}

impl JsonSequenceResponder {
    fn new(bodies: Vec<Value>) -> Self {
        Self {
            bodies: Arc::new(Mutex::new(VecDeque::from(bodies))),
        }
    }
}

impl Respond for JsonSequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let body = self
            .bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| panic!("missing queued JSON response"));
        ResponseTemplate::new(200).set_body_json(body)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feature_off_skips_agent_identity_requests() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config({
            let base_url = server.uri();
            move |config| {
                config.chatgpt_base_url = base_url;
                config.model_provider.supports_websockets = false;
            }
        });
    let test = builder.build(&server).await?;

    test.submit_turn_with_policy("hello", workspace_write_with_network())
        .await?;

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/agent/register"),
        0
    );
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/task/register"),
        0
    );
    assert_eq!(
        response_mock
            .single_request()
            .header(X_OPENAI_INTERNAL_CODEX_TASK_ID_HEADER),
        None
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_identity_registers_once_and_reuses_task_within_thread() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_agent_identity_endpoints(&server, vec!["task-1"]).await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config({
            let base_url = server.uri();
            move |config| {
                config.chatgpt_base_url = base_url;
                config.model_provider.supports_websockets = false;
                config
                    .features
                    .enable(Feature::UseAgentIdentity)
                    .expect("test config should allow feature update");
            }
        });
    let test = builder.build(&server).await?;

    test.submit_turn_with_policy("hello", workspace_write_with_network())
        .await?;
    test.submit_turn_with_policy("again", workspace_write_with_network())
        .await?;

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/agent/register"),
        1
    );
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/task/register"),
        1
    );
    assert_eq!(response_mock.requests().len(), 2);

    let task_request = request_json_body(
        requests
            .iter()
            .find(|request| request.url.path() == "/backend-api/task/register")
            .expect("missing task register request"),
    );
    assert_eq!(task_request["agent_id"], json!("agent-runtime-1"));
    assert_eq!(
        task_request["metadata"]["thread_id"],
        json!(test.session_configured.session_id.to_string())
    );
    assert!(
        !task_request["timestamp"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        !task_request["signature"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );

    for request in response_mock.requests() {
        assert_eq!(
            request
                .header(X_OPENAI_INTERNAL_CODEX_TASK_ID_HEADER)
                .as_deref(),
            Some("task-1")
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_thread_reuses_agent_identity_but_gets_new_task() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    mount_agent_identity_endpoints(&server, vec!["task-1", "task-2"]).await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;

    let mut first_builder = test_codex()
        .with_home(Arc::clone(&home))
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config({
            let base_url = server.uri();
            move |config| {
                config.chatgpt_base_url = base_url;
                config.model_provider.supports_websockets = false;
                config
                    .features
                    .enable(Feature::UseAgentIdentity)
                    .expect("test config should allow feature update");
            }
        });
    let first = first_builder.build(&server).await?;
    first
        .submit_turn_with_policy("hello", workspace_write_with_network())
        .await?;

    let mut second_builder = test_codex()
        .with_home(home)
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config({
            let base_url = server.uri();
            move |config| {
                config.chatgpt_base_url = base_url;
                config.model_provider.supports_websockets = false;
                config
                    .features
                    .enable(Feature::UseAgentIdentity)
                    .expect("test config should allow feature update");
            }
        });
    let second = second_builder.build(&server).await?;
    second
        .submit_turn_with_policy("hello from new thread", workspace_write_with_network())
        .await?;

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/agent/register"),
        1
    );
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/task/register"),
        2
    );

    let response_requests = response_mock.requests();
    assert_eq!(response_requests.len(), 2);
    assert_eq!(
        response_requests[0]
            .header(X_OPENAI_INTERNAL_CODEX_TASK_ID_HEADER)
            .as_deref(),
        Some("task-1")
    );
    assert_eq!(
        response_requests[1]
            .header(X_OPENAI_INTERNAL_CODEX_TASK_ID_HEADER)
            .as_deref(),
        Some("task-2")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_account_change_registers_a_new_agent_identity() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    mount_agent_identity_endpoints(&server, vec!["task-1", "task-2"]).await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(vec![
                    ev_response_created("resp-1"),
                    ev_completed("resp-1"),
                ])),
        )
        .mount(&server)
        .await;

    let mut first_builder = test_codex()
        .with_home(Arc::clone(&home))
        .with_auth(chatgpt_auth_for_account(home.path(), "account-1")?)
        .with_config({
            let base_url = server.uri();
            move |config| {
                config.chatgpt_base_url = base_url;
                config.model_provider.supports_websockets = false;
                config
                    .features
                    .enable(Feature::UseAgentIdentity)
                    .expect("test config should allow feature update");
            }
        });
    let first = first_builder.build(&server).await?;
    first
        .submit_turn_with_policy("hello", workspace_write_with_network())
        .await?;

    let mut second_builder = test_codex()
        .with_home(home)
        .with_auth(chatgpt_auth_for_account(first.home.path(), "account-2")?)
        .with_config({
            let base_url = server.uri();
            move |config| {
                config.chatgpt_base_url = base_url;
                config.model_provider.supports_websockets = false;
                config
                    .features
                    .enable(Feature::UseAgentIdentity)
                    .expect("test config should allow feature update");
            }
        });
    let second = second_builder.build(&server).await?;
    second
        .submit_turn_with_policy("hello from second account", workspace_write_with_network())
        .await?;

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/agent/register"),
        2
    );
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/task/register"),
        2
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_key_auth_bypasses_agent_identity_flow() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_codex().with_config({
        let base_url = server.uri();
        move |config| {
            config.chatgpt_base_url = base_url;
            config.model_provider.supports_websockets = false;
            config
                .features
                .enable(Feature::UseAgentIdentity)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&server).await?;

    test.submit_turn_with_policy("hello", workspace_write_with_network())
        .await?;

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/agent/register"),
        0
    );
    assert_eq!(
        count_requests_for_path(&requests, "/backend-api/task/register"),
        0
    );
    assert_eq!(
        response_mock
            .single_request()
            .header(X_OPENAI_INTERNAL_CODEX_TASK_ID_HEADER),
        None
    );

    Ok(())
}

async fn mount_agent_identity_endpoints(server: &wiremock::MockServer, task_ids: Vec<&str>) {
    Mock::given(method("POST"))
        .and(path("/backend-api/agent/register"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "agent_runtime_id": "agent-runtime-1",
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/backend-api/task/register"))
        .respond_with(JsonSequenceResponder::new(
            task_ids
                .into_iter()
                .map(|task_id| json!({ "task_id": task_id }))
                .collect(),
        ))
        .mount(server)
        .await;
}

fn count_requests_for_path(requests: &[wiremock::Request], expected_path: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.url.path() == expected_path)
        .count()
}

fn request_json_body(request: &wiremock::Request) -> Value {
    match serde_json::from_slice(&request.body) {
        Ok(body) => body,
        Err(err) => panic!("request body should be valid JSON: {err}"),
    }
}

fn workspace_write_with_network() -> codex_protocol::protocol::SandboxPolicy {
    codex_protocol::protocol::SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![],
        read_only_access: Default::default(),
        network_access: true,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    }
}

fn chatgpt_auth_for_account(codex_home: &std::path::Path, account_id: &str) -> Result<CodexAuth> {
    let mut id_token = IdTokenInfo::default();
    id_token.raw_jwt = fake_chatgpt_id_token(account_id)?;
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(AuthMode::Chatgpt),
        openai_api_key: None,
        tokens: Some(TokenData {
            id_token,
            access_token: format!("access-token-{account_id}"),
            refresh_token: "refresh-token".to_string(),
            account_id: Some(account_id.to_string()),
        }),
        last_refresh: Some(Utc::now()),
    };
    save_auth(codex_home, &auth_dot_json, AuthCredentialsStoreMode::File)?;
    let Some(auth) = CodexAuth::from_auth_storage(codex_home, AuthCredentialsStoreMode::File)?
    else {
        anyhow::bail!("auth should load from storage");
    };
    Ok(auth)
}

fn fake_chatgpt_id_token(account_id: &str) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Header {
        alg: &'static str,
        typ: &'static str,
    }

    let header = Header {
        alg: "none",
        typ: "JWT",
    };
    let payload = json!({
        "email": "test@example.com",
        "https://api.openai.com/auth": {
            "chatgpt_user_id": "user-123",
            "chatgpt_account_id": account_id,
        },
    });

    let header_bytes = serde_json::to_vec(&header)?;
    let payload_bytes = serde_json::to_vec(&payload)?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_bytes);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_bytes);
    let signature_b64 = URL_SAFE_NO_PAD.encode("sig");
    Ok(format!("{header_b64}.{payload_b64}.{signature_b64}"))
}
