#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-10: the repository watcher (SPEC §5 External change sync, §4 IPC
//! contract "repo-changed ... debounced 150 ms").
//!
//! Every test here drives a real OS watcher (FSEvents, inotify or
//! ReadDirectoryChangesW) on a real repository in a temp dir, so the timing
//! helpers are generous: an event must arrive within [`ARRIVAL`], and
//! "nothing happens" is checked over [`quiet`], several coalescing windows.
//! The pure classification rules have unit tests in the module itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::CancellationToken;
use git_engine::repo::{open_repo, Repo};
use git_engine::repo_actor::RepoActor;
use git_engine::status::{status, FileStatus, StatusOptions};
use git_engine::watcher::{
    ChangeKind, ChangeKinds, Events, RepoChanged, WatchOptions, Watcher, DEFAULT_WINDOW,
};
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tokio::time::timeout;

/// How long an event may take to arrive. Far above the 300 ms budget so a
/// loaded CI runner does not fail the test; the latency itself is printed.
const ARRIVAL: Duration = Duration::from_secs(5);

/// Long enough that a second coalescing window would have opened, closed
/// and been delivered: "no event within `quiet()`" means no event.
fn quiet() -> Duration {
    DEFAULT_WINDOW * 4 + Duration::from_millis(300)
}

async fn machine_git() -> PathBuf {
    resolve(&ResolveOptions::from_env(None)).await.unwrap().path
}

/// A fresh repository with one committed file, built with a git that ignores
/// the machine's and the user's config.
struct Fixture {
    dir: TempDir,
    git: PathBuf,
    global_config: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let global_config = dir.path().join("gitconfig");
        fs::write(&global_config, "").unwrap();
        let fixture = Self {
            dir,
            git: machine_git().await,
            global_config,
        };
        fs::create_dir(fixture.workdir()).unwrap();
        fixture.git(&fixture.workdir(), &["init", "-q", "--initial-branch=main"]);
        fs::write(fixture.workdir().join("tracked.txt"), "one\n").unwrap();
        fixture.git(&fixture.workdir(), &["add", "tracked.txt"]);
        fixture.git(&fixture.workdir(), &["commit", "-q", "-m", "base"]);
        fixture
    }

    /// The working tree, a subdirectory of the temp dir so the temp dir can
    /// hold a linked worktree and the global config beside it.
    fn workdir(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn git_dir(&self) -> PathBuf {
        self.workdir().join(".git")
    }

    fn repo(&self) -> Repo {
        // On purpose not `open_repo`: the temp dir is a symlink on macOS
        // (`/var` -> `/private/var`) and the watcher must cope with a
        // non-canonical `Repo` because FSEvents reports canonical paths.
        Repo::new(&self.git, self.workdir())
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new(&self.git)
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.global_config)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    async fn watch(&self) -> (Watcher, Events) {
        Watcher::spawn(&self.repo(), WatchOptions::default())
            .await
            .unwrap()
    }

    /// [`watch`](Self::watch), then [`settle`]: for tests that assert quiet
    /// or exact kinds after writes made before the watch started.
    async fn watch_settled(&self) -> (Watcher, Events) {
        let (watcher, mut events) = self.watch().await;
        settle(&mut events).await;
        (watcher, events)
    }
}

/// Writes `contents` to `path`, creating parent directories.
fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// `touch`: bumps the modification time of an existing file without
/// changing its contents.
fn touch(path: impl AsRef<Path>) {
    let file = fs::File::options().append(true).open(path).unwrap();
    file.set_modified(SystemTime::now()).unwrap();
}

/// The next event, which must arrive within [`ARRIVAL`] and must not be a
/// failure.
async fn next(events: &mut Events) -> RepoChanged {
    match timeout(ARRIVAL, events.recv()).await {
        Ok(Some(Ok(changed))) => changed,
        Ok(Some(Err(error))) => panic!("the watcher failed: {error}"),
        Ok(None) => panic!("the watcher stopped"),
        Err(_) => panic!("no event within {ARRIVAL:?}"),
    }
}

