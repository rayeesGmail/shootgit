#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-17: the app runs every async task on one tokio runtime sized for the
//! machine (`max(1, available_parallelism() - 1)` workers) and hands it to
//! Tauri before the builder is constructed (SPEC §4 Low-resource operation,
//! ADR 0004). A second runtime — Tauri's lazily created default — would
//! double the worker threads on a 2-core laptop.
//!
//! `tauri::async_runtime::set` can be called once per process, so this file
//! holds exactly one test.

use tauri::async_runtime::TokioHandle;

#[test]
fn the_app_hands_tauri_one_runtime_sized_for_the_machine() {
    let runtime = app_lib::install_runtime().expect("the runtime builds");
    let expected =
        git_engine::runtime::worker_threads(git_engine::runtime::available_parallelism());

    assert_eq!(runtime.metrics().num_workers(), expected);

    // Whatever Tauri spawns lands on that same runtime, not on a default one.
    let (id_seen_by_tauri, workers_seen_by_tauri) = tauri::async_runtime::block_on(async {
        let handle = TokioHandle::current();
        (handle.id(), handle.metrics().num_workers())
    });
    assert_eq!(id_seen_by_tauri, runtime.handle().id());
    assert_eq!(workers_seen_by_tauri, expected);
}
