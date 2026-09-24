#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Git binary resolution (SPEC §5 "Git binary resolution", P0-05).
//!
//! Three groups of tests:
//! - version parsing, over strings real gits print;
//! - the resolver's decisions, against fake PATH directories on disk and a
//!   [`FakeProbe`] that answers `--version` and `xcode-select -p` without
//!   running anything, so every rule (order, PATHEXT, the Xcode stub,
//!   duplicates, fall-through reasons) is checked on every OS;
//! - real spawns of fake `git` scripts (a shell script on Unix, a `.cmd` batch
//!   file on Windows) found through a real PATH search.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use git_engine::git_binary::{
    resolve, BundledGit, GitBinaryError, GitProbe, GitSearch, GitSource, GitVersion, ProbeError,
    ResolvedGit, SkipReason, Skipped, SystemProbe, UnrecognisedVersion, MINIMUM_GIT_VERSION,
};
use git_engine::GitError;

// ---------------------------------------------------------------------------
// Version parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_the_version_strings_real_gits_print() {
    let cases = [
        ("git version 2.39.3 (Apple Git-146)\n", (2, 39, 3)),
        ("git version 2.45.1.windows.1\r\n", (2, 45, 1)),
        ("git version 2.30.0", (2, 30, 0)),
        ("git version 2.47.0.rc1\n", (2, 47, 0)),
        ("git version 2.47.0-rc1\n", (2, 47, 0)),
        ("git version 2.47.0.rc1.12.gabc1234\n", (2, 47, 0)),
        ("git version 2.45.2.vfs.0.0\n", (2, 45, 2)),
        // A build from a source tarball without tags.
        ("git version 2.45.GIT\n", (2, 45, 0)),
        ("git version 1.8.3.1\n", (1, 8, 3)),
        ("git version 2.100.10\n", (2, 100, 10)),
        // Surrounding blank lines and a second line are tolerated.
        ("\n  git version 2.43.0\nsomething else\n", (2, 43, 0)),
    ];
    for (output, (major, minor, patch)) in cases {
        assert_eq!(
            GitVersion::from_version_output(output),
            Ok(GitVersion::new(major, minor, patch)),
            "parsing {output:?}"
        );
    }
}

#[test]
fn rejects_output_that_is_not_a_git_version() {
    let garbage = [
        "",
        "   \n",
        "hello",
        "git version",
        "git version \n",
        "git version abc",
        "git version 2",
        "git version 2.",
        "git version .30.0",
        "git version 2..0",
        "git version +2.30.0",
        "git version 2.+30.0",
        "git version -2.30.0",
        "git version 2x.30.0",
        "git version 99999999999.0.0",
        "git versions 2.30.0",
        "Git version 2.30.0",
        "version 2.30.0",
        "2.30.0",
        "hub version 2.14.2\ngit version 2.30.0",
    ];
    for output in garbage {
        assert_eq!(
            GitVersion::from_version_output(output),
            Err(UnrecognisedVersion {
                output: output.trim().to_owned()
            }),
            "{output:?} must be rejected"
        );
    }
}

#[test]
fn versions_compare_numerically_not_as_text() {
    assert!(GitVersion::new(2, 100, 0) > GitVersion::new(2, 30, 0));
    assert!(GitVersion::new(2, 30, 10) > GitVersion::new(2, 30, 9));
    assert!(GitVersion::new(3, 0, 0) > GitVersion::new(2, 99, 99));
    assert!(GitVersion::new(2, 29, 99) < GitVersion::new(2, 30, 0));
}

#[test]
fn the_minimum_is_2_30_and_is_itself_supported() {
    assert_eq!(MINIMUM_GIT_VERSION, GitVersion::new(2, 30, 0));
    assert!(GitVersion::new(2, 30, 0).is_supported());
    assert!(GitVersion::new(2, 45, 1).is_supported());
    assert!(!GitVersion::new(2, 29, 99).is_supported());
    assert!(!GitVersion::new(1, 99, 0).is_supported());
    // A release candidate counts as its release: 2.30.0-rc1 already has
    // every 2.30 feature, so it is accepted.
    assert!(GitVersion::from_version_output("git version 2.30.0.rc1")
        .unwrap()
        .is_supported());
}

