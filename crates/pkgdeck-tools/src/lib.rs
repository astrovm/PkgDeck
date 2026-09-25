//! Local acceptance drivers. All fixture state is disposable; no host writes.
use serde_json::{json, Value};
use std::{
    fs,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread::sleep,
    time::{Duration, Instant},
};

pub struct Temp(pub PathBuf);
impl Default for Temp {
    fn default() -> Self {
        Self::new()
    }
}
impl Temp {
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-tools-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub struct Process(pub Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn command(args: &[String]) -> Command {
    let mut c = Command::new(&args[0]);
    c.args(&args[1..]);
    c
}
pub fn run(c: &mut Command) -> String {
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "{c:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
/// Read a fixture log once the in-flight queries have stopped appending.
fn stable_bytes(path: &Path) -> Vec<u8> {
    let mut last = fs::read(path).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut unchanged_since = Instant::now();
    while Instant::now() < deadline {
        sleep(Duration::from_millis(40));
        let current = fs::read(path).unwrap_or_default();
        if current == last {
            if !last.is_empty() && unchanged_since.elapsed() >= Duration::from_millis(400) {
                return current;
            }
        } else {
            last = current;
            unchanged_since = Instant::now();
        }
    }
    panic!(
        "package queries did not settle at {}: {}",
        path.display(),
        String::from_utf8_lossy(&last)
    );
}
#[track_caller]
fn until(mut predicate: impl FnMut() -> bool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !predicate() {
        assert!(Instant::now() < deadline, "acceptance operation timed out");
        sleep(Duration::from_millis(20));
    }
}
fn fixture(batch: bool) -> Temp {
    use std::os::unix::fs::PermissionsExt;
    let dir = Temp::new();
    fs::write(
        dir.0.join("brew"),
        if batch {
            include_str!("../../pkd/tests/fixtures/brew_batch.sh")
        } else {
            include_str!("../../pkd/tests/fixtures/brew.sh")
        },
    )
    .unwrap();
    fs::set_permissions(dir.0.join("brew"), fs::Permissions::from_mode(0o755)).unwrap();
    dir
}
fn invocation(args: &[String], dir: &Path) -> Vec<String> {
    let mut result = vec![
        "/usr/bin/env".into(),
        "-u".into(),
        "SNAP".into(),
        "-u".into(),
        "FLATPAK_ID".into(),
        format!("HOME={}", dir.display()),
        format!("PATH={}", dir.display()),
    ];
    result.extend_from_slice(args);
    result.extend(["--from".into(), "homebrew".into()]);
    result
}
fn state(dir: &Path) -> Value {
    let path = dir.join("state.json");
    let raw =
        fs::read(&path).unwrap_or_else(|e| panic!("Cannot read fixture state at {path:?}: {e}"));
    serde_json::from_slice(&raw).unwrap()
}

pub struct Desktop {
    server: Process,
    process: Option<Process>,
    dir: Temp,
    display: String,
    window: String,
}
impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}
impl Desktop {
    pub fn new() -> Self {
        let dir = Temp::new();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700)).unwrap();
        let displayfile = dir.0.join("display");
        let output = fs::File::create(&displayfile).unwrap();
        let server = Process(
            Command::new("Xvfb")
                .args([
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    "1280x900x24",
                    "-nolisten",
                    "tcp",
                    "-ac",
                ])
                .stdout(output)
                .spawn()
                .unwrap(),
        );
        until(
            || fs::read_to_string(&displayfile).unwrap().contains('\n'),
            10,
        );
        let number = fs::read_to_string(displayfile).unwrap();
        assert!(number.trim().chars().all(|c| c.is_ascii_digit()));
        Self {
            server,
            process: None,
            dir,
            display: format!(":{}", number.trim()),
            window: String::new(),
        }
    }
    fn xcommand(&self) -> Command {
        let mut c = Command::new("timeout");
        c.args(["--kill-after=5s", "10s", "xdotool"]);
        c.env("DISPLAY", &self.display);
        c
    }
    pub fn xdo(&self, args: &[&str]) -> String {
        run(self.xcommand().args(args)).trim().to_string()
    }
    pub fn launch(&mut self, args: &[String]) {
        let log = fs::File::create(self.dir.0.join("application.log")).unwrap();
        self.process = Some(Process(
            command(args)
                .env("DISPLAY", &self.display)
                .env("QT_QPA_PLATFORM", "xcb")
                .env("QT_QUICK_BACKEND", "software")
                .env("QT_ACCESSIBILITY", "0")
                .env_remove("WAYLAND_DISPLAY")
                .env("XDG_RUNTIME_DIR", &self.dir.0)
                .env("XDG_CONFIG_HOME", &self.dir.0)
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        ));
        until(
            || {
                assert!(
                    self.process
                        .as_mut()
                        .unwrap()
                        .0
                        .try_wait()
                        .unwrap()
                        .is_none(),
                    "{}",
                    self.logs()
                );
                let out = self
                    .xcommand()
                    .args(["search", "--onlyvisible", "--name", "^PkgDeck"])
                    .output()
                    .unwrap();
                if out.status.success() {
                    self.window = String::from_utf8(out.stdout)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .into();
                    true
                } else {
                    false
                }
            },
            20,
        );
        // XCreateWindow precedes mapping. Even a visible search can race with
        // a resize/remap, so retry focus until X confirms the input target.
        until(
            || {
                self.xcommand()
                    .args(["windowfocus", &self.window])
                    .output()
                    .is_ok_and(|output| output.status.success())
            },
            20,
        );
        self.idle();
    }
    fn logs(&self) -> String {
        fs::read_to_string(self.dir.0.join("application.log")).unwrap_or_default()
    }
    pub fn idle(&mut self) {
        sleep(Duration::from_secs(2));
        assert!(
            self.process
                .as_mut()
                .unwrap()
                .0
                .try_wait()
                .unwrap()
                .is_none(),
            "{}",
            self.logs()
        );
    }
    pub fn key(&mut self, key: &str) {
        self.xdo(&["key", "--clearmodifiers", key]);
        self.idle();
    }
    pub fn search(&mut self, name: &str) {
        self.key("ctrl+1");
        self.key("ctrl+f");
        self.xdo(&["type", "--clearmodifiers", "--delay", "0", name]);
        self.key("Return");
        self.key("ctrl+l");
        self.key("Down");
    }
    pub fn write(&mut self, op: &str, name: &str) {
        if op == "update" {
            self.key("ctrl+5");
            self.key("ctrl+l");
            self.key("Down");
            self.key("ctrl+m");
        } else {
            self.search(name);
            self.key(match op {
                "install" => "ctrl+i",
                "remove" => "ctrl+d",
                "upgrade" => "ctrl+u",
                _ => panic!("unknown operation"),
            });
        }
        self.key(match op {
            "install" => "alt+i",
            "remove" | "update" => "alt+r",
            "upgrade" => "alt+u",
            _ => unreachable!(),
        });
    }
    pub fn close(mut self) {
        self.xdo(&["key", "--clearmodifiers", "ctrl+q"]);
        until(
            || {
                self.process
                    .as_mut()
                    .unwrap()
                    .0
                    .try_wait()
                    .unwrap()
                    .is_some()
            },
            10,
        );
        assert!(
            self.process.as_mut().unwrap().0.wait().unwrap().success(),
            "{}",
            self.logs()
        );
        assert!(
            !self.logs().contains("TypeError:") && !self.logs().contains("ReferenceError:"),
            "{}",
            self.logs()
        );
        let _ = &mut self.server;
    }
}
pub fn gui_lifecycle(args: &[String]) {
    let dir = fixture(false);
    let mut call = invocation(args, &dir.0);
    call.extend(["--auth".into(), "sudo".into()]);
    let mut gui = Desktop::new();
    gui.launch(&call);
    gui.search("fixture");
    // Selection can still be fetching details. Wait until that query lands
    // so the next arrow is what the assertion measures.
    let queries = stable_bytes(&dir.0.join("queries.log"));
    gui.key("Up");
    assert_eq!(fs::read(dir.0.join("queries.log")).unwrap(), queries);
    for (op, installed) in [("install", json!("1.0")), ("update", json!("1.0"))] {
        gui.write(op, "fixture");
        assert!(
            dir.0.join("state.json").exists(),
            "operation {op}: {}",
            gui.logs()
        );
        until(
            || {
                fs::read(dir.0.join("state.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|current| {
                        current["installed"] == installed
                            && (op != "update" || current["candidate"] == "2.0")
                    })
            },
            15,
        );
        assert_eq!(
            state(&dir.0)["installed"],
            installed,
            "operation {op}: {}",
            gui.logs()
        );
        if op == "update" {
            assert_eq!(
                state(&dir.0)["candidate"],
                "2.0",
                "operation {op}: {}",
                gui.logs()
            );
        }
    }
    gui.key("ctrl+2");
    gui.key("ctrl+3");
    gui.xdo(&["windowsize", &gui.window, "400", "520"]);
    gui.key("ctrl+f");
    gui.xdo(&["windowsize", &gui.window, "1100", "760"]);
    fs::write(dir.0.join("fail"), "").unwrap();
    gui.search("fixture");
    fs::remove_file(dir.0.join("fail")).unwrap();
    fs::write(dir.0.join("brew"), "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    gui.xdo(&["key", "--clearmodifiers", "ctrl+r"]);
    sleep(Duration::from_millis(300));
    gui.key("Escape");
    gui.close();
    for mode in ["success", "fail", "slow", "query-fails"] {
        let dir = fixture(true);
        let initial = json!({"fixture-a":"1","fixture-b":"1","fixture-current":"2"});
        fs::write(dir.0.join("state.json"), initial.to_string()).unwrap();
        if mode != "success" {
            fs::write(dir.0.join(mode), "").unwrap();
        }
        let mut call = invocation(args, &dir.0);
        call.extend(["--auth".into(), "sudo".into()]);
        let mut gui = Desktop::new();
        gui.launch(&call);
        gui.key("ctrl+3");
        gui.key("ctrl+shift+u");
        gui.key("alt+c");
        assert_eq!(state(&dir.0), initial, "{}", gui.logs());
        assert!(!dir.0.join("attempts").exists());
        gui.key("ctrl+shift+u");
        if mode == "slow" {
            gui.xdo(&["key", "--clearmodifiers", "alt+u"]);
            until(|| dir.0.join("started").exists(), 10);
            gui.key("Escape");
        } else {
            gui.key("alt+u");
        }
        let mut expected = initial;
        if matches!(mode, "success" | "slow") {
            expected["fixture-a"] = json!("2");
            expected["fixture-b"] = json!("2");
        }
        assert_eq!(state(&dir.0), expected, "mode {mode}: {}", gui.logs());
        if matches!(mode, "success" | "slow") {
            assert_eq!(fs::read_to_string(dir.0.join("attempts")).unwrap(), "all\n");
        } else {
            assert!(!dir.0.join("attempts").exists());
        }
        gui.key("ctrl+r");
        gui.close();
    }
}
pub fn gui(args: &[String], failure: bool) {
    let dir = Temp::new();
    let mut c = Command::new("timeout");
    c.args(["--kill-after=5s", "30s"]).args(args);
    c.env("QT_QPA_PLATFORM", "offscreen")
        .env("QT_QUICK_BACKEND", "software")
        .env("XDG_CONFIG_HOME", &dir.0)
        .env("XDG_DATA_HOME", &dir.0)
        .env("XDG_DATA_DIRS", &dir.0);
    if failure {
        let module = dir.0.join("org/kde/kirigami");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join("qmldir"),
            "module org.kde.kirigami\nHeading 1.0 Broken.qml\n",
        )
        .unwrap();
        fs::write(
            module.join("Broken.qml"),
            "import QtQuick\nItem { pkgdeckMissingProperty: true }\n",
        )
        .unwrap();
        c.env(
            "QML_IMPORT_PATH",
            format!(
                "{}:{}",
                dir.0.display(),
                std::env::var("QML_IMPORT_PATH").unwrap_or_default()
            ),
        );
    } else {
        c.arg("--smoke-test")
            .env("SNAP", "/synthetic-disabled-runtime");
    }
    let out = c.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(i32::from(failure)), "{stderr}");
    assert!(
        stderr.contains(if failure {
            "failed to load component"
        } else {
            "PKGDECK_GUI_READY"
        }),
        "{stderr}"
    );
}
pub fn qml() {
    let dir = Temp::new();
    run(Command::new("timeout")
        .args(["--kill-after=5s", "60s", "qmltestrunner"])
        .args([
            "-input",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pkgdeck/tests/qml"),
        ])
        .env("XDG_CONFIG_HOME", &dir.0)
        .env("XDG_DATA_HOME", &dir.0)
        .env("XDG_DATA_DIRS", &dir.0)
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("QT_QUICK_BACKEND", "software"));
}

pub fn apt_lock_probe() {
    assert_eq!(
        fs::read_to_string("/etc/pkgdeck-disposable-ci-runner")
            .unwrap()
            .trim(),
        "github-hosted-ubuntu-26.04"
    );
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/var/lib/dpkg/lock-frontend")
        .unwrap();
    let mut spec: libc::flock = unsafe { std::mem::zeroed() };
    spec.l_type = libc::F_WRLCK as _;
    spec.l_whence = libc::SEEK_SET as _;
    assert_eq!(
        unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_SETLK, &spec) },
        0
    );
    let out = Command::new("runuser")
        .args([
            "-u",
            "pkgdeck-test",
            "--",
            "/opt/pkgdeck-bin/examples/apt-probe",
            "sudo",
            "install",
            "pkgdeck-fixture",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("APT is busy"));
}
