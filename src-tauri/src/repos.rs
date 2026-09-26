//! The repositories open in this session, addressed by [`RepoId`] (SPEC §4
//! Process model, §5 `RepoInfo.id`, ADR 0008).
//!
//! Each open repository is one [`RepoActor`], which owns the repository's
//! file watcher, plus a task that turns the watcher's batches into
//! `repo-changed` events. One working tree gets one actor however often it
//! is opened: opening it again returns the same id.
//!
//! Only one repository is open at a time. §4 Low-resource operation, rule 6:
//! "Only the active repo has a live watcher and session", and Phase 0 shows
//! one repository. So a successful open closes every other repository: its
//! actor, watcher and event task stop, and its id stops working (a later
//! open of it gets a new one). The 60 s HEAD poll that rule 6 gives inactive
//! repositories arrives with the multi-repo sidebar (§3 G1).
//!
//! Opening a repository:
//!
//! 1. resolves git, honouring the git path from settings (§5 tier 1);
//! 2. finds the repository that contains the path, on the blocking pool,
//!    because discovery is `stat` calls and small reads that a network drive
//!    can make slow;
//! 3. starts its actor and watcher. A watcher that cannot start (an
//!    exhausted inotify watch limit, say) does not stop the repository from
//!    opening: the failure is reported with the result instead;
//! 4. reads the first status, which is also the check that git works there:
//!    a repository owned by another user fails with the `safe.directory`
//!    fix, and a repository that fails is not kept or remembered;
//! 5. makes it the one open repository, closing the others. Only now, so a
//!    failed open leaves the repository the user is looking at open;
//! 6. records it as recent, on the blocking pool because saving settings
//!    fsyncs. Failing to save only logs: the repository is open either way.
//!
//! A new status request for a repository cancels the one still in flight
//! (§4 Low-resource operation, rule 4); the superseded one fails with
//! `cancelled`, which the frontend drops.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use git_engine::error::GitError;
use git_engine::process::CancellationToken;
use git_engine::repo::{open_repo, Repo, RepoId};
use git_engine::repo_actor::RepoActor;
use git_engine::status::{status, Status, StatusOptions};
use git_engine::watcher::{Events, WatchError, WatchOptions};
use serde::Serialize;

use crate::commands::error::{with_causes, CommandError};
use crate::events::{RepoChanged, RepoWatchFailed, WatchFailure};
use crate::git::GitResolver;
use crate::settings::SettingsStore;

/// What [`Repos`] reports to the frontend. The app's sink emits each one as
/// its typed Tauri event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoEvent {
    Changed(RepoChanged),
    WatchFailed(RepoWatchFailed),
}

/// Where [`Repos`] sends its events. Called from the runtime's worker
/// threads.
pub type EventSink = Arc<dyn Fn(RepoEvent) + Send + Sync>;

/// The answer to `open_repo`: the first status of the repository, whose `repo.id` addresses it from now on, and why it is not being watched, if it is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct OpenedRepo {
    pub status: Status,
    pub watch_error: Option<WatchFailure>,
}

/// What the user can run when Linux's inotify watch limit is exhausted: raise
/// it now and on every boot. The app only shows it (§5 rule 8).
pub const INOTIFY_LIMIT_FIX: &str = "echo fs.inotify.max_user_watches=524288 | sudo tee /etc/sysctl.d/60-max-user-watches.conf && sudo sysctl --system";

/// The open repository (one at a time, see the module docs), by id. Kept in
/// Tauri's managed state; the commands reach it through
/// `tauri::State<'_, Repos>`.
pub struct Repos {
    settings: Arc<SettingsStore>,
    git: GitResolver,
    sink: EventSink,
    /// Held for the whole of an open, so that the same working tree opened
    /// twice at once still gets one actor. Status reads do not take it.
    opening: tokio::sync::Mutex<()>,
    sessions: Mutex<HashMap<RepoId, Arc<Session>>>,
}

