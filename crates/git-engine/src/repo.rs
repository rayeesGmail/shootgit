//! The repository handle every public engine operation takes (CLAUDE.md
//! Conventions), and [`open_repo`], which finds the repository that contains
//! a path.

use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::GitError;
use crate::process::GitCommand;

/// Names one repository handle for as long as the process runs: the `id` of
/// SPEC §5 `RepoInfo`, and how the app's IPC commands and the
/// `repo-changed` event address an open repository (ADR 0008).
///
/// Every [`Repo`] gets a new id when it is created ([`Repo::new`],
/// [`open_repo`]); its clones share it. Ids are never reused within a
/// process and mean nothing in another one, so they must not be persisted:
/// the path is what identifies a repository across launches. The same
/// working tree opened twice gets two ids, so a caller that keeps one handle
/// per repository (the app does) must look the path up before opening it
/// again.
///
/// On the wire it is a plain number.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
#[serde(transparent)]
pub struct RepoId(u32);

impl RepoId {
    /// A new id, different from every id handed out before in this process.
    ///
    /// Four billion repository handles per run is out of reach; if it ever
    /// wrapped, a stale id could name a newer handle, which the app would
    /// still check against its registry.
    fn next() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
impl RepoId {
    /// A fixed id for unit tests that build models by hand.
    pub(crate) const fn for_tests(raw: u32) -> Self {
        Self(raw)
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One repository: the git executable that operates on it, the root of its
/// working tree, and where its git data lives.
///
/// A linked worktree (`git worktree add`) has a git directory of its own,
/// `.git/worktrees/<name>` in the main repository, holding its `HEAD`,
/// `index` and any in-progress merge or rebase, and shares everything else
/// (objects, refs, `packed-refs`, config) through the common directory. In
/// every other working tree the two are the same directory.
///
/// Each handle has its own [`RepoId`]; clones share it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    id: RepoId,
    git: PathBuf,
    workdir: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl Repo {
    /// A repository whose git directory is `<workdir>/.git`, taken on trust:
    /// nothing is read from disk. `git` is the executable to run (normally
    /// [`GitBinary::path`](crate::git_binary::GitBinary)).
    ///
    /// For a path the user chose, use [`open_repo`]: it checks that there is
    /// a repository, finds its root from anywhere inside it, and follows the
    /// `.git` file of a linked worktree or submodule.
    pub fn new(git: impl Into<PathBuf>, workdir: impl Into<PathBuf>) -> Self {
        let workdir = workdir.into();
        let git_dir = workdir.join(".git");
        Self {
            id: RepoId::next(),
            git: git.into(),
            common_dir: git_dir.clone(),
            git_dir,
            workdir,
        }
    }

    /// This handle's id, shared by its clones.
    pub fn id(&self) -> RepoId {
        self.id
    }

    /// The git executable.
    pub fn git(&self) -> &Path {
        &self.git
    }

    /// The root of the working tree.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// This working tree's git directory: `HEAD`, `index`, and the markers
    /// of an in-progress merge, rebase or cherry-pick.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// The directory all working trees of the repository share: objects,
    /// refs, `packed-refs`, config. The same as [`git_dir`](Self::git_dir)
    /// except in a linked worktree.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// A [`GitCommand`] that runs this repository's git in its working tree.
    pub(crate) fn git_command(&self) -> GitCommand {
        let mut git = GitCommand::new(&self.git);
        git.current_dir(&self.workdir);
        git
    }

    /// [`git_command`](Self::git_command) for a spawn whose failure goes
    /// through [`classify_failure`]: git's messages come out in English
    /// whatever the user's locale, so they can be recognised (ADR 0009).
    ///
    /// `LC_ALL=C` changes only the language of git's messages here: the
    /// machine-readable `-z` output these spawns parse is locale-independent
    /// bytes, and with the C locale gettext ignores `LANGUAGE` too.
    pub(crate) fn classified_git_command(&self) -> GitCommand {
        let mut git = self.git_command();
        git.env("LC_ALL", "C");
        git
    }
}

/// The command that tells git to trust the working tree at `path` although
/// another user owns it (`safe.directory`, git-config(1)), quoted for the
/// user's shell: single quotes on Unix, double quotes and forward slashes on
/// Windows, where git itself prints the path that way.
///
/// It edits the user's global git config, so the app only shows it and never
/// runs it (CLAUDE.md Safety rules).
pub fn safe_directory_command(path: &Path) -> String {
    let path = path.to_string_lossy();
    let quoted = if cfg!(windows) {
        format!("\"{}\"", path.replace('\\', "/"))
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    };
    format!("git config --global --add safe.directory {quoted}")
}

/// Recognises git's refusal to work in a repository owned by another user
/// and turns it into [`GitError::DubiousOwnership`]; every other error is
/// returned unchanged.
///
/// The refusal is exit code 128 with a message that is the only sign of it
/// (git has no machine-readable form), so this is one of the few places
/// that read git's stderr, from a spawn made with
/// [`Repo::classified_git_command`] so the text is English (ADR 0009). Git
/// 2.35.3 and later say "detected dubious ownership"; the CVE-2022-24765
/// backports down to 2.30.3 say "unsafe repository".
pub(crate) fn classify_failure(repo: &Repo, error: GitError) -> GitError {
    match error {
        GitError::Failed {
            exit_code: Some(128),
            ref stderr,
            ..
        } if stderr.contains("detected dubious ownership")
            || stderr.contains("unsafe repository") =>
        {
            GitError::DubiousOwnership {
                path: repo.workdir.clone(),
            }
        }
        other => other,
    }
}

/// Opens the repository that contains `path`: the root of a working tree,
/// any directory or file below it, or a linked worktree. `git` is the
/// executable later operations run.
///
/// Discovery follows git's own (`setup_git_directory` in git's `setup.c`)
/// without spawning git. From `path` upwards, the first directory whose
/// `.git` is a repository is the root of the working tree:
///
/// - a `.git` directory counts if `HEAD` names a ref under `refs/` or a
///   commit and the common directory has `objects/` and `refs/`; one that
///   does not is skipped, as git skips it;
/// - a `.git` file must read `gitdir: <path>`, absolute or relative to the
///   file's directory, and point at a git directory. If it does not,
///   discovery stops with [`GitError::InvalidGitFile`], as git's does,
///   rather than open a repository further up.
///
/// A directory that is itself a git directory (a bare repository, or a path
/// inside some `.git`) stops discovery with [`GitError::NoWorkTree`]. No
/// repository at all is [`GitError::NotARepository`], and a `path` that
/// cannot be read is [`GitError::Io`].
///
/// The paths in the result are canonical: symlinks resolved and, on
/// Windows, no `\\?\` prefix where the plain form names the same file. So
/// they match what git itself reports, and what the OS reports to a file
/// watcher.
///
/// Nothing is taken from the environment: the repository is the one `path`
/// is in, never one an inherited `GIT_DIR` names, and every git spawn clears
/// such variables too (ADR 0007). Not honoured yet:
/// `GIT_CEILING_DIRECTORIES`, git's stop at file-system boundaries and
/// `core.worktree`. Ownership (`safe.directory`) is not checked here either:
/// a repository owned by another user opens, and the first git command run
/// in it (normally [`status`](crate::status::status)) fails with
/// [`GitError::DubiousOwnership`], which names the fix.
///
/// Cost: a few `stat` calls and reads of small files per directory walked.
/// That is blocking file-system work, slow on a network drive, so async
/// callers run it on the blocking pool (the app does,
/// `tokio::task::spawn_blocking`).
pub fn open_repo(git: impl Into<PathBuf>, path: impl AsRef<Path>) -> Result<Repo, GitError> {
    let path = path.as_ref();
    let start = canonical(path)?;
    let start = match start.parent() {
        Some(parent) if !start.is_dir() => parent.to_path_buf(),
        _ => start,
    };

    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        let found = match fs::metadata(&dot_git) {
            Ok(meta) if meta.is_file() => Some(read_git_file(&dot_git)?),
            Ok(meta) if meta.is_dir() => common_dir_of(&dot_git).map(|common| (dot_git, common)),
            // Missing or unreadable: git looks further up, and so do we.
            _ => None,
        };
        if let Some((git_dir, common_dir)) = found {
            tracing::debug!(workdir = ?dir, ?git_dir, "opened repository");
            return Ok(Repo {
                id: RepoId::next(),
                git: git.into(),
                workdir: dir.to_path_buf(),
                git_dir: canonical(&git_dir)?,
                common_dir: canonical(&common_dir)?,
            });
        }
        if common_dir_of(dir).is_some() {
            return Err(GitError::NoWorkTree {
                path: path.to_path_buf(),
            });
        }
    }
    Err(GitError::NotARepository {
        path: path.to_path_buf(),
    })
}

/// `path` with symlinks resolved, without Windows' `\\?\` prefix where the
/// plain form names the same file.
fn canonical(path: &Path) -> Result<PathBuf, GitError> {
    dunce::canonicalize(path).map_err(|source| GitError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The git directory a `.git` file points at, and that directory's common
/// directory (git's `read_gitfile_gently`).
fn read_git_file(file: &Path) -> Result<(PathBuf, PathBuf), GitError> {
    let invalid = |reason: String| GitError::InvalidGitFile {
        path: file.to_path_buf(),
        reason,
    };
    let contents = fs::read(file).map_err(|source| GitError::Io {
        path: file.to_path_buf(),
        source,
    })?;
    let target = contents
        .strip_prefix(b"gitdir: ")
        .ok_or_else(|| invalid("it does not start with `gitdir: `".to_owned()))?;
    let target = trim_line_end(target);
    if target.is_empty() {
        return Err(invalid("it names no directory".to_owned()));
    }
    let target = path_from_bytes(target)
        .ok_or_else(|| invalid("the directory it names is not UTF-8".to_owned()))?;
    // `join` keeps an absolute target as it is.
    let git_dir = match file.parent() {
        Some(base) => base.join(target),
        None => target,
    };
    let common_dir = common_dir_of(&git_dir)
        .ok_or_else(|| invalid(format!("{} is not a git directory", git_dir.display())))?;
    Ok((git_dir, common_dir))
}

/// If `dir` is a git directory, its common directory (git's
/// `is_git_directory` and `get_common_dir`).
///
/// A git directory has a valid `HEAD`, and its common directory, named by a
/// `commondir` file (absolute, or relative to `dir`) or else `dir` itself,
/// has `objects/` and `refs/`.
fn common_dir_of(dir: &Path) -> Option<PathBuf> {
    if !head_is_valid(&dir.join("HEAD")) {
        return None;
    }
    let common_dir = match fs::read(dir.join("commondir")) {
        Ok(contents) => dir.join(path_from_bytes(trim_line_end(&contents))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => dir.to_path_buf(),
        Err(_) => return None,
    };
    let valid = common_dir.join("objects").is_dir() && common_dir.join("refs").is_dir();
    valid.then_some(common_dir)
}

/// Whether `HEAD` names a ref under `refs/` (`ref: refs/heads/main`, or an
/// old-style symlink) or starts with a commit id (git's
/// `validate_headref`).
fn head_is_valid(head: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(head) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return fs::read_link(head).is_ok_and(|target| target.starts_with("refs"));
    }
    // git reads at most 255 bytes of HEAD.
    let mut contents = Vec::with_capacity(256);
    let read = fs::File::open(head).and_then(|file| file.take(255).read_to_end(&mut contents));
    if read.is_err() {
        return false;
    }
    if let Some(target) = contents.strip_prefix(b"ref:") {
        if target.trim_ascii_start().starts_with(b"refs/") {
            return true;
        }
    }
    starts_with_object_id(&contents)
}

/// Whether `bytes` start with an object id, as git's `get_oid_hex_any`
/// accepts one: 40 hex digits, a SHA-1 id or the start of a SHA-256 one.
fn starts_with_object_id(bytes: &[u8]) -> bool {
    bytes
        .get(..40)
        .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
}

/// `bytes` without the line ending git writes after a path (`\n`, or `\r\n`
/// on Windows). Git strips only these, so other trailing whitespace stays
/// part of the path.
fn trim_line_end(mut bytes: &[u8]) -> &[u8] {
    while let Some(rest) = bytes
        .strip_suffix(b"\n")
        .or_else(|| bytes.strip_suffix(b"\r"))
    {
        bytes = rest;
    }
    bytes
}

/// A path git wrote into a file: raw bytes on Unix, UTF-8 on Windows (Git for
/// Windows converts paths itself). `None` if it is not UTF-8 on Windows.
#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

/// A path git wrote into a file: raw bytes on Unix, UTF-8 on Windows (Git for
/// Windows converts paths itself). `None` if it is not UTF-8 on Windows.
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_repository_handle_gets_its_own_id_and_clones_share_it() {
        let first = Repo::new("git", "/work/a");
        let second = Repo::new("git", "/work/a");
        assert_ne!(first.id(), second.id());
        assert_eq!(first.clone().id(), first.id());
    }

    #[test]
    fn a_repository_id_crosses_ipc_as_a_plain_number() {
        let repo = Repo::new("git", "/work/a");
        let json = serde_json::to_string(&repo.id()).unwrap();
        assert_eq!(json, repo.id().to_string());
        let back: RepoId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, repo.id());
    }

    #[test]
    fn the_safe_directory_fix_quotes_the_path_for_a_shell() {
        if cfg!(windows) {
            assert_eq!(
                safe_directory_command(Path::new(r"C:\Users\me\my repo")),
                r#"git config --global --add safe.directory "C:/Users/me/my repo""#
            );
        } else {
            assert_eq!(
                safe_directory_command(Path::new("/srv/their repo")),
                "git config --global --add safe.directory '/srv/their repo'"
            );
            assert_eq!(
                safe_directory_command(Path::new("/srv/it's")),
                r"git config --global --add safe.directory '/srv/it'\''s'"
            );
        }
    }

    #[test]
    fn only_gits_ownership_refusal_counts_as_dubious_ownership() {
        let repo = Repo::new("git", "/srv/theirs");
        let failed = |exit_code, stderr: &str| GitError::Failed {
            args: vec!["status".to_owned()],
            exit_code,
            stderr: stderr.to_owned(),
        };
        // git 2.35.3 and later.
        let refused = failed(
            Some(128),
            "fatal: detected dubious ownership in repository at '/srv/theirs'\n\
             To add an exception for this directory, call:\n\n\
             \tgit config --global --add safe.directory /srv/theirs\n",
        );
        assert!(matches!(
            classify_failure(&repo, refused),
            GitError::DubiousOwnership { ref path } if path == Path::new("/srv/theirs")
        ));
        // git 2.30.3 to 2.35.2 (the CVE-2022-24765 backports).
        let older = failed(
            Some(128),
            "fatal: unsafe repository ('/srv/theirs' is owned by someone else)\n",
        );
        assert!(matches!(
            classify_failure(&repo, older),
            GitError::DubiousOwnership { .. }
        ));
        // Anything else is left alone.
        for other in [
            failed(Some(128), "fatal: not a git repository\n"),
            failed(Some(1), "detected dubious ownership"),
            failed(None, "detected dubious ownership"),
            GitError::Cancelled,
        ] {
            let before = format!("{other:?}");
            assert_eq!(format!("{:?}", classify_failure(&repo, other)), before);
        }
    }

    #[test]
    fn line_endings_are_trimmed_but_other_trailing_whitespace_is_kept() {
        assert_eq!(trim_line_end(b"../x\n"), b"../x");
        assert_eq!(trim_line_end(b"../x\r\n"), b"../x");
        assert_eq!(trim_line_end(b"../x \n"), b"../x ");
        assert_eq!(trim_line_end(b"\n"), b"");
    }

    #[test]
    fn an_object_id_starts_with_40_hex_digits() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert!(starts_with_object_id(sha1.as_bytes()));
        assert!(starts_with_object_id(format!("{sha1}\n").as_bytes()));
        assert!(starts_with_object_id(
            format!("{sha1}{}", &sha1[..24]).as_bytes()
        ));
        assert!(!starts_with_object_id(&sha1.as_bytes()[..39]));
        assert!(!starts_with_object_id(
            b"0123456789abcdef0123456789abcdef0123456g"
        ));
    }

    #[test]
    fn head_must_name_a_ref_under_refs_or_a_commit() {
        let dir = tempfile::tempdir().unwrap();
        let head = dir.path().join("HEAD");
        let check = |contents: &str| {
            fs::write(&head, contents).unwrap();
            head_is_valid(&head)
        };

        assert!(check("ref: refs/heads/main\n"));
        assert!(check("ref:\trefs/heads/.invalid\n"));
        assert!(check("0123456789abcdef0123456789abcdef01234567\n"));
        assert!(!check("ref: heads/main\n"));
        assert!(!check("main\n"));
        assert!(!check(""));
        fs::remove_file(&head).unwrap();
        assert!(!head_is_valid(&head));
    }
}
