//! `pnpm gen:types` — writes `packages/ipc-types/bindings.ts` from the Rust
//! command signatures (SPEC §4 IPC contract, ADR 0003).
//!
//! A binary of its own rather than a step in the app's startup, so generating
//! bindings never needs a window, a display or a frontend build, and a release
//! build of the app never writes into the source tree.
//!
//! Run it with `pnpm gen:types`, or directly:
//!
//! ```sh
//! cargo run --package app --bin gen-types            # the committed path
//! cargo run --package app --bin gen-types -- /tmp/b.ts
//! ```
//!
//! The committed file is never hand-edited; CI regenerates it and fails on a
//! diff.

// CLAUDE.md: `unwrap`/`expect` are allowed in binary entry points and tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    // Resolved against this crate's manifest directory, so the output lands in
    // the same place whatever the current directory is: pnpm runs the script
    // from the repo root, a developer may run cargo from `src-tauri/`.
    let default_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(app_lib::ipc::BINDINGS_PATH);

    let path = match std::env::args_os().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => default_path,
    };

    if let Err(error) = app_lib::ipc::export_bindings(&path) {
        eprintln!("gen-types: could not write {}: {error}", path.display());
        return ExitCode::FAILURE;
    }

    // Only for the log line: `src-tauri/../packages/...` is the same file but
    // reads as a mistake.
    let shown = path.canonicalize().unwrap_or(path);
    println!("gen-types: wrote {}", shown.display());
    ExitCode::SUCCESS
}
