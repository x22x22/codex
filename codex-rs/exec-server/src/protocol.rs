use std::collections::HashMap;
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use serde::Serialize;

pub const INITIALIZE_METHOD: &str = "initialize";
pub const INITIALIZED_METHOD: &str = "initialized";
pub const EXEC_METHOD: &str = "process/start";
pub const EXEC_READ_METHOD: &str = "process/read";
pub const EXEC_WRITE_METHOD: &str = "process/write";
pub const EXEC_TERMINATE_METHOD: &str = "process/terminate";
pub const EXEC_RESIZE_METHOD: &str = "process/resize";
pub const EXEC_WAIT_METHOD: &str = "process/wait";
pub const EXEC_OUTPUT_DELTA_METHOD: &str = "process/output";
pub const EXEC_EXITED_METHOD: &str = "process/exited";
pub const ENVIRONMENT_LIST_METHOD: &str = "environment/list";
pub const ENVIRONMENT_GET_METHOD: &str = "environment/get";
pub const ENVIRONMENT_CAPABILITIES_METHOD: &str = "environment/capabilities";
pub const FS_READ_FILE_METHOD: &str = "fs/readFile";
pub const FS_WRITE_FILE_METHOD: &str = "fs/writeFile";
pub const FS_CREATE_DIRECTORY_METHOD: &str = "fs/createDirectory";
pub const FS_GET_METADATA_METHOD: &str = "fs/getMetadata";
pub const FS_READ_DIRECTORY_METHOD: &str = "fs/readDirectory";
pub const FS_REMOVE_METHOD: &str = "fs/remove";
pub const FS_COPY_METHOD: &str = "fs/copy";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ByteChunk(#[serde(with = "base64_bytes")] pub Vec<u8>);

impl ByteChunk {
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for ByteChunk {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecParams {
    /// Client-chosen logical process handle scoped to this connection/session.
    /// This is a protocol key, not an OS pid.
    pub process_id: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub tty: bool,
    pub arg0: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecTerminalSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResizeParams {
    pub process_id: String,
    pub size: ExecTerminalSize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResizeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecWaitParams {
    pub process_id: String,
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecWaitResponse {
    pub exited: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResponse {
    pub process_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadParams {
    pub process_id: String,
    pub after_seq: Option<u64>,
    pub max_bytes: Option<usize>,
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutputChunk {
    pub seq: u64,
    pub stream: ExecOutputStream,
    pub chunk: ByteChunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResponse {
    pub chunks: Vec<ProcessOutputChunk>,
    pub next_seq: u64,
    pub exited: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteParams {
    pub process_id: String,
    pub chunk: ByteChunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateParams {
    pub process_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateResponse {
    pub running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecOutputStream {
    Stdout,
    Stderr,
    Pty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecOutputDeltaNotification {
    pub process_id: String,
    pub stream: ExecOutputStream,
    pub chunk: ByteChunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecExitedNotification {
    pub process_id: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCapabilities {
    pub filesystem: bool,
    pub process_resize: bool,
    pub process_wait: bool,
}

impl Default for EnvironmentCapabilities {
    fn default() -> Self {
        Self {
            filesystem: true,
            process_resize: true,
            process_wait: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub environment_id: String,
    pub experimental_exec_server_url: Option<String>,
    pub capabilities: EnvironmentCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentListResponse {
    pub environments: Vec<EnvironmentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentGetParams {
    pub environment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentGetResponse {
    pub environment: EnvironmentInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCapabilitiesParams {
    pub environment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCapabilitiesResponse {
    pub environment_id: String,
    pub capabilities: EnvironmentCapabilities,
}

mod base64_bytes {
    use super::BASE64_STANDARD;
    use base64::Engine as _;
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serializer;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64_STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        BASE64_STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::EnvironmentCapabilities;
    use super::EnvironmentCapabilitiesParams;
    use super::EnvironmentCapabilitiesResponse;
    use super::EnvironmentGetParams;
    use super::EnvironmentGetResponse;
    use super::EnvironmentInfo;
    use super::EnvironmentListParams;
    use super::EnvironmentListResponse;
    use super::ExecResizeParams;
    use super::ExecTerminalSize;
    use super::ExecWaitParams;
    use super::ExecWaitResponse;
    use pretty_assertions::assert_eq;

    #[test]
    fn environment_payloads_round_trip() {
        let capabilities = EnvironmentCapabilities::default();
        let info = EnvironmentInfo {
            environment_id: "local".to_string(),
            experimental_exec_server_url: None,
            capabilities: capabilities.clone(),
        };
        let list_response = EnvironmentListResponse {
            environments: vec![info.clone()],
        };
        let get_response = EnvironmentGetResponse {
            environment: info.clone(),
        };
        let capabilities_response = EnvironmentCapabilitiesResponse {
            environment_id: info.environment_id,
            capabilities,
        };

        assert_eq!(
            serde_json::to_value(EnvironmentListParams {}).expect("serialize list params"),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::from_value::<EnvironmentListParams>(serde_json::json!({}))
                .expect("deserialize list params"),
            EnvironmentListParams {}
        );
        assert_eq!(
            serde_json::from_value::<EnvironmentListResponse>(
                serde_json::to_value(&list_response).expect("serialize list response"),
            )
            .expect("deserialize list response"),
            list_response
        );
        assert_eq!(
            serde_json::from_value::<EnvironmentGetParams>(serde_json::json!({
                "environmentId": "local"
            }))
            .expect("deserialize get params"),
            EnvironmentGetParams {
                environment_id: "local".to_string(),
            }
        );
        assert_eq!(
            serde_json::from_value::<EnvironmentGetResponse>(
                serde_json::to_value(&get_response).expect("serialize get response"),
            )
            .expect("deserialize get response"),
            get_response
        );
        assert_eq!(
            serde_json::from_value::<EnvironmentCapabilitiesParams>(serde_json::json!({
                "environmentId": "local"
            }))
            .expect("deserialize capabilities params"),
            EnvironmentCapabilitiesParams {
                environment_id: "local".to_string(),
            }
        );
        assert_eq!(
            serde_json::from_value::<EnvironmentCapabilitiesResponse>(
                serde_json::to_value(&capabilities_response)
                    .expect("serialize capabilities response"),
            )
            .expect("deserialize capabilities response"),
            capabilities_response
        );
    }

    #[test]
    fn process_payloads_round_trip() {
        let resize_params = ExecResizeParams {
            process_id: "proc-1".to_string(),
            size: ExecTerminalSize { rows: 24, cols: 80 },
        };
        let wait_params = ExecWaitParams {
            process_id: "proc-1".to_string(),
            wait_ms: Some(250),
        };
        let wait_response = ExecWaitResponse {
            exited: true,
            exit_code: Some(0),
        };

        assert_eq!(
            serde_json::from_value::<ExecResizeParams>(
                serde_json::to_value(&resize_params).expect("serialize resize params"),
            )
            .expect("deserialize resize params"),
            resize_params
        );
        assert_eq!(
            serde_json::from_value::<ExecWaitParams>(
                serde_json::to_value(&wait_params).expect("serialize wait params"),
            )
            .expect("deserialize wait params"),
            wait_params
        );
        assert_eq!(
            serde_json::from_value::<ExecWaitResponse>(
                serde_json::to_value(&wait_response).expect("serialize wait response"),
            )
            .expect("deserialize wait response"),
            wait_response
        );
    }
}
