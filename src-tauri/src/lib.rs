//! Tauri shell for the app (SPEC §4: commands go down, events come up).
//!
//! Phase 0 opens one empty window; the frontend lives in `packages/ui`.
//!
//! The product name is still an open question (SPEC §12), so
//! `tauri.conf.json` carries the placeholder `Shootgit` as `productName` and
//! window title, and `dev.placeholder.shootgit` as the bundle identifier.

/// Builds the Tauri application and runs it until the last window closes.
///
/// Errors are returned rather than unwrapped so `main.rs` owns the only exit
/// path (CLAUDE.md: no `unwrap`/`expect` outside `main.rs` and tests).
pub fn run() -> Result<(), tauri::Error> {
    // P0-17 installs the single tokio runtime here, via
    // `tauri::async_runtime::set(..)`, *before* the builder is constructed.
    // Nothing above this line may start a runtime, a thread or a timer.
    tauri::Builder::default().run(tauri::generate_context!())
}