impl Repos {
    pub fn new(settings: Arc<SettingsStore>, sink: EventSink) -> Self {
        Self {
            settings,
            git: GitResolver::new(),
            sink,
            opening: tokio::sync::Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Opens the repository that contains `path` (see the module docs), or
    /// returns the one already open there.
    pub async fn open(&self, path: PathBuf) -> Result<OpenedRepo, CommandError> {
        let _opening = self.opening.lock().await;
        let git = self.git.resolve(self.settings.get().git_path).await?;
        let repo = discover(git.path, path).await?;

        if let Some(open) = self.find(repo.workdir()) {
            let status = open.status().await?;
            self.make_active(&open);
            self.remember(open.actor.repo().workdir()).await;
            return Ok(open.opened(status));
        }

        let (session, events) = Session::start(repo).await;
        // Dropping `session` on failure stops its actor and watcher: a
        // repository git refuses to work in is neither kept nor remembered,
        // and the one open before stays open.
        let status = session.status().await?;
        let id = session.actor.id();
        self.make_active(&session);
        if let Some(events) = events {
            drop(tauri::async_runtime::spawn(forward(
                id,
                events,
                Arc::clone(&session.watch_error),
                Arc::clone(&self.sink),
            )));
        }
        self.remember(session.actor.repo().workdir()).await;
        Ok(session.opened(status))
    }

    /// The status of the open repository `id`, cancelling a status read of
    /// it that is still in flight.
    pub async fn status(&self, id: RepoId) -> Result<Status, CommandError> {
        let session = self
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| CommandError::unknown_repo(id))?;
        session.status().await
    }

    /// Makes `session` the only open repository (§4 rule 6). The others are
    /// dropped outside the lock: once no status read holds them any more,
    /// their actors end, their watchers stop, and their event tasks see the
    /// end of the stream.
    fn make_active(&self, session: &Arc<Session>) {
        let id = session.actor.id();
        let mut closed = {
            let mut sessions = self.lock();
            let closed = std::mem::take(&mut *sessions);
            sessions.insert(id, Arc::clone(session));
            closed
        };
        closed.remove(&id);
        for closed_id in closed.keys() {
            tracing::debug!(repo = %closed_id, active = %id, "closed an inactive repository");
        }
    }

    fn find(&self, workdir: &Path) -> Option<Arc<Session>> {
        self.lock()
            .values()
            .find(|session| session.actor.repo().workdir() == workdir)
            .cloned()
    }

    /// Records `workdir` as the most recent repository. Best effort: the
    /// repository is open whether or not this is saved.
    async fn remember(&self, workdir: &Path) {
        if workdir.to_str().is_none() {
            tracing::info!(
                workdir = %workdir.display(),
                "the repository path is not valid UTF-8; not recorded as recent (ADR 0008)",
            );
            return;
        }
        let settings = Arc::clone(&self.settings);
        let workdir = workdir.to_path_buf();
        let saved = tauri::async_runtime::spawn_blocking(move || {
            settings.update(|settings| settings.record_recent_repo(workdir))
        })
        .await;
        match saved {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %with_causes(&error), "could not save the recent repositories")
            }
            Err(error) => tracing::warn!(%error, "saving the recent repositories did not finish"),
        }
    }

    /// The map is never locked across an `await`, and a panic while it is
    /// locked cannot leave it half-changed (every change is one insert).
    fn lock(&self) -> MutexGuard<'_, HashMap<RepoId, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// [`open_repo`] on the blocking pool.
async fn discover(git: PathBuf, path: PathBuf) -> Result<Repo, CommandError> {
    tauri::async_runtime::spawn_blocking(move || open_repo(git, path))
        .await
        .map_err(|_| CommandError::from(GitError::Aborted))?
        .map_err(CommandError::from)
}

/// One open repository.
struct Session {
    actor: RepoActor,
    /// Why the repository is not being watched, if it is not. Set when the
    /// watcher cannot start, and by [`forward`] when it stops.
    watch_error: Arc<Mutex<Option<WatchFailure>>>,
    status_in_flight: InFlight,
}

impl Session {
    /// Starts the actor with a watcher, or without one if the watcher cannot
    /// start; then the failure is recorded instead.
    async fn start(repo: Repo) -> (Arc<Self>, Option<Events>) {
        let (actor, events, watch_error) =
            match RepoActor::spawn_watched(repo.clone(), WatchOptions::default()).await {
                Ok((actor, events)) => (actor, Some(events), None),
                Err(error) => {
                    let failure = start_failure(&error);
                    tracing::warn!(
                        workdir = %repo.workdir().display(),
                        error = %failure.message,
                        "could not watch the repository; opening it without a watcher",
                    );
                    (RepoActor::spawn(repo), None, Some(failure))
                }
            };
        let session = Self {
            actor,
            watch_error: Arc::new(Mutex::new(watch_error)),
            status_in_flight: InFlight::default(),
        };
        (Arc::new(session), events)
    }

    async fn status(&self) -> Result<Status, CommandError> {
        let cancel = self.status_in_flight.replace();
        self.actor
            .read(
                move |repo| async move { status(&repo, &StatusOptions::default(), &cancel).await },
            )
            .await
            .map_err(CommandError::from)
    }

    fn opened(&self, status: Status) -> OpenedRepo {
        OpenedRepo {
            status,
            watch_error: lock(&self.watch_error).clone(),
        }
    }
}

/// The cancellation token of the request in flight.
#[derive(Debug, Default)]
struct InFlight(Mutex<CancellationToken>);

