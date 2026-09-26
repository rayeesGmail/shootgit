//! The login-shell `PATH` on macOS (§4 Process model, §5 step 2, §9
//! "Process spawn").
//!
//! An app started from Finder or the Dock inherits launchd's minimal `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), not the one the user's shell builds, so
//! Homebrew's git and `ssh` helpers would be invisible. The app asks the
//! user's shell once, at startup:
//!
//! ```text
//! $SHELL -ilc 'printf ... "$PATH" ...'
//! ```
//!
//! with a [`TIMEOUT`] of 2 s. If `$SHELL` is missing, the shell fails, prints
//! nothing usable or runs too long (it is killed with everything it started),
//! the app falls back to its own `PATH`, or [`DEFAULT_PATH`] if that is
//! empty. Either way [`init`] records the result with
//! [`process::set_login_shell_path`], so every later spawn through
//! `git_engine::process` (git and other tools) gets it, and
//! [`ResolveOptions::from_env`](crate::git_binary::ResolveOptions::from_env)
//! searches it for git.
//!
//! The shell prints `$PATH` between two marker lines rather than with a bare
//! `echo $PATH`, because interactive startup files often print banners,
//! prompts or terminal escape codes to stdout.
//!
//! The spawn goes through [`ProcessCommand`] like every other process: it is
//! counted, has a timeout, and on Windows would get `CREATE_NO_WINDOW`. It
//! takes no git limiter slot, as it is not git.
//!
//! Only macOS does this. Linux desktop sessions and Windows hand GUI apps the
//! user's `PATH` already, so there [`init`] does nothing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::OnceCell;

use crate::process::{self, ProcessCommand, ProcessError};

/// How long the login shell may take before the fallback is used (P0-07).
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// The fallback when this process has no `PATH` either: launchd's default.
pub const DEFAULT_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

const BEGIN_MARKER: &str = "_SHOOTGIT_PATH_BEGIN_";
const END_MARKER: &str = "_SHOOTGIT_PATH_END_";

/// What the shell runs. `printf` with separate arguments works the same in
/// sh, bash, zsh and fish (where a quoted `"$PATH"` is joined with `:`).
fn script() -> String {
    format!("printf '\\n%s\\n%s\\n%s\\n' {BEGIN_MARKER} \"$PATH\" {END_MARKER}")
}

/// Where a [`ResolvedPath`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    /// The user's login shell printed it.
    LoginShell,
    /// The shell could not be used; this is the process's own `PATH`, or
    /// [`DEFAULT_PATH`].
    Fallback,
}

/// The `PATH` every spawn should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    pub path: OsString,
    pub source: PathSource,
}

/// Why the login shell's `PATH` could not be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoginShellError {
    #[error("$SHELL is not set to an absolute path")]
    NoShell,
    /// The shell could not be started, or ran past its timeout and was
    /// killed together with everything it started.
    #[error("could not run the login shell {shell:?}")]
    Process {
        shell: PathBuf,
        #[source]
        source: ProcessError,
    },
    #[error("login shell {shell:?} failed (exit code {exit_code:?}): {stderr}")]
    Failed {
        shell: PathBuf,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("login shell {shell:?} did not print a usable PATH")]
    NoPath { shell: PathBuf },
}

