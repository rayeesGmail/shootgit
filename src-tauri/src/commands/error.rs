//! The error every command returns (SPEC §4 IPC contract).
//!
//! The frontend gets `{ kind, message, fix }`: `kind` to decide what to do,
//! an English `message` to show, and for errors the user has to fix outside
//! the app, the command that does it. `message` is the engine error with its
//! causes, never a raw stderr dump. Until `t()` lands (P6-20) the UI shows
//! `message` as is; it can then key its own text off `kind`.

use std::fmt;

use git_engine::error::GitError;
use git_engine::git_binary::GitBinaryError;
use git_engine::repo::{safe_directory_command, RepoId};
use serde::Serialize;

/// What went wrong, for the frontend to branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No usable git: none in settings, on `PATH` or bundled, or the one in
    /// settings is unusable (SPEC §5 Git binary resolution).
    GitNotFound,
    /// The path is not in a git repository, or its `.git` file is broken.
    NotARepository,
    /// The path is in a bare repository or inside a `.git` directory, which
    /// have no working tree to show.
    NoWorkTree,
    /// git refuses to work in the repository because another user owns it;
    /// `fix` holds the `safe.directory` command the user may run.
    DubiousOwnership,
    /// No open repository has this id (it was never opened in this run).
    UnknownRepo,
    /// A newer request for the same thing superseded this one (SPEC §4
    /// Low-resource operation, rule 4). Not an error to show.
    Cancelled,
    /// Anything else; `message` says what.
    Failed,
}

/// The error of every command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct CommandError {
    pub kind: ErrorKind,
    /// What happened, in English, with its causes.
    pub message: String,
    /// A command the user can run to fix it, for errors the app must not
    /// fix itself (it never edits the user's git config).
    pub fix: Option<String>,
}

impl CommandError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            fix: None,
        }
    }

    /// No open repository has `id`.
    pub fn unknown_repo(id: RepoId) -> Self {
        Self::new(
            ErrorKind::UnknownRepo,
            format!("no open repository has the id {id}"),
        )
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<GitError> for CommandError {
    fn from(error: GitError) -> Self {
        let kind = match &error {
            GitError::Binary(_) => ErrorKind::GitNotFound,
            GitError::NotARepository { .. } | GitError::InvalidGitFile { .. } => {
                ErrorKind::NotARepository
            }
            GitError::NoWorkTree { .. } => ErrorKind::NoWorkTree,
            GitError::DubiousOwnership { .. } => ErrorKind::DubiousOwnership,
            GitError::Cancelled => ErrorKind::Cancelled,
            _ => ErrorKind::Failed,
        };
        let fix = match &error {
            GitError::DubiousOwnership { path } => Some(safe_directory_command(path)),
            _ => None,
        };
        Self {
            kind,
            message: with_causes(&error),
            fix,
        }
    }
}

impl From<GitBinaryError> for CommandError {
    fn from(error: GitBinaryError) -> Self {
        GitError::Binary(error).into()
    }
}

/// `error` followed by each of its sources: "could not access /x: No such
/// file or directory". Variants marked `transparent` show their source's
/// text as their own, so a source equal to the text so far is skipped.
pub(crate) fn with_causes(error: &(dyn std::error::Error + 'static)) -> String {
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
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn dubious_ownership_carries_the_safe_directory_fix() {
        let path = PathBuf::from(if cfg!(windows) {
            r"C:\srv\theirs"
        } else {
            "/srv/theirs"
        });

        let error = CommandError::from(GitError::DubiousOwnership { path: path.clone() });

        assert_eq!(error.kind, ErrorKind::DubiousOwnership);
        assert_eq!(error.fix, Some(safe_directory_command(&path)));
        assert!(error.message.contains("another user owns it"), "{error}");
        assert_eq!(
            serde_json::to_value(&error).unwrap()["kind"],
            "dubious_ownership"
        );
    }

    #[test]
    fn io_errors_keep_their_cause() {
        let error = CommandError::from(GitError::Io {
            path: PathBuf::from("/nowhere"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such thing"),
        });

        assert_eq!(error.kind, ErrorKind::Failed);
        assert!(error.message.ends_with(": no such thing"), "{error}");
        assert_eq!(error.fix, None);
    }

    #[test]
    fn kinds_follow_the_engine_error() {
        let kind = |error: GitError| CommandError::from(error).kind;
        assert_eq!(
            kind(GitError::NotARepository {
                path: PathBuf::from("/x")
            }),
            ErrorKind::NotARepository
        );
        assert_eq!(
            kind(GitError::NoWorkTree {
                path: PathBuf::from("/x")
            }),
            ErrorKind::NoWorkTree
        );
        assert_eq!(kind(GitError::Cancelled), ErrorKind::Cancelled);
        assert_eq!(kind(GitError::Aborted), ErrorKind::Failed);
        assert_eq!(
            CommandError::from(GitBinaryError::NotExecutable {
                path: PathBuf::from("/x/git")
            })
            .kind,
            ErrorKind::GitNotFound
        );
    }

    #[test]
    fn it_serialises_as_kind_message_and_fix() {
        assert_eq!(
            serde_json::to_value(CommandError::new(ErrorKind::UnknownRepo, "gone")).unwrap(),
            serde_json::json!({ "kind": "unknown_repo", "message": "gone", "fix": null })
        );
    }
}
