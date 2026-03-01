mod support;

use codex_audio::AudioDeviceKind;
use codex_audio::InputCapture;
use codex_audio::OutputPlayback;
use codex_audio::list_audio_devices;
use pretty_assertions::assert_eq;
use serial_test::serial;

#[test]
#[serial]
fn enumerates_input_devices_without_error() {
    if let Err(error) = list_audio_devices(AudioDeviceKind::Input) {
        support::skip_without_backend(AudioDeviceKind::Input, &error);
    }
}

#[test]
#[serial]
fn enumerates_output_devices_without_error() {
    if let Err(error) = list_audio_devices(AudioDeviceKind::Output) {
        support::skip_without_backend(AudioDeviceKind::Output, &error);
    }
}

#[test]
#[serial]
fn opens_explicit_input_device_when_available() {
    let Some(device_id) = support::first_device_id(AudioDeviceKind::Input) else {
        support::skip_without_device(AudioDeviceKind::Input);
        return;
    };

    let capture =
        InputCapture::start_recording(Some(&device_id)).expect("explicit input device should open");
    let recorded = capture.stop().expect("explicit input device should stop");

    assert_eq!(recorded.sample_rate, codex_audio::AUDIO_SAMPLE_RATE);
}

#[test]
#[serial]
fn opens_explicit_output_device_when_available() {
    let Some(device_id) = support::first_device_id(AudioDeviceKind::Output) else {
        support::skip_without_device(AudioDeviceKind::Output);
        return;
    };

    let playback =
        OutputPlayback::start(Some(&device_id)).expect("explicit output device should open");
    playback
        .enqueue_samples(&[1, 2, 3, 4])
        .expect("enqueue should succeed");
}
