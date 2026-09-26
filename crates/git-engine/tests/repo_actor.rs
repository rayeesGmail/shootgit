#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-09: the per-repository actor (SPEC §4 Process model: "One `RepoActor`
//! per open repository (tokio task) serializes writes ...; reads run
//! concurrently").
//!
//! Operations run in the order they were submitted, with one relaxation:
//! consecutive reads run at the same time. A write waits for every read
//! submitted before it, runs alone, and every operation submitted after it
//! waits for it.
//!
//! Most tests run with tokio's clock paused, so "let everything that can run,
//! run" is exact: a sleep returns only once every task is idle.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use git_engine::error::GitError;
use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::{CancellationToken, GitCommand};
use git_engine::repo::{open_repo, Repo};
use git_engine::repo_actor::RepoActor;
use git_engine::status::{status, FileStatus, Status, StatusOptions};
use tokio::sync::Semaphore;

/// What the operations did, in the order they did it.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, event: impl Into<String>) {
        self.0.lock().unwrap().push(event.into());
    }

    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// The actor never touches the repository itself; these tests' operations
/// do not either.
fn unused_repo() -> Repo {
    Repo::new("git", "no-such-repository")
}

/// With the clock paused this returns once every task is idle: whatever
/// could run has run.
async fn settle() {
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// `events` sorted, for comparing a stretch whose order is not specified.
fn sorted(events: &[String]) -> Vec<String> {
    let mut events = events.to_vec();
    events.sort();
    events
}

// ---- ordering ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn two_concurrent_reads_and_one_write_keep_their_order() {
    let actor = RepoActor::spawn(unused_repo());
    let log = Log::default();
    // Holds both reads open until the test has checked on the write.
    let gate = Arc::new(Semaphore::new(0));

    let gated_read = |name: &'static str| {
        let log = log.clone();
        let gate = Arc::clone(&gate);
        actor.read(move |_repo| async move {
            log.push(format!("{name} start"));
            gate.acquire().await.unwrap().forget();
            log.push(format!("{name} end"));
            Ok(name)
        })
    };
    let r1 = gated_read("r1");
    let r2 = gated_read("r2");
    let w = actor.write({
        let log = log.clone();
        move |_repo| async move {
            log.push("w start");
            // Long enough for anything that wrongly runs beside the write to
            // start.
            tokio::time::sleep(Duration::from_millis(50)).await;
            log.push("w end");
            Ok("w")
        }
    });
    let r3 = actor.read({
        let log = log.clone();
        move |_repo| async move {
            log.push("r3 start");
            log.push("r3 end");
            Ok("r3")
        }
    });

    settle().await;
    // Both reads are running at once; the write waits for them, and the read
    // after the write waits for it.
    assert_eq!(sorted(&log.events()), ["r1 start", "r2 start"]);

    gate.add_permits(2);
    let (r1, r2, w, r3) = tokio::join!(r1, r2, w, r3);

    assert_eq!(
        [r1.unwrap(), r2.unwrap(), w.unwrap(), r3.unwrap()],
        ["r1", "r2", "w", "r3"]
    );
    let events = log.events();
    assert_eq!(sorted(&events[..2]), ["r1 start", "r2 start"], "{events:?}");
    assert_eq!(sorted(&events[2..4]), ["r1 end", "r2 end"], "{events:?}");
    assert_eq!(
        events[4..],
        ["w start", "w end", "r3 start", "r3 end"],
        "{events:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn writes_run_one_at_a_time_in_submission_order() {
    let actor = RepoActor::spawn(unused_repo());
    let log = Log::default();

    let write = |name: &'static str| {
        let log = log.clone();
        actor.write(move |_repo| async move {
            log.push(format!("{name} start"));
            tokio::time::sleep(Duration::from_millis(10)).await;
            log.push(format!("{name} end"));
            Ok(())
        })
    };
    let (a, b, c) = tokio::join!(write("w1"), write("w2"), write("w3"));

    a.unwrap();
    b.unwrap();
    c.unwrap();
    assert_eq!(
        log.events(),
        ["w1 start", "w1 end", "w2 start", "w2 end", "w3 start", "w3 end"]
    );
}

