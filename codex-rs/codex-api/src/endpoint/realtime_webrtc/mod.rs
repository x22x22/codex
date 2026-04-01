use crate::endpoint::realtime_websocket::methods_common::conversation_handoff_append_message;
use crate::endpoint::realtime_websocket::methods_common::conversation_item_create_message;
use crate::endpoint::realtime_websocket::methods_common::normalized_session_mode;
use crate::endpoint::realtime_websocket::methods_common::session_update_session;
use crate::endpoint::realtime_websocket::protocol::RealtimeEventParser;
use crate::endpoint::realtime_websocket::protocol::RealtimeOutboundMessage;
use crate::endpoint::realtime_websocket::protocol::RealtimeSessionConfig;
use crate::endpoint::realtime_websocket::protocol::RealtimeTranscriptDelta;
use crate::endpoint::realtime_websocket::protocol::RealtimeTranscriptEntry;
use crate::endpoint::realtime_websocket::protocol::parse_realtime_event;
use crate::error::ApiError;
use crate::provider::Provider;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures::StreamExt;
use http::HeaderMap;
use libwebrtc::MediaType;
use libwebrtc::audio_frame::AudioFrame;
use libwebrtc::audio_source::AudioSourceOptions;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_stream::native::NativeAudioStream;
use libwebrtc::audio_stream::native::NativeAudioStreamOptions;
use libwebrtc::data_channel::DataChannel;
use libwebrtc::data_channel::DataChannelInit;
use libwebrtc::data_channel::DataChannelState;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::OfferOptions;
use libwebrtc::peer_connection::PeerConnection;
use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::RtcConfiguration;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::rtp_transceiver::RtpTransceiverDirection;
use libwebrtc::rtp_transceiver::RtpTransceiverInit;
use libwebrtc::session_description::SdpType;
use libwebrtc::session_description::SessionDescription;
use reqwest::Client;
use reqwest::multipart::Form;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tracing::debug;
use tracing::info;
use tracing::warn;
use url::Url;

pub use codex_protocol::protocol::RealtimeAudioFrame;
pub use codex_protocol::protocol::RealtimeEvent;

const REALTIME_CALLS_PATH: &str = "/v1/realtime/calls";
const REALTIME_DATA_CHANNEL_LABEL: &str = "oai-events";
const REALTIME_AUDIO_SAMPLE_RATE: u32 = 24_000;
const REALTIME_AUDIO_NUM_CHANNELS: u32 = 1;
// Keep one extra 60 ms queue on the native source and roughly 100 ms on the
// sink side so WebRTC can absorb small scheduling spikes without drifting.
const REALTIME_AUDIO_SOURCE_QUEUE_SIZE_MS: u32 = 60;
const REALTIME_AUDIO_STREAM_QUEUE_SIZE_FRAMES: usize = 10;

pub struct RealtimeWebrtcConnection {
    writer: RealtimeWebrtcWriter,
    events: RealtimeWebrtcEvents,
}

#[derive(Clone)]
pub struct RealtimeWebrtcWriter {
    peer_connection: PeerConnection,
    data_channel: DataChannel,
    local_audio_source: NativeAudioSource,
    is_closed: Arc<AtomicBool>,
    event_parser: RealtimeEventParser,
}

#[derive(Clone)]
pub struct RealtimeWebrtcEvents {
    rx_event: Arc<Mutex<mpsc::UnboundedReceiver<RealtimeEvent>>>,
    active_transcript: Arc<Mutex<ActiveTranscriptState>>,
    is_closed: Arc<AtomicBool>,
    event_parser: RealtimeEventParser,
}

#[derive(Default)]
struct ActiveTranscriptState {
    entries: Vec<RealtimeTranscriptEntry>,
}

pub struct RealtimeWebrtcClient {
    provider: Provider,
}

impl RealtimeWebrtcConnection {
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

    pub async fn send_response_create(&self) -> Result<(), ApiError> {
        self.writer.send_response_create().await
    }

    pub async fn close(&self) -> Result<(), ApiError> {
        self.writer.close().await
    }

    pub async fn next_event(&self) -> Result<Option<RealtimeEvent>, ApiError> {
        self.events.next_event().await
    }

    pub fn writer(&self) -> RealtimeWebrtcWriter {
        self.writer.clone()
    }

    pub fn events(&self) -> RealtimeWebrtcEvents {
        self.events.clone()
    }