#[test]
fn a_version_displays_as_a_dotted_triple() {
    assert_eq!(GitVersion::new(2, 39, 3).to_string(), "2.39.3");
}

// ---------------------------------------------------------------------------
// Resolver decisions, with a fake probe
// ---------------------------------------------------------------------------

/// What a PATH search finds on this OS: `git`, or `git.exe` through PATHEXT.
const GIT: &str = if cfg!(windows) { "git.exe" } else { "git" };

/// Windows' default PATHEXT, trimmed to what can hold a git.
const WINDOWS_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// A search over exactly `dirs`, as this OS searches PATH, with no settings
/// path, no Xcode stub and no bundled git.
fn search(dirs: &[&Path]) -> GitSearch {
    GitSearch {
        settings_path: None,
        search_path: Some(std::env::join_paths(dirs).unwrap()),
        path_extensions: cfg!(windows).then(|| OsString::from(WINDOWS_PATHEXT)),
        xcode_stub: None,
        bundled: BundledGit::NotShipped,
    }
}

fn run(search: &GitSearch, probe: &FakeProbe) -> Result<ResolvedGit, GitError> {
    block_on(resolve(search, probe))
}

fn not_found(result: Result<ResolvedGit, GitError>) -> Vec<Skipped> {
    match result {
        Err(GitError::Binary(GitBinaryError::NotFound { skipped })) => skipped,
        other => panic!("expected no usable git, got {other:?}"),
    }
}

/// A new directory under `parent`.
fn dir(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An (empty) executable file; the fake probe decides what it "prints".
fn executable(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn skipped(source: GitSource, path: &Path, reason: SkipReason) -> Skipped {
    Skipped {
        source,
        path: path.to_path_buf(),
        reason,
    }
}

/// Answers `<git> --version` from a table and `xcode-select -p` from a flag,
/// and records every question it was asked.
#[derive(Default)]
struct FakeProbe {
    outputs: HashMap<PathBuf, Result<String, ProbeError>>,
    xcode_tools_installed: bool,
    version_calls: Mutex<Vec<PathBuf>>,
    xcode_calls: AtomicUsize,
}

impl FakeProbe {
    fn new() -> Self {
        Self::default()
    }

    /// `git` prints `git version <version>`.
    fn git(self, git: &Path, version: &str) -> Self {
        self.output(git, &format!("git version {version}\n"))
    }

    fn output(mut self, git: &Path, stdout: &str) -> Self {
        self.outputs
            .insert(git.to_path_buf(), Ok(stdout.to_owned()));
        self
    }

    fn failure(mut self, git: &Path, error: ProbeError) -> Self {
        self.outputs.insert(git.to_path_buf(), Err(error));
        self
    }

    fn xcode_tools(mut self, installed: bool) -> Self {
        self.xcode_tools_installed = installed;
        self
    }

    fn probed(&self) -> Vec<PathBuf> {
        self.version_calls.lock().unwrap().clone()
    }

    fn xcode_asked(&self) -> usize {
        self.xcode_calls.load(Ordering::SeqCst)
    }
}

impl GitProbe for FakeProbe {
    async fn version_output(&self, git: &Path) -> Result<String, ProbeError> {
        self.version_calls.lock().unwrap().push(git.to_path_buf());
        self.outputs.get(git).cloned().unwrap_or_else(|| {
            Err(ProbeError::Spawn(format!(
                "the fake probe has no git at {}",
                git.display()
            )))
        })
    }

    async fn xcode_tools_installed(&self) -> bool {
        self.xcode_calls.fetch_add(1, Ordering::SeqCst);
        self.xcode_tools_installed
    }
}

#[test]
fn the_settings_path_wins_over_path() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = executable(&dir(tmp.path(), "custom"), GIT);
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new()
        .git(&configured, "2.40.0")
        .git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.source, GitSource::Settings);
    assert_eq!(resolved.path, configured);
    assert_eq!(resolved.version, GitVersion::new(2, 40, 0));
    assert!(resolved.skipped.is_empty());
    assert_eq!(resolved.ignored_settings_path(), None);
    assert_eq!(probe.probed(), [configured], "PATH is not searched at all");
}