/// Runs `<shell> -ilc <script>` and returns the `PATH` it prints.
///
/// The printed value is accepted whatever the exit status, as long as the
/// markers are there: a startup file that fails after the `PATH` is built
/// does not make the `PATH` wrong. It must contain at least one absolute
/// entry.
pub async fn query_path(shell: &Path, timeout: Duration) -> Result<OsString, LoginShellError> {
    let mut command = ProcessCommand::new(shell);
    command.arg("-ilc").arg(script()).timeout(Some(timeout));
    let output = command
        .output()
        .await
        .map_err(|source| LoginShellError::Process {
            shell: shell.to_path_buf(),
            source,
        })?;
    let Some(printed) = extract(&output.stdout) else {
        if output.success() {
            return Err(LoginShellError::NoPath {
                shell: shell.to_path_buf(),
            });
        }
        return Err(LoginShellError::Failed {
            shell: shell.to_path_buf(),
            exit_code: output.exit_code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    };
    let path = os_string_from_bytes(printed)
        .filter(|path| std::env::split_paths(path).any(|dir| dir.is_absolute()))
        .ok_or_else(|| LoginShellError::NoPath {
            shell: shell.to_path_buf(),
        })?;
    if !output.success() {
        tracing::debug!(
            ?shell,
            exit_code = ?output.exit_code(),
            "login shell exited unsuccessfully after printing PATH; using it"
        );
    }
    Ok(path)
}

/// The `PATH` between the last begin marker and the end marker after it.
fn extract(stdout: &[u8]) -> Option<&[u8]> {
    let begin = format!("\n{BEGIN_MARKER}\n");
    let end = format!("\n{END_MARKER}\n");
    let start = rfind(stdout, begin.as_bytes())? + begin.len();
    let rest = stdout.get(start..)?;
    let stop = find(rest, end.as_bytes())?;
    rest.get(..stop)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// A login-shell `PATH` lookup that runs at most once.
///
/// The app uses the process-wide one behind [`init`]; tests build their own
/// with a fake shell.
#[derive(Debug)]
pub struct LoginShellEnv {
    shell: Option<PathBuf>,
    fallback: OsString,
    timeout: Duration,
    resolved: OnceCell<ResolvedPath>,
}

impl LoginShellEnv {
    /// A lookup that asks `shell` (normally `$SHELL`) and falls back to
    /// `fallback` (normally this process's `PATH`), or to [`DEFAULT_PATH`]
    /// when that is missing or empty.
    pub fn new(shell: Option<PathBuf>, fallback: Option<OsString>) -> Self {
        let fallback = fallback
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| OsString::from(DEFAULT_PATH));
        Self {
            shell,
            fallback,
            timeout: TIMEOUT,
            resolved: OnceCell::new(),
        }
    }

    /// `$SHELL` and this process's `PATH`.
    pub fn from_env() -> Self {
        Self::new(
            std::env::var_os("SHELL").map(PathBuf::from),
            std::env::var_os("PATH"),
        )
    }

    /// Overrides [`TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The resolved `PATH`. The first call runs the shell; every later or
    /// concurrent call waits for, and returns, that same result.
    pub async fn path(&self) -> &ResolvedPath {
        self.resolved.get_or_init(|| self.resolve()).await
    }

    async fn resolve(&self) -> ResolvedPath {
        let result = match &self.shell {
            Some(shell) if shell.is_absolute() => query_path(shell, self.timeout).await,
            _ => Err(LoginShellError::NoShell),
        };
        match result {
            Ok(path) => {
                tracing::debug!(?path, "resolved the login-shell PATH");
                ResolvedPath {
                    path,
                    source: PathSource::LoginShell,
                }
            }
            Err(error) => {
                tracing::warn!(%error, fallback = ?self.fallback, "using the fallback PATH");
                ResolvedPath {
                    path: self.fallback.clone(),
                    source: PathSource::Fallback,
                }
            }
        }
    }
}

/// The process-wide lookup behind [`init`].
static APP_ENV: LazyLock<LoginShellEnv> = LazyLock::new(LoginShellEnv::from_env);

/// On macOS, resolves the login-shell `PATH` (once per process) and records
/// it with [`process::set_login_shell_path`] for every later spawn. Call it
/// at startup, and await it before resolving git; concurrent and later calls
/// return the same value without spawning again.
///
/// Elsewhere it does nothing and returns `None`.
pub async fn init() -> Option<&'static ResolvedPath> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let resolved = APP_ENV.path().await;
    if process::login_shell_path() != Some(resolved.path.as_os_str())
        && process::set_login_shell_path(resolved.path.clone()).is_err()
    {
        tracing::warn!("the login-shell PATH had already been set to a different value");
    }
    Some(resolved)
}

/// Bytes printed by the shell as an `OsString`. Non-UTF-8 is kept on Unix.
fn os_string_from_bytes(bytes: &[u8]) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(std::ffi::OsStr::from_bytes(bytes).to_os_string())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(OsString::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SPAWN_LOCK;
    #[cfg(unix)]
    use std::fs;
    use std::future::Future;
    #[cfg(unix)]
    use std::time::Instant;

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    // ---- output parsing ----------------------------------------------------

    fn wrapped(path: &str) -> String {
        format!("\n{BEGIN_MARKER}\n{path}\n{END_MARKER}\n")
    }

    #[test]
    fn extracts_the_path_between_the_markers() {
        let out = wrapped("/opt/homebrew/bin:/usr/bin");
        assert_eq!(
            extract(out.as_bytes()),
            Some(&b"/opt/homebrew/bin:/usr/bin"[..])
        );
    }

    #[test]
    fn ignores_banners_and_escape_codes_around_the_markers() {
        let out = format!(
            "Last login: today\n\x1b]7;file://host/\x07{}\x1b[?1l bye\n",
            wrapped("/usr/local/bin:/usr/bin")
        );
        assert_eq!(
            extract(out.as_bytes()),
            Some(&b"/usr/local/bin:/usr/bin"[..])
        );
    }

    #[test]
    fn uses_the_last_begin_marker() {
        // A startup file that echoes our own command line cannot inject a PATH.
        let out = format!("\n{BEGIN_MARKER}\n/evil\n{}", wrapped("/usr/bin"));
        assert_eq!(extract(out.as_bytes()), Some(&b"/usr/bin"[..]));
    }

    #[test]
    fn missing_markers_yield_nothing() {
        assert_eq!(extract(b""), None);
        assert_eq!(extract(b"/usr/bin:/bin\n"), None);
        let no_end = format!("\n{BEGIN_MARKER}\n/usr/bin\n");
        assert_eq!(extract(no_end.as_bytes()), None);
    }

    #[test]
    fn default_timeout_is_two_seconds() {
        assert_eq!(TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn empty_fallback_becomes_the_default_path() {
        let env = LoginShellEnv::new(None, Some(OsString::new()));
        let resolved = block_on(env.path());
        assert_eq!(resolved.path, OsString::from(DEFAULT_PATH));
        assert_eq!(resolved.source, PathSource::Fallback);

        let env = LoginShellEnv::new(None, None);
        assert_eq!(block_on(env.path()).path, OsString::from(DEFAULT_PATH));
    }

    #[test]
    fn relative_or_missing_shell_falls_back_without_spawning() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = process::spawn_count();
        for shell in [None, Some(PathBuf::from("zsh"))] {
            let env = LoginShellEnv::new(shell, Some(OsString::from("/fallback/bin")));
            let resolved = block_on(env.path());
            assert_eq!(resolved.path, OsString::from("/fallback/bin"));
            assert_eq!(resolved.source, PathSource::Fallback);
        }
        assert_eq!(process::spawn_count(), before);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn init_does_nothing_off_macos() {
        let before = process::spawn_count();
        assert_eq!(block_on(init()), None);
        assert_eq!(process::login_shell_path(), None);
        assert_eq!(process::spawn_count(), before);
    }

    // ---- fake shells ---------------------------------------------------------
    //
    // A fake `$SHELL` is a `/bin/sh` script. It checks it was called as
    // `-ilc <script>`, then runs the script with a PATH of its own.

    #[cfg(unix)]
    fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-shell");
        let text = format!(
            "#!/bin/sh\n[ \"$1\" = -ilc ] || {{ echo \"bad args: $*\" >&2; exit 64; }}\n{body}\n"
        );
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A shell whose startup prints noise, then builds `path` and runs `-c`.
    #[cfg(unix)]
    fn shell_printing(dir: &Path, path: &str) -> PathBuf {
        fake_shell(
            dir,
            &format!("echo 'Welcome to fakesh'\nPATH='{path}'\neval \"$2\"\necho 'logout'"),
        )
    }

    #[cfg(unix)]
    #[test]
    fn reads_the_path_the_login_shell_builds() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let shell = shell_printing(dir.path(), "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        let before = process::spawn_count();

        let path = block_on(query_path(&shell, TIMEOUT)).unwrap();

        assert_eq!(
            path,
            OsString::from("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        );
        assert_eq!(
            process::spawn_count(),
            before + 1,
            "spawned via git_engine::process"
        );
    }

    #[cfg(unix)]
    #[test]
    fn keeps_spaces_and_unicode_in_the_path() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let shell = shell_printing(dir.path(), "/Users/zoë/my tools/bin:/usr/bin");

        let path = block_on(query_path(&shell, TIMEOUT)).unwrap();

        assert_eq!(path, OsString::from("/Users/zoë/my tools/bin:/usr/bin"));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_fires_and_falls_back() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // An rc file that never returns, e.g. one that waits on the network.
        let shell = fake_shell(dir.path(), "sleep 10\neval \"$2\"");
        let timeout = Duration::from_millis(300);

        let started = Instant::now();
        let err = block_on(query_path(&shell, timeout)).unwrap_err();
        assert!(
            matches!(
                err,
                LoginShellError::Process { source: ProcessError::Timeout { timeout: t, .. }, .. }
                    if t == timeout
            ),
            "{err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );

        let env = LoginShellEnv::new(Some(shell), Some(OsString::from("/fallback/bin")))
            .with_timeout(timeout);
        let started = Instant::now();
        let resolved = block_on(env.path());
        assert_eq!(resolved.path, OsString::from("/fallback/bin"));
        assert_eq!(resolved.source, PathSource::Fallback);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn failing_or_silent_shells_fall_back() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        type Check = fn(&LoginShellError) -> bool;
        let cases: [(&str, Check); 4] = [
            (
                "echo 'rc: syntax error' >&2\nexit 1",
                |e| matches!(e, LoginShellError::Failed { exit_code: Some(1), stderr, .. } if stderr.contains("syntax error")),
            ),
            ("exit 0", |e| matches!(e, LoginShellError::NoPath { .. })),
            // An empty PATH, or only relative entries, is not usable.
            ("PATH=''\neval \"$2\"", |e| {
                matches!(e, LoginShellError::NoPath { .. })
            }),
            ("PATH='bin:.'\neval \"$2\"", |e| {
                matches!(e, LoginShellError::NoPath { .. })
            }),
        ];
        for (body, expected) in cases {
            let shell = fake_shell(dir.path(), body);
            let err = block_on(query_path(&shell, TIMEOUT)).unwrap_err();
            assert!(expected(&err), "{body:?}: {err:?}");

            let env = LoginShellEnv::new(Some(shell), Some(OsString::from("/fallback/bin")));
            assert_eq!(
                block_on(env.path()).source,
                PathSource::Fallback,
                "{body:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn path_printed_before_a_failing_exit_is_still_used() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "PATH='/usr/local/bin:/usr/bin'\neval \"$2\"\nexit 1",
        );

        let path = block_on(query_path(&shell, TIMEOUT)).unwrap();

        assert_eq!(path, OsString::from("/usr/local/bin:/usr/bin"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_shell_is_a_process_error() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let err = block_on(query_path(&dir.path().join("no-such-shell"), TIMEOUT)).unwrap_err();
        assert!(
            matches!(
                err,
                LoginShellError::Process {
                    source: ProcessError::Spawn { .. },
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_runs_once_however_often_the_path_is_asked_for() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("runs");
        let shell = fake_shell(
            dir.path(),
            &format!(
                "echo run >> '{}'\nsleep 0.2\nPATH='/login/bin:/usr/bin'\neval \"$2\"",
                runs.display()
            ),
        );
        let env = LoginShellEnv::new(Some(shell), Some(OsString::from("/fallback/bin")));

        let (a, b) = block_on(async { tokio::join!(env.path(), env.path()) });
        let c = block_on(env.path());

        for resolved in [a, b, c] {
            assert_eq!(resolved.path, OsString::from("/login/bin:/usr/bin"));
            assert_eq!(resolved.source, PathSource::LoginShell);
        }
        assert_eq!(fs::read_to_string(&runs).unwrap(), "run\n");
    }

    /// The app runs `init()` on the shared runtime.
    #[test]
    fn init_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let future = init();
        assert_send(&future);
    }
}
