//! Asking a candidate binary for its version and, on macOS, asking whether the
//! Xcode developer tools are installed. The resolver's decisions depend only
//! on these answers, so tests swap [`SystemProbe`] for a fake.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use super::excerpt;
use crate::process::{self, ProcessError};

/// Runs the two commands the resolver needs answers from.
pub trait GitProbe {
    /// Runs `<git> --version` and returns what it printed on stdout.
    fn version_output(&self, git: &Path)
        -> impl Future<Output = Result<String, ProbeError>> + Send;

    /// Whether `xcode-select -p` succeeds, meaning Xcode or its Command Line
    /// Tools are installed and Apple's `/usr/bin/git` stub runs a real git
    /// instead of opening an installer dialog. The resolver asks at most once
    /// per resolution, and only when a candidate is the stub.
    fn xcode_tools_installed(&self) -> impl Future<Output = bool> + Send;
}

/// Why a candidate could not be asked for its version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    /// It could not be started (or waited for).
    #[error("it could not be started: {0}")]
    Spawn(String),
    /// It was still running after the timeout and was killed.
    #[error("it did not finish within {0:?}")]
    TimedOut(Duration),
    /// It exited unsuccessfully. `code` is `None` when a signal killed it.
    #[error("it exited unsuccessfully{}", exit_details(.code, .stderr))]
    Failed { code: Option<i32>, stderr: String },
}

fn exit_details(code: &Option<i32>, stderr: &str) -> String {
    let code = match code {
        Some(code) => format!(" with code {code}"),
        None => " (killed by a signal)".to_owned(),
    };
    if stderr.is_empty() {
        code
    } else {
        format!("{code}: {stderr}")
    }
}

/// The real probe: runs the commands through `git_engine::process`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemProbe {
    timeout: Duration,
}

/// Where macOS keeps `xcode-select`. The directory is SIP-protected, so the
/// absolute path is safe and does not depend on PATH.
const XCODE_SELECT: &str = "/usr/bin/xcode-select";

impl SystemProbe {
    /// How long a candidate may take to answer. Generous enough for a cold
    /// start from a slow disk or under an antivirus scan; short enough that a
    /// program which never answers cannot hold up startup for long.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn new() -> Self {
        Self::with_timeout(Self::DEFAULT_TIMEOUT)
    }

    /// A probe that gives up on each command after `timeout`.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Default for SystemProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl GitProbe for SystemProbe {
    async fn version_output(&self, git: &Path) -> Result<String, ProbeError> {
        let output = process::output(git, &["--version"], self.timeout)
            .await
            .map_err(|error| match error {
                ProcessError::Io(error) => ProbeError::Spawn(error.to_string()),
                ProcessError::TimedOut(after) => ProbeError::TimedOut(after),
            })?;
        if !output.status.success() {
            return Err(ProbeError::Failed {
                code: output.status.code(),
                stderr: excerpt(String::from_utf8_lossy(&output.stderr).trim()),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn xcode_tools_installed(&self) -> bool {
        // Off macOS there is no xcode-select, the spawn fails and the answer
        // is `false`; the resolver never asks there anyway.
        match process::output(Path::new(XCODE_SELECT), &["-p"], self.timeout).await {
            // `xcode-select -p` prints the active developer directory. Require
            // it to exist too: with the tools deleted by hand the stub would
            // still open the installer.
            Ok(output) if output.status.success() => {
                let dir = String::from_utf8_lossy(&output.stdout);
                let dir = dir.trim();
                !dir.is_empty() && Path::new(dir).is_dir()
            }
            // When in doubt, do not run the stub.
            _ => false,
        }
    }
}
