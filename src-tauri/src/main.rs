// A release build must not pop a console window on Windows (SPEC §9).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// CLAUDE.md: `unwrap`/`expect` are allowed in `main.rs` and tests only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

fn main() {
    app_lib::run().expect("the application window could not be started");
}
