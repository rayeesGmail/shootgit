//! Records RSS, git spawn count and wall time per named operation to JSON
//! (SPEC §4 Low-resource operation, design rule 10 "Measure on the
//! constrained VM"; ADR 0004).
//!
//! The smoke flow in `git-engine-cli` (P0-13, P0-18) wraps each step in
//! [`Harness::begin`] / [`Harness::end`] and ends with
//! [`Harness::write_json`]. The constrained-VM CI job compares that file to
//! `perf/baseline.json`: `totals.peak_rss_bytes` and `totals.git_spawns`
//! fail on a > 10 % regression, `operations[].wall_ms` on > 25 % over the
//! median of three runs.
//!
//! The JSON shape is pinned by `tests/golden/report.json`. Bump
//! [`SCHEMA_VERSION`] whenever it changes, so the gate can tell an old
//! baseline from a new report.
//!
//! Cost: a clock read, two counter reads and one RSS query per operation
//! (`proc_pidinfo`, `/proc/self/statm` or `GetProcessMemoryInfo`). Nothing
//! samples in the background; peak values are the kernel's own high-water
//! marks.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub mod rss;

/// Version of the JSON shape written by [`Report::to_json`].
pub const SCHEMA_VERSION: u32 = 1;

/// Why a report could not be written or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    #[error("could not write {path:?}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not encode the report as JSON")]
    Encode(#[source] serde_json::Error),
    #[error("could not parse the report")]
    Parse(#[source] serde_json::Error),
}

/// One measured step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub name: String,
    /// Wall time, rounded to the microsecond.
    pub wall_ms: f64,
    /// `git` processes started while the operation ran.
    pub git_spawns: u64,
    /// This process's resident set when the operation ended, or `None`
    /// where the platform cannot report it.
    pub rss_bytes: Option<u64>,
    /// This process's peak resident set so far.
    pub peak_rss_bytes: Option<u64>,
}

/// Figures over the whole run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    /// Sum of the operations' wall time.
    pub wall_ms: f64,
    /// Sum of the operations' git spawns.
    pub git_spawns: u64,
    /// This process's peak resident set when the report was made.
    pub peak_rss_bytes: Option<u64>,
    /// The largest peak resident set among the child processes that have
    /// been waited for (`git`), where the platform reports it (Unix
    /// `RUSAGE_CHILDREN`); `None` on Windows.
    pub children_peak_rss_bytes: Option<u64>,
}

/// What [`Harness::write_json`] writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    /// `std::env::consts::OS`.
    pub os: String,
    /// `std::env::consts::ARCH`.
    pub arch: String,
    pub available_parallelism: usize,
    /// Worker threads the app's runtime would get on this machine.
    pub worker_threads: usize,
    /// Concurrent git processes the shared limiter allows here.
    pub git_limiter_cap: usize,
    pub operations: Vec<Operation>,
    pub totals: Totals,
}

impl Report {
    /// Pretty-printed JSON with one trailing newline, for clean diffs.
    pub fn to_json(&self) -> Result<String, HarnessError> {
        let mut json = serde_json::to_string_pretty(self).map_err(HarnessError::Encode)?;
        json.push('\n');
        Ok(json)
    }

    pub fn from_json(text: &str) -> Result<Self, HarnessError> {
        serde_json::from_str(text).map_err(HarnessError::Parse)
    }
}

/// An operation in progress; hand it back to [`Harness::end`].
#[derive(Debug)]
#[must_use = "an operation is only recorded when it is passed to Harness::end"]
pub struct Timer {
    name: String,
    started: Instant,
    git_spawns_before: u64,
}

/// Collects [`Operation`]s and turns them into a [`Report`].
#[derive(Debug, Default)]
pub struct Harness {
    operations: Vec<Operation>,
}

impl Harness {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts timing `name`.
    pub fn begin(&self, name: impl Into<String>) -> Timer {
        Timer {
            name: name.into(),
            started: Instant::now(),
            git_spawns_before: git_engine::process::git_spawn_count(),
        }
    }

    /// Stops `timer` and records its operation.
    pub fn end(&mut self, timer: Timer) -> &Operation {
        let elapsed = timer.started.elapsed();
        let git_spawns =
            git_engine::process::git_spawn_count().saturating_sub(timer.git_spawns_before);
        self.operations.push(Operation {
            name: timer.name,
            wall_ms: to_ms(elapsed),
            git_spawns,
            rss_bytes: rss::current(),
            peak_rss_bytes: rss::peak(),
        });
        let last = self.operations.len() - 1;
        &self.operations[last]
    }

    /// Records `work` as the operation `name` and returns its result.
    pub fn measure<T>(&mut self, name: impl Into<String>, work: impl FnOnce() -> T) -> T {
        let timer = self.begin(name);
        let value = work();
        self.end(timer);
        value
    }

    /// Everything recorded so far, in order.
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// A snapshot of the recorded operations plus machine facts and totals.
    pub fn report(&self) -> Report {
        let parallelism = git_engine::runtime::available_parallelism();
        Report {
            schema: SCHEMA_VERSION,
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            available_parallelism: parallelism,
            worker_threads: git_engine::runtime::worker_threads(parallelism),
            git_limiter_cap: git_engine::process::cap_for(parallelism),
            operations: self.operations.clone(),
            totals: Totals {
                wall_ms: round_ms(self.operations.iter().map(|op| op.wall_ms).sum()),
                git_spawns: self.operations.iter().map(|op| op.git_spawns).sum(),
                peak_rss_bytes: rss::peak(),
                children_peak_rss_bytes: rss::children_peak(),
            },
        }
    }

    /// Writes [`Report::to_json`] to `path`, creating parent directories.
    pub fn write_json(&self, path: &Path) -> Result<(), HarnessError> {
        let json = self.report().to_json()?;
        let write = || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, json)
        };
        write().map_err(|source| HarnessError::Write {
            path: path.to_owned(),
            source,
        })
    }
}

fn to_ms(duration: Duration) -> f64 {
    round_ms(duration.as_secs_f64() * 1000.0)
}

/// Rounds to the microsecond, so reports diff cleanly and sums stay exact
/// enough to compare.
fn round_ms(ms: f64) -> f64 {
    (ms * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_time_is_rounded_to_the_microsecond() {
        assert_eq!(to_ms(Duration::from_micros(1_234_567)), 1234.567);
        assert_eq!(to_ms(Duration::from_nanos(1_500)), 0.002);
        assert_eq!(to_ms(Duration::ZERO), 0.0);
    }

    #[test]
    fn end_returns_the_operation_it_recorded() {
        let mut harness = Harness::new();
        let timer = harness.begin("step");
        let recorded = harness.end(timer).clone();
        assert_eq!(recorded.name, "step");
        assert_eq!(harness.operations(), [recorded]);
    }
}
