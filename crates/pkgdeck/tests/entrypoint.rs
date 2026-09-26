// Link the Qt dependency required by this package's generated QML initializer.
use pkgdeck as _;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn second_launch_activates_existing_instance() {
    let runtime = std::env::temp_dir().join(format!("pkgdeck-instance-{}", std::process::id()));
    fs::create_dir_all(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    struct Running {
        child: Child,
        runtime: std::path::PathBuf,
    }
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_dir_all(&self.runtime);
        }
    }
    let launch = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pkgdeck"));
        command
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("QT_QUICK_BACKEND", "software")
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_CONFIG_HOME", &runtime)
            .env("XDG_DATA_HOME", &runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    };
    let mut first = Running {
        child: launch().spawn().unwrap(),
        runtime: runtime.clone(),
    };
    let socket = runtime.join(format!(
        "pkgdeck-open-{}",
        rustix::process::geteuid().as_raw()
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() && Instant::now() < deadline {
        assert!(
            first.child.try_wait().unwrap().is_none(),
            "first launch exited"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(socket.exists(), "first launch did not start its socket");
    let mut second = launch().spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while second.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let status = second.try_wait().unwrap();
    if status.is_none() {
        let _ = second.kill();
        let _ = second.wait();
    }
    assert!(
        status.is_some_and(|status| status.success()),
        "second launch stayed open"
    );
    assert!(
        first.child.try_wait().unwrap().is_none(),
        "first launch exited"
    );
}

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
        "application/vnd.flatpak",
        "application/vnd.flatpak.repo",
        "application/x-rpm",
        "application/vnd.snap",
        "application/x-pacman-package",
        "text/x-suse-ymp",
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
fn qml_component_tests_pass() {
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
