//! Parsing what `git --version` prints.

use std::fmt;

use super::excerpt;

/// The oldest git the app runs (SPEC §5). Older gits on PATH are skipped.
pub const MINIMUM_GIT_VERSION: GitVersion = GitVersion::new(2, 30, 0);

/// A git release, as `major.minor.patch`.
///
/// Only the three numbers are kept. Vendor suffixes (`.windows.1`,
/// `(Apple Git-146)`, `.vfs.0.0`) and pre-release tags (`.rc1`) are dropped,
/// so a release candidate compares equal to its release. That is deliberate:
/// the minimum is about which features exist, and an rc already has its
/// release's features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// `git --version` printed something that is not a git version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`--version` printed something other than a git version: {output:?}")]
pub struct UnrecognisedVersion {
    /// What it printed, trimmed and cut to a few hundred characters.
    pub output: String,
}

impl GitVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses the stdout of `git --version`, e.g.
    /// `git version 2.39.3 (Apple Git-146)` or
    /// `git version 2.45.1.windows.1`.
    ///
    /// The first non-blank line must start with `git version ` followed by at
    /// least `major.minor` in plain digits. A third number is the patch
    /// level; anything else there (`2.45.GIT`, a tarball build without tags)
    /// counts as patch 0.
    pub fn from_version_output(output: &str) -> Result<Self, UnrecognisedVersion> {
        let output = output.trim();
        parse(output).ok_or_else(|| UnrecognisedVersion {
            output: excerpt(output),
        })
    }

    /// Whether this is at least [`MINIMUM_GIT_VERSION`].
    pub fn is_supported(self) -> bool {
        self >= MINIMUM_GIT_VERSION
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn parse(output: &str) -> Option<GitVersion> {
    let first_line = output.lines().next()?;
    let number = first_line
        .strip_prefix("git version ")?
        .split_whitespace()
        .next()?;
    // Git turns the `-` of a `git describe` version into `.`, but accept
    // either.
    let mut parts = number.split(['.', '-']);
    let major = digits(parts.next()?)?;
    let minor = digits(parts.next()?)?;
    let patch = match parts.next() {
        Some(part) if is_digits(part) => part.parse().ok()?,
        _ => 0,
    };
    Some(GitVersion::new(major, minor, patch))
}

/// `part` as a number, if it is nothing but ASCII digits. (`str::parse`
/// alone would also accept a leading `+`.)
fn digits(part: &str) -> Option<u32> {
    if is_digits(part) {
        part.parse().ok()
    } else {
        None
    }
}

fn is_digits(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
}
