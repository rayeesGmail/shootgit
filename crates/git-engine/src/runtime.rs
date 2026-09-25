//! The single tokio runtime and its sizing (§4 Low-resource operation,
//! budget row "Background threads"; ADR 0004).
//!
//! The app has exactly one runtime. The Tauri shell builds it with [`build`]
//! and hands it to `tauri::async_runtime::set` before constructing the
//! builder, so Tauri never creates its own; the dev CLI and the perf harness
//! use the same builder so they measure what the app runs on.
//!
//! Workers: `max(1, available_parallelism() - 1)`, leaving one core for the
//! UI thread and the WebView. On a 2-core laptop that is a single worker.

use std::num::NonZeroUsize;

/// The hardware threads this process may use (cgroup quotas included on
/// Linux), or 1 when the platform cannot say.
pub fn available_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

/// Worker threads for a machine with `parallelism` hardware threads:
/// `max(1, parallelism - 1)`.
pub fn worker_threads(parallelism: usize) -> usize {
    parallelism.saturating_sub(1).max(1)
}

/// Name given to the runtime's worker threads, so they can be told apart
/// from the WebView's and libraries' threads in a profiler.
pub const WORKER_THREAD_NAME: &str = "app-worker";

/// Builds the runtime: multi-threaded, sized by [`worker_threads`] from
/// [`available_parallelism`], with the I/O and time drivers enabled.
///
/// The blocking pool keeps tokio's defaults: its threads are created on
/// demand and exit after 10 s idle, and nothing in the engine uses it yet.
pub fn build() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads(available_parallelism()))
        .thread_name(WORKER_THREAD_NAME)
        .enable_all()
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_parallelism_is_at_least_one() {
        assert!(available_parallelism() >= 1);
    }

    #[test]
    fn workers_are_named() {
        let runtime = build().unwrap();
        let name = runtime.block_on(async {
            tokio::spawn(async { std::thread::current().name().map(str::to_owned) })
                .await
                .unwrap()
        });
        assert_eq!(name.as_deref(), Some(WORKER_THREAD_NAME));
    }
}
