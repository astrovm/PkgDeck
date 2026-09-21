use std::process::{Command, Stdio};

fn pkd() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pkd"));
    command.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
    command
}

#[test]
fn version_and_help_work_without_a_display() {
    let version = pkd().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("pkd {}", pkgdeck_core::VERSION)
    );
    let help = pkd().arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout).unwrap().contains("Usage:"));
}

#[test]
fn no_arguments_show_help_and_unsupported_commands_fail_clearly() {
    let result = pkd().stdin(Stdio::null()).output().unwrap();
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    assert!(String::from_utf8(result.stdout).unwrap().contains("Usage:"));
    let result = pkd()
        .arg("install")
        .arg("synthetic-fixture")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
}

#[test]
fn doctor_is_noninteractive_and_reports_sandbox_capabilities() {
    let output = pkd()
        .arg("doctor")
        .env("SNAP", "/synthetic-snap")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Runtime: Snap"));
    assert!(text.contains("disabled"));
    let output = pkd()
        .arg("doctor")
        .env_remove("SNAP")
        .env_remove("FLATPAK_ID")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("APT:"));
    assert!(text.contains("Homebrew:"));
}

#[test]
fn flatpak_doctor_does_not_enumerate_runtime_backends() {
    let output = pkd()
        .arg("doctor")
        .env_remove("SNAP")
        .env("FLATPAK_ID", "synthetic.fixture")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Flatpak host execution is disabled"));
    assert!(!text.contains("APT:"));
}

#[test]
fn repository_commands_respect_packaged_host_boundary() {
    let output = pkd()
        .args(["--json", "repos"])
        .env("FLATPAK_ID", "synthetic.fixture")
        .env_remove("SNAP")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("disabled"));
}
