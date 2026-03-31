use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use tokio::time::sleep;

pub const ROLLOUT_FILE_SUFFIX: &str = ".jsonl";
pub const COMPRESSED_ROLLOUT_FILE_SUFFIX: &str = ".jsonl.zst";

const DEFAULT_ZSTD_LEVEL: i32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RolloutFileEncoding {
    PlainJsonl,
    ZstdJsonl,
}

impl RolloutFileEncoding {
    pub(crate) fn for_path(path: &Path) -> Self {
        path.file_name()
            .and_then(|file_name| file_name.to_str())
            .and_then(file_encoding_from_name)
            .unwrap_or_else(|| Self::PlainJsonl)
    }

    pub(crate) fn preferred_suffix(self) -> &'static str {
        match self {
            Self::PlainJsonl => ROLLOUT_FILE_SUFFIX,
            Self::ZstdJsonl => COMPRESSED_ROLLOUT_FILE_SUFFIX,
        }
    }
}

/// Returns the suffix used for newly created rollout files.
pub fn preferred_rollout_file_suffix() -> &'static str {
    RolloutFileEncoding::ZstdJsonl.preferred_suffix()
}

/// Returns true when `name` matches a rollout filename in either supported encoding.
pub fn is_rollout_file_name(name: &str) -> bool {
    name.starts_with("rollout-") && strip_rollout_file_suffix(name).is_some()
}

/// Returns true when `path` points to a rollout file in either supported encoding.
pub fn is_rollout_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|file_name| file_name.to_str())
        .is_some_and(is_rollout_file_name)
}

/// Removes the rollout suffix from `name`, supporting both plain and compressed files.
pub fn strip_rollout_file_suffix(name: &str) -> Option<&str> {
    name.strip_suffix(COMPRESSED_ROLLOUT_FILE_SUFFIX)
        .or_else(|| name.strip_suffix(ROLLOUT_FILE_SUFFIX))
}

pub(crate) fn append_text(path: &Path, text: &str) -> io::Result<()> {
    match RolloutFileEncoding::for_path(path) {
        RolloutFileEncoding::PlainJsonl => {
            let mut file = OpenOptions::new().append(true).create(true).open(path)?;
            file.write_all(text.as_bytes())?;
            file.flush()
        }
        RolloutFileEncoding::ZstdJsonl => {
            let file = OpenOptions::new().append(true).create(true).open(path)?;
            let mut encoder = zstd::stream::write::Encoder::new(file, DEFAULT_ZSTD_LEVEL)?;
            encoder.write_all(text.as_bytes())?;
            let mut file = encoder.finish()?;
            file.flush()
        }
    }
}

/// Reads the full rollout file contents, transparently handling plain and zstd-compressed files.
pub fn read_rollout_text(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    match RolloutFileEncoding::for_path(path) {
        RolloutFileEncoding::PlainJsonl => {
            File::open(path)?.read_to_string(&mut text)?;
        }
        RolloutFileEncoding::ZstdJsonl => {
            let file = File::open(path)?;
            let mut decoder = zstd::stream::read::Decoder::new(file)?;
            decoder.read_to_string(&mut text)?;
        }
    }
    Ok(text)
}

/// Retries reading `path` until the rollout exists and contains non-empty text, or returns the
/// last read attempt.
pub async fn read_nonempty_rollout_text(path: &Path) -> io::Result<String> {
    const MAX_ATTEMPTS: usize = 50;
    const RETRY_DELAY: Duration = Duration::from_millis(20);

    for _ in 0..MAX_ATTEMPTS {
        if path.exists()
            && let Ok(text) = read_rollout_text(path)
            && !text.trim().is_empty()
        {
            return Ok(text);
        }
        sleep(RETRY_DELAY).await;
    }

    read_rollout_text(path)
}

pub(crate) struct RolloutLineReader {
    inner: RolloutLineReaderInner,
}

enum RolloutLineReaderInner {
    Plain(BufReader<File>),
    Zstd(BufReader<zstd::stream::read::Decoder<'static, BufReader<File>>>),
}

impl RolloutLineReader {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let inner = match RolloutFileEncoding::for_path(path) {
            RolloutFileEncoding::PlainJsonl => {
                RolloutLineReaderInner::Plain(BufReader::new(File::open(path)?))
            }
            RolloutFileEncoding::ZstdJsonl => {
                let file = BufReader::new(File::open(path)?);
                let decoder = zstd::stream::read::Decoder::with_buffer(file)?;
                RolloutLineReaderInner::Zstd(BufReader::new(decoder))
            }
        };
        Ok(Self { inner })
    }

    pub(crate) fn next_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        let bytes_read = match &mut self.inner {
            RolloutLineReaderInner::Plain(reader) => reader.read_line(&mut line)?,
            RolloutLineReaderInner::Zstd(reader) => reader.read_line(&mut line)?,
        };
        if bytes_read == 0 {
            return Ok(None);
        }
        trim_line_ending(&mut line);
        Ok(Some(line))
    }
}

fn file_encoding_from_name(name: &str) -> Option<RolloutFileEncoding> {
    if name.ends_with(COMPRESSED_ROLLOUT_FILE_SUFFIX) {
        return Some(RolloutFileEncoding::ZstdJsonl);
    }
    if name.ends_with(ROLLOUT_FILE_SUFFIX) {
        return Some(RolloutFileEncoding::PlainJsonl);
    }
    None
}

fn trim_line_ending(line: &mut String) {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
}

#[cfg(test)]
#[path = "file_io_tests.rs"]
mod tests;
