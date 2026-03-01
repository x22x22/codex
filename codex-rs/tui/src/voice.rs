use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use base64::Engine;
use codex_audio::AUDIO_CHANNELS;
use codex_audio::AUDIO_SAMPLE_RATE;
use codex_audio::InputCapture;
use codex_audio::OutputPlayback;
use codex_core::auth::AuthCredentialsStoreMode;
use codex_core::config::Config;
use codex_core::config::find_codex_home;
use codex_core::default_client::get_codex_user_agent;
use codex_login::AuthMode;
use codex_login::CodexAuth;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::RealtimeAudioFrame;
use hound::SampleFormat;
use hound::WavSpec;
use hound::WavWriter;
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::trace;
use tracing::warn;

const AUDIO_MODEL: &str = "gpt-4o-mini-transcribe";
const MODEL_AUDIO_SAMPLE_RATE: u32 = AUDIO_SAMPLE_RATE;
const MODEL_AUDIO_CHANNELS: u16 = AUDIO_CHANNELS as u16;

struct TranscriptionAuthContext {
    mode: AuthMode,
    bearer_token: String,
    chatgpt_account_id: Option<String>,
    chatgpt_base_url: String,
}

pub use codex_audio::RecordedAudio;

pub struct VoiceCapture {
    capture: InputCapture,
}

impl VoiceCapture {
    pub fn start() -> Result<Self, String> {
        debug!("starting push-to-talk voice capture");
        Ok(Self {
            capture: InputCapture::start_recording(None)?,
        })
    }

    pub fn start_realtime(config: &Config, tx: AppEventSender) -> Result<Self, String> {
        let device_id = config.realtime_audio.input_device_id.as_deref();
        info!(
            device_id = device_id.unwrap_or("system_default"),
            "starting realtime microphone capture"
        );
        let capture = InputCapture::start_streaming(device_id, move |samples| {
            send_realtime_audio_chunk(&tx, samples);
        })?;
        Ok(Self { capture })
    }

    pub fn stop(self) -> Result<RecordedAudio, String> {
        self.capture.stop()
    }

    pub fn data_arc(&self) -> Arc<Mutex<Vec<i16>>> {
        self.capture.data_arc()
    }

    pub fn stopped_flag(&self) -> Arc<AtomicBool> {
        self.capture.stopped_flag()
    }

    pub fn sample_rate(&self) -> u32 {
        self.capture.sample_rate()
    }

    pub fn channels(&self) -> u16 {
        self.capture.channels()
    }

    pub fn last_peak_arc(&self) -> Arc<AtomicU16> {
        self.capture.last_peak_arc()
    }
}

pub(crate) struct RecordingMeterState {
    history: VecDeque<char>,
    noise_ema: f64,
    env: f64,
}

impl RecordingMeterState {
    pub(crate) fn new() -> Self {
        let mut history = VecDeque::with_capacity(4);
        while history.len() < 4 {
            history.push_back('⠤');
        }
        Self {
            history,
            noise_ema: 0.02,
            env: 0.0,
        }
    }

    pub(crate) fn next_text(&mut self, peak: u16) -> String {
        const SYMBOLS: [char; 7] = ['⠤', '⠴', '⠶', '⠷', '⡷', '⡿', '⣿'];
        const ALPHA_NOISE: f64 = 0.05;
        const ATTACK: f64 = 0.80;
        const RELEASE: f64 = 0.25;

        let latest_peak = peak as f64 / (i16::MAX as f64);

        if latest_peak > self.env {
            self.env = ATTACK * latest_peak + (1.0 - ATTACK) * self.env;
        } else {
            self.env = RELEASE * latest_peak + (1.0 - RELEASE) * self.env;
        }

        let rms_approx = self.env * 0.7;
        self.noise_ema = (1.0 - ALPHA_NOISE) * self.noise_ema + ALPHA_NOISE * rms_approx;
        let ref_level = self.noise_ema.max(0.01);
        let fast_signal = 0.8 * latest_peak + 0.2 * self.env;
        let target = 2.0f64;
        let raw = (fast_signal / (ref_level * target)).max(0.0);
        let k = 1.6f64;
        let compressed = (raw.ln_1p() / k.ln_1p()).min(1.0);
        let idx = (compressed * (SYMBOLS.len() as f64 - 1.0))
            .round()
            .clamp(0.0, SYMBOLS.len() as f64 - 1.0) as usize;
        let level_char = SYMBOLS[idx];

        if self.history.len() >= 4 {
            self.history.pop_front();
        }
        self.history.push_back(level_char);

        let mut text = String::with_capacity(4);
        for ch in &self.history {
            text.push(*ch);
        }
        text
    }
}

