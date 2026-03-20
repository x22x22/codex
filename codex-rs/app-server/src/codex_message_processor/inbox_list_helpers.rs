use std::io;
use std::path::Path;

use chrono::DateTime;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use tokio::fs;
use tracing::warn;

#[derive(Debug)]
pub(super) enum InboxUpdateError {
    InvalidRequest(String),
    Io(io::Error),
}

impl From<io::Error> for InboxUpdateError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub(super) async fn load_inbox_entries(codex_home: &Path) -> io::Result<Vec<Value>> {
    let inbox_dir = codex_home.join("inbox");
    let mut dir = match fs::read_dir(&inbox_dir).await {
        Ok(dir) => dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(err) => {
            return Err(err);
        }
    };

    let user_tracking_by_thread = match load_user_tracking_by_thread(codex_home).await {
        Ok(user_tracking_by_thread) => user_tracking_by_thread,
        Err(err) => {
            warn!("Skipping malformed inbox tracking file: {err}");
            Map::new()
        }
    };

    let mut paths = Vec::new();
    while let Some(entry) = dir.next_entry().await? {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };

        if file_name.starts_with("thread__") && file_name.ends_with(".json") {
            paths.push((file_name.to_owned(), entry.path()));
        }
    }

    paths.sort_by(|a, b| b.0.cmp(&a.0));

    let mut data = Vec::with_capacity(paths.len());
    for (file_name, path) in paths {
        let contents = fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents) {
            Ok(mut value) => {
                remove_thread_owned_last_read_at(&mut value);
                apply_user_tracking(&mut value, user_tracking_by_thread.get(&file_name));
                data.push(value);
            }
            Err(err) => {
                warn!("Skipping malformed inbox entry {file_name}: {err}");
            }
        }
    }

    Ok(data)
}

pub(super) async fn update_inbox_entry_last_read_at(
    codex_home: &Path,
    thread_id: &str,
    last_read_at: &str,
) -> Result<Value, InboxUpdateError> {
    if thread_id.is_empty() {
        return Err(InboxUpdateError::InvalidRequest(
            "thread_id must not be empty".to_string(),
        ));
    }

    if thread_id.contains(['/', '\\']) {
        return Err(InboxUpdateError::InvalidRequest(format!(
            "thread_id must be a filename, not a path: {thread_id}"
        )));
    }

    if DateTime::parse_from_rfc3339(last_read_at).is_err() {
        return Err(InboxUpdateError::InvalidRequest(format!(
            "last_read_at must be an RFC 3339 timestamp: {last_read_at}"
        )));
    }

    let thread_path = codex_home.join("inbox").join(thread_id);
    if let Err(err) = fs::metadata(&thread_path).await {
        if err.kind() == io::ErrorKind::NotFound {
            return Err(InboxUpdateError::InvalidRequest(format!(
                "inbox entry not found: {thread_id}"
            )));
        }

        return Err(err.into());
    }

    let mut user_tracking_by_thread = match load_user_tracking_by_thread(codex_home).await {
        Ok(user_tracking_by_thread) => user_tracking_by_thread,
        Err(err) => {
            return Err(InboxUpdateError::InvalidRequest(format!(
                "inbox tracking file is not valid JSON: {err}"
            )));
        }
    };

    let user_tracking = user_tracking_by_thread
        .entry(thread_id.to_string())
        .or_insert_with(|| json!({}));
    let Some(user_tracking_object) = user_tracking.as_object_mut() else {
        return Err(InboxUpdateError::InvalidRequest(format!(
            "tracking entry for {thread_id} must be a JSON object"
        )));
    };
    user_tracking_object.insert("last_read_at".to_string(), json!(last_read_at));

    let tracking_path = codex_home.join("inbox").join("tracking.json");
    let serialized = serde_json::to_string_pretty(&user_tracking_by_thread).map_err(|err| {
        InboxUpdateError::InvalidRequest(format!("failed to serialize tracking file: {err}"))
    })?;
    fs::write(&tracking_path, format!("{serialized}\n")).await?;

    let contents = match fs::read_to_string(&thread_path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(InboxUpdateError::InvalidRequest(format!(
                "inbox entry not found: {thread_id}"
            )));
        }
        Err(err) => {
            return Err(err.into());
        }
    };

    let mut entry = serde_json::from_str::<Value>(&contents).map_err(|err| {
        InboxUpdateError::InvalidRequest(format!("inbox entry is not valid JSON: {err}"))
    })?;

    remove_thread_owned_last_read_at(&mut entry);
    apply_user_tracking(&mut entry, user_tracking_by_thread.get(thread_id));

    Ok(entry)
}

async fn load_user_tracking_by_thread(codex_home: &Path) -> Result<Map<String, Value>, String> {
    let path = codex_home.join("inbox").join("tracking.json");
    let contents = match fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Map::new());
        }
        Err(err) => {
            return Err(err.to_string());
        }
    };

    serde_json::from_str::<Map<String, Value>>(&contents).map_err(|err| err.to_string())
}

fn apply_user_tracking(entry: &mut Value, user_tracking: Option<&Value>) {
    let Some(user_tracking) = user_tracking else {
        return;
    };
    let Some(user_tracking_object) = user_tracking.as_object() else {
        return;
    };
    let Some(entry_object) = entry.as_object_mut() else {
        return;
    };

    let tracking = entry_object
        .entry("tracking".to_string())
        .or_insert_with(|| json!({}));
    let Some(tracking_object) = tracking.as_object_mut() else {
        return;
    };

    for (key, value) in user_tracking_object {
        tracking_object.insert(key.clone(), value.clone());
    }
}

