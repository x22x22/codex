mod backend;
mod miniaudio_backend;

use crate::backend::AudioBackend;
use crate::backend::InputOpenRequest;
use crate::backend::InputStreamHandle;
use crate::backend::OutputOpenRequest;
use crate::backend::OutputStreamHandle;
use crate::miniaudio_backend::MiniaudioBackend;
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
pub(crate) const SPEEX_RESAMPLE_QUALITY: u32 = 5;
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
    let backend = MiniaudioBackend;
    let backend_name = backend.backend_name();
    let devices = backend.list_devices(kind)?;
    debug!(
        kind = %audio_device_kind_label(kind),
        backend = %backend_name,
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
    stream: Box<dyn InputStreamHandle>,
    data: Arc<Mutex<Vec<i16>>>,
    stopped: Arc<AtomicBool>,
    last_peak: Arc<AtomicU16>,
}

impl InputCapture {
    pub fn start_recording(device_id: Option<&str>) -> Result<Self, String> {
        Self::start_with_backend(&MiniaudioBackend, device_id, true, None::<fn(Vec<i16>)>)
    }

    pub fn start_streaming<F>(device_id: Option<&str>, callback: F) -> Result<Self, String>
    where
        F: FnMut(Vec<i16>) + Send + 'static,
    {
        Self::start_with_backend(&MiniaudioBackend, device_id, false, Some(callback))
    }

    fn start_with_backend<F>(
        backend: &dyn AudioBackend,
        device_id: Option<&str>,
        collect_audio: bool,
        callback: Option<F>,
    ) -> Result<Self, String>
    where
        F: FnMut(Vec<i16>) + Send + 'static,
    {
        let data = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let last_peak = Arc::new(AtomicU16::new(0));
        let request = InputOpenRequest {
            device_id: device_id.map(ToString::to_string),
            sample_rate: AUDIO_SAMPLE_RATE,
            channels: AUDIO_CHANNELS,
            period_frames: PERIOD_SIZE_IN_FRAMES,
        };

        let callback: Option<ChunkCallback> = callback.map(|callback| {
            Arc::new(Mutex::new(
                Box::new(callback) as Box<dyn FnMut(Vec<i16>) + Send>
            ))
        });
        let data_for_callback = Arc::clone(&data);
        let last_peak_for_callback = Arc::clone(&last_peak);
        let chunk_callback = callback.clone();
        let stream = backend.open_input(
            &request,
            Arc::new(move |samples: &[i16]| {
                if samples.is_empty() {
                    return;
                }

                last_peak_for_callback.store(peak_i16(samples), Ordering::Relaxed);

                if collect_audio && let Ok(mut guard) = data_for_callback.lock() {
                    guard.extend_from_slice(samples);
                }

                if let Some(callback) = chunk_callback.as_ref()
                    && let Ok(mut callback) = callback.lock()
                {
                    callback(samples.to_vec());
                }
            }),
        )?;

        info!(
            mode = if collect_audio {
                "recording"
            } else {
                "realtime_streaming"
            },
            backend = %backend.backend_name(),
            device_id = device_id.unwrap_or("system_default"),
            sample_rate = AUDIO_SAMPLE_RATE,
            channels = AUDIO_CHANNELS,
            period_frames = PERIOD_SIZE_IN_FRAMES,
            "started input audio stream"
        );

        Ok(Self {
            stream,
            data,
            stopped,
            last_peak,
        })
    }

    pub fn stop(mut self) -> Result<RecordedAudio, String> {
        self.stopped.store(true, Ordering::SeqCst);
        self.stream.stop()?;
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

impl Drop for InputCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop();
    }
}

pub struct OutputPlayback {
    stream: Box<dyn OutputStreamHandle>,
    queue: Arc<Mutex<OutputSampleQueue>>,
}

impl OutputPlayback {
    pub fn start(device_id: Option<&str>) -> Result<Self, String> {
        Self::start_with_backend(&MiniaudioBackend, device_id)
    }

    fn start_with_backend(
        backend: &dyn AudioBackend,
        device_id: Option<&str>,
    ) -> Result<Self, String> {
        let request = OutputOpenRequest {
            device_id: device_id.map(ToString::to_string),
            sample_rate: AUDIO_SAMPLE_RATE,
            channels: AUDIO_CHANNELS,
            period_frames: PERIOD_SIZE_IN_FRAMES,
        };
        let queue = Arc::new(Mutex::new(OutputSampleQueue::new(MAX_OUTPUT_QUEUE_SAMPLES)));
        let queue_for_callback = Arc::clone(&queue);
        let stream = backend.open_output(
            &request,
            Arc::new(move |output: &mut [i16]| {
                if let Ok(mut guard) = queue_for_callback.lock() {
                    guard.drain_into(output);
                    return;
                }
                output.fill(0);
            }),
        )?;

        info!(
            backend = %backend.backend_name(),
            device_id = device_id.unwrap_or("system_default"),
            sample_rate = AUDIO_SAMPLE_RATE,
            channels = AUDIO_CHANNELS,
            period_frames = PERIOD_SIZE_IN_FRAMES,
            "started output audio stream"
        );

        Ok(Self { stream, queue })
    }

