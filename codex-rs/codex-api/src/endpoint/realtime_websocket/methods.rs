use crate::endpoint::realtime_websocket::protocol::ConversationItem;
use crate::endpoint::realtime_websocket::protocol::ConversationItemContent;
use crate::endpoint::realtime_websocket::protocol::RealtimeAudioFrame;
use crate::endpoint::realtime_websocket::protocol::RealtimeEvent;
use crate::endpoint::realtime_websocket::protocol::RealtimeOutboundMessage;
use crate::endpoint::realtime_websocket::protocol::RealtimeSessionConfig;
use crate::endpoint::realtime_websocket::protocol::RealtimeToolAction;
use crate::endpoint::realtime_websocket::protocol::RealtimeTranscriptDelta;
use crate::endpoint::realtime_websocket::protocol::RealtimeTranscriptEntry;
use crate::endpoint::realtime_websocket::protocol::SessionAudio;
use crate::endpoint::realtime_websocket::protocol::SessionAudioFormat;
use crate::endpoint::realtime_websocket::protocol::SessionAudioInput;
use crate::endpoint::realtime_websocket::protocol::SessionAudioOutput;
use crate::endpoint::realtime_websocket::protocol::SessionAudioOutputFormat;
use crate::endpoint::realtime_websocket::protocol::SessionNoiseReduction;
use crate::endpoint::realtime_websocket::protocol::SessionTool;
use crate::endpoint::realtime_websocket::protocol::SessionToolParameters;
use crate::endpoint::realtime_websocket::protocol::SessionToolProperty;
use crate::endpoint::realtime_websocket::protocol::SessionTurnDetection;
use crate::endpoint::realtime_websocket::protocol::SessionUpdateSession;
use crate::endpoint::realtime_websocket::protocol::parse_realtime_event;
use crate::error::ApiError;
use crate::provider::Provider;
use codex_utils_rustls_provider::ensure_rustls_crypto_provider;
use futures::SinkExt;
use futures::StreamExt;
use http::HeaderMap;
use http::HeaderValue;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::trace;
use tungstenite::protocol::WebSocketConfig;
use url::Url;

const REALTIME_AUDIO_FORMAT: &str = "audio/pcm";
const REALTIME_SAMPLE_RATE_HZ: u32 = 24_000;
const REALTIME_NOISE_REDUCTION: &str = "near_field";
const REALTIME_TURN_DETECTION: &str = "server_vad";
const REALTIME_INTERRUPT_RESPONSE: bool = true;
const REALTIME_CREATE_RESPONSE: bool = true;
const REALTIME_VOICE: &str = "marin";

struct WsStream {
    tx_command: mpsc::Sender<WsCommand>,
    pump_task: tokio::task::JoinHandle<()>,
}

enum WsCommand {
    Send {
        message: Message,
        tx_result: oneshot::Sender<Result<(), WsError>>,
    },
    Close {
        tx_result: oneshot::Sender<Result<(), WsError>>,
    },
}

impl WsStream {
    fn new(
        inner: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> (Self, mpsc::UnboundedReceiver<Result<Message, WsError>>) {
        let (tx_command, mut rx_command) = mpsc::channel::<WsCommand>(32);
        let (tx_message, rx_message) = mpsc::unbounded_channel::<Result<Message, WsError>>();

        let pump_task = tokio::spawn(async move {
            let mut inner = inner;
            loop {
                tokio::select! {
                    command = rx_command.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        match command {
                            WsCommand::Send { message, tx_result } => {
                                debug!("realtime websocket sending message");
                                let result = inner.send(message).await;
                                let should_break = result.is_err();
                                if let Err(err) = &result {
                                    error!("realtime websocket send failed: {err}");
                                }
                                let _ = tx_result.send(result);
                                if should_break {
                                    break;
                                }
                            }
                            WsCommand::Close { tx_result } => {
                                info!("realtime websocket sending close");
                                let result = inner.close(None).await;
                                if let Err(err) = &result {
                                    error!("realtime websocket close failed: {err}");
                                }
                                let _ = tx_result.send(result);
                                break;
                            }
                        }
                    }
                    message = inner.next() => {
                        let Some(message) = message else {
                            break;
                        };
                        match message {
                            Ok(Message::Ping(payload)) => {
                                trace!(payload_len = payload.len(), "realtime websocket received ping");
                                if let Err(err) = inner.send(Message::Pong(payload)).await {
                                    error!("realtime websocket failed to send pong: {err}");
                                    let _ = tx_message.send(Err(err));
                                    break;
                                }
                            }
                            Ok(Message::Pong(_)) => {}
                            Ok(message @ (Message::Text(_)
                                | Message::Binary(_)
                                | Message::Close(_)
                                | Message::Frame(_))) => {
                                let is_close = matches!(message, Message::Close(_));
                                match &message {
                                    Message::Text(_) => trace!("realtime websocket received text frame"),
                                    Message::Binary(binary) => {
                                        error!(
                                            payload_len = binary.len(),
                                            "realtime websocket received unexpected binary frame"
                                        );
                                    }
                                    Message::Close(frame) => info!(
                                        "realtime websocket received close frame: code={:?} reason={:?}",
                                        frame.as_ref().map(|frame| frame.code),
                                        frame.as_ref().map(|frame| frame.reason.as_str())
                                    ),
                                    Message::Frame(_) => {
                                        trace!("realtime websocket received raw frame");
                                    }
                                    Message::Ping(_) | Message::Pong(_) => {}
                                }
                                if tx_message.send(Ok(message)).is_err() {
                                    break;
                                }
                                if is_close {
                                    break;
                                }
                            }
                            Err(err) => {
                                error!("realtime websocket receive failed: {err}");
                                let _ = tx_message.send(Err(err));
                                break;
                            }
                        }
                    }
                }
            }
            info!("realtime websocket pump exiting");
        });

        (
            Self {
                tx_command,
                pump_task,
            },
            rx_message,
        )
    }

    async fn request(
        &self,
        make_command: impl FnOnce(oneshot::Sender<Result<(), WsError>>) -> WsCommand,
    ) -> Result<(), WsError> {
        let (tx_result, rx_result) = oneshot::channel();
        if self.tx_command.send(make_command(tx_result)).await.is_err() {
            return Err(WsError::ConnectionClosed);
        }
        rx_result.await.unwrap_or(Err(WsError::ConnectionClosed))
    }

    async fn send(&self, message: Message) -> Result<(), WsError> {
        self.request(|tx_result| WsCommand::Send { message, tx_result })
            .await
    }

    async fn close(&self) -> Result<(), WsError> {
        self.request(|tx_result| WsCommand::Close { tx_result })
            .await
    }
}

impl Drop for WsStream {
    fn drop(&mut self) {
        self.pump_task.abort();
    }
}

pub struct RealtimeWebsocketConnection {
    writer: RealtimeWebsocketWriter,
    events: RealtimeWebsocketEvents,
}