#[test]
fn a_settings_path_that_does_not_exist_is_reported_and_path_is_used() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = tmp.path().join("gone").join(GIT);
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new().git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.source, GitSource::Path);
    assert_eq!(resolved.path, on_path);
    let expected = skipped(GitSource::Settings, &configured, SkipReason::Missing);
    assert_eq!(resolved.skipped, std::slice::from_ref(&expected));
    assert_eq!(resolved.ignored_settings_path(), Some(&expected));
}

#[test]
fn a_settings_path_that_is_too_old_is_reported_and_path_is_used() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = executable(&dir(tmp.path(), "old"), GIT);
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new()
        .git(&configured, "2.29.2")
        .git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, on_path);
    assert_eq!(
        resolved.ignored_settings_path(),
        Some(&skipped(
            GitSource::Settings,
            &configured,
            SkipReason::TooOld(GitVersion::new(2, 29, 2))
        ))
    );
}

#[test]
fn a_settings_path_that_is_not_git_is_reported_with_what_it_printed() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = executable(&dir(tmp.path(), "other"), GIT);
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new()
        .output(&configured, "Python 3.12.1\n")
        .git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, on_path);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Settings,
            &configured,
            SkipReason::UnrecognisedVersion(UnrecognisedVersion {
                output: "Python 3.12.1".to_owned()
            })
        )]
    );
}

#[test]
fn a_settings_path_that_is_a_directory_is_reported_without_running_it() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = dir(tmp.path(), "Git");
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new().git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, on_path);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Settings,
            &configured,
            SkipReason::NotExecutable
        )]
    );
    assert_eq!(probe.probed(), [on_path]);
}

#[test]
fn a_relative_settings_path_is_reported_without_running_it() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = PathBuf::from("bin").join(GIT);
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new().git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, on_path);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Settings,
            &configured,
            SkipReason::NotAbsolute
        )]
    );
    assert_eq!(probe.probed(), [on_path]);
}

#[test]
fn an_empty_settings_path_means_no_override() {
    let tmp = tempfile::tempdir().unwrap();
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new().git(&on_path, "2.45.1");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.settings_path = Some(PathBuf::new());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.source, GitSource::Path);
    assert!(resolved.skipped.is_empty());
}

#[test]
fn path_is_searched_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let first = executable(&dir(tmp.path(), "first"), GIT);
    let second = executable(&dir(tmp.path(), "second"), GIT);
    let probe = FakeProbe::new()
        .git(&first, "2.40.0")
        .git(&second, "2.45.1");
    let search = search(&[first.parent().unwrap(), second.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, first);
    assert_eq!(resolved.version, GitVersion::new(2, 40, 0));
    assert_eq!(probe.probed(), [first]);
}

#[test]
fn a_too_old_git_early_on_path_is_skipped_for_a_newer_one_later() {
    let tmp = tempfile::tempdir().unwrap();
    let old = executable(&dir(tmp.path(), "usr-bin"), GIT);
    let new = executable(&dir(tmp.path(), "homebrew"), GIT);
    let probe = FakeProbe::new().git(&old, "2.25.1").git(&new, "2.45.1");
    let search = search(&[old.parent().unwrap(), new.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.source, GitSource::Path);
    assert_eq!(resolved.path, new);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Path,
            &old,
            SkipReason::TooOld(GitVersion::new(2, 25, 1))
        )]
    );
    assert_eq!(resolved.ignored_settings_path(), None);
}

#[test]
fn a_git_that_cannot_be_run_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let broken = executable(&dir(tmp.path(), "broken"), GIT);
    let good = executable(&dir(tmp.path(), "good"), GIT);
    let failure = ProbeError::Failed {
        code: Some(126),
        stderr: "cannot execute binary file".to_owned(),
    };
    let probe = FakeProbe::new()
        .failure(&broken, failure.clone())
        .git(&good, "2.45.1");
    let search = search(&[broken.parent().unwrap(), good.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, good);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Path,
            &broken,
            SkipReason::ProbeFailed(failure)
        )]
    );
}