/// The kinds of the next event, merged with anything else that trickles in
/// over the following [`quiet`] period. For tests about *which* kinds a
/// change produces, not how many events.
async fn kinds_after(events: &mut Events) -> ChangeKinds {
    let mut kinds = next(events).await.kinds;
    let settle = Instant::now() + quiet();
    while let Ok(Some(Ok(more))) = timeout(
        settle.saturating_duration_since(Instant::now()),
        events.recv(),
    )
    .await
    {
        kinds |= more.kinds;
    }
    kinds
}

/// Fails if any event arrives within [`quiet`].
async fn assert_quiet(events: &mut Events, what: &str) {
    match timeout(quiet(), events.recv()).await {
        Err(_) => {}
        Ok(Some(Ok(changed))) => panic!("{what} produced an event: {changed:?}"),
        Ok(Some(Err(error))) => panic!("{what}: the watcher failed: {error}"),
        Ok(None) => panic!("{what}: the watcher stopped"),
    }
}

/// Drains whatever the backend delivers for changes made just before the
/// watch started (FSEvents does that), returning once a full [`quiet`]
/// period passes with nothing. Nothing is asserted about what is drained.
async fn settle(events: &mut Events) {
    loop {
        match timeout(quiet(), events.recv()).await {
            Err(_) => return,
            Ok(Some(Ok(changed))) => eprintln!("drained a pre-watch event: {changed:?}"),
            Ok(Some(Err(error))) => panic!("the watcher failed while settling: {error}"),
            Ok(None) => panic!("the watcher stopped while settling"),
        }
    }
}

fn only(kind: ChangeKind) -> ChangeKinds {
    ChangeKinds::from_iter([kind])
}

// ---- the two tests named by the plan ---------------------------------------

#[tokio::test]
async fn touching_a_file_produces_exactly_one_event() {
    let fixture = Fixture::new().await;
    let (watcher, mut events) = fixture.watch().await;
    // The fixture's own `git commit` ran moments before the watch; FSEvents
    // may still deliver it. Draining first keeps the touch's event the one
    // measured; the touch-to-event latency itself is unaffected.
    settle(&mut events).await;

    let touched_at = Instant::now();
    touch(fixture.workdir().join("tracked.txt"));
    let changed = next(&mut events).await;
    let latency = touched_at.elapsed();
    eprintln!("touch -> event latency: {latency:?} (window {DEFAULT_WINDOW:?})");

    assert_eq!(changed.kinds, only(ChangeKind::Status), "{changed:?}");
    assert_eq!(changed.generation, 0);
    assert!(
        latency >= DEFAULT_WINDOW,
        "the event came before the coalescing window closed: {latency:?}"
    );
    assert!(latency < Duration::from_secs(2), "too slow: {latency:?}");
    assert_quiet(&mut events, "one touch").await;
    drop(watcher);
}

#[tokio::test]
async fn editing_head_reports_head() {
    let fixture = Fixture::new().await;
    let (_watcher, mut events) = fixture.watch_settled().await;

    write(fixture.git_dir().join("HEAD"), "ref: refs/heads/other\n");
    let kinds = kinds_after(&mut events).await;

    assert!(kinds.contains(ChangeKind::Head), "{kinds:?}");
    assert!(!kinds.contains(ChangeKind::Status), "{kinds:?}");
}

// ---- classification on disk -------------------------------------------------