    fn new(
        peer_connection: PeerConnection,
        data_channel: DataChannel,
        local_audio_source: NativeAudioSource,
        rx_event: mpsc::UnboundedReceiver<RealtimeEvent>,
        is_closed: Arc<AtomicBool>,
        event_parser: RealtimeEventParser,
    ) -> Self {
        Self {
            writer: RealtimeWebrtcWriter {
                peer_connection,
                data_channel,
                local_audio_source,
                is_closed: Arc::clone(&is_closed),
                event_parser,
            },
            events: RealtimeWebrtcEvents {
                rx_event: Arc::new(Mutex::new(rx_event)),
                active_transcript: Arc::new(Mutex::new(ActiveTranscriptState::default())),
                is_closed,
                event_parser,
            },
        }
    }
}

impl RealtimeWebrtcWriter {
    pub async fn send_audio_frame(&self, frame: RealtimeAudioFrame) -> Result<(), ApiError> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Err(ApiError::Stream(
                "realtime WebRTC connection is closed".to_string(),
            ));
        }

        if frame.sample_rate != self.local_audio_source.sample_rate() {
            return Err(ApiError::Stream(format!(
                "unexpected realtime audio sample rate: got {got}, expected {expected}",
                got = frame.sample_rate,
                expected = self.local_audio_source.sample_rate()
            )));
        }

        if u32::from(frame.num_channels) != self.local_audio_source.num_channels() {
            return Err(ApiError::Stream(format!(
                "unexpected realtime audio channel count: got {got}, expected {expected}",
                got = frame.num_channels,
                expected = self.local_audio_source.num_channels()
            )));
        }

        let decoded = BASE64_STANDARD.decode(&frame.data).map_err(|err| {
            ApiError::Stream(format!("failed to decode realtime audio frame: {err}"))
        })?;
        if decoded.len() % 2 != 0 {
            return Err(ApiError::Stream(
                "realtime audio frame has an odd byte length".to_string(),
            ));
        }

        let samples = decoded
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect::<Vec<_>>();
        let num_channels = self.local_audio_source.num_channels();
        let samples_per_channel = samples
            .len()
            .checked_div(num_channels as usize)
            .and_then(|samples_per_channel| u32::try_from(samples_per_channel).ok())
            .ok_or_else(|| {
                ApiError::Stream("failed to calculate realtime audio frame size".to_string())
            })?;
        let audio_frame = AudioFrame {
            data: Cow::Owned(samples),
            sample_rate: self.local_audio_source.sample_rate(),
            num_channels,
            samples_per_channel,
        };
        self.local_audio_source
            .capture_frame(&audio_frame)
            .await
            .map_err(|err| {
                ApiError::Stream(format!("failed to capture realtime audio frame: {err}"))
            })
    }

    pub async fn send_conversation_item_create(&self, text: String) -> Result<(), ApiError> {
        self.send_json(&conversation_item_create_message(self.event_parser, text))
            .await
    }

    pub async fn send_conversation_handoff_append(
        &self,
        handoff_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        self.send_json(&conversation_handoff_append_message(
            self.event_parser,
            handoff_id,
            output_text,
        ))
        .await
    }

    pub async fn send_response_create(&self) -> Result<(), ApiError> {
        self.send_json(&RealtimeOutboundMessage::ResponseCreate)
            .await
    }

    pub async fn close(&self) -> Result<(), ApiError> {
        if self.is_closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        self.local_audio_source.clear_buffer();
        self.data_channel.close();
        self.peer_connection.close();
        Ok(())
    }

    async fn send_json(&self, message: &impl Serialize) -> Result<(), ApiError> {
        let payload = serde_json::to_string(message)
            .map_err(|err| ApiError::Stream(format!("failed to encode realtime request: {err}")))?;
        debug!(payload = %payload, "realtime WebRTC data channel request");
        self.send_payload(payload).await
    }

    pub async fn send_payload(&self, payload: String) -> Result<(), ApiError> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Err(ApiError::Stream(
                "realtime WebRTC connection is closed".to_string(),
            ));
        }

        self.data_channel
            .send(payload.as_bytes(), false)
            .map_err(|err| {
                ApiError::Stream(format!("failed to send realtime data channel event: {err}"))
            })
    }
}

