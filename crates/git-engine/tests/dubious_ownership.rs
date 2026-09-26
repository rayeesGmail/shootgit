#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-12 (P0-09 follow-up): a repository git refuses to work in because it
//! is owned by another user (`safe.directory`, CVE-2022-24765) fails with
//! [`GitError::DubiousOwnership`], which names the fix, instead of a raw
//! stderr dump. The engine never runs the fix itself.
//!
//! git has a knob for exactly this: `GIT_TEST_ASSUME_DIFFERENT_OWNER=1`
//! makes its ownership check fail. The variable has to reach the git that
//! `status` spawns, which inherits this process's environment, so the test
//! sets it on the process. That is only safe while nothing else runs
//! concurrently, which is why this file holds exactly one test: every
//! integration-test file is its own process.

use std::path::PathBuf;
use std::process::Command;

use git_engine::error::GitError;
use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::CancellationToken;
use git_engine::repo::{open_repo, safe_directory_command};
use git_engine::status::{status, StatusOptions};

#[tokio::test]
async fn a_repository_owned_by_someone_else_fails_with_the_safe_directory_fix() {
    let dir = tempfile::tempdir().unwrap();
    let global_config = dir.path().join("gitconfig");
    std::fs::write(&global_config, "").unwrap();
    let workdir = dir.path().join("theirs");
    std::fs::create_dir(&workdir).unwrap();

    let git: PathBuf = resolve(&ResolveOptions::from_env(None)).await.unwrap().path;
    let init = Command::new(&git)
        .current_dir(&workdir)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");

    // Before any thread of ours spawns anything (see the module docs). A
    // `safe.directory = *` in the developer's own config would defeat the
    // check, so neither the global nor the system config is read.
    std::env::set_var("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1");
    std::env::set_var("GIT_CONFIG_GLOBAL", &global_config);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    // A localised git must still be recognised: the engine forces the C
    // locale on the spawn whose stderr it classifies.
    std::env::set_var("LANGUAGE", "de");
    std::env::set_var("LC_ALL", "de_DE.UTF-8");

    // Discovery reads the file system only, so it succeeds; git is the one
    // that refuses.
    let repo = open_repo(&git, &workdir).unwrap();
    let error = status(&repo, &StatusOptions::default(), &CancellationToken::new())
        .await
        .unwrap_err();

    let GitError::DubiousOwnership { ref path } = error else {
        panic!("expected DubiousOwnership, got {error:?}");
    };
    assert_eq!(path, repo.workdir());
    let fix = safe_directory_command(repo.workdir());
    assert!(
        fix.starts_with("git config --global --add safe.directory "),
        "{fix}"
    );
    let message = error.to_string();
    assert!(
        message.contains(&fix),
        "the message names the fix: {message}"
    );
}
