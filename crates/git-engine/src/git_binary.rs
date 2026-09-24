//! Which `git` executable the app runs (SPEC §5 "Git binary resolution",
//! §9 platform rules). Candidates are tried in this order:
//!
//! 1. the path configured in settings (stored by P0-11, edited in P1-22);
//! 2. `git` in each PATH directory in turn, if it is 2.30 or newer. On macOS
//!    the caller passes the login-shell PATH (P0-07), not the one a GUI app
//!    inherits;
//! 3. the git bundled with the app ([`BundledGit`]), which no build ships yet.
//!
//! Apple's `/usr/bin/git` is a stub that opens an installer dialog when the
//! Xcode Command Line Tools are missing, so it is never run unless
//! `xcode-select -p` says they are installed (step 4 of §5). That applies to
//! whichever step reaches it.
//!
//! A candidate that does not qualify is not an error: the search moves on and
//! records why in [`Skipped`], so the UI can explain why a configured path was
//! ignored. Only when nothing qualifies does [`resolve`] fail.
//!
//! Apart from reading file metadata, the decisions are pure: running
//! `<git> --version` and `xcode-select -p` goes through a [`GitProbe`], and
//! tests pass a fake one. [`SystemProbe`] is the real one.

mod probe;
mod version;

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use probe::{GitProbe, ProbeError, SystemProbe};
pub use version::{GitVersion, UnrecognisedVersion, MINIMUM_GIT_VERSION};

use crate::GitError;

/// Apple's git stub on macOS.
pub const XCODE_STUB: &str = "/usr/bin/git";

/// What Windows tries when PATHEXT is unset: the executable part of its
/// default value.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The most of a program's output kept for a message.
const EXCERPT_CHARS: usize = 300;

/// `text` cut to [`EXCERPT_CHARS`] characters, for messages that quote what a
/// program printed.
fn excerpt(text: &str) -> String {
    text.chars().take(EXCERPT_CHARS).collect()
}

/// Where a git came from, in the order the sources are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitSource {
    /// The path configured in settings.
    Settings,
    /// A directory on PATH.
    Path,
    /// The git shipped with the app.
    Bundled,
}

impl fmt::Display for GitSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Settings => "the configured git",
            Self::Path => "git on PATH",
            Self::Bundled => "the bundled git",
        })
    }
}

/// The git the app ships as a last resort (SPEC §5 step 3, §9): MinGit on
/// Windows, git inside the macOS app and the Linux AppImage. The .deb, .rpm
/// and Flatpak builds use the system git.
///
/// No build bundles git yet. Packaging it is release work, and whether macOS
/// bundles git at all is an open question (§12). Until then every build
/// passes [`BundledGit::NotShipped`]; [`resolve`] already handles
/// [`BundledGit::At`], so enabling it later only means passing the path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BundledGit {
    /// This build ships no git.
    #[default]
    NotShipped,
    /// Where this build's git should be, e.g. under the app's resource dir.
    /// If nothing is there the resolver reports it as missing.
    At(PathBuf),
}

/// What [`resolve`] searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSearch {
    /// Step 1: the git binary override from settings. `None` or an empty path
    /// means none is configured.
    pub settings_path: Option<PathBuf>,
    /// Step 2: the PATH to search, in this platform's list syntax. Relative
    /// and empty entries are ignored, so the directory the app happened to be
    /// started from is never searched.
    pub search_path: Option<OsString>,
    /// The extensions tried after `git` in each PATH directory, in PATHEXT
    /// syntax (`.COM;.EXE;.BAT;.CMD`). Set on Windows only; `None` looks for
    /// a plain `git`, as Unix does.
    pub path_extensions: Option<OsString>,
    /// Step 4: the Apple stub that is never run unless the Xcode tools are
    /// installed. [`XCODE_STUB`] on macOS, `None` elsewhere.
    pub xcode_stub: Option<PathBuf>,
    /// Step 3: the bundled git.
    pub bundled: BundledGit,
}

impl GitSearch {
    /// The search for this process: PATH (and PATHEXT on Windows) from its
    /// environment, and the Xcode stub rule on macOS.
    ///
    /// On macOS a GUI app inherits launchd's minimal PATH; replace
    /// `search_path` with the login-shell PATH (P0-07) before resolving.
    pub fn from_process_env(settings_path: Option<PathBuf>, bundled: BundledGit) -> Self {
        Self {
            settings_path,
            search_path: std::env::var_os("PATH"),
            path_extensions: cfg!(windows)
                .then(|| std::env::var_os("PATHEXT").unwrap_or_else(|| DEFAULT_PATHEXT.into())),
            xcode_stub: cfg!(target_os = "macos").then(|| PathBuf::from(XCODE_STUB)),
            bundled,
        }
    }
}

