#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-17: the shared git concurrency limiter and cancellable reads (SPEC §4
//! Low-resource operation, ADR 0004).
//!
//! The limiter caps concurrent `git` processes and serves the visible view
//! before background work. Every `GitCommand` takes a `CancellationToken`
//! and stops promptly, whether it is still queued or already running.

use std::future::Future;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Waker};
use std::time::{Duration, Instant};

use git_engine::error::GitError;
use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::{cap_for, CancellationToken, GitCommand, Limiter, Priority};
use git_engine::runtime;
use tokio::sync::watch;

fn machine_git() -> PathBuf {
    resolve(&ResolveOptions::from_env(None)).unwrap().path
}

/// Yields until `condition` holds, or panics after a few seconds.
async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition never became true");
        tokio::task::yield_now().await;
    }
}

// ---- sizing rules ----------------------------------------------------------

#[test]
fn limiter_cap_is_2_on_small_machines_and_4_otherwise() {
    assert_eq!(cap_for(1), 2);
    assert_eq!(cap_for(2), 2);
    assert_eq!(cap_for(4), 2);
    assert_eq!(cap_for(5), 4);
    assert_eq!(cap_for(64), 4);
}

#[test]
fn shared_limiter_is_sized_from_the_machine() {
    let shared = Limiter::shared();
    assert_eq!(shared.cap(), cap_for(runtime::available_parallelism()));
    assert!(
        std::ptr::eq(shared, Limiter::shared()),
        "one limiter per process"
    );
}

#[test]
fn worker_threads_leave_one_core_for_the_ui_and_never_drop_below_one() {
    assert_eq!(runtime::worker_threads(0), 1);
    assert_eq!(runtime::worker_threads(1), 1);
    assert_eq!(runtime::worker_threads(2), 1);
    assert_eq!(runtime::worker_threads(4), 3);
    assert_eq!(runtime::worker_threads(14), 13);
}

#[test]
fn the_runtime_builder_applies_the_worker_thread_rule() {
    let rt = runtime::build().unwrap();
    assert_eq!(
        rt.metrics().num_workers(),
        runtime::worker_threads(runtime::available_parallelism())
    );
}

// ---- cap -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn limiter_never_exceeds_its_cap_under_50_concurrent_requests() {
    const REQUESTS: usize = 50;
    let limiter = Limiter::new(NonZeroUsize::new(4).unwrap());
    let in_flight = Arc::new(AtomicUsize::new(0));
    let observed_max = Arc::new(AtomicUsize::new(0));
    // Holders keep their permit until told to go, so the queue provably
    // fills up to `REQUESTS - cap` before anything is released.
    let (go_tx, go_rx) = watch::channel(false);

    let tasks: Vec<_> = (0..REQUESTS)
        .map(|i| {
            let limiter = limiter.clone();
            let in_flight = Arc::clone(&in_flight);
            let observed_max = Arc::clone(&observed_max);
            let mut go = go_rx.clone();
            let priority = if i % 3 == 0 {
                Priority::Visible
            } else {
                Priority::Background
            };
            tokio::spawn(async move {
                let token = CancellationToken::new();
                let permit = limiter.acquire(priority, &token).await.unwrap();
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                observed_max.fetch_max(now, Ordering::SeqCst);
                go.wait_for(|&released| released).await.unwrap();
                // Hold a little longer so the in-flight count is observable.
                tokio::time::sleep(Duration::from_millis(2)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
            })
        })
        .collect();

    wait_until(|| {
        limiter.in_flight() == 4
            && limiter.waiting(Priority::Visible) + limiter.waiting(Priority::Background)
                == REQUESTS - 4
    })
    .await;
    go_tx.send(true).unwrap();
    for task in tasks {
        task.await.unwrap();
    }

    assert_eq!(observed_max.load(Ordering::SeqCst), 4);
    assert_eq!(limiter.peak_in_flight(), 4);
    assert_eq!(limiter.in_flight(), 0);
    assert_eq!(limiter.waiting(Priority::Visible), 0);
    assert_eq!(limiter.waiting(Priority::Background), 0);
}

// ---- priority --------------------------------------------------------------

