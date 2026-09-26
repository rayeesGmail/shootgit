#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-08: `git_engine::status` against real repositories (SPEC §5 Status).
//!
//! Most tests run on the `scripts/fixtures/basic.sh` fixture, which has one
//! or more entries of every kind porcelain v2 reports: ordinary, renamed or
//! copied, unmerged, untracked and ignored. Each test builds its own copy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use git_engine::error::GitError;
use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::CancellationToken;
use git_engine::repo::Repo;
use git_engine::status::{
    status, AheadBehind, FileStatus, Head, Status, StatusEntry, StatusOptions,
};
use tempfile::TempDir;

async fn machine_git() -> PathBuf {
    resolve(&ResolveOptions::from_env(None)).await.unwrap().path
}

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

/// Builds the basic fixture and runs status on it. The `TempDir` keeps the
/// repository alive for the test.
async fn basic_status(options: StatusOptions) -> (TempDir, Repo, Status) {
    let dir = fixture("basic");
    let repo = Repo::new(machine_git().await, dir.path());
    let status = status(&repo, &options, &CancellationToken::new())
        .await
        .unwrap();
    (dir, repo, status)
}

fn find<'a>(status: &'a Status, path: &str) -> &'a StatusEntry {
    status
        .entries
        .iter()
        .find(|entry| entry.path == Path::new(path))
        .unwrap_or_else(|| panic!("no entry for {path:?} in {:#?}", status.entries))
}

fn tracked(path: &str, index: FileStatus, worktree: FileStatus) -> StatusEntry {
    StatusEntry {
        path: PathBuf::from(path),
        old_path: None,
        index_status: index,
        worktree_status: worktree,
        is_conflicted: false,
        is_submodule: false,
    }
}

fn moved(path: &str, old_path: &str, index: FileStatus) -> StatusEntry {
    StatusEntry {
        old_path: Some(PathBuf::from(old_path)),
        ..tracked(path, index, FileStatus::Unmodified)
    }
}

fn unmerged(path: &str, ours: FileStatus, theirs: FileStatus) -> StatusEntry {
    StatusEntry {
        is_conflicted: true,
        ..tracked(path, ours, theirs)
    }
}

fn untracked(path: &str) -> StatusEntry {
    tracked(path, FileStatus::Untracked, FileStatus::Untracked)
}

fn ignored(path: &str) -> StatusEntry {
    tracked(path, FileStatus::Ignored, FileStatus::Ignored)
}

fn assert_entries(status: &Status, expected: &[StatusEntry]) {
    for want in expected {
        let path = want.path.to_str().unwrap();
        assert_eq!(find(status, path), want, "entry for {path:?}");
    }
}

// ---- RepoInfo --------------------------------------------------------------

