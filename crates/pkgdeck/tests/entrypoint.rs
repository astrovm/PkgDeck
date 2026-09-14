// Link the Qt dependency required by this package's generated QML initializer.
use pkgdeck as _;
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
fn package_window_loads_and_exits() {
    pkgdeck_tools::gui(&[env!("CARGO_BIN_EXE_pkgdeck").into()], false);
}

#[test]
fn invalid_qml_module_fails_without_hanging() {
    pkgdeck_tools::gui(&[env!("CARGO_BIN_EXE_pkgdeck").into()], true);
}

#[test]
fn quick_controls_search_confirm_resize_and_cancel() {
    pkgdeck_tools::qml();
}

#[test]
fn real_window_completes_synthetic_lifecycle() {
    pkgdeck_tools::gui_lifecycle(&[env!("CARGO_BIN_EXE_pkgdeck").into()]);
}