#[tokio::test]
async fn git_dir_paths_map_to_their_kinds() {
    let fixture = Fixture::new().await;
    let git_dir = fixture.git_dir();
    let (_watcher, mut events) = fixture.watch_settled().await;

    let cases: [(&str, ChangeKind); 8] = [
        ("index", ChangeKind::Index),
        ("refs/heads/feature", ChangeKind::Refs),
        ("packed-refs", ChangeKind::Refs),
        ("ORIG_HEAD", ChangeKind::Refs),
        ("MERGE_HEAD", ChangeKind::State),
        ("rebase-merge/head-name", ChangeKind::State),
        ("logs/HEAD", ChangeKind::Reflog),
        ("config", ChangeKind::Config),
    ];
    for (path, expected) in cases {
        write(
            git_dir.join(path),
            "0123456789abcdef0123456789abcdef01234567\n",
        );
        let kinds = kinds_after(&mut events).await;
        assert!(kinds.contains(expected), "{path}: {kinds:?}");
        assert!(
            !kinds.contains(ChangeKind::Status),
            "{path} is not a working-tree change: {kinds:?}"
        );
    }

    // Object writes, git's scratch files and lock files are noise.
    write(
        git_dir.join("objects/ab/cdef0123456789abcdef0123456789abcdef01"),
        "x",
    );
    write(git_dir.join("COMMIT_EDITMSG"), "wip\n");
    write(git_dir.join("refs/heads/feature.lock"), "x");
    fs::remove_file(git_dir.join("refs/heads/feature.lock")).unwrap();
    assert_quiet(&mut events, "objects, COMMIT_EDITMSG and a ref lock").await;
}

