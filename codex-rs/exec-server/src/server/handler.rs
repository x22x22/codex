use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_app_server_protocol::JSONRPCErrorError;

use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;
use crate::rpc::RpcNotificationSender;

pub(crate) struct ExecServerHandler {
    _notifications: RpcNotificationSender,
    initialize_requested: AtomicBool,
    initialized: AtomicBool,
}

impl ExecServerHandler {
    pub(crate) fn new(notifications: RpcNotificationSender) -> Self {
        Self {
            _notifications: notifications,
            initialize_requested: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
        }
    }

    pub(crate) async fn shutdown(&self) {}

    pub(crate) fn initialize(&self) -> Result<InitializeResponse, JSONRPCErrorError> {
        if self.initialize_requested.swap(true, Ordering::SeqCst) {
            return Err(crate::rpc::invalid_request(
                "initialize may only be sent once per connection".to_string(),
            ));
        }
        Ok(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
        })
    }

    pub(crate) fn initialized(&self) -> Result<(), String> {
        if !self.initialize_requested.load(Ordering::SeqCst) {
            return Err("received `initialized` notification before `initialize`".into());
        }
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }
}