fn remove_thread_owned_last_read_at(entry: &mut Value) {
    let Some(entry_object) = entry.as_object_mut() else {
        return;
    };
    let Some(tracking) = entry_object.get_mut("tracking") else {
        return;
    };
    let Some(tracking_object) = tracking.as_object_mut() else {
        return;
    };

    tracking_object.remove("last_read_at");
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn load_inbox_entries_returns_newest_first_and_skips_malformed_files() -> Result<()> {
        let tempdir = TempDir::new()?;
        let inbox_dir = tempdir.path().join("inbox");
        fs::create_dir(&inbox_dir).await?;

        fs::write(
            inbox_dir.join("thread__20260319T202114Z__slack__C1__111.000000.json"),
            json!({
                "schema_version": 1,
                "current_progress": "older"
            })
            .to_string(),
        )
        .await?;
        fs::write(
            inbox_dir.join("thread__20260320T012640Z__slack__C2__222.000000.json"),
            json!({
                "schema_version": 1,
                "current_progress": "newer",
                "tracking": {
                    "last_read_at": "2026-03-19T16:20:00-07:00"
                }
            })
            .to_string(),
        )
        .await?;
        fs::write(
            inbox_dir.join("thread__20260320T020000Z__slack__C3__333.000000.json"),
            "{",
        )
        .await?;
        fs::write(inbox_dir.join("notes.txt"), "ignored").await?;
        fs::write(
            inbox_dir.join("tracking.json"),
            json!({
                "thread__20260320T012640Z__slack__C2__222.000000.json": {
                    "last_read_at": "2026-03-20T09:15:00-07:00"
                }
            })
            .to_string(),
        )
        .await?;

        let entries = load_inbox_entries(tempdir.path()).await?;

        assert_eq!(
            entries,
            vec![
                json!({
                    "schema_version": 1,
                    "current_progress": "newer",
                    "tracking": {
                        "last_read_at": "2026-03-20T09:15:00-07:00"
                    }
                }),
                json!({
                    "schema_version": 1,
                    "current_progress": "older"
                }),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn load_inbox_entries_returns_empty_when_inbox_dir_is_missing() -> Result<()> {
        let tempdir = TempDir::new()?;

        let entries = load_inbox_entries(tempdir.path()).await?;

        assert_eq!(entries, Vec::<Value>::new());
        Ok(())
    }

    #[tokio::test]
    async fn update_inbox_entry_last_read_at_writes_tracking_file_and_leaves_thread_file_unchanged()
    -> Result<()> {
        let tempdir = TempDir::new()?;
        let inbox_dir = tempdir.path().join("inbox");
        fs::create_dir(&inbox_dir).await?;
        let thread_id = "thread__20260320T012640Z__slack__C08MGJXUCUQ__1773969612.866769.json";
        let thread_path = inbox_dir.join(thread_id);
        let original_thread_entry = json!({
            "schema_version": 1,
            "source": {
                "type": "slack",
                "channel_id": "C08MGJXUCUQ",
                "thread_ts": "1773969612.866769"
            },
            "tracking": {
                "last_refreshed_at": "2026-03-19T18:54:47-07:00"
            },
            "timeline": []
        });
        fs::write(&thread_path, original_thread_entry.to_string()).await?;

        let entry =
            update_inbox_entry_last_read_at(tempdir.path(), thread_id, "2026-03-20T09:15:00-07:00")
                .await
                .expect("update succeeds");

        assert_eq!(
            entry,
            json!({
                "schema_version": 1,
                "source": {
                    "type": "slack",
                    "channel_id": "C08MGJXUCUQ",
                    "thread_ts": "1773969612.866769"
                },
                "tracking": {
                    "last_read_at": "2026-03-20T09:15:00-07:00",
                    "last_refreshed_at": "2026-03-19T18:54:47-07:00"
                },
                "timeline": []
            })
        );

        let reloaded_thread =
            serde_json::from_str::<Value>(&fs::read_to_string(&thread_path).await?)?;
        assert_eq!(reloaded_thread, original_thread_entry);

        let reloaded_tracking = serde_json::from_str::<Value>(
            &fs::read_to_string(inbox_dir.join("tracking.json")).await?,
        )?;
        assert_eq!(
            reloaded_tracking,
            json!({
                thread_id: {
                    "last_read_at": "2026-03-20T09:15:00-07:00"
                }
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn update_inbox_entry_last_read_at_rejects_path_traversal() -> Result<()> {
        let tempdir = TempDir::new()?;

        let err = update_inbox_entry_last_read_at(
            tempdir.path(),
            "../thread__20260320T012640Z__slack__C08MGJXUCUQ__1773969612.866769.json",
            "2026-03-20T09:15:00-07:00",
        )
        .await
        .expect_err("path-like ids are rejected");

        match err {
            InboxUpdateError::InvalidRequest(message) => {
                assert_eq!(
                    message,
                    "thread_id must be a filename, not a path: ../thread__20260320T012640Z__slack__C08MGJXUCUQ__1773969612.866769.json"
                );
            }
            InboxUpdateError::Io(err) => panic!("unexpected io error: {err}"),
        }

        Ok(())
    }
}
