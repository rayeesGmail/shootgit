//! Locating the `git` executable the engine runs (§5 Git binary resolution).
//!
//! Resolution order:
//!
//! 1. The path the user configured in settings. When one is configured it is
//!    authoritative: if it is unusable, resolution fails with that error
//!    instead of silently picking a different git.
//! 2. The first `git` on `PATH` whose version is at least [`MIN_GIT_VERSION`].
//!    Older or broken candidates are skipped and the search continues.
//! 3. The bundled git (MinGit on Windows, bundled git on macOS and in the
//!    Linux AppImage). Bundling has not landed yet, so
//!    [`bundled_git_path`] is a placeholder that returns `None`.
//!
//! On every tier Apple's `/usr/bin/git` is refused when the Xcode Command
//! Line Tools are missing (`xcode-select -p` fails): that binary is only a
//! stub which opens an installer dialog instead of running git.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// The oldest git the engine supports (CLAUDE.md, §5).
pub const MIN_GIT_VERSION: GitVersion = GitVersion::new(2, 30, 0);

/// A git release number, parsed from `git --version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses the output of `git --version`.
    ///
    /// Accepts the forms git prints in practice: `git version 2.43.0`,
    /// `2.39.5 (Apple Git-154)`, `2.45.1.windows.1`, `2.30.0.rc1`,
    /// `2.46.GIT`. A missing or non-numeric patch component counts as 0;
    /// major and minor are required.
    pub fn parse(output: &str) -> Result<Self, GitBinaryError> {
        let unrecognised = || GitBinaryError::UnrecognisedVersion {
            output: output.to_owned(),
        };
        let number = output
            .trim()
            .strip_prefix("git version ")
            .and_then(|rest| rest.split_whitespace().next())
            .ok_or_else(unrecognised)?;
        let mut parts = number.split('.');
        let mut component = || parts.next().and_then(|p| p.parse::<u32>().ok());
        let major = component().ok_or_else(unrecognised)?;
        let minor = component().ok_or_else(unrecognised)?;
        let patch = component().unwrap_or(0);
        Ok(Self::new(major, minor, patch))
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Which resolution tier produced a [`GitBinary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSource {
    Settings,
    Path,
    Bundled,
}

/// A usable git executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBinary {
    pub path: PathBuf,
    pub version: GitVersion,
    pub source: GitSource,
}

/// A candidate that resolution looked at and skipped.
#[derive(Debug)]
pub struct RejectedCandidate {
    pub path: PathBuf,
    pub source: GitSource,
    pub reason: GitBinaryError,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitBinaryError {
    #[error("git at {path:?} does not exist or is not an executable file")]
    NotExecutable { path: PathBuf },
    #[error("{path:?} is Apple's git stub and the Xcode Command Line Tools are not installed")]
    XcodeStub { path: PathBuf },
    #[error("could not run {path:?} --version")]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path:?} --version failed: {stderr}")]
    VersionFailed { path: PathBuf, stderr: String },
    #[error("unrecognised `git --version` output: {output:?}")]
    UnrecognisedVersion { output: String },
    #[error("git at {path:?} is version {found}; {required} or newer is required")]
    TooOld {
        path: PathBuf,
        found: GitVersion,
        required: GitVersion,
    },
    #[error("no usable git found in settings, on PATH or bundled ({} candidate(s) rejected)", rejected.len())]
    NotFound { rejected: Vec<RejectedCandidate> },
}

/// Inputs to [`resolve`].
///
/// `Default` leaves every field empty, which is what the tests build on; the
/// app uses [`ResolveOptions::from_env`].
#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// Tier 1: the git path configured in settings, if any.
    pub settings_path: Option<PathBuf>,
    /// Tier 2: the `PATH` value to search.
    pub search_path: Option<OsString>,
    /// Tier 3: the bundled git, if this build ships one.
    pub bundled_path: Option<PathBuf>,
    /// Apple's stub location, refused unless the Xcode CLT are installed.
    /// `Some("/usr/bin/git")` on macOS, `None` elsewhere.
    pub apple_stub_path: Option<PathBuf>,
}

impl ResolveOptions {
    /// Options for the running process.
    ///
    /// `search_path` is this process's `PATH`. On macOS a GUI app does not
    /// inherit the login-shell `PATH` (§5 step 2); once P0-07 resolves it,
    /// the caller should overwrite `search_path` with that value.
    pub fn from_env(settings_path: Option<PathBuf>) -> Self {
        Self {
            settings_path,
            search_path: std::env::var_os("PATH"),
            bundled_path: bundled_git_path(),
            apple_stub_path: cfg!(target_os = "macos").then(|| PathBuf::from(APPLE_STUB_PATH)),
        }
    }
}

/// Where Apple's `git` shim lives; without the Xcode CLT it is only a stub.
const APPLE_STUB_PATH: &str = "/usr/bin/git";

#[cfg(windows)]
const GIT_EXE: &str = "git.exe";
#[cfg(not(windows))]
const GIT_EXE: &str = "git";

/// Placeholder for the bundled-git tier (§5 step 3, §9 packaging).
///
/// No build ships a git yet: MinGit on Windows, a bundled git on macOS and
/// the AppImage git on Linux arrive with the packaging work. Until then this
/// always returns `None`, so resolution ends after the `PATH` tier. When
/// bundling lands, this returns the path inside the app bundle.
pub fn bundled_git_path() -> Option<PathBuf> {
    None
}

/// Resolves the git executable to use.
pub fn resolve(options: &ResolveOptions) -> Result<GitBinary, GitBinaryError> {
    resolve_with(options, &SystemProbe)
}

/// The side effects resolution needs, injectable so tests can fake them.
trait Probe {
    /// Runs `<git> --version` and returns its stdout.
    fn version_output(&self, git: &Path) -> Result<String, GitBinaryError>;
    /// Whether `xcode-select -p` succeeds.
    fn xcode_clt_installed(&self) -> bool;
}

/// The real probe, spawning processes.
struct SystemProbe;

impl Probe for SystemProbe {
    fn version_output(&self, git: &Path) -> Result<String, GitBinaryError> {
        let output = run(git, &["--version"]).map_err(|source| GitBinaryError::Spawn {
            path: git.to_path_buf(),
            source,
        })?;
        if !output.status.success() {
            return Err(GitBinaryError::VersionFailed {
                path: git.to_path_buf(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn xcode_clt_installed(&self) -> bool {
        // Absolute path so a `PATH` entry cannot shadow it. Failing to spawn
        // counts as "not installed": then the stub is skipped, which is safe.
        match run(Path::new("/usr/bin/xcode-select"), &["-p"]) {
            Ok(output) => output.status.success(),
            Err(error) => {
                tracing::debug!(%error, "xcode-select -p could not run");
                false
            }
        }
    }
}

/// The only place this module spawns a process.
///
/// Bootstrap: `git_engine::process` (P0-06) does not exist yet. Once it does,
/// route this through its generic builder so the spawn gets the shared
/// limiter and a timeout.
fn run(program: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut command = std::process::Command::new(program);
    command.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.output()
}

fn resolve_with(options: &ResolveOptions, probe: &dyn Probe) -> Result<GitBinary, GitBinaryError> {
    let mut resolver = Resolver {
        options,
        probe,
        clt_installed: None,
    };

    if let Some(path) = &options.settings_path {
        return resolver.check(path, GitSource::Settings);
    }

    let mut rejected = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let path_candidates = options
        .search_path
        .iter()
        .flat_map(std::env::split_paths)
        // An empty or relative entry resolves against the current directory,
        // which may be an untrusted repository carrying its own `git`.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(GIT_EXE))
        .filter(|candidate| is_executable_file(candidate));
    for candidate in path_candidates {
        if seen.contains(&candidate) {
            continue;
        }
        seen.push(candidate.clone());
        match resolver.check(&candidate, GitSource::Path) {
            Ok(found) => return Ok(found),
            Err(reason) => {
                tracing::debug!(path = ?candidate, %reason, "skipping git on PATH");
                rejected.push(RejectedCandidate {
                    path: candidate,
                    source: GitSource::Path,
                    reason,
                });
            }
        }
    }

    if let Some(bundled) = &options.bundled_path {
        match resolver.check(bundled, GitSource::Bundled) {
            Ok(found) => return Ok(found),
            Err(reason) => rejected.push(RejectedCandidate {
                path: bundled.clone(),
                source: GitSource::Bundled,
                reason,
            }),
        }
    }

    Err(GitBinaryError::NotFound { rejected })
}

struct Resolver<'a> {
    options: &'a ResolveOptions,
    probe: &'a dyn Probe,
    /// `xcode-select -p` runs at most once per resolution, and only if the
    /// Apple stub is actually a candidate.
    clt_installed: Option<bool>,
}

impl Resolver<'_> {
    /// Accepts `path` if it is an executable git ≥ [`MIN_GIT_VERSION`] and not
    /// Apple's stub on a machine without the Xcode CLT.
    fn check(&mut self, path: &Path, source: GitSource) -> Result<GitBinary, GitBinaryError> {
        if !is_executable_file(path) {
            return Err(GitBinaryError::NotExecutable {
                path: path.to_path_buf(),
            });
        }
        if self.options.apple_stub_path.as_deref() == Some(path) && !self.clt_installed() {
            // Never run it: the stub opens the CLT installer dialog.
            return Err(GitBinaryError::XcodeStub {
                path: path.to_path_buf(),
            });
        }
        let version = GitVersion::parse(&self.probe.version_output(path)?)?;
        if version < MIN_GIT_VERSION {
            return Err(GitBinaryError::TooOld {
                path: path.to_path_buf(),
                found: version,
                required: MIN_GIT_VERSION,
            });
        }
        Ok(GitBinary {
            path: path.to_path_buf(),
            version,
            source,
        })
    }

    fn clt_installed(&mut self) -> bool {
        let probe = self.probe;
        *self
            .clt_installed
            .get_or_insert_with(|| probe.xcode_clt_installed())
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    #[cfg(windows)]
    const EXE: &str = "git.exe";
    #[cfg(not(windows))]
    const EXE: &str = "git";

    // ---- version parsing -------------------------------------------------

    fn v(major: u32, minor: u32, patch: u32) -> GitVersion {
        GitVersion::new(major, minor, patch)
    }

    #[test]
    fn parses_plain_release() {
        assert_eq!(
            GitVersion::parse("git version 2.43.0\n").unwrap(),
            v(2, 43, 0)
        );
    }

    #[test]
    fn parses_apple_git() {
        assert_eq!(
            GitVersion::parse("git version 2.39.5 (Apple Git-154)\n").unwrap(),
            v(2, 39, 5)
        );
    }

    #[test]
    fn parses_git_for_windows_with_crlf() {
        assert_eq!(
            GitVersion::parse("git version 2.45.1.windows.1\r\n").unwrap(),
            v(2, 45, 1)
        );
    }

    #[test]
    fn parses_release_candidates_and_dev_builds() {
        assert_eq!(
            GitVersion::parse("git version 2.30.0.rc1").unwrap(),
            v(2, 30, 0)
        );
        assert_eq!(
            GitVersion::parse("git version 2.46.GIT").unwrap(),
            v(2, 46, 0)
        );
        assert_eq!(
            GitVersion::parse("git version 2.44.0.123.gdeadbeef").unwrap(),
            v(2, 44, 0)
        );
        assert_eq!(GitVersion::parse("git version 3.0").unwrap(), v(3, 0, 0));
    }

    #[test]
    fn rejects_unrecognised_output() {
        for bad in [
            "",
            "git version",
            "git version x.y.z",
            "git version 2",
            "hg version 6.1.0",
            "xcrun: error: invalid active developer path",
        ] {
            assert!(
                matches!(
                    GitVersion::parse(bad),
                    Err(GitBinaryError::UnrecognisedVersion { .. })
                ),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(v(2, 30, 0) >= MIN_GIT_VERSION);
        assert!(v(2, 29, 9) < MIN_GIT_VERSION);
        assert!(v(2, 100, 0) > v(2, 30, 0));
        assert!(v(3, 0, 0) > v(2, 99, 99));
        assert_eq!(v(2, 45, 1).to_string(), "2.45.1");
    }

    // ---- resolver over fake PATH dirs with a fake probe ------------------

    #[derive(Default)]
    struct FakeProbe {
        versions: HashMap<PathBuf, String>,
        clt_installed: bool,
        version_calls: RefCell<Vec<PathBuf>>,
        xcode_calls: Cell<u32>,
    }

    impl FakeProbe {
        fn with(mut self, git: &Path, version: &str) -> Self {
            self.versions
                .insert(git.to_path_buf(), format!("git version {version}\n"));
            self
        }
    }

    impl Probe for FakeProbe {
        fn version_output(&self, git: &Path) -> Result<String, GitBinaryError> {
            self.version_calls.borrow_mut().push(git.to_path_buf());
            self.versions
                .get(git)
                .cloned()
                .ok_or_else(|| GitBinaryError::VersionFailed {
                    path: git.to_path_buf(),
                    stderr: "fake probe has no version for this path".into(),
                })
        }

        fn xcode_clt_installed(&self) -> bool {
            self.xcode_calls.set(self.xcode_calls.get() + 1);
            self.clt_installed
        }
    }

    /// Creates `<dir>/git` (`git.exe` on Windows), executable on Unix.
    fn fake_git(dir: &Path) -> PathBuf {
        let path = dir.join(EXE);
        fs::write(&path, b"not really git").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn search_path(dirs: &[&Path]) -> Option<OsString> {
        Some(env::join_paths(dirs).unwrap())
    }

    fn dirs<const N: usize>() -> [TempDir; N] {
        std::array::from_fn(|_| tempfile::tempdir().unwrap())
    }

    #[test]
    fn finds_first_new_enough_git_on_path() {
        let [empty, a, b] = dirs();
        let git_a = fake_git(a.path());
        let git_b = fake_git(b.path());
        let probe = FakeProbe::default()
            .with(&git_a, "2.43.0")
            .with(&git_b, "2.45.1");
        let options = ResolveOptions {
            search_path: search_path(&[empty.path(), a.path(), b.path()]),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(
            found,
            GitBinary {
                path: git_a.clone(),
                version: v(2, 43, 0),
                source: GitSource::Path,
            }
        );
        assert_eq!(*probe.version_calls.borrow(), vec![git_a]);
    }

    #[test]
    fn too_old_path_git_is_skipped_for_a_later_one() {
        let [old, new] = dirs();
        let git_old = fake_git(old.path());
        let git_new = fake_git(new.path());
        let probe = FakeProbe::default()
            .with(&git_old, "2.29.2")
            .with(&git_new, "2.30.0");
        let options = ResolveOptions {
            search_path: search_path(&[old.path(), new.path()]),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(found.path, git_new);
        assert_eq!(found.version, v(2, 30, 0));
        assert_eq!(found.source, GitSource::Path);
    }

    #[test]
    fn only_too_old_git_fails_with_the_rejection_listed() {
        let [old] = dirs();
        let git_old = fake_git(old.path());
        let probe = FakeProbe::default().with(&git_old, "2.25.1");
        let options = ResolveOptions {
            search_path: search_path(&[old.path()]),
            ..Default::default()
        };

        let err = resolve_with(&options, &probe).unwrap_err();

        let GitBinaryError::NotFound { rejected } = err else {
            panic!("expected NotFound, got {err:?}");
        };
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].path, git_old);
        assert_eq!(rejected[0].source, GitSource::Path);
        assert!(matches!(
            rejected[0].reason,
            GitBinaryError::TooOld { found, required, .. }
                if found == v(2, 25, 1) && required == MIN_GIT_VERSION
        ));
    }

    #[test]
    fn broken_path_git_is_skipped() {
        let [broken, good] = dirs();
        let git_broken = fake_git(broken.path());
        let git_good = fake_git(good.path());
        let mut probe = FakeProbe::default().with(&git_good, "2.44.0");
        probe
            .versions
            .insert(git_broken.clone(), "not a version".into());
        let options = ResolveOptions {
            search_path: search_path(&[broken.path(), good.path()]),
            ..Default::default()
        };

        assert_eq!(resolve_with(&options, &probe).unwrap().path, git_good);
    }

    #[test]
    fn nothing_anywhere_is_not_found() {
        let [empty] = dirs();
        let options = ResolveOptions {
            search_path: search_path(&[empty.path()]),
            ..Default::default()
        };

        let err = resolve_with(&options, &FakeProbe::default()).unwrap_err();

        assert!(matches!(err, GitBinaryError::NotFound { ref rejected } if rejected.is_empty()));
    }

    #[test]
    fn settings_path_wins_over_path() {
        let [configured, on_path] = dirs();
        let git_configured = fake_git(configured.path());
        let git_on_path = fake_git(on_path.path());
        let probe = FakeProbe::default()
            .with(&git_configured, "2.40.0")
            .with(&git_on_path, "2.46.0");
        let options = ResolveOptions {
            settings_path: Some(git_configured.clone()),
            search_path: search_path(&[on_path.path()]),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(found.path, git_configured);
        assert_eq!(found.source, GitSource::Settings);
        assert_eq!(*probe.version_calls.borrow(), vec![git_configured]);
    }

    #[test]
    fn unusable_settings_path_is_an_error_not_a_fallback() {
        let [configured, on_path] = dirs();
        let git_configured = fake_git(configured.path());
        let git_on_path = fake_git(on_path.path());
        let probe = FakeProbe::default()
            .with(&git_configured, "2.20.0")
            .with(&git_on_path, "2.46.0");
        let mut options = ResolveOptions {
            settings_path: Some(git_configured.clone()),
            search_path: search_path(&[on_path.path()]),
            ..Default::default()
        };

        let err = resolve_with(&options, &probe).unwrap_err();
        assert!(
            matches!(err, GitBinaryError::TooOld { ref path, .. } if *path == git_configured),
            "{err:?}"
        );

        options.settings_path = Some(configured.path().join("missing-git"));
        let err = resolve_with(&options, &probe).unwrap_err();
        assert!(
            matches!(err, GitBinaryError::NotExecutable { .. }),
            "{err:?}"
        );

        // A directory is not an executable file either.
        options.settings_path = Some(configured.path().to_path_buf());
        let err = resolve_with(&options, &probe).unwrap_err();
        assert!(
            matches!(err, GitBinaryError::NotExecutable { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn relative_and_empty_path_entries_are_ignored() {
        // A relative PATH entry would resolve against the current directory,
        // which may be an untrusted repository carrying its own `git`.
        let [good] = dirs();
        let git_good = fake_git(good.path());
        let probe = FakeProbe::default()
            .with(&git_good, "2.44.0")
            .with(&Path::new(".").join(EXE), "2.99.0")
            .with(&Path::new("bin").join(EXE), "2.99.0");
        let mut entries = OsString::from(".");
        let sep = if cfg!(windows) { ";" } else { ":" };
        for part in ["", "bin"] {
            entries.push(sep);
            entries.push(part);
        }
        entries.push(sep);
        entries.push(good.path());
        let options = ResolveOptions {
            search_path: Some(entries),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(found.path, git_good);
        assert_eq!(*probe.version_calls.borrow(), vec![git_good]);
    }

    #[test]
    fn duplicate_path_entries_are_probed_once() {
        let [old] = dirs();
        let git_old = fake_git(old.path());
        let probe = FakeProbe::default().with(&git_old, "2.20.0");
        let options = ResolveOptions {
            search_path: search_path(&[old.path(), old.path()]),
            ..Default::default()
        };

        let _ = resolve_with(&options, &probe).unwrap_err();

        assert_eq!(probe.version_calls.borrow().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_file_on_path_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let [plain, good] = dirs();
        let git_plain = fake_git(plain.path());
        fs::set_permissions(&git_plain, fs::Permissions::from_mode(0o644)).unwrap();
        let git_good = fake_git(good.path());
        let probe = FakeProbe::default()
            .with(&git_plain, "2.44.0")
            .with(&git_good, "2.44.0");
        let options = ResolveOptions {
            search_path: search_path(&[plain.path(), good.path()]),
            ..Default::default()
        };

        assert_eq!(resolve_with(&options, &probe).unwrap().path, git_good);
    }

    #[test]
    fn bundled_git_is_the_last_resort() {
        let [empty, bundle] = dirs();
        let git_bundled = fake_git(bundle.path());
        let probe = FakeProbe::default().with(&git_bundled, "2.46.0");
        let options = ResolveOptions {
            search_path: search_path(&[empty.path()]),
            bundled_path: Some(git_bundled.clone()),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(found.path, git_bundled);
        assert_eq!(found.source, GitSource::Bundled);
    }

    #[test]
    fn path_git_is_preferred_over_bundled() {
        let [on_path, bundle] = dirs();
        let git_on_path = fake_git(on_path.path());
        let git_bundled = fake_git(bundle.path());
        let probe = FakeProbe::default()
            .with(&git_on_path, "2.30.1")
            .with(&git_bundled, "2.46.0");
        let options = ResolveOptions {
            search_path: search_path(&[on_path.path()]),
            bundled_path: Some(git_bundled),
            ..Default::default()
        };

        assert_eq!(
            resolve_with(&options, &probe).unwrap().source,
            GitSource::Path
        );
    }

    #[test]
    fn missing_bundled_git_is_listed_as_rejected() {
        let [bundle] = dirs();
        let options = ResolveOptions {
            bundled_path: Some(bundle.path().join(EXE)),
            ..Default::default()
        };

        let err = resolve_with(&options, &FakeProbe::default()).unwrap_err();

        let GitBinaryError::NotFound { rejected } = err else {
            panic!("expected NotFound, got {err:?}");
        };
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].source, GitSource::Bundled);
        assert!(matches!(
            rejected[0].reason,
            GitBinaryError::NotExecutable { .. }
        ));
    }

    #[test]
    fn bundled_placeholder_is_empty_until_bundling_lands() {
        assert_eq!(bundled_git_path(), None);
    }

    // ---- macOS Xcode stub --------------------------------------------------
    //
    // The stub check is plain logic over `apple_stub_path`, so it runs on every
    // OS with a fake stub location; only the default value is macOS-specific.

    #[test]
    fn apple_stub_is_skipped_without_xcode_clt() {
        let [usr_bin, brew] = dirs();
        let stub = fake_git(usr_bin.path());
        let git_brew = fake_git(brew.path());
        let probe = FakeProbe::default()
            .with(&stub, "2.39.5 (Apple Git-154)")
            .with(&git_brew, "2.46.0");
        let options = ResolveOptions {
            search_path: search_path(&[usr_bin.path(), brew.path()]),
            apple_stub_path: Some(stub.clone()),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(found.path, git_brew);
        // Running the stub would pop the CLT installer, so it must never run.
        assert!(!probe.version_calls.borrow().contains(&stub));
        assert_eq!(probe.xcode_calls.get(), 1);
    }

    #[test]
    fn apple_stub_alone_without_xcode_clt_is_not_found() {
        let [usr_bin] = dirs();
        let stub = fake_git(usr_bin.path());
        let probe = FakeProbe::default().with(&stub, "2.39.5 (Apple Git-154)");
        let options = ResolveOptions {
            search_path: search_path(&[usr_bin.path()]),
            apple_stub_path: Some(stub.clone()),
            ..Default::default()
        };

        let err = resolve_with(&options, &probe).unwrap_err();

        let GitBinaryError::NotFound { rejected } = err else {
            panic!("expected NotFound, got {err:?}");
        };
        assert!(matches!(
            rejected[0].reason,
            GitBinaryError::XcodeStub { .. }
        ));
        assert!(probe.version_calls.borrow().is_empty());
    }

    #[test]
    fn apple_git_is_used_when_xcode_clt_is_installed() {
        let [usr_bin, brew] = dirs();
        let stub = fake_git(usr_bin.path());
        let git_brew = fake_git(brew.path());
        let mut probe = FakeProbe::default()
            .with(&stub, "2.39.5 (Apple Git-154)")
            .with(&git_brew, "2.46.0");
        probe.clt_installed = true;
        let options = ResolveOptions {
            search_path: search_path(&[usr_bin.path(), brew.path()]),
            apple_stub_path: Some(stub.clone()),
            ..Default::default()
        };

        let found = resolve_with(&options, &probe).unwrap();

        assert_eq!(found.path, stub);
        assert_eq!(found.version, v(2, 39, 5));
    }

    #[test]
    fn apple_stub_in_settings_without_xcode_clt_is_an_error() {
        let [usr_bin] = dirs();
        let stub = fake_git(usr_bin.path());
        let probe = FakeProbe::default().with(&stub, "2.39.5 (Apple Git-154)");
        let options = ResolveOptions {
            settings_path: Some(stub.clone()),
            apple_stub_path: Some(stub),
            ..Default::default()
        };

        let err = resolve_with(&options, &probe).unwrap_err();

        assert!(matches!(err, GitBinaryError::XcodeStub { .. }), "{err:?}");
        assert!(probe.version_calls.borrow().is_empty());
    }

    #[test]
    fn xcode_select_is_not_run_when_no_stub_is_involved() {
        let [brew] = dirs();
        let git_brew = fake_git(brew.path());
        let probe = FakeProbe::default().with(&git_brew, "2.46.0");
        let options = ResolveOptions {
            search_path: search_path(&[brew.path()]),
            apple_stub_path: Some(PathBuf::from("/usr/bin/git")),
            ..Default::default()
        };

        resolve_with(&options, &probe).unwrap();

        assert_eq!(probe.xcode_calls.get(), 0);
    }

    #[test]
    fn from_env_uses_process_path_and_platform_stub() {
        let options = ResolveOptions::from_env(Some(PathBuf::from("/opt/git/bin/git")));

        assert_eq!(
            options.settings_path,
            Some(PathBuf::from("/opt/git/bin/git"))
        );
        assert_eq!(options.search_path, env::var_os("PATH"));
        assert_eq!(options.bundled_path, bundled_git_path());
        if cfg!(target_os = "macos") {
            assert_eq!(options.apple_stub_path, Some(PathBuf::from("/usr/bin/git")));
        } else {
            assert_eq!(options.apple_stub_path, None);
        }
    }

    // ---- real processes ----------------------------------------------------
    //
    // These spawn processes, so they take the crate-wide lock: on Linux,
    // writing a script while another test thread forks can make exec fail
    // with ETXTBSY.

    use crate::test_support::SPAWN_LOCK;

    /// Writes an executable fake `git` that prints `stdout`.
    #[cfg(unix)]
    fn script_git(dir: &Path, stdout: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("git");
        fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{stdout}'\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_resolves_fake_path_scripts() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let [old, failing, good] = dirs();
        script_git(old.path(), "git version 2.20.1");
        let failing_git = failing.path().join("git");
        fs::write(&failing_git, "#!/bin/sh\necho boom >&2\nexit 3\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&failing_git, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let git_good = script_git(good.path(), "git version 2.45.2");
        let options = ResolveOptions {
            search_path: search_path(&[old.path(), failing.path(), good.path()]),
            ..Default::default()
        };

        let found = resolve(&options).unwrap();

        assert_eq!(found.path, git_good);
        assert_eq!(found.version, v(2, 45, 2));

        let err = SystemProbe.version_output(&failing_git).unwrap_err();
        assert!(
            matches!(err, GitBinaryError::VersionFailed { ref stderr, .. } if stderr.contains("boom")),
            "{err:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn system_probe_reads_version_on_windows() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let [dir] = dirs();
        // PATH lookup only accepts git.exe; a batch file is enough to check
        // that the probe spawns a program and parses what it prints.
        let fake = dir.path().join("fake-git.cmd");
        fs::write(&fake, "@echo git version 2.45.1.windows.1\r\n").unwrap();

        let output = SystemProbe.version_output(&fake).unwrap();

        assert_eq!(GitVersion::parse(&output).unwrap(), v(2, 45, 1));
    }

    #[test]
    fn system_probe_reports_spawn_failure() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let [dir] = dirs();
        let err = SystemProbe
            .version_output(&dir.path().join("no-such-git"))
            .unwrap_err();
        assert!(matches!(err, GitBinaryError::Spawn { .. }), "{err:?}");
    }

    /// CI images on all three OSes ship a git ≥ 2.30 (with the Xcode CLT on
    /// macOS), so resolving against the real environment must succeed.
    #[test]
    fn resolves_the_machine_git() {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let found = resolve(&ResolveOptions::from_env(None)).unwrap();
        assert_eq!(found.source, GitSource::Path);
        assert!(found.version >= MIN_GIT_VERSION);
        assert!(found.path.is_absolute());
    }
}
