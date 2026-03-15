use codex_app_server_client::AppServerClient;
use codex_app_server_protocol::Account;
use codex_app_server_protocol::CancelLoginAccountParams;
use codex_app_server_protocol::CancelLoginAccountResponse;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::LoginAccountParams;
use codex_app_server_protocol::LoginAccountResponse;
use codex_app_server_protocol::RequestId;
use codex_core::auth::AuthMode;
use serde::de::DeserializeOwned;
use tracing::warn;

use crate::LoginStatus;

#[derive(Debug, PartialEq)]
pub(crate) enum AuthCommand {
    StartApiKey { api_key: String },
    StartChatgpt,
    CancelChatgpt { login_id: String },
    DeviceCodeFailed { message: String },
}

#[derive(Default)]
pub(crate) struct OnboardingAccountApi {
    next_request_id: i64,
}

impl OnboardingAccountApi {
    pub(crate) async fn read_account(
        &mut self,
        app_server: &AppServerClient,
    ) -> Result<GetAccountResponse, String> {
        send_request_with_response(
            app_server,
            ClientRequest::GetAccount {
                request_id: self.next_request_id(),
                params: GetAccountParams {
                    refresh_token: false,
                },
            },
            "account/read",
        )
        .await
    }

    pub(crate) async fn start_api_key_login(
        &mut self,
        app_server: &AppServerClient,
        api_key: String,
    ) -> Result<LoginAccountResponse, String> {
        send_request_with_response(
            app_server,
            ClientRequest::LoginAccount {
                request_id: self.next_request_id(),
                params: LoginAccountParams::ApiKey { api_key },
            },
            "account/login/start",
        )
        .await
    }

    pub(crate) async fn start_chatgpt_login(
        &mut self,
        app_server: &AppServerClient,
    ) -> Result<LoginAccountResponse, String> {
        send_request_with_response(
            app_server,
            ClientRequest::LoginAccount {
                request_id: self.next_request_id(),
                params: LoginAccountParams::Chatgpt,
            },
            "account/login/start",
        )
        .await
    }

    pub(crate) async fn cancel_chatgpt_login(
        &mut self,
        app_server: &AppServerClient,
        login_id: String,
    ) -> Result<CancelLoginAccountResponse, String> {
        send_request_with_response(
            app_server,
            ClientRequest::CancelLoginAccount {
                request_id: self.next_request_id(),
                params: CancelLoginAccountParams { login_id },
            },
            "account/login/cancel",
        )
        .await
    }

    fn next_request_id(&mut self) -> RequestId {
        self.next_request_id += 1;
        RequestId::Integer(self.next_request_id)
    }
}

pub(crate) fn login_status_from_account(account: Option<&Account>) -> LoginStatus {
    match account {
        Some(Account::ApiKey {}) => LoginStatus::AuthMode(AuthMode::ApiKey),
        Some(Account::Chatgpt { .. }) => LoginStatus::AuthMode(AuthMode::Chatgpt),
        None => LoginStatus::NotAuthenticated,
    }
}

fn login_status_from_account_read_result(
    result: Result<GetAccountResponse, String>,
) -> LoginStatus {
    match result {
        Ok(response) => login_status_from_account(response.account.as_ref()),
        Err(err) => {
            warn!(
                "account/read failed during onboarding startup; continuing unauthenticated: {err}"
            );
            LoginStatus::NotAuthenticated
        }
    }
}

pub(crate) async fn read_login_status_via_app_server(app_server: &AppServerClient) -> LoginStatus {
    let mut api = OnboardingAccountApi::default();
    login_status_from_account_read_result(api.read_account(app_server).await)
}

async fn send_request_with_response<T>(
    app_server: &AppServerClient,
    request: ClientRequest,
    method: &str,
) -> Result<T, String>
where
    T: DeserializeOwned,
{
    app_server
        .request_typed(request)
        .await
        .map_err(|err| format!("{method} failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn read_login_status_falls_back_to_unauthenticated_on_rpc_error() {
        let status = login_status_from_account_read_result(Err("boom".to_string()));

        assert_eq!(status, LoginStatus::NotAuthenticated);
    }
}
