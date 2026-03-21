//! Watches skill roots for changes and broadcasts coarse-grained
//! `FileWatcherEvent`s that higher-level components react to on the next turn.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::Duration;

use codex_exec_server::ExecutorFileSystem;
use notify::Event;
use notify::EventKind;
use notify::RecommendedWatcher;
use notify::RecursiveMode;
use notify::Watcher;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::sleep_until;
use tracing::warn;

use crate::config::Config;
use crate::skills::SkillsManager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileWatcherEvent {
    SkillsChanged {
        environment_id: String,
        paths: Vec<PathBuf>,
    },
}

struct WatchState {
    skills_root_ref_counts: HashMap<PathBuf, usize>,
    skills_root_registrations: HashMap<PathBuf, HashMap<String, usize>>,
}

struct FileWatcherInner {
    watcher: RecommendedWatcher,
    watched_paths: HashMap<PathBuf, RecursiveMode>,
}

const WATCHER_THROTTLE_INTERVAL: Duration = Duration::from_secs(10);

/// Coalesces bursts of paths and emits at most once per interval.
struct ThrottledPaths {
    pending: HashMap<String, HashSet<PathBuf>>,
    next_allowed_at: Instant,
}

impl ThrottledPaths {
    fn new(now: Instant) -> Self {
        Self {
            pending: HashMap::new(),
            next_allowed_at: now,
        }
    }

    fn add(&mut self, paths: HashMap<String, Vec<PathBuf>>) {
        for (environment_id, environment_paths) in paths {
            self.pending
                .entry(environment_id)
                .or_default()
                .extend(environment_paths);
        }
    }

    fn next_deadline(&self, now: Instant) -> Option<Instant> {
        (!self.pending.is_empty() && now < self.next_allowed_at).then_some(self.next_allowed_at)
    }

    fn take_ready(&mut self, now: Instant) -> Option<Vec<(String, Vec<PathBuf>)>> {
        if self.pending.is_empty() || now < self.next_allowed_at {
            return None;
        }
        Some(self.take_with_next_allowed(now))
    }

    fn take_pending(&mut self, now: Instant) -> Option<Vec<(String, Vec<PathBuf>)>> {
        if self.pending.is_empty() {
            return None;
        }
        Some(self.take_with_next_allowed(now))
    }

    fn take_with_next_allowed(&mut self, now: Instant) -> Vec<(String, Vec<PathBuf>)> {
        let mut pending = self.pending.drain().collect::<Vec<_>>();
        pending.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        let mut paths = Vec::with_capacity(pending.len());
        for (environment_id, environment_paths) in pending {
            let mut environment_paths = environment_paths.into_iter().collect::<Vec<_>>();
            environment_paths.sort_unstable_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
            paths.push((environment_id, environment_paths));
        }
        self.next_allowed_at = now + WATCHER_THROTTLE_INTERVAL;
        paths
    }
}

pub(crate) struct FileWatcher {
    inner: Option<Mutex<FileWatcherInner>>,
    state: Arc<RwLock<WatchState>>,
    tx: broadcast::Sender<FileWatcherEvent>,
}

pub(crate) struct WatchRegistration {
    file_watcher: std::sync::Weak<FileWatcher>,
    environment_id: String,
    roots: Vec<PathBuf>,
}

impl Drop for WatchRegistration {
    fn drop(&mut self) {
        if let Some(file_watcher) = self.file_watcher.upgrade() {
            file_watcher.unregister_roots(&self.environment_id, &self.roots);
        }
    }
}

impl FileWatcher {
    pub(crate) fn new(_codex_home: PathBuf) -> notify::Result<Self> {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let raw_tx_clone = raw_tx;
        let watcher = notify::recommended_watcher(move |res| {
            let _ = raw_tx_clone.send(res);
        })?;
        let inner = FileWatcherInner {
            watcher,
            watched_paths: HashMap::new(),
        };
        let (tx, _) = broadcast::channel(128);
        let state = Arc::new(RwLock::new(WatchState {
            skills_root_ref_counts: HashMap::new(),
            skills_root_registrations: HashMap::new(),
        }));
        let file_watcher = Self {
            inner: Some(Mutex::new(inner)),
            state: Arc::clone(&state),
            tx: tx.clone(),
        };
        file_watcher.spawn_event_loop(raw_rx, state, tx);
        Ok(file_watcher)
    }

