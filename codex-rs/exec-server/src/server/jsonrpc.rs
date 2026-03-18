use codex_app_server_protocol::JSONRPCErrorError;

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

pub(crate) fn unauthorized(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32001,
        data: None,
        message,
    }
}
