use miniaudio::Backend;
use miniaudio::Context;
use miniaudio::Device;
use miniaudio::DeviceConfig;
use miniaudio::DeviceId;
use miniaudio::DeviceIdAndName;
use miniaudio::DeviceType;
use miniaudio::Format;
use miniaudio::Frames;
use miniaudio::FramesMut;
use miniaudio::ResampleAlgorithm;
use std::collections::VecDeque;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use tracing::debug;
use tracing::info;
use tracing::warn;

pub const AUDIO_SAMPLE_RATE: u32 = 24_000;
pub const AUDIO_CHANNELS: u32 = 1;
const PERIOD_SIZE_IN_FRAMES: u32 = 240;
const SPEEX_RESAMPLE_QUALITY: u32 = 5;
const MAX_OUTPUT_QUEUE_SAMPLES: usize = (AUDIO_SAMPLE_RATE as usize) * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDeviceKind {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub backend: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedAudio {
    pub data: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

type ChunkCallback = Arc<Mutex<Box<dyn FnMut(Vec<i16>) + Send>>>;

pub fn list_audio_devices(kind: AudioDeviceKind) -> Result<Vec<AudioDeviceInfo>, String> {
    let context = create_context()?;
    let backend = backend_label(context.backend());
    let mut devices = Vec::new();

    match kind {
        AudioDeviceKind::Input => {
            context
                .with_capture_devices(|infos| {
                    extend_device_infos(&mut devices, infos, backend.as_str());
                })
                .map_err(|err| format!("failed to enumerate input audio devices: {err}"))?;
        }
        AudioDeviceKind::Output => {
            context
                .with_playback_devices(|infos| {
                    extend_device_infos(&mut devices, infos, backend.as_str());
                })
                .map_err(|err| format!("failed to enumerate output audio devices: {err}"))?;
        }
    }

    devices.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    debug!(
        kind = %audio_device_kind_label(kind),
        backend = %backend,
        count = devices.len(),
        "enumerated audio devices"
    );
    Ok(devices)
}

pub fn resolve_device_name(
    kind: AudioDeviceKind,
    device_id: &str,
) -> Result<Option<String>, String> {
    let devices = list_audio_devices(kind)?;
    let resolved = devices
        .into_iter()
        .find(|device| device.id == device_id)
        .map(|device| device.name);
    debug!(
        kind = %audio_device_kind_label(kind),
        device_id,
        resolved = resolved.is_some(),
        "resolved persisted audio device"
    );
    Ok(resolved)
}

pub struct InputCapture {
    _device: Device,
    data: Arc<Mutex<Vec<i16>>>,
    stopped: Arc<AtomicBool>,
    last_peak: Arc<AtomicU16>,
}

impl InputCapture {
    pub fn start_recording(device_id: Option<&str>) -> Result<Self, String> {
        Self::start(device_id, true, None)
    }

    pub fn start_streaming<F>(device_id: Option<&str>, callback: F) -> Result<Self, String>
    where
        F: FnMut(Vec<i16>) + Send + 'static,
    {
        Self::start(
            device_id,
            false,
            Some(Arc::new(Mutex::new(Box::new(callback)))),
        )
    }

    fn start(
        device_id: Option<&str>,
        collect_audio: bool,
        callback: Option<ChunkCallback>,
    ) -> Result<Self, String> {
        let context = create_context()?;
        let backend = backend_label(context.backend());
        let mut config = DeviceConfig::new(DeviceType::Capture);
        config.set_sample_rate(AUDIO_SAMPLE_RATE);
        config.set_period_size_in_frames(PERIOD_SIZE_IN_FRAMES);
        config.set_resampling(ResampleAlgorithm::Speex {
            quality: SPEEX_RESAMPLE_QUALITY,
        });
        config.capture_mut().set_format(Format::S16);
        config.capture_mut().set_channels(AUDIO_CHANNELS);
        if let Some(device_id) = device_id {
            config
                .capture_mut()
                .set_device_id(Some(decode_device_id(device_id)?));
        }

        let data = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let last_peak = Arc::new(AtomicU16::new(0));

        let data_for_callback = Arc::clone(&data);
        let callback_for_callback = callback.clone();
        let last_peak_for_callback = Arc::clone(&last_peak);
        config.set_data_callback(move |_device, _output: &mut FramesMut, input: &Frames| {
            let samples = input.as_samples::<i16>();
            if samples.is_empty() {
                return;
            }

            last_peak_for_callback.store(peak_i16(samples), Ordering::Relaxed);

            if collect_audio && let Ok(mut guard) = data_for_callback.lock() {
                guard.extend_from_slice(samples);
            }

            if let Some(callback) = callback_for_callback.as_ref()
                && let Ok(mut callback) = callback.lock()
            {
                callback(samples.to_vec());
            }
        });

        let device = Device::new(Some(context), &config)
            .map_err(|err| format!("failed to open input audio device: {err}"))?;
        device
            .start()
            .map_err(|err| format!("failed to start input audio stream: {err}"))?;

        info!(
            mode = if collect_audio {
                "recording"
            } else {
                "realtime_streaming"
            },
            backend = %backend,
            device_id = device_id.unwrap_or("system_default"),
            sample_rate = AUDIO_SAMPLE_RATE,
            channels = AUDIO_CHANNELS,
            period_frames = PERIOD_SIZE_IN_FRAMES,
            "started input audio stream"
        );

        Ok(Self {
            _device: device,
            data,
            stopped,
            last_peak,
        })
    }

    pub fn stop(self) -> Result<RecordedAudio, String> {
        self.stopped.store(true, Ordering::SeqCst);
        self._device
            .stop()
            .map_err(|err| format!("failed to stop input audio stream: {err}"))?;
        let data = self
            .data
            .lock()
            .map_err(|_| "failed to lock audio buffer".to_string())?
            .clone();
        let peak = peak_i16(&data);
        let duration_ms = if AUDIO_SAMPLE_RATE > 0 {
            (data.len() as u64 * 1_000) / u64::from(AUDIO_SAMPLE_RATE)
        } else {
            0
        };
        info!(
            sample_count = data.len(),
            duration_ms,
            peak,
            sample_rate = AUDIO_SAMPLE_RATE,
            channels = AUDIO_CHANNELS,
            "stopped input audio stream"
        );
        Ok(RecordedAudio {
            data,
            sample_rate: AUDIO_SAMPLE_RATE,
            channels: AUDIO_CHANNELS as u16,
        })
    }

    pub fn data_arc(&self) -> Arc<Mutex<Vec<i16>>> {
        Arc::clone(&self.data)
    }

    pub fn stopped_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }

    pub fn last_peak_arc(&self) -> Arc<AtomicU16> {
        Arc::clone(&self.last_peak)
    }

    pub fn sample_rate(&self) -> u32 {
        AUDIO_SAMPLE_RATE
    }

    pub fn channels(&self) -> u16 {
        AUDIO_CHANNELS as u16
    }
}

pub struct OutputPlayback {
    _device: Device,
    queue: Arc<Mutex<VecDeque<i16>>>,
}

impl OutputPlayback {
    pub fn start(device_id: Option<&str>) -> Result<Self, String> {
        let context = create_context()?;
        let backend = backend_label(context.backend());
        let mut config = DeviceConfig::new(DeviceType::Playback);
        config.set_sample_rate(AUDIO_SAMPLE_RATE);
        config.set_period_size_in_frames(PERIOD_SIZE_IN_FRAMES);
        config.set_resampling(ResampleAlgorithm::Speex {
            quality: SPEEX_RESAMPLE_QUALITY,
        });
        config.playback_mut().set_format(Format::S16);
        config.playback_mut().set_channels(AUDIO_CHANNELS);
        if let Some(device_id) = device_id {
            config
                .playback_mut()
                .set_device_id(Some(decode_device_id(device_id)?));
        }

        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let queue_for_callback = Arc::clone(&queue);
        config.set_data_callback(move |_device, output: &mut FramesMut, _input: &Frames| {
            fill_output_samples(output, &queue_for_callback);
        });

        let device = Device::new(Some(context), &config)
            .map_err(|err| format!("failed to open output audio device: {err}"))?;
        device
            .start()
            .map_err(|err| format!("failed to start output audio stream: {err}"))?;

        info!(
            backend = %backend,
            device_id = device_id.unwrap_or("system_default"),
            sample_rate = AUDIO_SAMPLE_RATE,
            channels = AUDIO_CHANNELS,
            period_frames = PERIOD_SIZE_IN_FRAMES,
            "started output audio stream"
        );

        Ok(Self {
            _device: device,
            queue,
        })
    }

    pub fn enqueue_samples(&self, samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }

        let mut guard = self
            .queue
            .lock()
            .map_err(|_| "failed to lock output audio queue".to_string())?;
        let overflow = guard
            .len()
            .saturating_add(samples.len())
            .saturating_sub(MAX_OUTPUT_QUEUE_SAMPLES);
        if overflow > 0 {
            let current_len = guard.len();
            guard.drain(..overflow.min(current_len));
            warn!(
                dropped_samples = overflow.min(current_len),
                queue_samples_before = current_len,
                queue_samples_after = guard.len(),
                max_queue_samples = MAX_OUTPUT_QUEUE_SAMPLES,
                "trimmed output audio queue due to backlog"
            );
        }
        guard.extend(samples.iter().copied());
        Ok(())
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.queue.lock() {
            debug!(cleared_samples = guard.len(), "cleared output audio queue");
            guard.clear();
        }
    }
}

