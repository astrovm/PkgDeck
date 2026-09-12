// Link the Qt dependency required by this package's generated QML initializer.
use cxx_qt_lib as _;
use std::process::Command;

#[test]
fn version_does_not_require_a_display() {
    let output = Command::new(env!("CARGO_BIN_EXE_pkgdeck"))
        .arg("--version")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("pkgdeck {}", pkgdeck_core::VERSION)
    );
}

#[test]
fn kirigami_window_loads_and_exits() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/smoke.py"
        ))
        .args(["gui", env!("CARGO_BIN_EXE_pkgdeck")])
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("QT_QUICK_BACKEND", "software")
        .status()
        .unwrap();
    assert!(status.success());
}
