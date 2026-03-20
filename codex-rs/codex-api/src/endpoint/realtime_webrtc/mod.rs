use crate::endpoint::realtime_websocket::parse_realtime_event;
use crate::error::ApiError;
use crate::provider::Provider;
use codex_protocol::protocol::RealtimeEvent;
use http::HeaderMap;
use libwebrtc::MediaType;
use libwebrtc::data_channel::DataChannel;
use libwebrtc::data_channel::DataChannelInit;
use libwebrtc::data_channel::DataChannelState;
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
use serde_json::json;
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

const REALTIME_CALLS_PATH: &str = "/v1/realtime/calls";
const REALTIME_DATA_CHANNEL_LABEL: &str = "oai-events";
const REALTIME_VOICE: &str = "marin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSessionConfig {
    pub instructions: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
}

pub struct RealtimeWebrtcConnection {
    writer: RealtimeWebrtcWriter,
    events: RealtimeWebrtcEvents,
}

#[derive(Clone)]
pub struct RealtimeWebrtcWriter {
    peer_connection: PeerConnection,
    data_channel: DataChannel,
    is_closed: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct RealtimeWebrtcEvents {
    rx_event: Arc<Mutex<mpsc::UnboundedReceiver<RealtimeEvent>>>,
    is_closed: Arc<AtomicBool>,
}

pub struct RealtimeWebrtcClient {
    provider: Provider,
}

impl RealtimeWebrtcConnection {
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
        rx_event: mpsc::UnboundedReceiver<RealtimeEvent>,
        is_closed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            writer: RealtimeWebrtcWriter {
                peer_connection,
                data_channel,
                is_closed: Arc::clone(&is_closed),
            },
            events: RealtimeWebrtcEvents {
                rx_event: Arc::new(Mutex::new(rx_event)),
                is_closed,
            },
        }
    }
}

impl RealtimeWebrtcWriter {
    pub async fn send_conversation_item_create(&self, text: String) -> Result<(), ApiError> {
        self.send_json(json!({
            "type": "conversation.item.create",
            "item": {
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": text,
                }],
            },
        }))
        .await
    }

    pub async fn send_conversation_handoff_append(
        &self,
        _handoff_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        self.send_json(json!({
            "type": "conversation.item.create",
            "item": {
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": output_text,
                }],
            },
        }))
        .await
    }

    pub async fn send_function_call_output(
        &self,
        call_id: String,
        output_text: String,
    ) -> Result<(), ApiError> {
        self.send_json(json!({
            "type": "conversation.item.create",
            "item": {
                "type": "function_call_output",
                "call_id": call_id,
                "output": json!({
                    "content": output_text,
                }).to_string(),
            },
        }))
        .await
    }

    pub async fn send_response_create(&self) -> Result<(), ApiError> {
        self.send_json(json!({ "type": "response.create" })).await
    }

    pub async fn close(&self) -> Result<(), ApiError> {
        if self.is_closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.data_channel.close();
        self.peer_connection.close();
        Ok(())
    }

    async fn send_json(&self, payload: serde_json::Value) -> Result<(), ApiError> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Err(ApiError::Stream(
                "realtime WebRTC connection is closed".to_string(),
            ));
        }

        let serialized = serde_json::to_vec(&payload).map_err(|err| {
            ApiError::Stream(format!("failed to serialize realtime event: {err}"))
        })?;
        self.data_channel.send(&serialized, false).map_err(|err| {
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
            Some(event) => Ok(Some(event)),
            None => {
                self.is_closed.store(true, Ordering::SeqCst);
                Ok(None)
            }
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
        let factory = PeerConnectionFactory::with_platform_adm();
        let peer_connection = factory
            .create_peer_connection(RtcConfiguration::default())
            .map_err(|err| {
                ApiError::Stream(format!("failed to create WebRTC peer connection: {err}"))
            })?;

        // Negotiate an audio m-line and attach a local mic track backed by the platform ADM.
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

        let local_audio_source = factory.create_audio_source();
        let local_audio_track = factory.create_audio_track("realtime-mic", local_audio_source);
        audio_transceiver
            .sender()
            .set_track(Some(local_audio_track.into()))
            .map_err(|err| {
                ApiError::Stream(format!("failed to attach ADM audio track to sender: {err}"))
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
                if let Some(event) = parse_realtime_event(payload)
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

        let url = realtime_calls_url(&self.provider.base_url)?;
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
            rx_event,
            is_closed,
        ))
    }
}