fn extend_device_infos(
    devices: &mut Vec<AudioDeviceInfo>,
    infos: &[DeviceIdAndName],
    backend: &str,
) {
    for info in infos {
        devices.push(AudioDeviceInfo {
            id: encode_device_id(info.id()),
            name: info.name().to_string(),
            backend: backend.to_string(),
        });
    }
}

fn create_context() -> Result<Context, String> {
    Context::new(preferred_backends(), None)
        .map_err(|err| format!("failed to initialize audio backend: {err}"))
}

fn preferred_backends() -> &'static [Backend] {
    #[cfg(target_os = "linux")]
    {
        &[Backend::PulseAudio, Backend::Alsa, Backend::Jack]
    }
    #[cfg(target_os = "macos")]
    {
        &[Backend::CoreAudio]
    }
    #[cfg(target_os = "windows")]
    {
        &[Backend::Wasapi, Backend::DSound, Backend::WinMM]
    }
}

fn backend_label(backend: Backend) -> String {
    match backend {
        Backend::PulseAudio => "PulseAudio".to_string(),
        Backend::Alsa => "ALSA".to_string(),
        Backend::Jack => "JACK".to_string(),
        Backend::CoreAudio => "CoreAudio".to_string(),
        Backend::Wasapi => "WASAPI".to_string(),
        Backend::DSound => "DirectSound".to_string(),
        Backend::WinMM => "WinMM".to_string(),
        Backend::SNDIO => "sndio".to_string(),
        Backend::Audio4 => "audio4".to_string(),
        Backend::OSS => "OSS".to_string(),
        Backend::AAudio => "AAudio".to_string(),
        Backend::OpenSL => "OpenSL".to_string(),
        Backend::WebAudio => "WebAudio".to_string(),
        Backend::Null => "Null".to_string(),
    }
}

