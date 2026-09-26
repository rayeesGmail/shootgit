//! The repository watcher (SPEC §5 External change sync; §4 IPC contract,
//! "repo-changed ... debounced 150 ms"; §4 Low-resource operation, rule 4
//! "Coalesce and cancel").
//!
//! The app is never the only writer: editors, the git CLI, hooks and CI
//! scripts change the working tree and `.git` whenever they like. A
//! [`Watcher`] watches one repository through the OS (FSEvents on macOS,
//! ReadDirectoryChangesW on Windows, inotify on Linux, all via `notify`) and
//! turns the raw stream into one [`RepoChanged`] per coalescing window, whose
//! [`kinds`](RepoChanged::kinds) say what to refresh.
//!
//! # What is watched
//!
//! The working tree, recursively, and the repository's git directory and
//! common directory when they lie outside it (a linked worktree or a
//! submodule). Inside `.git` only the paths §5 lists count: `HEAD`, `index`,
//! `refs/`, `packed-refs`, the `*_HEAD` operation markers, `rebase-merge/`,
//! `rebase-apply/`, `logs/`, `config` and `info/exclude`. Everything else
//! there (`objects/`, `hooks/`, `COMMIT_EDITMSG`, the `*.lock` files git
//! writes through) is noise and never opens a window. Working-tree paths that
//! `.gitignore`, nested `.gitignore` files, `.git/info/exclude` or the user's
//! global excludes ignore are dropped too, so a build writing `target/` does
//! not cost a status refresh. A change to an ignore file reloads the rules.
//!
//! # Coalescing
//!
//! The first reportable event opens a window of [`WatchOptions::window`]
//! (150 ms, 250 ms on Windows, §5 rule 1). Everything that arrives meanwhile
//! joins the batch, and when the window closes one [`RepoChanged`] carries
//! the union. An editor's atomic save (write a temp file, rename it over the
//! original) is therefore one change (§5 rule 3). While `.git/index.lock`
//! exists another process is mid-write, so the batch is held until the lock
//! goes, but no longer than [`WatchOptions::lock_hold`] (§5 row 7).
//!
//! A directory's own modification event is noise: Windows reports one for
//! the parent whenever an entry inside it is created, removed or renamed
//! (its last-write time moves), and the entry has its own event. Creating,
//! removing or renaming a directory is a change and counts. On macOS,
//! FSEvents may also deliver changes made in the moments before the watcher
//! started; they cost one harmless refresh.
//!
//! Between windows the watcher is idle: no timer runs, nothing polls (§4
//! "Idle CPU 0 %").
//!
//! # Own-write suppression
//!
//! Every engine write is bracketed with [`Watcher::begin_write`], which bumps
//! a generation counter and returns an [`OwnWrite`] guard. Events that
//! arrive while any guard is alive are dropped (§5 rule 2): the caller keeps
//! the guard through its post-write status snapshot, so the snapshot already
//! reflects what the watcher would have reported, and drops it afterwards.
//! An external change that lands inside the bracket is dropped with them and
//! is caught by that same snapshot. The OS may deliver a write's last events
//! after the guard has gone (FSEvents in particular has some latency); those
//! cost one extra, harmless refresh.
//!
//! ```text
//! let own = watcher.begin_write();
//! actor.write(stage_files).await?;
//! let status = actor.read(status).await?;   // the post-write snapshot
//! drop(own);                                // events from here on are external
//! ```
//!
//! # Memory and threads
//!
//! Raw events are reduced to a bitset ([`ChangeKinds`]) in `notify`'s
//! callback, on the thread the backend owns, before anything is queued: a
//! burst of ten thousand events during a build costs a few microseconds each
//! and no allocation, and what waits for the window is one byte. The
//! coalescer is a tokio task on the runtime that spawned it (the single app
//! runtime, ADR 0004) and the backend's own thread is the one thread
//! `notify` manages internally.
//!
//! Events reach the consumer through a bounded channel. A consumer that
//! falls behind does not lose anything and costs no memory: the coalescer
//! waits for room while the callback keeps merging into the same bitset.
//!
//! # Relation to the actor
//!
//! The watcher runs beside the [`RepoActor`](crate::repo_actor::RepoActor),
//! not inside it, so an event that arrives while the actor is inside a write
//! is buffered here and the refresh it prompts queues behind that write. The
//! app layer (P0-12) holds both per repository and turns [`RepoChanged`] into
//! the `repo-changed` IPC event; the serde and specta derives arrive with it.
//!
//! # Not here yet
//!
//! The polling fallback for a failed watcher or an exhausted inotify watch
//! limit (§5 rule 8): the watcher reports the failure as the last item of
//! the stream, with [`WatchError::is_watch_limit`] naming the inotify case,
//! and stops. On Linux, ignored directories are still watched (inotify
//! needs a watch per directory), so a `node_modules/` can exhaust the limit;
//! `core.fsmonitor` as the primary signal (§5 rule 7) is Phase 6 work.

use std::fmt;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use notify::event::{AccessKind, AccessMode, CreateKind, EventKind, ModifyKind, RemoveKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::error::GitError;
use crate::repo::Repo;

/// The coalescing window: 150 ms, and 250 ms on Windows, where
/// ReadDirectoryChangesW is heavier and `git.exe` spawns cost more (§5
/// rule 1, §9 File watching).
pub const DEFAULT_WINDOW: Duration = if cfg!(windows) {
    Duration::from_millis(250)
} else {
    Duration::from_millis(150)
};

/// How long a batch waits for `.git/index.lock` to disappear before it is
/// delivered anyway. A lock older than this is most likely left behind by a
/// crashed process, and holding events for it would freeze the app.
pub const DEFAULT_LOCK_HOLD: Duration = Duration::from_secs(2);

/// Batches the consumer may leave unread before the coalescer waits for it.
/// One batch per window is the ceiling, so this is many seconds of backlog.
const EVENT_QUEUE: usize = 16;

/// Tuning for a [`Watcher`]. `Default` is what the app uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchOptions {
    /// The coalescing window, see [`DEFAULT_WINDOW`].
    pub window: Duration,
    /// How long to hold a batch for `.git/index.lock`, see
    /// [`DEFAULT_LOCK_HOLD`].
    pub lock_hold: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            window: DEFAULT_WINDOW,
            lock_hold: DEFAULT_LOCK_HOLD,
        }
    }
}

/// What part of the repository changed, one per row of the §5 table, named
/// as the `repo-changed` IPC event names them (§4: `status`, `refs`,
/// `index`, `head`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeKind {
    /// A working-tree file was saved, created, deleted or renamed: re-run
    /// status. Also set when the ignore rules change.
    Status,
    /// `.git/index`: staging changed outside the app.
    Index,
    /// `.git/HEAD`: a checkout, or a detach.
    Head,
    /// `.git/refs/**`, `packed-refs`, `ORIG_HEAD`, `FETCH_HEAD`, `shallow`:
    /// a commit, a branch or tag created, moved or deleted, a fetch, a stash.
    Refs,
    /// An operation marker: `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
    /// `BISECT_*`, `rebase-merge/`, `rebase-apply/`, `sequencer/`. A merge,
    /// rebase, cherry-pick, revert or bisect started or ended outside the app.
    State,
    /// `.git/logs/**`: the reflog grew, so the undo history changed.
    Reflog,
    /// `.git/config`, `.git/info/exclude`, `.git/info/attributes` or a
    /// `.gitattributes` in the tree.
    Config,
}

