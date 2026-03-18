use codex_app_server_protocol::JSONRPCErrorError;

use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;

pub(crate) struct ExecServerHandler {
    initialize_requested: bool,
    initialized: bool,
}

impl ExecServerHandler {
    pub(crate) fn new() -> Self {
        Self {
            initialize_requested: false,
            initialized: false,
        }
    }

    pub(crate) async fn shutdown(&self) {}

    pub(crate) fn initialize(&mut self) -> Result<InitializeResponse, JSONRPCErrorError> {
        if self.initialize_requested {
            return Err(crate::rpc::invalid_request(
                "initialize may only be sent once per connection".to_string(),
            ));
        }
        self.initialize_requested = true;
        Ok(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
        })
    }

    pub(crate) fn initialized(&mut self) -> Result<(), String> {
        if !self.initialize_requested {
            return Err("received `initialized` notification before `initialize`".into());
        }
        self.initialized = true;
        Ok(())
    }
}
