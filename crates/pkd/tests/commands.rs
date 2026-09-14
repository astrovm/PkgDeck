use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pkgdeck-cli-{}", std::process::id()));
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