    pub(crate) fn noop() -> Self {
        let (tx, _) = broadcast::channel(1);
        Self {
            inner: None,
            state: Arc::new(RwLock::new(WatchState {
                skills_root_ref_counts: HashMap::new(),
                skills_root_registrations: HashMap::new(),
            })),
            tx,
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<FileWatcherEvent> {
        self.tx.subscribe()
    }

    pub(crate) async fn register_config_with_filesystem<F>(
        self: &Arc<Self>,
        config: &Config,
        environment_id: String,
        skills_manager: &SkillsManager,
        filesystem: &F,
    ) -> WatchRegistration
    where
        F: ExecutorFileSystem + ?Sized,
    {
        let deduped_roots: HashSet<PathBuf> = skills_manager
            .skill_roots_for_config_with_filesystem(config, filesystem)
            .await
            .into_iter()
            .map(|root| root.path)
            .collect();
        let mut registered_roots: Vec<PathBuf> = deduped_roots.into_iter().collect();
        registered_roots.sort_unstable_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
        for root in &registered_roots {
            self.register_skills_root(root.clone(), &environment_id);
        }

        WatchRegistration {
            file_watcher: Arc::downgrade(self),
            environment_id,
            roots: registered_roots,
        }
    }

    // Bridge `notify`'s callback-based events into the Tokio runtime and
    // broadcast coarse-grained change signals to subscribers.
    fn spawn_event_loop(
        &self,
        mut raw_rx: mpsc::UnboundedReceiver<notify::Result<Event>>,
        state: Arc<RwLock<WatchState>>,
        tx: broadcast::Sender<FileWatcherEvent>,
    ) {
        if let Ok(handle) = Handle::try_current() {
            handle.spawn(async move {
                let now = Instant::now();
                let mut skills = ThrottledPaths::new(now);

                loop {
                    let now = Instant::now();
                    let next_deadline = skills.next_deadline(now);
                    let timer_deadline = next_deadline
                        .unwrap_or_else(|| now + Duration::from_secs(60 * 60 * 24 * 365));
                    let timer = sleep_until(timer_deadline);
                    tokio::pin!(timer);

                    tokio::select! {
                        res = raw_rx.recv() => {
                            match res {
                                Some(Ok(event)) => {
                                    let skills_paths = classify_event(&event, &state);
                                    let now = Instant::now();
                                    skills.add(skills_paths);

                                    if let Some(changes) = skills.take_ready(now) {
                                        for (environment_id, paths) in changes {
                                            let _ = tx.send(FileWatcherEvent::SkillsChanged {
                                                environment_id,
                                                paths,
                                            });
                                        }
                                    }
                                }
                                Some(Err(err)) => {
                                    warn!("file watcher error: {err}");
                                }
                                None => {
                                    // Flush any pending changes before shutdown so subscribers
                                    // see the latest state.
                                    let now = Instant::now();
                                    if let Some(changes) = skills.take_pending(now) {
                                        for (environment_id, paths) in changes {
                                            let _ = tx.send(FileWatcherEvent::SkillsChanged {
                                                environment_id,
                                                paths,
                                            });
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        _ = &mut timer => {
                            let now = Instant::now();
                            if let Some(changes) = skills.take_ready(now) {
                                for (environment_id, paths) in changes {
                                    let _ = tx.send(FileWatcherEvent::SkillsChanged {
                                        environment_id,
                                        paths,
                                    });
                                }
                            }
                        }
                    }
                }
            });
        } else {
            warn!("file watcher loop skipped: no Tokio runtime available");
        }
    }

    fn register_skills_root(&self, root: PathBuf, environment_id: &str) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state
            .skills_root_registrations
            .entry(root.clone())
            .or_default()
            .entry(environment_id.to_string())
            .or_insert(0) += 1;
        let count = state
            .skills_root_ref_counts
            .entry(root.clone())
            .or_insert(0);
        *count += 1;
        if *count == 1 {
            self.watch_path(root, RecursiveMode::Recursive);
        }
    }

    fn unregister_roots(&self, environment_id: &str, roots: &[PathBuf]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut inner_guard: Option<std::sync::MutexGuard<'_, FileWatcherInner>> = None;

        for root in roots {
            let mut should_unwatch = false;
            if let Some(environment_counts) = state.skills_root_registrations.get_mut(root) {
                if let Some(count) = environment_counts.get_mut(environment_id) {
                    if *count > 1 {
                        *count -= 1;
                    } else {
                        environment_counts.remove(environment_id);
                    }
                }
                if environment_counts.is_empty() {
                    state.skills_root_registrations.remove(root);
                }
            }
            if let Some(count) = state.skills_root_ref_counts.get_mut(root) {
                if *count > 1 {
                    *count -= 1;
                } else {
                    state.skills_root_ref_counts.remove(root);
                    should_unwatch = true;
                }
            }

            if !should_unwatch {
                continue;
            }
            let Some(inner) = &self.inner else {
                continue;
            };
            if inner_guard.is_none() {
                let guard = inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                inner_guard = Some(guard);
            }

            let Some(guard) = inner_guard.as_mut() else {
                continue;
            };
            if guard.watched_paths.remove(root).is_none() {
                continue;
            }
            if let Err(err) = guard.watcher.unwatch(root) {
                warn!("failed to unwatch {}: {err}", root.display());
            }
        }
    }

    fn watch_path(&self, path: PathBuf, mode: RecursiveMode) {
        let Some(inner) = &self.inner else {
            return;
        };
        if !path.exists() {
            return;
        }
        let watch_path = path;
        let mut guard = inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = guard.watched_paths.get(&watch_path) {
            if *existing == RecursiveMode::Recursive || *existing == mode {
                return;
            }
            if let Err(err) = guard.watcher.unwatch(&watch_path) {
                warn!("failed to unwatch {}: {err}", watch_path.display());
            }
        }
        if let Err(err) = guard.watcher.watch(&watch_path, mode) {
            warn!("failed to watch {}: {err}", watch_path.display());
            return;
        }
        guard.watched_paths.insert(watch_path, mode);
    }
}

fn classify_event(event: &Event, state: &RwLock<WatchState>) -> HashMap<String, Vec<PathBuf>> {
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return HashMap::new();
    }

    let registrations = match state.read() {
        Ok(state) => state.skills_root_registrations.clone(),
        Err(err) => err.into_inner().skills_root_registrations.clone(),
    };
    let mut skills_paths = HashMap::<String, HashSet<PathBuf>>::new();

    for path in &event.paths {
        for (root, environments) in &registrations {
            if path.starts_with(root) {
                for environment_id in environments.keys() {
                    skills_paths
                        .entry(environment_id.clone())
                        .or_default()
                        .insert(path.clone());
                }
            }
        }
    }

    skills_paths
        .into_iter()
        .map(|(environment_id, paths)| (environment_id, paths.into_iter().collect()))
        .collect()
}

#[cfg(test)]
#[path = "file_watcher_tests.rs"]
mod tests;
