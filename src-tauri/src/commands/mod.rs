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

/// Liveness probe for the IPC bridge: answers `"pong"`. It is the sample
/// command P0-04 uses to prove the round-trip Rust → generated bindings → UI,
/// and it stays afterwards as the cheapest check that the bridge is alive.
#[tauri::command]
#[specta::specta]
pub fn ping() -> String {
    "pong".to_owned()
}
