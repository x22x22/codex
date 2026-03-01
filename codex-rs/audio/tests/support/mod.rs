use codex_audio::AudioDeviceKind;
use codex_audio::list_audio_devices;

pub(crate) fn first_device_id(kind: AudioDeviceKind) -> Option<String> {
    match list_audio_devices(kind) {
        Ok(devices) => devices.into_iter().next().map(|device| device.id),
        Err(_) => None,
    }
}

pub(crate) fn skip_without_device(kind: AudioDeviceKind) {
    let label = match kind {
        AudioDeviceKind::Input => "input",
        AudioDeviceKind::Output => "output",
    };
    eprintln!("skipping host audio smoke test: no {label} device available");
}

pub(crate) fn skip_without_backend(kind: AudioDeviceKind, error: &str) {
    let label = match kind {
        AudioDeviceKind::Input => "input",
        AudioDeviceKind::Output => "output",
    };
    eprintln!("skipping host audio smoke test: could not enumerate {label} devices: {error}");
}