impl RealtimeWebrtcEvents {
    pub async fn next_event(&self) -> Result<Option<RealtimeEvent>, ApiError> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Ok(None);
        }

        match self.rx_event.lock().await.recv().await {
            Some(mut event) => {
                self.update_active_transcript(&mut event).await;
                Ok(Some(event))
            }
            None => {
                self.is_closed.store(true, Ordering::SeqCst);
                Ok(None)
            }
        }
    }

    async fn update_active_transcript(&self, event: &mut RealtimeEvent) {
        let mut active_transcript = self.active_transcript.lock().await;
        match event {
            RealtimeEvent::InputAudioSpeechStarted(_) => {}
            RealtimeEvent::InputTranscriptDelta(RealtimeTranscriptDelta { delta }) => {
                append_transcript_delta(&mut active_transcript.entries, "user", delta);
            }
            RealtimeEvent::OutputTranscriptDelta(RealtimeTranscriptDelta { delta }) => {
                append_transcript_delta(&mut active_transcript.entries, "assistant", delta);
            }
            RealtimeEvent::HandoffRequested(handoff) => {
                if self.event_parser == RealtimeEventParser::V1 {
                    handoff.active_transcript = std::mem::take(&mut active_transcript.entries);
                }
            }
            RealtimeEvent::SessionUpdated { .. }
            | RealtimeEvent::AudioOut(_)
            | RealtimeEvent::ResponseCancelled(_)
            | RealtimeEvent::ConversationItemAdded(_)
            | RealtimeEvent::ConversationItemDone { .. }
            | RealtimeEvent::Error(_) => {}
        }
    }
}

impl RealtimeWebrtcClient {
    pub fn new(provider: Provider) -> Self {
        Self { provider }
    }

    pub async fn connect(
        &self,
        config: RealtimeSessionConfig,
        extra_headers: HeaderMap,
        default_headers: HeaderMap,
    ) -> Result<RealtimeWebrtcConnection, ApiError> {
        info!("initializing realtime WebRTC peer connection");
        let factory = PeerConnectionFactory::default();
        let peer_connection = factory
            .create_peer_connection(RtcConfiguration::default())
            .map_err(|err| {
                ApiError::Stream(format!("failed to create WebRTC peer connection: {err}"))
            })?;

        let local_audio_source = NativeAudioSource::new(
            AudioSourceOptions::default(),
            REALTIME_AUDIO_SAMPLE_RATE,
            REALTIME_AUDIO_NUM_CHANNELS,
            REALTIME_AUDIO_SOURCE_QUEUE_SIZE_MS,
        );
        let local_audio_track =
            factory.create_audio_track("realtime-mic", local_audio_source.clone());
        let audio_transceiver = peer_connection
            .add_transceiver_for_media(
                MediaType::Audio,
                RtpTransceiverInit {
                    direction: RtpTransceiverDirection::SendRecv,
                    stream_ids: vec!["realtime".to_string()],
                    send_encodings: Vec::new(),
                },
            )
            .map_err(|err| ApiError::Stream(format!("failed to add audio transceiver: {err}")))?;
        audio_transceiver
            .sender()
            .set_track(Some(local_audio_track.into()))
            .map_err(|err| {
                ApiError::Stream(format!("failed to attach realtime audio track: {err}"))
            })?;

        let data_channel = peer_connection
            .create_data_channel(REALTIME_DATA_CHANNEL_LABEL, DataChannelInit::default())
            .map_err(|err| {
                ApiError::Stream(format!("failed to create realtime data channel: {err}"))
            })?;

        let (tx_event, rx_event) = mpsc::unbounded_channel();
        let is_closed = Arc::new(AtomicBool::new(false));
        let (tx_open, rx_open) = oneshot::channel::<()>();
        let tx_open = Arc::new(StdMutex::new(Some(tx_open)));

        {
            let tx_event = tx_event.clone();
            let event_parser = config.event_parser;
            data_channel.on_message(Some(Box::new(move |buffer| {
                if buffer.binary {
                    debug!(
                        payload_len = buffer.data.len(),
                        "ignoring binary realtime data channel message"
                    );
                    return;
                }

                let payload = match std::str::from_utf8(buffer.data) {
                    Ok(payload) => payload,
                    Err(err) => {
                        debug!("received non-utf8 realtime data channel message: {err}");
                        return;
                    }
                };
                if let Some(event) = parse_realtime_event(payload, event_parser)
                    && tx_event.send(event).is_err()
                {
                    debug!("dropping realtime event because receiver closed");
                }
            })));
        }

        {
            let is_closed = Arc::clone(&is_closed);
            let tx_open = Arc::clone(&tx_open);
            data_channel.on_state_change(Some(Box::new(move |state| match state {
                DataChannelState::Connecting => {}
                DataChannelState::Open => {
                    if let Ok(mut tx_open) = tx_open.lock()
                        && let Some(tx_open) = tx_open.take()
                    {
                        let _ = tx_open.send(());
                    }
                }
                DataChannelState::Closing | DataChannelState::Closed => {
                    is_closed.store(true, Ordering::SeqCst);
                }
            })));
        }

        {
            let tx_event = tx_event.clone();
            peer_connection.on_track(Some(Box::new(move |track_event| {
                let MediaStreamTrack::Audio(audio_track) = track_event.track else {
                    return;
                };

                let tx_event = tx_event.clone();
                tokio::spawn(async move {
                    let mut audio_stream = NativeAudioStream::with_options(
                        audio_track,
                        REALTIME_AUDIO_SAMPLE_RATE as i32,
                        REALTIME_AUDIO_NUM_CHANNELS as i32,
                        NativeAudioStreamOptions {
                            queue_size_frames: Some(REALTIME_AUDIO_STREAM_QUEUE_SIZE_FRAMES),
                        },
                    );

                    while let Some(frame) = audio_stream.next().await {
                        let event = RealtimeEvent::AudioOut(encode_audio_frame(frame));
                        if tx_event.send(event).is_err() {
                            break;
                        }
                    }
                });
            })));
        }

        let offer = peer_connection
            .create_offer(OfferOptions {
                ice_restart: false,
                offer_to_receive_audio: true,
                offer_to_receive_video: false,
            })
            .await
            .map_err(|err| ApiError::Stream(format!("failed to create WebRTC offer: {err}")))?;

        peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|err| ApiError::Stream(format!("failed to set local description: {err}")))?;