#[derive(Clone)]
pub struct RealtimeWebsocketWriter {
    stream: Arc<WsStream>,
    is_closed: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct RealtimeWebsocketEvents {
    rx_message: Arc<Mutex<mpsc::UnboundedReceiver<Result<Message, WsError>>>>,
    active_transcript: Arc<Mutex<ActiveTranscriptState>>,
    is_closed: Arc<AtomicBool>,
}

#[derive(Default)]
struct ActiveTranscriptState {
    entries: Vec<RealtimeTranscriptEntry>,
}

impl RealtimeWebsocketConnection {
    pub async fn send_audio_frame(&self, frame: RealtimeAudioFrame) -> Result<(), ApiError> {
        self.writer.send_audio_frame(frame).await
    }

    pub async fn send_conversation_item_create(&self, text: String) -> Result<(), ApiError> {
        self.writer.send_conversation_item_create(text).await
    }

    pub async fn send_conversation_handoff_append(
        &self,
        handoff_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        self.writer
            .send_conversation_handoff_append(handoff_id, output_text)
            .await
    }

    pub async fn send_function_call_output(
        &self,
        call_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        self.writer
            .send_function_call_output(call_id, output_text)
            .await
    }

    pub async fn send_response_create(&self) -> Result<(), ApiError> {
        self.writer.send_response_create().await
    }

    pub async fn send_conversation_item_truncate(
        &self,
        item_id: String,
        content_index: u32,
        audio_end_ms: u32,
    ) -> Result<(), ApiError> {
        self.writer
            .send_conversation_item_truncate(item_id, content_index, audio_end_ms)
            .await
    }

    pub async fn close(&self) -> Result<(), ApiError> {
        self.writer.close().await
    }

    pub async fn next_event(&self) -> Result<Option<RealtimeEvent>, ApiError> {
        self.events.next_event().await
    }

    pub fn writer(&self) -> RealtimeWebsocketWriter {
        self.writer.clone()
    }

    pub fn events(&self) -> RealtimeWebsocketEvents {
        self.events.clone()
    }

    fn new(
        stream: WsStream,
        rx_message: mpsc::UnboundedReceiver<Result<Message, WsError>>,
    ) -> Self {
        let stream = Arc::new(stream);
        let is_closed = Arc::new(AtomicBool::new(false));
        Self {
            writer: RealtimeWebsocketWriter {
                stream: Arc::clone(&stream),
                is_closed: Arc::clone(&is_closed),
            },
            events: RealtimeWebsocketEvents {
                rx_message: Arc::new(Mutex::new(rx_message)),
                active_transcript: Arc::new(Mutex::new(ActiveTranscriptState::default())),
                is_closed,
            },
        }
    }
}

impl RealtimeWebsocketWriter {
    pub async fn send_audio_frame(&self, frame: RealtimeAudioFrame) -> Result<(), ApiError> {
        self.send_json(RealtimeOutboundMessage::InputAudioBufferAppend { audio: frame.data })
            .await
    }

    pub async fn send_conversation_item_create(&self, text: String) -> Result<(), ApiError> {
        self.send_json(RealtimeOutboundMessage::ConversationItemCreate {
            item: ConversationItem::Message {
                role: "user".to_string(),
                content: vec![ConversationItemContent {
                    kind: "input_text".to_string(),
                    text,
                }],
            },
        })
        .await
    }

    pub async fn send_conversation_handoff_append(
        &self,
        _handoff_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        self.send_json(RealtimeOutboundMessage::ConversationItemCreate {
            item: ConversationItem::Message {
                role: "assistant".to_string(),
                content: vec![ConversationItemContent {
                    kind: "output_text".to_string(),
                    text: output_text,
                }],
            },
        })
        .await
    }

    pub async fn send_function_call_output(
        &self,
        call_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        let output = json!({
            "content": output_text,
        })
        .to_string();
        self.send_json(RealtimeOutboundMessage::ConversationItemCreate {
            item: ConversationItem::FunctionCallOutput { call_id, output },
        })
        .await
    }

    pub async fn send_response_create(&self) -> Result<(), ApiError> {
        self.send_json(RealtimeOutboundMessage::ResponseCreate)
            .await
    }

    pub async fn send_conversation_item_truncate(
        &self,
        item_id: String,
        content_index: u32,
        audio_end_ms: u32,
    ) -> Result<(), ApiError> {
        self.send_json(RealtimeOutboundMessage::ConversationItemTruncate {
            item_id,
            content_index,
            audio_end_ms,
        })
        .await
    }

