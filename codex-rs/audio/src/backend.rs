use crate::AudioDeviceInfo;
use crate::AudioDeviceKind;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputOpenRequest {
    pub(crate) device_id: Option<String>,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u32,
    pub(crate) period_frames: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OutputOpenRequest {
    pub(crate) device_id: Option<String>,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u32,
    pub(crate) period_frames: u32,
}

pub(crate) type InputDataCallback = Arc<dyn Fn(&[i16]) + Send + Sync>;
pub(crate) type OutputDataCallback = Arc<dyn Fn(&mut [i16]) + Send + Sync>;

pub(crate) trait InputStreamHandle: Send {
    fn stop(&mut self) -> Result<(), String>;
}

pub(crate) trait OutputStreamHandle: Send {
    fn stop(&mut self) -> Result<(), String>;
}

pub(crate) trait AudioBackend: Send + Sync {
    fn backend_name(&self) -> String;
    fn list_devices(&self, kind: AudioDeviceKind) -> Result<Vec<AudioDeviceInfo>, String>;
    fn open_input(
        &self,
        request: &InputOpenRequest,
        callback: InputDataCallback,
    ) -> Result<Box<dyn InputStreamHandle>, String>;
    fn open_output(
        &self,
        request: &OutputOpenRequest,
        callback: OutputDataCallback,
    ) -> Result<Box<dyn OutputStreamHandle>, String>;
}
