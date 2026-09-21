use pkgdeck_core::{
    backends::{Firmware, Transport},
    engine::{Backend, Engine},
    host::AptAction,
    package::*,
    process::*,
    repositories::{self, Action, Change},
};
use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
};
const DEVICE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
type Call = (String, Vec<String>, bool, bool);
#[derive(Clone)]
struct Fixture {
    calls: Arc<Mutex<Vec<Call>>>,
    devices: String,
    updates: String,
    remotes: String,
    fail: bool,
    no_updates: bool,
    deferred: bool,
}
fn completion(text: &str) -> Completion {
    Completion {
        code: Some(0),
        signal: None,
        stdout: text.as_bytes().to_vec(),
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    }
}
impl Default for Fixture {
    fn default() -> Self {
        Self { calls: Default::default(), devices: format!(r#"{{"Devices":[{{"DeviceId":"{DEVICE}","Name":"Synthetic BIOS","Version":"1","Flags":["require-ac","needs-reboot","needs-shutdown"]}}]}}"#), updates: format!(r#"{{"Devices":[{{"DeviceId":"{DEVICE}","Releases":[{{"Version":"2","Description":"Synthetic release"}}]}}]}}"#), remotes: r#"{"Remotes":[{"Id":"lvfs","Title":"Firmware service","Enabled":true,"MetadataUri":"https://example.invalid/metadata.xml.gz"}]}Trailing status"#.into(), fail: false, no_updates: false, deferred: false }
    }
}
impl Transport for Fixture {
    fn apt_query(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn brew(
        &self,
        _: &[OsString],
        _: &Cancellation,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn repository_editor(&self) -> Result<(), ExecutionError> {
        self.calls
            .lock()
            .unwrap()
            .push(("editor".into(), vec![], false, true));
        Ok(())
    }
    fn flatpak(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, ExecutionError> {
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        let args = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.calls
            .lock()
            .unwrap()
            .push(("flatpak".into(), args, write, system));
        if self.fail && system {
            return Err(ExecutionError::Io("synthetic read failure".into()));
        }
        Ok(completion(if write {
            ""
        } else {
            if system {
                "flathub\tFlathub\thttps://example.invalid/repo\t1\n"
            } else {
                "flathub\tFlathub\thttps://example.invalid/repo\t1\tdisabled\n"
            }
        }))
    }
    fn system_manager(
        &self,
        name: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        let args = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.calls
            .lock()
            .unwrap()
            .push((name.into(), args.clone(), write, true));
        if self.fail {
            return Err(ExecutionError::Io("synthetic daemon failure".into()));
        }
        if self.no_updates && args.iter().any(|s| s == "get-updates" || s == "refresh") {
            let mut result = completion("");
            result.code = Some(2);
            return Err(ExecutionError::Failed(result));
        }
        let text = if args.iter().any(|s| s == "get-devices") {
            &self.devices
        } else if args.iter().any(|s| s == "get-updates") {
            &self.updates
        } else if args.iter().any(|s| s == "get-remotes") {
            &self.remotes
        } else {
            "synthetic success"
        };
        let mut result = completion(text);
        result.cancellation_deferred = write && self.deferred;
        Ok(result)
    }
}
#[test]
fn firmware_shows_real_update_versions_and_requirements() {
    let fixture = Fixture::default();
    let mut firmware = Firmware::new(fixture);
    let cancel = Cancellation::default();
    assert_eq!(firmware.detect(&cancel).unwrap(), Availability::Available);
    let packages = firmware.installed(&cancel).unwrap();
    let package = &packages[0];
    assert_eq!(package.display_name, "Synthetic BIOS");
    assert_eq!(package.installed_version.as_deref(), Some("1"));
    assert_eq!(package.candidate_version.as_deref(), Some("2"));
    assert_eq!(package.update, UpdateAvailability::Available);
    for requirement in ["AC power", "Restart", "Shutdown"] {
        assert!(package.summary.contains(requirement));
    }
    assert!(firmware
        .details(&package.id, &cancel)
        .unwrap()
        .description
        .contains("Synthetic release"));
    assert!(firmware.search("BIOS", &cancel).unwrap().is_empty());
    let mut foreign = package.id.clone();
    foreign.scope = Scope::User { uid: 0 };
    assert!(firmware.details(&foreign, &cancel).is_err());
}
#[test]
fn firmware_updates_target_devices_without_rebooting_or_bypassing_checks() {
    let fixture = Fixture {
        deferred: true,
        ..Default::default()
    };
    let mut firmware = Firmware::new(fixture.clone());
    let cancel = Cancellation::default();
    let id = firmware.installed(&cancel).unwrap()[0].id.clone();
    let mut messages = vec![];
    assert!(
        firmware
            .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| messages
                .push(p))
            .unwrap()
            .cancellation_deferred
    );
    assert!(!messages.is_empty());
    assert!(firmware
        .execute(&Operation::Remove(id), &cancel, &mut |_| {})
        .is_err());
    firmware
        .execute(
            &Operation::Refresh {
                backend: "fwupd".into(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    let (_, args, _, _) = calls
        .iter()
        .find(|(_, args, write, _)| *write && args.contains(&"update".into()))
        .unwrap();
    assert_eq!(args.last().unwrap(), DEVICE);
    assert!(args.contains(&"--no-reboot-check".into()));
    assert!(args.contains(&"--no-unreported-check".into()));
    assert!(!args.iter().any(|a| matches!(
        a.as_str(),
        "--force" | "--allow-older" | "--allow-reinstall"
    )));
}
#[test]
fn firmware_participates_in_engine_update_all_and_propagates_failure() {
    let fixture = Fixture::default();
    let mut engine = Engine::default();
    engine.register(Firmware::new(fixture.clone())).unwrap();
    let cancel = Cancellation::default();
    let report = engine.installed(&cancel);
    assert!(report.failures.is_empty());
    assert_eq!(report.packages.len(), 1);
    let result = engine.execute_batch(
        &[Operation::UpgradeAll {
            backend: "fwupd".into(),
        }],
        &cancel,
        &mut |_| {},
    );
    assert!(result[0].is_ok());
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(_, args, write, _)| *write && args.last().is_some_and(|s| s == DEVICE)));
    let mut failed = Firmware::new(Fixture {
        fail: true,
        ..Default::default()
    });
    assert!(failed.installed(&cancel).is_err());
    assert!(failed
        .execute(
            &Operation::UpgradeAll {
                backend: "fwupd".into()
            },
            &cancel,
            &mut |_| {}
        )
        .is_err());
    cancel.cancel();
    assert!(engine.installed(&cancel).failures.len() == 1);
}
#[test]
fn firmware_no_updates_and_invalid_metadata_are_distinct() {
    let cancel = Cancellation::default();
    for fixture in [
        Fixture {
            no_updates: true,
            ..Default::default()
        },
        Fixture {
            updates: r#"{"Devices":[]}"#.into(),
            ..Default::default()
        },
    ] {
        let mut firmware = Firmware::new(fixture);
        let package = firmware.installed(&cancel).unwrap().remove(0);
        assert_eq!(package.update, UpdateAvailability::Current);
        firmware
            .execute(
                &Operation::Refresh {
                    backend: "fwupd".into(),
                },
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        assert!(firmware
            .execute(&Operation::Upgrade(package.id), &cancel, &mut |_| {})
            .is_err());
    }
    for devices in [
        "",
        "bad",
        "{}",
        r#"{"Error":{"Message":"daemon unavailable"}}"#,
        r#"{"Devices":[{"DeviceId":"--malicious"}]}"#,
    ] {
        assert!(Firmware::new(Fixture {
            devices: devices.into(),
            ..Default::default()
        })
        .installed(&cancel)
        .is_err());
    }
    let devices = format!(r#"{{"Devices":[{{"DeviceId":"{DEVICE}"}}]}}Status"#);
    assert!(Firmware::new(Fixture {
        devices,
        ..Default::default()
    })
    .installed(&cancel)
    .is_ok());
}
fn user() -> Scope {
    Scope::User {
        uid: rustix::process::getuid().as_raw(),
    }
}
fn action(change: Change, scope: Scope) -> Action {
    Action {
        backend: "flatpak".into(),
        name: "fixture".into(),
        scope,
        change,
    }
}
#[test]
fn repositories_keep_scopes_and_apt_enabled_states() {
    let root = std::env::temp_dir().join(format!("pkgdeck-repo-fixture-{}", std::process::id()));
    std::fs::create_dir_all(root.join("etc/apt/sources.list.d")).unwrap();
    std::fs::write(
        root.join("etc/apt/sources.list"),
        "# Native source comment\ndeb https://example.invalid stable main\n# deb https://disabled.invalid stable main\n",
    )
    .unwrap();
    std::fs::write(
        root.join("etc/apt/sources.list.d/test.sources"),
        "# Comment-only stanza\n\nTypes: deb\nURIs: https://example.invalid/deb822\nSuites: stable\nEnabled: no\n",
    )
    .unwrap();
    std::fs::write(
        root.join("etc/apt/sources.list.d/ignored.bak"),
        "not an active source",
    )
    .unwrap();
    let report = repositories::list(&Fixture::default(), &root, &Cancellation::default());
    assert!(report.errors.is_empty());
    assert_eq!(report.repositories.len(), 6);
    assert_ne!(report.repositories[0].scope, report.repositories[1].scope);
    assert!(!report.repositories[0].enabled);
    assert!(report.repositories[1].enabled);
    let firmware = report
        .repositories
        .iter()
        .find(|r| r.backend == "fwupd")
        .unwrap();
    assert_eq!(firmware.name, "lvfs");
    assert!(firmware.enabled);
    assert!(firmware.url.starts_with("https://"));
    assert_eq!(
        report
            .repositories
            .iter()
            .filter(|r| r.backend == "apt" && !r.enabled)
            .count(),
        2
    );
    let failed = repositories::list(
        &Fixture {
            fail: true,
            ..Default::default()
        },
        &root,
        &Cancellation::default(),
    );
    assert_eq!(failed.errors.len(), 2);
    assert!(failed.repositories.iter().any(|r| r.scope == user()));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn repository_actions_preserve_scope_and_native_signature_checks() {
    let fixture = Fixture::default();
    let cancel = Cancellation::default();
    for scope in [user(), Scope::System] {
        for change in [
            Change::Add {
                url: "https://example.invalid/repo.flatpakrepo".into(),
            },
            Change::SetEnabled { enabled: true },
            Change::SetEnabled { enabled: false },
            Change::SetPriority { priority: 4 },
            Change::Remove,
        ] {
            let request = action(change, scope.clone());
            assert!(!request.label().is_empty());
            repositories::apply(&fixture, &request, &cancel).unwrap();
        }
    }
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 10);
    for (_, args, write, system) in calls.iter() {
        assert!(*write);
        assert_eq!(args[0], if *system { "--system" } else { "--user" });
        assert!(!args
            .iter()
            .any(|arg| arg == "--no-gpg-verify" || arg == "--force"));
    }
    drop(calls);
    repositories::apply(
        &fixture,
        &Action {
            backend: "fwupd".into(),
            name: "lvfs".into(),
            scope: Scope::System,
            change: Change::SetEnabled { enabled: false },
        },
        &cancel,
    )
    .unwrap();
    repositories::apply(
        &fixture,
        &Action {
            backend: "apt".into(),
            name: "sources".into(),
            scope: Scope::System,
            change: Change::OpenEditor,
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(fixture.calls.lock().unwrap().last().unwrap().0, "editor");
}
#[test]
fn invalid_repository_changes_never_execute() {
    let fixture = Fixture::default();
    let cancel = Cancellation::default();
    for change in [
        Change::Add {
            url: "http://example.invalid/repo.flatpakrepo".into(),
        },
        Change::Add {
            url: "https://example.invalid/unsigned".into(),
        },
        Change::SetPriority { priority: -1 },
        Change::SetPriority { priority: 10000 },
        Change::OpenEditor,
    ] {
        assert!(repositories::apply(&fixture, &action(change, user()), &cancel).is_err());
    }
    let mut invalid = action(Change::Remove, user());
    invalid.name = "--all".into();
    assert!(invalid.validate().is_err());
    invalid.name = "repo".into();
    invalid.scope = Scope::Environment {
        path: "/fixture".into(),
    };
    assert!(invalid.validate().is_err());
    invalid.scope = Scope::User { uid: u32::MAX };
    assert!(invalid.validate().is_err());
    invalid.scope = Scope::System;
    invalid.backend = "unknown".into();
    assert!(invalid.validate().is_err());
    let parsed = repositories::parse_action(
        r#"{"backend":"flatpak","name":"fixture","scope":"user","action":"remove"}"#,
    )
    .unwrap();
    assert_eq!(parsed.scope, user());
    assert!(repositories::parse_action("invalid").is_err());
    cancel.cancel();
    assert!(repositories::apply(&fixture, &parsed, &cancel).is_err());
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[test]
fn repository_errors_preserve_other_sources_and_respect_source_filter() {
    let root = std::env::temp_dir().join(format!("pkgdeck-repo-errors-{}", std::process::id()));
    std::fs::create_dir_all(root.join("etc/apt")).unwrap();
    std::fs::write(root.join("etc/apt/sources.list"), [0xff]).unwrap();
    std::fs::write(root.join("etc/apt/sources.list.d"), "not a directory").unwrap();
    let fixture = Fixture {
        remotes: r#"{"Remotes":[{"Id":"--invalid","Enabled":true}]}"#.into(),
        ..Default::default()
    };
    let report = repositories::list(&fixture, &root, &Cancellation::default());
    assert_eq!(report.repositories.len(), 2);
    assert_eq!(report.errors.len(), 3);
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("invalid firmware remote ID")));
    let report = repositories::list_selected(
        &fixture,
        &root,
        &Cancellation::default(),
        &["flatpak".into()],
        Some(&Scope::System),
    );
    assert!(report.errors.is_empty());
    assert_eq!(report.repositories.len(), 1);
    assert_eq!(report.repositories[0].scope, Scope::System);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_firmware_repository_metadata_is_reported_instead_of_empty_success() {
    for remotes in ["", "broken", "{}", r#"{"Remotes":[{"Id":"lvfs"}]}"#] {
        let fixture = Fixture {
            remotes: remotes.into(),
            ..Default::default()
        };
        let report = repositories::list_selected(
            &fixture,
            std::path::Path::new("/unused-synthetic-root"),
            &Cancellation::default(),
            &["fwupd".into()],
            None,
        );
        assert!(report.repositories.is_empty());
        assert_eq!(report.errors.len(), 1);
    }
}
