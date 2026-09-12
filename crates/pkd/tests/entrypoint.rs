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
fn non_interactive_and_unsupported_commands_fail_clearly() {
    let result = pkd().stdin(Stdio::null()).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("requires a terminal"));
    let result = pkd()
        .arg("install")
        .arg("synthetic-fixture")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
}

#[test]
fn tui_renders_resizes_and_restores_terminal_on_exit() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/smoke.py"
        ))
        .args(["terminal", env!("CARGO_BIN_EXE_pkd")])
        .status()
        .unwrap();
    assert!(status.success());
}