#[tokio::test]
async fn head_upstream_and_ahead_behind_come_from_the_branch_headers() {
    let (dir, repo, status) = basic_status(StatusOptions::default()).await;

    let head = Command::new(repo.git())
        .current_dir(dir.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let oid = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    assert_eq!(status.repo.path, dir.path());
    assert_eq!(
        status.repo.head,
        Head::Branch {
            name: "main".to_owned(),
            oid,
        }
    );
    assert_eq!(status.repo.upstream.as_deref(), Some("origin/main"));
    assert_eq!(
        status.repo.ahead_behind,
        Some(AheadBehind {
            ahead: 1,
            behind: 1
        })
    );
}

#[tokio::test]
async fn a_fresh_repository_is_on_an_unborn_branch_with_nothing_to_report() {
    let dir = tempfile::tempdir().unwrap();
    let git = machine_git().await;
    let init = Command::new(&git)
        .current_dir(dir.path())
        .args(["init", "-q", "--initial-branch=trunk"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");

    let repo = Repo::new(git, dir.path());
    let status = status(&repo, &StatusOptions::default(), &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        status.repo.head,
        Head::Unborn {
            name: "trunk".to_owned()
        }
    );
    assert_eq!(status.repo.upstream, None);
    assert_eq!(status.repo.ahead_behind, None);
    assert_eq!(status.entries, []);
}

// ---- entry kinds -----------------------------------------------------------

#[tokio::test]
async fn ordinary_entries_carry_index_and_worktree_status() {
    let (_dir, _repo, status) = basic_status(StatusOptions::default()).await;

    use FileStatus::{Added, Deleted, Modified, Unmodified};
    assert_entries(
        &status,
        &[
            tracked("modified.txt", Unmodified, Modified),
            tracked("staged.txt", Modified, Unmodified),
            tracked("added.txt", Added, Unmodified),
            tracked("deleted.txt", Unmodified, Deleted),
            tracked("source.txt", Modified, Unmodified),
        ],
    );
}

#[tokio::test]
async fn renames_keep_both_paths_with_spaces_and_unicode() {
    let (_dir, _repo, status) = basic_status(StatusOptions::default()).await;

    assert_entries(
        &status,
        &[
            moved("new name.txt", "old name.txt", FileStatus::Renamed),
            moved("ünïcødé/日本語.txt", "café.txt", FileStatus::Renamed),
        ],
    );
    // The sources of the renames are not reported on their own.
    for gone in ["old name.txt", "café.txt"] {
        assert!(
            status.entries.iter().all(|e| e.path != Path::new(gone)),
            "{gone:?} listed separately"
        );
    }
}

#[tokio::test]
async fn copies_keep_their_source_path() {
    let (_dir, _repo, status) = basic_status(StatusOptions::default()).await;

    assert_entries(
        &status,
        &[moved("copy.txt", "source.txt", FileStatus::Copied)],
    );
}

#[tokio::test]
async fn unmerged_entries_are_conflicted() {
    let (_dir, _repo, status) = basic_status(StatusOptions::default()).await;

    use FileStatus::{Added, Deleted, Unmerged};
    assert_entries(
        &status,
        &[
            unmerged("both-modified.txt", Unmerged, Unmerged),
            unmerged("both-added.txt", Added, Added),
            unmerged("deleted-by-them.txt", Unmerged, Deleted),
        ],
    );
    let conflicted: BTreeSet<_> = status
        .entries
        .iter()
        .filter(|e| e.is_conflicted)
        .map(|e| e.path.clone())
        .collect();
    assert_eq!(conflicted.len(), 3, "{conflicted:?}");
}

#[tokio::test]
async fn untracked_files_are_listed_one_by_one() {
    let (_dir, _repo, status) = basic_status(StatusOptions::default()).await;

    assert_entries(
        &status,
        &[
            untracked("untracked.txt"),
            untracked("untracked dir/ñested.txt"),
        ],
    );
}

#[tokio::test]
async fn ignored_entries_appear_only_when_asked_for() {
    let (_dir, _repo, without) = basic_status(StatusOptions::default()).await;
    assert!(
        without
            .entries
            .iter()
            .all(|e| e.index_status != FileStatus::Ignored),
        "{:#?}",
        without.entries
    );

    let (_dir, _repo, with) = basic_status(StatusOptions {
        include_ignored: true,
    })
    .await;
    // A directory that matches an ignore pattern is one entry, not a list
    // of everything inside it.
    assert_entries(&with, &[ignored("debug.log"), ignored("build/")]);
    let ignored_count = with
        .entries
        .iter()
        .filter(|e| e.index_status == FileStatus::Ignored)
        .count();
    assert_eq!(ignored_count, 2, "{:#?}", with.entries);
}

#[tokio::test]
async fn every_entry_is_reported_exactly_once() {
    let (_dir, _repo, status) = basic_status(StatusOptions {
        include_ignored: true,
    })
    .await;

    let paths: Vec<&str> = status
        .entries
        .iter()
        .map(|e| e.path.to_str().unwrap())
        .collect();
    let unique: BTreeSet<&str> = paths.iter().copied().collect();
    assert_eq!(unique.len(), paths.len(), "duplicates in {paths:?}");
    let expected: BTreeSet<&str> = [
        "added.txt",
        "both-added.txt",
        "both-modified.txt",
        "build/",
        "copy.txt",
        "debug.log",
        "deleted-by-them.txt",
        "deleted.txt",
        "modified.txt",
        "new name.txt",
        "source.txt",
        "staged.txt",
        "untracked dir/ñested.txt",
        "untracked.txt",
        "ünïcødé/日本語.txt",
    ]
    .into_iter()
    .collect();
    assert_eq!(unique, expected);
}

// ---- cancellation ----------------------------------------------------------

#[tokio::test]
async fn a_cancelled_status_returns_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::new(machine_git().await, dir.path());
    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = status(&repo, &StatusOptions::default(), &cancel)
        .await
        .unwrap_err();

    assert!(matches!(err, GitError::Cancelled), "{err:?}");
}

// ---- the IPC shape (P0-12) ---------------------------------------------------

#[tokio::test]
async fn repo_info_carries_the_id_of_the_repository_handle() {
    let (_dir, repo, status) = basic_status(StatusOptions::default()).await;

    assert_eq!(status.repo.id, repo.id());
}

/// SPEC §5 Core models are "serde, shared with TS via specta": the JSON
/// here is what the frontend receives from `get_status`, so its field names
/// and tags are the contract `packages/ipc-types/bindings.ts` describes.
#[tokio::test]
async fn status_serialises_to_the_shape_the_frontend_reads() {
    let (_dir, repo, status) = basic_status(StatusOptions::default()).await;

    let json = serde_json::to_value(&status).unwrap();

    let info = &json["repo"];
    assert_eq!(info["id"], serde_json::json!(repo.id()));
    assert_eq!(info["path"], repo.workdir().to_str().unwrap());
    assert_eq!(info["head"]["kind"], "branch");
    assert_eq!(info["head"]["name"], "main");
    assert!(info["head"]["oid"].is_string());
    assert_eq!(info["upstream"], "origin/main");
    assert_eq!(
        info["ahead_behind"],
        serde_json::json!({ "ahead": 1, "behind": 1 })
    );

    let entries = json["entries"].as_array().unwrap();
    let entry = |path: &str| {
        entries
            .iter()
            .find(|e| e["path"] == path)
            .unwrap_or_else(|| panic!("no {path:?} in {entries:#?}"))
    };
    assert_eq!(
        entry("new name.txt"),
        &serde_json::json!({
            "path": "new name.txt",
            "old_path": "old name.txt",
            "index_status": "renamed",
            "worktree_status": "unmodified",
            "is_conflicted": false,
            "is_submodule": false,
        })
    );
    assert_eq!(entry("modified.txt")["old_path"], serde_json::Value::Null);
    assert_eq!(entry("both-modified.txt")["index_status"], "unmerged");
    assert_eq!(entry("both-modified.txt")["is_conflicted"], true);
    assert_eq!(entry("untracked.txt")["worktree_status"], "untracked");
    assert_eq!(entry("ünïcødé/日本語.txt")["old_path"], "café.txt");
}
