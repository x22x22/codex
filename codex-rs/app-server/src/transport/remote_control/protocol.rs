use crate::outgoing_message::OutgoingMessage;
use codex_app_server_protocol::JSONRPCMessage;
use serde::Deserialize;
use serde::Serialize;
use std::io;
use std::io::ErrorKind;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteControlTarget {
    pub(super) websocket_url: String,
    pub(super) enroll_url: String,
}

#[derive(Debug, Serialize)]
pub(super) struct EnrollRemoteServerRequest {
    pub(super) name: String,
    pub(super) os: &'static str,
    pub(super) arch: &'static str,
    pub(super) app_server_version: &'static str,
}

#[derive(Debug, Deserialize)]
pub(super) struct EnrollRemoteServerResponse {
    pub(super) server_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    ClientMessage {
        message: JSONRPCMessage,
    },
    Ack {
        #[serde(rename = "acked_seq_id", alias = "ackedSeqId")]
        acked_seq_id: u64,
    },
    Ping,
    ClientClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ClientEnvelope {
    #[serde(flatten)]
    pub(crate) event: ClientEvent,
    #[serde(rename = "client_id", alias = "clientId")]
    pub(crate) client_id: ClientId,
    #[serde(
        rename = "seq_id",
        alias = "seqId",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) seq_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PongStatus {
    Active,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    ServerMessage {
        message: Box<OutgoingMessage>,
    },
    #[allow(dead_code)]
    Ack,
    Pong {
        status: PongStatus,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ServerEnvelope {
    #[serde(flatten)]
    pub(crate) event: ServerEvent,
    #[serde(rename = "client_id", alias = "clientId")]
    pub(crate) client_id: ClientId,
    #[serde(
        rename = "seq_id",
        alias = "seqId",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) seq_id: Option<u64>,
}

pub(super) fn normalize_remote_control_url(
    remote_control_url: &str,
) -> io::Result<RemoteControlTarget> {
    let map_url_parse_error = |err: url::ParseError| -> io::Error {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control URL `{remote_control_url}`: {err}"),
        )
    };
    let map_scheme_error = |_: ()| -> io::Error {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "invalid remote control URL `{remote_control_url}`; expected absolute URL with http:// or https:// scheme"
            ),
        )
    };

    let mut remote_control_url = Url::parse(remote_control_url).map_err(map_url_parse_error)?;
    match remote_control_url.scheme() {
        "https" | "http" => {}
        _ => return Err(map_scheme_error(())),
    }
    if !remote_control_url.path().ends_with('/') {
        let normalized_path = format!("{}/", remote_control_url.path());
        remote_control_url.set_path(&normalized_path);
    }

    let mut enroll_url = remote_control_url
        .join("wham/remote/control/server/enroll")
        .map_err(map_url_parse_error)?;
    let mut websocket_url = remote_control_url
        .join("wham/remote/control/server")
        .map_err(map_url_parse_error)?;
    match remote_control_url.scheme() {
        "https" => {
            enroll_url.set_scheme("https").map_err(map_scheme_error)?;
            websocket_url.set_scheme("wss").map_err(map_scheme_error)?;
        }
        "http" => {
            enroll_url.set_scheme("http").map_err(map_scheme_error)?;
            websocket_url.set_scheme("ws").map_err(map_scheme_error)?;
        }
        _ => return Err(map_scheme_error(())),
    }

    Ok(RemoteControlTarget {
        websocket_url: websocket_url.to_string(),
        enroll_url: enroll_url.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn normalize_remote_control_url_rewrites_http_schemes() {
        assert_eq!(
            normalize_remote_control_url("http://example.com/backend-api")
                .expect("valid http prefix"),
            RemoteControlTarget {
                websocket_url: "ws://example.com/backend-api/wham/remote/control/server"
                    .to_string(),
                enroll_url: "http://example.com/backend-api/remote/control/server/enroll"
                    .to_string(),
            }
        );
        assert_eq!(
            normalize_remote_control_url("https://example.com/backend-api/")
                .expect("valid https prefix"),
            RemoteControlTarget {
                websocket_url: "wss://example.com/backend-api/wham/remote/control/server"
                    .to_string(),
                enroll_url: "https://example.com/backend-api/remote/control/server/enroll"
                    .to_string(),
            }
        );
    }

    #[test]
    fn normalize_remote_control_url_rejects_unsupported_schemes() {
        let err = normalize_remote_control_url("ftp://example.com/control")
            .expect_err("unsupported scheme should fail");
        assert_eq!(
            err.to_string(),
            "invalid remote control URL `ftp://example.com/control`; expected absolute URL with http:// or https:// scheme"
        );
    }
}
