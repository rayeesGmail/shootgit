//! The settings store: `settings.json` in Tauri's `app_config_dir` (SPEC §4
//! Data at rest).
//!
//! It holds the recent repositories and the git path override that §5 Git
//! binary resolution tries first. Settings are part of the startup diet (§4
//! Low-resource operation, rule 9), so [`crate::run`] loads them once at
//! launch into a [`SettingsStore`] kept in Tauri's managed state.
//!
//! Two rules keep the file safe:
//!
//! - **Saving is atomic.** [`save`] writes a temporary file in the same
//!   directory, flushes it to disk and renames it over `settings.json`. A
//!   crash or power cut mid-save leaves either the old file or the new one,
//!   never a truncated mix.
//! - **Loading never fails.** A missing file means defaults. A file that
//!   cannot be parsed is moved aside to `settings.json.corrupt`, so the next
//!   save does not destroy what the user may want to recover by hand, and
//!   the app starts with defaults. A broken settings file must never stop the
//!   app from opening, and neither does a config directory Tauri cannot
//!   locate: then the settings live in memory for the session
//!   ([`store_at`]).
//!
//! The file is JSON, whose strings are Unicode, so every path in it is valid
//! UTF-8. A repository whose path is not (possible on Linux) opens but is
//! not recorded as recent (ADR 0008).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Deserializer, Serialize};
use tauri::{Manager, Runtime};

/// The file's name inside `app_config_dir`.
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// How many repositories the recent list remembers.
pub const MAX_RECENT_REPOS: usize = 20;

/// Appended to the settings path to name the copy of a corrupt file.
const CORRUPT_SUFFIX: &str = ".corrupt";

/// Everything the app remembers between launches.
///
/// Serialised as a JSON object whose field names are the file format; a
/// field that is missing from the file takes its default, and a field the
/// app does not know is ignored, so a file written by an older or newer
/// version still loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Most recent first, no duplicates, at most [`MAX_RECENT_REPOS`].
    /// Private so those rules hold: change it through
    /// [`Settings::record_recent_repo`].
    #[serde(deserialize_with = "deserialize_recent_repos")]
    recent_repos: Vec<PathBuf>,

    /// The git executable chosen by the user, if any. It is the first
    /// candidate of §5 Git binary resolution
    /// (`git_engine::git_binary::ResolveOptions::settings_path`); `None`
    /// means detect it.
    pub git_path: Option<PathBuf>,
}

impl Settings {
    /// Recently opened repositories, most recent first.
    pub fn recent_repos(&self) -> &[PathBuf] {
        &self.recent_repos
    }

    /// Moves `path` to the front of the recent list, adding it if it is new
    /// and dropping the oldest entry beyond [`MAX_RECENT_REPOS`].
    ///
    /// A path that is not valid UTF-8 is left out: the file could not hold
    /// it, and every later save would fail (ADR 0008).
    ///
    /// Paths are compared exactly, so pass the canonical path that
    /// `git_engine::repo::open_repo` returns: otherwise one repository
    /// reached through a symlink, or spelled with different letter case on a
    /// case-insensitive file system, would take two places.
    pub fn record_recent_repo(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if path.to_str().is_none() {
            return;
        }
        self.recent_repos.retain(|known| *known != path);
        self.recent_repos.insert(0, path);
        self.recent_repos.truncate(MAX_RECENT_REPOS);
    }
}

/// Reads the recent list and restores its rules, which a hand-edited file
/// may break: the first occurrence of a duplicate wins and the list is cut
/// at [`MAX_RECENT_REPOS`].
fn deserialize_recent_repos<'de, D>(deserializer: D) -> Result<Vec<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let listed = Vec::<PathBuf>::deserialize(deserializer)?;
    let mut repos = Vec::with_capacity(listed.len().min(MAX_RECENT_REPOS));
    for path in listed {
        if repos.len() == MAX_RECENT_REPOS {
            break;
        }
        if !repos.contains(&path) {
            repos.push(path);
        }
    }
    Ok(repos)
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SettingsError {
    /// Tauri could not tell where the app's config directory is (no home
    /// directory, for example).
    #[error("the app config directory could not be determined")]
    ConfigDir(#[source] tauri::Error),
    /// The settings could not be turned into JSON. serde_json does this for
    /// a path that is not valid UTF-8.
    #[error("the settings could not be serialised")]
    Serialize(#[source] serde_json::Error),
    #[error("could not write the settings to {path:?}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// `app_config_dir/settings.json` for this app.
pub fn settings_path<R: Runtime, M: Manager<R>>(app: &M) -> Result<PathBuf, SettingsError> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(SettingsError::ConfigDir)?;
    Ok(dir.join(SETTINGS_FILE_NAME))
}

/// Reads the settings at `path`, falling back to the defaults.
///
/// Never fails. A missing file gives the defaults. A file that does not
/// parse gives the defaults and is renamed to `<path>.corrupt` (replacing an
/// older one), so the next [`save`] cannot overwrite it. A file that cannot
/// be read at all (permissions, say) gives the defaults and is left alone.
/// Only the rename writes to disk.
pub fn load(path: &Path) -> Settings {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::debug!(path = %path.display(), "no settings file yet; using the defaults");
            return Settings::default();
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "the settings file could not be read; using the defaults",
            );
            return Settings::default();
        }
    };

    match parse(&bytes) {
        Ok(settings) => settings,
        Err(error) => {
            let kept = corrupt_copy_path(path);
            match fs::rename(path, &kept) {
                Ok(()) => tracing::warn!(
                    path = %path.display(),
                    kept = %kept.display(),
                    %error,
                    "the settings file is corrupt; moved it aside and using the defaults",
                ),
                Err(rename_error) => tracing::warn!(
                    path = %path.display(),
                    %error,
                    %rename_error,
                    "the settings file is corrupt and could not be moved aside; using the defaults",
                ),
            }
            Settings::default()
        }
    }
}

