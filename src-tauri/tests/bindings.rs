#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `packages/ipc-types/bindings.ts` is generated from the Rust command
//! signatures and committed, so the frontend typechecks without a Rust
//! toolchain and IPC changes show up in review (ADR 0003).
//!
//! CI re-runs `pnpm gen:types` and diffs the result (P0-04). These tests are
//! the same guarantee one layer down, so `cargo test` catches a stale file
//! before a push does, and so the generator is checked to be deterministic —
//! a generator that reorders its output on every run would make the CI diff
//! fail at random.

use std::fs;
use std::path::{Path, PathBuf};

/// Where `pnpm gen:types` writes, resolved the way the generator resolves it:
/// relative to this crate's manifest, never to the current directory.
fn committed_bindings() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(app_lib::ipc::BINDINGS_PATH)
}

/// Exports to a scratch file and returns the bytes written.
fn fresh_export(dir: &Path, name: &str) -> Vec<u8> {
    let path = dir.join(name);
    app_lib::ipc::export_bindings(&path).expect("the bindings export succeeds");
    fs::read(&path).expect("the generator wrote the file it was given")
}

#[test]
fn the_committed_bindings_are_what_the_generator_writes_today() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = fresh_export(dir.path(), "bindings.ts");

    let committed = fs::read(committed_bindings()).expect(
        "packages/ipc-types/bindings.ts is committed; run `pnpm gen:types` if it is missing",
    );

    assert_eq!(
        String::from_utf8(committed).unwrap(),
        String::from_utf8(fresh).unwrap(),
        "packages/ipc-types/bindings.ts is stale or was hand-edited; \
         run `pnpm gen:types` and commit the result (ADR 0003)"
    );
}

#[test]
fn the_export_is_byte_for_byte_reproducible() {
    let dir = tempfile::tempdir().unwrap();

    let first = fresh_export(dir.path(), "first.ts");
    let second = fresh_export(dir.path(), "second.ts");

    assert_eq!(first, second, "`pnpm gen:types` must be idempotent");
}

#[test]
fn the_export_uses_lf_line_endings_on_every_os() {
    // A CRLF export on Windows would make the committed file differ by OS, and
    // `git diff --exit-code` in CI would fail on a file nobody touched.
    let dir = tempfile::tempdir().unwrap();
    let bytes = fresh_export(dir.path(), "bindings.ts");

    assert!(
        !bytes.contains(&b'\r'),
        "generated bindings must be LF-only"
    );
}

#[test]
fn the_generated_bindings_expose_the_ping_command() {
    let committed = fs::read_to_string(committed_bindings()).unwrap();

    assert!(
        committed.contains("export const commands"),
        "the bindings must expose the `commands` object the UI imports"
    );
    assert!(
        committed.contains("ping: () =>"),
        "the sample command must reach TypeScript as `commands.ping()`"
    );
    assert!(
        committed.contains("(\"ping\")"),
        "the binding must invoke the command under its Tauri name"
    );
}

#[test]
fn the_generated_bindings_say_they_are_generated() {
    let committed = fs::read_to_string(committed_bindings()).unwrap();
    let first_line = committed.lines().next().unwrap_or_default();

    assert!(
        first_line.contains("Do not edit"),
        "the committed file must carry the generator's do-not-edit header, \
         so a hand edit is obvious in review; found: {first_line:?}"
    );
}
