#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-07: the app-wide login-shell `PATH`, against the real `$SHELL`.
//!
//! Its own test binary, because `login_shell::init` sets process-wide state
//! once. On macOS this runs the machine's login shell (the CI macOS runner's
//! included); elsewhere `init` must do nothing.

use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::login_shell;
use git_engine::process;

#[cfg(target_os = "macos")]
#[tokio::test]
async fn init_resolves_the_login_shell_path_once_and_every_spawn_uses_it() {
    use git_engine::login_shell::PathSource;
    use git_engine::process::ProcessCommand;

    let resolved = login_shell::init().await.expect("macOS resolves a PATH");
    assert_eq!(
        resolved.source,
        PathSource::LoginShell,
        "$SHELL = {:?} did not print a PATH",
        std::env::var_os("SHELL")
    );
    assert!(std::env::split_paths(&resolved.path).any(|dir| dir.is_absolute()));
    assert_eq!(process::login_shell_path(), Some(resolved.path.as_os_str()));

    // Cached: a second call returns the same value and spawns nothing.
    let spawns = process::spawn_count();
    let again = login_shell::init().await.unwrap();
    assert!(std::ptr::eq(resolved, again));
    assert_eq!(process::spawn_count(), spawns);

    // git is searched for on it...
    let options = ResolveOptions::from_login_shell_env(None).await;
    assert_eq!(options.search_path.as_ref(), Some(&resolved.path));
    let git = resolve(&options).await.unwrap();
    assert!(git.path.is_absolute());

    // ...and children receive it.
    let mut printenv = ProcessCommand::new("/bin/sh");
    printenv.args(["-c", "printf %s \"$PATH\""]);
    let output = printenv.output().await.unwrap();
    assert_eq!(output.stdout, resolved.path.as_encoded_bytes());
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn init_does_nothing_off_macos() {
    let spawns = process::spawn_count();

    assert_eq!(login_shell::init().await, None);

    assert_eq!(process::login_shell_path(), None);
    assert_eq!(process::spawn_count(), spawns);
    let options = ResolveOptions::from_login_shell_env(None).await;
    assert_eq!(options.search_path, std::env::var_os("PATH"));
    resolve(&options).await.unwrap();
}
