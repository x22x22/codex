use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Error;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;
use tracing::warn;

#[cfg(test)]
use super::stats::SecurityStats;
use codex_protocol::protocol::SecurityEvent;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct AuditLog {
    pub(crate) recorded_at: i64,
    pub(crate) event: SecurityEvent,
}

pub(crate) struct AuditLogger {
    session_buffer_limit: usize,
    file_path: Option<PathBuf>,
    records: Mutex<VecDeque<AuditLog>>,
}

impl AuditLogger {
    pub(crate) fn new(session_buffer_limit: usize, file_path: Option<PathBuf>) -> Self {
        Self {
            session_buffer_limit,
            file_path,
            records: Mutex::new(VecDeque::with_capacity(session_buffer_limit)),
        }
    }

    pub(crate) fn record(&self, event: SecurityEvent) {
        let record = AuditLog {
            recorded_at: OffsetDateTime::now_utc().unix_timestamp(),
            event,
        };

        match self.records.lock() {
            Ok(mut records) => {
                records.push_back(record.clone());
                while records.len() > self.session_buffer_limit {
                    records.pop_front();
                }
            }
            Err(err) => {
                warn!("failed to lock in-memory audit log buffer: {err}");
                return;
            }
        }

        if let Some(file_path) = self.file_path.as_deref()
            && let Err(err) = Self::append_record(file_path, &record)
        {
            warn!(
                "failed to append audit log to {}: {err}",
                file_path.display()
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<AuditLog> {
        match self.records.lock() {
            Ok(records) => records.iter().cloned().collect(),
            Err(err) => {
                warn!("failed to read in-memory audit log buffer: {err}");
                Vec::new()
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> SecurityStats {
        let snapshot = self.snapshot();
        let allowed = snapshot
            .iter()
            .filter(|record| record.event.allowed == Some(true))
            .count();
        let denied = snapshot
            .iter()
            .filter(|record| record.event.allowed == Some(false))
            .count();
        SecurityStats {
            total: snapshot.len(),
            allowed,
            denied,
        }
    }

    fn append_record(file_path: &Path, record: &AuditLog) -> std::io::Result<()> {
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let serialized = serde_json::to_string(record)
            .map_err(|err| Error::other(format!("failed to serialize audit log: {err}")))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path)?;
        writeln!(file, "{serialized}")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AuditLogger;
    use codex_protocol::protocol::SecurityEvent;
    use codex_protocol::protocol::SecurityEventKind;
    use pretty_assertions::assert_eq;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    fn test_event(action: &str) -> SecurityEvent {
        SecurityEvent {
            kind: SecurityEventKind::Command,
            action: action.to_owned(),
            turn_id: "turn-1".to_owned(),
            call_id: Some("call-1".to_owned()),
            allowed: None,
            target: Some("echo hello".to_owned()),
            details: None,
            duration_ms: None,
        }
    }

    #[test]
    fn record_keeps_a_bounded_buffer() {
        let logger = AuditLogger::new(2, None);
        logger.record(test_event("one"));
        logger.record(test_event("two"));
        logger.record(test_event("three"));

        let snapshot = logger.snapshot();
        assert_eq!(2, snapshot.len());
        assert_eq!("two", snapshot[0].event.action);
        assert_eq!("three", snapshot[1].event.action);
        assert_eq!(2, logger.stats().total);
    }

    #[test]
    fn record_appends_jsonl_when_enabled() {
        let dir = TempDir::new().expect("tempdir");
        let file_path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(2, Some(file_path.clone()));
        logger.record(test_event("one"));

        let contents = std::fs::read_to_string(file_path).expect("auditlog");
        assert!(contents.contains("\"action\":\"one\""));
    }

    #[test]
    fn record_is_best_effort_when_file_write_fails() {
        let not_a_directory = NamedTempFile::new().expect("temp file");
        let impossible_path = not_a_directory.path().join("audit.jsonl");
        let logger = AuditLogger::new(2, Some(impossible_path));
        logger.record(test_event("one"));

        let snapshot = logger.snapshot();
        assert_eq!(1, snapshot.len());
        assert_eq!("one", snapshot[0].event.action);
    }
}
