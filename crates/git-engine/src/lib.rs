//! All local Git logic: status, diff, staging, commit, log, branches, rebase, merge, stash, shelve. Pure library with no Tauri or UI dependency.

pub mod error;
pub mod git_binary;
pub mod login_shell;
pub mod process;
pub mod repo;
pub mod repo_actor;
pub mod runtime;
pub mod status;
pub mod watcher;

#[cfg(test)]
pub(crate) mod test_support {
    /// Held by every test that spawns a process or writes an executable.
    ///
    /// On Linux, writing a script while another test thread forks can make
    /// exec fail with ETXTBSY, so all spawning tests in this crate share one
    /// lock.
    pub(crate) static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "git-engine");
    }
}
