use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use tokio_util::sync::CancellationToken;

use crate::RuntimeResponse;
use crate::runtime::ExecuteRequest;
use crate::runtime::WaitRequest;

const ANDROID_UNAVAILABLE_MESSAGE: &str = "code mode is unavailable on Android";

#[async_trait]
pub trait CodeModeTurnHost: Send + Sync {
    async fn invoke_tool(
        &self,
        tool_name: String,
        input: Option<JsonValue>,
        cancellation_token: CancellationToken,
    ) -> Result<JsonValue, String>;

    async fn notify(&self, call_id: String, cell_id: String, text: String) -> Result<(), String>;
}

#[derive(Default)]
pub struct CodeModeService {
    stored_values: tokio::sync::Mutex<HashMap<String, JsonValue>>,
}

impl CodeModeService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn stored_values(&self) -> HashMap<String, JsonValue> {
        self.stored_values.lock().await.clone()
    }

    pub async fn replace_stored_values(&self, values: HashMap<String, JsonValue>) {
        *self.stored_values.lock().await = values;
    }

    pub async fn execute(&self, _request: ExecuteRequest) -> Result<RuntimeResponse, String> {
        Err(ANDROID_UNAVAILABLE_MESSAGE.to_string())
    }

    pub async fn wait(&self, _request: WaitRequest) -> Result<RuntimeResponse, String> {
        Err(ANDROID_UNAVAILABLE_MESSAGE.to_string())
    }

    pub fn start_turn_worker(&self, _host: Arc<dyn CodeModeTurnHost>) -> CodeModeTurnWorker {
        CodeModeTurnWorker
    }
}

pub struct CodeModeTurnWorker;
