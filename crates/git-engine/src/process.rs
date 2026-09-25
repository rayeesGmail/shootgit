//! Every process the engine starts goes through this module (§4 Process
//! model, CLAUDE.md "Process spawning").
//!
//! Two layers:
//!
//! - [`ProcessCommand`] is the generic builder for any tool (`git`, and in
//!   later tasks `ssh`, `ssh-add`, `ssh-keygen`). It spawns on `tokio`, sets
//!   `CREATE_NO_WINDOW` on Windows, applies the login-shell `PATH` once it has
//!   been resolved, enforces a timeout and captures stdout and stderr as
//!   bytes. It never interprets the exit status: `ssh -T git@github.com`
//!   exits 1 on success, so that is the caller's call.
//! - [`GitCommand`] wraps it for git: it always passes
//!   `--no-optional-locks -c core.quotepath=off`, sets
//!   `GIT_TERMINAL_PROMPT=0`, carries a per-spawn `-c` list and askpass
//!   injection points, and maps a non-zero exit to [`GitError::Failed`].
//!
//! [`split_nul`] splits `-z` output into fields.
//!
//! Dropping an `output()` future before it finishes kills the child
//! (`kill_on_drop`), so cancelling a read also stops its process.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::GitError;

/// Timeout applied when the caller does not choose one.
///
/// Long operations (clone, fetch, push, rebase) stream progress and should
/// raise it or pass `None`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// `CREATE_NO_WINDOW`: keeps console programs from flashing a window (§9).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The login-shell `PATH`, set once at startup (P0-07 resolves it on macOS).
static LOGIN_SHELL_PATH: OnceLock<OsString> = OnceLock::new();

/// Records the login-shell `PATH` that every spawn should use.
///
/// A macOS GUI app does not inherit the user's shell `PATH` (§4 Process
/// model), so the app resolves it once at startup and hands it over here.
/// Until it is set, children inherit this process's `PATH`. The value can be
/// set only once; a second call returns the rejected value.
pub fn set_login_shell_path(path: OsString) -> Result<(), OsString> {
    LOGIN_SHELL_PATH.set(path)
}

/// The login-shell `PATH`, if [`set_login_shell_path`] has been called.
pub fn login_shell_path() -> Option<&'static OsStr> {
    LOGIN_SHELL_PATH.get().map(OsString::as_os_str)
}

/// Why a process could not be run to completion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessError {
    #[error("could not start {program:?}")]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("I/O error while running {program:?}")]
    Io {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{program:?} did not finish within {timeout:?} and was killed")]
    Timeout { program: PathBuf, timeout: Duration },
}

/// What a finished process produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ProcessOutput {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// The exit code, or `None` if the process was ended by a signal.
    pub fn exit_code(&self) -> Option<i32> {
        self.status.code()
    }
}

/// Generic builder for spawning a non-interactive tool.
///
/// Stdin is closed (`/dev/null`); stdout and stderr are captured in full.
#[derive(Debug, Clone)]
pub struct ProcessCommand {
    program: PathBuf,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    timeout: Option<Duration>,
}

impl ProcessCommand {
    /// A command for `program`: an absolute path, or a bare name looked up on
    /// the `PATH` the child receives.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
            current_dir: None,
            timeout: Some(DEFAULT_TIMEOUT),
        }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Sets an environment variable for the child. Later calls for the same
    /// key win. Setting `PATH` here overrides the login-shell `PATH`.
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.envs
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub fn current_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
        self.current_dir = Some(dir.into());
        self
    }

    /// Kills the child and fails with [`ProcessError::Timeout`] if it runs
    /// longer than this. `None` disables the timeout. Defaults to
    /// [`DEFAULT_TIMEOUT`].
    pub fn timeout(&mut self, timeout: Option<Duration>) -> &mut Self {
        self.timeout = timeout;
        self
    }

    fn build(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = login_shell_path() {
            command.env("PATH", path);
        }
        for (key, value) in &self.envs {
            command.env(key, value);
        }
        if let Some(dir) = &self.current_dir {
            command.current_dir(dir);
        }
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }

    /// Runs the command to completion and returns what it printed.
    ///
    /// A non-zero exit is not an error here; see [`ProcessOutput::success`].
    pub async fn output(&self) -> Result<ProcessOutput, ProcessError> {
        let started = Instant::now();
        let mut child = self.build().spawn().map_err(|source| ProcessError::Spawn {
            program: self.program.clone(),
            source,
        })?;
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        let run = async {
            let (stdout, stderr, status) = tokio::try_join!(
                read_pipe(stdout.as_mut()),
                read_pipe(stderr.as_mut()),
                child.wait(),
            )?;
            Ok::<_, std::io::Error>(ProcessOutput {
                status,
                stdout,
                stderr,
            })
        };

        let result = match self.timeout {
            None => run.await,
            Some(timeout) => match tokio::time::timeout(timeout, run).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    if let Err(error) = child.kill().await {
                        tracing::warn!(program = ?self.program, %error, "could not kill timed-out process");
                    }
                    tracing::warn!(program = ?self.program, ?timeout, "process timed out");
                    return Err(ProcessError::Timeout {
                        program: self.program.clone(),
                        timeout,
                    });
                }
            },
        };

        let output = result.map_err(|source| ProcessError::Io {
            program: self.program.clone(),
            source,
        })?;
        tracing::debug!(
            program = ?self.program,
            exit_code = ?output.exit_code(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "process finished"
        );
        Ok(output)
    }
}