    pub fn enqueue_samples(&self, samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }

        let mut guard = self
            .queue
            .lock()
            .map_err(|_| "failed to lock output audio queue".to_string())?;
        let queue_len_before = guard.len();
        let result = guard.enqueue(samples);
        if result.dropped_samples > 0 {
            warn!(
                dropped_samples = result.dropped_samples,
                queue_samples_before = queue_len_before,
                queue_samples_after = result.queue_len_after,
                max_queue_samples = MAX_OUTPUT_QUEUE_SAMPLES,
                "trimmed output audio queue due to backlog"
            );
        }
        Ok(())
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.queue.lock() {
            debug!(
                cleared_samples = guard.clear(),
                "cleared output audio queue"
            );
        }
    }
}

impl Drop for OutputPlayback {
    fn drop(&mut self) {
        let _ = self.stream.stop();
    }
}

#[derive(Debug)]
struct OutputSampleQueue {
    samples: VecDeque<i16>,
    max_samples: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueueEnqueueResult {
    dropped_samples: usize,
    queue_len_after: usize,
}

impl OutputSampleQueue {
    fn new(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            max_samples,
        }
    }

    fn enqueue(&mut self, samples: &[i16]) -> QueueEnqueueResult {
        let dropped_samples = self
            .samples
            .len()
            .saturating_add(samples.len())
            .saturating_sub(self.max_samples)
            .min(self.samples.len());
        if dropped_samples > 0 {
            self.samples.drain(..dropped_samples);
        }
        self.samples.extend(samples.iter().copied());
        QueueEnqueueResult {
            dropped_samples,
            queue_len_after: self.samples.len(),
        }
    }

    fn drain_into(&mut self, output: &mut [i16]) {
        for sample in output {
            *sample = self.samples.pop_front().unwrap_or(0);
        }
    }

    fn clear(&mut self) -> usize {
        let cleared = self.samples.len();
        self.samples.clear();
        cleared
    }

    fn len(&self) -> usize {
        self.samples.len()
    }
}

fn audio_device_kind_label(kind: AudioDeviceKind) -> &'static str {
    match kind {
        AudioDeviceKind::Input => "input",
        AudioDeviceKind::Output => "output",
    }
}

fn encode_device_id(device_id: &miniaudio::DeviceId) -> String {
    hex_encode(device_id_bytes(device_id))
}

fn decode_device_id(encoded: &str) -> Result<miniaudio::DeviceId, String> {
    let bytes = hex_decode(encoded)?;
    if bytes.len() != std::mem::size_of::<miniaudio::DeviceId>() {
        return Err("invalid persisted audio device id".to_string());
    }

    let mut device_id = MaybeUninit::<miniaudio::DeviceId>::uninit();
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            device_id.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
        Ok(device_id.assume_init())
    }
}

