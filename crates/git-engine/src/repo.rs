//! The repository handle every public engine operation takes (CLAUDE.md
//! Conventions).

use std::path::{Path, PathBuf};

use crate::process::GitCommand;

/// One repository: the git executable that operates on it and the root of
/// its working tree.
///
/// For now the caller names both. P0-09 adds `open_repo`, which discovers
/// the repository from any path inside it (worktree `.git` files included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    git: PathBuf,
    workdir: PathBuf,
}

impl Repo {
    /// `git` is the executable to run (normally
    /// [`GitBinary::path`](crate::git_binary::GitBinary)); `workdir` is the
    /// root of the working tree.
    pub fn new(git: impl Into<PathBuf>, workdir: impl Into<PathBuf>) -> Self {
        Self {
            git: git.into(),
            workdir: workdir.into(),
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

    /// A [`GitCommand`] that runs this repository's git in its working tree.
    pub(crate) fn git_command(&self) -> GitCommand {
        let mut git = GitCommand::new(&self.git);
        git.current_dir(&self.workdir);
        git
    }
}
