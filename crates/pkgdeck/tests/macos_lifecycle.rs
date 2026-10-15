//! Exercise the real macOS window and status item when Accessibility is available.

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a graphical macOS session with Accessibility access; run with --ignored"]
fn native_menu_bar_survives_closing_and_reopens_the_window() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/tests/macos-lifecycle.sh");
    let status = std::process::Command::new("bash")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_pkgdeck"))
        .status()
        .expect("run native macOS lifecycle test");
    assert!(status.success(), "native lifecycle test failed: {status}");
}
