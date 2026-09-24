#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-04: the sample command `ping` has to round-trip over the real Tauri IPC
//! bridge, through the invoke handler that `tauri_specta::Builder` generates —
//! not through a direct Rust call, which would prove nothing about the wiring
//! the frontend depends on (SPEC §4 IPC contract).
//!
//! `tauri::test`'s mock runtime runs that bridge without a display, so these
//! tests work on a headless CI runner on all three OSes.

use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};

/// An app wired exactly the way `app_lib::run` wires the real one: the invoke
/// handler comes from the specta builder, so a command that is not registered
/// there is not reachable from the frontend either.
fn test_app() -> App<MockRuntime> {
    let specta = app_lib::ipc::builder::<MockRuntime>();
    mock_builder()
        .invoke_handler(specta.invoke_handler())
        .build(mock_context(noop_assets()))
        .expect("the mock app builds")
}

fn main_webview(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .expect("the mock webview builds")
}

/// The origin the WebView posts from; Tauri rejects requests that claim
/// another one.
fn invoke(cmd: &str) -> InvokeRequest {
    let url = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    InvokeRequest {
        cmd: cmd.to_owned(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: url.parse().expect("the WebView origin is a valid URL"),
        body: InvokeBody::default(),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_owned(),
    }
}

#[test]
fn ping_answers_pong_across_the_ipc_bridge() {
    let app = test_app();
    let webview = main_webview(&app);

    let response = tauri::test::get_ipc_response(&webview, invoke("ping"))
        .expect("`ping` is registered on the invoke handler");

    assert_eq!(
        response.deserialize::<String>().unwrap(),
        "pong",
        "the frontend must get a plain JSON string back"
    );
}

#[test]
fn a_command_the_builder_does_not_know_is_not_reachable() {
    // Guards against `invoke_handler` being wired to `tauri::generate_handler!`
    // instead of the specta builder: then commands could exist in Rust without
    // a generated TypeScript binding, which is exactly the drift ADR 0003
    // exists to prevent.
    let app = test_app();
    let webview = main_webview(&app);

    assert!(
        tauri::test::get_ipc_response(&webview, invoke("pong")).is_err(),
        "only commands collected by the specta builder are exposed"
    );
}
