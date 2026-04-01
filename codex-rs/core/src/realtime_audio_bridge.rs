use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_api::RealtimeAudioFrame;
use codex_api::RealtimeEvent;
use codex_api::endpoint::realtime_webrtc::RealtimeWebrtcWriter;
use std::collections::VecDeque;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::MissedTickBehavior;
use tracing::error;
use tracing::warn;

// Buffer three 20 ms frames before playout, then cap at six frames so remote
// jitter does not turn into unbounded latency.
const TARGET_BUFFER_MS: u64 = 60;
const MAX_BUFFER_MS: u64 = 120;
const PACE_TICK_MS: u64 = 5;
const DEFAULT_FRAME_DURATION_MS: u64 = 20;

struct BufferedAudioFrame {
    frame: RealtimeAudioFrame,
    duration_ms: u64,
}

#[derive(Clone, Copy)]
struct AudioFrameShape {
    sample_rate: u32,
    num_channels: u16,
    samples_per_channel: u32,
}

pub(crate) fn spawn_realtime_audio_bridge(
    writer: RealtimeWebrtcWriter,
    audio_rx: async_channel::Receiver<RealtimeAudioFrame>,
    events_tx: async_channel::Sender<RealtimeEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut pending_frames = VecDeque::<BufferedAudioFrame>::new();
        let mut buffered_ms = 0_u64;
        let mut started = false;
        let mut next_send_at: Option<Instant> = None;
        let mut last_frame_shape: Option<AudioFrameShape> = None;
        let mut audio_rx_closed = false;
        let mut underrun_reported = false;
        let mut pace_tick = tokio::time::interval(Duration::from_millis(PACE_TICK_MS));
        pace_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                frame = audio_rx.recv(), if !audio_rx_closed => {
                    match frame {
                        Ok(frame) => {
                            let duration_ms = frame_duration_ms(&frame);
                            if let Some(frame_shape) = AudioFrameShape::from_frame(&frame) {
                                last_frame_shape = Some(frame_shape);
                            }
                            pending_frames.push_back(BufferedAudioFrame {
                                frame,
                                duration_ms,
                            });
                            buffered_ms = buffered_ms.saturating_add(duration_ms);

                            let mut dropped_frames = 0_u64;
                            while buffered_ms > MAX_BUFFER_MS {
                                let Some(frame) = pending_frames.pop_front() else {
                                    break;
                                };
                                buffered_ms = buffered_ms.saturating_sub(frame.duration_ms);
                                dropped_frames = dropped_frames.saturating_add(1);
                            }
                            if dropped_frames > 0 {
                                warn!(
                                    dropped_frames,
                                    buffered_ms,
                                    "dropping buffered input audio to keep realtime latency bounded"
                                );
                            }

                            if !started && buffered_ms >= TARGET_BUFFER_MS {
                                started = true;
                                next_send_at = Some(Instant::now());
                            }
                        }
                        Err(_) => {
                            audio_rx_closed = true;
                        }
                    }
                }
                _ = pace_tick.tick() => {
                    let Some(send_at) = next_send_at else {
                        if audio_rx_closed {
                            break;
                        }
                        continue;
                    };
                    if Instant::now() < send_at {
                        continue;
                    }

                    let frame = if let Some(frame) = pending_frames.pop_front() {
                        buffered_ms = buffered_ms.saturating_sub(frame.duration_ms);
                        underrun_reported = false;
                        frame.frame
                    } else if let Some(frame_shape) = last_frame_shape {
                        if !underrun_reported {
                            warn!("realtime audio bridge underrun; inserting silence");
                            underrun_reported = true;
                        }
                        frame_shape.silence_frame()
                    } else if audio_rx_closed {
                        break;
                    } else {
                        continue;
                    };

                    let duration_ms = frame_duration_ms(&frame);
                    if let Err(err) = writer.send_audio_frame(frame).await {
                        error!("failed to send bridged realtime audio: {err}");
                        let _ = events_tx.send(RealtimeEvent::Error(err.to_string())).await;
                        break;
                    }
                    next_send_at = Some(send_at + Duration::from_millis(duration_ms));

                    if audio_rx_closed && pending_frames.is_empty() {
                        break;
                    }
                }
            }
        }
    })
}

impl AudioFrameShape {
    fn from_frame(frame: &RealtimeAudioFrame) -> Option<Self> {
        Some(Self {
            sample_rate: frame.sample_rate,
            num_channels: frame.num_channels,
            samples_per_channel: frame
                .samples_per_channel
                .or(decoded_samples_per_channel(frame))?,
        })
    }

    fn silence_frame(self) -> RealtimeAudioFrame {
        let byte_len = usize::from(self.num_channels)
            .saturating_mul(self.samples_per_channel as usize)
            .saturating_mul(2);
        RealtimeAudioFrame {
            data: BASE64_STANDARD.encode(vec![0_u8; byte_len]),
            sample_rate: self.sample_rate,
            num_channels: self.num_channels,
            samples_per_channel: Some(self.samples_per_channel),
            item_id: None,
        }
    }
}

fn frame_duration_ms(frame: &RealtimeAudioFrame) -> u64 {
    u64::from(audio_duration_ms(frame)).max(DEFAULT_FRAME_DURATION_MS)
}

fn audio_duration_ms(frame: &RealtimeAudioFrame) -> u32 {
    let Some(samples_per_channel) = frame
        .samples_per_channel
        .or(decoded_samples_per_channel(frame))
    else {
        return 0;
    };
    let sample_rate = u64::from(frame.sample_rate.max(1));
    ((u64::from(samples_per_channel) * 1_000) / sample_rate) as u32
}

fn decoded_samples_per_channel(frame: &RealtimeAudioFrame) -> Option<u32> {
    let bytes = BASE64_STANDARD.decode(&frame.data).ok()?;
    let channels = usize::from(frame.num_channels.max(1));
    let samples = bytes.len().checked_div(2)?.checked_div(channels)?;
    u32::try_from(samples).ok()
}