#[cfg(unix)]
#[test]
fn a_file_on_path_without_execute_permission_is_skipped_without_running_it() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let plain = dir(tmp.path(), "plain").join("git");
    fs::write(&plain, b"").unwrap();
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
    let good = executable(&dir(tmp.path(), "good"), "git");
    let probe = FakeProbe::new().git(&plain, "2.45.1").git(&good, "2.45.1");
    let search = search(&[plain.parent().unwrap(), good.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, good);
    assert_eq!(
        resolved.skipped,
        [skipped(GitSource::Path, &plain, SkipReason::NotExecutable)]
    );
    assert_eq!(probe.probed(), [good]);
}

#[test]
fn a_directory_named_git_on_path_is_skipped_without_running_it() {
    let tmp = tempfile::tempdir().unwrap();
    let not_a_file = dir(&dir(tmp.path(), "src"), GIT);
    let good = executable(&dir(tmp.path(), "good"), GIT);
    let probe = FakeProbe::new().git(&good, "2.45.1");
    let search = search(&[not_a_file.parent().unwrap(), good.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, good);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Path,
            &not_a_file,
            SkipReason::NotExecutable
        )]
    );
    assert_eq!(probe.probed(), [good]);
}

#[test]
fn path_directories_without_git_are_not_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = dir(tmp.path(), "empty");
    let missing = tmp.path().join("missing");
    let good = executable(&dir(tmp.path(), "good"), GIT);
    let probe = FakeProbe::new().git(&good, "2.45.1");
    let search = search(&[&empty, &missing, good.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, good);
    assert!(resolved.skipped.is_empty(), "{:?}", resolved.skipped);
}

/// `path` relative to the current directory, e.g. `../../target/tmp/x`.
fn relative_to_current_dir(path: &Path) -> PathBuf {
    let cwd = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let path = fs::canonicalize(path).unwrap();
    let cwd: Vec<_> = cwd.components().collect();
    let path: Vec<_> = path.components().collect();
    let common = cwd.iter().zip(&path).take_while(|(a, b)| a == b).count();
    assert!(common > 0, "no common root with the current directory");
    let mut relative = PathBuf::new();
    for _ in common..cwd.len() {
        relative.push("..");
    }
    for component in &path[common..] {
        relative.push(component);
    }
    relative
}

#[test]
fn relative_and_empty_path_entries_are_never_searched() {
    // A relative PATH entry would resolve against whatever directory the app
    // was started from, so a repository could plant its own `git`.
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let planted = executable(&dir(tmp.path(), "planted"), GIT);
    let relative = relative_to_current_dir(planted.parent().unwrap());
    assert!(relative.is_relative());
    assert!(
        relative.join(GIT).is_file(),
        "the test reaches the planted git"
    );
    let good = executable(&dir(tmp.path(), "good"), GIT);
    let probe = FakeProbe::new()
        .git(&planted, "2.45.1")
        .git(&relative.join(GIT), "2.45.1")
        .git(&good, "2.40.0");
    let search = search(&[Path::new(""), &relative, good.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, good);
    assert!(resolved.skipped.is_empty(), "{:?}", resolved.skipped);
    assert_eq!(probe.probed(), [good]);
}

#[test]
fn the_same_git_reached_twice_on_path_is_run_once() {
    let tmp = tempfile::tempdir().unwrap();
    let old = executable(&dir(tmp.path(), "old"), GIT);
    let new = executable(&dir(tmp.path(), "new"), GIT);
    let probe = FakeProbe::new().git(&old, "2.25.1").git(&new, "2.45.1");
    let old_dir = old.parent().unwrap();
    let search = search(&[old_dir, old_dir, new.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, new);
    assert_eq!(resolved.skipped.len(), 1, "{:?}", resolved.skipped);
    assert_eq!(probe.probed(), [old, new]);
}

#[cfg(unix)]
#[test]
fn a_symlinked_path_entry_to_a_git_already_tried_is_not_run_again() {
    // Merged-/usr Linux: /bin -> /usr/bin, so PATH names one git twice.
    let tmp = tempfile::tempdir().unwrap();
    let old = executable(&dir(tmp.path(), "usr-bin"), "git");
    let link = tmp.path().join("bin");
    std::os::unix::fs::symlink(old.parent().unwrap(), &link).unwrap();
    let new = executable(&dir(tmp.path(), "local"), "git");
    let probe = FakeProbe::new().git(&old, "2.25.1").git(&new, "2.45.1");
    let search = search(&[old.parent().unwrap(), &link, new.parent().unwrap()]);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, new);
    assert_eq!(probe.probed(), [old, new]);
}

#[test]
fn a_settings_path_that_is_also_on_path_is_run_once() {
    let tmp = tempfile::tempdir().unwrap();
    let old = executable(&dir(tmp.path(), "old"), GIT);
    let new = executable(&dir(tmp.path(), "new"), GIT);
    let probe = FakeProbe::new().git(&old, "2.25.1").git(&new, "2.45.1");
    let mut search = search(&[old.parent().unwrap(), new.parent().unwrap()]);
    search.settings_path = Some(old.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, new);
    assert_eq!(
        resolved.skipped,
        [skipped(
            GitSource::Settings,
            &old,
            SkipReason::TooOld(GitVersion::new(2, 25, 1))
        )]
    );
    assert_eq!(probe.probed(), [old, new]);
}

// The Xcode stub rule is checked on every OS by pointing `xcode_stub` at a
// fake file; only `GitSearch::from_process_env` decides that the real stub is
// `/usr/bin/git` on macOS (tested further down).

#[test]
fn the_xcode_stub_is_never_run_when_the_command_line_tools_are_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = executable(&dir(tmp.path(), "usr/bin"), GIT);
    let brew = executable(&dir(tmp.path(), "opt/homebrew/bin"), GIT);
    let probe = FakeProbe::new()
        .git(&stub, "2.39.3 (Apple Git-146)")
        .git(&brew, "2.45.1")
        .xcode_tools(false);
    let mut search = search(&[stub.parent().unwrap(), brew.parent().unwrap()]);
    search.xcode_stub = Some(stub.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, brew);
    assert_eq!(
        resolved.skipped,
        [skipped(GitSource::Path, &stub, SkipReason::XcodeStub)]
    );
    assert_eq!(
        probe.probed(),
        [brew],
        "running the stub opens an installer"
    );
    assert_eq!(probe.xcode_asked(), 1);
}