#[tokio::test]
async fn changes_within_the_window_coalesce_into_one_event() {
    let fixture = Fixture::new().await;
    let (_watcher, mut events) = fixture.watch_settled().await;

    let names = [
        "a.txt",
        "dir with spaces/b.txt",
        "ünïcødé/日本語.txt",
        "deep/er/still/c.txt",
        "tracked.txt",
    ];
    for name in names {
        write(fixture.workdir().join(name), "changed\n");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let changed = next(&mut events).await;
    assert_eq!(changed.kinds, only(ChangeKind::Status), "{changed:?}");
    assert_quiet(&mut events, "five writes inside one window").await;
}

#[tokio::test]
async fn ignored_paths_produce_no_event() {
    let fixture = Fixture::new().await;
    let workdir = fixture.workdir();
    // CRLF on purpose: a `.gitignore` written on Windows must work as-is.
    write(workdir.join(".gitignore"), "*.log\r\nbuild/\r\n");
    write(workdir.join("sub/.gitignore"), "secret.txt\n");
    write(fixture.git_dir().join("info/exclude"), "excluded.txt\n");
    fs::create_dir(workdir.join("build")).unwrap();
    let (_watcher, mut events) = fixture.watch_settled().await;

    write(workdir.join("debug.log"), "log\n");
    write(workdir.join("build/out.bin"), "bin\n");
    write(workdir.join("build/nested/deeper.txt"), "bin\n");
    write(workdir.join("sub/secret.txt"), "shh\n");
    write(workdir.join("excluded.txt"), "excluded\n");
    assert_quiet(&mut events, "writes to ignored paths").await;

    // The watcher is alive: a non-ignored sibling is reported.
    write(workdir.join("sub/visible.txt"), "seen\n");
    assert_eq!(next(&mut events).await.kinds, only(ChangeKind::Status));
}

#[tokio::test]
async fn editing_gitignore_reloads_the_rules() {
    let fixture = Fixture::new().await;
    let workdir = fixture.workdir();
    write(workdir.join(".gitignore"), "");
    let (_watcher, mut events) = fixture.watch_settled().await;

    write(workdir.join(".gitignore"), "*.log\n");
    // Changing the rules changes the untracked set, so it is itself a change.
    assert!(next(&mut events).await.kinds.contains(ChangeKind::Status));
    write(workdir.join("now-ignored.log"), "x\n");
    assert_quiet(&mut events, "a file the new rules ignore").await;

    write(workdir.join(".gitignore"), "");
    assert!(next(&mut events).await.kinds.contains(ChangeKind::Status));
    write(workdir.join("visible-again.log"), "x\n");
    assert_eq!(next(&mut events).await.kinds, only(ChangeKind::Status));
}

// ---- own-write suppression --------------------------------------------------

#[tokio::test]
async fn own_writes_are_suppressed_until_the_guard_is_dropped() {
    let fixture = Fixture::new().await;
    let (watcher, mut events) = fixture.watch_settled().await;
    assert_eq!(watcher.generation(), 0);

    let own = watcher.begin_write();
    assert_eq!(own.generation(), 1);
    assert_eq!(watcher.generation(), 1);
    write(fixture.workdir().join("written-by-us.txt"), "ours\n");
    write(fixture.git_dir().join("index"), "ours\n");
    // The events land while the guard is held (this also gives a slow
    // backend time to deliver them before the guard goes).
    assert_quiet(&mut events, "our own write").await;
    assert!(
        watcher.suppressed_events() >= 1,
        "the backend saw nothing to suppress"
    );
    drop(own);
    assert_quiet(&mut events, "dropping the guard").await;

    // An external change afterwards is reported, tagged with the generation.
    write(fixture.workdir().join("external.txt"), "theirs\n");
    let changed = next(&mut events).await;
    assert_eq!(changed.kinds, only(ChangeKind::Status), "{changed:?}");
    assert_eq!(changed.generation, 1);
}

#[tokio::test]
async fn overlapping_guards_suppress_until_the_last_one_goes() {
    let fixture = Fixture::new().await;
    let (watcher, mut events) = fixture.watch_settled().await;

    let first = watcher.begin_write();
    let second = watcher.begin_write();
    drop(first);
    write(fixture.workdir().join("still-ours.txt"), "ours\n");
    assert_quiet(&mut events, "a write under the second guard").await;
    drop(second);

    write(fixture.workdir().join("external.txt"), "theirs\n");
    assert_eq!(next(&mut events).await.generation, 2);
}

// ---- index.lock ---------------------------------------------------------------

#[tokio::test]
async fn an_index_lock_holds_the_event_until_it_disappears() {
    let fixture = Fixture::new().await;
    let lock = fixture.git_dir().join("index.lock");
    let options = WatchOptions {
        lock_hold: Duration::from_secs(10),
        ..WatchOptions::default()
    };
    let (_watcher, mut events) = Watcher::spawn(&fixture.repo(), options).await.unwrap();
    settle(&mut events).await;

    write(&lock, "");
    write(fixture.workdir().join("mid-operation.txt"), "x\n");
    assert_quiet(&mut events, "a change while index.lock exists").await;

    let released_at = Instant::now();
    fs::remove_file(&lock).unwrap();
    let changed = next(&mut events).await;
    assert!(changed.kinds.contains(ChangeKind::Status), "{changed:?}");
    assert!(
        released_at.elapsed() < Duration::from_secs(2),
        "the held event should follow the lock's removal promptly"
    );
}

#[tokio::test]
async fn a_stale_index_lock_only_delays_events() {
    let fixture = Fixture::new().await;
    let options = WatchOptions {
        lock_hold: Duration::from_millis(400),
        ..WatchOptions::default()
    };
    let (_watcher, mut events) = Watcher::spawn(&fixture.repo(), options).await.unwrap();
    settle(&mut events).await;

    write(fixture.git_dir().join("index.lock"), "");
    let written_at = Instant::now();
    write(fixture.workdir().join("crashed-mid-write.txt"), "x\n");
    let changed = next(&mut events).await;

    assert!(changed.kinds.contains(ChangeKind::Status), "{changed:?}");
    assert!(
        written_at.elapsed() >= DEFAULT_WINDOW + Duration::from_millis(400),
        "the event came before the hold expired: {:?}",
        written_at.elapsed()
    );
}

// ---- lifetime -----------------------------------------------------------------

#[tokio::test]
async fn dropping_the_handle_stops_the_watcher() {
    let fixture = Fixture::new().await;
    let (watcher, mut events) = fixture.watch_settled().await;
    let stopped = watcher.stopped();

    drop(watcher);

    timeout(ARRIVAL, stopped).await.unwrap();
    assert!(
        timeout(ARRIVAL, events.recv()).await.unwrap().is_none(),
        "the event stream should end"
    );
    // Nothing is reported for a change after the stop.
    write(fixture.workdir().join("after.txt"), "x\n");
    tokio::time::sleep(quiet()).await;
    assert!(events.recv().await.is_none());
}

#[tokio::test]
async fn dropping_the_receiver_stops_the_watcher() {
    let fixture = Fixture::new().await;
    let (watcher, events) = fixture.watch().await;

    drop(events);

    timeout(ARRIVAL, watcher.stopped()).await.unwrap();
    // The handle stays usable; guards still count.
    let own = watcher.begin_write();
    assert_eq!(own.generation(), 1);
}

#[tokio::test]
async fn a_missing_repository_fails_to_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::new("git", dir.path().join("nowhere"));

    let error = Watcher::spawn(&repo, WatchOptions::default())
        .await
        .expect_err("spawning on a missing directory must fail");

    assert!(
        matches!(error, git_engine::error::GitError::Io { .. }),
        "{error:?}"
    );
}

