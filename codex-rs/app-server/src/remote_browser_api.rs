use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use codex_app_server_protocol::BrowserSessionCommandParams;
use codex_app_server_protocol::BrowserSessionCommandResponse;
use codex_app_server_protocol::JSONRPCErrorError;

#[derive(Clone)]
pub(crate) struct RemoteBrowserApi {
    endpoint: Option<String>,
    client: reqwest::Client,
}

impl RemoteBrowserApi {
    pub(crate) fn new(endpoint: Option<String>) -> Self {
        Self {
            endpoint,
            client: reqwest::Client::new(),
        }
    }

    pub(crate) async fn command(
        &self,
        params: BrowserSessionCommandParams,
    ) -> Result<BrowserSessionCommandResponse, JSONRPCErrorError> {
        let Some(endpoint) = &self.endpoint else {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: "Remote browser endpoint is not configured".to_string(),
                data: None,
            });
        };

        let response = self
            .client
            .post(endpoint)
            .json(&params)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        let body = response.text().await.map_err(map_reqwest_error)?;
        if !status.is_success() {
            return Err(JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: format!("remote browser endpoint returned HTTP {status}: {body}"),
                data: None,
            });
        }

        serde_json::from_str(&body).map_err(|err| JSONRPCErrorError {
            code: INTERNAL_ERROR_CODE,
            message: format!("failed to decode remote browser response: {err}; body={body}"),
            data: None,
        })
    }
}

fn map_reqwest_error(err: reqwest::Error) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message: format!("remote browser request failed: {err}"),
        data: None,
    }
}
