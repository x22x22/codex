use std::collections::HashMap;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RpcServerInboundMessage {
    Request(JSONRPCRequest),
    Notification(JSONRPCNotification),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RpcServerOutboundMessage {
    Response {
        request_id: RequestId,
        result: Value,
    },
    Error {
        request_id: RequestId,
        error: JSONRPCErrorError,
    },
}

type RequestRoute<I> = Box<dyn Fn(JSONRPCRequest) -> I + Send + Sync>;
type NotificationRoute<I> = Box<dyn Fn(JSONRPCNotification) -> Result<I, String> + Send + Sync>;

pub(crate) struct RpcRouter<I> {
    request_routes: HashMap<&'static str, RequestRoute<I>>,
    notification_routes: HashMap<&'static str, NotificationRoute<I>>,
}

impl<I> Default for RpcRouter<I> {
    fn default() -> Self {
        Self {
            request_routes: HashMap::new(),
            notification_routes: HashMap::new(),
        }
    }
}

impl<I> RpcRouter<I> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn raw_request<F>(&mut self, method: &'static str, route: F)
    where
        F: Fn(JSONRPCRequest) -> I + Send + Sync + 'static,
    {
        self.request_routes.insert(method, Box::new(route));
    }

    pub(crate) fn notification<F>(&mut self, method: &'static str, route: F)
    where
        F: Fn(JSONRPCNotification) -> Result<I, String> + Send + Sync + 'static,
    {
        self.notification_routes.insert(method, Box::new(route));
    }

    pub(crate) fn route_message(
        &self,
        message: JSONRPCMessage,
        unknown_request: impl FnOnce(JSONRPCRequest) -> I,
    ) -> Result<I, String> {
        match route_server_message(message)? {
            RpcServerInboundMessage::Request(request) => {
                if let Some(route) = self.request_routes.get(request.method.as_str()) {
                    Ok(route(request))
                } else {
                    Ok(unknown_request(request))
                }
            }
            RpcServerInboundMessage::Notification(notification) => {
                let Some(route) = self.notification_routes.get(notification.method.as_str()) else {
                    return Err(format!(
                        "unexpected notification method: {}",
                        notification.method
                    ));
                };
                route(notification)
            }
        }
    }
}

pub(crate) fn route_server_message(
    message: JSONRPCMessage,
) -> Result<RpcServerInboundMessage, String> {
    match message {
        JSONRPCMessage::Request(request) => Ok(RpcServerInboundMessage::Request(request)),
        JSONRPCMessage::Notification(notification) => {
            Ok(RpcServerInboundMessage::Notification(notification))
        }
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

pub(crate) fn encode_server_message(
    message: RpcServerOutboundMessage,
) -> Result<JSONRPCMessage, serde_json::Error> {
    match message {
        RpcServerOutboundMessage::Response { request_id, result } => {
            Ok(JSONRPCMessage::Response(JSONRPCResponse {
                id: request_id,
                result,
            }))
        }
        RpcServerOutboundMessage::Error { request_id, error } => {
            Ok(JSONRPCMessage::Error(JSONRPCError {
                id: request_id,
                error,
            }))
        }
    }
}
