#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-03's acceptance criterion is a window titled with the app name. The
//! window is described entirely by `tauri.conf.json`, and opening a real one
//! needs a display that CI does not have, so this checks the identity of the
//! main window by reading that file the way Tauri reads it.

use tauri::Config;

/// Placeholder product name; the real one is an open question (SPEC §12).
/// Spelled out here so half a rename fails the build.
const APP_NAME: &str = "Shootgit";

fn config() -> Config {
    serde_json::from_str(include_str!("../tauri.conf.json"))
        .expect("tauri.conf.json parses as a Tauri config")
}

#[test]
fn the_main_window_is_titled_with_the_app_name() {
    let config = config();
    let windows = &config.app.windows;

    assert_eq!(windows.len(), 1, "Phase 0 opens exactly one window");

    let main = &windows[0];
    assert_eq!(
        main.label, "main",
        "capabilities/default.json grants permissions to the `main` window"
    );
    assert_eq!(main.title, APP_NAME);
    assert_eq!(config.product_name.as_deref(), Some(APP_NAME));
}

#[test]
fn the_bundle_identifier_is_still_a_placeholder() {
    assert_eq!(config().identifier, "dev.placeholder.shootgit");
}
