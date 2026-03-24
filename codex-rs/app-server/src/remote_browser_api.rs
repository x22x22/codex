use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use codex_app_server_protocol::BrowserSessionArtifacts;
use codex_app_server_protocol::BrowserSessionCommandParams;
use codex_app_server_protocol::BrowserSessionCommandResponse;
use codex_app_server_protocol::BrowserSessionState;
use codex_app_server_protocol::BrowserTabState;
use codex_app_server_protocol::JSONRPCErrorError;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

#[derive(Clone)]
pub(crate) struct RemoteBrowserApi {
    endpoint: Option<String>,
    client: reqwest::Client,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBrowserTabState {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) url: Option<String>,
    #[serde(default)]
    pub(crate) selected: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBrowserState {
    #[serde(default)]
    pub(crate) selected_tab_id: Option<String>,
    #[serde(default)]
    pub(crate) tabs: Vec<RemoteBrowserTabState>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBrowserArtifacts {
    #[serde(default)]
    screenshot_base64: Option<String>,
    #[serde(default)]
    replay_gif_base64: Option<String>,
    #[serde(default)]
    replay_frame_count: Option<u32>,
    #[serde(default)]
    replay_frame_duration_ms: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBrowserCommandOutcome {
    pub(crate) browser_session_id: String,
    pub(crate) result: JsonValue,
    pub(crate) browser_state: RemoteBrowserState,
    #[serde(default)]
    artifacts: RemoteBrowserArtifacts,
}

impl RemoteBrowserState {
    fn selected_tab_id(&self) -> Option<String> {
        self.selected_tab_id.clone().or_else(|| {
            self.tabs
                .iter()
                .find(|tab| tab.selected)
                .map(|tab| tab.id.clone())
        })
    }

    pub(crate) fn to_public_state(&self) -> Option<BrowserSessionState> {
        let selected_tab_id = self.selected_tab_id()?;
        Some(BrowserSessionState {
            selected_tab_id,
            tabs: self
                .tabs
                .iter()
                .map(|tab| BrowserTabState {
                    id: tab.id.clone(),
                    title: tab.title.clone().unwrap_or_default(),
                    url: tab.url.clone().unwrap_or_default(),
                    selected: tab.selected,
                })
                .collect(),
        })
    }
}

impl RemoteBrowserCommandOutcome {
    pub(crate) fn to_public_response(
        &self,
    ) -> Result<BrowserSessionCommandResponse, JSONRPCErrorError> {
        let browser_state =
            self.browser_state
                .to_public_state()
                .ok_or_else(|| JSONRPCErrorError {
                    code: INTERNAL_ERROR_CODE,
                    message: "remote browser response did not include a selected tab".to_string(),
                    data: None,
                })?;

        Ok(BrowserSessionCommandResponse {
            browser_session_id: self.browser_session_id.clone(),
            result: self.result.clone(),
            browser_state,
            artifacts: self.to_public_artifacts(),
        })
    }

    pub(crate) fn screenshot_data_url(&self) -> Option<String> {
        self.artifacts
            .screenshot_base64
            .as_ref()
            .map(|encoded| format!("data:image/png;base64,{encoded}"))
    }

    pub(crate) fn screenshot_base64(&self) -> Option<String> {
        self.artifacts.screenshot_base64.clone()
    }

    pub(crate) fn replay_gif_data_url(&self) -> Option<String> {
        self.artifacts
            .replay_gif_base64
            .as_ref()
            .map(|encoded| format!("data:image/gif;base64,{encoded}"))
    }

    pub(crate) fn to_public_artifacts(&self) -> Option<BrowserSessionArtifacts> {
        let screenshot_image_url = self.screenshot_data_url();
        let replay_gif_image_url = self.replay_gif_data_url();
        let replay_frame_count = self.artifacts.replay_frame_count;
        let replay_frame_duration_ms = self.artifacts.replay_frame_duration_ms;

        if screenshot_image_url.is_none()
            && replay_gif_image_url.is_none()
            && replay_frame_count.is_none()
            && replay_frame_duration_ms.is_none()
        {
            return None;
        }

        Some(BrowserSessionArtifacts {
            screenshot_image_url,
            replay_gif_image_url,
            replay_frame_count,
            replay_frame_duration_ms,
        })
    }

    pub(crate) fn browser_state_json(&self) -> JsonValue {
        serde_json::to_value(&self.browser_state).unwrap_or(JsonValue::Null)
    }
}

impl RemoteBrowserApi {
    pub(crate) fn new(endpoint: Option<String>) -> Self {
        Self {
            endpoint,
            client: reqwest::Client::new(),
        }
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.endpoint.is_some()
    }

    pub(crate) async fn command(
        &self,
        params: BrowserSessionCommandParams,
    ) -> Result<BrowserSessionCommandResponse, JSONRPCErrorError> {
        self.command_with_artifacts(params)
            .await?
            .to_public_response()
    }

    pub(crate) async fn command_with_artifacts(
        &self,
        params: BrowserSessionCommandParams,
    ) -> Result<RemoteBrowserCommandOutcome, JSONRPCErrorError> {
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
