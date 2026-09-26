#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-13: the `git-engine-cli` binary end to end, against real repositories.
//!
//! `status` runs on the `scripts/fixtures/basic.sh` fixture (every entry kind
//! porcelain v2 reports, P0-08); `watch` drives a real OS watcher, so its
//! timing is generous, as in git-engine's own watcher tests.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_git-engine-cli");

/// How long the watcher may take to report a change, or to start. Far above
/// the 300 ms budget so a loaded CI runner does not fail the test.
const ARRIVAL: Duration = Duration::from_secs(10);

/// No output for this long means the watcher has nothing more to report
/// about changes made before it started (FSEvents delivers some).
const QUIET: Duration = Duration::from_secs(1);

/// Runs `scripts/fixtures/<name>.sh` with bash into a new temp dir.
fn fixture(name: &str) -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    let script = root
        .join("scripts")
        .join("fixtures")
        .join(format!("{name}.sh"));
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new("bash")
        // With PATH set on the child, Rust looks for `bash` on it before
        // System32 on Windows, where bash.exe is the WSL launcher; CI runs
        // the tests from Git Bash, whose bash comes first on PATH.
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .arg(&script)
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} failed ({}):\n{}{}",
        script.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    dir
}

fn cli<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(CLI).args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The single JSON document `status` prints, on one line.
fn status_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "status failed ({}): {}",
        output.status,
        stderr(output)
    );
    let text = stdout(output);
    assert_eq!(
        text.lines().count(),
        1,
        "status prints one line of JSON: {text}"
    );
    assert!(text.ends_with('\n'), "the line is terminated: {text:?}");
    serde_json::from_str(&text).unwrap()
}

fn entry<'a>(status: &'a Value, path: &str) -> &'a Value {
    status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap_or_else(|| panic!("no entry for {path:?} in {status:#}"))
}

fn has_entry(status: &Value, path: &str) -> bool {
    status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == path)
}

fn same_dir(reported: &Value, expected: &Path) -> bool {
    let reported = PathBuf::from(reported.as_str().unwrap());
    fs::canonicalize(reported).unwrap() == fs::canonicalize(expected).unwrap()
}

// ---- status ----------------------------------------------------------------

#[test]
fn status_prints_the_repository_and_every_entry_as_one_json_object() {
    let dir = fixture("basic");

    let output = cli([Path::new("status"), dir.path()]);

    let status = status_json(&output);
    assert!(stderr(&output).is_empty(), "stderr: {}", stderr(&output));
    let repo = &status["repo"];
    assert!(repo["id"].is_u64(), "repo.id is a number: {repo}");
    assert!(same_dir(&repo["path"], dir.path()), "repo.path: {repo}");
    assert_eq!(repo["head"]["kind"], "branch");
    assert_eq!(repo["head"]["name"], "main");
    assert_eq!(repo["head"]["oid"].as_str().unwrap().len(), 40);
    assert_eq!(repo["upstream"], "origin/main");
    assert_eq!(
        repo["ahead_behind"],
        serde_json::json!({ "ahead": 1, "behind": 1 })
    );

    let modified = entry(&status, "modified.txt");
    assert_eq!(modified["index_status"], "unmodified");
    assert_eq!(modified["worktree_status"], "modified");
    assert_eq!(modified["old_path"], Value::Null);
    assert_eq!(modified["is_conflicted"], false);
    assert_eq!(modified["is_submodule"], false);

    let renamed = entry(&status, "new name.txt");
    assert_eq!(renamed["index_status"], "renamed");
    assert_eq!(renamed["old_path"], "old name.txt");
    assert_eq!(entry(&status, "ünïcødé/日本語.txt")["old_path"], "café.txt");
    assert_eq!(entry(&status, "copy.txt")["index_status"], "copied");

    let conflicted = entry(&status, "both-modified.txt");
    assert_eq!(conflicted["is_conflicted"], true);
    assert_eq!(conflicted["index_status"], "unmerged");

    assert_eq!(
        entry(&status, "untracked dir/ñested.txt")["worktree_status"],
        "untracked"
    );
    assert!(
        !has_entry(&status, "debug.log"),
        "ignored paths are left out without --ignored"
    );
}

