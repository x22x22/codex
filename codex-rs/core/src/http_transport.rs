use crate::default_client::build_reqwest_client;
use crate::openai_socket::openai_unix_socket_path;
use crate::openai_socket::should_use_app_server_responses_bridge;
use async_trait::async_trait;
use codex_api::AnyTransport;
use codex_api::ReqwestTransport;
use codex_client::HttpTransport;
use codex_client::Request;
use codex_client::RequestCompression;
use codex_client::Response;
use codex_client::StreamResponse;
use codex_client::TransportError;
use http::HeaderMap;
use http::Method;
use http::StatusCode;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

#[cfg(unix)]
use codex_api::UdsTransport;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponsesBridgeHttpResponse {
    pub status_code: u16,
    pub body: String,
}

#[async_trait]
pub trait AppServerResponsesBridge: Send + Sync {
    async fn send_responses_request(
        &self,
        request_body: String,
    ) -> Result<ResponsesBridgeHttpResponse, TransportError>;
}

pub struct AppServerResponsesBridgeGuard {
    previous_bridge: Option<Arc<dyn AppServerResponsesBridge>>,
}

impl Drop for AppServerResponsesBridgeGuard {
    fn drop(&mut self) {
        let mut installed_bridge = installed_responses_bridge()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *installed_bridge = self.previous_bridge.take();
    }
}

pub fn install_app_server_responses_bridge(
    bridge: Arc<dyn AppServerResponsesBridge>,
) -> AppServerResponsesBridgeGuard {
    let mut installed_bridge = installed_responses_bridge()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous_bridge = installed_bridge.replace(bridge);
    AppServerResponsesBridgeGuard { previous_bridge }
}

pub fn build_api_transport() -> AnyTransport {
    if should_use_app_server_responses_bridge() {
        if let Some(bridge) = current_app_server_responses_bridge() {
            return AnyTransport::Custom(Arc::new(AppServerResponsesBridgeTransport { bridge }));
        }
        tracing::warn!(
            "Ignoring CODEX_OPENAI_APP_SERVER_BRIDGE because no app-server responses bridge is installed"
        );
    }

    if let Some(socket_path) = openai_unix_socket_path() {
        #[cfg(unix)]
        {
            return AnyTransport::Uds(UdsTransport::new(socket_path));
        }
        #[cfg(not(unix))]
        {
            tracing::warn!(
                "Ignoring CODEX_OPENAI_UNIX_SOCKET because unix sockets are unsupported"
            );
        }
    }

    AnyTransport::Reqwest(ReqwestTransport::new(build_reqwest_client()))
}

fn installed_responses_bridge() -> &'static Mutex<Option<Arc<dyn AppServerResponsesBridge>>> {
    static INSTALLED_BRIDGE: OnceLock<Mutex<Option<Arc<dyn AppServerResponsesBridge>>>> =
        OnceLock::new();
    INSTALLED_BRIDGE.get_or_init(|| Mutex::new(None))
}

fn current_app_server_responses_bridge() -> Option<Arc<dyn AppServerResponsesBridge>> {
    installed_responses_bridge()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

struct AppServerResponsesBridgeTransport {
    bridge: Arc<dyn AppServerResponsesBridge>,
}

impl AppServerResponsesBridgeTransport {
    async fn send_request(
        &self,
        req: Request,
    ) -> Result<ResponsesBridgeHttpResponse, TransportError> {
        let Request {
            method,
            url,
            body,
            compression,
            timeout,
            ..
        } = req;

        if method != Method::POST {
            return Err(TransportError::Build(format!(
                "app-server responses bridge only supports POST requests; got {method}"
            )));
        }

        let url =
            reqwest::Url::parse(&url).map_err(|err| TransportError::Build(err.to_string()))?;
        if !url.path().ends_with("/responses") {
            return Err(TransportError::Build(format!(
                "app-server responses bridge only supports /responses requests; got {}",
                url.path()
            )));
        }

        let request_body = match body {
            Some(body) if compression == RequestCompression::None => {
                serde_json::to_string(&body)
                    .map_err(|err| TransportError::Build(err.to_string()))?
            }
            Some(_) => {
                return Err(TransportError::Build(
                    "app-server responses bridge does not support compressed requests".to_string(),
                ));
            }
            None => {
                return Err(TransportError::Build(
                    "app-server responses bridge requires a JSON request body".to_string(),
                ));
            }
        };

        let request_future = self.bridge.send_responses_request(request_body);
        if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, request_future)
                .await
                .map_err(|_| TransportError::Timeout)?
        } else {
            request_future.await
        }
    }
}