impl InFlight {
    /// Cancels the request in flight, if there is one, and returns the token
    /// of the request that replaces it.
    fn replace(&self) -> CancellationToken {
        let mut current = lock(&self.0);
        current.cancel();
        *current = CancellationToken::new();
        current.clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Turns the watcher's batches for repository `id` into events until the
/// stream ends: when the actor is gone, or after the watcher failed. A
/// failure is recorded in `watch_error` and reported.
async fn forward(
    id: RepoId,
    mut events: Events,
    watch_error: Arc<Mutex<Option<WatchFailure>>>,
    sink: EventSink,
) {
    while let Some(batch) = events.recv().await {
        match batch {
            Ok(changed) => sink(RepoEvent::Changed(RepoChanged {
                repo_id: id,
                kinds: changed.kinds,
            })),
            Err(error) => {
                let failure = watch_failure(&error);
                tracing::warn!(repo = %id, error = %failure.message, "the repository watcher stopped");
                *lock(&watch_error) = Some(failure.clone());
                sink(RepoEvent::WatchFailed(RepoWatchFailed {
                    repo_id: id,
                    error: failure,
                }));
            }
        }
    }
    tracing::debug!(repo = %id, "repository events ended");
}

fn watch_failure(error: &WatchError) -> WatchFailure {
    WatchFailure {
        message: with_causes(error),
        is_watch_limit: error.is_watch_limit(),
        fix: error.is_watch_limit().then(|| INOTIFY_LIMIT_FIX.to_owned()),
    }
}

/// Why [`RepoActor::spawn_watched`] failed: the watcher itself, or a path of
/// the repository that could not be resolved.
fn start_failure(error: &GitError) -> WatchFailure {
    match error {
        GitError::Watch(watch) => watch_failure(watch),
        other => WatchFailure {
            message: with_causes(other),
            is_watch_limit: false,
            fix: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use git_engine::watcher::{ChangeKind, ChangeKinds, RepoChanged as Batch};
    use tokio::sync::mpsc;

    use super::*;

    fn recording_sink() -> (EventSink, Arc<Mutex<Vec<RepoEvent>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| lock(&seen).push(event))
        };
        (sink, seen)
    }

    fn id() -> RepoId {
        Repo::new("git", "/work/a").id()
    }

    #[test]
    fn a_new_request_cancels_the_one_in_flight() {
        let in_flight = InFlight::default();

        let first = in_flight.replace();
        assert!(!first.is_cancelled());
        let second = in_flight.replace();

        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }

    #[test]
    fn batches_become_repo_changed_events_with_the_repo_id() {
        let (sink, seen) = recording_sink();
        let (tx, rx) = mpsc::channel(4);
        let id = id();
        let kinds: ChangeKinds = [ChangeKind::Status, ChangeKind::Head].into_iter().collect();

        tauri::async_runtime::block_on(async {
            tx.send(Ok(Batch {
                kinds,
                generation: 3,
            }))
            .await
            .unwrap();
            drop(tx);
            forward(id, rx, Arc::default(), sink).await;
        });

        assert_eq!(
            *lock(&seen),
            [RepoEvent::Changed(RepoChanged { repo_id: id, kinds })]
        );
    }

    /// SPEC §5 rule 8: a failed watcher is shown as a warning with the fix.
    #[test]
    fn a_stopped_watcher_is_reported_and_recorded() {
        let (sink, seen) = recording_sink();
        let (tx, rx) = mpsc::channel(4);
        let id = id();
        let watch_error = Arc::new(Mutex::new(None));

        tauri::async_runtime::block_on(async {
            tx.send(Err(WatchError::Stopped(notify::Error::new(
                notify::ErrorKind::MaxFilesWatch,
            ))))
            .await
            .unwrap();
            drop(tx);
            forward(id, rx, Arc::clone(&watch_error), sink).await;
        });

        let expected = WatchFailure {
            message: with_causes(&WatchError::Stopped(notify::Error::new(
                notify::ErrorKind::MaxFilesWatch,
            ))),
            is_watch_limit: true,
            fix: Some(INOTIFY_LIMIT_FIX.to_owned()),
        };
        assert!(
            expected.message.starts_with("the file watcher stopped: "),
            "{}",
            expected.message
        );
        assert_eq!(*lock(&watch_error), Some(expected.clone()));
        assert_eq!(
            *lock(&seen),
            [RepoEvent::WatchFailed(RepoWatchFailed {
                repo_id: id,
                error: expected
            })]
        );
    }

    #[test]
    fn a_watcher_that_could_not_start_has_no_fix_unless_it_hit_the_limit() {
        let missing = start_failure(&GitError::Io {
            path: PathBuf::from("/gone"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
        });
        assert!(!missing.is_watch_limit);
        assert_eq!(missing.fix, None);

        let limit = start_failure(&GitError::Watch(WatchError::Watch {
            path: PathBuf::from("/work"),
            source: notify::Error::new(notify::ErrorKind::MaxFilesWatch),
        }));
        assert!(limit.is_watch_limit);
        assert_eq!(limit.fix.as_deref(), Some(INOTIFY_LIMIT_FIX));
    }
}