#[test]
fn status_with_ignored_also_lists_ignored_paths() {
    let dir = fixture("basic");

    let status = status_json(&cli([
        Path::new("status"),
        Path::new("--ignored"),
        dir.path(),
    ]));

    assert_eq!(entry(&status, "debug.log")["index_status"], "ignored");
    assert_eq!(entry(&status, "build/")["worktree_status"], "ignored");
}

#[test]
fn status_from_a_subdirectory_reports_the_repository_root() {
    let dir = fixture("basic");

    let status = status_json(&cli([
        Path::new("status"),
        &dir.path().join("untracked dir"),
    ]));

    assert!(same_dir(&status["repo"]["path"], dir.path()));
    // Entry paths stay relative to the root, not to the directory given.
    entry(&status, "modified.txt");
}

#[test]
fn status_outside_a_repository_fails_with_the_reason_on_stderr() {
    let dir = tempfile::tempdir().unwrap();

    let output = cli([Path::new("status"), dir.path()]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("not a git repository"),
        "{}",
        stderr(&output)
    );
}

// ---- usage -----------------------------------------------------------------

#[test]
fn no_command_is_a_usage_error() {
    let output = cli::<[&str; 0], &str>([]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("usage:"), "{}", stderr(&output));
}

#[test]
fn an_unknown_command_is_a_usage_error_that_names_it() {
    let output = cli(["frobnicate", "."]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("frobnicate"),
        "{}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("usage:"), "{}", stderr(&output));
}

#[test]
fn help_prints_the_usage_to_stdout() {
    let output = cli(["--help"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("usage:"));
    assert!(stdout(&output).contains("status"));
    assert!(stdout(&output).contains("watch"));
}

// ---- watch -----------------------------------------------------------------

/// A running `watch`, killed when the test ends, pass or fail.
struct Watching {
    child: Child,
    lines: Receiver<String>,
}

impl Watching {
    /// Starts `watch <dir>` and waits for its notice on stderr that the
    /// watch is in place.
    fn start(dir: &Path) -> Self {
        let mut child = Command::new(CLI)
            .arg("watch")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = read_lines(child.stdout.take().unwrap());
        let notices = read_lines(child.stderr.take().unwrap());
        let mut watching = Self { child, lines };
        match notices.recv_timeout(ARRIVAL) {
            Ok(notice) => assert!(notice.starts_with("watching "), "notice: {notice}"),
            Err(error) => panic!(
                "no notice from watch within {ARRIVAL:?} ({error}); exit: {:?}",
                watching.child.try_wait()
            ),
        }
        watching
    }

    /// Drops what the watcher reports for changes made before it started,
    /// until nothing arrives for [`QUIET`].
    fn settle(&self) {
        loop {
            match self.lines.recv_timeout(QUIET) {
                Ok(line) => eprintln!("drained a pre-watch line: {line}"),
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => panic!("watch exited while settling"),
            }
        }
    }

    fn next_line(&self) -> String {
        let started = Instant::now();
        let line = self
            .lines
            .recv_timeout(ARRIVAL)
            .unwrap_or_else(|error| panic!("no line from watch within {ARRIVAL:?}: {error}"));
        eprintln!("watch reported after {:?}: {line}", started.elapsed());
        line
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Forwards each line `pipe` produces to the returned channel.
fn read_lines(pipe: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (lines, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            let Ok(line) = line else { return };
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    receiver
}

#[test]
fn watch_prints_one_json_line_per_change() {
    let dir = fixture("basic");
    let watching = Watching::start(dir.path());
    watching.settle();

    fs::write(dir.path().join("untracked.txt"), "changed\n").unwrap();

    let line = watching.next_line();
    let changed: Value = serde_json::from_str(&line).unwrap();
    // Which kinds a change produces is the watcher's business (git-engine's
    // tests/watcher.rs); here it is enough that the batch names it.
    assert!(
        changed["kinds"]
            .as_array()
            .unwrap()
            .contains(&Value::from("status")),
        "a working-tree edit is a status change: {line}"
    );
    assert_eq!(changed["generation"], 0, "the CLI makes no writes: {line}");
}

#[test]
fn watch_outside_a_repository_fails_with_the_reason_on_stderr() {
    let dir = tempfile::tempdir().unwrap();

    let output = cli([Path::new("watch"), dir.path()]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("not a git repository"),
        "{}",
        stderr(&output)
    );
}