pub fn transcribe_async(
    id: String,
    audio: RecordedAudio,
    context: Option<String>,
    tx: AppEventSender,
) {
    std::thread::spawn(move || {
        const MIN_DURATION_SECONDS: f32 = 0.3;
        let duration_seconds = clip_duration_seconds(&audio);
        info!(
            duration_seconds = duration_seconds,
            sample_rate = audio.sample_rate,
            channels = audio.channels,
            sample_count = audio.data.len(),
            has_context = context.as_ref().is_some_and(|value| !value.is_empty()),
            "starting voice transcription"
        );
        if duration_seconds < MIN_DURATION_SECONDS {
            let msg = format!(
                "recording too short ({duration_seconds:.2}s); minimum is {MIN_DURATION_SECONDS:.2}s"
            );
            warn!("{msg}");
            tx.send(AppEvent::TranscriptionFailed { id, error: msg });
            return;
        }

        let wav_bytes = match encode_wav_normalized(&audio) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "failed to encode transcription wav");
                tx.send(AppEvent::TranscriptionFailed { id, error: e });
                return;
            }
        };
        debug!(wav_bytes = wav_bytes.len(), "encoded transcription wav");

        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                error!(error = %e, "failed to create transcription runtime");
                tx.send(AppEvent::TranscriptionFailed {
                    id,
                    error: format!("failed to create transcription runtime: {e}"),
                });
                return;
            }
        };

        let tx2 = tx.clone();
        let id2 = id.clone();
        let res: Result<String, String> = rt
            .block_on(async move { transcribe_bytes(wav_bytes, context, duration_seconds).await });

        match res {
            Ok(text) => {
                info!(
                    transcript_chars = text.chars().count(),
                    "voice transcription succeeded"
                );
                tx2.send(AppEvent::TranscriptionComplete { id: id2, text });
            }
            Err(e) => {
                error!(error = %e, "voice transcription failed");
                tx.send(AppEvent::TranscriptionFailed { id, error: e });
            }
        }
    });
}

fn send_realtime_audio_chunk(tx: &AppEventSender, samples: Vec<i16>) {
    if samples.is_empty() {
        return;
    }

    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in &samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let samples_per_channel = (samples.len() / usize::from(MODEL_AUDIO_CHANNELS)) as u32;

    tx.send(AppEvent::CodexOp(Op::RealtimeConversationAudio(
        ConversationAudioParams {
            frame: RealtimeAudioFrame {
                data: encoded,
                sample_rate: MODEL_AUDIO_SAMPLE_RATE,
                num_channels: MODEL_AUDIO_CHANNELS,
                samples_per_channel: Some(samples_per_channel),
            },
        },
    )));
}

pub(crate) struct RealtimeAudioPlayer {
    playback: OutputPlayback,
}

impl RealtimeAudioPlayer {
    pub(crate) fn start(config: &Config) -> Result<Self, String> {
        info!(
            device_id = config
                .realtime_audio
                .output_device_id
                .as_deref()
                .unwrap_or("system_default"),
            "starting realtime speaker output"
        );
        Ok(Self {
            playback: OutputPlayback::start(config.realtime_audio.output_device_id.as_deref())?,
        })
    }

