use std::process::{Command, Stdio};

fn pkd() -> Command {
    // Activity history and caches go to a scratch folder, never the real one.
    let state = std::env::temp_dir().join(format!("pkgdeck-entrypoint-{}", std::process::id()));
    let mut command = Command::new(env!("CARGO_BIN_EXE_pkd"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CACHE_HOME", &state);
    command
}

#[test]
fn inventory_only_source_is_rejected_before_noninteractive_confirmation() {
    // Removal is up to the source, so it needs approval like any change.
    let output = pkd()
        .args([
            "--json",
            "--from",
            "macos-apps",
            "remove",
            "/Applications/Fixture.app",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["data"]["error"], "confirmation_required");
    for verb in ["install", "upgrade"] {
        for yes in [false, true] {
            let mut command = pkd();
            command.args([
                "--json",
                "--from",
                "macos-apps",
                verb,
                "/Applications/Fixture.app",
            ]);
            if yes {
                command.arg("--yes");
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                data["data"]["error"]["Unsupported"]["backend"],
                "macos-apps"
            );
            assert!(data["data"].get("operations").is_none());
        }
    }
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
fn doctor_is_noninteractive_and_reports_packaged_capabilities() {
    let output = pkd()
        .arg("doctor")
        .env("SNAP", "/synthetic-snap")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Runtime       snap"), "{text}");
    assert!(text.contains(" APT "), "{text}");
    let output = pkd()
        .arg("doctor")
        .env_remove("SNAP")
        .env_remove("FLATPAK_ID")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(" APT "), "{text}");
    assert!(text.contains(" Homebrew "), "{text}");
}

#[test]
fn flatpak_doctor_requires_a_working_host_bridge() {
    let output = pkd()
        .arg("doctor")
        .env_remove("SNAP")
        .env("FLATPAK_ID", "synthetic.fixture")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/pkgdeck-synthetic-missing-bus",
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Runtime       flatpak"), "{text}");
    assert!(!text.contains(" APT "));
}

#[test]
fn repository_commands_respect_packaged_host_boundary() {
    let output = pkd()
        .args(["--json", "repos"])
        .env("FLATPAK_ID", "synthetic.fixture")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/pkgdeck-synthetic-missing-bus",
        )
        .env_remove("SNAP")
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let repositories = value["data"]["repositories"].as_array().unwrap();
    // Even without command access, readable host source files may be listed;
    // sandbox /etc sources must never be substituted for the host namespace.
    for repository in repositories {
        assert_eq!(repository["backend"], "apt");
        assert!(repository["name"]
            .as_str()
            .unwrap()
            .starts_with("/run/host/etc/apt/"));
    }
}