async fn read_pipe<R: AsyncRead + Unpin>(pipe: Option<&mut R>) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    if let Some(pipe) = pipe {
        pipe.read_to_end(&mut buffer).await?;
    }
    Ok(buffer)
}

/// Builder for one `git` invocation.
///
/// The spawned command line is
/// `git --no-optional-locks -c core.quotepath=off [-c key=value]... <args>`
/// with `GIT_TERMINAL_PROMPT=0`, so git never blocks on a terminal prompt.
///
/// Injection points reserved for later tasks:
/// - [`config`](Self::config): the per-spawn `-c` list, e.g.
///   `credential.helper=<path>` for hosts with a signed-in account (P4-13).
/// - [`git_askpass`](Self::git_askpass) / [`ssh_askpass`](Self::ssh_askpass)
///   and [`env`](Self::env): `GIT_ASKPASS`, `SSH_ASKPASS`,
///   `SSH_ASKPASS_REQUIRE` (P1-25).
#[derive(Debug, Clone)]
pub struct GitCommand {
    process: ProcessCommand,
    config: Vec<OsString>,
    args: Vec<OsString>,
}

/// Flags every git spawn gets, before any per-spawn `-c`.
const FIXED_GIT_ARGS: [&str; 3] = ["--no-optional-locks", "-c", "core.quotepath=off"];

impl GitCommand {
    /// A git command using the executable at `git` (normally
    /// [`GitBinary::path`](crate::git_binary::GitBinary)).
    pub fn new(git: impl Into<PathBuf>) -> Self {
        Self {
            process: ProcessCommand::new(git),
            config: Vec::new(),
            args: Vec::new(),
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Adds `-c key=value` for this spawn only. Nothing is written to any
    /// git config file (§7).
    pub fn config(&mut self, key: &str, value: impl AsRef<OsStr>) -> &mut Self {
        let mut pair = OsString::from(key);
        pair.push("=");
        pair.push(value.as_ref());
        self.config.push(pair);
        self
    }

    /// Sets an environment variable for this spawn. `GIT_TERMINAL_PROMPT`
    /// is always `0` and cannot be overridden.
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.process.env(key, value);
        self
    }

    /// Sets `GIT_ASKPASS` for this spawn.
    pub fn git_askpass(&mut self, program: impl AsRef<OsStr>) -> &mut Self {
        self.env("GIT_ASKPASS", program)
    }

    /// Sets `SSH_ASKPASS` for this spawn.
    pub fn ssh_askpass(&mut self, program: impl AsRef<OsStr>) -> &mut Self {
        self.env("SSH_ASKPASS", program)
    }

    pub fn current_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
        self.process.current_dir(dir);
        self
    }

    /// See [`ProcessCommand::timeout`].
    pub fn timeout(&mut self, timeout: Option<Duration>) -> &mut Self {
        self.process.timeout(timeout);
        self
    }