        let url = realtime_calls_url(&self.provider.base_url, self.provider.query_params.as_ref())?;
        let headers = merge_request_headers(&self.provider.headers, extra_headers, default_headers);
        info!(url = %url, "posting realtime WebRTC offer");
        let http_client = Client::new();
        let mut request = http_client
            .post(url)
            .multipart(session_form(&config, &offer)?);
        for (name, value) in &headers {
            request = request.header(name, value);
        }

        let response = request.send().await.map_err(|err| {
            ApiError::Stream(format!("failed to post realtime WebRTC offer: {err}"))
        })?;
        let status = response.status();
        let answer_sdp = response.text().await.map_err(|err| {
            ApiError::Stream(format!("failed to read realtime WebRTC answer body: {err}"))
        })?;
        if !status.is_success() {
            return Err(ApiError::Stream(format!(
                "realtime WebRTC offer failed with HTTP {status}: {answer_sdp}"
            )));
        }

        let answer = SessionDescription::parse(&answer_sdp, SdpType::Answer)
            .map_err(|err| ApiError::Stream(format!("failed to parse WebRTC answer SDP: {err}")))?;
        peer_connection
            .set_remote_description(answer)
            .await
            .map_err(|err| ApiError::Stream(format!("failed to set remote description: {err}")))?;

        if tokio::time::timeout(Duration::from_secs(10), rx_open)
            .await
            .is_err()
        {
            warn!("timed out waiting for realtime data channel to open");
        }

        Ok(RealtimeWebrtcConnection::new(
            peer_connection,
            data_channel,
            local_audio_source,
            rx_event,
            is_closed,
            config.event_parser,
        ))
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

fn encode_audio_frame(frame: AudioFrame<'_>) -> RealtimeAudioFrame {
    let mut bytes = Vec::with_capacity(frame.data.len() * 2);
    for sample in frame.data.iter() {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    RealtimeAudioFrame {
        data: BASE64_STANDARD.encode(bytes),
        sample_rate: frame.sample_rate,
        num_channels: frame.num_channels as u16,
        samples_per_channel: Some(frame.samples_per_channel),
        item_id: None,
    }
}

fn realtime_calls_url(
    base_url: &str,
    query_params: Option<&HashMap<String, String>>,
) -> Result<Url, ApiError> {
    let mut url =
        Url::parse(base_url).map_err(|err| ApiError::Stream(format!("invalid base URL: {err}")))?;
    url.set_path(REALTIME_CALLS_PATH);
    if let Some(query_params) = query_params
        && !query_params.is_empty()
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in query_params {
            query.append_pair(key, value);
        }
    }
    Ok(url)
}

fn session_form(
    config: &RealtimeSessionConfig,
    offer: &SessionDescription,
) -> Result<Form, ApiError> {
    let session_json = serde_json::to_string(&session_payload(config)?)
        .map_err(|err| ApiError::Stream(format!("failed to serialize realtime session: {err}")))?;

    Ok(Form::new()
        .text("sdp", offer.to_string())
        .text("session", session_json))
}

fn session_payload(config: &RealtimeSessionConfig) -> Result<Value, ApiError> {
    let session_mode = normalized_session_mode(config.event_parser, config.session_mode);
    let mut session = serde_json::to_value(session_update_session(
        config.event_parser,
        config.instructions.clone(),
        session_mode,
    ))
    .map_err(|err| ApiError::Stream(format!("failed to encode realtime session: {err}")))?;
    if let Some(model) = &config.model {
        session["model"] = json!(model);
    }
    Ok(session)
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
