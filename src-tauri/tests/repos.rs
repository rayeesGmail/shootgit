#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-12: `open_repo`, `get_status` and `list_recent_repos`, and the
//! `repo-changed` event, driven over the real Tauri IPC bridge (SPEC §4 IPC
//! contract).
//!
//! The app is built by `app_lib::install`, the same function `run()` calls in
//! `setup`, so these tests cover the managed state and `mount_events` too:
//! without `mount_events` no event would reach a listener. `tauri::test`'s
//! mock runtime needs no display, and each test uses its own settings file
//! and repository in a temp dir.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use app_lib::settings::{Settings, SettingsStore};
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{App, Listener, WebviewWindow, WebviewWindowBuilder};
use tempfile::TempDir;

/// How long an event may take: far above the 300 ms budget, for loaded CI
/// runners.
const ARRIVAL: Duration = Duration::from_secs(10);

/// Long enough that a live watcher would have delivered a change: several
/// coalescing windows plus FSEvents latency. "No event within `QUIET`"
/// means nobody is watching.
fn quiet() -> Duration {
    git_engine::watcher::DEFAULT_WINDOW * 4 + Duration::from_millis(500)
}

/// A git on this machine, found the way the app finds it.
fn machine_git() -> PathBuf {
    tauri::async_runtime::block_on(async {
        let options = git_engine::git_binary::ResolveOptions::from_env(None);
        git_engine::git_binary::resolve(&options)
            .await
            .unwrap()
            .path
    })
}

struct TestApp {
    app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
    config: TempDir,
}

impl TestApp {
    /// An app wired as `run()` wires the real one, with `settings` in a temp
    /// dir.
    fn new(settings: Settings) -> Self {
        let config = tempfile::tempdir().unwrap();
        let settings_path = config.path().join("settings.json");
        app_lib::settings::save(&settings_path, &settings).unwrap();

        let specta = app_lib::ipc::builder::<MockRuntime>();
        let app = mock_builder()
            .invoke_handler(specta.invoke_handler())
            .build(mock_context(noop_assets()))
            .expect("the mock app builds");
        app_lib::install(&app, &specta, SettingsStore::load(settings_path));
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("the mock webview builds");
        Self {
            app,
            webview,
            config,
        }
    }

    /// The usual app: git from this machine, nothing recent yet.
    fn with_machine_git() -> Self {
        let mut settings = Settings::default();
        settings.git_path = Some(machine_git());
        Self::new(settings)
    }

    /// Invokes `cmd` with `args` over the IPC bridge and returns the JSON
    /// answer, or the JSON error.
    fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        let request = InvokeRequest {
            cmd: cmd.to_owned(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        };
        tauri::test::get_ipc_response(&self.webview, request)
            .map(|body| body.deserialize::<Value>().unwrap())
    }

    fn open(&self, path: &Path) -> Value {
        self.invoke("open_repo", json!({ "path": path }))
            .unwrap_or_else(|error| panic!("open_repo {path:?} failed: {error}"))
    }

    fn open_error(&self, path: &Path) -> Value {
        match self.invoke("open_repo", json!({ "path": path })) {
            Ok(opened) => panic!("open_repo {path:?} succeeded: {opened}"),
            Err(error) => error,
        }
    }

    fn recent(&self) -> Value {
        self.invoke("list_recent_repos", json!({})).unwrap()
    }

    fn settings_on_disk(&self) -> Settings {
        app_lib::settings::load(&self.config.path().join("settings.json"))
    }
}

/// A repository with one committed file, built with a git that ignores the
/// machine's and the user's config.
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir(fixture.workdir()).unwrap();
        std::fs::write(fixture.dir.path().join("gitconfig"), "").unwrap();
        fixture.git(&["init", "-q", "--initial-branch=main"]);
        std::fs::write(fixture.workdir().join("tracked.txt"), "one\n").unwrap();
        std::fs::create_dir(fixture.workdir().join("sub dir")).unwrap();
        std::fs::write(fixture.workdir().join("sub dir/nested.txt"), "n\n").unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-q", "-m", "base"]);
        fixture
    }

    fn workdir(&self) -> PathBuf {
        self.dir.path().join("répo")
    }

    /// The working tree as the app reports it: symlinks resolved (the temp
    /// dir is behind one on macOS), no `\\?\` prefix on Windows.
    fn canonical_workdir(&self) -> String {
        dunce::canonicalize(self.workdir())
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new(machine_git())
            .current_dir(self.workdir())
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.dir.path().join("gitconfig"))
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
    }
}

fn entry<'a>(status: &'a Value, path: &str) -> Option<&'a Value> {
    status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
}

// ---- open_repo ------------------------------------------------------------------