#[test]
fn the_xcode_stub_is_used_when_the_command_line_tools_are_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = executable(&dir(tmp.path(), "usr/bin"), GIT);
    let brew = executable(&dir(tmp.path(), "opt/homebrew/bin"), GIT);
    let probe = FakeProbe::new()
        .git(&stub, "2.39.3 (Apple Git-146)")
        .git(&brew, "2.45.1")
        .xcode_tools(true);
    let mut search = search(&[stub.parent().unwrap(), brew.parent().unwrap()]);
    search.xcode_stub = Some(stub.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, stub);
    assert_eq!(resolved.version, GitVersion::new(2, 39, 3));
    assert_eq!(probe.xcode_asked(), 1);
}

#[test]
fn xcode_select_is_only_asked_when_the_stub_is_reached() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = executable(&dir(tmp.path(), "usr/bin"), GIT);
    let brew = executable(&dir(tmp.path(), "opt/homebrew/bin"), GIT);
    let probe = FakeProbe::new().git(&brew, "2.45.1").xcode_tools(false);
    let mut search = search(&[brew.parent().unwrap(), stub.parent().unwrap()]);
    search.xcode_stub = Some(stub);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, brew);
    assert_eq!(probe.xcode_asked(), 0);
}

#[test]
fn a_settings_path_naming_the_xcode_stub_is_skipped_too() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = executable(&dir(tmp.path(), "usr/bin"), GIT);
    let brew = executable(&dir(tmp.path(), "opt/homebrew/bin"), GIT);
    let probe = FakeProbe::new()
        .git(&stub, "2.39.3 (Apple Git-146)")
        .git(&brew, "2.45.1")
        .xcode_tools(false);
    let mut search = search(&[stub.parent().unwrap(), brew.parent().unwrap()]);
    search.settings_path = Some(stub.clone());
    search.xcode_stub = Some(stub.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, brew);
    assert_eq!(
        resolved.skipped,
        [skipped(GitSource::Settings, &stub, SkipReason::XcodeStub)]
    );
    assert_eq!(probe.probed(), [brew]);
    assert_eq!(probe.xcode_asked(), 1, "the answer is reused, not re-asked");
}

