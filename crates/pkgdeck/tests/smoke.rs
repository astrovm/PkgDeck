//! The real window opens on a private X display, draws a frame and exits.
#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

/// The X server, stopped however the test ends. Left running after a
/// failure, it keeps the test's output open and the run waits for it.
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_window_opens_on_a_real_display() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-smoke-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let displayfile = dir.join("display");
    let log = dir.join("xvfb.log");
    let mut server = Server(
        Command::new("Xvfb")
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
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .expect("Xvfb"),
    );
    // A busy CI runner can take a while to start it.
    let started = Instant::now();
    let display = loop {
        let number = fs::read_to_string(&displayfile).unwrap();
        if number.ends_with('\n') {
            break format!(":{}", number.trim());
        }
        let said = || fs::read_to_string(&log).unwrap_or_default();
        if let Some(status) = server.0.try_wait().unwrap() {
            panic!("Xvfb stopped ({status}): {}", said());
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "Xvfb didn't start in a minute: {}",
            said()
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
        // Never the real home: it would read and save the real settings,
        // and check the real tools for updates.
        .env("HOME", &dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .output()
        .unwrap();
    drop(server);
    let _ = fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("PKGDECK_GUI_READY"), "{stdout}{stderr}");
}
