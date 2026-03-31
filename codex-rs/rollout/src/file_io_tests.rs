use super::*;
use crate::file_io::RolloutLineReader;
use crate::file_io::append_text;
use std::io;
use tempfile::TempDir;

#[test]
fn strip_rollout_file_suffix_supports_both_formats() {
    assert_eq!(
        strip_rollout_file_suffix("rollout-2026-01-01T00-00-00-thread.jsonl"),
        Some("rollout-2026-01-01T00-00-00-thread")
    );
    assert_eq!(
        strip_rollout_file_suffix("rollout-2026-01-01T00-00-00-thread.jsonl.zst"),
        Some("rollout-2026-01-01T00-00-00-thread")
    );
    assert_eq!(strip_rollout_file_suffix("rollout.txt"), None);
}

#[test]
fn compressed_appends_are_read_back_as_one_stream() -> io::Result<()> {
    let temp_dir = TempDir::new()?;
    let path = temp_dir
        .path()
        .join("rollout-2026-01-01T00-00-00-thread.jsonl.zst");

    append_text(path.as_path(), "{\"a\":1}\n")?;
    append_text(path.as_path(), "{\"b\":2}\n")?;

    assert_eq!(read_rollout_text(path.as_path())?, "{\"a\":1}\n{\"b\":2}\n");

    let mut reader = RolloutLineReader::open(path.as_path())?;
    assert_eq!(reader.next_line()?, Some("{\"a\":1}".to_string()));
    assert_eq!(reader.next_line()?, Some("{\"b\":2}".to_string()));
    assert_eq!(reader.next_line()?, None);
    Ok(())
}
