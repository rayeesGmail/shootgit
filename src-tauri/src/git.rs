//! Which git executable the app runs (SPEC §5 Git binary resolution).
//!
//! Resolution spawns `git --version` for each candidate, so the result is
//! cached for the session. It is resolved on first use, not at startup
//! (§4 Low-resource operation, rule 9), and again whenever the git path in
//! settings has changed since.

use std::path::PathBuf;

use git_engine::git_binary::{self, GitBinary, GitBinaryError, ResolveOptions};
use tokio::sync::Mutex;

/// The session's resolved git, keyed by the settings path it was resolved
/// with.
#[derive(Debug, Default)]
pub struct GitResolver {
    resolved: Mutex<Option<(Option<PathBuf>, GitBinary)>>,
}

impl GitResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// The git to run, given the path configured in settings
    /// (`Settings::git_path`, tier 1 of §5). The login-shell `PATH` is
    /// awaited first on macOS (tier 2). A failure is not cached: installing
    /// git or fixing the setting works without a restart.
    pub async fn resolve(&self, configured: Option<PathBuf>) -> Result<GitBinary, GitBinaryError> {
        let mut resolved = self.resolved.lock().await;
        if let Some((key, git)) = resolved.as_ref() {
            if *key == configured {
                return Ok(git.clone());
            }
        }
        let options = ResolveOptions::from_login_shell_env(configured.clone()).await;
        let git = git_binary::resolve(&options).await?;
        tracing::info!(path = %git.path.display(), version = %git.version, source = ?git.source, "resolved git");
        *resolved = Some((configured, git.clone()));
        Ok(git)
    }
}
