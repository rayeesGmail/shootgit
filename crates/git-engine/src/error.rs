//! The engine's error type.

use crate::git_binary::GitBinaryError;
use crate::process::ProcessError;

/// Every fallible public operation in `git-engine` returns this.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitError {
    /// No usable git executable could be resolved.
    #[error(transparent)]
    Binary(#[from] GitBinaryError),
    /// git could not be started, its output could not be read, or it ran
    /// past its timeout and was killed.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// git ran and exited unsuccessfully.
    ///
    /// `args` are the arguments the caller passed, without the flags
    /// [`GitCommand`](crate::process::GitCommand) always adds or the
    /// per-spawn `-c` list (which may carry helper paths). `exit_code` is
    /// `None` when git was ended by a signal.
    #[error("`git {}` failed ({}): {}", args.join(" "), describe_exit(*exit_code), stderr.trim())]
    Failed {
        args: Vec<String>,
        exit_code: Option<i32>,
        stderr: String,
    },
    /// The command's `CancellationToken` fired: either while it waited for a
    /// git slot (nothing was spawned) or while git ran (git and everything
    /// it started were killed). A superseded read ends this way; callers
    /// normally drop the error rather than show it.
    #[error("cancelled")]
    Cancelled,
    /// git succeeded but printed something that does not have the documented
    /// machine-readable shape. `command` is the git subcommand, `reason` what
    /// was wrong with its output.
    #[error("unexpected output from `git {command}`: {reason}")]
    UnexpectedOutput {
        command: &'static str,
        reason: String,
    },
}

fn describe_exit(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "terminated by a signal".to_owned(),
    }
}
