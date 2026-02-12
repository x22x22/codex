use crate::default_client::CodexHttpClient;
use crate::default_client::CodexRequestBuilder;
use crate::error::TransportError;
use crate::request::Request;
use crate::request::RequestCompression;
use crate::request::Response;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use http::HeaderMap;
use http::Method;
use http::StatusCode;
use http_body_util::BodyExt;
use http_body_util::Full;
use tracing::Level;
use tracing::enabled;
use tracing::trace;

pub type ByteStream = BoxStream<'static, Result<Bytes, TransportError>>;

pub struct StreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: ByteStream,
}

#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn execute(&self, req: Request) -> Result<Response, TransportError>;
    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError>;
}

#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: CodexHttpClient,
}

impl ReqwestTransport {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client: CodexHttpClient::new(client),
        }
    }

    fn build(&self, req: Request) -> Result<CodexRequestBuilder, TransportError> {
        let Request {
            method,
            url,
            mut headers,
            body,
            compression,
            timeout,
        } = req;

        let mut builder = self.client.request(
            Method::from_bytes(method.as_str().as_bytes()).unwrap_or(Method::GET),
            &url,
        );

        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }

        if let Some(body) = body {
            if compression != RequestCompression::None {
                if headers.contains_key(http::header::CONTENT_ENCODING) {
                    return Err(TransportError::Build(
                        "request compression was requested but content-encoding is already set"
                            .to_string(),
                    ));
                }

                let json = serde_json::to_vec(&body)
                    .map_err(|err| TransportError::Build(err.to_string()))?;
                let pre_compression_bytes = json.len();
                let compression_start = std::time::Instant::now();
                let (compressed, content_encoding) = match compression {
                    RequestCompression::None => unreachable!("guarded by compression != None"),
                    RequestCompression::Zstd => (
                        zstd::stream::encode_all(std::io::Cursor::new(json), 3)
                            .map_err(|err| TransportError::Build(err.to_string()))?,
                        http::HeaderValue::from_static("zstd"),
                    ),
                };
                let post_compression_bytes = compressed.len();
                let compression_duration = compression_start.elapsed();

                // Ensure the server knows to unpack the request body.
                headers.insert(http::header::CONTENT_ENCODING, content_encoding);
                if !headers.contains_key(http::header::CONTENT_TYPE) {
                    headers.insert(
                        http::header::CONTENT_TYPE,
                        http::HeaderValue::from_static("application/json"),
                    );
                }

                tracing::info!(
                    pre_compression_bytes,
                    post_compression_bytes,
                    compression_duration_ms = compression_duration.as_millis(),
                    "Compressed request body with zstd"
                );

                builder = builder.headers(headers).body(compressed);
            } else {
                builder = builder.headers(headers).json(&body);
            }
        } else {
            builder = builder.headers(headers);
        }
        Ok(builder)
    }

    fn map_error(err: reqwest::Error) -> TransportError {
        if err.is_timeout() {
            TransportError::Timeout
        } else {
            TransportError::Network(err.to_string())
        }
    }
}

#[derive(Clone, Debug)]
pub enum AnyTransport {
    Reqwest(ReqwestTransport),
    #[cfg(unix)]
    Uds(UdsTransport),
}

#[cfg(unix)]
#[derive(Clone, Debug)]
pub struct UdsTransport {
    socket_path: std::path::PathBuf,
}

#[cfg(unix)]
impl UdsTransport {
    pub fn new(socket_path: std::path::PathBuf) -> Self {
        Self { socket_path }
    }

    async fn send_request(
        &self,
        req: Request,
    ) -> Result<hyper::Response<hyper::body::Incoming>, TransportError> {
        use hyper::client::conn::http1;
        use hyper_util::rt::TokioIo;
        use tokio::net::UnixStream;

        let PreparedRequest {
            method,
            uri,
            headers,
            body,
            timeout,
        } = prepare_request(req)?;

        let request_body = match body {
            Some(body) => Full::new(Bytes::from(body)),
            None => Full::new(Bytes::new()),
        };

        let request = {
            let mut builder = hyper::Request::builder().method(method).uri(uri);
            for (name, value) in headers.iter() {
                builder = builder.header(name, value);
            }
            builder
                .body(request_body)
                .map_err(|err| TransportError::Build(err.to_string()))?
        };

        let connect = async {
            let stream = UnixStream::connect(&self.socket_path)
                .await
                .map_err(|err| TransportError::Network(err.to_string()))?;
            let io = TokioIo::new(stream);
            let (mut sender, conn) = http1::handshake(io)
                .await
                .map_err(|err| TransportError::Network(err.to_string()))?;
            tokio::spawn(async move {
                if let Err(err) = conn.await {
                    tracing::debug!("UDS connection closed: {err}");
                }
            });
            sender
                .send_request(request)
                .await
                .map_err(|err| TransportError::Network(err.to_string()))
        };

        if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, connect)
                .await
                .map_err(|_| TransportError::Timeout)?
        } else {
            connect.await
        }
    }
}

#[cfg(unix)]
struct PreparedRequest {
    method: Method,
    uri: http::Uri,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    timeout: Option<std::time::Duration>,
}