    pub(crate) fn enqueue_frame(&self, frame: &RealtimeAudioFrame) -> Result<(), String> {
        if frame.num_channels != MODEL_AUDIO_CHANNELS
            || frame.sample_rate != MODEL_AUDIO_SAMPLE_RATE
        {
            warn!(
                sample_rate = frame.sample_rate,
                num_channels = frame.num_channels,
                expected_sample_rate = MODEL_AUDIO_SAMPLE_RATE,
                expected_channels = MODEL_AUDIO_CHANNELS,
                "received unexpected realtime audio format"
            );
            return Err(format!(
                "unexpected realtime audio format: {} Hz / {} channels",
                frame.sample_rate, frame.num_channels
            ));
        }

        let raw_bytes = base64::engine::general_purpose::STANDARD
            .decode(&frame.data)
            .map_err(|e| format!("failed to decode realtime audio: {e}"))?;
        if raw_bytes.len() % 2 != 0 {
            return Err("realtime audio frame had odd byte length".to_string());
        }

        let mut pcm = Vec::with_capacity(raw_bytes.len() / 2);
        for pair in raw_bytes.chunks_exact(2) {
            pcm.push(i16::from_le_bytes([pair[0], pair[1]]));
        }
        self.playback.enqueue_samples(&pcm)
    }

    pub(crate) fn clear(&self) {
        self.playback.clear();
    }
}

fn clip_duration_seconds(audio: &RecordedAudio) -> f32 {
    let total_samples = audio.data.len() as f32;
    let samples_per_second = (audio.sample_rate as f32) * (audio.channels as f32);
    if samples_per_second > 0.0 {
        total_samples / samples_per_second
    } else {
        0.0
    }
}

