//! End-to-end paths of the `pkd` binary against synthetic managers in a
//! temporary PATH and HOME.
use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-cli-paths-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.executable("brew", include_str!("fixtures/brew.sh"));
        fixture
    }
    fn executable(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pkd"));
        command
            .env("PATH", &self.0)
            .env("HOME", &self.0)
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("SNAP")
            .env_remove("FLATPAK_ID")
            .env_remove("NO_COLOR");
        command
    }
    fn json(&self, args: &[&str]) -> (Value, i32) {
        let output = self.command().arg("--json").args(args).output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (value["data"].clone(), output.status.code().unwrap())
    }
    /// Runs `pkd` in a pseudo-terminal, typing `answer`.
    fn terminal(&self, args: &str, answer: &str) -> Output {
        let timeout = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join("timeout"))
            .find(|path| path.is_file())
            .expect("GNU timeout (coreutils) is required for terminal tests");
        let mut command = Command::new(timeout);
        command.args(["15s", "/usr/bin/script"]);
        let invocation = format!("exec \"$PKGDECK_TEST_CLI\" {args}");
        if cfg!(target_os = "macos") {
            command.args(["-q", "/dev/null", "/bin/sh", "-c", &invocation]);
        } else {
            command.args(["-qec", &invocation, "/dev/null"]);
        }
        command
            .env("PKGDECK_TEST_CLI", env!("CARGO_BIN_EXE_pkd"))
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .env("PATH", &self.0)
            .env("HOME", &self.0)
            .env_remove("XDG_STATE_HOME")
            .env_remove("SNAP")
            .env_remove("FLATPAK_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        // Keep the pipe open until the command exits so the answer is never
        // replaced by end of input.
        let mut input = child.stdin.take().unwrap();
        input.write_all(answer.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        drop(input);
        output
    }
    fn state(&self) -> Option<Value> {
        let bytes = fs::read(self.0.join("state.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> bool {
    rustix::process::geteuid().is_root()
}

#[test]
fn inspect_reads_the_inventory_of_a_manager_that_owns_the_file() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("state.json"),
        r#"{"installed":"1.0","candidate":"1.0"}"#,
    )
    .unwrap();
    let prefix = fixture.0.join("prefix");
    let tool = fixture.executable("prefix/Cellar/fixture/1.0/bin/fixture-tool", "#!/bin/sh\n");
    fs::create_dir_all(fixture.0.join("bin")).unwrap();
    symlink(&tool, fixture.0.join("bin/fixture-tool")).unwrap();
    let path = std::env::join_paths([&fixture.0, &fixture.0.join("bin")]).unwrap();
    let inspect = |from: &[&str]| {
        let output = fixture
            .command()
            .env("PATH", &path)
            .env("HOMEBREW_PREFIX", &prefix)
            .arg("--json")
            .args(from)
            .args(["inspect", "fixture-tool"])
            .output()
            .unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = value["data"]["inspection"]["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["path"]
                    .as_str()
                    .unwrap()
                    .ends_with("bin/fixture-tool")
            })
            .cloned()
            .unwrap();
        (
            candidate["owners"][0].clone(),
            output.status.code().unwrap(),
        )
    };
    // Homebrew was not read up front; its folder names it, so it is read next.
    let (owner, code) = inspect(&[]);
    assert_eq!(code, 0, "{owner}");
    assert_eq!(owner["manager"], "homebrew");
    assert_eq!(owner["state"], "known", "{owner}");
    assert_eq!(owner["packages"][0]["name"], "fixture");
    // A failed read of that manager is reported with the owner unmatched.
    fs::write(fixture.0.join("fail"), "").unwrap();
    let (owner, code) = inspect(&[]);
    assert_eq!(code, 8, "{owner}");
    assert_eq!(owner["state"], "unmatched", "{owner}");
    fs::remove_file(fixture.0.join("fail")).unwrap();
    // Other sources, or none that can own files, leave the owner unmatched.
    let (owner, _) = inspect(&["--from", "dnf"]);
    assert_eq!(owner["state"], "unmatched", "{owner}");
    let (owner, code) = inspect(&["--from", "flatpak"]);
    assert_eq!(code, 0);
    assert_eq!(owner["state"], "unmatched", "{owner}");
}

#[test]
fn a_closed_output_is_an_error() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["--json", "--from", "homebrew", "sources"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    assert_eq!(child.wait().unwrap().code(), Some(1));
}

#[test]
fn polkit_approval_skips_the_sudo_login() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args([
            "--from", "homebrew", "--auth", "polkit", "--yes", "install", "fixture",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    if root() {
        assert!(!output.status.success(), "{text}");
    } else {
        assert!(output.status.success(), "{text}");
        assert!(text.contains("Done. 1 change applied."), "{text}");
        assert_eq!(fixture.state().unwrap()["installed"], "1.0");
    }
    let (data, code) = fixture.json(&["--from", "homebrew", "list"]);
    assert_eq!(code, 0, "{data}");
}

#[test]
fn repository_changes_are_confirmed_in_a_terminal() {
    let fixture = Fixture::new();
    let record = fixture.0.join("repository-call");
    fixture.executable(
        "flatpak",
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  *remotes*) printf 'fixture\\tFixture\\thttps://example.invalid\\t1\\n' ;;\n  *) printf '%s\\n' \"$@\" > '{}' ;;\nesac\n",
            record.display()
        ),
    );
    let args = "--from flatpak --scope user repos disable fixture";
    let declined = fixture.terminal(args, "\n");
    let text = String::from_utf8_lossy(&declined.stdout);
    assert_eq!(declined.status.code(), Some(7), "{text}");
    assert!(text.contains("Disable repository"), "{text}");
    assert!(
        text.contains("Apply this repository change? [y/N]"),
        "{text}"
    );
    assert!(!record.exists());
    if !root() {
        let approved = fixture.terminal(args, "y\n");
        let text = String::from_utf8_lossy(&approved.stdout);
        assert!(approved.status.success(), "{text}");
        assert!(text.contains("Repository operation completed"), "{text}");
        let call = fs::read_to_string(&record).unwrap();
        assert!(
            call.contains("remote-modify\n--disable\nfixture\n"),
            "{call}"
        );
    }
}
