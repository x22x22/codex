use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use serde::Serialize;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::io::Write;
use std::path::PathBuf;

/// Special argv[1] flag used when the Codex executable self-invokes to run the
/// internal sandbox-backed filesystem helper path.
pub const CODEX_CORE_FS_OPS_ARG1: &str = "--codex-run-as-fs-ops";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsCommand {
    ReadBytes { path: PathBuf },
    ReadText { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsErrorKind {
    NotFound,
    PermissionDenied,
    IsADirectory,
    InvalidData,
    Other,
}

impl From<ErrorKind> for FsErrorKind {
    fn from(value: ErrorKind) -> Self {
        match value {
            ErrorKind::NotFound => Self::NotFound,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::IsADirectory => Self::IsADirectory,
            ErrorKind::InvalidData => Self::InvalidData,
            _ => Self::Other,
        }
    }
}

impl FsErrorKind {
    pub fn to_io_error_kind(&self) -> ErrorKind {
        match self {
            Self::NotFound => ErrorKind::NotFound,
            Self::PermissionDenied => ErrorKind::PermissionDenied,
            Self::IsADirectory => ErrorKind::IsADirectory,
            Self::InvalidData => ErrorKind::InvalidData,
            Self::Other => ErrorKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsError {
    pub kind: FsErrorKind,
    pub message: String,
    pub raw_os_error: Option<i32>,
}

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl FsError {
    pub fn to_io_error(&self) -> std::io::Error {
        if let Some(raw_os_error) = self.raw_os_error {
            std::io::Error::from_raw_os_error(raw_os_error)
        } else {
            std::io::Error::new(self.kind.to_io_error_kind(), self.message.clone())
        }
    }
}

impl From<std::io::Error> for FsError {
    fn from(error: std::io::Error) -> Self {
        Self {
            kind: error.kind().into(),
            message: error.to_string(),
            raw_os_error: error.raw_os_error(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsPayload {
    Bytes { base64: String },
    Text { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FsResponse {
    Success { payload: FsPayload },
    Error { error: FsError },
}

pub fn parse_command_from_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<FsCommand, String> {
    let Some(operation) = args.next() else {
        return Err("missing operation".to_string());
    };
    let Some(operation) = operation.to_str() else {
        return Err("operation must be valid UTF-8".to_string());
    };
    let Some(path) = args.next() else {
        return Err(format!("missing path for operation `{operation}`"));
    };
    if args.next().is_some() {
        return Err(format!(
            "unexpected extra arguments for operation `{operation}`"
        ));
    }

    let path = PathBuf::from(path);
    match operation {
        "read_bytes" => Ok(FsCommand::ReadBytes { path }),
        "read_text" => Ok(FsCommand::ReadText { path }),
        _ => Err(format!(
            "unsupported filesystem operation `{operation}`; expected one of `read_bytes`, `read_text`"
        )),
    }
}

pub fn execute(command: FsCommand) -> FsResponse {
    match command {
        FsCommand::ReadBytes { path } => match std::fs::read(&path) {
            Ok(bytes) => FsResponse::Success {
                payload: FsPayload::Bytes {
                    base64: BASE64_STANDARD.encode(bytes),
                },
            },
            Err(error) => FsResponse::Error {
                error: error.into(),
            },
        },
        FsCommand::ReadText { path } => match std::fs::read_to_string(&path) {
            Ok(text) => FsResponse::Success {
                payload: FsPayload::Text { text },
            },
            Err(error) => FsResponse::Error {
                error: error.into(),
            },
        },
    }
}

pub fn write_response(stdout: &mut impl Write, response: &FsResponse) -> Result<()> {
    serde_json::to_writer(&mut *stdout, response).context("failed to serialize fs response")?;
    writeln!(stdout).context("failed to terminate fs response with newline")?;
    Ok(())
}

pub fn run_from_args(args: impl Iterator<Item = OsString>) -> Result<()> {
    let command = parse_command_from_args(args).map_err(anyhow::Error::msg)?;
    let response = execute(command);
    let mut stdout = std::io::stdout().lock();
    write_response(&mut stdout, &response)
}

#[cfg(test)]
mod tests {
    use super::FsCommand;
    use super::FsErrorKind;
    use super::FsPayload;
    use super::FsResponse;
    use super::execute;
    use super::parse_command_from_args;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn parse_read_bytes_command() {
        let command = parse_command_from_args(
            ["read_bytes", "/tmp/example.png"]
                .into_iter()
                .map(Into::into),
        )
        .expect("command should parse");

        assert_eq!(
            command,
            FsCommand::ReadBytes {
                path: "/tmp/example.png".into(),
            }
        );
    }

    #[test]
    fn read_text_returns_text_payload() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("note.txt");
        std::fs::write(&path, "hello").expect("write test file");

        let response = execute(FsCommand::ReadText { path });

        assert_eq!(
            response,
            FsResponse::Success {
                payload: FsPayload::Text {
                    text: "hello".to_string(),
                },
            }
        );
    }

    #[test]
    fn read_bytes_reports_directory_error() {
        let tempdir = tempdir().expect("tempdir");

        let response = execute(FsCommand::ReadBytes {
            path: tempdir.path().to_path_buf(),
        });

        let FsResponse::Error { error } = response else {
            panic!("expected error response");
        };
        #[cfg(target_os = "windows")]
        assert_eq!(error.kind, FsErrorKind::PermissionDenied);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(error.kind, FsErrorKind::IsADirectory);
    }
}
