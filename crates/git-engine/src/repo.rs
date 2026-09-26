//! The repository handle every public engine operation takes (CLAUDE.md
//! Conventions), and [`open_repo`], which finds the repository that contains
//! a path.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::process::GitCommand;

/// One repository: the git executable that operates on it, the root of its
/// working tree, and where its git data lives.
///
/// A linked worktree (`git worktree add`) has a git directory of its own,
/// `.git/worktrees/<name>` in the main repository, holding its `HEAD`,
/// `index` and any in-progress merge or rebase, and shares everything else
/// (objects, refs, `packed-refs`, config) through the common directory. In
/// every other working tree the two are the same directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
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
            git: git.into(),
            common_dir: git_dir.clone(),
            git_dir,
            workdir,
        }
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
/// `GIT_CEILING_DIRECTORIES`, git's stop at file-system boundaries,
/// `core.worktree`, and `safe.directory` (a repository owned by another user
/// opens here, and git then refuses to work in it).
///
/// Cost: a few `stat` calls and reads of small files per directory walked.
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
