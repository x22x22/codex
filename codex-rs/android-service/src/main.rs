use std::collections::HashMap;
use std::convert::Infallible;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context as TaskContext;
use std::task::Poll;

use anyhow::Context;
use anyhow::Result;
use bytes::Bytes;
use clap::Parser;
use clap::ValueEnum;
use codex_core::ModelProviderInfo;
use codex_core::auth::AuthCredentialsStoreMode;
use codex_core::auth::AuthManager;
use codex_core::auth::CodexAuth;
use codex_core::config::ConfigBuilder;
use codex_core::config::find_codex_home;
use codex_core::default_client::build_reqwest_client;
use codex_core::models_manager::collaboration_mode_presets::CollaborationModesConfig;
use codex_core::models_manager::manager::ModelsManager;
use codex_core::models_manager::manager::RefreshStrategy;
use codex_login::CLIENT_ID;
use codex_login::DeviceCode;
use codex_login::ServerOptions;
use codex_login::complete_device_code_login;
use codex_login::request_device_code;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::StreamBody;
use http_body_util::combinators::BoxBody;
use hyper::Method;
use hyper::Request;
use hyper::Response;
use hyper::StatusCode;
use hyper::body::Frame;
use hyper::body::Incoming;
use hyper::header::AUTHORIZATION;
use hyper::header::HOST;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use reqwest::Url;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::ReadBuf;
use tokio::net::UnixListener;
use tokio::net::UnixStream;
use tokio::sync::Mutex;

#[derive(Clone)]
struct ServiceState {
    upstream_base_override: Option<Url>,
    client: reqwest::Client,
    codex_home: PathBuf,
    auth_manager: Arc<AuthManager>,
    auth_store: AuthCredentialsStoreMode,
    auth_header_override: Option<String>,
    device_auth: Arc<Mutex<DeviceAuthState>>,
    active_connections: Arc<AtomicUsize>,
    client_registry: Arc<ClientRegistry>,
}

#[derive(Debug, Default)]
struct ClientRegistry {
    next_anonymous_id: AtomicUsize,
    clients: Mutex<HashMap<String, Arc<ClientEntry>>>,
}

#[derive(Debug)]
struct ClientEntry {
    id: String,
    active_connections: AtomicUsize,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
}

#[derive(Debug)]
struct ConnectionGuard {
    counter: Arc<AtomicUsize>,
    client_entry: Arc<ClientEntry>,
}