    pub async fn send_session_update(&self, instructions: String) -> Result<(), ApiError> {
        self.send_json(RealtimeOutboundMessage::SessionUpdate {
            session: Box::new(SessionUpdateSession {
                kind: "realtime".to_string(),
                instructions,
                output_modalities: vec!["audio".to_string()],
                audio: SessionAudio {
                    input: SessionAudioInput {
                        format: SessionAudioFormat {
                            kind: REALTIME_AUDIO_FORMAT.to_string(),
                            rate: REALTIME_SAMPLE_RATE_HZ,
                        },
                        noise_reduction: SessionNoiseReduction {
                            kind: REALTIME_NOISE_REDUCTION.to_string(),
                        },
                        turn_detection: SessionTurnDetection {
                            kind: REALTIME_TURN_DETECTION.to_string(),
                            interrupt_response: REALTIME_INTERRUPT_RESPONSE,
                            create_response: REALTIME_CREATE_RESPONSE,
                        },
                    },
                    output: SessionAudioOutput {
                        format: SessionAudioOutputFormat {
                            kind: REALTIME_AUDIO_FORMAT.to_string(),
                            rate: REALTIME_SAMPLE_RATE_HZ,
                        },
                        voice: REALTIME_VOICE.to_string(),
                    },
                },
                tools: vec![
                    SessionTool {
                        kind: "function".to_string(),
                        name: "codex".to_string(),
                        description:
                            "Delegate a request to Codex and return the final result to the user. Use this as the default action. If the user asks to do something next, later, after this, or once current work finishes, call this tool so the work is actually queued instead of merely promising to do it later."
                                .to_string(),
                        parameters: SessionToolParameters {
                            kind: "object".to_string(),
                            properties: BTreeMap::from([
                                (
                                    "prompt".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "The user request to delegate to Codex."
                                            .to_string(),
                                    },
                                ),
                                (
                                    "send_immediately".to_string(),
                                    SessionToolProperty {
                                        kind: "boolean".to_string(),
                                        description: "When true, send this to Codex immediately, steering any running turn. When false or omitted, queue it for the next turn.".to_string(),
                                    },
                                ),
                            ]),
                            required: vec!["prompt".to_string()],
                        },
                    },
                    SessionTool {
                        kind: "function".to_string(),
                        name: "cancel_current_operation".to_string(),
                        description: "Cancel the current Codex operation, equivalent to pressing Ctrl-C without exiting Codex.".to_string(),
                        parameters: SessionToolParameters {
                            kind: "object".to_string(),
                            properties: BTreeMap::new(),
                            required: Vec::new(),
                        },
                    },
                    SessionTool {
                        kind: "function".to_string(),
                        name: "turn_off_realtime_mode".to_string(),
                        description: "Turn off realtime voice mode, equivalent to using /realtime to stop live voice.".to_string(),
                        parameters: SessionToolParameters {
                            kind: "object".to_string(),
                            properties: BTreeMap::new(),
                            required: Vec::new(),
                        },
                    },
                    SessionTool {
                        kind: "function".to_string(),
                        name: "manage_message_queue".to_string(),
                        description: "Inspect or edit queued draft messages. Prefer this over codex when the user asks what is queued or wants to replace, remove, or clear queued draft work. Supported actions: list, replace_last, remove_last, clear. Queue editing affects queued draft messages only, not pending steers.".to_string(),
                        parameters: SessionToolParameters {
                            kind: "object".to_string(),
                            properties: BTreeMap::from([
                                (
                                    "action".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Queue action to run. Use one of: list, replace_last, remove_last, clear.".to_string(),
                                    },
                                ),
                                (
                                    "message".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Replacement text for action=replace_last.".to_string(),
                                    },
                                ),
                            ]),
                            required: vec!["action".to_string()],
                        },
                    },
                    SessionTool {
                        kind: "function".to_string(),
                        name: "manage_runtime_settings".to_string(),
                        description: "Inspect or update runtime settings for future Codex turns, and inspect quick local context like the current working_directory and git_branch. Prefer this over codex when the user wants to inspect or change model, working_directory, reasoning_effort, fast_mode, personality, or collaboration_mode, or asks quick local questions like which branch Codex is on. Call with no setting fields to list current settings, current local context, possible settings, and allowed values. Supported writable setting keys: model, working_directory, reasoning_effort, fast_mode, personality, collaboration_mode. git_branch is read-only context. Changes are not persisted to disk.".to_string(),
                        parameters: SessionToolParameters {
                            kind: "object".to_string(),
                            properties: BTreeMap::from([
                                (
                                    "model".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Optional model slug to use for future Codex turns.".to_string(),
                                    },
                                ),
                                (
                                    "working_directory".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Optional working directory for future Codex turns. Relative paths are resolved against the current working directory.".to_string(),
                                    },
                                ),
                                (
                                    "reasoning_effort".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Optional reasoning effort. Supported values: default, minimal, low, medium, high, xhigh.".to_string(),
                                    },
                                ),
                                (
                                    "fast_mode".to_string(),
                                    SessionToolProperty {
                                        kind: "boolean".to_string(),
                                        description: "Optional Fast mode toggle for future Codex turns.".to_string(),
                                    },
                                ),
                                (
                                    "personality".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Optional personality. Supported values: none, friendly, pragmatic.".to_string(),
                                    },
                                ),
                                (
                                    "collaboration_mode".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Optional collaboration mode. Supported values: default, plan.".to_string(),
                                    },
                                ),
                            ]),
                            required: Vec::new(),
                        },
                    },
                    SessionTool {
                        kind: "function".to_string(),
                        name: "run_tui_command".to_string(),
                        description: "Run a small set of built-in TUI actions. Prefer this over codex for built-in control actions like compact, review, plan, diff, or opening the agent picker. Supported commands: compact, review, plan, diff, agent. review can take an optional prompt for custom review instructions. plan can take an optional prompt to switch to Plan mode and submit work immediately.".to_string(),
                        parameters: SessionToolParameters {
                            kind: "object".to_string(),
                            properties: BTreeMap::from([
                                (
                                    "command".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Built-in TUI command to run. Use one of: compact, review, plan, diff, agent.".to_string(),
                                    },
                                ),
                                (
                                    "prompt".to_string(),
                                    SessionToolProperty {
                                        kind: "string".to_string(),
                                        description: "Optional text argument for command=review or command=plan.".to_string(),
                                    },
                                ),
                            ]),
                            required: vec!["command".to_string()],
                        },
                    },
                ],
                tool_choice: "auto".to_string(),
            }),
        })
        .await
    }

    pub async fn close(&self) -> Result<(), ApiError> {
        if self.is_closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        if let Err(err) = self.stream.close().await
            && !matches!(err, WsError::ConnectionClosed | WsError::AlreadyClosed)
        {
            return Err(ApiError::Stream(format!(
                "failed to close websocket: {err}"
            )));
        }
        Ok(())
    }

    async fn send_json(&self, message: RealtimeOutboundMessage) -> Result<(), ApiError> {
        let payload = serde_json::to_string(&message)
            .map_err(|err| ApiError::Stream(format!("failed to encode realtime request: {err}")))?;
        debug!(?message, "realtime websocket request");

        if self.is_closed.load(Ordering::SeqCst) {
            return Err(ApiError::Stream(
                "realtime websocket connection is closed".to_string(),
            ));
        }

        self.stream
            .send(Message::Text(payload.into()))
            .await
            .map_err(|err| ApiError::Stream(format!("failed to send realtime request: {err}")))?;
        Ok(())
    }
}

impl RealtimeWebsocketEvents {
    pub async fn next_event(&self) -> Result<Option<RealtimeEvent>, ApiError> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Ok(None);
        }

        loop {
            let msg = match self.rx_message.lock().await.recv().await {
                Some(Ok(msg)) => msg,
                Some(Err(err)) => {
                    self.is_closed.store(true, Ordering::SeqCst);
                    error!("realtime websocket read failed: {err}");
                    return Err(ApiError::Stream(format!(
                        "failed to read websocket message: {err}"
                    )));
                }
                None => {
                    self.is_closed.store(true, Ordering::SeqCst);
                    info!("realtime websocket event stream ended");
                    return Ok(None);
                }
            };

            match msg {
                Message::Text(text) => {
                    if let Some(mut event) = parse_realtime_event(&text) {
                        self.update_active_transcript(&mut event).await;
                        log_realtime_event(&event);
                        debug!(?event, "realtime websocket parsed event");
                        return Ok(Some(event));
                    }
                    debug!("realtime websocket ignored unsupported text frame");
                }
                Message::Close(frame) => {
                    self.is_closed.store(true, Ordering::SeqCst);
                    info!(
                        "realtime websocket closed: code={:?} reason={:?}",
                        frame.as_ref().map(|frame| frame.code),
                        frame.as_ref().map(|frame| frame.reason.as_str())
                    );
                    return Ok(None);
                }
                Message::Binary(_) => {
                    return Ok(Some(RealtimeEvent::Error(
                        "unexpected binary realtime websocket event".to_string(),
                    )));
                }
                Message::Frame(_) | Message::Ping(_) | Message::Pong(_) => {}
            }
        }
    }

    async fn update_active_transcript(&self, event: &mut RealtimeEvent) {
        let mut active_transcript = self.active_transcript.lock().await;
        match event {
            RealtimeEvent::InputTranscriptDelta(RealtimeTranscriptDelta { delta }) => {
                append_transcript_delta(&mut active_transcript.entries, "user", delta);
            }
            RealtimeEvent::OutputTranscriptDelta(RealtimeTranscriptDelta { delta }) => {
                append_transcript_delta(&mut active_transcript.entries, "assistant", delta);
            }
            RealtimeEvent::HandoffRequested(handoff) => {
                handoff.active_transcript = std::mem::take(&mut active_transcript.entries);
            }
            RealtimeEvent::SessionUpdated { .. }
            | RealtimeEvent::InterruptRequested(_)
            | RealtimeEvent::CloseRequested(_)
            | RealtimeEvent::AudioOut(_)
            | RealtimeEvent::InputAudioSpeechStarted(_)
            | RealtimeEvent::ResponseCancelled(_)
            | RealtimeEvent::ConversationItemAdded(_)
            | RealtimeEvent::ConversationItemDone { .. }
            | RealtimeEvent::ToolActionRequested(_)
            | RealtimeEvent::Error(_) => {}
        }
    }
}