/// The git the app will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGit {
    pub source: GitSource,
    /// The executable as found, e.g. `/opt/homebrew/bin/git`. Not
    /// canonicalised, so it reads the way the user configured or sees it.
    pub path: PathBuf,
    pub version: GitVersion,
    /// Every candidate passed over before this one, in the order tried.
    pub skipped: Vec<Skipped>,
}

impl ResolvedGit {
    /// Why the git configured in settings was not used, if one was
    /// configured and passed over. The settings screen shows this next to
    /// the override.
    pub fn ignored_settings_path(&self) -> Option<&Skipped> {
        self.skipped
            .iter()
            .find(|skipped| skipped.source == GitSource::Settings)
    }
}

/// A candidate that was passed over, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub source: GitSource,
    pub path: PathBuf,
    pub reason: SkipReason,
}

impl fmt::Display for Skipped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}: {}",
            self.source,
            self.path.display(),
            self.reason
        )
    }
}

/// Why a candidate was passed over.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkipReason {
    /// The configured path is relative, so what it names would depend on the
    /// directory the app was started from.
    #[error("the path is not absolute")]
    NotAbsolute,
    #[error("nothing exists at this path")]
    Missing,
    /// It exists but its metadata could not be read.
    #[error("it cannot be read: {0}")]
    Inaccessible(String),
    /// A directory, or (on Unix) a file without execute permission.
    #[error("it is not an executable file")]
    NotExecutable,
    /// Apple's stub, and the Xcode Command Line Tools are not installed.
    /// Running it would open an installer dialog.
    #[error(
        "it is Apple's placeholder for git, and the Xcode Command Line Tools are not installed"
    )]
    XcodeStub,
    /// Running `--version` failed.
    #[error("running `--version` failed: {0}")]
    ProbeFailed(#[from] ProbeError),
    #[error(transparent)]
    UnrecognisedVersion(#[from] UnrecognisedVersion),
    #[error("git {0} is older than the minimum, {min}", min = MINIMUM_GIT_VERSION)]
    TooOld(GitVersion),
}

impl SkipReason {
    /// Nothing usable is there at all. On PATH that is the normal case for
    /// most directories and is not reported.
    fn is_absence(&self) -> bool {
        matches!(self, Self::Missing | Self::Inaccessible(_))
    }
}

/// Errors from git binary resolution.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitBinaryError {
    /// No candidate qualified. `skipped` lists every candidate tried, in
    /// order, with the reason it was passed over. It is empty when nothing is
    /// configured, no PATH directory has a git and no git is bundled.
    #[error(
        "no usable git was found ({min} or newer is required){}",
        SkippedList(.skipped),
        min = MINIMUM_GIT_VERSION
    )]
    NotFound { skipped: Vec<Skipped> },
}

/// Formats the candidates in a [`GitBinaryError::NotFound`] message.
struct SkippedList<'a>(&'a [Skipped]);

impl fmt::Display for SkippedList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str(": there is no git on PATH");
        }
        for (index, skipped) in self.0.iter().enumerate() {
            f.write_str(if index == 0 { ": " } else { "; " })?;
            write!(f, "{skipped}")?;
        }
        Ok(())
    }
}

/// Finds the git to run: the first candidate, in [`GitSearch`] order, that is
/// an executable file printing a git version of at least
/// [`MINIMUM_GIT_VERSION`].
///
/// Each distinct binary is run at most once (a PATH that names one directory
/// twice, or through a symlink, costs no second spawn), the search stops at
/// the first git that qualifies, and `xcode-select -p` runs at most once, only
/// when a candidate is the Xcode stub.
///
/// Returns [`GitBinaryError::NotFound`] when nothing qualifies.
pub async fn resolve(search: &GitSearch, probe: &impl GitProbe) -> Result<ResolvedGit, GitError> {
    let mut resolution = Resolution::new(search.xcode_stub.as_deref(), probe);

    if let Some(path) = search
        .settings_path
        .as_deref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        if let Some(found) = resolution.consider(GitSource::Settings, path).await {
            return Ok(found);
        }
    }

    let names = executable_names(search.path_extensions.as_deref());
    for dir in search.search_path.iter().flat_map(std::env::split_paths) {
        if !dir.is_absolute() {
            continue;
        }
        for name in &names {
            if let Some(found) = resolution.consider(GitSource::Path, &dir.join(name)).await {
                return Ok(found);
            }
        }
    }

    if let BundledGit::At(path) = &search.bundled {
        if let Some(found) = resolution.consider(GitSource::Bundled, path).await {
            return Ok(found);
        }
    }

    Err(GitBinaryError::NotFound {
        skipped: resolution.skipped,
    }
    .into())
}

