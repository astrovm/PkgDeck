//! The real window opens on a private X display, draws a frame and exits.
#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[test]
fn the_window_opens_on_a_real_display() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-smoke-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let displayfile = dir.join("display");
    let mut server = Command::new("Xvfb")
        .args([
            "-displayfd",
            "1",
            "-screen",
            "0",
            "1280x900x24",
            "-nolisten",
            "tcp",
        ])
        .stdout(fs::File::create(&displayfile).unwrap())
        .spawn()
        .expect("Xvfb");
    let started = Instant::now();
    let display = loop {
        let number = fs::read_to_string(&displayfile).unwrap();
        if number.ends_with('\n') {
            break format!(":{}", number.trim());
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Xvfb never started"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let output = Command::new("timeout")
        .args([
            "--kill-after=5s",
            "60s",
            env!("CARGO_BIN_EXE_pkgdeck"),
            "--smoke-test",
        ])
        .env("DISPLAY", &display)
        .env_remove("WAYLAND_DISPLAY")
        .env("XDG_RUNTIME_DIR", &dir)
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .output()
        .unwrap();
    let _ = server.kill();
    let _ = server.wait();
    let _ = fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("PKGDECK_GUI_READY"), "{stdout}{stderr}");
}
