#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-11: the settings store. SPEC §4 Data at rest puts settings and the
//! recent repos in `app_config_dir/settings.json`; §5 Git binary resolution
//! reads its first candidate, the git path override, from there.
//!
//! Every test works in its own temporary directory. None of them reads or
//! writes the real config directory of the user running the tests.

use std::fs;
use std::path::{Path, PathBuf};

use app_lib::settings::{self, Settings, SettingsStore, MAX_RECENT_REPOS, SETTINGS_FILE_NAME};
use tauri::Manager;

fn settings_file(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join(SETTINGS_FILE_NAME)
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Settings that differ from the defaults in every field, with paths that
/// contain a space and non-ASCII characters.
fn sample() -> Settings {
    let mut settings = Settings::default();
    settings.git_path = Some(PathBuf::from("/opt/git/bin/git"));
    settings.record_recent_repo("/work/older repo");
    settings.record_recent_repo("/work/ünïcødé");
    settings.record_recent_repo("/work/newest");
    settings
}

#[test]
fn defaults_have_no_recent_repos_and_no_git_override() {
    let defaults = Settings::default();

    assert!(defaults.recent_repos().is_empty());
    assert_eq!(defaults.git_path, None);
}

#[test]
fn settings_round_trip_through_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);
    let written = sample();

    settings::save(&path, &written).unwrap();

    assert_eq!(settings::load(&path), written);
}

#[test]
fn the_store_keeps_updates_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);

    let store = SettingsStore::load(path.clone());
    assert_eq!(store.path(), Some(path.as_path()));
    assert_eq!(store.get(), Settings::default());

    let updated = store
        .update(|settings| {
            settings.git_path = Some(PathBuf::from("/usr/local/bin/git"));
            settings.record_recent_repo("/work/a");
        })
        .unwrap();
    assert_eq!(store.get(), updated);
    assert_eq!(updated.git_path, Some(PathBuf::from("/usr/local/bin/git")));
    assert_eq!(updated.recent_repos(), [PathBuf::from("/work/a")]);

    let reopened = SettingsStore::load(path);
    assert_eq!(reopened.get(), updated);
}

/// The file is the contract with earlier and later versions of the app, and
/// people read and edit it by hand, so its field names are pinned here.
#[test]
fn the_on_disk_format_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);

    settings::save(&path, &sample()).unwrap();

    let on_disk: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).expect("settings.json is valid JSON");
    assert_eq!(
        on_disk,
        serde_json::json!({
            "recent_repos": ["/work/newest", "/work/ünïcødé", "/work/older repo"],
            "git_path": "/opt/git/bin/git",
        })
    );
}

#[test]
fn a_missing_file_loads_defaults_without_creating_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);

    assert_eq!(settings::load(&path), Settings::default());
    assert!(file_names(dir.path()).is_empty(), "loading never writes");
}

#[test]
fn a_corrupt_file_recovers_to_defaults() {
    let cases: [(&str, &[u8]); 7] = [
        ("empty", b""),
        ("truncated", b"{\"recent_repos\": [\"/wo"),
        ("not JSON", b"not json at all"),
        ("wrong top-level type", b"[]"),
        // serde would read this as the fields in declaration order.
        ("array of the fields", b"[[\"/work/a\"], \"/usr/bin/git\"]"),
        ("wrong field type", b"{\"recent_repos\": 5}"),
        ("invalid UTF-8", b"{\"git_path\": \"\xff\xfe\"}"),
    ];

    for (case, bytes) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_file(&dir);
        fs::write(&path, bytes).unwrap();

        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get(), Settings::default(), "{case}");

        // The unreadable file is moved aside rather than silently replaced by
        // the next save, so whatever the user had in it can still be
        // recovered by hand.
        let kept = dir.path().join(format!("{SETTINGS_FILE_NAME}.corrupt"));
        let kept_bytes = fs::read(&kept)
            .unwrap_or_else(|error| panic!("{case}: no corrupt copy at {kept:?}: {error}"));
        assert_eq!(kept_bytes, bytes, "{case}");
        assert!(!path.exists(), "{case}");

        // The app keeps working: the next change writes a valid file.
        store
            .update(|settings| settings.record_recent_repo("/work/a"))
            .unwrap();
        assert_eq!(
            settings::load(&path).recent_repos(),
            [PathBuf::from("/work/a")],
            "{case}"
        );
    }
}

#[test]
fn missing_fields_take_defaults_and_unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);
    fs::write(
        &path,
        r#"{ "git_path": "/usr/local/bin/git", "written_by_a_newer_version": true }"#,
    )
    .unwrap();

    let loaded = settings::load(&path);

    assert_eq!(loaded.git_path, Some(PathBuf::from("/usr/local/bin/git")));
    assert!(loaded.recent_repos().is_empty());
    assert_eq!(
        file_names(dir.path()),
        [SETTINGS_FILE_NAME],
        "a readable file is not treated as corrupt"
    );
}

#[test]
fn recent_repos_are_newest_first_without_duplicates() {
    let mut settings = Settings::default();

    settings.record_recent_repo("/work/a");
    settings.record_recent_repo("/work/b");
    settings.record_recent_repo("/work/a");

    assert_eq!(
        settings.recent_repos(),
        [PathBuf::from("/work/a"), PathBuf::from("/work/b")]
    );
}

#[test]
fn recent_repos_keep_at_most_twenty() {
    assert_eq!(MAX_RECENT_REPOS, 20);
    let mut settings = Settings::default();

    for i in 0..25 {
        settings.record_recent_repo(format!("/repo/{i}"));
    }

    let newest_twenty: Vec<PathBuf> = (5..25)
        .rev()
        .map(|i| PathBuf::from(format!("/repo/{i}")))
        .collect();
    assert_eq!(settings.recent_repos(), newest_twenty);
}