// ---- linked worktrees ---------------------------------------------------------

#[tokio::test]
async fn a_linked_worktree_watches_its_own_git_dir_and_the_common_dir() {
    let fixture = Fixture::new().await;
    let main = fixture.workdir();
    let linked = fixture.dir.path().join("linked");
    fixture.git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            linked.to_str().unwrap(),
        ],
    );
    let repo = open_repo(&fixture.git, &linked).unwrap();
    assert_ne!(repo.git_dir(), repo.common_dir());
    let (_watcher, mut events) = Watcher::spawn(&repo, WatchOptions::default())
        .await
        .unwrap();
    settle(&mut events).await;

    write(linked.join("new.txt"), "x\n");
    assert_eq!(next(&mut events).await.kinds, only(ChangeKind::Status));

    write(repo.git_dir().join("HEAD"), "ref: refs/heads/main\n");
    let kinds = kinds_after(&mut events).await;
    assert!(kinds.contains(ChangeKind::Head), "{kinds:?}");

    write(
        repo.common_dir().join("refs/heads/shared"),
        "0123456789abcdef0123456789abcdef01234567\n",
    );
    let kinds = kinds_after(&mut events).await;
    assert!(kinds.contains(ChangeKind::Refs), "{kinds:?}");

    // The main worktree's HEAD and index are not ours.
    write(repo.common_dir().join("HEAD"), "ref: refs/heads/wt\n");
    write(repo.common_dir().join("index"), "not ours\n");
    write(main.join("main-only.txt"), "not watched\n");
    assert_quiet(&mut events, "the main worktree's own state").await;
}

// ---- with the actor (P0-09 follow-up) -----------------------------------------

/// A watcher event that arrives while the actor is inside a write must not
/// be lost: the watcher runs and buffers outside the actor loop, and the
/// status read it prompts queues behind the write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_during_a_write_are_buffered_outside_the_actor() {
    let fixture = Fixture::new().await;
    let repo = open_repo(&fixture.git, fixture.workdir()).unwrap();
    let (watcher, mut events) = Watcher::spawn(&repo, WatchOptions::default())
        .await
        .unwrap();
    settle(&mut events).await;
    let actor = RepoActor::spawn(repo);
    let gate = Arc::new(Semaphore::new(0));

    // A write that blocks the actor loop until the gate opens.
    let write_op = actor.write({
        let gate = Arc::clone(&gate);
        move |_repo| async move {
            gate.acquire().await.unwrap().forget();
            Ok(())
        }
    });
    tokio::task::yield_now().await;

    // An external change while the write is running.
    write(fixture.workdir().join("external.txt"), "x\n");
    let changed = next(&mut events).await;
    assert!(changed.kinds.contains(ChangeKind::Status), "{changed:?}");

    // The refresh it prompts waits for the write; nothing was dropped.
    let refresh = actor.read(|repo| async move {
        status(&repo, &StatusOptions::default(), &CancellationToken::new()).await
    });
    let blocked = timeout(Duration::from_millis(200), refresh);
    let (blocked, _) = tokio::join!(blocked, async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        gate.add_permits(1);
    });
    assert!(
        blocked.is_err(),
        "the read should have waited for the write"
    );
    write_op.await.unwrap();

    let status = actor
        .read(|repo| async move {
            status(&repo, &StatusOptions::default(), &CancellationToken::new()).await
        })
        .await
        .unwrap();
    let external = status
        .entries
        .iter()
        .find(|entry| entry.path == Path::new("external.txt"))
        .expect("the external file is in the status");
    assert_eq!(external.worktree_status, FileStatus::Untracked);
    drop(watcher);
}
