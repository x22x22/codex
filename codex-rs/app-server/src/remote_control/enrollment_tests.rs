use super::super::RemoteControlConnectionAuth;
use super::super::RemoteControlEnrollment;
use super::super::RemoteControlTarget;
use super::super::load_remote_control_auth;
use super::super::normalize_remote_control_url;
use super::super::test_support::accept_http_request;
use super::super::test_support::respond_with_json;
use super::*;
use codex_core::CodexAuth;
use codex_core::test_support::auth_manager_from_auth;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpListener;

#[tokio::test]
async fn validate_remote_control_auth_rejects_api_key_auth() {
    let auth_manager = auth_manager_from_auth(CodexAuth::from_api_key("sk-test"));

    let err = load_remote_control_auth(auth_manager.as_ref())
        .await
        .expect_err("API key auth should be rejected");

    assert_eq!(
        err.to_string(),
        "remote control requires ChatGPT authentication; API key auth is not supported"
    );
}

#[test]
fn normalize_remote_control_url_handles_supported_and_unsupported_inputs() {
    assert_eq!(
        normalize_remote_control_url("http://example.com/backend-api/wham")
            .expect("valid http prefix"),
        RemoteControlTarget {
            websocket_url: "ws://example.com/backend-api/wham/remote/control/server".to_string(),
            enroll_url: "http://example.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("https://example.com/backend-api/wham/remote/control/server")
            .expect("valid https full path"),
        RemoteControlTarget {
            websocket_url: "wss://example.com/backend-api/wham/remote/control/server".to_string(),
            enroll_url: "https://example.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("http://example.com/legacy/server")
            .expect("valid legacy http url"),
        RemoteControlTarget {
            websocket_url: "ws://example.com/legacy/server".to_string(),
            enroll_url: "http://example.com/legacy/server/enroll".to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("https://chatgpt.com/backend-api")
            .expect("chatgpt backend-api base should target the public wham path"),
        RemoteControlTarget {
            websocket_url: "wss://chatgpt.com/backend-api/wham/remote/control/server".to_string(),
            enroll_url: "https://chatgpt.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("https://chat.openai.com")
            .expect("chat.openai.com root should normalize"),
        RemoteControlTarget {
            websocket_url: "wss://chat.openai.com/backend-api/wham/remote/control/server"
                .to_string(),
            enroll_url: "https://chat.openai.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("https://chatgpt.com/api/codex/remote/control/server")
            .expect("internal chatgpt remote-control path should rewrite to the public wham path"),
        RemoteControlTarget {
            websocket_url: "wss://chatgpt.com/backend-api/wham/remote/control/server".to_string(),
            enroll_url: "https://chatgpt.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("https://chatgpt.com/api/codex")
            .expect("explicit chatgpt api/codex base should rewrite to the public wham path"),
        RemoteControlTarget {
            websocket_url: "wss://chatgpt.com/backend-api/wham/remote/control/server".to_string(),
            enroll_url: "https://chatgpt.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
        }
    );
    let err = normalize_remote_control_url("ftp://example.com/control")
        .expect_err("unsupported scheme should fail");
    assert_eq!(
        err.to_string(),
        "invalid remote control URL `ftp://example.com/control`; expected http:// or https://"
    );

    let err =
        normalize_remote_control_url("ws://example.com/control").expect_err("ws url should fail");
    assert_eq!(
        err.to_string(),
        "invalid remote control URL `ws://example.com/control`; expected http:// or https://"
    );
}

#[tokio::test]
async fn persisted_remote_control_enrollment_is_scoped_and_selectively_cleared() {
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_path = remote_control_state_path(codex_home.path());
    let first_target = normalize_remote_control_url("http://example.com/remote/control")
        .expect("first target should parse");
    let second_target = normalize_remote_control_url("http://example.com/other/control")
        .expect("second target should parse");
    let first_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_first".to_string(),
        server_name: REMOTE_CONTROL_SERVER_NAME.to_string(),
    };
    let same_target_other_account_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_first_account_b".to_string(),
        server_name: REMOTE_CONTROL_SERVER_NAME.to_string(),
    };
    let second_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_second".to_string(),
        server_name: REMOTE_CONTROL_SERVER_NAME.to_string(),
    };

    update_persisted_remote_control_enrollment(
        state_path.as_path(),
        &first_target,
        Some("account-a"),
        Some(&first_enrollment),
    )
    .await
    .expect("first enrollment should persist");
    update_persisted_remote_control_enrollment(
        state_path.as_path(),
        &second_target,
        Some("account-a"),
        Some(&second_enrollment),
    )
    .await
    .expect("second enrollment should persist");

    update_persisted_remote_control_enrollment(
        state_path.as_path(),
        &first_target,
        Some("account-b"),
        Some(&same_target_other_account_enrollment),
    )
    .await
    .expect("other-account enrollment should persist");

    assert_eq!(
        load_persisted_remote_control_enrollment(
            state_path.as_path(),
            &first_target,
            Some("account-a")
        )
        .await,
        Some(first_enrollment.clone())
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            state_path.as_path(),
            &first_target,
            Some("account-b")
        )
        .await,
        Some(same_target_other_account_enrollment.clone())
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            state_path.as_path(),
            &second_target,
            Some("account-a")
        )
        .await,
        Some(second_enrollment.clone())
    );

    update_persisted_remote_control_enrollment(
        state_path.as_path(),
        &first_target,
        Some("account-a"),
        None,
    )
    .await
    .expect("matching enrollment should clear");

    assert_eq!(
        load_persisted_remote_control_enrollment(
            state_path.as_path(),
            &first_target,
            Some("account-b")
        )
        .await,
        Some(same_target_other_account_enrollment)
    );
    assert_eq!(
        load_persisted_remote_control_enrollment(
            state_path.as_path(),
            &second_target,
            Some("account-a")
        )
        .await,
        Some(second_enrollment)
    );
}

#[tokio::test]
async fn enrollment_manager_cache_is_scoped_to_the_current_account() {
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_path = remote_control_state_path(codex_home.path());
    let remote_control_target = normalize_remote_control_url("http://example.com/remote/control")
        .expect("target should parse");
    let account_a_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_account_a".to_string(),
        server_name: REMOTE_CONTROL_SERVER_NAME.to_string(),
    };
    let account_b_enrollment = RemoteControlEnrollment {
        server_id: "srv_e_account_b".to_string(),
        server_name: REMOTE_CONTROL_SERVER_NAME.to_string(),
    };

    update_persisted_remote_control_enrollment(
        state_path.as_path(),
        &remote_control_target,
        Some("account-a"),
        Some(&account_a_enrollment),
    )
    .await
    .expect("account-a enrollment should persist");
    update_persisted_remote_control_enrollment(
        state_path.as_path(),
        &remote_control_target,
        Some("account-b"),
        Some(&account_b_enrollment),
    )
    .await
    .expect("account-b enrollment should persist");

    let mut enrollment_manager =
        EnrollmentManager::new(remote_control_target, codex_home.path().to_path_buf());

    assert_eq!(
        enrollment_manager
            .enroll(&RemoteControlConnectionAuth {
                bearer_token: "Access Token".to_string(),
                account_id: Some("account-a".to_string()),
            })
            .await
            .expect("account-a enrollment should load from the cache or state file"),
        account_a_enrollment
    );
    assert_eq!(
        enrollment_manager
            .enroll(&RemoteControlConnectionAuth {
                bearer_token: "Access Token".to_string(),
                account_id: Some("account-b".to_string()),
            })
            .await
            .expect("account-b enrollment should replace the cached account-a enrollment"),
        account_b_enrollment
    );
}

#[tokio::test]
async fn enroll_remote_control_server_parse_failure_includes_response_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = format!(
        "http://{}/api/codex",
        listener
            .local_addr()
            .expect("listener should have a local addr")
    );
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should parse");
    let enroll_url = remote_control_target.enroll_url.clone();
    let response_body = json!({
        "error": "not enrolled",
    });
    let expected_body = response_body.to_string();
    let server_task = tokio::spawn(async move {
        let enroll_request = accept_http_request(&listener).await;
        respond_with_json(enroll_request.stream, response_body).await;
    });

    let err = enroll_remote_control_server(
        &remote_control_target,
        &RemoteControlConnectionAuth {
            bearer_token: "Access Token".to_string(),
            account_id: Some("account_id".to_string()),
        },
    )
    .await
    .expect_err("invalid response should fail to parse");

    server_task.await.expect("server task should succeed");
    assert_eq!(
        err.to_string(),
        format!(
            "failed to parse remote control enrollment response from `{enroll_url}`: HTTP 200 OK, body: {expected_body}, decode error: missing field `server_id` at line 1 column {}",
            expected_body.len()
        )
    );
}