impl ChangeKind {
    /// Every kind, in declaration order.
    pub const ALL: [ChangeKind; 7] = [
        ChangeKind::Status,
        ChangeKind::Index,
        ChangeKind::Head,
        ChangeKind::Refs,
        ChangeKind::State,
        ChangeKind::Reflog,
        ChangeKind::Config,
    ];

    /// The IPC name (§4): `status`, `index`, `head`, `refs`, `state`,
    /// `reflog`, `config`.
    pub const fn name(self) -> &'static str {
        match self {
            ChangeKind::Status => "status",
            ChangeKind::Index => "index",
            ChangeKind::Head => "head",
            ChangeKind::Refs => "refs",
            ChangeKind::State => "state",
            ChangeKind::Reflog => "reflog",
            ChangeKind::Config => "config",
        }
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of [`ChangeKind`]s: one byte, so merging events costs nothing.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ChangeKinds(u8);

impl ChangeKinds {
    pub const EMPTY: Self = Self(0);
    /// Every kind: what a batch carries after the OS reports that it dropped
    /// events, when anything may have changed.
    pub const ALL: Self = Self((1 << ChangeKind::ALL.len()) - 1);

    pub const fn of(kind: ChangeKind) -> Self {
        Self(kind.bit())
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, kind: ChangeKind) -> bool {
        self.0 & kind.bit() != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn insert(&mut self, kind: ChangeKind) {
        self.0 |= kind.bit();
    }

    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The kinds in the set, in [`ChangeKind::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = ChangeKind> {
        ChangeKind::ALL
            .into_iter()
            .filter(move |kind| self.contains(*kind))
    }
}

impl fmt::Debug for ChangeKinds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl From<ChangeKind> for ChangeKinds {
    fn from(kind: ChangeKind) -> Self {
        Self::of(kind)
    }
}

impl std::ops::BitOr for ChangeKinds {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl std::ops::BitOrAssign for ChangeKinds {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl FromIterator<ChangeKind> for ChangeKinds {
    fn from_iter<I: IntoIterator<Item = ChangeKind>>(kinds: I) -> Self {
        let mut set = Self::EMPTY;
        set.extend(kinds);
        set
    }
}

impl Extend<ChangeKind> for ChangeKinds {
    fn extend<I: IntoIterator<Item = ChangeKind>>(&mut self, kinds: I) {
        for kind in kinds {
            self.insert(kind);
        }
    }
}

/// One coalescing window's worth of changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoChanged {
    /// The union of what changed in the window; never empty.
    pub kinds: ChangeKinds,
    /// [`Watcher::generation`] when the batch was delivered: how many engine
    /// writes had begun by then.
    pub generation: u64,
}

/// Why a [`Watcher`] could not start, or stopped.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WatchError {
    /// The OS watcher could not be created.
    #[error("could not start the file watcher")]
    Start(#[source] notify::Error),
    /// `path` could not be watched. On Linux this is where an exhausted
    /// inotify watch limit shows up; see [`is_watch_limit`](Self::is_watch_limit).
    #[error("could not watch {}", path.display())]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
    /// The running watcher reported an error and stopped; this is the last
    /// item of its event stream. Events after it are not being observed, so
    /// the caller should fall back to polling or restart the watcher.
    #[error("the file watcher stopped")]
    Stopped(#[source] notify::Error),
}

impl WatchError {
    /// Whether the OS limit on watches was hit (inotify's
    /// `fs.inotify.max_user_watches`). The fix is a sysctl, which the app
    /// should show (§5 rule 8).
    pub fn is_watch_limit(&self) -> bool {
        let source = match self {
            WatchError::Start(source) | WatchError::Stopped(source) => source,
            WatchError::Watch { source, .. } => source,
        };
        matches!(source.kind, notify::ErrorKind::MaxFilesWatch)
    }
}

/// The event stream of a [`Watcher`]: a [`RepoChanged`] per window, or one
/// [`WatchError`] as the final item if the watcher fails. It ends when the
/// last `Watcher` handle is dropped.
pub type Events = mpsc::Receiver<Result<RepoChanged, WatchError>>;

/// A running watcher on one repository. Cloning is cheap; the watch stops
/// when the last clone is dropped, or when the [`Events`] receiver is.
#[derive(Debug, Clone)]
pub struct Watcher {
    shared: Arc<Shared>,
    _stop: Arc<DropGuard>,
}

impl Watcher {
    /// Starts watching `repo` and returns the handle and its event stream.
    ///
    /// Returns once the OS watch is in place, so a change made after this
    /// returns is reported. Setting the watch up is blocking file-system work
    /// (inotify registers every directory), so it runs on tokio's blocking
    /// pool. Fails with [`GitError::Io`] when a path of `repo` does not exist
    /// and [`GitError::Watch`] when the backend refuses.
    ///
    /// # Panics
    ///
    /// When called outside a tokio runtime, as `tokio::spawn` does.
    pub async fn spawn(repo: &Repo, options: WatchOptions) -> Result<(Watcher, Events), GitError> {
        let repo = repo.clone();
        let shared = Arc::new(Shared::default());
        let started = {
            let shared = Arc::clone(&shared);
            tokio::task::spawn_blocking(move || start(&repo, shared))
        };
        // A join error is a panic inside `start`.
        let (backend, roots) = started.await.map_err(|_| GitError::Aborted)??;

        let (events, receiver) = mpsc::channel(EVENT_QUEUE);
        let stop = CancellationToken::new();
        let index_lock = roots.git_dir.join("index.lock");
        drop(tokio::spawn(run(
            Arc::clone(&shared),
            options,
            events,
            stop.clone(),
            backend,
            index_lock,
        )));
        Ok((
            Self {
                shared,
                _stop: Arc::new(stop.drop_guard()),
            },
            receiver,
        ))
    }

    /// Marks the start of an engine write. Events are dropped while the
    /// returned guard, or any other, is alive; hold it through the post-write
    /// status snapshot (see the module docs).
    pub fn begin_write(&self) -> OwnWrite {
        self.shared.open_writes.fetch_add(1, Ordering::AcqRel);
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        tracing::trace!(generation, "engine write begins; watcher suppressed");
        OwnWrite {
            shared: Arc::clone(&self.shared),
            generation,
        }
    }

    /// How many engine writes have begun since the watcher started.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// How many reportable events were dropped as the app's own writes.
    pub fn suppressed_events(&self) -> u64 {
        self.shared.suppressed.load(Ordering::Relaxed)
    }

    /// Resolves once the watcher has stopped, for whatever reason: the last
    /// handle or the receiver was dropped, or the backend failed.
    pub fn stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        self.shared.stopped.clone().cancelled_owned()
    }
}

/// The bracket around one engine write; see [`Watcher::begin_write`].
#[derive(Debug)]
#[must_use = "events are suppressed only while the guard is alive"]
pub struct OwnWrite {
    shared: Arc<Shared>,
    generation: u64,
}

impl OwnWrite {
    /// The generation this write was given: the number of writes begun so
    /// far, including this one.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Ends the suppression. The same as dropping the guard, for callers who
    /// want to say so.
    pub fn finish(self) {
        drop(self);
    }
}

impl Drop for OwnWrite {
    fn drop(&mut self) {
        self.shared.open_writes.fetch_sub(1, Ordering::AcqRel);
        tracing::trace!(generation = self.generation, "engine write ends");
    }
}

// ---- shared state ----------------------------------------------------------------

/// What the backend's callback and the coalescer task share.
#[derive(Debug, Default)]
struct Shared {
    pending: Mutex<Pending>,
    /// Signalled by the callback whenever `pending` gains something.
    wake: Notify,
    generation: AtomicU64,
    /// Live [`OwnWrite`] guards; events are suppressed while it is non-zero.
    open_writes: AtomicUsize,
    suppressed: AtomicU64,
    /// Cancelled by the coalescer task when it ends.
    stopped: CancellationToken,
}

/// What has arrived since the last batch was taken: the union of kinds, and
/// the backend failure if there was one.
#[derive(Debug, Default)]
struct Pending {
    kinds: ChangeKinds,
    failure: Option<notify::Error>,
}

impl Shared {
    fn is_idle(&self) -> bool {
        let pending = lock(&self.pending);
        pending.kinds.is_empty() && pending.failure.is_none()
    }

    fn take(&self) -> Pending {
        std::mem::take(&mut *lock(&self.pending))
    }
}

/// Locks even if a callback panicked while holding the lock: the state is a
/// byte and an option, never half-written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the backend side -------------------------------------------------------------

/// The watched roots, canonical (symlinks resolved, no `\\?\` prefix), so
/// they match the paths the OS reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Roots {
    workdir: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl Roots {
    fn canonical(repo: &Repo) -> Result<Self, GitError> {
        let canonical = |path: &Path| {
            dunce::canonicalize(path).map_err(|source| GitError::Io {
                path: path.to_path_buf(),
                source,
            })
        };
        Ok(Self {
            workdir: canonical(repo.workdir())?,
            git_dir: canonical(repo.git_dir())?,
            common_dir: canonical(repo.common_dir())?,
        })
    }

    /// The paths to watch recursively: the working tree, then the common and
    /// git directories unless a path already listed contains them. In the
    /// usual layout `.git` is inside the working tree and this is one path.
    fn watch_paths(&self) -> Vec<&Path> {
        let mut paths: Vec<&Path> = vec![&self.workdir];
        for candidate in [&self.common_dir, &self.git_dir] {
            if !paths.iter().any(|root| candidate.starts_with(root)) {
                paths.push(candidate);
            }
        }
        paths
    }
}

/// Canonicalises the roots, builds the backend and registers the watches.
/// Blocking; runs on the blocking pool.
fn start(repo: &Repo, shared: Arc<Shared>) -> Result<(RecommendedWatcher, Roots), GitError> {
    let roots = Roots::canonical(repo)?;
    let mut handler = Handler {
        classifier: Classifier::new(roots.clone()),
        shared,
    };
    // Symlinked directories are not followed: git does not either, and a
    // link into a large tree would register it all (inotify).
    let config = notify::Config::default().with_follow_symlinks(false);
    let mut backend = RecommendedWatcher::new(move |result| handler.handle(result), config)
        .map_err(WatchError::Start)?;
    for path in roots.watch_paths() {
        backend
            .watch(path, RecursiveMode::Recursive)
            .map_err(|source| WatchError::Watch {
                path: path.to_path_buf(),
                source,
            })?;
    }
    tracing::debug!(paths = ?roots.watch_paths(), "watching repository");
    Ok((backend, roots))
}

/// Runs on the backend's thread: reduces each raw event to its kinds and
/// merges them into the shared byte.
struct Handler {
    classifier: Classifier,
    shared: Arc<Shared>,
}

impl Handler {
    fn handle(&mut self, result: notify::Result<Event>) {
        match result {
            Ok(event) => self.handle_event(&event),
            Err(error) => {
                tracing::warn!(%error, "file watcher backend failed");
                lock(&self.shared.pending).failure.get_or_insert(error);
                self.shared.wake.notify_one();
            }
        }
    }

    fn handle_event(&mut self, event: &Event) {
        let kinds = if event.need_rescan() {
            // The OS dropped events: anything may have changed, ignore rules
            // included.
            self.classifier.reload_ignore_rules();
            ChangeKinds::ALL
        } else {
            self.classifier.classify(event)
        };
        if kinds.is_empty() {
            // `index.lock` itself is noise, but its removal is what a held
            // batch waits for: wake the coalescer so it looks again.
            if event
                .paths
                .iter()
                .any(|path| self.classifier.is_index_lock(path))
            {
                self.shared.wake.notify_one();
            }
            return;
        }
        if self.shared.open_writes.load(Ordering::Acquire) > 0 {
            self.shared.suppressed.fetch_add(1, Ordering::Relaxed);
            tracing::trace!(?kinds, paths = ?event.paths, "own write, suppressed");
            return;
        }
        lock(&self.shared.pending).kinds |= kinds;
        self.shared.wake.notify_one();
    }
}

/// Maps an event's paths to [`ChangeKinds`] by where they lie and what they
/// are named. Pure apart from the ignore matcher, which reads ignore files
/// on first use and is rebuilt when they change.
struct Classifier {
    roots: Roots,
    /// `<git dir>/index.lock`, the one noise path whose events still matter.
    index_lock: PathBuf,
    /// `None` when no matcher could be built; nothing is ignored then, which
    /// errs towards reporting.
    ignore: Option<ignore::IncrementalIgnore>,
}

impl Classifier {
    fn new(roots: Roots) -> Self {
        let ignore = build_ignore(&roots.workdir);
        let index_lock = roots.git_dir.join("index.lock");
        Self {
            roots,
            index_lock,
            ignore,
        }
    }

    fn is_index_lock(&self, path: &Path) -> bool {
        path == self.index_lock
    }

    /// Drops the cached ignore rules; the next match re-reads the files.
    fn reload_ignore_rules(&mut self) {
        self.ignore = build_ignore(&self.roots.workdir);
    }

    fn classify(&mut self, event: &Event) -> ChangeKinds {
        if is_read_only_access(&event.kind) {
            return ChangeKinds::EMPTY;
        }
        let hint = dir_hint(&event.kind);
        let content_modify = is_content_modify(&event.kind);
        let mut kinds = ChangeKinds::EMPTY;
        for path in &event.paths {
            kinds |= self.classify_path(path, hint, content_modify);
        }
        kinds
    }

    /// `is_dir` says whether `path` is a directory when the event told us;
    /// otherwise the file system is asked, which a removed path answers with
    /// "no". `content_modify` says the event reports the path's contents or
    /// metadata changing (not a create, remove or rename).
    fn classify_path(
        &mut self,
        path: &Path,
        is_dir: Option<bool>,
        content_modify: bool,
    ) -> ChangeKinds {
        // A directory's own contents or metadata "changing" is its mtime
        // moving because an entry inside it was created, removed or renamed
        // (ReadDirectoryChangesW reports exactly that for the parent), and
        // the entry has its own event. The stat is paid for Modify events
        // only; a create, remove or rename of a directory is a real change.
        let is_dir = if content_modify {
            Some(is_existing_dir(path))
        } else {
            is_dir
        };
        if content_modify && is_dir == Some(true) {
            tracing::trace!(?path, "directory modified: an entry's own event carries it");
            return ChangeKinds::EMPTY;
        }
        // The git dir first: in the usual layout it is inside the working
        // tree, and in a linked worktree it is inside the common dir.
        if let Ok(relative) = path.strip_prefix(&self.roots.git_dir) {
            return self.classify_git_path(relative, false);
        }
        if let Ok(relative) = path.strip_prefix(&self.roots.common_dir) {
            return self.classify_git_path(relative, true);
        }
        if let Ok(relative) = path.strip_prefix(&self.roots.workdir) {
            return self.classify_worktree_path(path, relative, is_dir);
        }
        tracing::trace!(?path, "event outside the watched roots");
        ChangeKinds::EMPTY
    }

    fn classify_git_path(&mut self, relative: &Path, shared_only: bool) -> ChangeKinds {
        let (kinds, reloads_ignore_rules) = git_path_kinds(relative, shared_only);
        if reloads_ignore_rules {
            self.reload_ignore_rules();
        }
        kinds
    }

    fn classify_worktree_path(
        &mut self,
        path: &Path,
        relative: &Path,
        is_dir: Option<bool>,
    ) -> ChangeKinds {
        let Some(first) = relative.components().next() else {
            // The working tree itself (its mtime moved because an entry was
            // added or removed; the entry has its own event).
            return ChangeKinds::EMPTY;
        };
        if first.as_os_str() == ".git" {
            // The `.git` entry: a directory whose contents came in through
            // the git dir above, or a linked worktree's `.git` file.
            return ChangeKinds::EMPTY;
        }
        let is_dir =
            is_dir.unwrap_or_else(|| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()));
        if let Some(ignore) = &mut self.ignore {
            if ignore.matched(relative, is_dir).is_ignore() {
                tracing::trace!(?relative, "ignored path");
                return ChangeKinds::EMPTY;
            }
        }
        let mut kinds = ChangeKinds::of(ChangeKind::Status);
        match relative.file_name() {
            Some(name) if name == ".gitignore" => self.reload_ignore_rules(),
            Some(name) if name == ".gitattributes" => kinds.insert(ChangeKind::Config),
            _ => {}
        }
        kinds
    }
}

/// The kinds for a path relative to a git directory, and whether it changes
/// the ignore rules (`info/exclude`).
///
/// `shared_only` is set for a linked worktree's common directory, where only
/// what all worktrees share counts: `HEAD`, `index`, the operation markers
/// and `logs/HEAD` there belong to the main worktree.
fn git_path_kinds(relative: &Path, shared_only: bool) -> (ChangeKinds, bool) {
    const NONE: (ChangeKinds, bool) = (ChangeKinds::EMPTY, false);
    let mut components = relative.components().map(|c| c.as_os_str());
    let Some(first) = components.next() else {
        return NONE;
    };
    // git writes every file through a `<name>.lock` sibling and renames it
    // into place; the lock's own events are noise (the rename target's are
    // the signal). A path ending in `.lock` cannot be a ref name.
    if relative.extension().is_some_and(|ext| ext == "lock") {
        return NONE;
    }
    // Every name below is ASCII, so a non-UTF-8 component is none of them.
    let Some(first) = first.to_str() else {
        return NONE;
    };
    let second = components.next().and_then(|c| c.to_str());
    let kind = match first {
        "refs" | "packed-refs" | "shallow" => ChangeKind::Refs,
        "logs" if shared_only && second != Some("refs") => return NONE,
        "logs" => ChangeKind::Reflog,
        "config" => ChangeKind::Config,
        "info" => match second {
            Some("exclude") => return (ChangeKinds::of(ChangeKind::Config), true),
            Some("attributes") => ChangeKind::Config,
            Some("sparse-checkout") => ChangeKind::Status,
            _ => return NONE,
        },
        _ if shared_only => return NONE,
        "HEAD" => ChangeKind::Head,
        "index" => ChangeKind::Index,
        _ if first.starts_with("sharedindex.") => ChangeKind::Index,
        "ORIG_HEAD" | "FETCH_HEAD" => ChangeKind::Refs,
        "rebase-merge" | "rebase-apply" | "sequencer" | "CHERRY_PICK_HEAD" | "REVERT_HEAD"
        | "SQUASH_MSG" | "AUTO_MERGE" => ChangeKind::State,
        _ if first.starts_with("MERGE_") || first.starts_with("BISECT_") => ChangeKind::State,
        "config.worktree" => ChangeKind::Config,
        // objects/, hooks/, info/refs, COMMIT_EDITMSG, description, modules/
        // (submodule git dirs), worktrees/ (other worktrees), lfs/, ...
        _ => return NONE,
    };
    (ChangeKinds::of(kind), false)
}

/// Whether the event only says a file was opened or read (inotify reports
/// `IN_OPEN`, and git itself opens files constantly). A close after writing
/// is a change.
fn is_read_only_access(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => false,
        EventKind::Access(_) => true,
        _ => false,
    }
}

/// Whether the event says its path is a directory, when the backend knows.
fn dir_hint(kind: &EventKind) -> Option<bool> {
    match kind {
        EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder) => Some(true),
        EventKind::Create(CreateKind::File) | EventKind::Remove(RemoveKind::File) => Some(false),
        _ => None,
    }
}

/// Whether the event reports the path's contents or metadata changing, as
/// opposed to the path being created, removed or renamed.
fn is_content_modify(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(
            ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Other
        )
    )
}

/// Whether `path` is a directory right now (a symlink to one is not).
fn is_existing_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// A matcher for `workdir` that applies `.gitignore` files at every level,
/// `.git/info/exclude` and the user's global excludes, and nothing else (no
/// hidden-file rule, no `.ignore` files, no ignore files above the root).
/// It reads nothing until the first match.
fn build_ignore(workdir: &Path) -> Option<ignore::IncrementalIgnore> {
    let mut builder = ignore::WalkBuilder::new(workdir);
    builder
        .standard_filters(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        // We know this is a repository; the check would look for a `.git`
        // directory and a linked worktree has a file.
        .require_git(false)
        .follow_links(false);
    let matcher = builder.build_matchers().pop();
    if matcher.is_none() {
        tracing::warn!(
            ?workdir,
            "no ignore matcher for the working tree; nothing is ignored"
        );
    }
    matcher
}

// ---- the coalescer task -----------------------------------------------------------

/// Why the coalescer stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The last [`Watcher`] handle was dropped.
    HandleDropped,
    /// The [`Events`] receiver was dropped.
    ReceiverDropped,
    /// The backend failed; the error was delivered.
    Failed,
}

/// The coalescer task. Owns the backend so that the OS watch ends with it.
async fn run(
    shared: Arc<Shared>,
    options: WatchOptions,
    events: mpsc::Sender<Result<RepoChanged, WatchError>>,
    stop: CancellationToken,
    backend: RecommendedWatcher,
    index_lock: PathBuf,
) {
    let outcome = coalesce(&shared, &options, &events, &stop, &index_lock).await;
    drop(backend);
    shared.stopped.cancel();
    tracing::debug!(?outcome, "repository watcher stopped");
}

/// Waits for events, opens a window per batch and delivers the union.
async fn coalesce(
    shared: &Shared,
    options: &WatchOptions,
    events: &mpsc::Sender<Result<RepoChanged, WatchError>>,
    stop: &CancellationToken,
    index_lock: &Path,
) -> Outcome {
    loop {
        // Idle: nothing runs until the callback has something.
        while shared.is_idle() {
            tokio::select! {
                _ = shared.wake.notified() => {}
                _ = stop.cancelled() => return Outcome::HandleDropped,
                _ = events.closed() => return Outcome::ReceiverDropped,
            }
        }

        // The window: whatever arrives meanwhile joins the batch.
        let window_end = Instant::now() + options.window;
        tokio::select! {
            _ = tokio::time::sleep_until(window_end) => {}
            _ = stop.cancelled() => return Outcome::HandleDropped,
            _ = events.closed() => return Outcome::ReceiverDropped,
        }

        // Another process is mid-write while `index.lock` exists: hold the
        // batch until the lock goes (its removal is itself an event) or the
        // hold runs out.
        let hold_end = window_end + options.lock_hold;
        while index_lock.exists() {
            if Instant::now() >= hold_end {
                tracing::warn!(
                    path = ?index_lock,
                    held_for = ?options.lock_hold,
                    "index.lock still present; delivering anyway (stale lock?)"
                );
                break;
            }
            tokio::select! {
                _ = shared.wake.notified() => {}
                _ = tokio::time::sleep_until(hold_end) => {}
                _ = stop.cancelled() => return Outcome::HandleDropped,
                _ = events.closed() => return Outcome::ReceiverDropped,
            }
        }

        // Room in the channel first, the batch second: a slow consumer holds
        // the coalescer here while the callback keeps merging into
        // `pending`, so no backlog grows and what arrives meanwhile joins
        // this batch rather than forming the next one.
        let permit = tokio::select! {
            permit = events.reserve() => match permit {
                Ok(permit) => permit,
                Err(_) => return Outcome::ReceiverDropped,
            },
            _ = stop.cancelled() => return Outcome::HandleDropped,
        };
        let batch = shared.take();
        if let Some(error) = batch.failure {
            permit.send(Err(WatchError::Stopped(error)));
            return Outcome::Failed;
        }
        if batch.kinds.is_empty() {
            // Cannot happen (only `take` empties `pending`), but never
            // deliver an empty change.
            drop(permit);
            continue;
        }
        let changed = RepoChanged {
            kinds: batch.kinds,
            generation: shared.generation.load(Ordering::Acquire),
        };
        tracing::trace!(?changed, "repository changed");
        permit.send(Ok(changed));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use notify::event::{DataChange, Flag, MetadataKind, ModifyKind};

    use super::*;

    fn kinds(list: impl IntoIterator<Item = ChangeKind>) -> ChangeKinds {
        list.into_iter().collect()
    }

    /// Polls `future` once, outside any runtime.
    fn now_or_never<F: Future>(future: F) -> Option<F::Output> {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(value) => Some(value),
            std::task::Poll::Pending => None,
        }
    }

    fn git_kinds(path: &str, shared_only: bool) -> ChangeKinds {
        git_path_kinds(Path::new(path), shared_only).0
    }

    // ---- constants and the set type -----------------------------------------

    #[test]
    fn the_window_is_150ms_and_250ms_on_windows() {
        let expected = if cfg!(windows) { 250 } else { 150 };
        assert_eq!(DEFAULT_WINDOW, Duration::from_millis(expected));
        assert_eq!(WatchOptions::default().window, DEFAULT_WINDOW);
        assert_eq!(WatchOptions::default().lock_hold, DEFAULT_LOCK_HOLD);
    }

    #[test]
    fn change_kinds_is_a_one_byte_set() {
        assert_eq!(std::mem::size_of::<ChangeKinds>(), 1);
        let mut set = ChangeKinds::EMPTY;
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);

        set.insert(ChangeKind::Head);
        set |= ChangeKinds::of(ChangeKind::Status);
        assert!(set.contains(ChangeKind::Head));
        assert!(set.contains(ChangeKind::Status));
        assert!(!set.contains(ChangeKind::Refs));
        assert_eq!(set.len(), 2);
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [ChangeKind::Status, ChangeKind::Head]
        );
        assert_eq!(format!("{set:?}"), "{Status, Head}");
        assert_eq!(set, kinds([ChangeKind::Head, ChangeKind::Status]));
        assert_eq!(set | ChangeKinds::ALL, ChangeKinds::ALL);

        assert_eq!(ChangeKinds::ALL.len(), ChangeKind::ALL.len());
        assert!(ChangeKind::ALL
            .iter()
            .all(|kind| ChangeKinds::ALL.contains(*kind)));
        assert_eq!(
            ChangeKind::ALL.map(ChangeKind::name).join(","),
            "status,index,head,refs,state,reflog,config"
        );
    }

    // ---- git-dir classification ---------------------------------------------

    #[test]
    fn git_dir_paths_map_to_kinds() {
        let one = ChangeKinds::of;
        assert_eq!(git_kinds("HEAD", false), one(ChangeKind::Head));
        assert_eq!(git_kinds("index", false), one(ChangeKind::Index));
        assert_eq!(
            git_kinds("sharedindex.0123abcd", false),
            one(ChangeKind::Index)
        );
        assert_eq!(git_kinds("refs/heads/main", false), one(ChangeKind::Refs));
        assert_eq!(git_kinds("refs/stash", false), one(ChangeKind::Refs));
        assert_eq!(
            git_kinds("refs", false),
            one(ChangeKind::Refs),
            "the refs dir itself"
        );
        assert_eq!(git_kinds("packed-refs", false), one(ChangeKind::Refs));
        assert_eq!(git_kinds("shallow", false), one(ChangeKind::Refs));
        assert_eq!(git_kinds("ORIG_HEAD", false), one(ChangeKind::Refs));
        assert_eq!(git_kinds("FETCH_HEAD", false), one(ChangeKind::Refs));
        assert_eq!(git_kinds("MERGE_HEAD", false), one(ChangeKind::State));
        assert_eq!(git_kinds("MERGE_MSG", false), one(ChangeKind::State));
        assert_eq!(git_kinds("CHERRY_PICK_HEAD", false), one(ChangeKind::State));
        assert_eq!(git_kinds("REVERT_HEAD", false), one(ChangeKind::State));
        assert_eq!(git_kinds("BISECT_LOG", false), one(ChangeKind::State));
        assert_eq!(git_kinds("BISECT_START", false), one(ChangeKind::State));
        assert_eq!(git_kinds("rebase-merge", false), one(ChangeKind::State));
        assert_eq!(
            git_kinds("rebase-merge/done", false),
            one(ChangeKind::State)
        );
        assert_eq!(
            git_kinds("rebase-apply/0001", false),
            one(ChangeKind::State)
        );
        assert_eq!(git_kinds("sequencer/todo", false), one(ChangeKind::State));
        assert_eq!(git_kinds("logs/HEAD", false), one(ChangeKind::Reflog));
        assert_eq!(
            git_kinds("logs/refs/heads/main", false),
            one(ChangeKind::Reflog)
        );
        assert_eq!(git_kinds("config", false), one(ChangeKind::Config));
        assert_eq!(git_kinds("config.worktree", false), one(ChangeKind::Config));
        assert_eq!(git_kinds("info/attributes", false), one(ChangeKind::Config));
        assert_eq!(
            git_kinds("info/sparse-checkout", false),
            one(ChangeKind::Status)
        );
        assert_eq!(
            git_path_kinds(Path::new("info/exclude"), false),
            (one(ChangeKind::Config), true),
            "info/exclude reloads the ignore rules"
        );
    }

    #[test]
    fn git_dir_noise_maps_to_nothing() {
        for path in [
            "",
            "objects/ab/cdef0123456789abcdef0123456789abcdef01",
            "objects/pack/pack-0123.idx",
            "hooks/pre-commit",
            "COMMIT_EDITMSG",
            "description",
            "info",
            "info/refs",
            "modules/sub/HEAD",
            "worktrees/other/HEAD",
            "worktrees/other/index",
            "lfs/objects/aa/bb",
            "gitk.cache",
            "fsmonitor--daemon.ipc",
            // git's lock files, at every level
            "index.lock",
            "HEAD.lock",
            "packed-refs.lock",
            "config.lock",
            "refs/heads/main.lock",
            "logs/refs/heads/main.lock",
        ] {
            assert_eq!(git_kinds(path, false), ChangeKinds::EMPTY, "{path:?}");
            assert_eq!(
                git_kinds(path, true),
                ChangeKinds::EMPTY,
                "{path:?} (shared)"
            );
        }
    }

    #[test]
    fn a_shared_common_dir_reports_only_what_worktrees_share() {
        let one = ChangeKinds::of;
        assert_eq!(git_kinds("refs/heads/main", true), one(ChangeKind::Refs));
        assert_eq!(git_kinds("packed-refs", true), one(ChangeKind::Refs));
        assert_eq!(
            git_kinds("logs/refs/heads/main", true),
            one(ChangeKind::Reflog)
        );
        assert_eq!(git_kinds("config", true), one(ChangeKind::Config));
        assert_eq!(git_kinds("info/exclude", true), one(ChangeKind::Config));
        // The main worktree's own state.
        for path in [
            "HEAD",
            "index",
            "ORIG_HEAD",
            "MERGE_HEAD",
            "rebase-merge/done",
            "logs/HEAD",
        ] {
            assert_eq!(git_kinds(path, true), ChangeKinds::EMPTY, "{path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_git_dir_name_is_nothing() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"HEA\xffD"));
        assert_eq!(git_path_kinds(path, false), (ChangeKinds::EMPTY, false));
    }

    // ---- event-kind filters ---------------------------------------------------

    #[test]
    fn opens_and_reads_are_dropped_but_a_close_after_writing_counts() {
        assert!(is_read_only_access(&EventKind::Access(AccessKind::Open(
            AccessMode::Read
        ))));
        assert!(is_read_only_access(&EventKind::Access(AccessKind::Read)));
        assert!(is_read_only_access(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(is_read_only_access(&EventKind::Access(AccessKind::Any)));
        assert!(!is_read_only_access(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(!is_read_only_access(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        )));
        assert!(!is_read_only_access(&EventKind::Any));
        assert!(!is_read_only_access(&EventKind::Other));
    }

    #[test]
    fn directory_hints_come_from_create_and_remove_events() {
        assert_eq!(dir_hint(&EventKind::Create(CreateKind::Folder)), Some(true));
        assert_eq!(dir_hint(&EventKind::Remove(RemoveKind::Folder)), Some(true));
        assert_eq!(dir_hint(&EventKind::Create(CreateKind::File)), Some(false));
        assert_eq!(dir_hint(&EventKind::Remove(RemoveKind::File)), Some(false));
        assert_eq!(dir_hint(&EventKind::Create(CreateKind::Any)), None);
        assert_eq!(
            dir_hint(&EventKind::Modify(ModifyKind::Data(DataChange::Any))),
            None
        );
    }

    // ---- worktree classification on a scratch tree ----------------------------

    /// A fake repository layout: a working tree with ignore files and an
    /// empty `.git` directory. Nothing here runs git.
    fn scratch_roots() -> (tempfile::TempDir, Roots) {
        let dir = tempfile::tempdir().unwrap();
        let workdir = dunce::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(workdir.join(".git/info")).unwrap();
        fs::write(
            workdir.join(".gitignore"),
            "*.log\r\nbuild/\r\n!keep.log\r\n",
        )
        .unwrap();
        fs::create_dir_all(workdir.join("sub")).unwrap();
        fs::write(workdir.join("sub/.gitignore"), "secret.txt\n").unwrap();
        fs::write(workdir.join(".git/info/exclude"), "excluded.txt\n").unwrap();
        fs::create_dir_all(workdir.join("build/nested")).unwrap();
        let roots = Roots {
            git_dir: workdir.join(".git"),
            common_dir: workdir.join(".git"),
            workdir,
        };
        (dir, roots)
    }

    fn modify(path: PathBuf) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any))).add_path(path)
    }

    #[test]
    fn worktree_paths_honour_every_ignore_source_including_crlf_rules() {
        let (_dir, roots) = scratch_roots();
        let workdir = roots.workdir.clone();
        let mut classifier = Classifier::new(roots);
        let classify = |classifier: &mut Classifier, relative: &str| {
            classifier.classify(&modify(workdir.join(relative)))
        };
        let status = ChangeKinds::of(ChangeKind::Status);

        assert_eq!(classify(&mut classifier, "src/main.rs"), status);
        assert_eq!(
            classify(&mut classifier, "dir with spaces/ünïcødé.txt"),
            status
        );
        assert_eq!(classify(&mut classifier, "keep.log"), status, "whitelisted");
        assert_eq!(classify(&mut classifier, "sub/visible.txt"), status);
        for ignored in [
            "debug.log",
            "build",
            "build/out.bin",
            "build/nested/deep.txt",
            "sub/secret.txt",
            "excluded.txt",
        ] {
            assert_eq!(
                classify(&mut classifier, ignored),
                ChangeKinds::EMPTY,
                "{ignored:?}"
            );
        }
        // The roots themselves and the `.git` entry.
        assert_eq!(classify(&mut classifier, ""), ChangeKinds::EMPTY);
        assert_eq!(classify(&mut classifier, ".git"), ChangeKinds::EMPTY);
        // Through the git dir.
        assert_eq!(
            classify(&mut classifier, ".git/HEAD"),
            ChangeKinds::of(ChangeKind::Head)
        );
        assert_eq!(
            classify(&mut classifier, ".git/objects/aa/bb"),
            ChangeKinds::EMPTY
        );
        // Ignore-rule files are changes themselves.
        assert_eq!(classify(&mut classifier, ".gitignore"), status);
        assert_eq!(
            classify(&mut classifier, ".gitattributes"),
            kinds([ChangeKind::Status, ChangeKind::Config])
        );
        // A path outside every root.
        assert_eq!(
            classifier.classify(&modify(workdir.parent().unwrap().join("elsewhere"))),
            ChangeKinds::EMPTY
        );
    }

    #[test]
    fn a_removed_ignored_directory_uses_the_event_hint() {
        let (_dir, roots) = scratch_roots();
        let workdir = roots.workdir.clone();
        let mut classifier = Classifier::new(roots);
        fs::remove_dir_all(workdir.join("build")).unwrap();
        // `build/` is a directory-only rule; the hint says it was one.
        let removed =
            Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(workdir.join("build"));
        assert_eq!(classifier.classify(&removed), ChangeKinds::EMPTY);
        // Without a hint a gone path counts as a file, and `build/` does not
        // match a file: reported, which errs towards refreshing.
        assert_eq!(
            classifier.classify(&modify(workdir.join("build"))),
            ChangeKinds::of(ChangeKind::Status)
        );
    }

    #[test]
    fn editing_an_ignore_file_reloads_the_rules() {
        let (_dir, roots) = scratch_roots();
        let workdir = roots.workdir.clone();
        let mut classifier = Classifier::new(roots);
        assert_eq!(
            classifier.classify(&modify(workdir.join("x.log"))),
            ChangeKinds::EMPTY
        );

        fs::write(workdir.join(".gitignore"), "").unwrap();
        // Until the `.gitignore` event arrives the old rules stand.
        assert_eq!(
            classifier.classify(&modify(workdir.join("x.log"))),
            ChangeKinds::EMPTY
        );
        classifier.classify(&modify(workdir.join(".gitignore")));
        assert_eq!(
            classifier.classify(&modify(workdir.join("x.log"))),
            ChangeKinds::of(ChangeKind::Status)
        );

        fs::write(workdir.join(".git/info/exclude"), "").unwrap();
        assert_eq!(
            classifier.classify(&modify(workdir.join("excluded.txt"))),
            ChangeKinds::EMPTY
        );
        classifier.classify(&modify(workdir.join(".git/info/exclude")));
        assert_eq!(
            classifier.classify(&modify(workdir.join("excluded.txt"))),
            ChangeKinds::of(ChangeKind::Status)
        );
    }

    /// ReadDirectoryChangesW reports `Modify(Any)` for a directory whenever
    /// an entry inside it changes; the other backends report similar
    /// metadata events. Only the entry's own event counts.
    #[test]
    fn a_directorys_own_modify_is_noise_but_create_remove_and_rename_count() {
        use notify::event::RenameMode;

        let (_dir, roots) = scratch_roots();
        let workdir = roots.workdir.clone();
        fs::create_dir_all(workdir.join(".git/refs/heads")).unwrap();
        fs::write(workdir.join(".git/refs/heads/main"), "0123\n").unwrap();
        fs::write(workdir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(workdir.join("src")).unwrap();
        fs::write(workdir.join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut classifier = Classifier::new(roots);
        let event =
            |kind: EventKind, relative: &str| Event::new(kind).add_path(workdir.join(relative));
        let status = ChangeKinds::of(ChangeKind::Status);

        // Content or metadata "changes" of existing directories: nothing, in
        // the working tree and in the git dir alike.
        for kind in [
            EventKind::Modify(ModifyKind::Any),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            EventKind::Modify(ModifyKind::Other),
        ] {
            assert_eq!(
                classifier.classify(&event(kind, "sub")),
                ChangeKinds::EMPTY,
                "{kind:?}"
            );
            assert_eq!(
                classifier.classify(&event(kind, "src")),
                ChangeKinds::EMPTY,
                "{kind:?}"
            );
            assert_eq!(
                classifier.classify(&event(kind, ".git/refs/heads")),
                ChangeKinds::EMPTY,
                "{kind:?}"
            );
            assert_eq!(
                classifier.classify(&event(kind, ".git/refs")),
                ChangeKinds::EMPTY,
                "{kind:?}"
            );
        }
        // The same events on regular files are changes.
        assert_eq!(
            classifier.classify(&event(EventKind::Modify(ModifyKind::Any), "src/main.rs")),
            status
        );
        assert_eq!(
            classifier.classify(&event(
                EventKind::Modify(ModifyKind::Any),
                ".git/refs/heads/main"
            )),
            ChangeKinds::of(ChangeKind::Refs)
        );
        assert_eq!(
            classifier.classify(&event(
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
                ".git/HEAD"
            )),
            ChangeKinds::of(ChangeKind::Head)
        );
        // Creating, removing or renaming a directory is a change.
        for kind in [
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Create(CreateKind::Any),
            EventKind::Create(CreateKind::Folder),
        ] {
            assert_eq!(classifier.classify(&event(kind, "src")), status, "{kind:?}");
            assert_eq!(
                classifier.classify(&event(kind, ".git/refs/heads")),
                ChangeKinds::of(ChangeKind::Refs),
                "{kind:?}"
            );
        }
        assert_eq!(
            classifier.classify(&event(EventKind::Remove(RemoveKind::Any), "gone")),
            status
        );
        assert_eq!(
            classifier.classify(&event(EventKind::Remove(RemoveKind::Folder), "gone")),
            status
        );

        // The Windows shape of "create an ignored file": the file's own
        // event and its parent's Modify, both nothing.
        assert_eq!(
            classifier.classify(&event(EventKind::Create(CreateKind::Any), "sub/secret.txt")),
            ChangeKinds::EMPTY
        );
        assert_eq!(
            classifier.classify(&event(EventKind::Modify(ModifyKind::Any), "sub")),
            ChangeKinds::EMPTY
        );
        // And of "create then remove a ref lock": only the parent's Modify
        // is left, which is nothing.
        assert_eq!(
            classifier.classify(&event(
                EventKind::Create(CreateKind::Any),
                ".git/refs/heads/x.lock"
            )),
            ChangeKinds::EMPTY
        );
        assert_eq!(
            classifier.classify(&event(
                EventKind::Modify(ModifyKind::Any),
                ".git/refs/heads"
            )),
            ChangeKinds::EMPTY
        );
    }

    #[test]
    fn an_event_with_no_paths_is_nothing() {
        let (_dir, roots) = scratch_roots();
        let mut classifier = Classifier::new(roots);
        assert_eq!(
            classifier.classify(&Event::new(EventKind::Modify(ModifyKind::Any))),
            ChangeKinds::EMPTY
        );
    }

    // ---- watch roots -----------------------------------------------------------

    #[test]
    fn watch_paths_cover_every_root_once() {
        let usual = Roots {
            workdir: PathBuf::from("/r"),
            git_dir: PathBuf::from("/r/.git"),
            common_dir: PathBuf::from("/r/.git"),
        };
        assert_eq!(usual.watch_paths(), [Path::new("/r")]);

        let linked = Roots {
            workdir: PathBuf::from("/wt"),
            git_dir: PathBuf::from("/main/.git/worktrees/wt"),
            common_dir: PathBuf::from("/main/.git"),
        };
        assert_eq!(
            linked.watch_paths(),
            [Path::new("/wt"), Path::new("/main/.git")],
            "the git dir is inside the common dir"
        );

        let submodule = Roots {
            workdir: PathBuf::from("/parent/sub"),
            git_dir: PathBuf::from("/parent/.git/modules/sub"),
            common_dir: PathBuf::from("/parent/.git/modules/sub"),
        };
        assert_eq!(
            submodule.watch_paths(),
            [
                Path::new("/parent/sub"),
                Path::new("/parent/.git/modules/sub")
            ]
        );
    }

    // ---- the handler: reduction, suppression, failure ---------------------------

    fn handler() -> (tempfile::TempDir, Handler, Arc<Shared>) {
        let (dir, roots) = scratch_roots();
        let shared = Arc::new(Shared::default());
        let handler = Handler {
            classifier: Classifier::new(roots),
            shared: Arc::clone(&shared),
        };
        (dir, handler, shared)
    }

    #[test]
    fn a_huge_burst_reduces_to_one_byte_and_one_wakeup() {
        let (_dir, mut handler, shared) = handler();
        let workdir = handler.classifier.roots.workdir.clone();
        for i in 0..100_000u32 {
            handler.handle(Ok(modify(workdir.join(format!("src/file{}.rs", i % 500)))));
            handler.handle(Ok(modify(workdir.join(format!("build/out{i}.o")))));
        }
        let pending = shared.take();
        assert_eq!(pending.kinds, ChangeKinds::of(ChangeKind::Status));
        assert!(pending.failure.is_none());
        assert!(shared.is_idle(), "take() empties the state");
        // A stored wakeup is at most one permit, however many events came.
        assert!(now_or_never(shared.wake.notified()).is_some());
        assert!(now_or_never(shared.wake.notified()).is_none());
    }

    #[test]
    fn events_during_an_own_write_are_counted_not_queued() {
        let (_dir, mut handler, shared) = handler();
        let workdir = handler.classifier.roots.workdir.clone();
        shared.open_writes.fetch_add(1, Ordering::AcqRel);
        handler.handle(Ok(modify(workdir.join("ours.txt"))));
        handler.handle(Ok(modify(workdir.join(".git/index"))));
        handler.handle(Ok(modify(workdir.join("debug.log")))); // ignored: not counted
        assert!(shared.is_idle());
        assert_eq!(shared.suppressed.load(Ordering::Relaxed), 2);
        assert!(now_or_never(shared.wake.notified()).is_none());

        shared.open_writes.fetch_sub(1, Ordering::AcqRel);
        handler.handle(Ok(modify(workdir.join("theirs.txt"))));
        assert_eq!(shared.take().kinds, ChangeKinds::of(ChangeKind::Status));
    }

    #[test]
    fn index_lock_activity_wakes_the_coalescer_without_a_change() {
        let (_dir, mut handler, shared) = handler();
        let git_dir = handler.classifier.roots.git_dir.clone();
        let removed =
            Event::new(EventKind::Remove(RemoveKind::File)).add_path(git_dir.join("index.lock"));
        handler.handle(Ok(removed));
        assert!(shared.is_idle(), "the lock is not a change");
        assert!(
            now_or_never(shared.wake.notified()).is_some(),
            "but a held batch must look again"
        );
        // Other lock files do not.
        handler.handle(Ok(modify(git_dir.join("refs/heads/main.lock"))));
        assert!(now_or_never(shared.wake.notified()).is_none());
    }

    #[test]
    fn a_rescan_notice_means_everything_changed() {
        let (_dir, mut handler, shared) = handler();
        handler.handle(Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
        assert_eq!(shared.take().kinds, ChangeKinds::ALL);
    }

    #[test]
    fn a_backend_error_is_kept_for_the_coalescer() {
        let (_dir, mut handler, shared) = handler();
        handler.handle(Err(notify::Error::generic("boom")));
        handler.handle(Err(notify::Error::generic("second")));
        assert!(!shared.is_idle());
        let pending = shared.take();
        assert_eq!(
            pending.failure.map(|e| e.to_string()).as_deref(),
            Some("boom")
        );
    }

    // ---- the coalescer, on a paused clock ----------------------------------------

    struct Harness {
        shared: Arc<Shared>,
        stop: CancellationToken,
        receiver: Events,
        task: tokio::task::JoinHandle<Outcome>,
        _dir: tempfile::TempDir,
        index_lock: PathBuf,
    }

    fn harness(capacity: usize, options: WatchOptions) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let index_lock = dir.path().join("index.lock");
        let shared = Arc::new(Shared::default());
        let stop = CancellationToken::new();
        let (sender, receiver) = mpsc::channel(capacity);
        let task = tokio::spawn({
            let shared = Arc::clone(&shared);
            let stop = stop.clone();
            let index_lock = index_lock.clone();
            async move { coalesce(&shared, &options, &sender, &stop, &index_lock).await }
        });
        Harness {
            shared,
            stop,
            receiver,
            task,
            _dir: dir,
            index_lock,
        }
    }

    impl Harness {
        fn arrive(&self, kinds: ChangeKinds) {
            lock(&self.shared.pending).kinds |= kinds;
            self.shared.wake.notify_one();
        }
    }

    /// With the clock paused, returns once every task is idle.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_batch_is_delivered_when_its_window_closes() {
        let options = WatchOptions::default();
        let mut h = harness(4, options);
        settle().await;
        assert!(h.receiver.try_recv().is_err(), "idle: nothing delivered");

        h.arrive(ChangeKinds::of(ChangeKind::Status));
        tokio::time::sleep(options.window / 2).await;
        h.arrive(ChangeKinds::of(ChangeKind::Head));
        assert!(h.receiver.try_recv().is_err(), "the window is still open");
        tokio::time::sleep(options.window / 2).await;
        settle().await;

        let changed = h.receiver.try_recv().unwrap().unwrap();
        assert_eq!(changed.kinds, kinds([ChangeKind::Status, ChangeKind::Head]));
        assert_eq!(changed.generation, 0);
        assert!(h.receiver.try_recv().is_err(), "one batch per window");
        h.stop.cancel();
        assert_eq!(h.task.await.unwrap(), Outcome::HandleDropped);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_consumer_loses_nothing_and_grows_nothing() {
        let options = WatchOptions::default();
        let mut h = harness(1, options);
        for kind in [ChangeKind::Status, ChangeKind::Index, ChangeKind::Refs] {
            h.arrive(ChangeKinds::of(kind));
            tokio::time::sleep(options.window * 2).await;
        }
        settle().await;
        // The first batch filled the channel; the second waits for room and
        // the third merged into it meanwhile.
        assert_eq!(
            h.receiver.recv().await.unwrap().unwrap().kinds,
            ChangeKinds::of(ChangeKind::Status)
        );
        settle().await;
        assert_eq!(
            h.receiver.recv().await.unwrap().unwrap().kinds,
            kinds([ChangeKind::Index, ChangeKind::Refs])
        );
        assert!(h.shared.is_idle());
        drop(h.receiver);
        assert_eq!(h.task.await.unwrap(), Outcome::ReceiverDropped);
    }

    #[tokio::test(start_paused = true)]
    async fn an_index_lock_holds_the_batch_until_it_goes_or_the_hold_ends() {
        let options = WatchOptions {
            window: Duration::from_millis(150),
            lock_hold: Duration::from_secs(2),
        };
        let mut h = harness(4, options);
        fs::write(&h.index_lock, "").unwrap();

        h.arrive(ChangeKinds::of(ChangeKind::Status));
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert!(h.receiver.try_recv().is_err(), "held while the lock exists");
        fs::remove_file(&h.index_lock).unwrap();
        h.arrive(ChangeKinds::of(ChangeKind::Index)); // the lock's removal event
        settle().await;
        assert_eq!(
            h.receiver.try_recv().unwrap().unwrap().kinds,
            kinds([ChangeKind::Status, ChangeKind::Index])
        );

        // A stale lock: delivered when the hold runs out.
        fs::write(&h.index_lock, "").unwrap();
        h.arrive(ChangeKinds::of(ChangeKind::Head));
        tokio::time::sleep(Duration::from_millis(150 + 1999)).await;
        assert!(h.receiver.try_recv().is_err());
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert_eq!(
            h.receiver.try_recv().unwrap().unwrap().kinds,
            ChangeKinds::of(ChangeKind::Head)
        );
        h.stop.cancel();
        assert_eq!(h.task.await.unwrap(), Outcome::HandleDropped);
    }

    #[tokio::test(start_paused = true)]
    async fn a_backend_failure_ends_the_stream_with_the_error() {
        let mut h = harness(4, WatchOptions::default());
        lock(&h.shared.pending).failure =
            Some(notify::Error::new(notify::ErrorKind::MaxFilesWatch));
        h.shared.wake.notify_one();
        assert_eq!(h.task.await.unwrap(), Outcome::Failed);
        let error = h.receiver.recv().await.unwrap().unwrap_err();
        assert!(matches!(error, WatchError::Stopped(_)), "{error:?}");
        assert!(error.is_watch_limit());
        assert!(h.receiver.recv().await.is_none(), "the sender is gone");
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_mid_window_delivers_nothing() {
        let mut h = harness(4, WatchOptions::default());
        h.arrive(ChangeKinds::of(ChangeKind::Status));
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.stop.cancel();
        assert_eq!(h.task.await.unwrap(), Outcome::HandleDropped);
        assert!(h.receiver.try_recv().is_err());
    }
}
