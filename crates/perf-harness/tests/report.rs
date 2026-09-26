#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-17: the perf harness records RSS, git spawn count and wall time per
//! named operation and writes them as JSON (SPEC §4 Low-resource operation).
//! P0-18's constrained-VM gate parses that JSON, so its shape is pinned by a
//! golden file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::process::GitCommand;
use perf_harness::{rss, Harness, Operation, Report, Totals, SCHEMA_VERSION};

async fn machine_git() -> PathBuf {
    resolve(&ResolveOptions::from_env(None)).await.unwrap().path
}

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("golden file exists");
    // CR is not significant in JSON; tolerate a CRLF checkout on Windows.
    text.replace("\r\n", "\n")
}

/// A report with every field set to a fixed value, so the JSON it produces
/// can be compared byte for byte.
fn fixed_report() -> Report {
    Report {
        schema: SCHEMA_VERSION,
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        available_parallelism: 2,
        worker_threads: 1,
        git_limiter_cap: 2,
        operations: vec![
            Operation {
                name: "open_repo".to_owned(),
                wall_ms: 12.5,
                git_spawns: 1,
                rss_bytes: Some(52_428_800),
                peak_rss_bytes: Some(62_914_560),
            },
            Operation {
                name: "status".to_owned(),
                wall_ms: 7.25,
                git_spawns: 2,
                rss_bytes: Some(54_525_952),
                peak_rss_bytes: Some(62_914_560),
            },
            Operation {
                name: "shutdown".to_owned(),
                wall_ms: 0.0,
                git_spawns: 0,
                rss_bytes: None,
                peak_rss_bytes: None,
            },
        ],
        totals: Totals {
            wall_ms: 19.75,
            git_spawns: 3,
            peak_rss_bytes: Some(62_914_560),
            children_peak_rss_bytes: None,
        },
    }
}

#[test]
fn the_json_shape_matches_the_golden_file() {
    let json = fixed_report().to_json().unwrap();
    assert_eq!(json, golden("report.json"));
}

#[test]
fn the_golden_file_parses_back_into_the_same_report() {
    let parsed = Report::from_json(&golden("report.json")).unwrap();
    assert_eq!(parsed, fixed_report());
}

#[test]
fn an_empty_harness_still_produces_a_complete_report() {
    let report = Harness::new().report();
    assert_eq!(report.schema, SCHEMA_VERSION);
    assert!(report.operations.is_empty());
    assert_eq!(report.totals.git_spawns, 0);
    assert_eq!(report.totals.wall_ms, 0.0);
    assert_eq!(report.os, std::env::consts::OS);
    assert_eq!(report.arch, std::env::consts::ARCH);
    assert_eq!(
        report.available_parallelism,
        git_engine::runtime::available_parallelism()
    );
    assert_eq!(
        report.worker_threads,
        git_engine::runtime::worker_threads(report.available_parallelism)
    );
    assert_eq!(
        report.git_limiter_cap,
        git_engine::process::cap_for(report.available_parallelism)
    );
    // Round trip through JSON for the same result.
    let json = report.to_json().unwrap();
    assert_eq!(Report::from_json(&json).unwrap(), report);
}

#[tokio::test]
async fn operations_record_wall_time_git_spawns_and_rss() {
    // Resolving git spawns `git --version` too, so it happens before timing.
    let mut git = GitCommand::new(machine_git().await);
    git.arg("--version");
    let mut harness = Harness::new();

    let timer = harness.begin("git_version");
    git.output().await.unwrap();
    harness.end(timer);

    let timer = harness.begin("idle");
    tokio::time::sleep(Duration::from_millis(20)).await;
    harness.end(timer);

    let doubled = harness.measure("closure", || 21 * 2);
    assert_eq!(doubled, 42);

    let report = harness.report();
    let names: Vec<&str> = report.operations.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["git_version", "idle", "closure"]);

    let git_version = &report.operations[0];
    assert_eq!(git_version.git_spawns, 1);
    assert!(git_version.wall_ms > 0.0);

    let idle = &report.operations[1];
    assert_eq!(idle.git_spawns, 0);
    assert!(idle.wall_ms >= 20.0, "{}", idle.wall_ms);

    assert_eq!(report.totals.git_spawns, 1);
    let sum: f64 = report.operations.iter().map(|o| o.wall_ms).sum();
    assert!((report.totals.wall_ms - sum).abs() < 0.001);

    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        for op in &report.operations {
            assert!(op.rss_bytes.unwrap() > 0, "{op:?}");
            assert!(op.peak_rss_bytes.unwrap() > 0, "{op:?}");
        }
        assert!(report.totals.peak_rss_bytes.unwrap() > 0);
    }
}

#[test]
fn write_json_produces_a_file_that_parses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("perf.json");
    let mut harness = Harness::new();
    harness.measure("noop", || ());

    harness.write_json(&path).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'), "one trailing newline for clean diffs");
    let parsed = Report::from_json(&text).unwrap();
    assert_eq!(parsed.operations.len(), 1);
    assert_eq!(parsed.operations[0].name, "noop");
}

#[test]
fn rss_is_readable_on_the_three_supported_platforms() {
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        assert!(rss::current().unwrap() > 0);
        assert!(rss::peak().unwrap() > 0);
    }
}

#[test]
fn touching_memory_shows_up_in_current_rss() {
    if !cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        return;
    }
    const BYTES: usize = 32 * 1024 * 1024;
    let before = rss::current().unwrap();

    let mut block = vec![0u8; BYTES];
    for (i, byte) in block.iter_mut().enumerate().step_by(4096) {
        *byte = i as u8;
    }
    let after = rss::current().unwrap();
    std::hint::black_box(&block);

    assert!(
        after >= before + BYTES as u64 / 2,
        "before {before}, after {after}"
    );
    assert!(rss::peak().unwrap() >= after / 2);
}
