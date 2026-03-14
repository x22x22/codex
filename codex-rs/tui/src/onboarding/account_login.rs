use codex_app_server_client::InProcessAppServerClient;
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

use crate::LoginStatus;

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
        app_server: &InProcessAppServerClient,
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
        app_server: &InProcessAppServerClient,
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
        app_server: &InProcessAppServerClient,
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
        app_server: &InProcessAppServerClient,
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

pub(crate) async fn read_login_status_via_app_server(
    app_server: &InProcessAppServerClient,
) -> Result<LoginStatus, String> {
    let mut api = OnboardingAccountApi::default();
    let response = api.read_account(app_server).await?;
    Ok(login_status_from_account(response.account.as_ref()))
}

async fn send_request_with_response<T>(
    app_server: &InProcessAppServerClient,
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