#[cfg(unix)]
#[test]
fn a_symlink_to_the_xcode_stub_is_treated_as_the_stub() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = executable(&dir(tmp.path(), "usr/bin"), "git");
    let link = dir(tmp.path(), "usr/local/bin").join("git");
    std::os::unix::fs::symlink(&stub, &link).unwrap();
    let brew = executable(&dir(tmp.path(), "opt/homebrew/bin"), "git");
    let probe = FakeProbe::new()
        .git(&link, "2.39.3 (Apple Git-146)")
        .git(&brew, "2.45.1")
        .xcode_tools(false);
    let mut search = search(&[link.parent().unwrap(), brew.parent().unwrap()]);
    search.xcode_stub = Some(stub);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.path, brew);
    assert_eq!(
        resolved.skipped,
        [skipped(GitSource::Path, &link, SkipReason::XcodeStub)]
    );
    assert_eq!(probe.probed(), [brew]);
}

#[test]
fn pathext_extensions_are_tried_in_their_listed_order() {
    // Checked on every OS by passing a PATHEXT explicitly; on Windows,
    // `GitSearch::from_process_env` passes the real one.
    let tmp = tempfile::tempdir().unwrap();
    let bin = dir(tmp.path(), "bin");
    let exe = executable(&bin, "git.exe");
    let cmd = executable(&bin, "git.cmd");
    let bare = executable(&bin, "git");
    let probe = FakeProbe::new()
        .git(&exe, "2.45.1.windows.1")
        .git(&cmd, "2.40.0")
        .git(&bare, "2.41.0");
    let mut search = search(&[&bin]);

    search.path_extensions = Some(".COM;.EXE;.BAT;.CMD".into());
    assert_eq!(run(&search, &probe).unwrap().path, exe);

    // PATHEXT is upper-case by convention; the file names looked for are
    // lower-cased so the path reads the way it does on disk.
    search.path_extensions = Some(".CMD;.EXE".into());
    assert_eq!(run(&search, &probe).unwrap().path, cmd);

    // With PATHEXT in effect an extension-less `git` is not a candidate.
    search.path_extensions = Some(";;.COM".into());
    assert!(not_found(run(&search, &probe)).is_empty());
}

#[test]
fn the_bundled_git_is_the_last_resort() {
    let tmp = tempfile::tempdir().unwrap();
    let old = executable(&dir(tmp.path(), "bin"), GIT);
    let bundled = executable(&dir(tmp.path(), "resources/git/bin"), GIT);
    let probe = FakeProbe::new().git(&old, "2.25.1").git(&bundled, "2.47.0");
    let mut search = search(&[old.parent().unwrap()]);
    search.bundled = BundledGit::At(bundled.clone());

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.source, GitSource::Bundled);
    assert_eq!(resolved.path, bundled);
    assert_eq!(resolved.version, GitVersion::new(2, 47, 0));
    assert_eq!(resolved.skipped.len(), 1);
}

#[test]
fn a_git_on_path_is_preferred_over_the_bundled_one() {
    let tmp = tempfile::tempdir().unwrap();
    let on_path = executable(&dir(tmp.path(), "bin"), GIT);
    let bundled = executable(&dir(tmp.path(), "resources/git/bin"), GIT);
    let probe = FakeProbe::new()
        .git(&on_path, "2.45.1")
        .git(&bundled, "2.47.0");
    let mut search = search(&[on_path.parent().unwrap()]);
    search.bundled = BundledGit::At(bundled);

    let resolved = run(&search, &probe).unwrap();

    assert_eq!(resolved.source, GitSource::Path);
    assert_eq!(probe.probed(), [on_path]);
}

#[test]
fn a_bundled_git_missing_from_the_install_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let bundled = tmp.path().join("resources/git/bin").join(GIT);
    let mut search = search(&[]);
    search.bundled = BundledGit::At(bundled.clone());

    let skipped_candidates = not_found(run(&search, &FakeProbe::new()));

    assert_eq!(
        skipped_candidates,
        [skipped(GitSource::Bundled, &bundled, SkipReason::Missing)]
    );
}

