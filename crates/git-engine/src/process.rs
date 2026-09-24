//! Child processes. Every process `git-engine` starts goes through this module
//! (CLAUDE.md, SPEC §4 "Process model").
//!
//! For now it holds only what the git binary resolver needs (P0-05): run a
//! program with stdin closed, capture stdout and stderr as bytes, and kill it
//! if it has not exited within a timeout. P0-06 grows this into the generic
//! process builder and `GitCommand`, P0-07 adds the macOS login-shell PATH and
//! P0-17 the shared concurrency limiter.

use std::io;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

/// `CREATE_NO_WINDOW`: a console program started by a GUI app must not flash
/// a console window (SPEC §9).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Why [`output`] returned no output.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    /// The program could not be started, or waiting for it failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The program was still running when the timeout passed, so it was
    /// killed.
    #[error("did not finish within {0:?}")]
    TimedOut(Duration),
}

/// Runs `program` with `args` and returns its exit status, stdout and stderr.
///
/// Stdin is closed so a program waiting for input sees EOF instead of hanging.
/// A non-zero exit is not an error here; callers read `Output::status`.
pub(crate) async fn output(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, ProcessError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let child = command.spawn()?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => Ok(output?),
        // The unfinished wait owned the child; dropping it killed the child
        // (`kill_on_drop`), so nothing is left running.
        Err(_elapsed) => Err(ProcessError::TimedOut(timeout)),
    }
}
