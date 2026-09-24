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

/// Builds the Tauri application and runs it until the last window closes.
///
/// Errors are returned rather than unwrapped so `main.rs` owns the only exit
/// path (CLAUDE.md: no `unwrap`/`expect` outside `main.rs` and tests).
pub fn run() -> Result<(), tauri::Error> {
    // P0-17 installs the single tokio runtime here, via
    // `tauri::async_runtime::set(..)`, *before* the builder is constructed.
    // Nothing above this line may start a runtime, a thread or a timer.

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