#[test]
fn no_usable_git_is_an_error_that_lists_every_candidate_tried() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = tmp.path().join("gone").join(GIT);
    let old = executable(&dir(tmp.path(), "bin"), GIT);
    let probe = FakeProbe::new().git(&old, "2.25.1");
    let mut search = search(&[old.parent().unwrap()]);
    search.settings_path = Some(configured.clone());

    let result = run(&search, &probe);
    let message = result.as_ref().unwrap_err().to_string();

    assert_eq!(
        not_found(result),
        [
            skipped(GitSource::Settings, &configured, SkipReason::Missing),
            skipped(
                GitSource::Path,
                &old,
                SkipReason::TooOld(GitVersion::new(2, 25, 1))
            ),
        ]
    );
    assert!(message.contains("2.30.0"), "{message}");
    assert!(
        message.contains(&*configured.to_string_lossy()),
        "{message}"
    );
    assert!(message.contains(&*old.to_string_lossy()), "{message}");
    assert!(message.contains("2.25.1"), "{message}");
}

#[test]
fn an_empty_search_finds_nothing_and_reports_nothing() {
    let mut search = search(&[]);
    search.search_path = None;

    assert!(not_found(run(&search, &FakeProbe::new())).is_empty());
}

#[test]
fn resolving_can_run_on_a_multi_threaded_runtime() {
    // Compile-time check: Tauri commands and `tokio::spawn` need `Send`
    // futures, so nothing the resolver holds across an await may be `!Send`.
    fn assert_send<T: Send>(_: &T) {}
    let search = GitSearch::from_process_env(None, BundledGit::NotShipped);
    let probe = SystemProbe::new();
    assert_send(&resolve(&search, &probe));
}

// ---------------------------------------------------------------------------
// Real processes
// ---------------------------------------------------------------------------

/// Tests that write a script and then run it hold this lock throughout.
/// Linux refuses to exec a file that any process still has open for writing
/// (ETXTBSY), and a child forked by a concurrent test inherits the script's
/// write handle until it execs; running these one at a time avoids the race.
static REAL_PROCESSES: Mutex<()> = Mutex::new(());

fn with_real_processes<T>(test: impl FnOnce() -> T) -> T {
    let _guard = REAL_PROCESSES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    test()
}

