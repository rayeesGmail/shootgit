//! Dev CLI over git-engine for manual testing and debugging (SPEC §4 crate
//! table, P0-13).
//!
//! ```text
//! git-engine-cli status [--ignored] <path>
//! git-engine-cli watch <path>
//! ```
//!
//! Both commands open the repository that contains `<path>` the way the app
//! does ([`open_repo`]: from any directory inside it, following a linked
//! worktree's `.git` file) and print the engine's own serde models as JSON,
//! one document per line, so the output reads with `jq` and parses line by
//! line:
//!
//! - `status` prints one [`Status`](git_engine::status::Status): the
//!   `repo` (`RepoInfo`) and its `entries`, exactly as the app receives it
//!   over IPC.
//! - `watch` prints one [`RepoChanged`] per coalesced batch
//!   (`{"kinds":["status"],"generation":0}`), flushed as it arrives, until
//!   the process is interrupted (Ctrl+C ends it; nothing needs cleaning up).
//!   Once the watch is in place it says so on stderr (`watching <root> ...`),
//!   so a script knows when changes start to count.
//!
//! Errors go to stderr as `error: ...` with their causes, and the exit code
//! is 1; a command line that does not parse exits with 2. A pipeline reader
//! that stops early (`watch . | head -1`) ends the command quietly.
//!
//! Logic lives here rather than in `main.rs`, so it follows the library
//! rules (no `unwrap`, `thiserror` errors) and is unit-tested.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use git_engine::error::GitError;
use git_engine::git_binary::{self, ResolveOptions};
use git_engine::process::CancellationToken;
use git_engine::repo::{open_repo, Repo};
use git_engine::repo_actor::RepoActor;
use git_engine::status::{self, StatusOptions};
use git_engine::watcher::{RepoChanged, WatchError, WatchOptions};
use tokio::runtime::Runtime;

/// The help text, without a trailing newline.
pub const USAGE: &str = "\
usage: git-engine-cli status [--ignored] <path>
       git-engine-cli watch <path>
       git-engine-cli help

  status  Print the status of the repository that contains <path> as one
          line of JSON. --ignored also lists ignored paths.
  watch   Print one line of JSON per batch of changes to the repository that
          contains <path>, until interrupted (Ctrl+C).
  help    Print this help (also -h, --help).";

/// The one option `status` takes.
const IGNORED: &str = "--ignored";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print [`USAGE`].
    Help,
    /// Print the status of the repository that contains `path`.
    Status {
        path: PathBuf,
        include_ignored: bool,
    },
    /// Print the changes to the repository that contains `path` as they
    /// happen.
    Watch { path: PathBuf },
}

/// A command line that does not say what to do. The caller prints it with
/// [`USAGE`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(String);

