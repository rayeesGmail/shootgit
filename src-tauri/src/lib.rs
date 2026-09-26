//! Tauri shell for the app (SPEC §4: commands go down, events come up).
//!
//! Phase 0 opens one window; the frontend lives in `packages/ui` and reaches
//! this crate only through the commands in [`commands`] and the events in
//! [`events`], whose TypeScript types are generated from [`ipc::builder`].
//! The open repositories live in [`repos`]; what the app remembers between
//! launches lives in [`settings`].
//!
//! The product name is still an open question (SPEC §12), so
//! `tauri.conf.json` carries the placeholder `Shootgit` as `productName` and
//! window title, and `dev.placeholder.shootgit` as the bundle identifier.

pub mod commands;
pub mod events;
pub mod git;
pub mod ipc;
pub mod repos;
pub mod settings;

use std::sync::Arc;

use tauri::async_runtime::TokioRuntime;
use tauri::{App, Manager, Runtime};
use tauri_specta::Event as _;

use crate::repos::{EventSink, RepoEvent, Repos};
use crate::settings::SettingsStore;

/// Builds the app's single tokio runtime and makes it Tauri's (SPEC §4
/// Low-resource operation, ADR 0004).
///
/// `git_engine::runtime::build` sizes it to `max(1, parallelism - 1)`
/// workers, where `parallelism` is `available_parallelism()`. Call this
/// before `tauri::Builder::default()` and at most once per process:
/// `tauri::async_runtime::set` panics on a second call, and Tauri would
/// otherwise lazily create a default runtime with its own full set of worker
/// threads. The returned runtime has to stay alive for as long as the app
/// runs; Tauri only holds a handle to it.
pub fn install_runtime() -> std::io::Result<TokioRuntime> {
    let runtime = git_engine::runtime::build()?;
    tauri::async_runtime::set(runtime.handle().clone());
    Ok(runtime)
}

/// Builds the Tauri application and runs it until the last window closes.
///
/// Errors are returned rather than unwrapped so `main.rs` owns the only exit
/// path (CLAUDE.md: no `unwrap`/`expect` outside `main.rs` and tests).
pub fn run() -> Result<(), tauri::Error> {
    // The single runtime, installed before the builder is constructed and
    // kept alive until the event loop returns. Nothing above this line may
    // start a runtime, a thread or a timer.
    let runtime = install_runtime()?;

    // macOS: ask the user's login shell for its PATH once (2 s timeout,
    // falls back to this process's PATH), so git and other tools are found
    // where a terminal finds them (SPEC §4 Process model, §5 step 2). It runs
    // while the window comes up; git resolution awaits the same result via
    // `ResolveOptions::from_login_shell_env`. A no-op on other OSes.
    drop(runtime.spawn(git_engine::login_shell::init()));

    // The invoke handler comes from the specta builder, never from a second
    // `tauri::generate_handler!`: a command reachable from the frontend but
    // absent from the builder would have no generated binding (ADR 0003).
    // Bindings are written by `pnpm gen:types`, not on startup, so a release
    // build never touches the source tree.
    let specta = ipc::builder();
    tauri::Builder::default()
        // The native folder picker behind "Open repository"; the frontend
        // may only call its `open` (capabilities/default.json).
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(specta.invoke_handler())
        .setup(move |app| {
            // Settings are part of the startup diet (SPEC §4 Low-resource
            // operation, rule 9): one small read. Loading never fails: a
            // missing or corrupt file gives the defaults, and a config
            // directory Tauri cannot locate keeps them in memory.
            let settings = settings::store_at(settings::settings_path(app.handle()));
            install(app, &specta, settings);
            Ok(())
        })
        .run(tauri::generate_context!())
}

/// Puts the app's state in place and mounts its typed events: what `run()`
/// does in `setup`, and what the tests do to get the same app.
///
/// Commands reach the settings through `tauri::State<'_, Arc<SettingsStore>>`
/// and the open repositories through `tauri::State<'_, Repos>`, whose events
/// are emitted as the typed events of [`events`]. Nothing here spawns or
/// reads anything; git is resolved when the first repository is opened.
pub fn install<R: Runtime>(
    app: &App<R>,
    specta: &tauri_specta::Builder<R>,
    settings: SettingsStore,
) {
    // Before anything can emit: tauri-specta panics on an event it has not
    // mounted.
    specta.mount_events(app);

    let handle = app.handle().clone();
    let sink: EventSink = Arc::new(move |event| {
        let emitted = match &event {
            RepoEvent::Changed(changed) => changed.emit(&handle),
            RepoEvent::WatchFailed(failed) => failed.emit(&handle),
        };
        if let Err(error) = emitted {
            tracing::warn!(%error, ?event, "could not emit a repository event");
        }
    });

    let settings = Arc::new(settings);
    app.manage(Arc::clone(&settings));
    app.manage(Repos::new(settings, sink));
}