fn append_transcript_delta(entries: &mut Vec<RealtimeTranscriptEntry>, role: &str, delta: &str) {
    if delta.is_empty() {
        return;
    }

    if let Some(last_entry) = entries.last_mut()
        && last_entry.role == role
    {
        last_entry.text.push_str(delta);
        return;
    }

    entries.push(RealtimeTranscriptEntry {
        role: role.to_string(),
        text: delta.to_string(),
    });
}

pub struct RealtimeWebsocketClient {
    provider: Provider,
}

impl RealtimeWebsocketClient {
    pub fn new(provider: Provider) -> Self {
        Self { provider }
    }

    pub async fn connect(
        &self,
        config: RealtimeSessionConfig,
        extra_headers: HeaderMap,
        default_headers: HeaderMap,
    ) -> Result<RealtimeWebsocketConnection, ApiError> {
        ensure_rustls_crypto_provider();
        let ws_url = websocket_url_from_api_url(
            self.provider.base_url.as_str(),
            self.provider.query_params.as_ref(),
            config.model.as_deref(),
        )?;

        let mut request = ws_url
            .as_str()
            .into_client_request()
            .map_err(|err| ApiError::Stream(format!("failed to build websocket request: {err}")))?;
        let headers = merge_request_headers(
            &self.provider.headers,
            with_session_id_header(extra_headers, config.session_id.as_deref())?,
            default_headers,
        );
        request.headers_mut().extend(headers);

        info!("connecting realtime websocket: {ws_url}");
        let (stream, response) =
            tokio_tungstenite::connect_async_with_config(request, Some(websocket_config()), false)
                .await
                .map_err(|err| {
                    ApiError::Stream(format!("failed to connect realtime websocket: {err}"))
                })?;
        info!(
            ws_url = %ws_url,
            status = %response.status(),
            "realtime websocket connected"
        );

        let (stream, rx_message) = WsStream::new(stream);
        let connection = RealtimeWebsocketConnection::new(stream, rx_message);
        info!(
            requested_session_id = config.session_id.as_deref().unwrap_or("<none>"),
            model = config.model.as_deref().unwrap_or("<provider-default>"),
            input_audio_format = REALTIME_AUDIO_FORMAT,
            input_sample_rate_hz = REALTIME_SAMPLE_RATE_HZ,
            output_audio_format = REALTIME_AUDIO_FORMAT,
            output_sample_rate_hz = REALTIME_SAMPLE_RATE_HZ,
            voice = REALTIME_VOICE,
            turn_detection = REALTIME_TURN_DETECTION,
            interrupt_response = REALTIME_INTERRUPT_RESPONSE,
            create_response = REALTIME_CREATE_RESPONSE,
            noise_reduction = REALTIME_NOISE_REDUCTION,
            "sending realtime session.update"
        );
        connection
            .writer
            .send_session_update(config.instructions)
            .await?;
        Ok(connection)
    }
}

fn merge_request_headers(
    provider_headers: &HeaderMap,
    extra_headers: HeaderMap,
    default_headers: HeaderMap,
) -> HeaderMap {
    let mut headers = provider_headers.clone();
    headers.extend(extra_headers);
    for (name, value) in &default_headers {
        if let http::header::Entry::Vacant(entry) = headers.entry(name) {
            entry.insert(value.clone());
        }
    }
    headers
}

fn log_realtime_event(event: &RealtimeEvent) {
    match event {
        RealtimeEvent::SessionUpdated { session_id, .. } => {
            info!(session_id = %session_id, "realtime session updated");
        }
        RealtimeEvent::HandoffRequested(handoff) => {
            info!(
                function_name = "codex",
                call_id = %handoff.handoff_id,
                arguments = %json!({
                    "prompt": handoff.input_transcript,
                    "send_immediately": handoff.send_immediately,
                }),
                "realtime function call requested"
            );
        }
        RealtimeEvent::InterruptRequested(interrupt) => {
            info!(
                function_name = "cancel_current_operation",
                call_id = %interrupt.call_id,
                arguments = %json!({}),
                "realtime function call requested"
            );
        }
        RealtimeEvent::CloseRequested(close) => {
            info!(
                function_name = "turn_off_realtime_mode",
                call_id = %close.call_id,
                arguments = %json!({}),
                "realtime function call requested"
            );
        }
        RealtimeEvent::ToolActionRequested(request) => {
            let (function_name, arguments) = normalized_realtime_tool_call(&request.action);
            info!(
                function_name,
                call_id = %request.call_id,
                arguments = %arguments,
                "realtime function call requested"
            );
        }
        _ => {}
    }
}

fn normalized_realtime_tool_call(action: &RealtimeToolAction) -> (&'static str, serde_json::Value) {
    match action {
        RealtimeToolAction::ManageMessageQueue { action, message } => (
            "manage_message_queue",
            json!({
                "action": action,
                "message": message,
            }),
        ),
        RealtimeToolAction::ListMessageQueue => (
            "manage_message_queue",
            json!({
                "action": "list",
            }),
        ),
        RealtimeToolAction::ReplaceLastQueuedMessage { message } => (
            "manage_message_queue",
            json!({
                "action": "replace_last",
                "message": message,
            }),
        ),
        RealtimeToolAction::RemoveLastQueuedMessage => (
            "manage_message_queue",
            json!({
                "action": "remove_last",
            }),
        ),
        RealtimeToolAction::ClearQueuedMessages => (
            "manage_message_queue",
            json!({
                "action": "clear",
            }),
        ),
        RealtimeToolAction::ManageRuntimeSettings {
            model,
            working_directory,
            reasoning_effort,
            fast_mode,
            personality,
            collaboration_mode,
        }
        | RealtimeToolAction::UpdateRuntimeSettings {
            model,
            working_directory,
            reasoning_effort,
            fast_mode,
            personality,
            collaboration_mode,
        } => (
            "manage_runtime_settings",
            json!({
                "model": model,
                "working_directory": working_directory,
                "reasoning_effort": reasoning_effort,
                "fast_mode": fast_mode,
                "personality": personality,
                "collaboration_mode": collaboration_mode,
            }),
        ),
        RealtimeToolAction::ListRuntimeSettings => ("manage_runtime_settings", json!({})),
        RealtimeToolAction::RunTuiCommand { command, prompt } => (
            "run_tui_command",
            json!({
                "command": command,
                "prompt": prompt,
            }),
        ),
        RealtimeToolAction::CompactConversation => (
            "run_tui_command",
            json!({
                "command": "compact",
            }),
        ),
    }
}