/// Why a command failed.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("could not start the async runtime")]
    Runtime(#[source] io::Error),
    /// Resolving git, finding the repository or running git failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The watcher stopped; this is the last item of its stream.
    #[error(transparent)]
    Watch(#[from] WatchError),
    #[error("could not encode the output as JSON")]
    Json(#[from] serde_json::Error),
    #[error("could not write to stdout")]
    Output(#[source] io::Error),
}

impl CliError {
    /// Whether whoever reads stdout has gone (`watch . | head -1`): the
    /// normal end of a pipeline, not a failure worth reporting.
    pub fn is_closed_output(&self) -> bool {
        matches!(self, CliError::Output(error) if error.kind() == io::ErrorKind::BrokenPipe)
    }
}

/// Parses the arguments after the program name.
///
/// Arguments are `OsString`s so a path that is not UTF-8 reaches the engine
/// unchanged. `-h` or `--help` anywhere before `--` asks for help; `--`
/// ends the options, so a path may start with `-`.
pub fn parse_args<I>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let Some(name) = args.next() else {
        return Err(UsageError("no command given".to_owned()));
    };
    let rest: Vec<OsString> = args.collect();
    let asks_for_help = rest
        .iter()
        .take_while(|arg| arg.as_os_str() != "--")
        .any(|arg| is_help(arg));
    if name == "help" || is_help(&name) || asks_for_help {
        return Ok(Command::Help);
    }
    match name.to_str() {
        Some("status") => {
            let (path, flags) = path_and_flags("status", rest, &[IGNORED])?;
            Ok(Command::Status {
                path,
                include_ignored: flags.contains(&IGNORED),
            })
        }
        Some("watch") => {
            let (path, _) = path_and_flags("watch", rest, &[])?;
            Ok(Command::Watch { path })
        }
        _ => Err(UsageError(format!(
            "unknown command `{}`",
            name.to_string_lossy()
        ))),
    }
}

fn is_help(arg: &OsStr) -> bool {
    arg == "-h" || arg == "--help"
}

/// Whether `arg` is an option rather than a path: it starts with `-` and is
/// not `-` alone.
fn is_option(arg: &OsStr) -> bool {
    let bytes = arg.as_encoded_bytes();
    bytes.starts_with(b"-") && bytes != b"-"
}

/// The one `<path>` operand of `command`, and which of the options in
/// `known` were given, before or after it.
fn path_and_flags(
    command: &str,
    args: Vec<OsString>,
    known: &[&'static str],
) -> Result<(PathBuf, Vec<&'static str>), UsageError> {
    let mut path: Option<PathBuf> = None;
    let mut flags = Vec::new();
    let mut options_ended = false;
    for arg in args {
        if !options_ended && arg == "--" {
            options_ended = true;
        } else if !options_ended && is_option(&arg) {
            let flag = known
                .iter()
                .copied()
                .find(|flag| arg == *flag)
                .ok_or_else(|| {
                    UsageError(format!(
                        "unknown option `{}` for `{command}`",
                        arg.to_string_lossy()
                    ))
                })?;
            flags.push(flag);
        } else if let Some(first) = &path {
            return Err(UsageError(format!(
                "`{command}` takes one <path>, but `{}` follows `{}`",
                arg.to_string_lossy(),
                first.display()
            )));
        } else {
            path = Some(PathBuf::from(arg));
        }
    }
    let path = path.ok_or_else(|| UsageError(format!("`{command}` needs a <path>")))?;
    Ok((path, flags))
}

/// Runs `command`, writing its JSON to `out` and progress notices (the
/// `watch` start) to `notices`. `watch` returns only when it fails.
pub fn run(
    command: Command,
    out: &mut impl Write,
    notices: &mut impl Write,
) -> Result<(), CliError> {
    match command {
        Command::Help => writeln!(out, "{USAGE}").map_err(CliError::Output),
        Command::Status {
            path,
            include_ignored,
        } => {
            let runtime = runtime()?;
            let repo = open(&runtime, &path)?;
            let options = StatusOptions { include_ignored };
            let status =
                runtime.block_on(status::status(&repo, &options, &CancellationToken::new()))?;
            emit(out, &serde_json::to_string(&status)?)
        }
        Command::Watch { path } => {
            let runtime = runtime()?;
            let repo = open(&runtime, &path)?;
            runtime.block_on(watch(repo, out, notices))
        }
    }
}

/// The app's runtime, as the app sizes it (ADR 0004), so what the CLI
/// measures is what the app runs on.
fn runtime() -> Result<Runtime, CliError> {
    git_engine::runtime::build().map_err(CliError::Runtime)
}

/// Resolves git and opens the repository that contains `path`.
fn open(runtime: &Runtime, path: &Path) -> Result<Repo, CliError> {
    // The CLI runs from a terminal, whose PATH is already the login shell's,
    // so `from_env`: `from_login_shell_env` would start the user's shell and
    // run their rc files for nothing (ADR 0006 is about GUI launches).
    let options = ResolveOptions::from_env(None);
    let git = runtime
        .block_on(git_binary::resolve(&options))
        .map_err(GitError::from)?;
    // Discovery is blocking file-system work. Here it runs on the main
    // thread, outside the runtime, so it holds up no worker.
    Ok(open_repo(git.path, path)?)
}

/// Watches `repo` through a [`RepoActor`], which owns the watcher as in the
/// app, and prints every batch until the watcher fails.
async fn watch(repo: Repo, out: &mut impl Write, notices: &mut impl Write) -> Result<(), CliError> {
    // `actor` holds the watch: it lives to the end of this function, and
    // dropping it would stop the watcher and end `events`.
    let (actor, mut events) = RepoActor::spawn_watched(repo, WatchOptions::default()).await?;
    // Best effort: a closed stderr must not stop the watch.
    let _ = writeln!(
        notices,
        "watching {} (Ctrl+C to stop)",
        actor.repo().workdir().display()
    );
    while let Some(batch) = events.recv().await {
        let changed: RepoChanged = batch?;
        emit(out, &serde_json::to_string(&changed)?)?;
    }
    // The stream ends without an error only once every watcher handle is
    // gone, which cannot happen while `actor` holds one.
    Ok(())
}

/// Writes `json` as one line and flushes it, so a reader sees each document
/// as soon as it is printed.
fn emit(out: &mut impl Write, json: &str) -> Result<(), CliError> {
    writeln!(out, "{json}")
        .and_then(|()| out.flush())
        .map_err(CliError::Output)
}

/// `error` followed by each of its sources: "could not access /x: No such
/// file or directory". A source whose text the message already ends with
/// (a `transparent` variant repeats its source's) is skipped.
pub fn describe(error: &(dyn Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.ends_with(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "git-engine-cli");
    }

    fn parse(args: &[&str]) -> Result<Command, UsageError> {
        parse_args(args.iter().map(OsString::from))
    }

    fn message(args: &[&str]) -> String {
        parse(args).unwrap_err().to_string()
    }

    #[test]
    fn status_takes_a_path() {
        assert_eq!(
            parse(&["status", "some/repo"]),
            Ok(Command::Status {
                path: PathBuf::from("some/repo"),
                include_ignored: false,
            })
        );
    }

    #[test]
    fn status_takes_ignored_before_or_after_the_path() {
        let expected = Ok(Command::Status {
            path: PathBuf::from("repo"),
            include_ignored: true,
        });
        assert_eq!(parse(&["status", "--ignored", "repo"]), expected);
        assert_eq!(parse(&["status", "repo", "--ignored"]), expected);
    }

    #[test]
    fn watch_takes_a_path() {
        assert_eq!(
            parse(&["watch", "."]),
            Ok(Command::Watch {
                path: PathBuf::from("."),
            })
        );
    }

    #[test]
    fn help_is_a_command_and_a_flag_anywhere() {
        for args in [
            &["help"][..],
            &["-h"],
            &["--help"],
            &["status", "--help"],
            &["watch", "-h", "repo"],
        ] {
            assert_eq!(parse(args), Ok(Command::Help), "{args:?}");
        }
    }

    #[test]
    fn a_double_dash_ends_the_options() {
        assert_eq!(
            parse(&["status", "--", "--ignored"]),
            Ok(Command::Status {
                path: PathBuf::from("--ignored"),
                include_ignored: false,
            })
        );
        assert_eq!(
            parse(&["watch", "--", "--help"]),
            Ok(Command::Watch {
                path: PathBuf::from("--help"),
            })
        );
    }

    #[test]
    fn a_lone_dash_is_a_path() {
        assert_eq!(
            parse(&["watch", "-"]),
            Ok(Command::Watch {
                path: PathBuf::from("-"),
            })
        );
    }

    #[test]
    fn a_command_is_required() {
        assert!(message(&[]).contains("no command"), "{}", message(&[]));
    }

    #[test]
    fn an_unknown_command_is_named() {
        assert!(message(&["stauts", "."]).contains("`stauts`"));
    }

    #[test]
    fn a_path_is_required() {
        assert!(message(&["status"]).contains("<path>"));
        assert!(message(&["watch", "--"]).contains("<path>"));
    }

    #[test]
    fn only_one_path_is_taken() {
        assert!(message(&["status", "a", "b"]).contains("`b`"));
    }

    #[test]
    fn unknown_options_are_named() {
        assert!(message(&["watch", "--ignored", "."]).contains("`--ignored`"));
        assert!(message(&["status", "-x", "."]).contains("`-x`"));
    }

    /// Paths come from `args_os`, so one that is not UTF-8 reaches the
    /// engine unchanged.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_kept_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;

        let path = OsStr::from_bytes(b"caf\xe9");
        let args = [OsString::from("watch"), path.to_os_string()];
        assert_eq!(
            parse_args(args),
            Ok(Command::Watch {
                path: PathBuf::from(path),
            })
        );
    }

    #[test]
    fn help_goes_to_out() {
        let mut out = Vec::new();
        let mut notices = Vec::new();
        run(Command::Help, &mut out, &mut notices).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{USAGE}\n"));
        assert!(notices.is_empty());
    }

    #[test]
    fn a_closed_stdout_is_not_a_failure_but_other_write_errors_are() {
        let closed = CliError::Output(io::Error::from(io::ErrorKind::BrokenPipe));
        assert!(closed.is_closed_output());
        let full = CliError::Output(io::Error::from(io::ErrorKind::StorageFull));
        assert!(!full.is_closed_output());
    }

    #[test]
    fn an_error_is_described_with_its_causes_once_each() {
        let missing = GitError::Io {
            path: PathBuf::from("/x"),
            source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
        };
        assert_eq!(
            describe(&CliError::from(missing)),
            "could not access /x: no such file"
        );
    }
}