/// The file names to look for in each PATH directory: `git`, or with PATHEXT
/// `git` plus each listed extension in order. Extensions are lower-cased so
/// the resolved path reads the way it does on disk (Windows file names are
/// case-insensitive, and PATHEXT is upper-case by convention).
fn executable_names(path_extensions: Option<&OsStr>) -> Vec<OsString> {
    match path_extensions {
        None => vec![OsString::from("git")],
        Some(list) => list
            .to_string_lossy()
            .split(';')
            .map(str::trim)
            .filter(|extension| !extension.is_empty())
            .map(|extension| OsString::from(format!("git{}", extension.to_ascii_lowercase())))
            .collect(),
    }
}

/// The state of one [`resolve`] call.
struct Resolution<'a, P> {
    probe: &'a P,
    xcode_stub: Option<XcodeStub<'a>>,
    /// Asked lazily, at most once.
    xcode_tools_installed: Option<bool>,
    /// Canonical paths of every binary already considered.
    considered: HashSet<PathBuf>,
    skipped: Vec<Skipped>,
}

struct XcodeStub<'a> {
    path: &'a Path,
    /// Set when the stub exists, so a symlink to it is recognised too.
    canonical: Option<PathBuf>,
}

enum Verdict {
    Usable(GitVersion),
    /// The same binary was already considered under another path.
    AlreadyConsidered,
    Rejected(SkipReason),
}

impl<'a, P: GitProbe> Resolution<'a, P> {
    fn new(xcode_stub: Option<&'a Path>, probe: &'a P) -> Self {
        Self {
            probe,
            xcode_stub: xcode_stub.map(|path| XcodeStub {
                path,
                canonical: fs::canonicalize(path).ok(),
            }),
            xcode_tools_installed: None,
            considered: HashSet::new(),
            skipped: Vec::new(),
        }
    }

    /// Returns the candidate if it qualifies. Otherwise records why not
    /// (unless it is a PATH entry with simply nothing there) and returns
    /// `None`.
    async fn consider(&mut self, source: GitSource, path: &Path) -> Option<ResolvedGit> {
        match self.judge(path).await {
            Verdict::Usable(version) => Some(ResolvedGit {
                source,
                path: path.to_path_buf(),
                version,
                skipped: std::mem::take(&mut self.skipped),
            }),
            Verdict::AlreadyConsidered => None,
            Verdict::Rejected(reason) if source == GitSource::Path && reason.is_absence() => None,
            Verdict::Rejected(reason) => {
                self.skipped.push(Skipped {
                    source,
                    path: path.to_path_buf(),
                    reason,
                });
                None
            }
        }
    }

    async fn judge(&mut self, path: &Path) -> Verdict {
        let metadata = match metadata(path) {
            Ok(metadata) => metadata,
            Err(reason) => return Verdict::Rejected(reason),
        };
        let canonical = fs::canonicalize(path).ok();
        let identity = canonical.clone().unwrap_or_else(|| path.to_path_buf());
        if !self.considered.insert(identity) {
            return Verdict::AlreadyConsidered;
        }
        match self.qualify(path, &metadata, canonical.as_deref()).await {
            Ok(version) => Verdict::Usable(version),
            Err(reason) => Verdict::Rejected(reason),
        }
    }

    /// The checks that apply once a candidate is known to exist. Cheap ones
    /// first; the stub check comes before anything is run.
    async fn qualify(
        &mut self,
        path: &Path,
        metadata: &fs::Metadata,
        canonical: Option<&Path>,
    ) -> Result<GitVersion, SkipReason> {
        if !is_executable(metadata) {
            return Err(SkipReason::NotExecutable);
        }
        if self.is_xcode_stub(path, canonical) && !self.xcode_tools_installed().await {
            return Err(SkipReason::XcodeStub);
        }
        let output = self.probe.version_output(path).await?;
        let version = GitVersion::from_version_output(&output)?;
        if version.is_supported() {
            Ok(version)
        } else {
            Err(SkipReason::TooOld(version))
        }
    }

    fn is_xcode_stub(&self, path: &Path, canonical: Option<&Path>) -> bool {
        let Some(stub) = &self.xcode_stub else {
            return false;
        };
        path == stub.path
            || matches!(
                (canonical, stub.canonical.as_deref()),
                (Some(candidate), Some(stub)) if candidate == stub
            )
    }

    async fn xcode_tools_installed(&mut self) -> bool {
        if let Some(installed) = self.xcode_tools_installed {
            return installed;
        }
        let installed = self.probe.xcode_tools_installed().await;
        self.xcode_tools_installed = Some(installed);
        installed
    }
}

/// The metadata of what is at `path` (following symlinks), or why there is
/// nothing to consider there.
fn metadata(path: &Path) -> Result<fs::Metadata, SkipReason> {
    if !path.is_absolute() {
        return Err(SkipReason::NotAbsolute);
    }
    fs::metadata(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => SkipReason::Missing,
        _ => SkipReason::Inaccessible(error.to_string()),
    })
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

/// Windows has no execute bit: whether a file runs depends on its extension,
/// which the PATHEXT search already chose.
#[cfg(not(unix))]
fn is_executable(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
}
