use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use tracing::warn;

use super::ARCHIVED_SESSIONS_SUBDIR;
use super::SESSIONS_SUBDIR;

pub fn feedback_rollout_attachment_paths(
    codex_home: &Path,
    rollout_path: Option<&Path>,
) -> Vec<PathBuf> {
    let Some(rollout_path) = rollout_path else {
        return Vec::new();
    };

    let mut attachment_paths = Vec::new();
    let mut seen_paths = HashSet::new();
    push_existing_unique_path(
        &mut attachment_paths,
        &mut seen_paths,
        rollout_path.to_path_buf(),
    );

    let guardian_thread_ids = match guardian_thread_ids_from_rollout(rollout_path) {
        Ok(thread_ids) => thread_ids,
        Err(err) => {
            warn!(
                path = %rollout_path.display(),
                error = %err,
                "failed to read guardian review thread ids from rollout"
            );
            return attachment_paths;
        }
    };

    for guardian_thread_id in guardian_thread_ids {
        let Some(guardian_rollout_path) =
            find_rollout_path_by_thread_id(codex_home, guardian_thread_id)
        else {
            continue;
        };
        push_existing_unique_path(
            &mut attachment_paths,
            &mut seen_paths,
            guardian_rollout_path,
        );
    }

    attachment_paths
}

fn guardian_thread_ids_from_rollout(rollout_path: &Path) -> io::Result<Vec<ThreadId>> {
    let file = fs::File::open(rollout_path)?;
    let reader = BufReader::new(file);
    let mut thread_ids = Vec::new();
    let mut seen_thread_ids = HashSet::new();

    for line in reader.lines() {
        let line = line?;
        let rollout_line = match serde_json::from_str::<RolloutLine>(&line) {
            Ok(rollout_line) => rollout_line,
            Err(err) => {
                warn!(
                    path = %rollout_path.display(),
                    error = %err,
                    "failed to parse rollout line while collecting guardian review thread ids"
                );
                continue;
            }
        };
        if let RolloutItem::EventMsg(EventMsg::GuardianAssessment(assessment)) = rollout_line.item
            && let Some(guardian_thread_id) = assessment.guardian_thread_id
            && seen_thread_ids.insert(guardian_thread_id)
        {
            thread_ids.push(guardian_thread_id);
        }
    }

    Ok(thread_ids)
}

fn find_rollout_path_by_thread_id(codex_home: &Path, thread_id: ThreadId) -> Option<PathBuf> {
    let thread_id = thread_id.to_string();
    find_rollout_path_by_thread_id_in_subdir(codex_home, SESSIONS_SUBDIR, &thread_id).or_else(
        || {
            find_rollout_path_by_thread_id_in_subdir(
                codex_home,
                ARCHIVED_SESSIONS_SUBDIR,
                &thread_id,
            )
        },
    )
}

fn find_rollout_path_by_thread_id_in_subdir(
    codex_home: &Path,
    subdir: &str,
    thread_id: &str,
) -> Option<PathBuf> {
    let root = codex_home.join(subdir);
    if !root.exists() {
        return None;
    }

    let expected_suffix = format!("-{thread_id}.jsonl");
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                warn!(
                    path = %dir.display(),
                    error = %err,
                    "failed to scan rollout directory while resolving guardian rollout"
                );
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    warn!(
                        path = %dir.display(),
                        error = %err,
                        "failed to read rollout directory entry while resolving guardian rollout"
                    );
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(err) => {
                    warn!(
                        path = %path.display(),
                        error = %err,
                        "failed to inspect rollout directory entry while resolving guardian rollout"
                    );
                    continue;
                }
            };

            if file_type.is_dir() {
                dirs.push(path);
                continue;
            }

            if file_type.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&expected_suffix))
            {
                return Some(path);
            }
        }
    }

    None
}