fn encode_wav_normalized(audio: &RecordedAudio) -> Result<Vec<u8>, String> {
    if audio.channels != MODEL_AUDIO_CHANNELS || audio.sample_rate != MODEL_AUDIO_SAMPLE_RATE {
        return Err(format!(
            "unexpected recorded audio format: {} Hz / {} channels",
            audio.sample_rate, audio.channels
        ));
    }

    let mut wav_bytes: Vec<u8> = Vec::new();
    let spec = WavSpec {
        channels: audio.channels,
        sample_rate: audio.sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut cursor = Cursor::new(&mut wav_bytes);
    let mut writer =
        WavWriter::new(&mut cursor, spec).map_err(|_| "failed to create wav writer".to_string())?;

    let mut peak: i16 = 0;
    for &sample in &audio.data {
        let absolute = sample.unsigned_abs();
        if absolute > peak.unsigned_abs() {
            peak = sample;
        }
    }
    let peak_abs = (peak as i32).unsigned_abs() as i32;
    let target = (i16::MAX as f32) * 0.9;
    let gain = if peak_abs > 0 {
        target / (peak_abs as f32)
    } else {
        1.0
    };

    for &sample in &audio.data {
        let normalized = ((sample as f32) * gain)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer
            .write_sample(normalized)
            .map_err(|_| "failed writing wav sample".to_string())?;
    }
    writer
        .finalize()
        .map_err(|_| "failed to finalize wav".to_string())?;
    Ok(wav_bytes)
}

fn normalize_chatgpt_base_url(input: &str) -> String {
    let mut base_url = input.to_string();
    while base_url.ends_with('/') {
        base_url.pop();
    }
    if (base_url.starts_with("https://chatgpt.com")
        || base_url.starts_with("https://chat.openai.com"))
        && !base_url.contains("/backend-api")
    {
        base_url = format!("{base_url}/backend-api");
    }
    base_url
}

async fn resolve_auth() -> Result<TranscriptionAuthContext, String> {
    let codex_home = find_codex_home().map_err(|e| format!("failed to find codex home: {e}"))?;
    let auth = CodexAuth::from_auth_storage(&codex_home, AuthCredentialsStoreMode::Auto)
        .map_err(|e| format!("failed to read auth.json: {e}"))?
        .ok_or_else(|| "No Codex auth is configured; please run `codex login`".to_string())?;

    let chatgpt_account_id = auth.get_account_id();

    let token = auth
        .get_token()
        .map_err(|e| format!("failed to get auth token: {e}"))?;
    let config = Config::load_with_cli_overrides(Vec::new())
        .await
        .map_err(|e| format!("failed to load config: {e}"))?;
    Ok(TranscriptionAuthContext {
        mode: auth.api_auth_mode(),
        bearer_token: token,
        chatgpt_account_id,
        chatgpt_base_url: normalize_chatgpt_base_url(&config.chatgpt_base_url),
    })
}

async fn transcribe_bytes(
    wav_bytes: Vec<u8>,
    context: Option<String>,
    duration_seconds: f32,
) -> Result<String, String> {
    let auth = resolve_auth().await?;
    let client = reqwest::Client::new();
    let audio_bytes = wav_bytes.len();
    let prompt_for_log = context.as_deref().unwrap_or("").to_string();
    let (endpoint, request) =
        if matches!(auth.mode, AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens) {
            let part = reqwest::multipart::Part::bytes(wav_bytes)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| format!("failed to set mime: {e}"))?;
            let form = reqwest::multipart::Form::new().part("file", part);
            let endpoint = format!("{}/transcribe", auth.chatgpt_base_url);
            let mut req = client
                .post(&endpoint)
                .bearer_auth(&auth.bearer_token)
                .multipart(form)
                .header("User-Agent", get_codex_user_agent());
            if let Some(acc) = auth.chatgpt_account_id {
                req = req.header("ChatGPT-Account-Id", acc);
            }
            (endpoint, req)
        } else {
            let part = reqwest::multipart::Part::bytes(wav_bytes)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| format!("failed to set mime: {e}"))?;
            let mut form = reqwest::multipart::Form::new()
                .text("model", AUDIO_MODEL)
                .part("file", part);
            if let Some(context) = context {
                form = form.text("prompt", context);
            }
            let endpoint = "https://api.openai.com/v1/audio/transcriptions".to_string();
            (
                endpoint,
                client
                    .post("https://api.openai.com/v1/audio/transcriptions")
                    .bearer_auth(&auth.bearer_token)
                    .multipart(form)
                    .header("User-Agent", get_codex_user_agent()),
            )
        };

    let audio_kib = audio_bytes as f32 / 1024.0;
    let mode = auth.mode;
    let prompt_chars = prompt_for_log.chars().count();
    info!(
        ?mode,
        endpoint,
        duration_seconds,
        audio_kib,
        prompt_chars,
        model = AUDIO_MODEL,
        "sending transcription request"
    );

    let resp = request
        .send()
        .await
        .map_err(|e| format!("transcription request failed: {e}"))?;
    let status = resp.status();
    trace!(%status, "received transcription response");

    if !status.is_success() {
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read body>".to_string());
        error!(%status, body, "transcription request returned error");
        return Err(format!("transcription failed: {status} {body}"));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse json: {e}"))?;
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        warn!("transcription response was empty");
        Err("empty transcription result".to_string())
    } else {
        debug!(
            transcript_chars = text.chars().count(),
            "parsed transcription response"
        );
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::RecordedAudio;
    use super::encode_wav_normalized;
    use pretty_assertions::assert_eq;
    use std::io::Cursor;

    #[test]
    fn encode_wav_normalized_outputs_24khz_mono_audio() {
        let audio = RecordedAudio {
            data: vec![100, 300, 200, 400],
            sample_rate: 24_000,
            channels: 1,
        };

        let wav = encode_wav_normalized(&audio).expect("wav should encode");
        let reader = hound::WavReader::new(Cursor::new(wav)).expect("wav should parse");
        let spec = reader.spec();
        let samples = reader
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .expect("samples should decode");

        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(samples, vec![7_373, 22_118, 14_745, 29_490]);
    }
}
