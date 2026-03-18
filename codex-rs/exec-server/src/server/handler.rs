use crate::protocol::InitializeParams;
use crate::protocol::InitializeResponse;
use crate::server::invalid_request;
use crate::server::unauthorized;

pub(crate) struct ExecServerHandler {
    required_auth_token: Option<String>,
    initialize_requested: bool,
    initialized: bool,
}

impl ExecServerHandler {
    pub(crate) fn new(required_auth_token: Option<String>) -> Self {
        Self {
            required_auth_token,
            initialize_requested: false,
            initialized: false,
        }
    }

    pub(crate) fn initialize(
        &mut self,
        params: InitializeParams,
    ) -> Result<InitializeResponse, codex_app_server_protocol::JSONRPCErrorError> {
        if self.initialize_requested {
            return Err(invalid_request(
                "initialize may only be sent once per connection".to_string(),
            ));
        }
        if let Some(required_auth_token) = &self.required_auth_token
            && params.auth_token.as_deref() != Some(required_auth_token.as_str())
        {
            return Err(unauthorized("invalid exec-server auth token".to_string()));
        }
        self.initialize_requested = true;
        Ok(InitializeResponse {})
    }

    pub(crate) fn initialized(&mut self) -> Result<(), String> {
        if !self.initialize_requested {
            return Err("received `initialized` notification before `initialize`".to_string());
        }
        self.initialized = true;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn is_ready(&self) -> bool {
        self.initialize_requested && self.initialized
    }
}