fn audio_device_kind_label(kind: AudioDeviceKind) -> &'static str {
    match kind {
        AudioDeviceKind::Input => "input",
        AudioDeviceKind::Output => "output",
    }
}

fn encode_device_id(device_id: &DeviceId) -> String {
    hex_encode(device_id_bytes(device_id))
}

fn decode_device_id(encoded: &str) -> Result<DeviceId, String> {
    let bytes = hex_decode(encoded)?;
    if bytes.len() != std::mem::size_of::<DeviceId>() {
        return Err("invalid persisted audio device id".to_string());
    }

    let mut device_id = MaybeUninit::<DeviceId>::uninit();
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            device_id.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
        Ok(device_id.assume_init())
    }
}

fn device_id_bytes(device_id: &DeviceId) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            (device_id as *const DeviceId).cast::<u8>(),
            std::mem::size_of::<DeviceId>(),
        )
    }
}

fn fill_output_samples(output: &mut FramesMut, queue: &Arc<Mutex<VecDeque<i16>>>) {
    let output_samples = output.as_samples_mut::<i16>();
    if let Ok(mut guard) = queue.lock() {
        for sample in output_samples {
            *sample = guard.pop_front().unwrap_or(0);
        }
        return;
    }
    output_samples.fill(0);
}

fn peak_i16(samples: &[i16]) -> u16 {
    let mut peak: i32 = 0;
    for &sample in samples {
        let absolute = (sample as i32).unsigned_abs() as i32;
        if absolute > peak {
            peak = absolute;
        }
    }
    peak as u16
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(encoded: &str) -> Result<Vec<u8>, String> {
    if !encoded.len().is_multiple_of(2) {
        return Err("invalid persisted audio device id".to_string());
    }

    let mut decoded = Vec::with_capacity(encoded.len() / 2);
    let bytes = encoded.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let high = decode_hex_nibble(bytes[index])?;
        let low = decode_hex_nibble(bytes[index + 1])?;
        decoded.push((high << 4) | low);
        index += 2;
    }
    Ok(decoded)
}

fn decode_hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid persisted audio device id".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::hex_decode;
    use super::hex_encode;
    use pretty_assertions::assert_eq;

    #[test]
    fn hex_round_trip() {
        let bytes = [0x00, 0x10, 0xab, 0xff];
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded, "0010abff");
        assert_eq!(hex_decode(&encoded).expect("hex decode"), bytes);
    }

    #[test]
    fn hex_decode_rejects_invalid_input() {
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
