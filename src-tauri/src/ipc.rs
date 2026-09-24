//! The single description of the IPC surface (SPEC §4 IPC contract).
//!
//! [`builder`] is the one place commands — and later events — are registered.
//! Tauri reads it for dispatch, and `pnpm gen:types` reads the same value to
//! write `packages/ipc-types/bindings.ts`, so TypeScript cannot drift from
//! Rust: adding a command without regenerating fails CI (ADR 0003).

use std::path::Path;

use specta_typescript::Typescript;
use tauri::Runtime;
use tauri_specta::collect_commands;

use crate::commands;

/// Where the generated bindings belong, relative to this crate's manifest
/// directory (`src-tauri/`).
///
/// A relative path on purpose: `src/bin/gen-types.rs` and the tests each join
/// it with their own `CARGO_MANIFEST_DIR`, so neither depends on the current
/// directory and no absolute build-machine path ends up in the binary.
pub const BINDINGS_PATH: &str = "../packages/ipc-types/bindings.ts";

/// The IPC surface: every command the frontend may invoke.
///
/// Generic over the runtime so `tauri::test`'s mock runtime can drive exactly
/// the handler the real app uses.
///
/// Events are not here yet. The first typed event (`repo-changed`, P0-12) adds
/// `.events(collect_events![..])`, tauri-specta's `derive` feature for the
/// `Event` macro, and a `builder.mount_events(app)` call in
/// [`crate::run`]'s `setup` — without that call the frontend never receives
/// them.
pub fn builder<R: Runtime>() -> tauri_specta::Builder<R> {
    tauri_specta::Builder::<R>::new().commands(collect_commands![commands::ping])
}

/// Writes the TypeScript bindings for [`builder`] to `path`, creating parent
/// directories as needed.
///
/// Called by `pnpm gen:types` (see `src/bin/gen-types.rs`). The output is
/// deterministic and LF-only, which is what lets CI diff it against the
/// committed file.
pub fn export_bindings(path: &Path) -> Result<(), specta_typescript::Error> {
    // `tauri::Wry` is only the type the generator is instantiated with; the
    // emitted TypeScript does not depend on the runtime.
    builder::<tauri::Wry>().export(Typescript::default(), path)
}
