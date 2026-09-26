//! The single description of the IPC surface (SPEC §4 IPC contract).
//!
//! [`builder`] is the one place commands and events are registered.
//! Tauri reads it for dispatch, and `pnpm gen:types` reads the same value to
//! write `packages/ipc-types/bindings.ts`, so TypeScript cannot drift from
//! Rust: adding a command without regenerating fails CI (ADR 0003).

use std::path::Path;

use specta_typescript::Typescript;
use tauri::Runtime;
use tauri_specta::{collect_commands, collect_events};

use crate::{commands, events};

/// Where the generated bindings belong, relative to this crate's manifest
/// directory (`src-tauri/`).
///
/// A relative path on purpose: `src/bin/gen-types.rs` and the tests each join
/// it with their own `CARGO_MANIFEST_DIR`, so neither depends on the current
/// directory and no absolute build-machine path ends up in the binary.
pub const BINDINGS_PATH: &str = "../packages/ipc-types/bindings.ts";

/// The IPC surface: every command the frontend may invoke and every event
/// it may listen to.
///
/// Generic over the runtime so `tauri::test`'s mock runtime can drive exactly
/// the handler the real app uses.
///
/// Events need one more step than commands: [`crate::install`] calls
/// `builder.mount_events(app)` in `setup`. Without it tauri-specta panics on
/// the first emit, and the frontend never receives an event.
pub fn builder<R: Runtime>() -> tauri_specta::Builder<R> {
    tauri_specta::Builder::<R>::new()
        .commands(collect_commands![
            commands::ping,
            commands::open_repo,
            commands::get_status,
            commands::list_recent_repos,
        ])
        .events(collect_events![
            events::RepoChanged,
            events::RepoWatchFailed
        ])
        // One TypeScript type per Rust type. In its default mode specta
        // splits every type that has a serde codec attribute (the lossy path
        // fields of the status models, ADR 0008) and everything containing
        // it into `X_Serialize | X_Deserialize` twins, though our IPC types
        // look the same in both directions. Unified mode still refuses what
        // it cannot represent (an `alias`, a `skip_serializing_if`), and a
        // codec field without an explicit `#[specta(type = ..)]`, so a type
        // that truly differs by direction fails the export loudly.
        .disable_serde_phases()
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
