use super::execute;
use crate::FsCommand;
use crate::FsErrorKind;
use crate::FsPayload;
use crate::FsResponse;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

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
