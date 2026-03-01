use crate::AudioDeviceInfo;
use crate::AudioDeviceKind;
use crate::backend::AudioBackend;
use crate::backend::InputDataCallback;
use crate::backend::InputOpenRequest;
use crate::backend::InputStreamHandle;
use crate::backend::OutputDataCallback;
use crate::backend::OutputOpenRequest;
use crate::backend::OutputStreamHandle;
use crate::decode_device_id;
use miniaudio::Backend;
use miniaudio::Context;
use miniaudio::Device;
use miniaudio::DeviceConfig;
use miniaudio::DeviceIdAndName;
use miniaudio::DeviceType;
use miniaudio::Format;
use miniaudio::Frames;
use miniaudio::FramesMut;
use miniaudio::ResampleAlgorithm;

pub(crate) struct MiniaudioBackend;

impl AudioBackend for MiniaudioBackend {
    fn backend_name(&self) -> String {
        match create_context() {
            Ok(context) => backend_label(context.backend()),
            Err(_) => "Unknown".to_string(),
        }
    }

    fn list_devices(&self, kind: AudioDeviceKind) -> Result<Vec<AudioDeviceInfo>, String> {
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
        Ok(devices)
    }

    fn open_input(
        &self,
        request: &InputOpenRequest,
        callback: InputDataCallback,
    ) -> Result<Box<dyn InputStreamHandle>, String> {
        let context = create_context()?;
        let mut config = DeviceConfig::new(DeviceType::Capture);
        config.set_sample_rate(request.sample_rate);
        config.set_period_size_in_frames(request.period_frames);
        config.set_resampling(ResampleAlgorithm::Speex {
            quality: crate::SPEEX_RESAMPLE_QUALITY,
        });
        config.capture_mut().set_format(Format::S16);
        config.capture_mut().set_channels(request.channels);
        if let Some(device_id) = request.device_id.as_deref() {
            config
                .capture_mut()
                .set_device_id(Some(decode_device_id(device_id)?));
        }

        let data_callback = callback.clone();
        config.set_data_callback(move |_device, _output: &mut FramesMut, input: &Frames| {
            let samples = input.as_samples::<i16>();
            if samples.is_empty() {
                return;
            }
            data_callback(samples);
        });

        let device = Device::new(Some(context), &config)
            .map_err(|err| format!("failed to open input audio device: {err}"))?;
        device
            .start()
            .map_err(|err| format!("failed to start input audio stream: {err}"))?;
        Ok(Box::new(MiniaudioInputStream { device }))
    }

    fn open_output(
        &self,
        request: &OutputOpenRequest,
        callback: OutputDataCallback,
    ) -> Result<Box<dyn OutputStreamHandle>, String> {
        let context = create_context()?;
        let mut config = DeviceConfig::new(DeviceType::Playback);
        config.set_sample_rate(request.sample_rate);
        config.set_period_size_in_frames(request.period_frames);
        config.set_resampling(ResampleAlgorithm::Speex {
            quality: crate::SPEEX_RESAMPLE_QUALITY,
        });
        config.playback_mut().set_format(Format::S16);
        config.playback_mut().set_channels(request.channels);
        if let Some(device_id) = request.device_id.as_deref() {
            config
                .playback_mut()
                .set_device_id(Some(decode_device_id(device_id)?));
        }

        let data_callback = callback.clone();
        config.set_data_callback(move |_device, output: &mut FramesMut, _input: &Frames| {
            data_callback(output.as_samples_mut::<i16>());
        });

        let device = Device::new(Some(context), &config)
            .map_err(|err| format!("failed to open output audio device: {err}"))?;
        device
            .start()
            .map_err(|err| format!("failed to start output audio stream: {err}"))?;
        Ok(Box::new(MiniaudioOutputStream { device }))
    }
}

struct MiniaudioInputStream {
    device: Device,
}

impl InputStreamHandle for MiniaudioInputStream {
    fn stop(&mut self) -> Result<(), String> {
        self.device
            .stop()
            .map_err(|err| format!("failed to stop input audio stream: {err}"))
    }
}

struct MiniaudioOutputStream {
    device: Device,
}

impl OutputStreamHandle for MiniaudioOutputStream {
    fn stop(&mut self) -> Result<(), String> {
        self.device
            .stop()
            .map_err(|err| format!("failed to stop output audio stream: {err}"))
    }
}

fn extend_device_infos(
    devices: &mut Vec<AudioDeviceInfo>,
    infos: &[DeviceIdAndName],
    backend: &str,
) {
    for info in infos {
        devices.push(AudioDeviceInfo {
            id: crate::encode_device_id(info.id()),
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
