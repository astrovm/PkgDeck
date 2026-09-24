// Link the Qt dependency required by this package's generated QML initializer.
use pkgdeck as _;
use std::process::Command;

#[test]
fn packaged_desktop_entry_passes_supported_inputs_to_the_gui() {
    let desktop = include_str!("../../../assets/io.github.astrovm.PkgDeck.desktop");
    let value = |key: &str| {
        desktop
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap()
    };
    assert_eq!(value("Exec="), "pkgdeck %u");
    let mimes: Vec<_> = value("MimeType=")
        .split(';')
        .filter(|value| !value.is_empty())
        .collect();
    for supported in [
        "application/vnd.debian.binary-package",
        "application/vnd.flatpak.ref",
        "application/vnd.appimage",
        "x-scheme-handler/flatpak+https",
    ] {
        assert!(
            mimes.contains(&supported),
            "missing association for {supported}"
        );
    }
    assert!(
        include_str!("../../../packaging/flatpak/io.github.astrovm.PkgDeck.yml")
            .contains("assets/io.github.astrovm.PkgDeck.desktop /app/share/applications")
    );
    assert!(include_str!("../../../scripts/bundle.sh").contains("desktop:applications"));
    assert!(include_str!("../../../packaging/appimage/AppRun")
        .contains("exec \"$appdir/usr/bin/pkgdeck\" \"$@\""));
}

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

#[test]
fn https_screenshots_persist_across_restarts_and_expire() {
    let output = Command::new("bash")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/tests/media-cache.sh"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
