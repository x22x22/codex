use std::num::NonZero;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_app_server_protocol::FuzzyFileSearchMatchType;
use codex_app_server_protocol::FuzzyFileSearchResult;
use codex_app_server_protocol::FuzzyFileSearchSessionCompletedNotification;
use codex_app_server_protocol::FuzzyFileSearchSessionUpdatedNotification;
use codex_app_server_protocol::ServerNotification;
use codex_exec_server::ExecutorFileSystem;
use codex_file_search as file_search;
use codex_file_search::SearchCandidate;
use codex_utils_absolute_path::AbsolutePathBuf;
use tracing::warn;

use crate::outgoing_message::OutgoingMessageSender;

const MATCH_LIMIT: usize = 50;
const MAX_THREADS: usize = 12;

pub(crate) async fn run_fuzzy_file_search(
    query: String,
    roots: Vec<String>,
    cancellation_flag: Arc<AtomicBool>,
) -> Vec<FuzzyFileSearchResult> {
    if roots.is_empty() {
        return Vec::new();
    }

    #[expect(clippy::expect_used)]
    let limit = NonZero::new(MATCH_LIMIT).expect("MATCH_LIMIT should be a valid non-zero usize");

    let cores = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1);
    let threads = cores.min(MAX_THREADS);
    #[expect(clippy::expect_used)]
    let threads = NonZero::new(threads.max(1)).expect("threads should be non-zero");
    let search_dirs: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();

    let mut files = match tokio::task::spawn_blocking(move || {
        file_search::run(
            query.as_str(),
            search_dirs,
            file_search::FileSearchOptions {
                limit,
                threads,
                compute_indices: true,
                ..Default::default()
            },
            Some(cancellation_flag),
        )
    })
    .await
    {
        Ok(Ok(res)) => res
            .matches
            .into_iter()
            .map(|m| {
                let file_name = m.path.file_name().unwrap_or_default();
                FuzzyFileSearchResult {
                    root: m.root.to_string_lossy().to_string(),
                    path: m.path.to_string_lossy().to_string(),
                    match_type: match m.match_type {
                        file_search::MatchType::File => FuzzyFileSearchMatchType::File,
                        file_search::MatchType::Directory => FuzzyFileSearchMatchType::Directory,
                    },
                    file_name: file_name.to_string_lossy().to_string(),
                    score: m.score,
                    indices: m.indices,
                }
            })
            .collect::<Vec<_>>(),
        Ok(Err(err)) => {
            warn!("fuzzy-file-search failed: {err}");
            Vec::new()
        }
        Err(err) => {
            warn!("fuzzy-file-search join failed: {err}");
            Vec::new()
        }
    };

    files.sort_by(file_search::cmp_by_score_desc_then_path_asc::<
        FuzzyFileSearchResult,
        _,
        _,
    >(|f| f.score, |f| f.path.as_str()));

    files
}

pub(crate) async fn run_fuzzy_file_search_with_filesystem<F>(
    query: String,
    roots: Vec<String>,
    filesystem: &F,
    cancellation_flag: Arc<AtomicBool>,
) -> Vec<FuzzyFileSearchResult>
where
    F: ExecutorFileSystem + ?Sized,
{
    if roots.is_empty() || cancellation_flag.load(Ordering::Relaxed) {
        return Vec::new();
    }

    #[expect(clippy::expect_used)]
    let limit = NonZero::new(MATCH_LIMIT).expect("MATCH_LIMIT should be a valid non-zero usize");

    let mut candidates = Vec::new();
    for root in roots {
        if cancellation_flag.load(Ordering::Relaxed) {
            break;
        }
        let root_path = PathBuf::from(&root);
        let Ok(root) = AbsolutePathBuf::from_absolute_path(root_path.as_path()) else {
            continue;
        };
        collect_search_candidates(&root, filesystem, &cancellation_flag, &mut candidates).await;
    }

    let results = match file_search::run_with_candidates(
        &query,
        candidates,
        file_search::FileSearchOptions {
            limit,
            compute_indices: true,
            ..Default::default()
        },
        Some(cancellation_flag),
    ) {
        Ok(results) => results,
        Err(err) => {
            warn!("environment fuzzy-file-search failed: {err}");
            return Vec::new();
        }
    };

    let mut files = results
        .matches
        .into_iter()
        .map(|m| {
            let file_name = m.path.file_name().unwrap_or_default();
            FuzzyFileSearchResult {
                root: m.root.to_string_lossy().to_string(),
                path: m.path.to_string_lossy().to_string(),
                match_type: match m.match_type {
                    file_search::MatchType::File => FuzzyFileSearchMatchType::File,
                    file_search::MatchType::Directory => FuzzyFileSearchMatchType::Directory,
                },
                file_name: file_name.to_string_lossy().to_string(),
                score: m.score,
                indices: m.indices,
            }
        })
        .collect::<Vec<_>>();

    files.sort_by(file_search::cmp_by_score_desc_then_path_asc::<
        FuzzyFileSearchResult,
        _,
        _,
    >(|f| f.score, |f| f.path.as_str()));
    files
}