fn with_session_id_header(
    mut headers: HeaderMap,
    session_id: Option<&str>,
) -> Result<HeaderMap, ApiError> {
    let Some(session_id) = session_id else {
        return Ok(headers);
    };
    headers.insert(
        "x-session-id",
        HeaderValue::from_str(session_id).map_err(|err| {
            ApiError::Stream(format!("invalid realtime session id header: {err}"))
        })?,
    );
    Ok(headers)
}

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
}

fn websocket_url_from_api_url(
    api_url: &str,
    query_params: Option<&HashMap<String, String>>,
    model: Option<&str>,
) -> Result<Url, ApiError> {
    let mut url = Url::parse(api_url)
        .map_err(|err| ApiError::Stream(format!("failed to parse realtime api_url: {err}")))?;

    normalize_realtime_path(&mut url);

    match url.scheme() {
        "ws" | "wss" => {}
        "http" | "https" => {
            let scheme = if url.scheme() == "http" { "ws" } else { "wss" };
            let _ = url.set_scheme(scheme);
        }
        scheme => {
            return Err(ApiError::Stream(format!(
                "unsupported realtime api_url scheme: {scheme}"
            )));
        }
    }

    let has_additional_query_params = query_params
        .is_some_and(|params| params.keys().any(|key| key != "model" || model.is_none()));
    if model.is_some() || has_additional_query_params {
        let mut query = url.query_pairs_mut();
        if let Some(model) = model {
            query.append_pair("model", model);
        }
        if let Some(query_params) = query_params {
            for (key, value) in query_params {
                if key == "model" && model.is_some() {
                    continue;
                }
                query.append_pair(key, value);
            }
        }
    }

    Ok(url)
}

