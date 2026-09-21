use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let brew = path.join("brew");
        fs::write(&brew, include_str!("fixtures/brew.sh")).unwrap();
        fs::set_permissions(brew, fs::Permissions::from_mode(0o755)).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pkd"));
        command
            .env("PATH", &self.0)
            .env("HOME", &self.0)
            .env_remove("SNAP")
            .env_remove("FLATPAK_ID")
            .args(["--from", "homebrew"]);
        command
    }
    fn terminal_install(&self, answer: &str, no_color: bool) -> Output {
        let mut command = Command::new("/usr/bin/timeout");
        command
            .args(["15s", "/usr/bin/script", "-qec"])
            .arg("exec \"$PKGDECK_TEST_CLI\" --from homebrew install fixture")
            .arg("/dev/null")
            .env("PKGDECK_TEST_CLI", env!("CARGO_BIN_EXE_pkd"))
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("PATH", &self.0)
            .env("HOME", &self.0)
            .env_remove("SNAP")
            .env_remove("FLATPAK_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if no_color {
            command.env("NO_COLOR", "1");
        } else {
            command.env_remove("NO_COLOR");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answer.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
    fn call(&self, args: &[&str], code: i32) -> Value {
        let output = self.command().arg("--json").args(args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["exit_code"], code);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic native progress"));
        value["data"].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn commands_use_sanitized_host_transport_and_one_json_document() {
    let fixture = Fixture::new();
    fixture.call(&["sources"], 0);
    assert_eq!(
        fixture.call(&["search", "fixture"], 0)["packages"][0]["id"]["name"],
        "fixture"
    );
    fixture.call(&["info", "fixture"], 0);
    fixture.call(&["doctor"], 2);
    fixture.call(&["install", "fixture"], 2);
    if !rustix_root() {
        fixture.call(&["--yes", "install", "fixture"], 0);
        fixture.call(&["--yes", "update"], 0);
        assert_eq!(
            fixture.call(&["list"], 0)["packages"][0]["update"],
            "available"
        );
        fixture.call(&["--yes", "upgrade", "fixture"], 0);
        assert_eq!(
            fixture.call(&["list"], 0)["packages"][0]["installed_version"],
            "2.0"
        );
        fixture.call(&["--yes", "remove", "fixture"], 0);
    }
    assert!(fixture
        .command()
        .args(["search", "fixture"])
        .output()
        .unwrap()
        .status
        .success());
    fs::write(fixture.0.join("fail"), "").unwrap();
    fixture.call(&["sources"], 1);
    fixture.call(&["search", "fixture"], 8);
    fs::remove_file(fixture.0.join("brew")).unwrap();
    fixture.call(&["sources"], 0);
    fixture.call(&["--yes", "update"], 1);
}
fn rustix_root() -> bool {
    std::env::var("USER").is_ok_and(|v| v == "root")
}
#[test]
fn sandbox_commands_fail_closed() {
    for args in [
        vec!["--json", "--from", "apt", "search", "fixture"],
        vec!["--json", "--from", "homebrew", "list"],
        vec!["--json", "--yes", "--from", "apt", "install", "fixture"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pkd"))
            .env("SNAP", "/synthetic")
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.to_string().contains("disabled"));
    }
}

#[test]
fn terminal_confirmation_defaults_to_no_and_accepts_explicit_approval() {
    let fixture = Fixture::new();
    let declined = fixture.terminal_install("\n", false);
    let output = String::from_utf8_lossy(&declined.stdout);
    assert_eq!(declined.status.code(), Some(7), "{output}");
    assert!(output.contains("Install fixture"));
    assert!(output.contains("Source: homebrew"));
    assert!(output.contains("[y/N]"));
    assert!(output.contains("confirmation_declined"));
    assert!(output.contains("\x1b[1;34mPkgDeck"));
    assert!(!fixture.0.join("state.json").exists());

    if !rustix_root() {
        let approved = fixture.terminal_install("yes\n", true);
        let output = String::from_utf8_lossy(&approved.stdout);
        assert!(approved.status.success(), "{output}");
        assert!(output.contains("[OK] Completed"));
        assert!(!output.contains('\x1b'));
        let state: Value =
            serde_json::from_slice(&fs::read(fixture.0.join("state.json")).unwrap()).unwrap();
        assert_eq!(state["installed"], "1.0");
    }
}

#[test]
fn native_search_uses_installed_inventory_for_package_actions() {
    let fixture = Fixture::new();
    let executable = fixture.0.join("dnf");
    fs::write(&executable, r#"#!/bin/sh
case " $* " in
  *" --installed "*)
    if [ -f "$HOME/installed" ]; then
      printf '%s\n' 'synthetic-player|x86_64|1.0|Synthetic installed player'
    fi
    ;;
  *)
    printf '%s\n' 'synthetic-player|x86_64|2.0|Synthetic player' 'synthetic-player-plugin|x86_64|1.0|Synthetic plugin'
    ;;
esac
"#).unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    let search = || {
        let output = Command::new(env!("CARGO_BIN_EXE_pkd"))
            .env("PATH", &fixture.0)
            .env("HOME", &fixture.0)
            .env_remove("SNAP")
            .env_remove("FLATPAK_ID")
            .args(["--json", "--from", "dnf", "search", "synthetic-player"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"]["packages"].clone()
    };
    let offers = search();
    assert_eq!(offers.as_array().unwrap().len(), 2);
    assert!(offers
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["installed_version"].is_null()));
    fs::write(fixture.0.join("installed"), "").unwrap();
    let packages = search();
    assert_eq!(packages[0]["id"]["name"], "synthetic-player");
    assert_eq!(packages[0]["installed_version"], "1.0");
    assert_eq!(packages[0]["candidate_version"], "2.0");
    assert!(packages[1]["installed_version"].is_null());
}
