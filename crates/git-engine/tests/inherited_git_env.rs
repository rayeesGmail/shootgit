#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-09 (P0-06 follow-up): git's repository-local environment variables
//! that this process inherited do not reach the git it spawns (ADR 0007).
//!
//! An app started from a git hook, or from a shell that exported `GIT_DIR`,
//! inherits variables such as `GIT_DIR`, `GIT_WORK_TREE` and
//! `GIT_INDEX_FILE`. Passed on, they would make every git command operate on
//! that repository instead of the one `open_repo` found.
//!
//! This is the only test in its binary because it sets environment
//! variables for the whole process.

use std::path::Path;
use std::process::Command;

use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::CancellationToken;
use git_engine::repo::open_repo;
use git_engine::status::{status, FileStatus, StatusOptions};

fn init(git: &Path, dir: &Path, untracked: &str) {
    let output = Command::new(git)
        .current_dir(dir)
        .args(["init", "-q", "--initial-branch=main"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::write(dir.join(untracked), "x\n").unwrap();
}

#[tokio::test]
async fn inherited_repository_variables_do_not_redirect_git() {
    let git = resolve(&ResolveOptions::from_env(None)).await.unwrap().path;
    let decoy = tempfile::tempdir().unwrap();
    let real = tempfile::tempdir().unwrap();
    init(&git, decoy.path(), "decoy.txt");
    init(&git, real.path(), "real.txt");
    let repo = open_repo(&git, real.path()).unwrap();

    // Set only now, so the setup above was not redirected. Nothing else in
    // this binary runs concurrently.
    std::env::set_var("GIT_DIR", decoy.path().join(".git"));
    std::env::set_var("GIT_WORK_TREE", decoy.path());
    std::env::set_var("GIT_INDEX_FILE", decoy.path().join(".git").join("index"));
    std::env::set_var("GIT_COMMON_DIR", decoy.path().join(".git"));

    let status = status(&repo, &StatusOptions::default(), &CancellationToken::new())
        .await
        .unwrap();

    let paths: Vec<_> = status.entries.iter().map(|e| e.path.clone()).collect();
    assert_eq!(paths, [Path::new("real.txt")], "{status:#?}");
    assert_eq!(status.entries[0].index_status, FileStatus::Untracked);
}