#[test]
fn open_repo_returns_the_first_status_of_the_repository() {
    let app = TestApp::with_machine_git();
    let fixture = Fixture::new();
    std::fs::write(fixture.workdir().join("new file.txt"), "new\n").unwrap();

    let opened = app.open(&fixture.workdir());

    let status = &opened["status"];
    assert!(status["repo"]["id"].is_u64(), "{opened:#}");
    assert_eq!(status["repo"]["path"], fixture.canonical_workdir());
    assert_eq!(status["repo"]["head"]["kind"], "branch");
    assert_eq!(status["repo"]["head"]["name"], "main");
    assert_eq!(
        entry(status, "new file.txt").map(|e| &e["worktree_status"]),
        Some(&json!("untracked")),
        "{status:#}"
    );
    assert_eq!(opened["watch_error"], Value::Null);
}

#[test]
fn open_repo_finds_the_repository_from_a_path_inside_it() {
    let app = TestApp::with_machine_git();
    let fixture = Fixture::new();

    let opened = app.open(&fixture.workdir().join("sub dir"));

    assert_eq!(
        opened["status"]["repo"]["path"],
        fixture.canonical_workdir()
    );
}

#[test]
fn opening_a_repository_again_keeps_its_id() {
    let app = TestApp::with_machine_git();
    let fixture = Fixture::new();

    let first = app.open(&fixture.workdir());
    let again = app.open(&fixture.workdir().join("sub dir"));

    assert_eq!(first["status"]["repo"]["id"], again["status"]["repo"]["id"]);
}

#[test]
fn two_repositories_get_different_ids() {
    let app = TestApp::with_machine_git();
    let (one, two) = (Fixture::new(), Fixture::new());

    let one = app.open(&one.workdir());
    let two = app.open(&two.workdir());

    assert_ne!(one["status"]["repo"]["id"], two["status"]["repo"]["id"]);
}

#[test]
fn open_repo_outside_any_repository_fails_and_records_nothing() {
    let app = TestApp::with_machine_git();
    let plain = tempfile::tempdir().unwrap();

    let error = app.open_error(plain.path());

    assert_eq!(error["kind"], "not_a_repository", "{error:#}");
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("not a git repository"));
    assert_eq!(error["fix"], Value::Null);
    assert_eq!(app.recent(), json!([]));
}

/// SPEC §5 Git binary resolution, tier 1: the git path from settings is
/// used, and when it is unusable that is an error, not a silent fallback
/// (P0-05).
#[test]
fn open_repo_resolves_git_from_the_settings_path() {
    let mut settings = Settings::default();
    let missing = tempfile::tempdir().unwrap().path().join("no-git-here");
    settings.git_path = Some(missing);
    let app = TestApp::new(settings);
    let fixture = Fixture::new();

    let error = app.open_error(&fixture.workdir());

    assert_eq!(error["kind"], "git_not_found", "{error:#}");
    assert!(
        error["message"].as_str().unwrap().contains("no-git-here"),
        "{error:#}"
    );
}

// ---- recent repositories --------------------------------------------------------

#[test]
fn opened_repositories_are_recent_newest_first_and_saved() {
    let app = TestApp::with_machine_git();
    let (older, newer) = (Fixture::new(), Fixture::new());

    app.open(&older.workdir());
    app.open(&newer.workdir());
    app.open(&newer.workdir().join("sub dir"));

    assert_eq!(
        app.recent(),
        json!([newer.canonical_workdir(), older.canonical_workdir()])
    );
    assert_eq!(
        app.settings_on_disk().recent_repos(),
        [
            PathBuf::from(newer.canonical_workdir()),
            PathBuf::from(older.canonical_workdir())
        ]
    );
}

#[test]
fn recent_repositories_come_from_the_settings_file() {
    let mut settings = Settings::default();
    settings.record_recent_repo("/work/older");
    settings.record_recent_repo("/work/newer");
    let app = TestApp::new(settings);

    assert_eq!(app.recent(), json!(["/work/newer", "/work/older"]));
}

// ---- get_status -----------------------------------------------------------------

#[test]
fn get_status_reads_the_open_repository_by_id() {
    let app = TestApp::with_machine_git();
    let fixture = Fixture::new();
    let opened = app.open(&fixture.workdir());
    let id = opened["status"]["repo"]["id"].clone();
    assert_eq!(entry(&opened["status"], "later.txt"), None);

    std::fs::write(fixture.workdir().join("later.txt"), "x\n").unwrap();
    let status = app.invoke("get_status", json!({ "repoId": id })).unwrap();

    assert_eq!(status["repo"]["id"], id);
    assert_eq!(
        entry(&status, "later.txt").map(|e| &e["worktree_status"]),
        Some(&json!("untracked")),
        "{status:#}"
    );
}

