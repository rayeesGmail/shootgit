//! Every Tauri command the frontend can call lives under this module.
//!
//! SPEC §4: the frontend never spawns git, touches the filesystem or reaches
//! the network — it only invokes commands from here. Each one carries both
//! `#[tauri::command]` (so Tauri can dispatch it) and `#[specta::specta]` (so
//! `pnpm gen:types` can give it a TypeScript signature); a command with only
//! the first would be invisible to `packages/ipc-types/bindings.ts` and could
//! drift from the frontend's idea of it.
//!
//! Register new commands in [`crate::ipc::builder`], never in a second
//! `tauri::generate_handler!`.
//!
//! Keep the doc comment on a command to a single paragraph: specta copies it
//! into `bindings.ts`, where a blank line becomes a ` * ` line with trailing
//! whitespace that editors and formatters like to strip — which would then
//! show up as a hand edit of a generated file.
//!
//! Commands that touch git, the file system or the settings file are
//! `async`: Tauri runs synchronous commands on the main thread, which must
//! never stall (SPEC §4 Low-resource operation). Blocking work inside them
//! goes through `spawn_blocking`. Every command that can fail returns
//! [`CommandError`].

pub mod error;

use std::path::PathBuf;
use std::sync::Arc;

use git_engine::repo::RepoId;
use git_engine::status::Status;
use tauri::State;

pub use self::error::{CommandError, ErrorKind};
use crate::repos::{OpenedRepo, Repos};
use crate::settings::SettingsStore;

/// Liveness probe for the IPC bridge: answers `"pong"`. It is the sample
/// command P0-04 uses to prove the round-trip Rust → generated bindings → UI,
/// and it stays afterwards as the cheapest check that the bridge is alive.
#[tauri::command]
#[specta::specta]
pub fn ping() -> String {
    "pong".to_owned()
}

/// Opens the repository that contains `path` (a directory in it, or its root) and returns its first status. One repository is open at a time: once this succeeds, the one open before is closed and its id stops working, while opening the repository that is already open keeps its id. Every repository opened is recorded as the most recent one. Changes to it on disk are reported with the `repo-changed` event from now on.
#[tauri::command]
#[specta::specta]
pub async fn open_repo(repos: State<'_, Repos>, path: PathBuf) -> Result<OpenedRepo, CommandError> {
    repos.open(path).await
}

/// The current status of the open repository `repo_id`. A newer call for the same repository cancels this one, which then fails with the `cancelled` kind.
#[tauri::command]
#[specta::specta]
pub async fn get_status(repos: State<'_, Repos>, repo_id: RepoId) -> Result<Status, CommandError> {
    repos.status(repo_id).await
}

/// The repositories opened most recently, newest first, at most 20.
#[tauri::command]
#[specta::specta]
pub async fn list_recent_repos(
    settings: State<'_, Arc<SettingsStore>>,
) -> Result<Vec<String>, CommandError> {
    // Recorded paths are valid UTF-8 (ADR 0008), so nothing is lost here.
    Ok(settings
        .get()
        .recent_repos()
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}
