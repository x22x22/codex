use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use serde::de::DeserializeOwned;

use crate::protocol::EXEC_EXITED_METHOD;
use crate::protocol::EXEC_METHOD;
use crate::protocol::EXEC_OUTPUT_DELTA_METHOD;
use crate::protocol::EXEC_READ_METHOD;
use crate::protocol::EXEC_TERMINATE_METHOD;
use crate::protocol::EXEC_WRITE_METHOD;
use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecParams;
use crate::protocol::ExecResponse;
use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::protocol::InitializeResponse;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::TerminateParams;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteParams;
use crate::protocol::WriteResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecServerInboundMessage {
    Request(ExecServerRequest),
    Notification(ExecServerClientNotification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecServerRequest {
    Initialize {
        request_id: RequestId,
        params: InitializeParams,
    },
    Exec {
        request_id: RequestId,
        params: ExecParams,
    },
    Read {
        request_id: RequestId,
        params: ReadParams,
    },
    Write {
        request_id: RequestId,
        params: WriteParams,
    },
    Terminate {
        request_id: RequestId,
        params: TerminateParams,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecServerClientNotification {
    Initialized,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExecServerOutboundMessage {
    Response {
        request_id: RequestId,
        response: ExecServerResponseMessage,
    },
    Error {
        request_id: RequestId,
        error: JSONRPCErrorError,
    },
    Notification(ExecServerServerNotification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecServerResponseMessage {
    Initialize(InitializeResponse),
    Exec(ExecResponse),
    Read(ReadResponse),
    Write(WriteResponse),
    Terminate(TerminateResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecServerServerNotification {
    OutputDelta(ExecOutputDeltaNotification),
    Exited(ExecExitedNotification),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RoutedExecServerMessage {
    Inbound(ExecServerInboundMessage),
    ImmediateOutbound(ExecServerOutboundMessage),
}

pub(crate) fn route_jsonrpc_message(
    message: JSONRPCMessage,
) -> Result<RoutedExecServerMessage, String> {
    match message {
        JSONRPCMessage::Request(request) => route_request(request),
        JSONRPCMessage::Notification(notification) => route_notification(notification),
        JSONRPCMessage::Response(response) => Err(format!(
            "unexpected client response for request id {:?}",
            response.id
        )),
        JSONRPCMessage::Error(error) => Err(format!(
            "unexpected client error for request id {:?}",
            error.id
        )),
    }
}

pub(crate) fn encode_outbound_message(
    message: ExecServerOutboundMessage,
) -> Result<JSONRPCMessage, serde_json::Error> {
    match message {
        ExecServerOutboundMessage::Response {
            request_id,
            response,
        } => Ok(JSONRPCMessage::Response(JSONRPCResponse {
            id: request_id,
            result: serialize_response(response)?,
        })),
        ExecServerOutboundMessage::Error { request_id, error } => {
            Ok(JSONRPCMessage::Error(JSONRPCError {
                id: request_id,
                error,
            }))
        }
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

fn route_request(request: JSONRPCRequest) -> Result<RoutedExecServerMessage, String> {
    match request.method.as_str() {
        INITIALIZE_METHOD => Ok(parse_request_params(request, |request_id, params| {
            ExecServerRequest::Initialize { request_id, params }
        })),
        EXEC_METHOD => Ok(parse_request_params(request, |request_id, params| {
            ExecServerRequest::Exec { request_id, params }
        })),
        EXEC_READ_METHOD => Ok(parse_request_params(request, |request_id, params| {
            ExecServerRequest::Read { request_id, params }
        })),
        EXEC_WRITE_METHOD => Ok(parse_request_params(request, |request_id, params| {
            ExecServerRequest::Write { request_id, params }
        })),
        EXEC_TERMINATE_METHOD => Ok(parse_request_params(request, |request_id, params| {
            ExecServerRequest::Terminate { request_id, params }
        })),
        other => Ok(RoutedExecServerMessage::ImmediateOutbound(
            ExecServerOutboundMessage::Error {
                request_id: request.id,
                error: invalid_request(format!("unknown method: {other}")),
            },
        )),
    }
}

fn route_notification(
    notification: JSONRPCNotification,
) -> Result<RoutedExecServerMessage, String> {
    match notification.method.as_str() {
        INITIALIZED_METHOD => Ok(RoutedExecServerMessage::Inbound(
            ExecServerInboundMessage::Notification(ExecServerClientNotification::Initialized),
        )),
        other => Err(format!("unexpected notification method: {other}")),
    }
}

fn parse_request_params<P, F>(request: JSONRPCRequest, build: F) -> RoutedExecServerMessage
where
    P: DeserializeOwned,
    F: FnOnce(RequestId, P) -> ExecServerRequest,
{
    let request_id = request.id;
    match serde_json::from_value::<P>(request.params.unwrap_or(serde_json::Value::Null)) {
        Ok(params) => RoutedExecServerMessage::Inbound(ExecServerInboundMessage::Request(build(
            request_id, params,
        ))),
        Err(err) => RoutedExecServerMessage::ImmediateOutbound(ExecServerOutboundMessage::Error {
            request_id,
            error: invalid_params(err.to_string()),
        }),
    }
}

fn serialize_response(
    response: ExecServerResponseMessage,
) -> Result<serde_json::Value, serde_json::Error> {
    match response {
        ExecServerResponseMessage::Initialize(response) => serde_json::to_value(response),
        ExecServerResponseMessage::Exec(response) => serde_json::to_value(response),
        ExecServerResponseMessage::Read(response) => serde_json::to_value(response),
        ExecServerResponseMessage::Write(response) => serde_json::to_value(response),
        ExecServerResponseMessage::Terminate(response) => serde_json::to_value(response),
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::ExecServerClientNotification;
    use super::ExecServerInboundMessage;
    use super::ExecServerOutboundMessage;
    use super::ExecServerRequest;
    use super::ExecServerResponseMessage;
    use super::ExecServerServerNotification;
    use super::RoutedExecServerMessage;
    use super::encode_outbound_message;
    use super::route_jsonrpc_message;
    use crate::protocol::EXEC_EXITED_METHOD;
    use crate::protocol::EXEC_METHOD;
    use crate::protocol::ExecExitedNotification;
    use crate::protocol::ExecParams;
    use crate::protocol::ExecResponse;
    use crate::protocol::ExecSandboxConfig;
    use crate::protocol::ExecSandboxMode;
    use crate::protocol::INITIALIZE_METHOD;
    use crate::protocol::INITIALIZED_METHOD;
    use crate::protocol::InitializeParams;
    use codex_app_server_protocol::JSONRPCMessage;
    use codex_app_server_protocol::JSONRPCNotification;
    use codex_app_server_protocol::JSONRPCRequest;
    use codex_app_server_protocol::JSONRPCResponse;
    use codex_app_server_protocol::RequestId;

    #[test]
    fn routes_initialize_requests_to_typed_variants() {
        let routed = route_jsonrpc_message(JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(1),
            method: INITIALIZE_METHOD.to_string(),
            params: Some(json!({ "clientName": "test-client" })),
            trace: None,
        }))
        .expect("initialize request should route");

        assert_eq!(
            routed,
            RoutedExecServerMessage::Inbound(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test-client".to_string(),
                    },
                },
            ))
        );
    }

    #[test]
    fn malformed_exec_params_return_immediate_error_outbound() {
        let routed = route_jsonrpc_message(JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(2),
            method: EXEC_METHOD.to_string(),
            params: Some(json!({ "processId": "proc-1" })),
            trace: None,
        }))
        .expect("exec request should route");

        let RoutedExecServerMessage::ImmediateOutbound(ExecServerOutboundMessage::Error {
            request_id,
            error,
        }) = routed
        else {
            panic!("expected invalid-params error outbound");
        };
        assert_eq!(request_id, RequestId::Integer(2));
        assert_eq!(error.code, -32602);
    }

    #[test]
    fn routes_initialized_notifications_to_typed_variants() {
        let routed = route_jsonrpc_message(JSONRPCMessage::Notification(JSONRPCNotification {
            method: INITIALIZED_METHOD.to_string(),
            params: Some(json!({})),
        }))
        .expect("initialized notification should route");

        assert_eq!(
            routed,
            RoutedExecServerMessage::Inbound(ExecServerInboundMessage::Notification(
                ExecServerClientNotification::Initialized,
            ))
        );
    }

    #[test]
    fn serializes_typed_notifications_back_to_jsonrpc() {
        let message = encode_outbound_message(ExecServerOutboundMessage::Notification(
            ExecServerServerNotification::Exited(ExecExitedNotification {
                process_id: "proc-1".to_string(),
                exit_code: 0,
            }),
        ))
        .expect("notification should serialize");

        assert_eq!(
            message,
            JSONRPCMessage::Notification(JSONRPCNotification {
                method: EXEC_EXITED_METHOD.to_string(),
                params: Some(json!({
                    "processId": "proc-1",
                    "exitCode": 0,
                })),
            })
        );
    }

    #[test]
    fn serializes_typed_responses_back_to_jsonrpc() {
        let message = encode_outbound_message(ExecServerOutboundMessage::Response {
            request_id: RequestId::Integer(3),
            response: ExecServerResponseMessage::Exec(ExecResponse {
                process_id: "proc-1".to_string(),
            }),
        })
        .expect("response should serialize");

        assert_eq!(
            message,
            JSONRPCMessage::Response(codex_app_server_protocol::JSONRPCResponse {
                id: RequestId::Integer(3),
                result: json!({
                    "processId": "proc-1",
                }),
            })
        );
    }

    #[test]
    fn routes_exec_requests_with_typed_params() {
        let cwd = std::env::current_dir().expect("cwd");
        let routed = route_jsonrpc_message(JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(4),
            method: EXEC_METHOD.to_string(),
            params: Some(json!({
                "processId": "proc-1",
                "argv": ["bash", "-lc", "true"],
                "cwd": cwd,
                "env": {},
                "tty": true,
                "arg0": null,
            })),
            trace: None,
        }))
        .expect("exec request should route");

        let RoutedExecServerMessage::Inbound(ExecServerInboundMessage::Request(
            ExecServerRequest::Exec { request_id, params },
        )) = routed
        else {
            panic!("expected typed exec request");
        };
        assert_eq!(request_id, RequestId::Integer(4));
        assert_eq!(
            params,
            ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().expect("cwd"),
                env: std::collections::HashMap::new(),
                tty: true,
                arg0: None,
                sandbox: None,
            }
        );
    }

    #[test]
    fn routes_exec_requests_with_optional_sandbox_config() {
        let cwd = std::env::current_dir().expect("cwd");
        let routed = route_jsonrpc_message(JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(4),
            method: EXEC_METHOD.to_string(),
            params: Some(json!({
                "processId": "proc-1",
                "argv": ["bash", "-lc", "true"],
                "cwd": cwd,
                "env": {},
                "tty": true,
                "arg0": null,
                "sandbox": {
                    "mode": "none",
                },
            })),
            trace: None,
        }))
        .expect("exec request with sandbox should route");

        let RoutedExecServerMessage::Inbound(ExecServerInboundMessage::Request(
            ExecServerRequest::Exec { request_id, params },
        )) = routed
        else {
            panic!("expected typed exec request");
        };
        assert_eq!(request_id, RequestId::Integer(4));
        assert_eq!(
            params,
            ExecParams {
                process_id: "proc-1".to_string(),
                argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                cwd: std::env::current_dir().expect("cwd"),
                env: std::collections::HashMap::new(),
                tty: true,
                arg0: None,
                sandbox: Some(ExecSandboxConfig {
                    mode: ExecSandboxMode::None,
                }),
            }
        );
    }

    #[test]
    fn unknown_request_methods_return_immediate_invalid_request_errors() {
        let routed = route_jsonrpc_message(JSONRPCMessage::Request(JSONRPCRequest {
            id: RequestId::Integer(5),
            method: "process/unknown".to_string(),
            params: Some(json!({})),
            trace: None,
        }))
        .expect("unknown request should still route");

        assert_eq!(
            routed,
            RoutedExecServerMessage::ImmediateOutbound(ExecServerOutboundMessage::Error {
                request_id: RequestId::Integer(5),
                error: super::invalid_request("unknown method: process/unknown".to_string()),
            })
        );
    }

    #[test]
    fn unexpected_client_notifications_are_rejected() {
        let err = route_jsonrpc_message(JSONRPCMessage::Notification(JSONRPCNotification {
            method: "process/output".to_string(),
            params: Some(json!({})),
        }))
        .expect_err("unexpected client notification should fail");

        assert_eq!(err, "unexpected notification method: process/output");
    }

    #[test]
    fn unexpected_client_responses_are_rejected() {
        let err = route_jsonrpc_message(JSONRPCMessage::Response(JSONRPCResponse {
            id: RequestId::Integer(6),
            result: json!({}),
        }))
        .expect_err("unexpected client response should fail");

        assert_eq!(err, "unexpected client response for request id Integer(6)");
    }
}