    /// The full argument list passed to git, fixed flags included.
    pub fn full_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = FIXED_GIT_ARGS.iter().map(OsString::from).collect();
        for pair in &self.config {
            args.push(OsString::from("-c"));
            args.push(pair.clone());
        }
        args.extend(self.args.iter().cloned());
        args
    }

    fn process(&self) -> ProcessCommand {
        let mut process = self.process.clone();
        process
            .args(self.full_args())
            .env("GIT_TERMINAL_PROMPT", "0");
        process
    }

    /// Runs git and returns its output, mapping a non-zero exit to
    /// [`GitError::Failed`].
    pub async fn output(&self) -> Result<ProcessOutput, GitError> {
        let output = self.output_unchecked().await?;
        if output.success() {
            return Ok(output);
        }
        Err(GitError::Failed {
            args: self
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            exit_code: output.exit_code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Runs git and returns its output whatever the exit status, for commands
    /// whose exit code carries an answer (`merge-base --is-ancestor`,
    /// `diff --exit-code`).
    pub async fn output_unchecked(&self) -> Result<ProcessOutput, GitError> {
        // P0-17: acquire the shared git concurrency limiter here.
        Ok(self.process().output().await?)
    }
}

/// Splits `-z` output into its NUL-terminated fields.
///
/// git ends every field with NUL, including the last, so a single trailing
/// NUL does not start another field. Consecutive NULs yield empty fields,
/// and a last field without its terminator is still returned. Fields are
/// bytes: paths need not be UTF-8.
pub fn split_nul(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = bytes.strip_suffix(b"\0").unwrap_or(bytes);
    let fields = (!bytes.is_empty()).then(|| body.split(|&b| b == 0));
    fields.into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GitError;
    use crate::git_binary::{resolve, ResolveOptions};
    use crate::test_support::SPAWN_LOCK;
    use std::future::Future;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn machine_git() -> PathBuf {
        resolve(&ResolveOptions::from_env(None)).unwrap().path
    }

    // ---- -z splitting ------------------------------------------------------

    fn fields(bytes: &[u8]) -> Vec<&[u8]> {
        split_nul(bytes).collect()
    }

    #[test]
    fn nul_split_of_empty_output_has_no_fields() {
        assert!(fields(b"").is_empty());
    }

    #[test]
    fn nul_split_drops_only_the_final_terminator() {
        assert_eq!(fields(b"a\0bc\0"), vec![&b"a"[..], b"bc"]);
    }

    #[test]
    fn nul_split_keeps_empty_fields() {
        assert_eq!(fields(b"\0"), vec![&b""[..]]);
        assert_eq!(fields(b"a\0\0b\0"), vec![&b"a"[..], b"", b"b"]);
        assert_eq!(fields(b"\0\0"), vec![&b""[..], b""]);
        assert_eq!(fields(b"a\0\0"), vec![&b"a"[..], b""]);
    }

    #[test]
    fn nul_split_accepts_a_missing_final_terminator() {
        assert_eq!(fields(b"a\0b"), vec![&b"a"[..], b"b"]);
        assert_eq!(fields(b"only"), vec![&b"only"[..]]);
    }

    #[test]
    fn nul_split_passes_non_utf8_bytes_through() {
        assert_eq!(
            fields(b"\xff\xfe\0caf\xc3\xa9\0"),
            vec![&b"\xff\xfe"[..], b"caf\xc3\xa9"]
        );
    }

    // ---- generic builder ---------------------------------------------------

    #[cfg(unix)]
    fn slow_command() -> ProcessCommand {
        let mut command = ProcessCommand::new("sleep");
        command.arg("10");
        command
    }

    #[cfg(windows)]
    fn slow_command() -> ProcessCommand {
        let mut command = ProcessCommand::new("ping");
        command.args(["-n", "11", "127.0.0.1"]);
        command
    }

    #[test]
    fn process_timeout_fires_and_kills_the_child() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut command = slow_command();
        command.timeout(Some(Duration::from_millis(200)));

        let started = Instant::now();
        let err = block_on(command.output()).unwrap_err();

        assert!(
            matches!(err, ProcessError::Timeout { timeout, .. } if timeout == Duration::from_millis(200)),
            "{err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn process_reports_spawn_failure() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let command = ProcessCommand::new(dir.path().join("no-such-program"));

        let err = block_on(command.output()).unwrap_err();

        assert!(matches!(err, ProcessError::Spawn { .. }), "{err:?}");
    }

    #[test]
    fn process_returns_non_zero_exit_without_mapping_it() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // ssh -T exits 1 on a successful GitHub test, so the generic builder
        // leaves the exit status to the caller.
        let mut command = ProcessCommand::new(machine_git());
        command.arg("definitely-not-a-git-command");

        let output = block_on(command.output()).unwrap();

        assert!(!output.success());
        assert_eq!(output.exit_code(), Some(1));
        // The message itself is localised, so only check that stderr was kept.
        assert!(!output.stderr.is_empty());
    }

    #[test]
    fn process_captures_stdout_bytes_and_passes_env_and_cwd() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mut command = ProcessCommand::new(machine_git());
        command
            .current_dir(dir.path())
            .env("SHOOTGIT_TEST_VALUE", "héllo")
            .args([
                "-c",
                "alias.show-env=!printf '%s\\0' \"$SHOOTGIT_TEST_VALUE\"",
            ])
            .arg("show-env");

        let output = block_on(command.output()).unwrap();

        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout, "héllo\0".as_bytes());
    }

    // ---- GitCommand --------------------------------------------------------

    /// Runs a shell alias through `GitCommand` and returns its NUL-split stdout.
    fn alias_output(configure: impl FnOnce(&mut GitCommand), script: &str) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitCommand::new(machine_git());
        git.current_dir(dir.path())
            .config("alias.probe", format!("!{script}"));
        configure(&mut git);
        git.arg("probe");
        let output = block_on(git.output()).unwrap();
        split_nul(&output.stdout)
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect()
    }

    #[test]
    fn git_command_always_sets_the_fixed_flags_and_env() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `--no-optional-locks` makes git export GIT_OPTIONAL_LOCKS=0 to its
        // children; `-c core.quotepath=off` is visible through `git config`.
        // A caller-set GIT_TERMINAL_PROMPT must not win over the fixed 0.
        let got = alias_output(
            |git| {
                git.env("GIT_TERMINAL_PROMPT", "1");
            },
            "printf '%s\\0' \"$GIT_OPTIONAL_LOCKS\" \"$GIT_TERMINAL_PROMPT\" \"$(git config core.quotepath)\"",
        );
        assert_eq!(got, ["0", "0", "off"]);
    }

    #[test]
    fn git_command_passes_per_spawn_config_and_askpass_env() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let got = alias_output(
            |git| {
                git.config("shootgit.probe", "a b=c")
                    .git_askpass("/opt/askpass one")
                    .ssh_askpass("/opt/askpass two")
                    .env("SHOOTGIT_EXTRA", "x");
            },
            "printf '%s\\0' \"$(git config shootgit.probe)\" \"$GIT_ASKPASS\" \"$SSH_ASKPASS\" \"$SHOOTGIT_EXTRA\"",
        );
        assert_eq!(got, ["a b=c", "/opt/askpass one", "/opt/askpass two", "x"]);
    }

    #[test]
    fn git_command_builds_args_in_order() {
        let mut git = GitCommand::new("git");
        git.config("credential.helper", "/x/helper")
            .args(["status", "--porcelain=v2", "-z"]);
        let args: Vec<String> = git
            .full_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--no-optional-locks",
                "-c",
                "core.quotepath=off",
                "-c",
                "credential.helper=/x/helper",
                "status",
                "--porcelain=v2",
                "-z",
            ]
        );
    }

    #[test]
    fn git_command_maps_non_zero_exit_to_git_error() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitCommand::new(machine_git());
        git.current_dir(dir.path())
            .config("alias.fail", "!echo boom >&2; exit 3")
            .arg("fail");

        let err = block_on(git.output()).unwrap_err();

        match err {
            GitError::Failed {
                exit_code,
                ref stderr,
                ref args,
            } => {
                assert_eq!(exit_code, Some(3));
                assert_eq!(stderr.trim(), "boom");
                assert_eq!(args, &["fail"]);
            }
            other => panic!("expected GitError::Failed, got {other:?}"),
        }
    }

    #[test]
    fn git_command_unchecked_output_keeps_non_zero_exit() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitCommand::new(machine_git());
        git.current_dir(dir.path())
            .config("alias.fail", "!exit 1")
            .arg("fail");

        let output = block_on(git.output_unchecked()).unwrap();

        assert_eq!(output.exit_code(), Some(1));
    }

    #[test]
    fn git_command_timeout_maps_to_git_error() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitCommand::new(machine_git());
        git.current_dir(dir.path())
            .config("alias.hang", "!sleep 10")
            .arg("hang")
            .timeout(Some(Duration::from_millis(300)));

        let started = Instant::now();
        let err = block_on(git.output()).unwrap_err();

        assert!(
            matches!(err, GitError::Process(ProcessError::Timeout { .. })),
            "{err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn git_command_missing_binary_is_a_spawn_error() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let git = GitCommand::new(dir.path().join("no-such-git"));

        let err = block_on(git.output()).unwrap_err();

        assert!(
            matches!(err, GitError::Process(ProcessError::Spawn { .. })),
            "{err:?}"
        );
    }
}
