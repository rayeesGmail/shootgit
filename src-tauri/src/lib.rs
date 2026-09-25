//! Tauri shell for the app (SPEC §4: commands go down, events come up).
//!
//! Phase 0 opens one window; the frontend lives in `packages/ui` and reaches
//! this crate only through the commands in [`commands`], whose TypeScript
//! signatures are generated from [`ipc::builder`].
//!
//! The product name is still an open question (SPEC §12), so
//! `tauri.conf.json` carries the placeholder `Shootgit` as `productName` and
//! window title, and `dev.placeholder.shootgit` as the bundle identifier.

pub mod commands;
pub mod ipc;

use tauri::async_runtime::TokioRuntime;

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
    let _runtime = install_runtime()?;

    // The invoke handler comes from the specta builder, never from a second
    // `tauri::generate_handler!`: a command reachable from the frontend but
    // absent from the builder would have no generated binding (ADR 0003).
    // Bindings are written by `pnpm gen:types`, not on startup, so a release
    // build never touches the source tree.
    let specta = ipc::builder();
    tauri::Builder::default()
        .invoke_handler(specta.invoke_handler())
        .run(tauri::generate_context!())
}
