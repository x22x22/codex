use codex_audio::AudioDeviceInfo;
use codex_audio::AudioDeviceKind;
use codex_audio::list_audio_devices;
use codex_audio::resolve_device_name;

use crate::app_event::RealtimeAudioDeviceKind;

pub(crate) fn list_realtime_audio_devices(
    kind: RealtimeAudioDeviceKind,
) -> Result<Vec<AudioDeviceInfo>, String> {
    list_audio_devices(audio_device_kind(kind))
}

pub(crate) fn resolve_realtime_audio_device_name(
    kind: RealtimeAudioDeviceKind,
    device_id: &str,
) -> Result<Option<String>, String> {
    resolve_device_name(audio_device_kind(kind), device_id)
}

fn audio_device_kind(kind: RealtimeAudioDeviceKind) -> AudioDeviceKind {
    match kind {
        RealtimeAudioDeviceKind::Microphone => AudioDeviceKind::Input,
        RealtimeAudioDeviceKind::Speaker => AudioDeviceKind::Output,
    }
}