#[tokio::test]
async fn high_priority_request_completes_before_queued_low_priority_ones() {
    let limiter = Limiter::new(NonZeroUsize::new(1).unwrap());
    let order = Arc::new(Mutex::new(Vec::new()));
    let token = CancellationToken::new();

    let held = limiter.acquire(Priority::Visible, &token).await.unwrap();

    let queue = |label: &'static str, priority: Priority| {
        let limiter = limiter.clone();
        let order = Arc::clone(&order);
        tokio::spawn(async move {
            let token = CancellationToken::new();
            let permit = limiter.acquire(priority, &token).await.unwrap();
            order.lock().unwrap().push(label);
            drop(permit);
        })
    };

    let mut tasks = Vec::new();
    for label in ["bg-1", "bg-2", "bg-3", "bg-4", "bg-5"] {
        tasks.push(queue(label, Priority::Background));
    }
    wait_until(|| limiter.waiting(Priority::Background) == 5).await;
    tasks.push(queue("visible", Priority::Visible));
    wait_until(|| limiter.waiting(Priority::Visible) == 1).await;

    drop(held);
    for task in tasks {
        task.await.unwrap();
    }

    assert_eq!(
        *order.lock().unwrap(),
        ["visible", "bg-1", "bg-2", "bg-3", "bg-4", "bg-5"],
        "the visible lane goes first; each lane is FIFO"
    );
}

// ---- cancellation ----------------------------------------------------------

#[tokio::test]
async fn already_cancelled_token_is_refused_without_taking_a_permit() {
    let limiter = Limiter::new(NonZeroUsize::new(1).unwrap());
    let token = CancellationToken::new();
    token.cancel();

    let result = limiter.acquire(Priority::Visible, &token).await;

    assert!(result.is_err());
    assert_eq!(limiter.in_flight(), 0);
    assert_eq!(limiter.peak_in_flight(), 0);
}

#[tokio::test]
async fn cancelling_a_queued_request_frees_its_slot_and_leaks_no_permit() {
    let limiter = Limiter::new(NonZeroUsize::new(1).unwrap());
    let never = CancellationToken::new();
    let held = limiter.acquire(Priority::Visible, &never).await.unwrap();

    let token = CancellationToken::new();
    let waiter = {
        let limiter = limiter.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let result = limiter.acquire(Priority::Background, &token).await;
            (result.is_err(), Instant::now())
        })
    };
    wait_until(|| limiter.waiting(Priority::Background) == 1).await;

    let cancelled_at = Instant::now();
    token.cancel();
    let (was_cancelled, returned_at) = waiter.await.unwrap();

    assert!(was_cancelled);
    assert!(
        returned_at.duration_since(cancelled_at) <= Duration::from_millis(50),
        "took {:?}",
        returned_at.duration_since(cancelled_at)
    );
    assert_eq!(limiter.waiting(Priority::Background), 0, "slot removed");

    drop(held);
    assert_eq!(limiter.in_flight(), 0);
    let again = limiter.acquire(Priority::Background, &never).await;
    assert!(again.is_ok(), "the permit came back");
}

#[tokio::test]
async fn a_permit_handed_to_a_dropped_waiter_is_released_again() {
    // The race: the holder releases (so the permit is sent to the queued
    // waiter) and the waiter's future is dropped before it is polled again.
    let limiter = Limiter::new(NonZeroUsize::new(1).unwrap());
    let never = CancellationToken::new();
    let held = limiter.acquire(Priority::Visible, &never).await.unwrap();

    let token = CancellationToken::new();
    // Boxed so `drop` below drops the future itself, not a pin to it.
    let mut waiting = Box::pin(limiter.acquire(Priority::Visible, &token));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(waiting.as_mut().poll(&mut cx).is_pending());
    assert_eq!(limiter.waiting(Priority::Visible), 1);

    drop(held); // hands the permit to `waiting` without it being polled
    assert_eq!(limiter.in_flight(), 1, "the permit is in transit");
    drop(waiting);

    assert_eq!(
        limiter.in_flight(),
        0,
        "dropping the unpolled future released it"
    );
    assert!(limiter.acquire(Priority::Background, &never).await.is_ok());
}

