use std::env;
use std::fs;
use std::path::Path;

fn main() {
    tauri_build::build();
    embed_common_controls_manifest_in_tests();
}

/// Gives test executables the Common-Controls v6 manifest that `tauri_build`
/// gives only to the app binary.
///
/// `tauri_build` embeds the manifest through `embed-resource`, which emits
/// `cargo:rustc-link-arg-bins` — bin targets and nothing else. Any test that
/// links the Tauri runtime still imports `SetWindowSubclass`,
/// `RemoveWindowSubclass` and `DefSubclassProc` (through `muda`, pulled in by
/// tauri's default `common-controls-v6` feature), and ComCtl32 exports those
/// only in version 6. With no manifest to request it, the loader binds to the
/// 5.82 copy in System32, which does not export them, and the test process
/// dies at startup with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) before a
/// single test runs.
///
/// `tests/window_config.rs` survives without this because it only touches
/// `tauri::Config`, so the linker never pulls the runtime in.
fn embed_common_controls_manifest_in_tests() {
    // Guard on the target, not on `cfg!(windows)`: build scripts run on the
    // host, which is not necessarily what we are building for.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let Ok(out_dir) = env::var("OUT_DIR") else {
        return;
    };
    let manifest = Path::new(&out_dir).join("tests-common-controls-v6.manifest");

    if let Err(error) = fs::write(&manifest, COMMON_CONTROLS_V6_MANIFEST) {
        println!(
            "cargo:warning=could not write {}: {error}",
            manifest.display()
        );
        return;
    }

    // `/MANIFESTINPUT` is only honoured alongside `/MANIFEST:EMBED`.
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}

/// The dependency `tauri_build` puts in the app binary's manifest, kept
/// byte-identical so test binaries request exactly what the app requests.
const COMMON_CONTROLS_V6_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