#[async_trait]
impl HttpTransport for AppServerResponsesBridgeTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        let response = self.send_request(req).await?;
        let status = StatusCode::from_u16(response.status_code)
            .map_err(|err| TransportError::Network(err.to_string()))?;
        let headers = bridge_response_headers();
        let body = response.body;
        if !status.is_success() {
            return Err(TransportError::Http {
                status,
                url: None,
                headers: Some(headers),
                body: Some(body),
            });
        }
        Ok(Response {
            status,
            headers,
            body: body.into(),
        })
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        let response = self.send_request(req).await?;
        let status = StatusCode::from_u16(response.status_code)
            .map_err(|err| TransportError::Network(err.to_string()))?;
        let headers = bridge_response_headers();
        if !status.is_success() {
            return Err(TransportError::Http {
                status,
                url: None,
                headers: Some(headers),
                body: Some(response.body),
            });
        }
        Ok(StreamResponse {
            status,
            headers,
            bytes: Box::pin(futures::stream::once(async move {
                Ok::<_, TransportError>(response.body.into())
            })),
        })
    }
}

fn bridge_response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-cache"),
    );
    headers
}

#[cfg(test)]
mod tests {
    use super::AppServerResponsesBridge;
    use super::AppServerResponsesBridgeTransport;
    use super::ResponsesBridgeHttpResponse;
    use async_trait::async_trait;
    use codex_client::HttpTransport;
    use codex_client::Request;
    use codex_client::StreamResponse;
    use codex_client::TransportError;
    use futures::StreamExt;
    use http::Method;
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;

    struct MockResponsesBridge {
        request_bodies: Mutex<Vec<String>>,
        response: ResponsesBridgeHttpResponse,
    }

    #[async_trait]
    impl AppServerResponsesBridge for MockResponsesBridge {
        async fn send_responses_request(
            &self,
            request_body: String,
        ) -> Result<ResponsesBridgeHttpResponse, TransportError> {
            self.request_bodies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request_body);
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn app_server_responses_bridge_transport_executes_requests() {
        let bridge = Arc::new(MockResponsesBridge {
            request_bodies: Mutex::new(Vec::new()),
            response: ResponsesBridgeHttpResponse {
                status_code: 200,
                body: "data: ok\n\n".to_string(),
            },
        });

        let transport = AppServerResponsesBridgeTransport {
            bridge: bridge.clone(),
        };
        let response = transport
            .execute(
                Request::new(Method::POST, "http://localhost/v1/responses".to_string())
                    .with_json(&json!({ "model": "gpt-5.3-codex" })),
            )
            .await
            .expect("bridge response");

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body.as_ref(), b"data: ok\n\n");
        assert_eq!(
            bridge
                .request_bodies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            vec!["{\"model\":\"gpt-5.3-codex\"}".to_string()],
        );
    }

    #[tokio::test]
    async fn app_server_responses_bridge_transport_streams_buffered_sse_body() {
        let transport = AppServerResponsesBridgeTransport {
            bridge: Arc::new(MockResponsesBridge {
                request_bodies: Mutex::new(Vec::new()),
                response: ResponsesBridgeHttpResponse {
                    status_code: 200,
                    body: "data: first\n\ndata: second\n\n".to_string(),
                },
            }),
        };

        let response = transport
            .stream(
                Request::new(Method::POST, "http://localhost/v1/responses".to_string())
                    .with_json(&json!({ "model": "gpt-5.3-codex" })),
            )
            .await
            .expect("stream response");

        assert_stream_response(response, StatusCode::OK, "data: first\n\ndata: second\n\n").await;
    }

    async fn assert_stream_response(
        response: StreamResponse,
        expected_status: StatusCode,
        expected_body: &str,
    ) {
        assert_eq!(response.status, expected_status);
        let body = response
            .bytes
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("stream bytes");
        assert_eq!(
            body.into_iter()
                .map(|bytes| bytes.to_vec())
                .collect::<Vec<_>>(),
            vec![expected_body.as_bytes().to_vec()],
        );
    }
}
