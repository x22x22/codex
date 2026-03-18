use super::execute;
use crate::FsCommand;
use crate::FsErrorKind;
use crate::run_from_args;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn run_from_args_streams_file_bytes_to_stdout() {
    let tempdir = tempdir().expect("tempdir");
    let path = tempdir.path().join("image.bin");
    let expected = b"hello\x00world".to_vec();
    std::fs::write(&path, &expected).expect("write test file");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run_from_args(
        ["read", path.to_str().expect("utf-8 test path")]
            .into_iter()
            .map(Into::into),
        &mut stdout,
        &mut stderr,
    )
    .expect("read should succeed");

    assert_eq!(stdout, expected);
    assert_eq!(stderr, Vec::<u8>::new());
}

#[test]
fn read_reports_directory_error() {
    let tempdir = tempdir().expect("tempdir");
    let mut stdout = Vec::new();

    let error = execute(
        FsCommand::Read {
            path: tempdir.path().to_path_buf(),
        },
        &mut stdout,
    )
    .expect_err("reading a directory should fail");

    #[cfg(target_os = "windows")]
    assert_eq!(error.kind, FsErrorKind::PermissionDenied);
    #[cfg(not(target_os = "windows"))]
    assert_eq!(error.kind, FsErrorKind::IsADirectory);
}

#[test]
fn run_from_args_serializes_errors_to_stderr() {
    let tempdir = tempdir().expect("tempdir");
    let missing = tempdir.path().join("missing.txt");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let result = run_from_args(
        ["read", missing.to_str().expect("utf-8 test path")]
            .into_iter()
            .map(Into::into),
        &mut stdout,
        &mut stderr,
    );

    assert!(result.is_err(), "missing file should fail");
    assert_eq!(stdout, Vec::<u8>::new());

    let error: crate::FsError = serde_json::from_slice(&stderr).expect("structured fs error");
    assert_eq!(error.kind, FsErrorKind::NotFound);
    assert_eq!(error.raw_os_error.is_some(), true);
}