fn device_id_bytes(device_id: &miniaudio::DeviceId) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            (device_id as *const miniaudio::DeviceId).cast::<u8>(),
            std::mem::size_of::<miniaudio::DeviceId>(),
        )
    }
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
    use super::AUDIO_CHANNELS;
    use super::AUDIO_SAMPLE_RATE;
    use super::AudioDeviceInfo;
    use super::AudioDeviceKind;
    use super::InputCapture;
    use super::OutputPlayback;
    use super::OutputSampleQueue;
    use super::PERIOD_SIZE_IN_FRAMES;
    use super::QueueEnqueueResult;
    use super::RecordedAudio;
    use super::backend::AudioBackend;
    use super::backend::InputDataCallback;
    use super::backend::InputOpenRequest;
    use super::backend::InputStreamHandle;
    use super::backend::OutputDataCallback;
    use super::backend::OutputOpenRequest;
    use super::backend::OutputStreamHandle;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct FakeOpenRequest {
        device_id: Option<String>,
        sample_rate: u32,
        channels: u32,
        period_frames: u32,
    }

    #[derive(Default)]
    struct FakeAudioBackend {
        state: Mutex<FakeAudioBackendState>,
    }

    #[derive(Default)]
    struct FakeAudioBackendState {
        input_devices: Vec<AudioDeviceInfo>,
        output_devices: Vec<AudioDeviceInfo>,
        input_open_requests: Vec<FakeOpenRequest>,
        output_open_requests: Vec<FakeOpenRequest>,
        next_input_error: Option<String>,
        next_output_error: Option<String>,
        input_callbacks: Vec<InputDataCallback>,
        output_callbacks: Vec<OutputDataCallback>,
    }

    impl FakeAudioBackend {
        fn with_devices(
            input_devices: Vec<AudioDeviceInfo>,
            output_devices: Vec<AudioDeviceInfo>,
        ) -> Self {
            Self {
                state: Mutex::new(FakeAudioBackendState {
                    input_devices,
                    output_devices,
                    ..FakeAudioBackendState::default()
                }),
            }
        }

        fn push_capture_samples(&self, samples: &[i16]) {
            let callbacks = self
                .state
                .lock()
                .expect("backend state")
                .input_callbacks
                .clone();
            for callback in callbacks {
                callback(samples);
            }
        }

        fn drain_playback(&self, len: usize) -> Vec<i16> {
            let callbacks = self
                .state
                .lock()
                .expect("backend state")
                .output_callbacks
                .clone();
            let mut output = vec![0; len];
            for callback in callbacks {
                callback(&mut output);
            }
            output
        }

        fn fail_next_input_open(&self, error: &str) {
            self.state.lock().expect("backend state").next_input_error = Some(error.to_string());
        }

        fn input_open_requests(&self) -> Vec<FakeOpenRequest> {
            self.state
                .lock()
                .expect("backend state")
                .input_open_requests
                .clone()
        }

        fn output_open_requests(&self) -> Vec<FakeOpenRequest> {
            self.state
                .lock()
                .expect("backend state")
                .output_open_requests
                .clone()
        }
    }

    impl AudioBackend for FakeAudioBackend {
        fn backend_name(&self) -> String {
            "FakeAudio".to_string()
        }

        fn list_devices(&self, kind: AudioDeviceKind) -> Result<Vec<AudioDeviceInfo>, String> {
            let state = self.state.lock().expect("backend state");
            Ok(match kind {
                AudioDeviceKind::Input => state.input_devices.clone(),
                AudioDeviceKind::Output => state.output_devices.clone(),
            })
        }

        fn open_input(
            &self,
            request: &InputOpenRequest,
            callback: InputDataCallback,
        ) -> Result<Box<dyn InputStreamHandle>, String> {
            let mut state = self.state.lock().expect("backend state");
            if let Some(error) = state.next_input_error.take() {
                return Err(error);
            }
            state.input_open_requests.push(FakeOpenRequest {
                device_id: request.device_id.clone(),
                sample_rate: request.sample_rate,
                channels: request.channels,
                period_frames: request.period_frames,
            });
            state.input_callbacks.push(callback);
            Ok(Box::new(FakeInputStream))
        }

        fn open_output(
            &self,
            request: &OutputOpenRequest,
            callback: OutputDataCallback,
        ) -> Result<Box<dyn OutputStreamHandle>, String> {
            let mut state = self.state.lock().expect("backend state");
            if let Some(error) = state.next_output_error.take() {
                return Err(error);
            }
            state.output_open_requests.push(FakeOpenRequest {
                device_id: request.device_id.clone(),
                sample_rate: request.sample_rate,
                channels: request.channels,
                period_frames: request.period_frames,
            });
            state.output_callbacks.push(callback);
            Ok(Box::new(FakeOutputStream {
                state: Arc::new(Mutex::new(())),
            }))
        }
    }

    struct FakeInputStream;

    impl InputStreamHandle for FakeInputStream {
        fn stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    struct FakeOutputStream {
        state: Arc<Mutex<()>>,
    }

    impl OutputStreamHandle for FakeOutputStream {
        fn stop(&mut self) -> Result<(), String> {
            let _guard = self.state.lock().expect("output stream state");
            Ok(())
        }
    }

    #[test]
    fn hex_round_trip() {
        let bytes = [0x00, 0x10, 0xab, 0xff];
        let encoded = super::hex_encode(&bytes);
        assert_eq!(encoded, "0010abff");
        assert_eq!(super::hex_decode(&encoded).expect("hex decode"), bytes);
    }

    #[test]
    fn hex_decode_rejects_invalid_input() {
        assert!(super::hex_decode("abc").is_err());
        assert!(super::hex_decode("zz").is_err());
    }

    #[test]
    fn output_sample_queue_trims_oldest_samples() {
        let mut queue = OutputSampleQueue::new(4);
        let initial = queue.enqueue(&[1, 2, 3]);
        let overflow = queue.enqueue(&[4, 5, 6]);

        assert_eq!(
            initial,
            QueueEnqueueResult {
                dropped_samples: 0,
                queue_len_after: 3,
            }
        );
        assert_eq!(
            overflow,
            QueueEnqueueResult {
                dropped_samples: 2,
                queue_len_after: 4,
            }
        );

        let mut output = [0; 4];
        queue.drain_into(&mut output);
        assert_eq!(output, [3, 4, 5, 6]);
    }

    #[test]
    fn output_sample_queue_zero_fills_on_underrun() {
        let mut queue = OutputSampleQueue::new(8);
        queue.enqueue(&[11, 22]);

        let mut output = [0; 4];
        queue.drain_into(&mut output);

        assert_eq!(output, [11, 22, 0, 0]);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn output_sample_queue_clear_returns_cleared_count() {
        let mut queue = OutputSampleQueue::new(8);
        queue.enqueue(&[1, 2, 3, 4]);

        assert_eq!(queue.clear(), 4);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn peak_i16_handles_negative_samples() {
        assert_eq!(super::peak_i16(&[-7, 3, -200, 1]), 200);
        assert_eq!(super::peak_i16(&[i16::MIN]), i16::MAX as u16 + 1);
    }

    #[test]
    fn fake_backend_records_input_request_and_collects_audio() {
        let backend = FakeAudioBackend::with_devices(
            vec![AudioDeviceInfo {
                id: "usb-mic".to_string(),
                name: "USB Mic".to_string(),
                backend: "FakeAudio".to_string(),
            }],
            Vec::new(),
        );
        let capture =
            InputCapture::start_with_backend(&backend, Some("usb-mic"), true, None::<fn(Vec<i16>)>)
                .expect("capture should start");

        backend.push_capture_samples(&[1, -2, 3, -4]);
        let recorded = capture.stop().expect("capture should stop");

        assert_eq!(
            backend.input_open_requests(),
            vec![FakeOpenRequest {
                device_id: Some("usb-mic".to_string()),
                sample_rate: AUDIO_SAMPLE_RATE,
                channels: AUDIO_CHANNELS,
                period_frames: PERIOD_SIZE_IN_FRAMES,
            }]
        );
        assert_eq!(
            recorded,
            RecordedAudio {
                data: vec![1, -2, 3, -4],
                sample_rate: AUDIO_SAMPLE_RATE,
                channels: AUDIO_CHANNELS as u16,
            }
        );
    }

    #[test]
    fn fake_backend_streaming_callback_receives_samples() {
        let backend = FakeAudioBackend::default();
        let streamed = Arc::new(Mutex::new(Vec::new()));
        let streamed_for_callback = Arc::clone(&streamed);
        let capture = InputCapture::start_with_backend(
            &backend,
            None,
            false,
            Some(move |samples| {
                streamed_for_callback
                    .lock()
                    .expect("streamed samples")
                    .extend(samples);
            }),
        )
        .expect("streaming capture should start");

        backend.push_capture_samples(&[10, 20, 30]);
        let _ = capture.stop().expect("capture should stop");

        assert_eq!(
            streamed.lock().expect("streamed samples").as_slice(),
            [10, 20, 30]
        );
    }

    #[test]
    fn fake_backend_input_open_failure_surfaces() {
        let backend = FakeAudioBackend::default();
        backend.fail_next_input_open("input failed");

        let err = InputCapture::start_with_backend(&backend, None, true, None::<fn(Vec<i16>)>);

        assert_eq!(err.err().as_deref(), Some("input failed"));
    }

    #[test]
    fn fake_backend_records_output_request_and_drains_playback() {
        let backend = FakeAudioBackend::with_devices(
            Vec::new(),
            vec![AudioDeviceInfo {
                id: "desk-speakers".to_string(),
                name: "Desk Speakers".to_string(),
                backend: "FakeAudio".to_string(),
            }],
        );
        let playback = OutputPlayback::start_with_backend(&backend, Some("desk-speakers"))
            .expect("playback should start");

        playback
            .enqueue_samples(&[7, 8, 9])
            .expect("enqueue should succeed");

        assert_eq!(
            backend.output_open_requests(),
            vec![FakeOpenRequest {
                device_id: Some("desk-speakers".to_string()),
                sample_rate: AUDIO_SAMPLE_RATE,
                channels: AUDIO_CHANNELS,
                period_frames: PERIOD_SIZE_IN_FRAMES,
            }]
        );
        assert_eq!(backend.drain_playback(4), vec![7, 8, 9, 0]);
    }
}