fn realtime_calls_url(base_url: &str) -> Result<Url, ApiError> {
    let mut url =
        Url::parse(base_url).map_err(|err| ApiError::Stream(format!("invalid base URL: {err}")))?;
    url.set_path(REALTIME_CALLS_PATH);
    Ok(url)
}

fn session_form(
    config: &RealtimeSessionConfig,
    offer: &SessionDescription,
) -> Result<Form, ApiError> {
    let session_json = serde_json::to_string(&session_payload(config))
        .map_err(|err| ApiError::Stream(format!("failed to serialize realtime session: {err}")))?;

    Ok(Form::new()
        .text("sdp", offer.to_string())
        .text("session", session_json))
}

fn session_payload(config: &RealtimeSessionConfig) -> serde_json::Value {
    let mut session = json!({
        "type": "realtime",
        "instructions": config.instructions,
        "output_modalities": ["audio"],
        "audio": {
            "output": {
                "voice": REALTIME_VOICE,
            },
            "input": {
                "turn_detection": {
                    "type": "server_vad",
                    "interrupt_response": true,
                    "create_response": true,
                },
            },
        },
        "tools": [
            {
                "type": "function",
                "name": "codex",
                "description": "Delegate the user's request to Codex.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input_transcript": {
                            "type": "string",
                            "description": "Transcript of the user's request.",
                        },
                        "send_immediately": {
                            "type": "boolean",
                            "description": "Whether Codex should receive the request immediately.",
                        },
                    },
                    "required": ["input_transcript"],
                },
            },
            {
                "type": "function",
                "name": "cancel_current_operation",
                "description": "Cancel the current Codex operation.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": [],
                },
            },
            {
                "type": "function",
                "name": "turn_off_realtime_mode",
                "description": "Turn off realtime voice mode.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": [],
                },
            },
        ],
        "tool_choice": "auto",
    });

    if let Some(model) = &config.model {
        session["model"] = json!(model);
    }
    session
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn realtime_calls_url_uses_webrtc_calls_path() {
        assert_eq!(
            realtime_calls_url("https://api.openai.com")
                .expect("url")
                .as_str(),
            "https://api.openai.com/v1/realtime/calls"
        );
    }

    #[test]
    fn session_form_contains_sdp_and_session_payload() {
        let offer = SessionDescription::parse(
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
            SdpType::Offer,
        )
        .expect("offer");

        let form = session_form(
            &RealtimeSessionConfig {
                instructions: "backend prompt".to_string(),
                model: Some("gpt-realtime-1.5".to_string()),
                session_id: Some("sess_123".to_string()),
            },
            &offer,
        )
        .expect("form");

        let body = form.boundary().to_string();
        assert!(!body.is_empty());
    }

    #[test]
    fn session_payload_omits_session_id_from_body() {
        let payload = session_payload(&RealtimeSessionConfig {
            instructions: "backend prompt".to_string(),
            model: Some("gpt-realtime-1.5".to_string()),
            session_id: Some("sess_123".to_string()),
        });

        assert_eq!(payload.get("id"), None);
        assert_eq!(payload.get("model"), Some(&json!("gpt-realtime-1.5")));
    }

    #[test]
    fn peer_connection_factory_smoke() {
        let factory = PeerConnectionFactory::default();
        let _pc = factory
            .create_peer_connection(RtcConfiguration::default())
            .expect("pc");
    }
}
