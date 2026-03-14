use codex_core::INTERACTIVE_SESSION_SOURCES;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::fs;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;

const HEAD_LINES_TO_SCAN: usize = 40;
const MAX_RECENT_INTERACTIVE_SESSIONS: usize = 40;
const MAX_RECOMMENDED_DIRECTORIES: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConfigWizardRecommendation {
    pub(crate) access_mode: SandboxMode,
    pub(crate) network_access: bool,
    pub(crate) directories: Vec<AbsolutePathBuf>,
    pub(crate) sampled_sessions: usize,
    pub(crate) workspace_write_sessions: usize,
    pub(crate) workspace_write_network_sessions: usize,
}

impl ConfigWizardRecommendation {
    pub(crate) fn summary(&self) -> String {
        let mut parts = vec![format!(
            "Recommended from {} recent interactive sessions: {}",
            self.sampled_sessions,
            match self.access_mode {
                SandboxMode::ReadOnly => "read-only access",
                SandboxMode::WorkspaceWrite => "workspace-write access",
                SandboxMode::DangerFullAccess => "full access",
            }
        )];

        if self.workspace_write_sessions > 0 {
            parts.push(format!(
                "network {} in {}/{} workspace-write sessions",
                if self.network_access {
                    "enabled"
                } else {
                    "disabled"
                },
                self.workspace_write_network_sessions,
                self.workspace_write_sessions
            ));
        }

        if !self.directories.is_empty() {
            let dirs = self
                .directories
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("common directories: {dirs}"));
        }

        format!("{}.", parts.join("; "))
    }
}

#[derive(Clone, Debug)]
struct SessionRecommendationSample {
    cwd: PathBuf,
    sandbox_policy: SandboxPolicy,
}

pub(crate) fn detect_config_wizard_recommendation_for_codex_home(
    codex_home: &Path,
    current_cwd: Option<&Path>,
) -> Option<ConfigWizardRecommendation> {
    detect_config_wizard_recommendation_in_root(codex_home.join("sessions"), current_cwd)
}

fn detect_config_wizard_recommendation_in_root(
    sessions_root: PathBuf,
    current_cwd: Option<&Path>,
) -> Option<ConfigWizardRecommendation> {
    if !sessions_root.exists() {
        return None;
    }

    let codex_home = sessions_root.parent();
    let rollout_paths = newest_rollout_paths(&sessions_root, MAX_RECENT_INTERACTIVE_SESSIONS * 4);
    if rollout_paths.is_empty() {
        return None;
    }

    let mut samples = Vec::new();
    for rollout_path in rollout_paths {
        let Some(sample) = read_session_sample(&rollout_path) else {
            continue;
        };
        samples.push(sample);
        if samples.len() == MAX_RECENT_INTERACTIVE_SESSIONS {
            break;
        }
    }

    if samples.is_empty() {
        return None;
    }

    let workspace_write_sessions = samples
        .iter()
        .filter(|sample| matches!(sample.sandbox_policy, SandboxPolicy::WorkspaceWrite { .. }))
        .count();
    let workspace_write_network_sessions = samples
        .iter()
        .filter(|sample| {
            matches!(sample.sandbox_policy, SandboxPolicy::WorkspaceWrite { .. })
                && sample.sandbox_policy.has_full_network_access()
        })
        .count();

    Some(ConfigWizardRecommendation {
        access_mode: recommended_access_mode(&samples),
        network_access: workspace_write_sessions > 0
            && workspace_write_network_sessions * 2 >= workspace_write_sessions,
        directories: recommended_directories(&samples, current_cwd, codex_home),
        sampled_sessions: samples.len(),
        workspace_write_sessions,
        workspace_write_network_sessions,
    })
}

fn recommended_access_mode(samples: &[SessionRecommendationSample]) -> SandboxMode {
    let mut read_only_count = 0usize;
    let mut workspace_write_count = 0usize;
    let mut full_access_count = 0usize;

    for sample in samples {
        match sample.sandbox_policy {
            SandboxPolicy::DangerFullAccess => full_access_count += 1,
            SandboxPolicy::ReadOnly { .. } => read_only_count += 1,
            SandboxPolicy::WorkspaceWrite { .. } | SandboxPolicy::ExternalSandbox { .. } => {
                workspace_write_count += 1;
            }
        }
    }

    [
        (SandboxMode::WorkspaceWrite, workspace_write_count),
        (SandboxMode::ReadOnly, read_only_count),
        (SandboxMode::DangerFullAccess, full_access_count),
    ]
    .into_iter()
    .max_by_key(|(mode, count)| (*count, access_mode_priority(*mode)))
    .map(|(mode, _)| mode)
    .unwrap_or(SandboxMode::WorkspaceWrite)
}

