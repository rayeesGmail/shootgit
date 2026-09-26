//! Every typed event the backend emits (SPEC §4: commands go down, events
//! come up).
//!
//! Each event is registered in [`crate::ipc::builder`] with
//! `collect_events!`, which gives it a TypeScript binding, and
//! [`crate::install`] mounts them on the app; an event that is emitted
//! without being mounted panics in `tauri-specta`, and one that is not
//! collected has no binding. The event names are pinned with
//! `event_name` rather than derived from the struct names, because the
//! frontend and the spec know them by name.
//!
//! Keep doc comments on the payload types to a single paragraph, as for
//! commands (see [`crate::commands`]).

use git_engine::repo::RepoId;
use git_engine::watcher::ChangeKinds;
use serde::Serialize;

/// Something changed on disk in an open repository: one coalesced event per
/// 150 ms window (250 ms on Windows). `kinds` says what to refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "repo-changed")]
pub struct RepoChanged {
    pub repo_id: RepoId,
    pub kinds: ChangeKinds,
}

/// The file watcher of an open repository stopped, so changes made outside the app are no longer reported. The app shows the warning; the polling fallback of SPEC §5 rule 8 is not there yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "repo-watch-failed")]
pub struct RepoWatchFailed {
    pub repo_id: RepoId,
    pub error: WatchFailure,
}

/// Why a repository is not being watched: its watcher could not start or stopped. `fix` is a command for the user to run when there is one (raising Linux's inotify watch limit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct WatchFailure {
    pub message: String,
    pub is_watch_limit: bool,
    pub fix: Option<String>,
}
