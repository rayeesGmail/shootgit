//! All local Git logic: status, diff, staging, commit, log, branches, rebase, merge, stash, shelve. Pure library with no Tauri or UI dependency.

pub mod error;
pub mod git_binary;
mod process;

pub use error::GitError;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "git-engine");
    }
}