#[tokio::test]
async fn cancelled_read_returns_within_50_ms_while_git_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let token = CancellationToken::new();
    let mut git = GitCommand::new(machine_git());
    // git -> sh -> sleep: the grandchild holds the pipes, so only a tree kill
    // lets the read return.
    git.current_dir(dir.path())
        .config("alias.hang", "!sleep 10")
        .arg("hang")
        .timeout(None)
        .cancel_token(token.clone());

    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn({
        let token = token.clone();
        async move {
            // Long enough for git, sh and sleep to all be up.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let cancelled_at = Instant::now();
            token.cancel();
            tx.send(cancelled_at).unwrap();
        }
    });

    let result = git.output().await;
    let returned_at = Instant::now();
    let cancelled_at = rx.await.unwrap();

    assert!(matches!(result, Err(GitError::Cancelled)), "{result:?}");
    let latency = returned_at.duration_since(cancelled_at);
    assert!(latency <= Duration::from_millis(50), "took {latency:?}");
}

#[tokio::test]
async fn cancelled_read_returns_within_50_ms_while_queued_behind_the_limiter() {
    let limiter = Limiter::new(NonZeroUsize::new(1).unwrap());
    let never = CancellationToken::new();
    let held = limiter.acquire(Priority::Visible, &never).await.unwrap();

    let token = CancellationToken::new();
    let mut git = GitCommand::new(machine_git());
    git.arg("--version")
        .limiter(limiter.clone())
        .cancel_token(token.clone());

    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn({
        let token = token.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let cancelled_at = Instant::now();
            token.cancel();
            tx.send(cancelled_at).unwrap();
        }
    });

    let result = git.output().await;
    let returned_at = Instant::now();
    let cancelled_at = rx.await.unwrap();

    assert!(matches!(result, Err(GitError::Cancelled)), "{result:?}");
    let latency = returned_at.duration_since(cancelled_at);
    assert!(latency <= Duration::from_millis(50), "took {latency:?}");
    // A spawn needs a slot, and the only slot never changed hands.
    assert_eq!(limiter.peak_in_flight(), 1);
    assert_eq!(limiter.waiting(Priority::Visible), 0);
    drop(held);
    assert_eq!(limiter.in_flight(), 0);
}

#[tokio::test]
async fn pre_cancelled_read_never_takes_a_slot() {
    let limiter = Limiter::new(NonZeroUsize::new(1).unwrap());
    let token = CancellationToken::new();
    token.cancel();
    let mut git = GitCommand::new(machine_git());
    git.arg("--version")
        .limiter(limiter.clone())
        .cancel_token(token);

    let result = git.output().await;

    assert!(matches!(result, Err(GitError::Cancelled)), "{result:?}");
    assert_eq!(limiter.peak_in_flight(), 0, "no slot, so no spawn");
}

// ---- GitCommand goes through the limiter ------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_git_commands_are_bounded_by_their_limiter() {
    // 5 commands of 300 ms through a cap of 2 need three rounds: at least
    // 900 ms in total, and never more than 2 in flight.
    let dir = tempfile::tempdir().unwrap();
    let limiter = Limiter::new(NonZeroUsize::new(2).unwrap());
    let started = Instant::now();

    let tasks: Vec<_> = (0..5)
        .map(|_| {
            let mut git = GitCommand::new(machine_git());
            git.current_dir(dir.path())
                .config("alias.nap", "!sleep 0.3")
                .arg("nap")
                .limiter(limiter.clone());
            tokio::spawn(async move { git.output().await })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }

    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(limiter.peak_in_flight(), 2);
    assert_eq!(limiter.in_flight(), 0);
}

#[tokio::test]
async fn git_commands_use_the_shared_limiter_and_are_counted() {
    // Other tests in this binary spawn git concurrently, so the process-wide
    // counters can only be checked for growth here; the exact +1 is checked
    // under the unit tests' spawn lock in `process.rs`.
    let spawns_before = git_engine::process::git_spawn_count();
    let total_before = git_engine::process::spawn_count();
    let mut git = GitCommand::new(machine_git());
    git.arg("--version");

    git.output().await.unwrap();

    assert!(git_engine::process::git_spawn_count() > spawns_before);
    assert!(git_engine::process::spawn_count() > total_before);
    assert!(Limiter::shared().peak_in_flight() >= 1);
    assert!(Limiter::shared().peak_in_flight() <= Limiter::shared().cap());
}

#[test]
fn priority_defaults_to_visible() {
    // A spawn nobody classified is most likely a user action; the safe
    // mistake is to serve it early, not to park it behind bulk work.
    assert_eq!(Priority::default(), Priority::Visible);
}