impl ConnectionGuard {
    fn new(counter: Arc<AtomicUsize>, client_entry: Arc<ClientEntry>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        client_entry
            .active_connections
            .fetch_add(1, Ordering::SeqCst);
        Self {
            counter,
            client_entry,
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
        self.client_entry
            .active_connections
            .fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct CountingStream {
    inner: UnixStream,
    client_entry: Arc<ClientEntry>,
}

impl ClientEntry {
    fn new(id: String) -> Self {
        Self {
            id,
            active_connections: AtomicUsize::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> ClientStats {
        ClientStats {
            id: self.id.clone(),
            active_connections: self.active_connections.load(Ordering::SeqCst),
            bytes_sent: self.bytes_sent.load(Ordering::SeqCst),
            bytes_received: self.bytes_received.load(Ordering::SeqCst),
        }
    }
}

impl ClientRegistry {
    async fn register_connection(&self, stream: &UnixStream) -> Arc<ClientEntry> {
        let client_id = Self::client_id(stream, &self.next_anonymous_id);
        let mut clients = self.clients.lock().await;
        clients
            .entry(client_id.clone())
            .or_insert_with(|| Arc::new(ClientEntry::new(client_id)))
            .clone()
    }

    async fn clients_snapshot(&self) -> Vec<ClientStats> {
        let clients = self.clients.lock().await;
        let mut connected_clients: Vec<ClientStats> = clients
            .values()
            .map(Arc::as_ref)
            .map(ClientEntry::snapshot)
            .collect();
        connected_clients.sort_by(|left, right| left.id.cmp(&right.id));
        connected_clients
    }

    fn client_id(stream: &UnixStream, next_anonymous_id: &AtomicUsize) -> String {
        if let Ok(peer_cred) = stream.peer_cred() {
            if let Some(pid) = peer_cred.pid() {
                return format!("uid:{} pid:{pid}", peer_cred.uid());
            }
            return format!("uid:{}", peer_cred.uid());
        }
        let next_id = next_anonymous_id.fetch_add(1, Ordering::SeqCst);
        format!("anonymous-{next_id}")
    }
}

impl CountingStream {
    fn new(inner: UnixStream, client_entry: Arc<ClientEntry>) -> Self {
        Self {
            inner,
            client_entry,
        }
    }
}

impl AsyncRead for CountingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let previous_len = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let bytes_read = buf.filled().len().saturating_sub(previous_len);
                if bytes_read > 0 {
                    self.client_entry
                        .bytes_received
                        .fetch_add(bytes_read as u64, Ordering::SeqCst);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl AsyncWrite for CountingStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(bytes_written)) => {
                if bytes_written > 0 {
                    self.client_entry
                        .bytes_sent
                        .fetch_add(bytes_written as u64, Ordering::SeqCst);
                }
                Poll::Ready(Ok(bytes_written))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write_vectored(cx, bufs) {
            Poll::Ready(Ok(bytes_written)) => {
                if bytes_written > 0 {
                    self.client_entry
                        .bytes_sent
                        .fetch_add(bytes_written as u64, Ordering::SeqCst);
                }
                Poll::Ready(Ok(bytes_written))
            }
            other => other,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    authenticated: bool,
    auth_mode: Option<String>,
    account_id: Option<String>,
    account_email: Option<String>,
    client_count: usize,
    clients: Vec<ClientStats>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    authenticated: bool,
    auth_mode: Option<String>,
    account_id: Option<String>,
    account_email: Option<String>,
    client_count: usize,
    clients: Vec<ClientStats>,
    model_provider_id: String,
    configured_model: Option<String>,
    effective_model: Option<String>,
    upstream_base_url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientStats {
    id: String,
    active_connections: usize,
    bytes_sent: u64,
    bytes_received: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeviceAuthStatus {
    Pending,
    InProgress,
    AlreadyAuthenticated,
}

#[derive(Clone, Debug, Serialize)]
struct DeviceAuthResponse {
    status: DeviceAuthStatus,
    verification_url: Option<String>,
    user_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct DeviceAuthRequest {
    issuer_base_url: Option<String>,
    client_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct DeviceAuthState {
    device_code: Option<DeviceCode>,
}

#[derive(Clone, Debug, ValueEnum)]
enum AuthStoreModeArg {
    Auto,
    File,
    Keyring,
    Ephemeral,
}

impl From<AuthStoreModeArg> for AuthCredentialsStoreMode {
    fn from(value: AuthStoreModeArg) -> Self {
        match value {
            AuthStoreModeArg::Auto => Self::Auto,
            AuthStoreModeArg::File => Self::File,
            AuthStoreModeArg::Keyring => Self::Keyring,
            AuthStoreModeArg::Ephemeral => Self::Ephemeral,
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "codexd", about = "Codex Android local socket service")]
struct Args {
    /// Unix domain socket path to listen on.
    #[arg(long, value_name = "PATH")]
    socket_path: PathBuf,

    /// Base URL for upstream OpenAI API.
    #[arg(long)]
    upstream_base_url: Option<String>,

    /// Override CODEX_HOME for reading auth.json.
    #[arg(long)]
    codex_home: Option<PathBuf>,

    /// Auth storage backend to read tokens from.
    #[arg(long, value_enum, default_value = "auto")]
    auth_store: AuthStoreModeArg,

    /// Explicit Authorization header value (for testing).
    #[arg(long)]
    auth_header: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum UnixSocketBindTarget {
    Filesystem(PathBuf),
    Abstract(String),
}

type BoxedBody = BoxBody<Bytes, anyhow::Error>;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let codex_home = match args.codex_home {
        Some(path) => path,
        None => find_codex_home()?,
    };
    let auth_store = AuthCredentialsStoreMode::from(args.auth_store);
    let auth_manager = Arc::new(AuthManager::new(codex_home.clone(), true, auth_store));
    let upstream_base_override = args
        .upstream_base_url
        .as_deref()
        .map(Url::parse)
        .transpose()
        .context("failed to parse --upstream-base-url")?;
    let client = build_reqwest_client();
    let socket_bind_target = parse_socket_bind_target(&args.socket_path)?;
    let listener = bind_listener(&socket_bind_target)
        .with_context(|| format!("failed to bind socket at {}", args.socket_path.display()))?;
    if let UnixSocketBindTarget::Filesystem(path) = &socket_bind_target {
        set_socket_permissions(path);
    }

    let state = Arc::new(ServiceState {
        upstream_base_override,
        client,
        codex_home,
        auth_manager,
        auth_store,
        auth_header_override: args.auth_header,
        device_auth: Arc::new(Mutex::new(DeviceAuthState::default())),
        active_connections: Arc::new(AtomicUsize::new(0)),
        client_registry: Arc::new(ClientRegistry::default()),
    });

    let auto_state = state.clone();
    tokio::spawn(async move {
        match start_device_auth_flow(&auto_state, DeviceAuthRequest::default()).await {
            Ok(response) => {
                if let Some(code) = response.user_code.as_deref()
                    && let Some(url) = response.verification_url.as_deref()
                    && matches!(
                        response.status,
                        DeviceAuthStatus::Pending | DeviceAuthStatus::InProgress
                    )
                {
                    log_device_code_prompt(url, code);
                }
            }
            Err(err) => {
                eprintln!("device auth auto-start failed: {err}");
            }
        }
    });

    eprintln!("codexd listening on {}", args.socket_path.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let client_entry = state.client_registry.register_connection(&stream).await;
            let _guard =
                ConnectionGuard::new(state.active_connections.clone(), client_entry.clone());
            let io = TokioIo::new(CountingStream::new(stream, client_entry));
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(|req| handle_request(req, state.clone())))
                .await
            {
                eprintln!("connection error: {err}");
            }
        });
    }
}

async fn handle_request(
    req: Request<Incoming>,
    state: Arc<ServiceState>,
) -> Result<Response<BoxedBody>, Infallible> {
    let path = req.uri().path();
    if path == "/internal/runtime/status" {
        return Ok(handle_runtime_status(req, &state).await);
    }
    if path == "/internal/auth/status" {
        return Ok(handle_auth_status(req, &state).await);
    }
    if path == "/internal/auth/logout" {
        return Ok(handle_auth_logout(req, &state).await);
    }
    if path == "/internal/auth/device" {
        return Ok(handle_auth_device(req, &state).await);
    }

    Ok(handle_proxy_request(req, &state).await)
}

async fn handle_auth_status(req: Request<Incoming>, state: &ServiceState) -> Response<BoxedBody> {
    if req.method() != Method::GET {
        return empty_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    let auth = state.auth_manager.auth().await;
    let clients = state.client_registry.clients_snapshot().await;
    let active_client_count = clients
        .iter()
        .filter(|client| client.active_connections > 0)
        .count();
    let status = AuthStatus {
        authenticated: auth.is_some(),
        auth_mode: auth
            .as_ref()
            .map(|auth| api_auth_mode_label(auth).to_string()),
        account_id: auth.as_ref().and_then(CodexAuth::get_account_id),
        account_email: auth.as_ref().and_then(CodexAuth::get_account_email),
        client_count: active_client_count,
        clients,
    };

    json_response(StatusCode::OK, &status)
}

async fn handle_runtime_status(
    req: Request<Incoming>,
    state: &ServiceState,
) -> Response<BoxedBody> {
    if req.method() != Method::GET {
        return empty_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    let auth = state.auth_manager.auth().await;
    let clients = state.client_registry.clients_snapshot().await;
    let active_client_count = clients
        .iter()
        .filter(|client| client.active_connections > 0)
        .count();
    let runtime_model = resolve_runtime_model(state).await;
    let status = RuntimeStatus {
        authenticated: auth.is_some(),
        auth_mode: auth
            .as_ref()
            .map(|active_auth| api_auth_mode_label(active_auth).to_string()),
        account_id: auth.as_ref().and_then(CodexAuth::get_account_id),
        account_email: auth.as_ref().and_then(CodexAuth::get_account_email),
        client_count: active_client_count,
        clients,
        model_provider_id: runtime_model.model_provider_id,
        configured_model: runtime_model.configured_model,
        effective_model: runtime_model.effective_model,
        upstream_base_url: state.upstream_base(auth.as_ref()).to_string(),
    };

    json_response(StatusCode::OK, &status)
}

async fn handle_auth_logout(req: Request<Incoming>, state: &ServiceState) -> Response<BoxedBody> {
    if req.method() != Method::POST {
        return empty_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    match state.auth_manager.logout() {
        Ok(_) => empty_response(StatusCode::OK),
        Err(err) => text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn handle_auth_device(req: Request<Incoming>, state: &ServiceState) -> Response<BoxedBody> {
    if req.method() != Method::POST {
        return empty_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    let body = req.into_body();
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return text_response(StatusCode::BAD_REQUEST, err.to_string());
        }
    };

    let device_request = if body_bytes.is_empty() {
        DeviceAuthRequest::default()
    } else {
        match serde_json::from_slice::<DeviceAuthRequest>(&body_bytes) {
            Ok(request) => request,
            Err(err) => {
                return text_response(StatusCode::BAD_REQUEST, err.to_string());
            }
        }
    };

    match start_device_auth_flow(state, device_request).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(err) => text_response(StatusCode::BAD_GATEWAY, err),
    }
}

async fn handle_proxy_request(req: Request<Incoming>, state: &ServiceState) -> Response<BoxedBody> {
    let local_uri = req.uri().to_string();
    let auth = state.auth_manager.auth().await;
    let auth_header = match state.auth_header(auth.as_ref()) {
        Ok(header) => header,
        Err(AuthHeaderError::Missing) => {
            return text_response(StatusCode::UNAUTHORIZED, "not authenticated".to_string());
        }
        Err(AuthHeaderError::Invalid(err)) => {
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
        }
    };

    let upstream_url = match build_upstream_url(
        &state.upstream_base(auth.as_ref()),
        req.uri(),
        auth.as_ref().is_some_and(CodexAuth::is_chatgpt_auth),
    ) {
        Ok(url) => url,
        Err(err) => {
            return text_response(StatusCode::BAD_REQUEST, err.to_string());
        }
    };

    let (parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return text_response(StatusCode::BAD_REQUEST, err.to_string());
        }
    };

    let auth_mode = auth.as_ref().map_or("missing", api_auth_mode_label);
    let request_method = parts.method.clone();
    let upstream_url_string = upstream_url.to_string();
    eprintln!(
        "proxy {request_method} {local_uri} -> {upstream_url_string} (auth_mode={auth_mode})"
    );

    let mut builder = state
        .client
        .request(parts.method, upstream_url)
        .body(body_bytes);

    for (name, value) in parts.headers.iter() {
        if name == AUTHORIZATION || name == HOST {
            continue;
        }
        builder = builder.header(name, value);
    }

    builder = builder.header(AUTHORIZATION, auth_header);
    if auth.as_ref().is_some_and(CodexAuth::is_chatgpt_auth)
        && let Some(account_id) = auth.as_ref().and_then(CodexAuth::get_account_id)
    {
        builder = builder.header("ChatGPT-Account-ID", account_id);
    }

    let upstream = match builder.send().await {
        Ok(response) => response,
        Err(err) => {
            return text_response(StatusCode::BAD_GATEWAY, err.to_string());
        }
    };

    let mut response_builder = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers().iter() {
        if should_skip_response_header(name) {
            continue;
        }
        response_builder = response_builder.header(name, value);
    }

    let stream = upstream
        .bytes_stream()
        .map(|chunk| chunk.map(Frame::data).map_err(anyhow::Error::from));
    let body = BodyExt::boxed(StreamBody::new(stream));

    response_builder
        .body(body)
        .unwrap_or_else(|err| text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

async fn start_device_auth_flow(
    state: &ServiceState,
    device_request: DeviceAuthRequest,
) -> std::result::Result<DeviceAuthResponse, String> {
    if state.auth_manager.auth().await.is_some() {
        return Ok(DeviceAuthResponse {
            status: DeviceAuthStatus::AlreadyAuthenticated,
            verification_url: None,
            user_code: None,
        });
    }

    let mut device_auth = state.device_auth.lock().await;
    if let Some(device_code) = device_auth.device_code.as_ref() {
        return Ok(DeviceAuthResponse {
            status: DeviceAuthStatus::InProgress,
            verification_url: Some(device_code.verification_url.clone()),
            user_code: Some(device_code.user_code.clone()),
        });
    }

    let mut opts = ServerOptions::new(
        state.codex_home.clone(),
        device_request
            .client_id
            .unwrap_or_else(|| CLIENT_ID.to_string()),
        None,
        state.auth_store,
    );
    if let Some(issuer) = device_request.issuer_base_url {
        opts.issuer = issuer;
    }
    opts.open_browser = false;

    let device_code = request_device_code(&opts)
        .await
        .map_err(|err| err.to_string())?;

    let response = DeviceAuthResponse {
        status: DeviceAuthStatus::Pending,
        verification_url: Some(device_code.verification_url.clone()),
        user_code: Some(device_code.user_code.clone()),
    };
    device_auth.device_code = Some(device_code.clone());
    drop(device_auth);

    let auth_manager = state.auth_manager.clone();
    let device_auth = state.device_auth.clone();
    tokio::spawn(async move {
        let result = complete_device_code_login(opts, device_code).await;
        if let Err(err) = result {
            eprintln!("device auth failed: {err}");
        } else {
            auth_manager.reload();
        }
        let mut guard = device_auth.lock().await;
        guard.device_code = None;
    });

    Ok(response)
}

fn api_auth_mode_label(auth: &CodexAuth) -> &'static str {
    match auth {
        CodexAuth::ApiKey(_) => "api_key",
        CodexAuth::Chatgpt(_) => "chatgpt",
        CodexAuth::ChatgptAuthTokens(_) => "chatgpt_auth_tokens",
    }
}

#[derive(Clone, Debug)]
struct RuntimeModelStatus {
    model_provider_id: String,
    configured_model: Option<String>,
    effective_model: Option<String>,
}

async fn resolve_runtime_model(state: &ServiceState) -> RuntimeModelStatus {
    let fallback_provider_id = "openai".to_string();
    let fallback_provider = ModelProviderInfo::create_openai_provider(/* base_url */ None);
    let runtime_config = ConfigBuilder::default()
        .codex_home(state.codex_home.clone())
        .fallback_cwd(Some(state.codex_home.clone()))
        .build()
        .await;

    let (model_provider_id, configured_model, model_provider) = match runtime_config {
        Ok(config) => (
            config.model_provider_id,
            config.model,
            config.model_provider,
        ),
        Err(err) => {
            eprintln!(
                "failed to load runtime config from {}: {err}",
                state.codex_home.display()
            );
            (fallback_provider_id, None, fallback_provider)
        }
    };
    let models_manager = ModelsManager::new_with_provider(
        state.codex_home.clone(),
        state.auth_manager.clone(),
        None,
        CollaborationModesConfig::default(),
        model_provider,
    );
    let effective_model = models_manager
        .get_default_model(&configured_model, RefreshStrategy::Offline)
        .await;

    RuntimeModelStatus {
        model_provider_id,
        configured_model,
        effective_model: (!effective_model.is_empty()).then_some(effective_model),
    }
}

fn should_skip_response_header(name: &hyper::header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "content-length" | "transfer-encoding" | "connection" | "trailer" | "upgrade"
    )
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<BoxedBody> {
    match serde_json::to_vec(value) {
        Ok(data) => Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(boxed_body(Bytes::from(data)))
            .unwrap_or_else(|err| {
                text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
            }),
        Err(err) => text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

fn text_response(status: StatusCode, message: String) -> Response<BoxedBody> {
    let body = boxed_body(Bytes::from(message));
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
        .unwrap_or_else(|_| empty_response(StatusCode::INTERNAL_SERVER_ERROR))
}

fn empty_response(status: StatusCode) -> Response<BoxedBody> {
    Response::builder()
        .status(status)
        .body(boxed_body(Bytes::new()))
        .unwrap_or_else(|_| Response::new(boxed_body(Bytes::new())))
}

fn boxed_body(bytes: Bytes) -> BoxedBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

fn log_device_code_prompt(verification_url: &str, code: &str) {
    eprintln!("Device auth started. Visit {verification_url} and enter code {code} to sign in.");
}

fn build_upstream_url(base: &Url, uri: &hyper::Uri, strip_v1_prefix: bool) -> Result<Url> {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let base_str = base.as_str().trim_end_matches('/');
    let mut path = path_and_query.trim_start_matches('/');
    if (base_str.ends_with("/v1") || strip_v1_prefix) && path.starts_with("v1/") {
        path = path.trim_start_matches("v1/");
    }
    let joined = format!("{base_str}/{path}");
    Url::parse(&joined).context("failed to construct upstream url")
}

fn prepare_socket_path(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket dir {}", parent.display()))?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove existing socket {}", path.display()))
        }
    }
}

fn parse_socket_bind_target(path: &Path) -> Result<UnixSocketBindTarget> {
    let value = path.as_os_str().to_string_lossy();
    if let Some(name) = value.strip_prefix('@') {
        if name.is_empty() {
            anyhow::bail!("abstract unix socket name cannot be empty");
        }
        return Ok(UnixSocketBindTarget::Abstract(name.to_string()));
    }
    if let Some(name) = value.strip_prefix("abstract:") {
        if name.is_empty() {
            anyhow::bail!("abstract unix socket name cannot be empty");
        }
        return Ok(UnixSocketBindTarget::Abstract(name.to_string()));
    }
    Ok(UnixSocketBindTarget::Filesystem(path.to_path_buf()))
}

fn bind_listener(target: &UnixSocketBindTarget) -> Result<UnixListener> {
    match target {
        UnixSocketBindTarget::Filesystem(path) => {
            prepare_socket_path(path)?;
            UnixListener::bind(path)
                .with_context(|| format!("failed to bind socket at {}", path.display()))
        }
        UnixSocketBindTarget::Abstract(name) => bind_abstract_listener(name),
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn bind_abstract_listener(name: &str) -> Result<UnixListener> {
    use std::os::unix::net::UnixListener as StdUnixListener;

    let address = abstract_socket_addr(name.as_bytes())
        .with_context(|| format!("failed to create abstract socket address @{name}"))?;
    let listener = StdUnixListener::bind_addr(&address)
        .with_context(|| format!("failed to bind abstract socket @{name}"))?;
    listener
        .set_nonblocking(true)
        .with_context(|| format!("failed to set abstract socket @{name} nonblocking"))?;
    UnixListener::from_std(listener)
        .with_context(|| format!("failed to adopt abstract socket @{name} into tokio"))
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn abstract_socket_addr(name: &[u8]) -> std::io::Result<std::os::unix::net::SocketAddr> {
    use std::os::unix::net::SocketAddr;

    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;

    SocketAddr::from_abstract_name(name)
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn bind_abstract_listener(name: &str) -> Result<UnixListener> {
    anyhow::bail!("abstract unix sockets are unsupported on this platform: @{name}")
}

fn set_socket_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o666);
        if let Err(err) = std::fs::set_permissions(path, permissions) {
            eprintln!("failed to chmod {}: {err}", path.display());
        }
    }
}

enum AuthHeaderError {
    Missing,
    Invalid(anyhow::Error),
}

impl ServiceState {
    fn auth_header(
        &self,
        auth: Option<&CodexAuth>,
    ) -> std::result::Result<String, AuthHeaderError> {
        if let Some(value) = &self.auth_header_override {
            return Ok(value.clone());
        }
        let auth = auth.ok_or(AuthHeaderError::Missing)?;
        let token = auth
            .get_token()
            .map_err(|err| AuthHeaderError::Invalid(err.into()))?;
        Ok(format!("Bearer {token}"))
    }

    fn upstream_base(&self, auth: Option<&CodexAuth>) -> Url {
        self.upstream_base_override.clone().unwrap_or_else(|| {
            let base = default_upstream_base_url(auth);
            Url::parse(base).expect("hardcoded upstream base url must parse")
        })
    }
}

fn default_upstream_base_url(auth: Option<&CodexAuth>) -> &'static str {
    if auth.is_some_and(CodexAuth::is_chatgpt_auth) {
        "https://chatgpt.com/backend-api/codex"
    } else {
        "https://api.openai.com/v1"
    }
}

#[cfg(test)]
mod tests {
    use super::UnixSocketBindTarget;
    use super::build_upstream_url;
    use super::default_upstream_base_url;
    use super::parse_socket_bind_target;
    use codex_core::auth::CodexAuth;
    use hyper::Uri;
    use reqwest::Url;
    use std::path::Path;
    use std::path::PathBuf;

    #[test]
    fn chatgpt_auth_uses_chatgpt_backend() {
        let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
        assert_eq!(
            default_upstream_base_url(Some(&auth)),
            "https://chatgpt.com/backend-api/codex"
        );
    }

    #[test]
    fn missing_auth_uses_openai_backend() {
        assert_eq!(default_upstream_base_url(None), "https://api.openai.com/v1");
    }

    #[test]
    fn chatgpt_proxy_strips_v1_prefix() {
        let base = Url::parse("https://chatgpt.com/backend-api/codex").expect("url");
        let uri = "/v1/responses?foo=bar".parse::<Uri>().expect("uri");

        let upstream = build_upstream_url(&base, &uri, /*strip_v1_prefix*/ true).expect("url");

        assert_eq!(
            upstream.as_str(),
            "https://chatgpt.com/backend-api/codex/responses?foo=bar"
        );
    }

    #[test]
    fn openai_proxy_keeps_v1_prefix_once() {
        let base = Url::parse("https://api.openai.com/v1").expect("url");
        let uri = "/v1/responses".parse::<Uri>().expect("uri");

        let upstream = build_upstream_url(&base, &uri, /*strip_v1_prefix*/ false).expect("url");

        assert_eq!(upstream.as_str(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn parse_socket_bind_target_preserves_filesystem_paths() {
        assert_eq!(
            parse_socket_bind_target(Path::new("/tmp/codexd.sock")).expect("socket target"),
            UnixSocketBindTarget::Filesystem(PathBuf::from("/tmp/codexd.sock")),
        );
    }

    #[test]
    fn parse_socket_bind_target_supports_abstract_prefixes() {
        assert_eq!(
            parse_socket_bind_target(Path::new("@codex-agent")).expect("socket target"),
            UnixSocketBindTarget::Abstract("codex-agent".to_string()),
        );
        assert_eq!(
            parse_socket_bind_target(Path::new("abstract:codex-agent")).expect("socket target"),
            UnixSocketBindTarget::Abstract("codex-agent".to_string()),
        );
    }
}
