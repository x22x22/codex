use http::Error as HttpError;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use opentelemetry::global;
use opentelemetry::propagation::Injector;
use reqwest::IntoUrl;
use reqwest::Method;
use reqwest::Response;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::time::Duration;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Clone, Debug)]
pub struct CodexHttpClient {
    inner: reqwest::Client,
}

const CODEX_TRACE_HTTP_HEADERS_ENV: &str = "CODEX_TRACE_HTTP_HEADERS";
const CODEX_TRACE_HTTP_HEADERS_INCLUDE_SENSITIVE_ENV: &str =
    "CODEX_TRACE_HTTP_HEADERS_INCLUDE_SENSITIVE";
const REDACTED_HEADER_VALUE: &str = "<redacted>";

impl CodexHttpClient {
    pub fn new(inner: reqwest::Client) -> Self {
        Self { inner }
    }

    pub fn get<U>(&self, url: U) -> CodexRequestBuilder
    where
        U: IntoUrl,
    {
        self.request(Method::GET, url)
    }

    pub fn post<U>(&self, url: U) -> CodexRequestBuilder
    where
        U: IntoUrl,
    {
        self.request(Method::POST, url)
    }

    pub fn request<U>(&self, method: Method, url: U) -> CodexRequestBuilder
    where
        U: IntoUrl,
    {
        let url_str = url.as_str().to_string();
        CodexRequestBuilder::new(self.inner.request(method.clone(), url), method, url_str)
    }
}

#[must_use = "requests are not sent unless `send` is awaited"]
#[derive(Debug)]
pub struct CodexRequestBuilder {
    builder: reqwest::RequestBuilder,
    method: Method,
    url: String,
}

impl CodexRequestBuilder {
    fn new(builder: reqwest::RequestBuilder, method: Method, url: String) -> Self {
        Self {
            builder,
            method,
            url,
        }
    }

    fn map(self, f: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder) -> Self {
        Self {
            builder: f(self.builder),
            method: self.method,
            url: self.url,
        }
    }

    pub fn headers(self, headers: HeaderMap) -> Self {
        self.map(|builder| builder.headers(headers))
    }

    pub fn header<K, V>(self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<HttpError>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<HttpError>,
    {
        self.map(|builder| builder.header(key, value))
    }

    pub fn bearer_auth<T>(self, token: T) -> Self
    where
        T: Display,
    {
        self.map(|builder| builder.bearer_auth(token))
    }

    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|builder| builder.timeout(timeout))
    }

    pub fn json<T>(self, value: &T) -> Self
    where
        T: ?Sized + Serialize,
    {
        self.map(|builder| builder.json(value))
    }

    pub fn body<B>(self, body: B) -> Self
    where
        B: Into<reqwest::Body>,
    {
        self.map(|builder| builder.body(body))
    }

    pub async fn send(self) -> Result<Response, reqwest::Error> {
        let builder = self.builder.headers(trace_headers());
        if let Some(request_builder) = builder.try_clone()
            && let Ok(request) = request_builder.build()
        {
            log_http_request(
                self.method.as_str(),
                request.url().as_str(),
                request.headers(),
            );
        }

        match builder.send().await {
            Ok(response) => {
                tracing::debug!(
                    method = %self.method,
                    url = %self.url,
                    status = %response.status(),
                    headers = ?response.headers(),
                    version = ?response.version(),
                    "Request completed"
                );

                Ok(response)
            }
            Err(error) => {
                let status = error.status();
                tracing::debug!(
                    method = %self.method,
                    url = %self.url,
                    status = status.map(|s| s.as_u16()),
                    error = %error,
                    "Request failed"
                );
                Err(error)
            }
        }
    }
}

struct HeaderMapInjector<'a>(&'a mut HeaderMap);

impl<'a> Injector for HeaderMapInjector<'a> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            self.0.insert(name, val);
        }
    }
}

fn trace_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    global::get_text_map_propagator(|prop| {
        prop.inject_context(
            &Span::current().context(),
            &mut HeaderMapInjector(&mut headers),
        );
    });
    headers
}

pub fn log_http_request(method: &str, url: &str, headers: &HeaderMap) {
    if !http_trace_headers_enabled() {
        return;
    }

    tracing::info!(
        method,
        url,
        headers = ?format_headers_for_log(headers),
        "Outbound HTTP request"
    );
}

pub fn format_headers_for_log(headers: &HeaderMap) -> BTreeMap<String, String> {
    let include_sensitive = http_trace_include_sensitive_headers();
    headers
        .iter()
        .map(|(name, value)| {
            let name_str = name.as_str().to_ascii_lowercase();
            let value_str = if include_sensitive || !is_sensitive_header(&name_str) {
                value.to_str().unwrap_or("<binary>").to_string()
            } else {
                REDACTED_HEADER_VALUE.to_string()
            };
            (name_str, value_str)
        })
        .collect()
}

fn http_trace_headers_enabled() -> bool {
    env_flag_enabled(CODEX_TRACE_HTTP_HEADERS_ENV)
}

fn http_trace_include_sensitive_headers() -> bool {
    env_flag_enabled(CODEX_TRACE_HTTP_HEADERS_INCLUDE_SENSITIVE_ENV)
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !normalized.is_empty()
                && normalized != "0"
                && normalized != "false"
                && normalized != "no"
                && normalized != "off"
        })
        .unwrap_or(false)
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key" | "api-key"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::propagation::Extractor;
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry::trace::TraceContextExt;
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing::trace_span;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    #[test]
    fn inject_trace_headers_uses_current_span_context() {
        global::set_text_map_propagator(TraceContextPropagator::new());

        let provider = SdkTracerProvider::builder().build();
        let tracer = provider.tracer("test-tracer");
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
        let _guard = subscriber.set_default();

        let span = trace_span!("client_request");
        let _entered = span.enter();
        let span_context = span.context().span().span_context().clone();

        let headers = trace_headers();

        let extractor = HeaderMapExtractor(&headers);
        let extracted = TraceContextPropagator::new().extract(&extractor);
        let extracted_span = extracted.span();
        let extracted_context = extracted_span.span_context();

        assert!(extracted_context.is_valid());
        assert_eq!(extracted_context.trace_id(), span_context.trace_id());
        assert_eq!(extracted_context.span_id(), span_context.span_id());
    }

    struct HeaderMapExtractor<'a>(&'a HeaderMap);

    impl<'a> Extractor for HeaderMapExtractor<'a> {
        fn get(&self, key: &str) -> Option<&str> {
            self.0.get(key).and_then(|value| value.to_str().ok())
        }

        fn keys(&self) -> Vec<&str> {
            self.0.keys().map(HeaderName::as_str).collect()
        }
    }
}