/// Parses the contents of a settings file, which must be a JSON object.
///
/// Going through a map first rejects a JSON array: serde's derived
/// deserializer would otherwise accept one as the fields in declaration
/// order, so reordering fields would silently change what such a file means.
fn parse(bytes: &[u8]) -> Result<Settings, serde_json::Error> {
    let fields: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(bytes)?;
    serde_json::from_value(serde_json::Value::Object(fields))
}

/// The store for the settings file at `path`, or, when there is no path
/// because Tauri could not locate the config directory, a store that keeps
/// the settings in memory for the session.
///
/// `run()` calls it with [`settings_path`]. Losing the settings of one
/// session is better than an app that does not start, the same trade-off as
/// a corrupt file.
pub fn store_at(path: Result<PathBuf, SettingsError>) -> SettingsStore {
    match path {
        Ok(path) => SettingsStore::load(path),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "no config directory; settings are kept in memory and not saved",
            );
            SettingsStore::in_memory()
        }
    }
}

/// Writes `settings` to `path` atomically, creating the directory if needed.
///
/// The JSON goes to a temporary file beside `path`, is flushed to disk, and
/// the temporary file is renamed over `path`. On failure `path` is untouched
/// and the temporary file is removed. Blocking, including an `fsync`: call
/// it off the main thread.
pub fn save(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let mut json = serde_json::to_vec_pretty(settings).map_err(SettingsError::Serialize)?;
    json.push(b'\n');
    write_atomically(path, &json).map_err(|source| SettingsError::Write {
        path: path.to_owned(),
        source,
    })
}

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    // A bare file name has an empty parent; the file then lives in the
    // current directory.
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;

    // Same directory, so the rename below stays on one file system and is
    // atomic. `tempfile` picks an unused name and deletes the file again if
    // anything fails before it is persisted.
    let mut temp = tempfile::Builder::new()
        .prefix(".settings-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    temp.write_all(contents)?;
    // Without this, a crash soon after the rename can leave an empty file on
    // file systems that delay allocation (ext4, for one).
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn corrupt_copy_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(CORRUPT_SUFFIX);
    PathBuf::from(name)
}

/// The app's settings: the copy in memory and the file it is saved to, if
/// any.
///
/// Reads come from memory. Every change goes through [`SettingsStore::update`],
/// which saves before the copy in memory changes, so the two never disagree.
#[derive(Debug)]
pub struct SettingsStore {
    /// `None` for [`SettingsStore::in_memory`].
    path: Option<PathBuf>,
    current: Mutex<Settings>,
}

impl SettingsStore {
    /// Loads the settings at `path` (see [`load`] for what happens when the
    /// file is missing or corrupt).
    pub fn load(path: PathBuf) -> Self {
        let settings = load(&path);
        Self {
            path: Some(path),
            current: Mutex::new(settings),
        }
    }

    /// A store with the default settings that saves nothing: updates apply
    /// for the session only (see [`store_at`]).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            current: Mutex::new(Settings::default()),
        }
    }

    /// The file this store saves to; `None` when it keeps the settings in
    /// memory only.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// A copy of the current settings.
    pub fn get(&self) -> Settings {
        self.lock().clone()
    }

    /// Applies `change` and saves the result, returning the new settings.
    ///
    /// Updates are serialised. If saving fails, the error is returned and
    /// the settings stay as they were, in memory and on disk. A change that
    /// leaves the settings equal to what they were writes nothing, and an
    /// in-memory store never writes. Blocking, like [`save`]: call it off the
    /// main thread (the app's commands use `spawn_blocking`).
    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings, SettingsError> {
        let mut current = self.lock();
        let mut next = current.clone();
        change(&mut next);
        if next != *current {
            if let Some(path) = &self.path {
                save(path, &next)?;
            }
            *current = next.clone();
        }
        Ok(next)
    }

    /// A panic inside an `update` closure poisons the lock, but it cannot
    /// leave the settings half-changed: the closure only ever sees a copy.
    fn lock(&self) -> MutexGuard<'_, Settings> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
