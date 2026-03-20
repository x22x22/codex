use super::run_from_args;
use crate::READ_FILE_OPERATION_ARG;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn run_from_args_streams_file_bytes_to_stdout() {
    let tempdir = tempdir().expect("tempdir");
    let path = tempdir.path().join("image.bin");
    let expected = b"hello\x00world".to_vec();
    std::fs::write(&path, &expected).expect("write test file");

    let mut stdin = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run_from_args(
        [
            READ_FILE_OPERATION_ARG,
            path.to_str().expect("utf-8 test path"),
        ]
        .into_iter()
        .map(Into::into),
        &mut stdin,
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
    let mut stdin = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let error = run_from_args(
        [
            READ_FILE_OPERATION_ARG,
            tempdir.path().to_str().expect("utf-8 test path"),
        ]
        .into_iter()
        .map(Into::into),
        &mut stdin,
        &mut stdout,
        &mut stderr,
    )
    .expect_err("reading a directory should fail");

    #[cfg(not(target_os = "windows"))]
    assert_eq!(error.kind(), std::io::ErrorKind::IsADirectory);
    #[cfg(target_os = "windows")]
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn run_from_args_serializes_errors_to_stderr() {
    let tempdir = tempdir().expect("tempdir");
    let missing = tempdir.path().join("missing.txt");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    let result = run_from_args(
        [
            READ_FILE_OPERATION_ARG,
            missing.to_str().expect("utf-8 test path"),
        ]
        .into_iter()
        .map(Into::into),
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );

    assert!(result.is_err(), "missing file should fail");
    assert_eq!(stdout, Vec::<u8>::new());
}