fn push_existing_unique_path(
    attachment_paths: &mut Vec<PathBuf>,
    seen_paths: &mut HashSet<PathBuf>,
    path: PathBuf,
) {
    if path.exists() && seen_paths.insert(path.clone()) {
        attachment_paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use codex_protocol::protocol::GuardianAssessmentEvent;
    use codex_protocol::protocol::GuardianAssessmentStatus;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn feedback_rollout_attachment_paths_include_guardian_rollouts() {
        let tempdir = tempdir().expect("tempdir");
        let codex_home = tempdir.path();

        let parent_thread_id = ThreadId::new();
        let guardian_thread_id = ThreadId::new();
        let parent_rollout_path = write_rollout(
            codex_home,
            SESSIONS_SUBDIR,
            parent_thread_id,
            &[
                GuardianAssessmentEvent {
                    id: "assessment-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    guardian_thread_id: Some(guardian_thread_id),
                    status: GuardianAssessmentStatus::Denied,
                    risk_score: Some(100),
                    risk_level: None,
                    rationale: Some("too risky".to_string()),
                    action: None,
                },
                GuardianAssessmentEvent {
                    id: "assessment-2".to_string(),
                    turn_id: "turn-2".to_string(),
                    guardian_thread_id: Some(guardian_thread_id),
                    status: GuardianAssessmentStatus::Approved,
                    risk_score: Some(0),
                    risk_level: None,
                    rationale: Some("safe".to_string()),
                    action: None,
                },
            ],
        );
        let guardian_rollout_path = write_rollout(
            codex_home,
            ARCHIVED_SESSIONS_SUBDIR,
            guardian_thread_id,
            &[],
        );

        let attachment_paths =
            feedback_rollout_attachment_paths(codex_home, Some(parent_rollout_path.as_path()));

        assert_eq!(
            attachment_paths,
            vec![parent_rollout_path, guardian_rollout_path]
        );
    }

    #[test]
    fn feedback_rollout_attachment_paths_ignore_missing_guardian_rollouts() {
        let tempdir = tempdir().expect("tempdir");
        let codex_home = tempdir.path();

        let parent_thread_id = ThreadId::new();
        let missing_guardian_thread_id = ThreadId::new();
        let parent_rollout_path = write_rollout(
            codex_home,
            SESSIONS_SUBDIR,
            parent_thread_id,
            &[GuardianAssessmentEvent {
                id: "assessment-1".to_string(),
                turn_id: "turn-1".to_string(),
                guardian_thread_id: Some(missing_guardian_thread_id),
                status: GuardianAssessmentStatus::Denied,
                risk_score: Some(100),
                risk_level: None,
                rationale: Some("too risky".to_string()),
                action: None,
            }],
        );

        let attachment_paths =
            feedback_rollout_attachment_paths(codex_home, Some(parent_rollout_path.as_path()));

        assert_eq!(attachment_paths, vec![parent_rollout_path]);
    }

    fn write_rollout(
        codex_home: &Path,
        subdir: &str,
        thread_id: ThreadId,
        assessments: &[GuardianAssessmentEvent],
    ) -> PathBuf {
        let dir = codex_home.join(subdir).join("2026").join("03").join("18");
        fs::create_dir_all(&dir).expect("create rollout dir");
        let path = dir.join(format!("rollout-2026-03-18T12-00-00-{thread_id}.jsonl"));

        let contents = assessments
            .iter()
            .map(|assessment| {
                serde_json::to_string(&RolloutLine {
                    timestamp: "2026-03-18T12:00:00.000Z".to_string(),
                    item: RolloutItem::EventMsg(EventMsg::GuardianAssessment(assessment.clone())),
                })
                .expect("serialize rollout line")
            })
            .collect::<Vec<_>>()
            .join("\n");

        if contents.is_empty() {
            fs::write(&path, "").expect("write rollout");
        } else {
            fs::write(&path, format!("{contents}\n")).expect("write rollout");
        }

        path
    }
}