/// A hand-edited file can break the list's rules; loading restores them
/// instead of rejecting the whole file.
#[test]
fn a_hand_edited_recent_list_is_deduplicated_and_capped_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);
    let mut listed: Vec<String> = vec!["/repo/0".to_owned(), "/repo/0".to_owned()];
    listed.extend((1..30).map(|i| format!("/repo/{i}")));
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({ "recent_repos": listed })).unwrap(),
    )
    .unwrap();

    let loaded = settings::load(&path);

    let expected: Vec<PathBuf> = (0..20)
        .map(|i| PathBuf::from(format!("/repo/{i}")))
        .collect();
    assert_eq!(loaded.recent_repos(), expected);
}

#[test]
fn saving_creates_the_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("not")
        .join("created")
        .join("yet")
        .join(SETTINGS_FILE_NAME);

    settings::save(&path, &sample()).unwrap();

    assert_eq!(settings::load(&path), sample());
}

#[test]
fn saving_leaves_no_temporary_files_behind() {
    let dir = tempfile::tempdir().unwrap();
    let store = SettingsStore::load(settings_file(&dir));

    for i in 0..5 {
        store
            .update(|settings| settings.record_recent_repo(format!("/repo/{i}")))
            .unwrap();
    }

    assert_eq!(file_names(dir.path()), [SETTINGS_FILE_NAME]);
}

/// The store's copy only changes once the new settings are safely on disk,
/// so what the app shows never disagrees with what the next launch reads.
#[test]
fn a_failed_save_leaves_the_store_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the config directory should be: creating it fails on
    // every OS.
    let blocker = dir.path().join("not-a-directory");
    fs::write(&blocker, b"").unwrap();
    let store = SettingsStore::load(blocker.join(SETTINGS_FILE_NAME));

    let result = store.update(|settings| settings.record_recent_repo("/work/a"));

    assert!(result.is_err(), "saving under a regular file must fail");
    assert_eq!(store.get(), Settings::default());
}

/// Re-opening the repository that is already at the top of the recent list
/// is the common case; it must not rewrite the file every time.
#[test]
fn an_update_that_changes_nothing_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    let path = config_dir.join(SETTINGS_FILE_NAME);
    settings::save(&path, &sample()).unwrap();
    let store = SettingsStore::load(path);
    // Replace the config directory with a regular file: from here on any
    // attempt to save fails, so a write would show up as an error.
    fs::remove_dir_all(&config_dir).unwrap();
    fs::write(&config_dir, b"").unwrap();

    let unchanged = store
        .update(|settings| settings.record_recent_repo("/work/newest"))
        .expect("nothing changed, so nothing was written");

    assert_eq!(unchanged, sample());
    assert_eq!(store.get(), sample());
}

/// Atomic write: the new file is written beside the old one and renamed over
/// it, so the old file is never truncated or rewritten in place. A reader
/// that opened the old file keeps seeing the complete old contents.
///
/// Unix only because that observation depends on Unix rename semantics; on
/// Windows an open handle can block the rename instead. The code path is the
/// same on every OS and the other tests cover the replacement itself.
#[cfg(unix)]
#[test]
fn saving_replaces_the_file_instead_of_rewriting_it_in_place() {
    use std::io::Read;

    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);
    settings::save(&path, &Settings::default()).unwrap();
    let before = fs::read(&path).unwrap();
    let mut old_reader = fs::File::open(&path).unwrap();

    settings::save(&path, &sample()).unwrap();

    let mut seen_by_old_reader = Vec::new();
    old_reader.read_to_end(&mut seen_by_old_reader).unwrap();
    assert_eq!(seen_by_old_reader, before);
    assert_eq!(settings::load(&path), sample());
}

#[test]
fn the_settings_file_lives_in_the_app_config_dir() {
    let app = tauri::test::mock_app();
    let expected = app.path().app_config_dir().unwrap().join("settings.json");

    assert_eq!(settings::settings_path(app.handle()).unwrap(), expected);
}

// ---- P0-12: the app's use of the store -------------------------------------

/// JSON strings are Unicode, so a path that is not (possible on Linux) cannot
/// be saved; recording it would make every later save fail. Such a repository
/// still opens, it is just not remembered (ADR 0008).
#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_not_recorded_as_recent() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut settings = Settings::default();
    settings.record_recent_repo("/work/fine");

    settings.record_recent_repo(PathBuf::from(OsStr::from_bytes(b"/work/caf\xe9")));

    assert_eq!(settings.recent_repos(), [PathBuf::from("/work/fine")]);
    let dir = tempfile::tempdir().unwrap();
    settings::save(&settings_file(&dir), &settings).unwrap();
}

/// When Tauri cannot say where the config directory is (no home directory,
/// say), the app still starts: settings live in memory for the session.
#[test]
fn without_a_config_dir_the_store_keeps_settings_in_memory() {
    let store = settings::store_at(Err(settings::SettingsError::ConfigDir(
        tauri::Error::UnknownPath,
    )));

    assert_eq!(store.path(), None);
    assert_eq!(store.get(), Settings::default());
    let updated = store
        .update(|settings| {
            settings.record_recent_repo("/work/a");
        })
        .unwrap();
    assert_eq!(updated.recent_repos(), [PathBuf::from("/work/a")]);
    assert_eq!(store.get(), updated);
}

#[test]
fn with_a_config_dir_the_store_loads_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_file(&dir);
    settings::save(&path, &sample()).unwrap();

    let store = settings::store_at(Ok(path.clone()));

    assert_eq!(store.path(), Some(path.as_path()));
    assert_eq!(store.get(), sample());
}
