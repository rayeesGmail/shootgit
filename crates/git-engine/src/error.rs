//! The crate's error type. Public functions return `Result<T, GitError>`
//! (CLAUDE.md); each area has its own `thiserror` enum folded in here with
//! `#[from]`, so callers can match on the detail they care about.

use crate::git_binary::GitBinaryError;

/// Everything that can go wrong in `git-engine`.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// No usable git binary was found (SPEC §5 "Git binary resolution").
    #[error(transparent)]
    Binary(#[from] GitBinaryError),
}