async fn collect_search_candidates<F>(
    root: &AbsolutePathBuf,
    filesystem: &F,
    cancellation_flag: &AtomicBool,
    out: &mut Vec<SearchCandidate>,
) where
    F: ExecutorFileSystem + ?Sized,
{
    let mut pending = vec![root.clone()];
    while let Some(current) = pending.pop() {
        if cancellation_flag.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entries) = filesystem.read_directory(&current).await else {
            continue;
        };
        for entry in entries {
            if cancellation_flag.load(Ordering::Relaxed) {
                return;
            }
            let path = current.as_path().join(&entry.file_name);
            let Ok(path) = AbsolutePathBuf::try_from(path) else {
                continue;
            };
            let Ok(relative_path) = path.as_path().strip_prefix(root.as_path()) else {
                continue;
            };
            if entry.is_file || entry.is_directory {
                out.push(SearchCandidate {
                    root: root.clone().into_path_buf(),
                    path: relative_path.to_path_buf(),
                    match_type: if entry.is_directory {
                        file_search::MatchType::Directory
                    } else {
                        file_search::MatchType::File
                    },
                });
            }
            if entry.is_directory {
                pending.push(path);
            }
        }
    }
}

pub(crate) struct FuzzyFileSearchSession {
    session: file_search::FileSearchSession,
    shared: Arc<SessionShared>,
}

impl FuzzyFileSearchSession {
    pub(crate) fn update_query(&self, query: String) {
        if self.shared.canceled.load(Ordering::Relaxed) {
            return;
        }
        {
            #[expect(clippy::unwrap_used)]
            let mut latest_query = self.shared.latest_query.lock().unwrap();
            *latest_query = query.clone();
        }
        self.session.update_query(&query);
    }
}

impl Drop for FuzzyFileSearchSession {
    fn drop(&mut self) {
        self.shared.canceled.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn start_fuzzy_file_search_session(
    session_id: String,
    roots: Vec<String>,
    outgoing: Arc<OutgoingMessageSender>,
) -> anyhow::Result<FuzzyFileSearchSession> {
    #[expect(clippy::expect_used)]
    let limit = NonZero::new(MATCH_LIMIT).expect("MATCH_LIMIT should be a valid non-zero usize");
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1);
    let threads = cores.min(MAX_THREADS);
    #[expect(clippy::expect_used)]
    let threads = NonZero::new(threads.max(1)).expect("threads should be non-zero");
    let search_dirs: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
    let canceled = Arc::new(AtomicBool::new(false));

    let shared = Arc::new(SessionShared {
        session_id,
        latest_query: Mutex::new(String::new()),
        outgoing,
        runtime: tokio::runtime::Handle::current(),
        canceled: canceled.clone(),
    });

    let reporter = Arc::new(SessionReporterImpl {
        shared: shared.clone(),
    });
    let session = file_search::create_session(
        search_dirs,
        file_search::FileSearchOptions {
            limit,
            threads,
            compute_indices: true,
            ..Default::default()
        },
        reporter,
        Some(canceled),
    )?;

    Ok(FuzzyFileSearchSession { session, shared })
}

struct SessionShared {
    session_id: String,
    latest_query: Mutex<String>,
    outgoing: Arc<OutgoingMessageSender>,
    runtime: tokio::runtime::Handle,
    canceled: Arc<AtomicBool>,
}

struct SessionReporterImpl {
    shared: Arc<SessionShared>,
}

impl SessionReporterImpl {
    fn send_snapshot(&self, snapshot: &file_search::FileSearchSnapshot) {
        if self.shared.canceled.load(Ordering::Relaxed) {
            return;
        }

        let query = {
            #[expect(clippy::unwrap_used)]
            self.shared.latest_query.lock().unwrap().clone()
        };
        if snapshot.query != query {
            return;
        }

        let files = if query.is_empty() {
            Vec::new()
        } else {
            collect_files(snapshot)
        };

        let notification = ServerNotification::FuzzyFileSearchSessionUpdated(
            FuzzyFileSearchSessionUpdatedNotification {
                session_id: self.shared.session_id.clone(),
                query,
                files,
            },
        );
        let outgoing = self.shared.outgoing.clone();
        self.shared.runtime.spawn(async move {
            outgoing.send_server_notification(notification).await;
        });
    }

    fn send_complete(&self) {
        if self.shared.canceled.load(Ordering::Relaxed) {
            return;
        }
        let session_id = self.shared.session_id.clone();
        let outgoing = self.shared.outgoing.clone();
        self.shared.runtime.spawn(async move {
            let notification = ServerNotification::FuzzyFileSearchSessionCompleted(
                FuzzyFileSearchSessionCompletedNotification { session_id },
            );
            outgoing.send_server_notification(notification).await;
        });
    }
}

impl file_search::SessionReporter for SessionReporterImpl {
    fn on_update(&self, snapshot: &file_search::FileSearchSnapshot) {
        self.send_snapshot(snapshot);
    }

    fn on_complete(&self) {
        self.send_complete();
    }
}

fn collect_files(snapshot: &file_search::FileSearchSnapshot) -> Vec<FuzzyFileSearchResult> {
    let mut files = snapshot
        .matches
        .iter()
        .map(|m| {
            let file_name = m.path.file_name().unwrap_or_default();
            FuzzyFileSearchResult {
                root: m.root.to_string_lossy().to_string(),
                path: m.path.to_string_lossy().to_string(),
                match_type: match m.match_type {
                    file_search::MatchType::File => FuzzyFileSearchMatchType::File,
                    file_search::MatchType::Directory => FuzzyFileSearchMatchType::Directory,
                },
                file_name: file_name.to_string_lossy().to_string(),
                score: m.score,
                indices: m.indices.clone(),
            }
        })
        .collect::<Vec<_>>();

    files.sort_by(file_search::cmp_by_score_desc_then_path_asc::<
        FuzzyFileSearchResult,
        _,
        _,
    >(|f| f.score, |f| f.path.as_str()));
    files
}