/// Writes a fake `git` into `dir` that runs `body` (lines of `sh` on Unix,
/// of `cmd` on Windows) and returns the path a PATH search finds it at.
fn script(dir: &Path, unix_body: &str, windows_body: &str) -> PathBuf {
    if cfg!(windows) {
        let path = dir.join("git.cmd");
        let text = format!("@echo off\r\n{}\r\n", windows_body.replace('\n', "\r\n"));
        fs::write(&path, text).unwrap();
        path
    } else {
        let path = dir.join("git");
        fs::write(&path, format!("#!/bin/sh\n{unix_body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
}

/// A fake `git` that prints `git version <version>`.
fn script_git(dir: &Path, version: &str) -> PathBuf {
    script(
        dir,
        &format!("echo 'git version {version}'"),
        &format!("echo git version {version}"),
    )
}

#[test]
fn a_real_path_search_runs_each_git_and_parses_what_it_prints() {
    with_real_processes(|| {
        let tmp = tempfile::tempdir().unwrap();
        let old = script_git(&dir(tmp.path(), "old"), "2.29.2");
        let printed = if cfg!(windows) {
            "2.45.1.windows.1"
        } else {
            "2.39.3 (Apple Git-146)"
        };
        let new = script_git(&dir(tmp.path(), "new dir with spaces"), printed);
        let search = search(&[old.parent().unwrap(), new.parent().unwrap()]);

        let resolved = block_on(resolve(&search, &SystemProbe::new())).unwrap();

        assert_eq!(resolved.source, GitSource::Path);
        assert_eq!(resolved.path, new);
        let expected = if cfg!(windows) {
            GitVersion::new(2, 45, 1)
        } else {
            GitVersion::new(2, 39, 3)
        };
        assert_eq!(resolved.version, expected);
        assert_eq!(
            resolved.skipped,
            [skipped(
                GitSource::Path,
                &old,
                SkipReason::TooOld(GitVersion::new(2, 29, 2))
            )]
        );
    });
}

#[test]
fn a_real_settings_path_is_run_and_used() {
    with_real_processes(|| {
        let tmp = tempfile::tempdir().unwrap();
        let configured = script_git(&dir(tmp.path(), "ünïcödé"), "2.44.0");
        let mut search = search(&[]);
        search.settings_path = Some(configured.clone());

        let resolved = block_on(resolve(&search, &SystemProbe::new())).unwrap();

        assert_eq!(resolved.source, GitSource::Settings);
        assert_eq!(resolved.path, configured);
        assert_eq!(resolved.version, GitVersion::new(2, 44, 0));
    });
}

#[test]
fn a_git_that_exits_with_an_error_is_skipped_with_its_exit_code_and_stderr() {
    with_real_processes(|| {
        let tmp = tempfile::tempdir().unwrap();
        let broken = script(
            &dir(tmp.path(), "broken"),
            "echo 'fatal: broken' >&2\nexit 3",
            ">&2 echo fatal: broken\nexit /b 3",
        );

        let skipped_candidates = not_found(block_on(resolve(
            &search(&[broken.parent().unwrap()]),
            &SystemProbe::new(),
        )));

        assert_eq!(
            skipped_candidates,
            [skipped(
                GitSource::Path,
                &broken,
                SkipReason::ProbeFailed(ProbeError::Failed {
                    code: Some(3),
                    stderr: "fatal: broken".to_owned()
                })
            )]
        );
    });
}

#[test]
fn a_git_that_never_answers_is_given_up_on_after_the_timeout() {
    with_real_processes(|| {
        let tmp = tempfile::tempdir().unwrap();
        let hanging = script(
            &dir(tmp.path(), "hanging"),
            "exec sleep 30",
            "ping -n 31 127.0.0.1 >nul",
        );
        let timeout = Duration::from_millis(300);
        let started = Instant::now();

        let skipped_candidates = not_found(block_on(resolve(
            &search(&[hanging.parent().unwrap()]),
            &SystemProbe::with_timeout(timeout),
        )));

        assert_eq!(
            skipped_candidates,
            [skipped(
                GitSource::Path,
                &hanging,
                SkipReason::ProbeFailed(ProbeError::TimedOut(timeout))
            )]
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "gave up after {:?}",
            started.elapsed()
        );
    });
}

#[test]
fn the_git_installed_on_this_machine_is_found() {
    // Every machine that builds this repository has a git >= 2.30 on PATH,
    // and so does every CI runner. On macOS this also asks the real
    // `xcode-select -p` when /usr/bin/git comes first on PATH.
    with_real_processes(|| {
        let search = GitSearch::from_process_env(None, BundledGit::NotShipped);

        let resolved = block_on(resolve(&search, &SystemProbe::new())).unwrap();

        assert_eq!(resolved.source, GitSource::Path);
        assert!(resolved.path.is_absolute(), "{}", resolved.path.display());
        assert!(resolved.path.is_file(), "{}", resolved.path.display());
        assert!(resolved.version.is_supported(), "{}", resolved.version);
    });
}

#[test]
fn the_process_search_uses_this_platforms_rules() {
    let configured = std::env::temp_dir().join("custom-git");
    let search = GitSearch::from_process_env(Some(configured.clone()), BundledGit::NotShipped);

    assert_eq!(search.settings_path, Some(configured));
    assert_eq!(search.search_path, std::env::var_os("PATH"));
    assert_eq!(search.bundled, BundledGit::NotShipped);
    if cfg!(target_os = "macos") {
        assert_eq!(search.xcode_stub, Some(PathBuf::from("/usr/bin/git")));
    } else {
        assert_eq!(search.xcode_stub, None);
    }
    if cfg!(windows) {
        let pathext = std::env::var_os("PATHEXT").unwrap_or_else(|| WINDOWS_PATHEXT.into());
        assert_eq!(search.path_extensions, Some(pathext));
    } else {
        assert_eq!(search.path_extensions, None);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn xcode_select_finds_the_developer_tools_this_test_was_linked_with() {
    // rustc links through the Command Line Tools' `cc`, so any Mac that built
    // this test has them (or full Xcode) and `xcode-select -p` succeeds.
    with_real_processes(|| {
        assert!(block_on(SystemProbe::new().xcode_tools_installed()));
    });
}

#[cfg(not(target_os = "macos"))]
#[test]
fn there_are_no_xcode_tools_off_macos() {
    with_real_processes(|| {
        assert!(!block_on(SystemProbe::new().xcode_tools_installed()));
    });
}
