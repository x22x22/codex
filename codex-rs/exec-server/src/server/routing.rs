use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;

use crate::protocol::EXEC_EXITED_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExecServerOutboundMessage {
    Notification(ExecServerServerNotification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecServerServerNotification {
    OutputDelta(ExecOutputDeltaNotification),
    Exited(ExecExitedNotification),
}

pub(crate) fn encode_outbound_message(
    message: ExecServerOutboundMessage,
) -> Result<JSONRPCMessage, serde_json::Error> {
    match message {
        ExecServerOutboundMessage::Notification(notification) => Ok(JSONRPCMessage::Notification(
            serialize_notification(notification)?,
        )),
    }
}

pub(crate) fn invalid_request(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32600,
        data: None,
        message,
    }
}

pub(crate) fn invalid_params(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32602,
        data: None,
        message,
    }
}

pub(crate) fn internal_error(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        data: None,
        message,
    }
}

fn serialize_notification(
    notification: ExecServerServerNotification,
) -> Result<JSONRPCNotification, serde_json::Error> {
    match notification {
        ExecServerServerNotification::OutputDelta(params) => Ok(JSONRPCNotification {
            method: EXEC_OUTPUT_DELTA_METHOD.to_string(),
            params: Some(serde_json::to_value(params)?),
        }),
        ExecServerServerNotification::Exited(params) => Ok(JSONRPCNotification {
            method: EXEC_EXITED_METHOD.to_string(),
            params: Some(serde_json::to_value(params)?),
        }),
    }
}