#[tokio::test(start_paused = true)]
async fn reads_queued_behind_a_write_run_together_after_it() {
    let actor = RepoActor::spawn(unused_repo());
    let log = Log::default();
    let gate = Arc::new(Semaphore::new(0));

    let w = actor.write({
        let log = log.clone();
        let gate = Arc::clone(&gate);
        move |_repo| async move {
            log.push("w start");
            gate.acquire().await.unwrap().forget();
            log.push("w end");
            Ok(())
        }
    });
    let read = |name: &'static str| {
        let log = log.clone();
        let gate = Arc::clone(&gate);
        actor.read(move |_repo| async move {
            log.push(format!("{name} start"));
            gate.acquire().await.unwrap().forget();
            log.push(format!("{name} end"));
            Ok(())
        })
    };
    let r1 = read("r1");
    let r2 = read("r2");

    settle().await;
    assert_eq!(log.events(), ["w start"]);

    gate.add_permits(1);
    settle().await;
    let events = log.events();
    assert_eq!(events[..2], ["w start", "w end"]);
    assert_eq!(sorted(&events[2..]), ["r1 start", "r2 start"]);

    gate.add_permits(2);
    let (r1, r2, w) = tokio::join!(r1, r2, w);
    r1.unwrap();
    r2.unwrap();
    w.unwrap();
}

// ---- results and failures ------------------------------------------------------

#[tokio::test]
async fn an_operation_gets_the_repository_and_its_result_reaches_the_caller() {
    let actor = RepoActor::spawn(Repo::new("my-git", "some/tree"));

    let workdir = actor
        .read(|repo| async move { Ok(repo.workdir().to_path_buf()) })
        .await
        .unwrap();
    let err = actor
        .write(|_repo| async { Err::<(), _>(GitError::Cancelled) })
        .await
        .unwrap_err();

    assert_eq!(workdir, Path::new("some/tree"));
    assert_eq!(actor.repo().git(), Path::new("my-git"));
    assert!(matches!(err, GitError::Cancelled), "{err:?}");
}

fn panics(what: &str) -> Result<(), GitError> {
    panic!("{what} panicked on purpose")
}

#[tokio::test]
async fn a_panicking_operation_fails_alone_and_the_actor_carries_on() {
    let actor = RepoActor::spawn(unused_repo());

    let read = actor
        .read(|_repo| async { panics("read") })
        .await
        .unwrap_err();
    let write = actor
        .write(|_repo| async { panics("write") })
        .await
        .unwrap_err();
    let after = actor.read(|_repo| async { Ok(7) }).await.unwrap();

    assert!(matches!(read, GitError::Aborted), "{read:?}");
    assert!(matches!(write, GitError::Aborted), "{write:?}");
    assert_eq!(after, 7);
}

#[tokio::test(start_paused = true)]
async fn submitted_operations_still_run_after_the_last_handle_is_dropped() {
    let actor = RepoActor::spawn(unused_repo());
    let gate = Arc::new(Semaphore::new(0));
    let write = actor.write({
        let gate = Arc::clone(&gate);
        move |_repo| async move {
            gate.acquire().await.unwrap().forget();
            Ok("written")
        }
    });
    let read = actor.read(|_repo| async { Ok("read") });

    drop(actor);
    settle().await;
    gate.add_permits(1);

    assert_eq!(write.await.unwrap(), "written");
    assert_eq!(read.await.unwrap(), "read");
}

// ---- on a real repository ------------------------------------------------------

async fn machine_git() -> PathBuf {
    resolve(&ResolveOptions::from_env(None)).await.unwrap().path
}

fn state_of<'a>(status: &'a Status, path: &str) -> Option<&'a FileStatus> {
    status
        .entries
        .iter()
        .find(|entry| entry.path == Path::new(path))
        .map(|entry| &entry.index_status)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reads_see_a_staging_write_only_if_submitted_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let git = machine_git().await;
    let init = Command::new(&git)
        .current_dir(dir.path())
        .args(["init", "-q", "--initial-branch=main"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::write(dir.path().join("file.txt"), "content\n").unwrap();
    let actor = RepoActor::spawn(open_repo(&git, dir.path()).unwrap());

    let read_status = || {
        actor.read(|repo| async move {
            status(&repo, &StatusOptions::default(), &CancellationToken::new()).await
        })
    };
    let before_1 = read_status();
    let before_2 = read_status();
    let stage = actor.write(|repo| async move {
        let mut git = GitCommand::new(repo.git());
        git.current_dir(repo.workdir())
            .args(["add", "--", "file.txt"]);
        git.output().await.map(drop)
    });
    let after = read_status();
    let (before_1, before_2, stage, after) = tokio::join!(before_1, before_2, stage, after);

    stage.unwrap();
    for before in [before_1.unwrap(), before_2.unwrap()] {
        assert_eq!(state_of(&before, "file.txt"), Some(&FileStatus::Untracked));
    }
    assert_eq!(
        state_of(&after.unwrap(), "file.txt"),
        Some(&FileStatus::Added)
    );
}
