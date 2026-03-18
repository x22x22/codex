use serde::Deserialize;
use serde::Serialize;
use std::io::ErrorKind;

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