fn access_mode_priority(mode: SandboxMode) -> u8 {
    match mode {
        SandboxMode::WorkspaceWrite => 3,
        SandboxMode::ReadOnly => 2,
        SandboxMode::DangerFullAccess => 1,
    }
}

fn recommended_directories(
    samples: &[SessionRecommendationSample],
    current_cwd: Option<&Path>,
    codex_home: Option<&Path>,
) -> Vec<AbsolutePathBuf> {
    let mut counts: Vec<(AbsolutePathBuf, usize)> = Vec::new();

    for sample in samples {
        for directory in sample.recommended_directories(current_cwd, codex_home) {
            if let Some((_, count)) = counts.iter_mut().find(|(path, _)| *path == directory) {
                *count += 1;
            } else {
                counts.push((directory, 1));
            }
        }
    }

    counts.sort_by(|(left_path, left_count), (right_path, right_count)| {
        right_count.cmp(left_count).then_with(|| {
            left_path
                .display()
                .to_string()
                .cmp(&right_path.display().to_string())
        })
    });

    counts
        .into_iter()
        .map(|(path, _)| path)
        .take(MAX_RECOMMENDED_DIRECTORIES)
        .collect()
}

impl SessionRecommendationSample {
    fn recommended_directories(
        &self,
        current_cwd: Option<&Path>,
        codex_home: Option<&Path>,
    ) -> Vec<AbsolutePathBuf> {
        let mut directories = Vec::new();
        push_recommended_directory(&mut directories, &self.cwd, current_cwd, codex_home);

        if let SandboxPolicy::WorkspaceWrite { .. } = &self.sandbox_policy {
            for writable_root in self.sandbox_policy.get_writable_roots_with_cwd(&self.cwd) {
                push_recommended_directory(
                    &mut directories,
                    writable_root.root.as_path(),
                    current_cwd,
                    codex_home,
                );
            }
        }

        directories
    }
}

fn push_recommended_directory(
    directories: &mut Vec<AbsolutePathBuf>,
    candidate: &Path,
    current_cwd: Option<&Path>,
    codex_home: Option<&Path>,
) {
    if !candidate.is_absolute()
        || current_cwd.is_some_and(|cwd| cwd == candidate)
        || is_temp_directory(candidate)
        || is_codex_internal_directory(candidate, codex_home)
    {
        return;
    }

    let Ok(candidate) = AbsolutePathBuf::try_from(candidate.to_path_buf()) else {
        return;
    };
    if !directories.contains(&candidate) {
        directories.push(candidate);
    }
}

fn is_temp_directory(path: &Path) -> bool {
    path.starts_with(std::env::temp_dir())
        || path.starts_with("/tmp")
        || path.starts_with("/private/tmp")
        || path.starts_with("/var/folders")
        || path.starts_with("/private/var/folders")
}

fn is_codex_internal_directory(path: &Path, codex_home: Option<&Path>) -> bool {
    codex_home.is_some_and(|codex_home| {
        path.starts_with(codex_home.join("worktrees"))
            || path.starts_with(codex_home.join("memories"))
    })
}

fn read_session_sample(path: &Path) -> Option<SessionRecommendationSample> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut source = None;
    let mut turn_context = None;

    for line in reader.lines().take(HEAD_LINES_TO_SCAN) {
        let line = line.ok()?;
        let rollout_line = serde_json::from_str::<RolloutLine>(&line).ok()?;
        match rollout_line.item {
            RolloutItem::SessionMeta(meta_line) => {
                source = Some(meta_line.meta.source);
            }
            RolloutItem::TurnContext(context) => {
                turn_context = Some(context);
                if source.is_some() {
                    break;
                }
            }
            _ => {}
        }
    }

    let source = source?;
    if !is_interactive_source(&source) {
        return None;
    }
    let turn_context = turn_context?;

    Some(SessionRecommendationSample {
        cwd: turn_context.cwd,
        sandbox_policy: turn_context.sandbox_policy,
    })
}

fn is_interactive_source(source: &SessionSource) -> bool {
    INTERACTIVE_SESSION_SOURCES.contains(source)
}

fn newest_rollout_paths(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_rollout_paths(root, limit, &mut paths);
    paths
}

fn collect_rollout_paths(dir: &Path, limit: usize, paths: &mut Vec<PathBuf>) {
    if paths.len() >= limit {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut entries = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    entries.reverse();

    for path in entries {
        if paths.len() >= limit {
            return;
        }
        if path.is_dir() {
            collect_rollout_paths(&path, limit, paths);
            continue;
        }
        if is_rollout_path(&path) {
            paths.push(path);
        }
    }
}

fn is_rollout_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-"))
}

#[cfg(test)]
#[path = "config_wizard_recommendation_tests.rs"]
mod tests;
