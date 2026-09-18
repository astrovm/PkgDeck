//! Local acceptance drivers. All fixture state is disposable; no host writes.
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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
    serde_json::from_slice(&fs::read(dir.join("state.json")).unwrap()).unwrap()
}

pub struct Terminal {
    child: Process,
    master: fs::File,
    before: libc::termios,
    screen: Vec<Vec<char>>,
    pending: Vec<u8>,
    row: usize,
    col: usize,
    pub output: String,
}
impl Terminal {
    pub fn new(args: &[String]) -> Self {
        // openpty owns both descriptors; pre_exec only makes async-signal-safe calls.
        let (mut master, mut slave) = (-1, -1);
        let size = libc::winsize {
            ws_row: 40,
            ws_col: 160,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        let master = unsafe { fs::File::from_raw_fd(master) };
        let slave = unsafe { fs::File::from_raw_fd(slave) };
        let mut before = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(master.as_raw_fd(), &mut before) },
            0
        );
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
        let mut c = command(args);
        c.env("TERM", "xterm-256color")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            child: Process(c.spawn().unwrap()),
            master,
            before,
            screen: vec![vec![' '; 160]; 40],
            pending: vec![],
            row: 0,
            col: 0,
            output: String::new(),
        }
    }
    pub fn send(&mut self, bytes: &[u8]) {
        self.output.clear();
        self.master.write_all(bytes).unwrap();
    }
    fn drain(&mut self) {
        let mut bytes = [0; 65536];
        while let Ok(n) = self.master.read(&mut bytes) {
            if n == 0 {
                break;
            }
            self.output.push_str(&String::from_utf8_lossy(&bytes[..n]));
            self.pending.extend_from_slice(&bytes[..n]);
        }
        while !self.pending.is_empty() {
            if self.pending[0] == 27 {
                if self.pending.len() < 2 {
                    break;
                }
                if self.pending[1] != b'[' {
                    self.pending.drain(..2);
                    continue;
                }
                let Some(end) =
                    (2..self.pending.len()).find(|&i| (0x40..=0x7e).contains(&self.pending[i]))
                else {
                    break;
                };
                let params = String::from_utf8_lossy(&self.pending[2..end]);
                let nums: Vec<usize> = params
                    .trim_start_matches('?')
                    .split(';')
                    .map(|s| s.parse().unwrap_or(0))
                    .collect();
                match self.pending[end] {
                    b'H' | b'f' => {
                        self.row = nums[0].max(1) - 1;
                        self.col = nums.get(1).copied().unwrap_or(1).max(1) - 1;
                    }
                    b'J' if matches!(nums[0], 2 | 3) => self.screen = vec![vec![' '; 160]; 40],
                    _ => {}
                }
                self.pending.drain(..=end);
                continue;
            }
            let width = match self.pending[0] {
                0..=127 => 1,
                192..=223 => 2,
                224..=239 => 3,
                _ => 4,
            };
            if self.pending.len() < width {
                break;
            }
            let ch = String::from_utf8_lossy(&self.pending[..width])
                .chars()
                .next()
                .unwrap();
            self.pending.drain(..width);
            match ch {
                '\r' => self.col = 0,
                '\n' => self.row += 1,
                c if c >= ' ' => {
                    if self.row < 40 && self.col < 160 {
                        self.screen[self.row][self.col] = c;
                    }
                    self.col += 1;
                }
                _ => {}
            }
        }
    }
    pub fn wait(&mut self, text: &str) {
        until(
            || {
                self.drain();
                self.screen
                    .iter()
                    .map(|r| r.iter().collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .contains(text)
            },
            240,
        );
    }
    pub fn close(mut self) {
        self.send(b"q");
        self.finish();
    }
    fn finish(&mut self) {
        until(|| self.child.0.try_wait().unwrap().is_some(), 5);
        assert!(self.child.0.wait().unwrap().success(), "{}", self.output);
        let mut after = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.master.as_raw_fd(), &mut after) },
            0
        );
        let mask = libc::ECHO | libc::ICANON;
        assert_eq!(self.before.c_lflag & mask, after.c_lflag & mask);
    }
}
pub fn tui_write(args: &[String], operation: &str, name: &str) {
    let mut t = Terminal::new(args);
    t.wait("Press /");
    if operation == "update" {
        t.send(b"4");
        t.wait("Select a source");
        t.send(b"u");
        t.wait("Confirm Refresh");
    } else {
        t.send(format!("/{name}\r").as_bytes());
        t.wait("1 packages");
        t.send(b"\r");
        t.wait("Homepage:");
        t.send(match operation {
            "install" => b"i",
            "upgrade" => b"g",
            "remove" => b"d",
            _ => panic!("unknown operation"),
        });
        t.wait("Confirm ");
    }
    t.send(b"y");
    t.wait("Completed.");
    t.close();
}
pub fn terminal(args: &[String]) {
    for key in [Some(b"q".as_slice()), Some(b"\x1b"), Some(b"\x03"), None] {
        let mut t = Terminal::new(args);
        t.wait("Search");
        t.send(b" ");
        until(
            || {
                t.drain();
                t.output.contains("\x1b[?25l")
            },
            15,
        );
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { libc::ioctl(t.master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
        until(
            || {
                t.drain();
                t.output.contains("\x1b[2J")
            },
            15,
        );
        if let Some(k) = key {
            t.send(k)
        } else {
            // Package launchers may retain a supervising process. Exercise the
            // frontend's signal handler without killing that supervisor first.
            fn frontend(pid: u32) -> Option<u32> {
                if fs::read_to_string(format!("/proc/{pid}/comm"))
                    .is_ok_and(|name| name.trim() == "pkd")
                {
                    return Some(pid);
                }
                fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
                    .ok()?
                    .split_whitespace()
                    .filter_map(|id| id.parse().ok())
                    .find_map(frontend)
            }
            let pid = frontend(t.child.0.id()).expect("running terminal frontend");
            unsafe {
                assert_eq!(libc::kill(pid as i32, libc::SIGTERM), 0);
            }
        }
        t.finish();
    }
    let mut master = -1;
    let mut slave = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let _master = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    for redirected_input in [true, false] {
        let mut c = command(args);
        if redirected_input {
            c.stdin(Stdio::null()).stdout(slave.try_clone().unwrap());
        } else {
            c.stdin(slave.try_clone().unwrap()).stdout(Stdio::piped());
        }
        let out = c.output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("requires a terminal"));
    }
}
pub fn terminal_interactions(args: &[String]) {
    let mut disabled = vec![
        "/usr/bin/env".into(),
        "SNAP=/synthetic-disabled-runtime".into(),
        "PATH=/synthetic-empty-path".into(),
    ];
    disabled.extend_from_slice(args);
    disabled.extend(["--from".into(), "homebrew".into()]);
    let mut t = Terminal::new(&disabled);
    t.wait("Press /");
    t.send(b"/synthetic\r");
    t.wait("PkgDeck - Search");
    t.wait("disabled");
    t.close();
    let dir = fixture(false);
    let call = invocation(args, &dir.0);
    for op in ["install", "update", "upgrade", "remove"] {
        tui_write(&call, op, "fixture");
    }
    fs::write(
        dir.0.join("brew"),
        "#!/bin/sh\n: > \"$HOME/started\"\nexec /bin/sleep 30\n",
    )
    .unwrap();
    let mut t = Terminal::new(&call);
    t.wait("Press /");
    t.send(b"/fixture\r");
    until(|| dir.0.join("started").exists(), 10);
    t.send(b"\x1b");
    t.wait("cancelled");
    t.close();
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
                    .args(["search", "--name", "^PkgDeck"])
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
        self.xdo(&["windowfocus", "--sync", &self.window]);
        self.idle();
    }
    fn logs(&self) -> String {
        fs::read_to_string(self.dir.0.join("application.log")).unwrap_or_default()
    }
    pub fn idle(&mut self) {
        sleep(Duration::from_millis(150));
        let mut stable = 0;
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
                if self
                    .xdo(&["getwindowname", &self.window])
                    .contains("Working")
                {
                    stable = 0
                } else {
                    stable += 1
                };
                sleep(Duration::from_millis(30));
                stable >= 5
            },
            240,
        );
    }
    pub fn key(&mut self, key: &str) {
        self.xdo(&["key", "--clearmodifiers", key]);
        self.idle();
    }
    pub fn search(&mut self, name: &str) {
        self.key("ctrl+f");
        self.xdo(&["type", "--clearmodifiers", "--delay", "0", name]);
        self.key("Return");
        self.key("ctrl+l");
        self.key("Down");
    }
    pub fn write(&mut self, op: &str, name: &str) {
        if op == "update" {
            self.key("ctrl+4");
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
        self.key("alt+y");
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
    let queries = fs::read(dir.0.join("queries.log")).unwrap();
    gui.key("Up");
    assert_eq!(fs::read(dir.0.join("queries.log")).unwrap(), queries);
    gui.key("ctrl+r");
    gui.key("ctrl+l");
    gui.key("Down");
    assert!(fs::read(dir.0.join("queries.log")).unwrap().len() > queries.len());
    for (op, installed) in [
        ("install", json!("1.0")),
        ("update", json!("1.0")),
        ("upgrade", json!("2.0")),
        ("remove", Value::Null),
    ] {
        gui.write(op, "fixture");
        assert_eq!(state(&dir.0)["installed"], installed, "{}", gui.logs());
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
        gui.key("alt+n");
        assert_eq!(state(&dir.0), initial);
        assert!(!dir.0.join("attempts").exists());
        gui.key("ctrl+shift+u");
        if mode == "slow" {
            gui.xdo(&["key", "--clearmodifiers", "alt+y"]);
            until(|| dir.0.join("started").exists(), 10);
            gui.key("Escape");
        } else {
            gui.key("alt+y");
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
        fs::read_to_string("/etc/pkgdeck-disposable-vm")
            .unwrap()
            .trim(),
        "host-execution-test"
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
            "/mnt/pkgdeck-bin/examples/apt-probe",
            "sudo",
            "install",
            "pkgdeck-fixture",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("APT is busy"));
}