#[cfg(unix)]
fn prepare_request(req: Request) -> Result<PreparedRequest, TransportError> {
    use http::header::CONTENT_ENCODING;
    use http::header::CONTENT_TYPE;
    use http::header::HOST;

    let Request {
        method,
        url,
        mut headers,
        body,
        compression,
        timeout,
    } = req;

    let uri = build_uds_uri(&url)?;

    if !headers.contains_key(HOST)
        && let Ok(host) = host_header_from_url(&url)
    {
        headers.insert(HOST, host);
    }

    let body_bytes = if let Some(body) = body {
        if compression != RequestCompression::None {
            if headers.contains_key(CONTENT_ENCODING) {
                return Err(TransportError::Build(
                    "request compression was requested but content-encoding is already set"
                        .to_string(),
                ));
            }

            let json =
                serde_json::to_vec(&body).map_err(|err| TransportError::Build(err.to_string()))?;
            let pre_compression_bytes = json.len();
            let compression_start = std::time::Instant::now();
            let (compressed, content_encoding) = match compression {
                RequestCompression::None => unreachable!("guarded by compression != None"),
                RequestCompression::Zstd => (
                    zstd::stream::encode_all(std::io::Cursor::new(json), 3)
                        .map_err(|err| TransportError::Build(err.to_string()))?,
                    http::HeaderValue::from_static("zstd"),
                ),
            };
            let post_compression_bytes = compressed.len();
            let compression_duration = compression_start.elapsed();

            headers.insert(CONTENT_ENCODING, content_encoding);
            if !headers.contains_key(CONTENT_TYPE) {
                headers.insert(
                    CONTENT_TYPE,
                    http::HeaderValue::from_static("application/json"),
                );
            }

            tracing::info!(
                pre_compression_bytes,
                post_compression_bytes,
                compression_duration_ms = compression_duration.as_millis(),
                "Compressed request body with zstd"
            );

            Some(compressed)
        } else {
            if !headers.contains_key(CONTENT_TYPE) {
                headers.insert(
                    CONTENT_TYPE,
                    http::HeaderValue::from_static("application/json"),
                );
            }
            Some(serde_json::to_vec(&body).map_err(|err| TransportError::Build(err.to_string()))?)
        }
    } else {
        None
    };

    Ok(PreparedRequest {
        method,
        uri,
        headers,
        body: body_bytes,
        timeout,
    })
}

#[cfg(unix)]
fn build_uds_uri(url: &str) -> Result<http::Uri, TransportError> {
    let url = reqwest::Url::parse(url).map_err(|err| TransportError::Build(err.to_string()))?;
    let path = url.path();
    let path_and_query = match url.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    http::Uri::builder()
        .path_and_query(path_and_query)
        .build()
        .map_err(|err| TransportError::Build(err.to_string()))
}

#[cfg(unix)]
fn host_header_from_url(url: &str) -> Result<http::HeaderValue, TransportError> {
    let url = reqwest::Url::parse(url).map_err(|err| TransportError::Build(err.to_string()))?;
    let host = match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        _ => return Err(TransportError::Build("missing host".to_string())),
    };
    http::HeaderValue::from_str(&host).map_err(|err| TransportError::Build(err.to_string()))
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        if enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                req.body.as_ref().unwrap_or_default()
            );
        }

        let url = req.url.clone();
        let builder = self.build(req)?;
        let resp = builder.send().await.map_err(Self::map_error)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.map_err(Self::map_error)?;
        if !status.is_success() {
            let body = String::from_utf8(bytes.to_vec()).ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        Ok(Response {
            status,
            headers,
            body: bytes,
        })
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        if enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                req.body.as_ref().unwrap_or_default()
            );
        }

        let url = req.url.clone();
        let builder = self.build(req)?;
        let resp = builder.send().await.map_err(Self::map_error)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let body = resp.text().await.ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        let stream = resp
            .bytes_stream()
            .map(|result| result.map_err(Self::map_error));
        Ok(StreamResponse {
            status,
            headers,
            bytes: Box::pin(stream),
        })
    }
}

#[cfg(unix)]
#[async_trait]
impl HttpTransport for UdsTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        if enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                req.body.as_ref().unwrap_or_default()
            );
        }

        let url = req.url.clone();
        let resp = self.send_request(req).await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|err| TransportError::Network(err.to_string()))?
            .to_bytes();
        if !status.is_success() {
            let body = String::from_utf8(bytes.to_vec()).ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        Ok(Response {
            status,
            headers,
            body: bytes,
        })
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        if enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                req.body.as_ref().unwrap_or_default()
            );
        }

        let url = req.url.clone();
        let resp = self.send_request(req).await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let body = resp
                .into_body()
                .collect()
                .await
                .ok()
                .and_then(|collected| String::from_utf8(collected.to_bytes().to_vec()).ok());
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        let stream = resp
            .into_body()
            .into_data_stream()
            .map(|result| result.map_err(|err| TransportError::Network(err.to_string())));
        Ok(StreamResponse {
            status,
            headers,
            bytes: Box::pin(stream),
        })
    }
}

#[async_trait]
impl HttpTransport for AnyTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        match self {
            AnyTransport::Reqwest(transport) => transport.execute(req).await,
            #[cfg(unix)]
            AnyTransport::Uds(transport) => transport.execute(req).await,
        }
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        match self {
            AnyTransport::Reqwest(transport) => transport.stream(req).await,
            #[cfg(unix)]
            AnyTransport::Uds(transport) => transport.stream(req).await,
        }
    }
}