fn normalize_realtime_path(url: &mut Url) {
    let path = url.path().to_string();
    if path.is_empty() || path == "/" {
        url.set_path("/v1/realtime");
        return;
    }

    if path.ends_with("/realtime") {
        return;
    }

    if path.ends_with("/realtime/") {
        url.set_path(path.trim_end_matches('/'));
        return;
    }

    if path.ends_with("/v1") {
        url.set_path(&format!("{path}/realtime"));
        return;
    }

    if path.ends_with("/v1/") {
        url.set_path(&format!("{path}realtime"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::realtime_websocket::protocol::RealtimeCloseRequested;
    use crate::endpoint::realtime_websocket::protocol::RealtimeHandoffRequested;
    use crate::endpoint::realtime_websocket::protocol::RealtimeInputAudioSpeechStarted;
    use crate::endpoint::realtime_websocket::protocol::RealtimeInterruptRequested;
    use crate::endpoint::realtime_websocket::protocol::RealtimeOutputAudioDelta;
    use crate::endpoint::realtime_websocket::protocol::RealtimeResponseCancelled;
    use crate::endpoint::realtime_websocket::protocol::RealtimeToolAction;
    use crate::endpoint::realtime_websocket::protocol::RealtimeToolActionRequested;
    use crate::endpoint::realtime_websocket::protocol::RealtimeTranscriptDelta;
    use crate::endpoint::realtime_websocket::protocol::RealtimeTranscriptEntry;
    use http::HeaderValue;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn parse_session_updated_event() {
        let payload = json!({
            "type": "session.updated",
            "session": {"id": "sess_123", "instructions": "backend prompt"}
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::SessionUpdated {
                session_id: "sess_123".to_string(),
                instructions: Some("backend prompt".to_string()),
            })
        );
    }

    #[test]
    fn parse_audio_delta_event() {
        let payload = json!({
            "type": "response.output_audio.delta",
            "delta": "AAA=",
            "sample_rate": 48000,
            "channels": 1,
            "samples_per_channel": 960
        })
        .to_string();
        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::AudioOut(RealtimeOutputAudioDelta {
                frame: RealtimeAudioFrame {
                    data: "AAA=".to_string(),
                    sample_rate: 48000,
                    num_channels: 1,
                    samples_per_channel: Some(960),
                },
                item_id: None,
            }))
        );
    }

    #[test]
    fn parse_audio_delta_event_defaults_audio_shape() {
        let payload = json!({
            "type": "response.output_audio.delta",
            "delta": "AAA="
        })
        .to_string();
        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::AudioOut(RealtimeOutputAudioDelta {
                frame: RealtimeAudioFrame {
                    data: "AAA=".to_string(),
                    sample_rate: 24_000,
                    num_channels: 1,
                    samples_per_channel: None,
                },
                item_id: None,
            }))
        );
    }

    #[test]
    fn parse_audio_delta_event_with_item_id() {
        let payload = json!({
            "type": "response.audio.delta",
            "delta": "AAA=",
            "item_id": "item_audio_1"
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::AudioOut(RealtimeOutputAudioDelta {
                frame: RealtimeAudioFrame {
                    data: "AAA=".to_string(),
                    sample_rate: 24_000,
                    num_channels: 1,
                    samples_per_channel: None,
                },
                item_id: Some("item_audio_1".to_string()),
            }))
        );
    }

    #[test]
    fn parse_conversation_item_added_event() {
        let payload = json!({
            "type": "conversation.item.added",
            "item": {"type": "message", "seq": 7}
        })
        .to_string();
        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ConversationItemAdded(
                json!({"type": "message", "seq": 7})
            ))
        );
    }

    #[test]
    fn parse_conversation_item_done_event() {
        let payload = json!({
            "type": "conversation.item.done",
            "item": {"id": "item_123", "type": "message"}
        })
        .to_string();
        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ConversationItemDone {
                item_id: "item_123".to_string(),
            })
        );
    }

    #[test]
    fn parse_handoff_requested_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_123",
                        "type": "function_call",
                        "name": "codex",
                        "call_id": "handoff_123",
                        "arguments": "{\"prompt\":\"delegate this\"}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::HandoffRequested(RealtimeHandoffRequested {
                handoff_id: "handoff_123".to_string(),
                item_id: "item_123".to_string(),
                input_transcript: "delegate this".to_string(),
                send_immediately: false,
                active_transcript: Vec::new(),
            }))
        );
    }

    #[test]
    fn parse_handoff_requested_event_with_send_immediately() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_456",
                        "type": "function_call",
                        "name": "codex",
                        "call_id": "handoff_456",
                        "arguments": "{\"prompt\":\"delegate this now\",\"send_immediately\":true}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::HandoffRequested(RealtimeHandoffRequested {
                handoff_id: "handoff_456".to_string(),
                item_id: "item_456".to_string(),
                input_transcript: "delegate this now".to_string(),
                send_immediately: true,
                active_transcript: Vec::new(),
            }))
        );
    }

    #[test]
    fn parse_interrupt_requested_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_cancel",
                        "type": "function_call",
                        "name": "cancel_current_operation",
                        "call_id": "cancel_123",
                        "arguments": "{}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::InterruptRequested(
                RealtimeInterruptRequested {
                    call_id: "cancel_123".to_string(),
                }
            ))
        );
    }

    #[test]
    fn parse_close_requested_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_close",
                        "type": "function_call",
                        "name": "turn_off_realtime_mode",
                        "call_id": "close_123",
                        "arguments": "{}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::CloseRequested(RealtimeCloseRequested {
                call_id: "close_123".to_string(),
            }))
        );
    }

    #[test]
    fn parse_manage_message_queue_requested_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_queue",
                        "type": "function_call",
                        "name": "manage_message_queue",
                        "call_id": "queue_123",
                        "arguments": "{\"action\":\"list\"}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ToolActionRequested(
                RealtimeToolActionRequested {
                    call_id: "queue_123".to_string(),
                    action: RealtimeToolAction::ManageMessageQueue {
                        action: "list".to_string(),
                        message: None,
                    },
                }
            ))
        );
    }

    #[test]
    fn parse_manage_runtime_settings_requested_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_settings",
                        "type": "function_call",
                        "name": "manage_runtime_settings",
                        "call_id": "settings_123",
                        "arguments": "{\"model\":\"gpt-5\",\"working_directory\":\"src\",\"reasoning_effort\":\"low\"}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ToolActionRequested(
                RealtimeToolActionRequested {
                    call_id: "settings_123".to_string(),
                    action: RealtimeToolAction::ManageRuntimeSettings {
                        model: Some("gpt-5".to_string()),
                        working_directory: Some("src".to_string()),
                        reasoning_effort: Some("low".to_string()),
                        fast_mode: None,
                        personality: None,
                        collaboration_mode: None,
                    },
                }
            ))
        );
    }

    #[test]
    fn parse_run_tui_command_requested_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "output": [
                    {
                        "id": "item_command",
                        "type": "function_call",
                        "name": "run_tui_command",
                        "call_id": "command_123",
                        "arguments": "{\"command\":\"plan\",\"prompt\":\"make a plan\"}"
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ToolActionRequested(
                RealtimeToolActionRequested {
                    call_id: "command_123".to_string(),
                    action: RealtimeToolAction::RunTuiCommand {
                        command: "plan".to_string(),
                        prompt: Some("make a plan".to_string()),
                    },
                }
            ))
        );
    }

    #[test]
    fn parse_input_audio_speech_started_event() {
        let payload = json!({
            "type": "input_audio_buffer.speech_started",
            "item_id": "item_user_1"
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::InputAudioSpeechStarted(
                RealtimeInputAudioSpeechStarted {
                    item_id: Some("item_user_1".to_string()),
                }
            ))
        );
    }

    #[test]
    fn parse_cancelled_response_done_event() {
        let payload = json!({
            "type": "response.done",
            "response": {
                "id": "resp_cancelled",
                "status": "cancelled",
                "output": []
            }
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ResponseCancelled(
                RealtimeResponseCancelled {
                    response_id: Some("resp_cancelled".to_string()),
                }
            ))
        );
    }

    #[test]
    fn parse_unknown_event_as_conversation_item_added() {
        let payload = json!({
            "type": "response.output_text.delta",
            "delta": "hello",
            "response_id": "resp_1"
        })
        .to_string();
        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::ConversationItemAdded(json!({
                "type": "response.output_text.delta",
                "delta": "hello",
                "response_id": "resp_1"
            })))
        );
    }

    #[test]
    fn parse_input_transcript_delta_event() {
        let payload = json!({
            "type": "conversation.input_transcript.delta",
            "delta": "hello "
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::InputTranscriptDelta(
                RealtimeTranscriptDelta {
                    delta: "hello ".to_string(),
                }
            ))
        );
    }

    #[test]
    fn parse_output_transcript_delta_event() {
        let payload = json!({
            "type": "conversation.output_transcript.delta",
            "delta": "hi"
        })
        .to_string();

        assert_eq!(
            parse_realtime_event(payload.as_str()),
            Some(RealtimeEvent::OutputTranscriptDelta(
                RealtimeTranscriptDelta {
                    delta: "hi".to_string(),
                }
            ))
        );
    }

    #[test]
    fn merge_request_headers_matches_http_precedence() {
        let mut provider_headers = HeaderMap::new();
        provider_headers.insert(
            "originator",
            HeaderValue::from_static("provider-originator"),
        );
        provider_headers.insert("x-priority", HeaderValue::from_static("provider"));

        let mut extra_headers = HeaderMap::new();
        extra_headers.insert("x-priority", HeaderValue::from_static("extra"));

        let mut default_headers = HeaderMap::new();
        default_headers.insert("originator", HeaderValue::from_static("default-originator"));
        default_headers.insert("x-priority", HeaderValue::from_static("default"));
        default_headers.insert("x-default-only", HeaderValue::from_static("default-only"));

        let merged = merge_request_headers(&provider_headers, extra_headers, default_headers);

        assert_eq!(
            merged.get("originator"),
            Some(&HeaderValue::from_static("provider-originator"))
        );
        assert_eq!(
            merged.get("x-priority"),
            Some(&HeaderValue::from_static("extra"))
        );
        assert_eq!(
            merged.get("x-default-only"),
            Some(&HeaderValue::from_static("default-only"))
        );
    }

    #[test]
    fn websocket_url_from_http_base_defaults_to_ws_path() {
        let url =
            websocket_url_from_api_url("http://127.0.0.1:8011", None, None).expect("build ws url");
        assert_eq!(url.as_str(), "ws://127.0.0.1:8011/v1/realtime");
    }

    #[test]
    fn websocket_url_from_ws_base_defaults_to_ws_path() {
        let url =
            websocket_url_from_api_url("wss://example.com", None, Some("realtime-test-model"))
                .expect("build ws url");
        assert_eq!(
            url.as_str(),
            "wss://example.com/v1/realtime?model=realtime-test-model"
        );
    }

    #[test]
    fn websocket_url_from_v1_base_appends_realtime_path() {
        let url = websocket_url_from_api_url("https://api.openai.com/v1", None, Some("snapshot"))
            .expect("build ws url");
        assert_eq!(
            url.as_str(),
            "wss://api.openai.com/v1/realtime?model=snapshot"
        );
    }

    #[test]
    fn websocket_url_from_nested_v1_base_appends_realtime_path() {
        let url =
            websocket_url_from_api_url("https://example.com/openai/v1", None, Some("snapshot"))
                .expect("build ws url");
        assert_eq!(
            url.as_str(),
            "wss://example.com/openai/v1/realtime?model=snapshot"
        );
    }

    #[test]
    fn websocket_url_preserves_existing_realtime_path_and_extra_query_params() {
        let url = websocket_url_from_api_url(
            "https://example.com/v1/realtime?foo=bar",
            Some(&HashMap::from([("trace".to_string(), "1".to_string())])),
            Some("snapshot"),
        )
        .expect("build ws url");
        assert_eq!(
            url.as_str(),
            "wss://example.com/v1/realtime?foo=bar&model=snapshot&trace=1"
        );
    }

    #[tokio::test]
    async fn e2e_connect_and_exchange_events_against_mock_ws_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = accept_async(stream).await.expect("accept ws");

            let first = ws
                .next()
                .await
                .expect("first msg")
                .expect("first msg ok")
                .into_text()
                .expect("text");
            let first_json: Value = serde_json::from_str(&first).expect("json");
            assert_eq!(first_json["type"], "session.update");
            assert_eq!(
                first_json["session"]["type"],
                Value::String("realtime".to_string())
            );
            assert_eq!(
                first_json["session"]["instructions"],
                Value::String("backend prompt".to_string())
            );
            assert_eq!(
                first_json["session"]["output_modalities"][0],
                Value::String("audio".to_string())
            );
            assert_eq!(
                first_json["session"]["audio"]["input"]["format"]["type"],
                Value::String("audio/pcm".to_string())
            );
            assert_eq!(
                first_json["session"]["audio"]["input"]["format"]["rate"],
                Value::from(24_000)
            );
            assert_eq!(
                first_json["session"]["audio"]["input"]["noise_reduction"]["type"],
                Value::String("near_field".to_string())
            );
            assert_eq!(
                first_json["session"]["audio"]["input"]["turn_detection"]["type"],
                Value::String("server_vad".to_string())
            );
            assert_eq!(
                first_json["session"]["audio"]["input"]["turn_detection"]["interrupt_response"],
                Value::Bool(true)
            );
            assert_eq!(
                first_json["session"]["audio"]["input"]["turn_detection"]["create_response"],
                Value::Bool(true)
            );
            assert_eq!(
                first_json["session"]["audio"]["output"]["format"]["type"],
                Value::String("audio/pcm".to_string())
            );
            assert_eq!(
                first_json["session"]["audio"]["output"]["format"]["rate"],
                Value::from(24_000)
            );
            assert_eq!(
                first_json["session"]["audio"]["output"]["voice"],
                Value::String("marin".to_string())
            );
            assert_eq!(
                first_json["session"]["tool_choice"],
                Value::String("auto".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][0]["type"],
                Value::String("function".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][0]["name"],
                Value::String("codex".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][0]["parameters"]["required"][0],
                Value::String("prompt".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][0]["parameters"]["properties"]["send_immediately"]["type"],
                Value::String("boolean".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][1]["type"],
                Value::String("function".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][1]["name"],
                Value::String("cancel_current_operation".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][1]["parameters"]["type"],
                Value::String("object".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][1]["parameters"]["required"],
                Value::Array(Vec::new())
            );
            assert_eq!(
                first_json["session"]["tools"][1]["parameters"]["properties"],
                json!({})
            );
            assert_eq!(
                first_json["session"]["tools"][2]["type"],
                Value::String("function".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][2]["name"],
                Value::String("turn_off_realtime_mode".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][2]["parameters"]["type"],
                Value::String("object".to_string())
            );
            assert_eq!(
                first_json["session"]["tools"][2]["parameters"]["required"],
                Value::Array(Vec::new())
            );
            assert_eq!(
                first_json["session"]["tools"][2]["parameters"]["properties"],
                json!({})
            );
            assert_eq!(
                first_json["session"]["tools"]
                    .as_array()
                    .expect("tools array")
                    .iter()
                    .map(|tool| tool["name"].as_str().expect("tool name"))
                    .collect::<Vec<_>>(),
                vec![
                    "codex",
                    "cancel_current_operation",
                    "turn_off_realtime_mode",
                    "manage_message_queue",
                    "manage_runtime_settings",
                    "run_tui_command",
                ]
            );
            assert_eq!(
                first_json["session"]["tools"][4]["parameters"]["properties"]["working_directory"]
                    ["type"],
                Value::String("string".to_string())
            );

            ws.send(Message::Text(
                json!({
                    "type": "session.updated",
                    "session": {"id": "sess_mock", "instructions": "backend prompt"}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send session.updated");

            let second = ws
                .next()
                .await
                .expect("second msg")
                .expect("second msg ok")
                .into_text()
                .expect("text");
            let second_json: Value = serde_json::from_str(&second).expect("json");
            assert_eq!(second_json["type"], "input_audio_buffer.append");

            let third = ws
                .next()
                .await
                .expect("third msg")
                .expect("third msg ok")
                .into_text()
                .expect("text");
            let third_json: Value = serde_json::from_str(&third).expect("json");
            assert_eq!(third_json["type"], "conversation.item.create");
            assert_eq!(third_json["item"]["content"][0]["text"], "hello agent");

            let fourth = ws
                .next()
                .await
                .expect("fourth msg")
                .expect("fourth msg ok")
                .into_text()
                .expect("text");
            let fourth_json: Value = serde_json::from_str(&fourth).expect("json");
            assert_eq!(fourth_json["type"], "conversation.item.create");
            assert_eq!(fourth_json["item"]["type"], "message");
            assert_eq!(fourth_json["item"]["role"], "assistant");
            assert_eq!(
                fourth_json["item"]["content"][0]["type"],
                Value::String("output_text".to_string())
            );
            assert_eq!(
                fourth_json["item"]["content"][0]["text"],
                Value::String("hello from codex".to_string())
            );

            let fifth = ws
                .next()
                .await
                .expect("fifth msg")
                .expect("fifth msg ok")
                .into_text()
                .expect("text");
            let fifth_json: Value = serde_json::from_str(&fifth).expect("json");
            assert_eq!(fifth_json["type"], "conversation.item.create");
            assert_eq!(fifth_json["item"]["type"], "function_call_output");
            assert_eq!(fifth_json["item"]["call_id"], "handoff_1");

            let sixth = ws
                .next()
                .await
                .expect("sixth msg")
                .expect("sixth msg ok")
                .into_text()
                .expect("text");
            let sixth_json: Value = serde_json::from_str(&sixth).expect("json");
            assert_eq!(sixth_json["type"], "response.create");

            ws.send(Message::Text(
                json!({
                    "type": "response.output_audio.delta",
                    "delta": "AQID",
                    "sample_rate": 48000,
                    "channels": 1
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send audio");

            ws.send(Message::Text(
                json!({
                    "type": "conversation.input_transcript.delta",
                    "delta": "delegate "
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send input transcript delta");

            ws.send(Message::Text(
                json!({
                    "type": "conversation.input_transcript.delta",
                    "delta": "now"
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send input transcript delta");

            ws.send(Message::Text(
                json!({
                    "type": "conversation.output_transcript.delta",
                    "delta": "working"
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send output transcript delta");

            ws.send(Message::Text(
                json!({
                    "type": "response.done",
                    "response": {
                        "output": [
                            {
                                "id": "item_2",
                                "type": "function_call",
                                "name": "codex",
                                "call_id": "handoff_1",
                                "arguments": "{\"prompt\":\"delegate now\"}"
                            }
                        ]
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send item added");
        });

        let provider = Provider {
            name: "test".to_string(),
            base_url: format!("http://{addr}"),
            query_params: Some(HashMap::new()),
            headers: HeaderMap::new(),
            retry: crate::provider::RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: Duration::from_secs(5),
        };
        let client = RealtimeWebsocketClient::new(provider);
        let connection = client
            .connect(
                RealtimeSessionConfig {
                    instructions: "backend prompt".to_string(),
                    model: Some("realtime-test-model".to_string()),
                    session_id: Some("conv_1".to_string()),
                },
                HeaderMap::new(),
                HeaderMap::new(),
            )
            .await
            .expect("connect");

        let created = connection
            .next_event()
            .await
            .expect("next event")
            .expect("event");
        assert_eq!(
            created,
            RealtimeEvent::SessionUpdated {
                session_id: "sess_mock".to_string(),
                instructions: Some("backend prompt".to_string()),
            }
        );

        connection
            .send_audio_frame(RealtimeAudioFrame {
                data: "AQID".to_string(),
                sample_rate: 48000,
                num_channels: 1,
                samples_per_channel: Some(960),
            })
            .await
            .expect("send audio");
        connection
            .send_conversation_item_create("hello agent".to_string())
            .await
            .expect("send item");
        connection
            .send_conversation_handoff_append(
                "handoff_1".to_string(),
                "hello from codex".to_string(),
            )
            .await
            .expect("send handoff");
        connection
            .send_function_call_output("handoff_1".to_string(), "final from codex".to_string())
            .await
            .expect("send function output");
        connection
            .send_response_create()
            .await
            .expect("send response.create");

        let audio_event = connection
            .next_event()
            .await
            .expect("next event")
            .expect("event");
        assert_eq!(
            audio_event,
            RealtimeEvent::AudioOut(RealtimeOutputAudioDelta {
                frame: RealtimeAudioFrame {
                    data: "AQID".to_string(),
                    sample_rate: 48000,
                    num_channels: 1,
                    samples_per_channel: None,
                },
                item_id: None,
            })
        );

        let input_delta_event = connection
            .next_event()
            .await
            .expect("next event")
            .expect("event");
        assert_eq!(
            input_delta_event,
            RealtimeEvent::InputTranscriptDelta(RealtimeTranscriptDelta {
                delta: "delegate ".to_string(),
            })
        );

        let input_delta_event = connection
            .next_event()
            .await
            .expect("next event")
            .expect("event");
        assert_eq!(
            input_delta_event,
            RealtimeEvent::InputTranscriptDelta(RealtimeTranscriptDelta {
                delta: "now".to_string(),
            })
        );

        let output_delta_event = connection
            .next_event()
            .await
            .expect("next event")
            .expect("event");
        assert_eq!(
            output_delta_event,
            RealtimeEvent::OutputTranscriptDelta(RealtimeTranscriptDelta {
                delta: "working".to_string(),
            })
        );

        let added_event = connection
            .next_event()
            .await
            .expect("next event")
            .expect("event");
        assert_eq!(
            added_event,
            RealtimeEvent::HandoffRequested(RealtimeHandoffRequested {
                handoff_id: "handoff_1".to_string(),
                item_id: "item_2".to_string(),
                input_transcript: "delegate now".to_string(),
                send_immediately: false,
                active_transcript: vec![
                    RealtimeTranscriptEntry {
                        role: "user".to_string(),
                        text: "delegate now".to_string(),
                    },
                    RealtimeTranscriptEntry {
                        role: "assistant".to_string(),
                        text: "working".to_string(),
                    },
                ],
            })
        );

        connection.close().await.expect("close");
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn send_does_not_block_while_next_event_waits_for_inbound_data() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = accept_async(stream).await.expect("accept ws");

            let first = ws
                .next()
                .await
                .expect("first msg")
                .expect("first msg ok")
                .into_text()
                .expect("text");
            let first_json: Value = serde_json::from_str(&first).expect("json");
            assert_eq!(first_json["type"], "session.update");

            let second = ws
                .next()
                .await
                .expect("second msg")
                .expect("second msg ok")
                .into_text()
                .expect("text");
            let second_json: Value = serde_json::from_str(&second).expect("json");
            assert_eq!(second_json["type"], "input_audio_buffer.append");

            ws.send(Message::Text(
                json!({
                    "type": "session.updated",
                    "session": {"id": "sess_after_send", "instructions": "backend prompt"}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send session.updated");
        });

        let provider = Provider {
            name: "test".to_string(),
            base_url: format!("http://{addr}"),
            query_params: Some(HashMap::new()),
            headers: HeaderMap::new(),
            retry: crate::provider::RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: Duration::from_secs(5),
        };
        let client = RealtimeWebsocketClient::new(provider);
        let connection = client
            .connect(
                RealtimeSessionConfig {
                    instructions: "backend prompt".to_string(),
                    model: Some("realtime-test-model".to_string()),
                    session_id: Some("conv_1".to_string()),
                },
                HeaderMap::new(),
                HeaderMap::new(),
            )
            .await
            .expect("connect");

        let (send_result, next_result) = tokio::join!(
            async {
                tokio::time::timeout(
                    Duration::from_millis(200),
                    connection.send_audio_frame(RealtimeAudioFrame {
                        data: "AQID".to_string(),
                        sample_rate: 48000,
                        num_channels: 1,
                        samples_per_channel: Some(960),
                    }),
                )
                .await
            },
            connection.next_event()
        );

        send_result
            .expect("send should not block on next_event")
            .expect("send audio");
        let next_event = next_result.expect("next event").expect("event");
        assert_eq!(
            next_event,
            RealtimeEvent::SessionUpdated {
                session_id: "sess_after_send".to_string(),
                instructions: Some("backend prompt".to_string()),
            }
        );

        connection.close().await.expect("close");
        server.await.expect("server task");
    }
}