#[test]
fn get_status_for_a_repository_that_is_not_open_fails() {
    let app = TestApp::with_machine_git();

    let error = app
        .invoke("get_status", json!({ "repoId": 4_000_000_000_u32 }))
        .unwrap_err();

    assert_eq!(error["kind"], "unknown_repo", "{error:#}");
}

// ---- repo-changed ----------------------------------------------------------------

/// SPEC §4: repo state changes emit one coalesced
/// `repo-changed { repo_id, kinds }` event.
#[test]
fn a_change_on_disk_emits_repo_changed_for_the_open_repository() {
    let app = TestApp::with_machine_git();
    let fixture = Fixture::new();
    let (events, received) = mpsc::channel::<Value>();
    app.app.listen_any("repo-changed", move |event| {
        let _ = events.send(serde_json::from_str(event.payload()).unwrap());
    });
    let opened = app.open(&fixture.workdir());
    let id = opened["status"]["repo"]["id"].clone();

    std::fs::write(fixture.workdir().join("edited.txt"), "x\n").unwrap();

    // FSEvents may also deliver the fixture's own writes from just before the
    // watch started; wait for the one that reports the working tree.
    let deadline = Instant::now() + ARRIVAL;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = received
            .recv_timeout(left)
            .expect("a repo-changed event arrives");
        assert_eq!(event["repo_id"], id, "{event:#}");
        let kinds = event["kinds"].as_array().unwrap();
        if kinds.contains(&json!("status")) {
            break;
        }
    }
}

/// Records every `repo-changed` payload the app emits.
fn record_repo_changed(app: &TestApp) -> mpsc::Receiver<Value> {
    let (events, received) = mpsc::channel::<Value>();
    app.app.listen_any("repo-changed", move |event| {
        let _ = events.send(serde_json::from_str(event.payload()).unwrap());
    });
    received
}

/// Everything received within `period`.
fn drain(received: &mpsc::Receiver<Value>, period: Duration) -> Vec<Value> {
    let deadline = Instant::now() + period;
    let mut drained = Vec::new();
    while let Ok(event) = received.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        drained.push(event);
    }
    drained
}

// ---- one active repository (SPEC §4 Low-resource operation, rule 6) ----------

/// "Only the active repo has a live watcher and session." Phase 0 shows one
/// repository, so opening another closes the one before: its id stops
/// working and its watcher stops reporting.
#[test]
fn opening_another_repository_closes_the_previous_one() {
    let app = TestApp::with_machine_git();
    let (a, b) = (Fixture::new(), Fixture::new());
    let received = record_repo_changed(&app);
    let a_id = app.open(&a.workdir())["status"]["repo"]["id"].clone();

    let b_id = app.open(&b.workdir())["status"]["repo"]["id"].clone();

    let error = app
        .invoke("get_status", json!({ "repoId": a_id }))
        .unwrap_err();
    assert_eq!(error["kind"], "unknown_repo", "{error:#}");
    // Whatever A's watcher had already queued may still trickle in.
    drain(&received, quiet());
    std::fs::write(a.workdir().join("after-close.txt"), "x\n").unwrap();
    let late: Vec<Value> = drain(&received, quiet())
        .into_iter()
        .filter(|event| event["repo_id"] == a_id)
        .collect();
    assert_eq!(late, Vec::<Value>::new(), "A is no longer watched");
    // B is the open one.
    let status = app.invoke("get_status", json!({ "repoId": b_id })).unwrap();
    assert_eq!(status["repo"]["id"], b_id);

    // Reopening A works, with a new id, and sees what changed meanwhile.
    let reopened = app.open(&a.workdir());
    assert_eq!(reopened["status"]["repo"]["path"], a.canonical_workdir());
    assert!(
        entry(&reopened["status"], "after-close.txt").is_some(),
        "{reopened:#}"
    );
    let error = app
        .invoke("get_status", json!({ "repoId": b_id }))
        .unwrap_err();
    assert_eq!(error["kind"], "unknown_repo", "B was closed in turn");
}

/// The repository the user is looking at stays open when opening another
/// one fails.
#[test]
fn a_failed_open_keeps_the_open_repository() {
    let app = TestApp::with_machine_git();
    let fixture = Fixture::new();
    let id = app.open(&fixture.workdir())["status"]["repo"]["id"].clone();
    let plain = tempfile::tempdir().unwrap();

    app.open_error(plain.path());

    let status = app.invoke("get_status", json!({ "repoId": id })).unwrap();
    assert_eq!(status["repo"]["id"], id);
}
