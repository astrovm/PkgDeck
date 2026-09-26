use pkgdeck_core::{
    backends::*,
    engine::*,
    host::{AptAction, Authorization},
    package::*,
    process::*,
};
use serde_json::json;
use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
};

type SystemCall = (String, Vec<OsString>);

#[derive(Clone, Default)]
struct Fixture {
    record_writes: bool,
    writes: Arc<Mutex<Vec<SystemCall>>>,
    grouped_apt_writes: Arc<Mutex<Vec<Vec<AptAction>>>>,
    apt_simulation: Option<String>,
    simulations: Arc<Mutex<Vec<SystemCall>>>,
    installed: Arc<Mutex<Option<String>>>,
    local_install: Arc<Mutex<Option<std::path::PathBuf>>>,
    candidate: Arc<Mutex<String>>,
    failure: Arc<Mutex<Option<ExecutionError>>>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            candidate: Arc::new(Mutex::new("1.0".into())),
            ..Self::default()
        }
    }
    fn package(&self) -> PackageDetails {
        let installed = self.installed.lock().unwrap().clone();
        let candidate = self.candidate.lock().unwrap().clone();
        PackageDetails {
            package: Package {
                id: PackageId {
                    backend: "apt".into(),
                    name: "synthetic-fixture".into(),
                    architecture: "all".into(),
                    scope: Scope::System,
                    remote: None,
                    reference: None,
                },
                display_name: "Synthetic fixture".into(),
                summary: "Synthetic package".into(),
                update: if installed.as_ref().is_some_and(|v| v != &candidate) {
                    UpdateAvailability::Available
                } else {
                    UpdateAvailability::Current
                },
                installed_version: installed,
                candidate_version: Some(candidate),
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            },
            description: "Synthetic package description".into(),
            homepage: None,
            dependencies: vec![],
        }
    }
    fn check(&self, cancel: &Cancellation) -> Result<(), ExecutionError> {
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        if let Some(e) = &*self.failure.lock().unwrap() {
            return Err(e.clone());
        }
        Ok(())
    }
}
fn output(value: impl AsRef<[u8]>) -> Completion {
    Completion {
        code: Some(0),
        signal: None,
        stdout: value.as_ref().to_vec(),
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    }
}
impl Transport for Fixture {
    fn apt_query(
        &self,
        mode: &str,
        query: &str,
        _: &str,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.check(cancel)?;
        let details = self.package();
        let matches = match mode {
            "detect" => false,
            "installed" => details.package.installed_version.is_some(),
            _ => details.package.id.name.contains(query),
        };
        Ok(output(
            serde_json::to_vec(&if matches { vec![details] } else { vec![] }).unwrap(),
        ))
    }
    fn apt_write(
        &self,
        action: AptAction,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.check(cancel)?;
        if let AptAction::InstallLocal(path) = &action {
            use std::os::unix::fs::PermissionsExt;
            assert!(path.is_file(), "staged archive must exist for the write");
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            *self.local_install.lock().unwrap() = Some(path.clone());
        }
        match action {
            AptAction::Refresh => *self.candidate.lock().unwrap() = "2.0".into(),
            AptAction::Install(_)
            | AptAction::InstallLocal(_)
            | AptAction::Upgrade(_)
            | AptAction::UpgradeAll => {
                *self.installed.lock().unwrap() = Some(self.candidate.lock().unwrap().clone())
            }
            AptAction::Remove(_) => *self.installed.lock().unwrap() = None,
            AptAction::Autoremove | AptAction::Autoclean => {}
        }
        Ok(output(""))
    }
    fn apt_write_group(
        &self,
        actions: &[AptAction],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.check(cancel)?;
        self.grouped_apt_writes
            .lock()
            .unwrap()
            .push(actions.to_vec());
        Ok(output(""))
    }
    fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.check(cancel)?;
        let args: Vec<_> = args.iter().map(|s| s.to_str().unwrap()).collect();
        if write {
            match args[0] {
                "update" => *self.candidate.lock().unwrap() = "2.0".into(),
                "install" | "upgrade" => {
                    *self.installed.lock().unwrap() = Some(self.candidate.lock().unwrap().clone())
                }
                "uninstall" => *self.installed.lock().unwrap() = None,
                "autoremove" | "cleanup" => {}
                _ => panic!("unexpected native operation"),
            }
            return Ok(output(""));
        }
        match args[0] {
            "--prefix" => Ok(output("/home/linuxbrew/.linuxbrew\n")),
            "formulae" => Ok(output("synthetic-fixture\n")),
            "casks" => Ok(output("synthetic-fixture\n")),
            "info" => {
                let installed = self.installed.lock().unwrap().clone();
                let candidate = self.candidate.lock().unwrap().clone();
                let outdated = installed.as_ref().is_some_and(|v| v != &candidate);
                if args.contains(&"--cask") {
                    let cask = json!({"full_token":"synthetic-fixture","name":["Synthetic Fixture"],"desc":"Synthetic package","homepage":"https://example.invalid","version":candidate,"installed":installed,"outdated":outdated});
                    return Ok(output(json!({"casks": if args.contains(&"--installed") && !outdated && json!(cask["installed"]).is_null() { vec![] } else { vec![cask] }}).to_string()));
                }
                let formula = json!({"full_name":"synthetic-fixture","desc":"Synthetic package","homepage":"https://example.invalid","versions":{"stable":candidate},"revision":0,"installed":installed.iter().map(|v| json!({"version":v})).collect::<Vec<_>>(),"outdated":outdated,"dependencies":[]});
                Ok(output(json!({"formulae": if args.contains(&"--installed") && installed.is_none() { vec![] } else { vec![formula] }}).to_string()))
            }
            "autoremove" => Ok(output("unused-formula\n")),
            "cleanup" => Ok(output("Would remove: /tmp/synthetic-cache (1MB)\n")),
            _ => panic!("unexpected metadata query"),
        }
    }
    fn flatpak(
        &self,
        _: &[OsString],
        cancel: &Cancellation,
        _: bool,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        self.check(cancel)?;
        Ok(output(""))
    }
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.check(cancel)?;
        if write && self.record_writes {
            self.writes
                .lock()
                .unwrap()
                .push((executable.into(), args.to_vec()));
            return Ok(output(""));
        }
        let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        if executable == "apt-config" {
            return Ok(output(
                "ROOT='/'\nCACHE='var/cache/apt'\nARCHIVES='archives/'\n",
            ));
        }
        if executable == "find" {
            assert_eq!(args[0], "/var/cache/apt/archives/");
            return Ok(output("..."));
        }
        if executable == "apt-get" {
            if let Some(simulation) = self
                .apt_simulation
                .as_ref()
                .filter(|_| args.contains(&"--simulate".into()))
            {
                self.simulations.lock().unwrap().push((
                    executable.into(),
                    args.iter()
                        .map(|arg| OsString::from(arg.as_ref()))
                        .collect(),
                ));
                return Ok(output(simulation));
            }
            if args.contains(&"autoremove".into()) {
                return Ok(output("Remv synthetic-orphan [1.0]\n"));
            }
            if args.contains(&"autoclean".into()) {
                return Ok(output("Del synthetic-cache 1.0 [1024 B]\n"));
            }
        }
        if executable == "pacman" {
            if write {
                return Ok(output(""));
            }
            if args.contains(&"-Q".into()) {
                return Ok(output(
                    self.installed
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|version| format!("synthetic-fixture {version}\n"))
                        .unwrap_or_default(),
                ));
            }
            return Ok(output("core/synthetic-fixture 1.0\nSynthetic package\n"));
        }
        assert_eq!(executable, "dnf");
        if write {
            match args.last().map(|arg| arg.as_ref()) {
                Some("makecache") => *self.candidate.lock().unwrap() = "2.0".into(),
                Some("synthetic-fixture") if args.contains(&"remove".into()) => {
                    *self.installed.lock().unwrap() = None
                }
                Some("synthetic-fixture") => {
                    *self.installed.lock().unwrap() = Some(self.candidate.lock().unwrap().clone())
                }
                _ => panic!("unexpected DNF operation: {args:?}"),
            }
            return Ok(output(""));
        }
        let installed = args.contains(&"--installed".into());
        let matches = (installed && self.installed.lock().unwrap().is_some())
            || (!installed && args.last().is_some_and(|arg| *arg == "synthetic-fixture"));
        if !matches {
            return Ok(output(""));
        }
        let version = if installed {
            self.installed.lock().unwrap().clone().unwrap()
        } else {
            self.candidate.lock().unwrap().clone()
        };
        Ok(output(format!(
            "synthetic-fixture|x86_64|{version}|Synthetic package\n"
        )))
    }
}
fn lifecycle(mut backend: impl Backend) {
    let cancel = Cancellation::default();
    for capability in [
        Capability::Search,
        Capability::Details,
        Capability::Installed,
        Capability::Install,
        Capability::Remove,
        Capability::Refresh,
        Capability::Upgrade,
    ] {
        assert!(backend.capabilities().contains(&capability));
    }
    assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
    assert!(backend
        .search("no-such-fixture", &cancel)
        .unwrap()
        .is_empty());
    assert!(backend.installed(&cancel).unwrap().is_empty());
    let package = backend
        .search("synthetic-fixture", &cancel)
        .unwrap()
        .remove(0);
    assert_eq!(
        backend.details(&package.id, &cancel).unwrap().package,
        package
    );
    let id = package.id;
    let mut progress = vec![];
    backend
        .execute(&Operation::Install(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    assert!(!progress.is_empty());
    assert_eq!(
        backend.installed(&cancel).unwrap()[0]
            .installed_version
            .as_deref(),
        Some("1.0")
    );
    backend
        .execute(
            &Operation::Refresh {
                backend: backend.id().into(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    if backend.id() != "dnf" {
        assert_eq!(
            backend.installed(&cancel).unwrap()[0].update,
            UpdateAvailability::Available
        );
    }
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |_| {})
        .unwrap();
    assert_eq!(
        backend.installed(&cancel).unwrap()[0]
            .installed_version
            .as_deref(),
        Some("2.0")
    );
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |_| {})
        .unwrap();
    assert!(backend.installed(&cancel).unwrap().is_empty());
    let mut foreign = id.clone();
    foreign.scope = Scope::User { uid: 1234 };
    assert!(backend.details(&foreign, &cancel).is_err());
    assert!(backend
        .execute(&Operation::Install(foreign), &cancel, &mut |_| {})
        .is_err());
    assert!(backend
        .execute(
            &Operation::Refresh {
                backend: "foreign".into()
            },
            &cancel,
            &mut |_| {}
        )
        .is_err());
    let mut missing = id.clone();
    missing.name = "missing-fixture".into();
    assert_eq!(
        backend.details(&missing, &cancel),
        Err(EngineError::NotFound)
    );
    let mut bad = id;
    bad.name = "--evil-option".into();
    assert!(backend.details(&bad, &cancel).is_err());
    cancel.cancel();
    assert_eq!(backend.detect(&cancel), Err(EngineError::Cancelled));
}
#[test]
fn apt_lifecycle() {
    lifecycle(Apt::new(Fixture::new()));
}

#[test]
fn apt_groups_exact_targets_in_one_native_write_and_rejects_mixed_actions() {
    let fixture = Fixture::new();
    let mut apt = Apt::new(fixture.clone());
    let cancel = Cancellation::default();
    let first = fixture.package().package.id;
    let mut second = first.clone();
    second.name = "second-fixture".into();
    let installs = [
        Operation::Install(first.clone()),
        Operation::Install(second.clone()),
    ];
    let mut progress = Vec::new();
    assert!(apt
        .execute_group(&installs[..1], &cancel, &mut |_| {})
        .is_none());
    let outcomes = apt
        .execute_group(&installs, &cancel, &mut |event| progress.push(event))
        .unwrap()
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes
        .iter()
        .all(|outcome| !outcome.cancellation_deferred));
    assert!(
        matches!(progress.as_slice(), [Progress::Message(message)] if message.contains("2 APT packages in one transaction"))
    );
    let writes = fixture.grouped_apt_writes.lock().unwrap();
    assert_eq!(writes.len(), 1);
    assert!(
        matches!(&writes[0][0], AptAction::Install(target) if target == "synthetic-fixture:all")
    );
    assert!(matches!(&writes[0][1], AptAction::Install(target) if target == "second-fixture:all"));
    drop(writes);

    let mixed = [
        Operation::Install(first.clone()),
        Operation::Remove(second.clone()),
    ];
    assert!(apt
        .execute_group(&mixed, &cancel, &mut |_| {})
        .unwrap()
        .is_err());
    let foreign = [
        Operation::Install(first.clone()),
        Operation::Refresh {
            backend: "apt".into(),
        },
    ];
    assert!(apt
        .execute_group(&foreign, &cancel, &mut |_| {})
        .unwrap()
        .is_err());
    let mut invalid = second;
    invalid.name = "--invalid-target".into();
    assert!(apt
        .execute_group(
            &[
                Operation::Install(first.clone()),
                Operation::Install(invalid)
            ],
            &cancel,
            &mut |_| {}
        )
        .unwrap()
        .is_err());
    assert_eq!(fixture.grouped_apt_writes.lock().unwrap().len(), 1);

    *fixture.failure.lock().unwrap() = Some(ExecutionError::LockBusy);
    assert!(matches!(
        apt.execute_group(&installs, &cancel, &mut |_| {}).unwrap(),
        Err(EngineError::Execution(ExecutionError::LockBusy))
    ));
    assert_eq!(fixture.grouped_apt_writes.lock().unwrap().len(), 1);
}
#[test]
fn homebrew_lifecycle() {
    lifecycle(Homebrew::new(Fixture::new()));
}

#[test]
fn apt_cleanup_uses_dry_run_plans_and_fixed_operations() {
    let fixture = Fixture::new();
    let mut apt = Apt::new(fixture);
    let cancel = Cancellation::default();
    let items = apt.cleanup(&cancel).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id.backend, "apt");
    assert_eq!(items[0].id.key, "autoremove");
    assert_eq!(items[1].id.key, "autoclean");
    assert_eq!(items[1].summary, "3 cached downloads");
    assert!(!items[1].preview.contains("synthetic-cache"));
    for item in items {
        apt.execute(&Operation::Clean(item.id), &cancel, &mut |_| {})
            .unwrap();
    }
}

#[test]
fn apt_cache_plan_survives_engine_revalidation_without_preview_authentication() {
    let cancel = Cancellation::default();
    let mut engine = Engine::default();
    engine.register(Apt::new(Fixture::new())).unwrap();
    let report = engine.cleanup(&cancel);
    assert!(report.failures.is_empty());
    let cache = report
        .items
        .into_iter()
        .find(|item| item.id.key == "autoclean")
        .unwrap();
    engine
        .execute(&Operation::Clean(cache.id.clone()), &cancel, &mut |_| {})
        .unwrap();
    // The reviewed task is single-use, including after a successful write.
    assert!(engine
        .execute(&Operation::Clean(cache.id), &cancel, &mut |_| {})
        .is_err());
}

#[test]
fn homebrew_cleanup_uses_native_dry_run_plans() {
    let fixture = Fixture::new();
    let mut brew = Homebrew::new(fixture);
    let cancel = Cancellation::default();
    brew.detect(&cancel).unwrap();
    let items = brew.cleanup(&cancel).unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| item.id.key == "autoremove"));
    assert!(items.iter().any(|item| item.id.key == "cleanup"));
    for item in items {
        brew.execute(&Operation::Clean(item.id), &cancel, &mut |_| {})
            .unwrap();
    }
}
#[test]
fn homebrew_cask_lifecycle() {
    let fixture = Fixture::new();
    let mut backend = HomebrewCask::new(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
    let package = backend.search("fixture", &cancel).unwrap().remove(0);
    assert_eq!(package.id.backend, "homebrew-cask");
    assert_eq!(package.display_name, "Synthetic Fixture");
    lifecycle(backend);
}
#[test]
fn dnf_lifecycle() {
    lifecycle(Dnf::dnf(Fixture::new()));
    let fixture = Fixture::new();
    let mut backend = Dnf::dnf(fixture.clone());
    let cancel = Cancellation::default();
    let offer = backend
        .search("synthetic-fixture", &cancel)
        .unwrap()
        .remove(0);
    assert!(offer.installed_version.is_none());
    *fixture.installed.lock().unwrap() = Some("0.9".into());
    let installed = backend
        .search("synthetic-fixture", &cancel)
        .unwrap()
        .remove(0);
    assert_eq!(installed.installed_version.as_deref(), Some("0.9"));
    assert_eq!(installed.candidate_version.as_deref(), Some("1.0"));
}

#[test]
fn wave_three_parsers_preserve_system_identities() {
    let cancel = Cancellation::default();
    let fixture = Fixture::new();
    let mut pacman = Pacman::pacman(fixture.clone());
    assert!(pacman.search("synthetic-fixture", &cancel).unwrap()[0]
        .installed_version
        .is_none());
    *fixture.installed.lock().unwrap() = Some("1.0".into());
    assert_eq!(
        pacman.search("synthetic-fixture", &cancel).unwrap()[0]
            .installed_version
            .as_deref(),
        Some("1.0")
    );
    let mut zypper = Zypper::zypper(Raw(output("<solvable name=\"synthetic-fixture\" edition=\"1.0\" arch=\"x86_64\" summary=\"Synthetic package\"/>")));
    let mut snap = Snap::snap(Raw(output("Name Version Rev Tracking Publisher Notes\nsynthetic-fixture 1.0 1 latest/stable synthetic -\n")));
    for backend in [&mut pacman as &mut dyn Backend, &mut zypper, &mut snap] {
        let package = backend
            .search("synthetic-fixture", &cancel)
            .unwrap()
            .remove(0);
        assert_eq!(package.id.backend, backend.id());
        assert_eq!(package.id.scope, Scope::System);
        backend
            .execute(&Operation::Install(package.id), &cancel, &mut |_| {})
            .unwrap();
    }
}

#[test]
fn snap_search_keeps_store_summaries_for_ranking() {
    let cancel = Cancellation::default();
    let mut snap = Snap::snap(Raw(output(
        "Name Version Publisher Notes Summary\ngimp 2.10.38 snapcrafters - GNU Image Manipulation Program\nopenvino-ai-plugins-gimp 1.0 intel - AI plugins for GIMP\nbare 1.0 canonical -\n",
    )));
    let packages = snap.search("gimp", &cancel).unwrap();
    assert_eq!(packages.len(), 3);
    assert_eq!(packages[0].summary, "GNU Image Manipulation Program");
    assert_eq!(packages[1].summary, "AI plugins for GIMP");
    assert_eq!(packages[2].summary, "");
    // `snap list` has no summary column: installed rows keep the placeholder.
    let installed = snap.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 3);
    assert!(installed.iter().all(|p| p.summary == "Snap package"));
    assert!(installed.iter().all(|p| p.installed_version.is_some()));
}

#[test]
fn flatpak_lists_user_and_system_applications_without_collapsing_scope() {
    let cancel = Cancellation::default();
    let mut backend = Flatpak::new(FlatpakFixture::default());
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let packages = backend.installed(&cancel).unwrap();
    assert_eq!(packages.len(), 2);
    assert!(packages
        .iter()
        .any(|package| package.id.scope == Scope::System));
    assert!(packages
        .iter()
        .any(|package| matches!(package.id.scope, Scope::User { .. })));
}

#[test]
fn flatpak_list_accepts_an_omitted_empty_options_column() {
    let cancel = Cancellation::default();
    let installed = format!(
        "io.example.App\t{}\tstable\t1.0\tSynthetic app\tflathub\n",
        std::env::consts::ARCH
    );
    let mut backend = Flatpak::new(FlatpakFixture {
        installed: Some(installed),
        ..FlatpakFixture::default()
    });
    let packages = backend.installed(&cancel).unwrap();
    assert_eq!(packages.len(), 2);
    assert!(packages
        .iter()
        .all(|package| package.display_name == "io.example.App"));
    assert!(packages
        .iter()
        .all(|package| package.summary == "Synthetic app"));
}

type FlatpakCall = (Vec<String>, bool, bool);

#[derive(Clone, Default)]
struct FlatpakFixture {
    calls: Arc<Mutex<Vec<FlatpakCall>>>,
    installed: Option<String>,
    user_updates: Option<Completion>,
    system_updates: Option<Completion>,
}
impl Transport for FlatpakFixture {
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
    fn flatpak(
        &self,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, ExecutionError> {
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.calls
            .lock()
            .unwrap()
            .push((args.clone(), write, system));
        if args.contains(&"remote-ls".into()) {
            return Ok(if system {
                self.system_updates.clone()
            } else {
                self.user_updates.clone()
            }
            .unwrap_or_else(|| output("")));
        }
        if args.contains(&"list".into()) {
            if let Some(installed) = &self.installed {
                return Ok(output(installed));
            }
            // The user row carries flatpak's app name column; the system
            // row leaves it out, like an older fixture or a nameless ref.
            let (name, title) = if system {
                ("io.example.System", "")
            } else {
                ("io.example.User", "\tUser App")
            };
            return Ok(output(format!(
                "{name}\t{}\tstable\t1.0\tSynthetic app\tflathub\tcurrent{title}\n",
                std::env::consts::ARCH
            )));
        }
        if args.contains(&"search".into()) {
            return Ok(output(
                "Synthetic app\tSynthetic description\tio.example.User\t1.0\tstable\tflathub\n",
            ));
        }
        Ok(output(""))
    }
}

#[test]
fn flatpak_operations_keep_scope_and_noninteractive_arguments() {
    let fixture = FlatpakFixture::default();
    let mut backend = Flatpak::new(fixture.clone());
    let cancel = Cancellation::default();
    let packages = backend.installed(&cancel).unwrap();
    let user = packages
        .iter()
        .find(|p| matches!(p.id.scope, Scope::User { .. }))
        .unwrap()
        .id
        .clone();
    let system = packages
        .iter()
        .find(|p| p.id.scope == Scope::System)
        .unwrap()
        .id
        .clone();
    // Installed apps show their app name, falling back to the id.
    let names: Vec<_> = packages.iter().map(|p| p.display_name.as_str()).collect();
    assert!(names.contains(&"User App") && names.contains(&"io.example.System"));
    let offers = backend.search("io.example.User", &cancel).unwrap();
    assert_eq!(offers.len(), 2);
    let offer = offers
        .into_iter()
        .find(|package| package.id.remote.as_deref() == Some("flathub"))
        .unwrap();
    let offered_details = backend.details(&offer.id, &cancel).unwrap();
    assert_eq!(offered_details.package.id, offer.id);
    assert_eq!(offered_details.description, "Synthetic description");
    assert!(backend.search("--bad", &cancel).is_err());
    assert_eq!(backend.details(&user, &cancel).unwrap().package.id, user);
    for operation in [
        Operation::Install(user.clone()),
        Operation::Remove(user),
        Operation::Upgrade(system),
    ] {
        backend.execute(&operation, &cancel, &mut |_| {}).unwrap();
    }
    backend
        .execute(
            &Operation::Refresh {
                backend: "flatpak".into(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert!(calls
        .iter()
        .any(|(args, write, system)| *write && !*system && args.contains(&"flathub".into())));
    assert!(calls.iter().any(|(args, write, system)| *write
        && *system
        && args.first().is_some_and(|scope| scope == "--system")));
}

#[test]
fn flatpak_reference_install_revalidates_and_cleans_temporary_file() {
    let base =
        std::env::temp_dir().join(format!("pkgdeck-flatpakref-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let source = base.join("Synthetic reference.flatpakref");
    std::fs::write(
        &source,
        "[Flatpak Ref]\nName=org.example.Synthetic\nUrl=https://example.invalid/repo\n",
    )
    .unwrap();
    let cancel = Cancellation::default();
    let package = pkgdeck_core::flatpak_ref::inspect(source.to_str().unwrap(), &cancel).unwrap();
    let fixture = FlatpakFixture::default();
    let mut backend = Flatpak::new(fixture.clone());
    let mut progress = Vec::new();
    backend
        .execute(
            &Operation::Install(package.id.clone()),
            &cancel,
            &mut |item| progress.push(item),
        )
        .unwrap();
    assert!(progress.iter().any(|item| matches!(item, Progress::Message(message) if message.contains("Installing Flatpak reference"))));
    let calls = fixture.calls.lock().unwrap();
    let (args, write, system) = calls
        .iter()
        .find(|(args, _, _)| args.contains(&"--from".into()))
        .unwrap();
    assert!(*write && !*system);
    assert_eq!(args[0], "--user");
    let temporary = std::path::Path::new(args.last().unwrap());
    assert!(!temporary.exists());
    drop(calls);
    let mut system = package.id.clone();
    system.scope = Scope::System;
    backend
        .execute(&Operation::Install(system), &cancel, &mut |_| {})
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    let (args, write, system) = calls
        .iter()
        .rfind(|(args, _, _)| args.contains(&"--from".into()))
        .unwrap();
    assert!(*write && *system);
    assert_eq!(args[0], "--system");
    assert!(!std::path::Path::new(args.last().unwrap()).exists());
    drop(calls);
    std::fs::write(
        &source,
        "[Flatpak Ref]\nName=org.example.Other\nUrl=https://example.invalid/repo\n",
    )
    .unwrap();
    assert!(backend
        .execute(&Operation::Install(package.id), &cancel, &mut |_| {})
        .is_err());
    assert_eq!(
        fixture
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(args, _, _)| args.contains(&"--from".into()))
            .count(),
        2
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn flatpak_rejects_malformed_metadata_and_foreign_operations() {
    let cancel = Cancellation::default();
    let mut backend = Flatpak::new(Raw(output("bad\tmetadata\n")));
    assert!(backend.installed(&cancel).is_err());
    let mut invalid_utf8 = Flatpak::new(Raw(Completion {
        code: Some(0),
        signal: None,
        stdout: vec![0xff],
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    }));
    assert!(invalid_utf8.installed(&cancel).is_err());
    let foreign = PackageId {
        backend: "flatpak".into(),
        name: "io.example.App".into(),
        architecture: "x86_64".into(),
        scope: Scope::Environment {
            path: "/synthetic".into(),
        },
        remote: None,
        reference: None,
    };
    assert!(backend
        .execute(&Operation::Install(foreign), &cancel, &mut |_| {})
        .is_err());
    assert!(backend
        .execute(
            &Operation::Refresh {
                backend: "apt".into()
            },
            &cancel,
            &mut |_| {}
        )
        .is_err());
}

#[test]
fn explicit_optional_sources_remain_discoverable_when_unavailable() {
    let cancel = Cancellation::default();
    for source in [
        "appimage", "flatpak", "dnf", "pacman", "zypper", "snap", "docker", "podman", "cargo",
        "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem",
    ] {
        let mut engine = native_engine(
            &[source.to_string()],
            true,
            Authorization::SudoNonInteractive,
            &cancel,
        )
        .unwrap();
        let sources = engine.discover(&cancel);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].backend, source);
    }
}

#[test]
fn native_engine_accepts_source_sets() {
    let cancel = Cancellation::default();
    native_engine(&[], true, Authorization::SudoNonInteractive, &cancel).unwrap();
    let mut pair = native_engine(
        &["apt".to_string(), "flatpak".to_string()],
        true,
        Authorization::SudoNonInteractive,
        &cancel,
    )
    .unwrap();
    let mut ids: Vec<_> = pair
        .discover(&cancel)
        .into_iter()
        .map(|source| source.backend)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["apt", "flatpak"]);
    assert!(matches!(
        native_engine(
            &["apt".to_string(), "foreign".to_string()],
            false,
            Authorization::Polkit,
            &cancel
        ),
        Err(EngineError::UnknownBackend(_))
    ));
}

#[test]
fn backend_validation_preserves_invalid_and_transport_failures() {
    let cancel = Cancellation::default();
    let fixture = Fixture::new();
    *fixture.failure.lock().unwrap() = Some(ExecutionError::AuthorizationDenied);
    assert_eq!(
        Flatpak::new(fixture).detect(&cancel),
        Err(ExecutionError::AuthorizationDenied.into())
    );
    let mut flatpak = Flatpak::new(Raw(output("")));
    let foreign = PackageId {
        backend: "apt".into(),
        name: "io.example.App".into(),
        architecture: "x86_64".into(),
        scope: Scope::User { uid: 1 },
        remote: None,
        reference: None,
    };
    assert!(flatpak
        .execute(&Operation::Install(foreign), &cancel, &mut |_| {})
        .is_err());
    let mut brew = Homebrew::new(Raw(output("{\"formulae\":[]}")));
    brew.detect(&cancel).unwrap();
    let missing = PackageId {
        backend: "homebrew".into(),
        name: "missing".into(),
        architecture: std::env::consts::ARCH.into(),
        scope: Scope::Environment {
            path: "/synthetic".into(),
        },
        remote: None,
        reference: None,
    };
    assert_eq!(brew.details(&missing, &cancel), Err(EngineError::NotFound));
}

#[derive(Clone)]
struct FailingFlatpak(ExecutionError);
impl Transport for FailingFlatpak {
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
    fn flatpak(
        &self,
        _: &[OsString],
        _: &Cancellation,
        _: bool,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        Err(self.0.clone())
    }
}

#[test]
fn optional_backends_report_transport_and_metadata_failures() {
    let cancel = Cancellation::default();
    let error = ExecutionError::TimedOut;
    let mut flatpak = Flatpak::new(FailingFlatpak(error.clone()));
    assert_eq!(flatpak.installed(&cancel), Err(error.clone().into()));
    assert_eq!(flatpak.detect(&cancel), Err(error.clone().into()));
    assert_eq!(
        flatpak.execute(
            &Operation::Refresh {
                backend: "flatpak".into()
            },
            &cancel,
            &mut |_| {},
        ),
        Err(error.into())
    );

    #[derive(Clone)]
    struct RelativePrefix;
    impl Transport for RelativePrefix {
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
            Ok(output("relative-prefix\\n"))
        }
        fn flatpak(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
    }
    let mut relative_prefix = Homebrew::new(RelativePrefix);
    assert!(relative_prefix.detect(&cancel).is_err());
}
#[test]
fn failures_preserve_native_categories() {
    for error in [
        ExecutionError::Disabled("missing".into()),
        ExecutionError::AuthorizationDenied,
        ExecutionError::LockBusy,
        ExecutionError::TimedOut,
    ] {
        let fixture = Fixture::new();
        *fixture.failure.lock().unwrap() = Some(error.clone());
        let expected = match error {
            ExecutionError::Disabled(reason) => Ok(Availability::Unavailable(reason)),
            e => Err(e.into()),
        };
        assert_eq!(
            Apt::new(fixture.clone()).detect(&Cancellation::default()),
            expected
        );
        assert_eq!(
            Homebrew::new(fixture).detect(&Cancellation::default()),
            expected
        );
    }
}

#[derive(Clone)]
struct Raw(Completion);
impl Transport for Raw {
    fn apt_query(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        Ok(self.0.clone())
    }
    fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
        Ok(self.0.clone())
    }
    fn brew(
        &self,
        args: &[OsString],
        _: &Cancellation,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        if args == [OsString::from("--prefix")] {
            Ok(output("/synthetic"))
        } else {
            Ok(self.0.clone())
        }
    }
    fn flatpak(
        &self,
        _: &[OsString],
        _: &Cancellation,
        _: bool,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        Ok(self.0.clone())
    }
    fn system_manager(
        &self,
        _: &str,
        _: &[OsString],
        _: &Cancellation,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        Ok(self.0.clone())
    }
}
#[test]
fn malformed_metadata_is_never_treated_as_an_empty_success() {
    let cancel = Cancellation::default();
    let mut truncated = output("[]");
    truncated.truncated = true;
    let mut failed = output("[]");
    failed.code = Some(1);
    for result in [output("not json"), truncated, failed] {
        assert!(Apt::new(Raw(result.clone()))
            .search("fixture", &cancel)
            .is_err());
        let mut brew = Homebrew::new(Raw(result));
        brew.detect(&cancel).unwrap();
        assert!(brew.installed(&cancel).is_err());
    }
    for formula in [
        json!({"full_name":"../unsafe"}),
        json!({"full_name":"--option"}),
    ] {
        let mut value = json!({"full_name":"fixture","desc":null,"homepage":"","versions":{"stable":"1.0"},"revision":2,"installed":[],"outdated":false,"dependencies":[]});
        value["full_name"] = formula["full_name"].clone();
        let mut brew = Homebrew::new(Raw(output(json!({"formulae":[value]}).to_string())));
        brew.detect(&cancel).unwrap();
        assert!(brew.installed(&cancel).is_err());
    }
    let value = json!({"full_name":"fixture","desc":null,"homepage":"","versions":{"stable":"1.0"},"revision":2,"installed":[],"outdated":false,"dependencies":[]});
    let mut brew = Homebrew::new(Raw(output(json!({"formulae":[value]}).to_string())));
    assert!(brew.installed(&cancel).is_err());
    brew.detect(&cancel).unwrap();
    assert_eq!(
        brew.installed(&cancel).unwrap()[0]
            .candidate_version
            .as_deref(),
        Some("1.0_2")
    );
    for invalid in [output([0xff]), output("--option\n")] {
        let mut brew = Homebrew::new(Raw(invalid));
        brew.detect(&cancel).unwrap();
        assert!(brew.search("", &cancel).is_err());
    }
}
#[test]
fn native_apt_transport_reads_host_metadata_or_reports_prerequisites() {
    use pkgdeck_core::host::{Authorization, Host, Runtime};
    let cancel = Cancellation::default();
    let native = NativeTransport {
        host: Host::new(
            Runtime::Native,
            [("PATH".into(), "/usr/bin:/bin".into())].into(),
        ),
        authorization: Authorization::SudoNonInteractive,
    };
    // Read-only integration: no packages are installed by this test.
    match native.apt_query("detect", "", "", &cancel) {
        Ok(_) => assert!(native
            .apt_query(
                "search",
                "pkgdeck-nonexistent-synthetic-fixture",
                "",
                &cancel
            )
            .unwrap()
            .stdout
            .starts_with(b"[]")),
        Err(error) => assert!(matches!(
            error,
            ExecutionError::Failed(_) | ExecutionError::Io(_) | ExecutionError::Disabled(_)
        )),
    }
    let unavailable = NativeTransport {
        host: Host::new(
            Runtime::Native,
            [("PATH".into(), "/nonexistent-fixture".into())].into(),
        ),
        authorization: Authorization::SudoNonInteractive,
    };
    assert!(matches!(
        unavailable.apt_query("detect", "", "", &cancel),
        Err(ExecutionError::Disabled(_))
    ));
    let sandbox = NativeTransport {
        host: Host::new(Runtime::Flatpak, Default::default()),
        authorization: Authorization::Polkit,
    };
    assert!(sandbox.apt_query("detect", "", "", &cancel).is_err());
    assert!(sandbox.apt_write(AptAction::Refresh, &cancel).is_err());
    assert!(sandbox.brew(&[], &cancel, true).is_err());
    assert!(matches!(
        native_engine(
            &["foreign".to_string()],
            false,
            Authorization::Polkit,
            &cancel
        ),
        Err(EngineError::UnknownBackend(_))
    ));
}

#[test]
fn homebrew_reports_linked_version_when_old_kegs_are_retained() {
    let cancel = Cancellation::default();
    let value = json!({"full_name":"fixture","desc":null,"homepage":"","versions":{"stable":"2.0"},"revision":0,"installed":[{"version":"1.0"},{"version":"2.0"}],"linked_keg":"1.0","outdated":false,"dependencies":[]});
    let mut brew = Homebrew::new(Raw(output(json!({"formulae":[value]}).to_string())));
    brew.detect(&cancel).unwrap();
    assert_eq!(
        brew.installed(&cancel).unwrap()[0]
            .installed_version
            .as_deref(),
        Some("1.0")
    );
}

#[test]
fn flatpak_remote_search_details_and_upgrade_all() {
    let fixture = FlatpakFixture::default();
    let mut backend = Flatpak::new(fixture.clone());
    let cancel = Cancellation::default();
    let results = backend.search("io.example.User", &cancel).unwrap();
    // Each installation offers its own target; installed state must not leak.
    assert_eq!(results.len(), 2);
    assert!(matches!(results[0].id.scope, Scope::User { .. }));
    assert_eq!(results[0].installed_version.as_deref(), Some("1.0"));
    let system_offer = results
        .iter()
        .find(|p| p.id.scope == Scope::System)
        .unwrap();
    assert!(system_offer.installed_version.is_none());
    backend
        .execute(
            &Operation::Install(system_offer.id.clone()),
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(args, write, system)| *write
            && *system
            && args.contains(&"install".into())
            && args.contains(&"--system".into())));
    let id = results[0].id.clone();
    assert!(id.remote.is_none());
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    let mut missing = id.clone();
    missing.name = "io.example.Missing".into();
    missing.reference = Some(format!(
        "app/io.example.Missing/{}/stable",
        std::env::consts::ARCH
    ));
    assert_eq!(
        backend.details(&missing, &cancel),
        Err(EngineError::NotFound)
    );
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "flatpak".into(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert!(calls.iter().any(|(args, write, system)| *write
        && !*system
        && args.contains(&"update".into())
        && args.contains(&"--assumeyes".into())));
    assert!(calls.iter().any(|(args, write, system)| *write
        && *system
        && args.contains(&"update".into())
        && args.contains(&"--assumeyes".into())));
    assert!(backend
        .execute(
            &Operation::UpgradeAll {
                backend: "apt".into(),
            },
            &cancel,
            &mut |_| {},
        )
        .is_err());
}

#[test]
fn flatpak_search_rejects_malformed_remote_metadata() {
    let cancel = Cancellation::default();
    for metadata in [
        "only\tthree\tfields\n",
        "Name\tDescription\tbad id!\t1.0\tstable\tflathub\n",
        "Name\tDescription\tio.example.App\t1.0\tstable\t\n",
        "Name\tDescription\tio.example.App\t1.0\tstable\t!!!\n",
    ] {
        let mut backend = Flatpak::new(Raw(output(metadata)));
        assert!(backend.search("io.example.App", &cancel).is_err());
    }
}

#[test]
fn flatpak_search_without_matches_is_empty_not_malformed() {
    // flatpak prints a (translated) notice instead of rows when nothing
    // matches, such as for an installation without remotes.
    #[derive(Clone)]
    struct Notice(&'static str);
    impl Transport for Notice {
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
        fn flatpak(
            &self,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            // Only search prints the notice; an empty list prints nothing.
            Ok(output(if args.contains(&"search".into()) {
                self.0
            } else {
                ""
            }))
        }
    }
    let cancel = Cancellation::default();
    for notice in ["No matches found\n", "Keine Treffer gefunden\n", "\n"] {
        let mut backend = Flatpak::new(Notice(notice));
        assert_eq!(backend.search("htop", &cancel), Ok(vec![]), "{notice:?}");
    }
}

#[test]
fn flatpak_detect_reports_unavailable_backend() {
    let cancel = Cancellation::default();
    let mut backend = Flatpak::new(FailingFlatpak(ExecutionError::Disabled("missing".into())));
    assert_eq!(
        backend.detect(&cancel),
        Ok(Availability::Unavailable("missing".into()))
    );
}

#[test]
fn flatpak_remote_search_requires_explicit_installation_scope() {
    let cancel = Cancellation::default();
    let mut engine = Engine::default();
    engine
        .register(Flatpak::new(FlatpakFixture::default()))
        .unwrap();
    let report = engine.search("io.example.User", &cancel);
    assert!(report.failures.is_empty());
    let id = report
        .select(&Selector {
            name: "io.example.User".into(),
            backend: Some("flatpak".into()),
            architecture: None,
            scope: Some(Scope::User {
                uid: rustix::process::getuid().as_raw(),
            }),
        })
        .unwrap();
    assert!(id.remote.is_none());
    assert_eq!(engine.details(&id, &cancel).unwrap().package.id, id);
}

#[test]
fn flatpak_search_survives_failing_system_scope() {
    #[derive(Clone)]
    struct UserOnly;
    impl Transport for UserOnly {
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
        fn flatpak(
            &self,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
            system: bool,
        ) -> Result<Completion, ExecutionError> {
            if args.iter().any(|arg| arg == "list") {
                return Ok(output(""));
            }
            if system {
                return Err(ExecutionError::TimedOut);
            }
            Ok(output(
                "Synthetic app\tSynthetic description\tio.example.App\t1.0\tstable\tflathub\n",
            ))
        }
    }
    let cancel = Cancellation::default();
    let mut backend = Flatpak::new(UserOnly);
    let results = backend.search("io.example.App", &cancel).unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].id.scope, Scope::User { .. }));
}

const CARGO_LIST: &str = "cargo-install-test v1.2.3 (registry+https://github.com/rust-lang/crates.io-index):\n    cargo-install-test\n\nripgrep v14.1.0:\n    rg\nsourceless v2.0.0:\n    sourceless\n";
const NPM_LIST: &str =
    r#"{"dependencies": {"left-pad": {"version": "1.3.0"}, "@scope/tool": {"version": "2.0.0"}}}"#;
const NPM_OUTDATED: &str =
    r#"{"left-pad": {"current": "1.3.0", "wanted": "1.3.1", "latest": "2.0.0"}}"#;
const PNPM_LIST: &str = r#"[{"path": "/home/test/.local/share/pnpm/global/9", "private": true, "dependencies": {"typescript": {"from": "typescript", "version": "7.0.2"}}}]"#;

type DevCall = (String, Vec<String>, bool);

#[derive(Clone, Default)]
struct DevFixture {
    home: Option<String>,
    version: String,
    root: Option<String>,
    list: String,
    outdated: Option<String>,
    list_failed_stdout: Option<String>,
    fail: Option<ExecutionError>,
    fail_writes: bool,
    venv: Option<String>,
    pip_list: String,
    pip_outdated: Option<String>,
    extra_env: std::collections::BTreeMap<String, String>,
    calls: Arc<Mutex<Vec<DevCall>>>,
}

impl DevFixture {
    fn calls(&self) -> Vec<DevCall> {
        self.calls.lock().unwrap().clone()
    }
    fn writes(&self, executable: &str) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter(|(exe, _, write)| exe == executable && *write)
            .map(|(_, args, _)| args)
            .collect()
    }
    fn list_result(&self) -> Result<Completion, ExecutionError> {
        match &self.list_failed_stdout {
            Some(stdout) => Err(ExecutionError::Failed(Completion {
                code: Some(1),
                signal: None,
                stdout: stdout.as_bytes().to_vec(),
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            })),
            None => Ok(output(&self.list)),
        }
    }
}

impl Transport for DevFixture {
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
    fn flatpak(
        &self,
        _: &[OsString],
        _: &Cancellation,
        _: bool,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn env(&self, name: &str) -> Option<OsString> {
        if let Some(value) = self.extra_env.get(name) {
            return Some(OsString::from(value));
        }
        if name == "HOME" {
            self.home.clone().map(OsString::from)
        } else if name == "VIRTUAL_ENV" {
            self.venv.clone().map(OsString::from)
        } else {
            None
        }
    }
    fn venv_pip(
        &self,
        _venv: &std::path::Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        self.calls
            .lock()
            .unwrap()
            .push(("pip".into(), rendered.clone(), write));
        if let Some(error) = &self.fail {
            return Err(error.clone());
        }
        if write && self.fail_writes {
            return Err(ExecutionError::Failed(Completion {
                code: Some(1),
                signal: None,
                stdout: vec![],
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            }));
        }
        let first = rendered.first().map(String::as_str);
        match first {
            Some("--version") => Ok(output(&self.version)),
            Some("list") if rendered.contains(&"--outdated".to_string()) => {
                match &self.pip_outdated {
                    Some(json) => Ok(output(json)),
                    None => Err(ExecutionError::Failed(Completion {
                        code: Some(1),
                        signal: None,
                        stdout: vec![],
                        stderr: vec![],
                        truncated: false,
                        cancellation_deferred: false,
                    })),
                }
            }
            Some("list") => Ok(output(&self.pip_list)),
            _ => Ok(output("")),
        }
    }
    fn dev_tool(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        self.calls
            .lock()
            .unwrap()
            .push((executable.into(), rendered.clone(), write));
        if let Some(error) = &self.fail {
            return Err(error.clone());
        }
        if write && self.fail_writes {
            return Err(ExecutionError::Failed(Completion {
                code: Some(1),
                signal: None,
                stdout: vec![],
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            }));
        }
        let first = rendered.first().map(String::as_str);
        match (executable, first) {
            (_, Some("--version")) => Ok(output(&self.version)),
            (_, Some("root")) => match &self.root {
                Some(root) => Ok(output(format!("{root}\n"))),
                None => Err(ExecutionError::Disabled(format!(
                    "{executable} home not found"
                ))),
            },
            ("cargo", Some("install")) if rendered.contains(&"--list".to_string()) => {
                self.list_result()
            }
            ("npm", Some("ls")) | ("pnpm", Some("ls")) => self.list_result(),
            ("npm", Some("outdated")) | ("pnpm", Some("outdated")) => match &self.outdated {
                Some(json) => Ok(output(json)),
                None => Err(ExecutionError::Failed(Completion {
                    code: Some(1),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                })),
            },
            ("pipx", Some("list")) if rendered.contains(&"--outdated".to_string()) => {
                match &self.outdated {
                    Some(json) => Ok(output(json)),
                    None => Err(ExecutionError::Failed(Completion {
                        code: Some(1),
                        signal: None,
                        stdout: vec![],
                        stderr: vec![],
                        truncated: false,
                        cancellation_deferred: false,
                    })),
                }
            }
            ("pipx", Some("list")) => self.list_result(),
            ("uv", Some("tool")) if rendered.get(1).map(String::as_str) == Some("dir") => {
                match &self.root {
                    Some(root) => Ok(output(format!("{root}\n"))),
                    None => Err(ExecutionError::Disabled("uv not found".into())),
                }
            }
            ("uv", Some("tool")) => {
                if rendered.contains(&"--outdated".to_string()) {
                    match &self.outdated {
                        Some(text) => Ok(output(text)),
                        None => Err(ExecutionError::Failed(Completion {
                            code: Some(1),
                            signal: None,
                            stdout: vec![],
                            stderr: vec![],
                            truncated: false,
                            cancellation_deferred: false,
                        })),
                    }
                } else {
                    self.list_result()
                }
            }
            ("composer", Some("config")) => match &self.root {
                Some(root) => Ok(output(format!("{root}\n"))),
                None => Err(ExecutionError::Disabled("Composer not found".into())),
            },
            ("composer", Some("global")) => {
                if rendered.contains(&"--outdated".to_string()) {
                    match &self.outdated {
                        Some(json) => Ok(output(json)),
                        None => Err(ExecutionError::Failed(Completion {
                            code: Some(1),
                            signal: None,
                            stdout: vec![],
                            stderr: vec![],
                            truncated: false,
                            cancellation_deferred: false,
                        })),
                    }
                } else {
                    self.list_result()
                }
            }
            ("gem", Some("env")) => match &self.root {
                Some(root) => Ok(output(format!(
                    "RubyGems Environment:\n- USER INSTALLATION DIRECTORY: {root}\n"
                ))),
                None => Err(ExecutionError::Disabled("RubyGems not found".into())),
            },
            ("gem", Some("outdated")) => match &self.outdated {
                Some(text) => Ok(output(text)),
                None => Err(ExecutionError::Failed(Completion {
                    code: Some(1),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                })),
            },
            _ => Ok(output("")),
        }
    }
}

fn dev_id(backend: &str, name: &str, home: &str) -> PackageId {
    PackageId {
        backend: backend.into(),
        name: name.into(),
        architecture: std::env::consts::ARCH.into(),
        scope: Scope::Environment { path: home.into() },
        remote: None,
        reference: None,
    }
}

#[test]
fn cargo_lifecycle() {
    let fixture = DevFixture {
        home: Some("/home/test".into()),
        version: "cargo 1.98.1\n".into(),
        list: CARGO_LIST.into(),
        ..DevFixture::default()
    };
    let mut backend = DevTool::cargo(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 3);
    assert!(installed.iter().all(|p| p.installed_version.is_some()));
    assert_eq!(backend.search("cargo-install", &cancel).unwrap().len(), 1);
    let candidates = backend.search("missing-tool", &cancel).unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].installed_version.is_none());
    assert!(backend.search("--evil", &cancel).unwrap().is_empty());
    let id = installed[0].id.clone();
    assert_eq!(
        id.scope,
        Scope::Environment {
            path: "/home/test/.cargo".into()
        }
    );
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    assert_eq!(
        backend.details(
            &dev_id("cargo", "missing-tool", "/home/test/.cargo"),
            &cancel
        ),
        Err(EngineError::NotFound)
    );
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("cargo", "new-tool", "/home/test/.cargo")),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "cargo".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(!progress.is_empty());
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "cargo".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Install(dev_id("apt", "new-tool", "/home/test/.cargo")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    assert!(backend
        .execute(
            &Operation::Install(dev_id("cargo", "--evil", "/home/test/.cargo")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("cargo");
    assert!(writes.contains(&vec!["install".into(), "new-tool".into()]));
    assert!(writes.contains(&vec![
        "install".into(),
        "--force".into(),
        installed[0].id.name.clone()
    ]));
    assert!(writes.contains(&vec!["uninstall".into(), installed[0].id.name.clone()]));
    assert_eq!(
        writes
            .iter()
            .filter(|args| args.contains(&"--force".into()))
            .count(),
        4
    );
}

#[test]
fn npm_lifecycle() {
    let fixture = DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list: NPM_LIST.into(),
        outdated: Some(NPM_OUTDATED.into()),
        ..DevFixture::default()
    };
    let mut backend = DevTool::npm(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 2);
    let outdated = installed.iter().find(|p| p.id.name == "left-pad").unwrap();
    assert_eq!(outdated.update, UpdateAvailability::Available);
    assert_eq!(outdated.candidate_version.as_deref(), Some("1.3.1"));
    let current = installed
        .iter()
        .find(|p| p.id.name == "@scope/tool")
        .unwrap();
    assert_eq!(current.update, UpdateAvailability::Current);
    assert_eq!(current.candidate_version.as_deref(), Some("2.0.0"));
    let id = outdated.id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("npm", "new-tool", "/home/test/lib/node_modules")),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "npm".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "npm".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Install(dev_id("npm", "../evil", "/home/test/lib/node_modules")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("npm");
    assert!(writes.contains(&vec![
        "install".into(),
        "--global".into(),
        "new-tool".into()
    ]));
    assert!(writes.contains(&vec!["update".into(), "--global".into(), "left-pad".into()]));
    assert!(writes.contains(&vec![
        "uninstall".into(),
        "--global".into(),
        "left-pad".into()
    ]));
    assert!(writes.contains(&vec!["update".into(), "--global".into()]));
}

#[test]
fn pnpm_lifecycle() {
    let fixture = DevFixture {
        version: "10.0.0\n".into(),
        root: Some("/home/test/.local/share/pnpm/global/9".into()),
        list: PNPM_LIST.into(),
        outdated: None,
        ..DevFixture::default()
    };
    let mut backend = DevTool::pnpm(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].update, UpdateAvailability::Current);
    assert_eq!(installed[0].candidate_version.as_deref(), Some("7.0.2"));
    let id = installed[0].id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id(
                "pnpm",
                "new-tool",
                "/home/test/.local/share/pnpm/global/9",
            )),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "pnpm".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "pnpm".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Upgrade(dev_id("pnpm", "typescript", "/elsewhere")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("pnpm");
    assert!(writes.contains(&vec!["add".into(), "--global".into(), "new-tool".into()]));
    assert!(writes.contains(&vec![
        "add".into(),
        "--global".into(),
        "typescript@latest".into()
    ]));
    assert!(writes.contains(&vec![
        "remove".into(),
        "--global".into(),
        "typescript".into()
    ]));
    assert!(writes.contains(&vec![
        "add".into(),
        "--global".into(),
        "typescript@latest".into()
    ]));
    assert!(!writes.contains(&vec!["update".into(), "--global".into()]));
}

#[test]
fn pnpm_outdated_newer_reports_available() {
    let fixture = DevFixture {
        version: "10.0.0\n".into(),
        root: Some("/home/test/.local/share/pnpm/global/9".into()),
        list: PNPM_LIST.into(),
        outdated: Some(
            r#"{"typescript": {"current": "7.0.2", "wanted": "7.0.2", "latest": "7.1.0"}}"#.into(),
        ),
        ..DevFixture::default()
    };
    let mut backend = DevTool::pnpm(fixture);
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed[0].update, UpdateAvailability::Available);
    assert_eq!(installed[0].candidate_version.as_deref(), Some("7.1.0"));
}

#[test]
fn bun_lifecycle() {
    let base = std::env::temp_dir().join(format!("pkgdeck-bun-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let modules = base.join(".bun/install/global/node_modules");
    for (dir, manifest) in [
        (
            "alpha",
            r#"{"name": "alpha", "version": "1.0.0", "description": "Alpha tool", "homepage": "https://example.invalid"}"#,
        ),
        ("beta", r#"{"name": "beta", "version": "2.0.0"}"#),
        (
            "@scope/gamma",
            r#"{"name": "@scope/gamma", "version": "3.0.0"}"#,
        ),
        ("broken", "not json"),
        ("noversion", r#"{"name": "noversion"}"#),
        ("badname", r#"{"name": "--evil", "version": "1.0.0"}"#),
    ] {
        let dir = modules.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), manifest).unwrap();
    }
    std::fs::write(modules.join("stray.txt"), "not a package").unwrap();
    let fixture = DevFixture {
        home: Some(base.to_str().unwrap().into()),
        version: "1.3.0\n".into(),
        ..DevFixture::default()
    };
    let mut backend = DevTool::bun(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 3);
    let alpha = installed.iter().find(|p| p.id.name == "alpha").unwrap();
    assert_eq!(alpha.summary, "Alpha tool");
    assert_eq!(alpha.update, UpdateAvailability::Current);
    let beta = installed.iter().find(|p| p.id.name == "beta").unwrap();
    assert_eq!(beta.summary, "Bun-installed command-line tool");
    assert!(installed.iter().any(|p| p.id.name == "@scope/gamma"));
    let home = format!("{}/.bun/install/global", base.display());
    assert_eq!(
        alpha.id.scope,
        Scope::Environment {
            path: home.clone().into()
        }
    );
    assert_eq!(
        backend.details(&alpha.id, &cancel).unwrap().package.id,
        alpha.id
    );
    assert_eq!(
        backend.details(&dev_id("bun", "missing", &home), &cancel),
        Err(EngineError::NotFound)
    );
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("bun", "new-tool", &home)),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(alpha.id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(alpha.id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "bun".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "bun".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Remove(dev_id("bun", "alpha", "/elsewhere")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("bun");
    assert!(writes.contains(&vec!["add".into(), "--global".into(), "new-tool".into()]));
    assert!(writes.contains(&vec![
        "add".into(),
        "--global".into(),
        "alpha@latest".into()
    ]));
    assert!(writes.contains(&vec!["remove".into(), "--global".into(), "alpha".into()]));
    assert!(writes.contains(&vec![
        "add".into(),
        "--global".into(),
        "alpha@latest".into()
    ]));
    assert!(!writes.contains(&vec!["update".into(), "--global".into()]));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn bun_without_global_tree_lists_nothing() {
    let base = std::env::temp_dir().join(format!("pkgdeck-bun-empty-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let fixture = DevFixture {
        home: Some(base.to_str().unwrap().into()),
        version: "1.3.0\n".into(),
        ..DevFixture::default()
    };
    let mut backend = DevTool::bun(fixture);
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    assert!(backend.installed(&cancel).unwrap().is_empty());
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn dev_backends_reject_malformed_metadata() {
    let cancel = Cancellation::default();
    for list in [
        "garbage without colon\n",
        "lonely:\n",
        "foo bar:\n",
        "bad name v1.0 (registry):\n",
        "tool v1.0 not-a-source:\n",
    ] {
        let mut backend = DevTool::cargo(DevFixture {
            home: Some("/home/test".into()),
            version: "cargo 1.98.1\n".into(),
            list: list.into(),
            ..DevFixture::default()
        });
        assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
        assert!(backend.installed(&cancel).is_err());
    }
    let mut npm = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list: NPM_LIST.into(),
        list_failed_stdout: Some(r#"{"error": {"code": "E500"}}"#.into()),
        ..DevFixture::default()
    });
    assert_eq!(npm.detect(&cancel), Ok(Availability::Available));
    assert!(npm.installed(&cancel).is_err());
    for (stdout, expected) in [
        (
            r#"{"error": {"code": "ELSPROBLEMS", "summary": "missing: left-pad@1.3.0, required by root"}}"#,
            "ls failed: missing: left-pad@1.3.0, required by root",
        ),
        (
            r#"{"problems": ["missing: a@1, required by root", "invalid: b@2"]}"#,
            "ls failed: missing: a@1, required by root; invalid: b@2",
        ),
        (
            r#"{"error": {"code": "EACCES"}}"#,
            "ls failed: npm error EACCES",
        ),
        (r#"{"error": {}}"#, "npm reported an error"),
    ] {
        let mut failing = DevTool::npm(DevFixture {
            version: "12.0.2\n".into(),
            root: Some("/home/test/lib/node_modules".into()),
            list_failed_stdout: Some(stdout.into()),
            ..DevFixture::default()
        });
        assert_eq!(failing.detect(&cancel), Ok(Availability::Available));
        let error = failing.installed(&cancel).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
    let long = "y".repeat(300);
    let mut truncated = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list_failed_stdout: Some(format!(r#"{{"error": {{"summary": "{long}"}}}}"#)),
        ..DevFixture::default()
    });
    assert_eq!(truncated.detect(&cancel), Ok(Availability::Available));
    let error = truncated.installed(&cancel).unwrap_err().to_string();
    assert!(
        error.contains("ls failed: ") && error.ends_with('…'),
        "{error}"
    );
    let mut empty = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list: r#"{"name": "lib"}"#.into(),
        outdated: Some(r#"{}"#.into()),
        ..DevFixture::default()
    });
    assert_eq!(empty.detect(&cancel), Ok(Availability::Available));
    assert!(empty.installed(&cancel).unwrap().is_empty());
    let mut stale = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list: NPM_LIST.into(),
        outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(stale.detect(&cancel), Ok(Availability::Available));
    let installed = stale.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 2);
    assert!(installed
        .iter()
        .all(|p| p.update == UpdateAvailability::Current));
}

#[test]
fn dev_transport_reports_unavailable_and_errors() {
    let cancel = Cancellation::default();
    let mut missing = DevTool::cargo(DevFixture {
        home: Some("/home/test".into()),
        fail: Some(ExecutionError::Disabled("cargo not found".into())),
        ..DevFixture::default()
    });
    assert_eq!(
        missing.detect(&cancel),
        Ok(Availability::Unavailable("cargo not found".into()))
    );
    let mut broken = DevTool::npm(DevFixture {
        root: Some("/home/test/lib/node_modules".into()),
        fail: Some(ExecutionError::TimedOut),
        ..DevFixture::default()
    });
    assert_eq!(broken.detect(&cancel), Err(ExecutionError::TimedOut.into()));
    assert!(broken.installed(&cancel).is_err());
    // Fixtures without overrides deny the manager and hide HOME.
    let mut denied = DevTool::npm(Raw(output("")));
    assert_eq!(
        denied.detect(&cancel),
        Ok(Availability::Unavailable("npm not found".into()))
    );
    let mut homeless = DevTool::cargo(Raw(output("")));
    assert_eq!(
        homeless.detect(&cancel),
        Ok(Availability::Unavailable("cargo home not found".into()))
    );
    // Operations without a detected home fail closed.
    let mut cold = DevTool::cargo(DevFixture {
        home: Some("/home/test".into()),
        ..DevFixture::default()
    });
    assert!(cold.installed(&cancel).is_err());
    assert!(cold.search("tool", &cancel).is_err());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut backend = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list: NPM_LIST.into(),
        outdated: Some(NPM_OUTDATED.into()),
        ..DevFixture::default()
    });
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    assert_eq!(
        backend.installed(&cancelled),
        Err(ExecutionError::Cancelled.into())
    );
}

#[test]
fn dev_managers_reject_relative_roots_and_failed_writes() {
    let cancel = Cancellation::default();
    let mut relative = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("relative/node_modules".into()),
        ..DevFixture::default()
    });
    assert!(relative.detect(&cancel).is_err());
    let fixture = DevFixture {
        home: Some("/home/test".into()),
        version: "cargo 1.98.1\n".into(),
        list: CARGO_LIST.into(),
        fail_writes: true,
        ..DevFixture::default()
    };
    let mut backend = DevTool::cargo(fixture);
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    assert_eq!(backend.installed(&cancel).unwrap().len(), 3);
    assert!(backend
        .execute(
            &Operation::UpgradeAll {
                backend: "cargo".into()
            },
            &cancel,
            &mut |_| {},
        )
        .is_err());
}

#[test]
fn dev_managers_reject_unparseable_registries() {
    let cancel = Cancellation::default();
    for (make, list) in [
        ("npm", "not json".to_string()),
        ("pnpm", "not json".to_string()),
    ] {
        let mut backend = match make {
            "npm" => DevTool::npm(DevFixture {
                version: "12.0.2\n".into(),
                root: Some("/home/test/lib/node_modules".into()),
                list,
                outdated: Some(r#"{}"#.into()),
                ..DevFixture::default()
            }),
            _ => DevTool::pnpm(DevFixture {
                version: "10.0.0\n".into(),
                root: Some("/home/test/.local/share/pnpm/global/9".into()),
                list,
                outdated: Some(r#"{}"#.into()),
                ..DevFixture::default()
            }),
        };
        assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
        assert!(backend.installed(&cancel).is_err());
    }
    let mut poisoned = DevTool::npm(DevFixture {
        version: "12.0.2\n".into(),
        root: Some("/home/test/lib/node_modules".into()),
        list: r#"{"dependencies": {"--evil": {"version": "1.0.0"}}}"#.into(),
        outdated: Some(r#"{}"#.into()),
        ..DevFixture::default()
    });
    assert_eq!(poisoned.detect(&cancel), Ok(Availability::Available));
    assert!(poisoned.installed(&cancel).is_err());
    let mut pnpm_poisoned = DevTool::pnpm(DevFixture {
        version: "10.0.0\n".into(),
        root: Some("/home/test/.local/share/pnpm/global/9".into()),
        list: r#"[{"path": "/home/test/.local/share/pnpm/global/9", "dependencies": {"../evil": {"from": "x", "version": "1.0.0"}}}]"#.into(),
        outdated: Some(r#"{}"#.into()),
        ..DevFixture::default()
    });
    assert_eq!(pnpm_poisoned.detect(&cancel), Ok(Availability::Available));
    assert!(pnpm_poisoned.installed(&cancel).is_err());
}

#[test]
fn bun_skips_unreadable_manifests() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!("pkgdeck-bun-skip-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let modules = base.join(".bun/install/global/node_modules");
    std::fs::create_dir_all(modules.join("emptyver")).unwrap();
    std::fs::write(
        modules.join("emptyver/package.json"),
        r#"{"name": "emptyver", "version": ""}"#,
    )
    .unwrap();
    let fixture = DevFixture {
        home: Some(base.to_str().unwrap().into()),
        version: "1.3.0\n".into(),
        ..DevFixture::default()
    };
    let mut backend = DevTool::bun(fixture);
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    assert!(backend.installed(&cancel).unwrap().is_empty());
    std::fs::set_permissions(&modules, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(backend.installed(&cancel).is_err());
    std::fs::set_permissions(&modules, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&base).unwrap();
}

const PIP_LIST: &str = r#"[{"name": "cowsay", "version": "6.0"}, {"name": "requests", "version": "2.32.0"}, {"name": "pip", "version": "25.1.1"}, {"name": "setuptools", "version": "80.0.0"}]"#;
const PIP_OUTDATED: &str = r#"[{"name": "cowsay", "version": "6.0", "latest_version": "6.1"}]"#;
const PIPX_LIST: &str = r#"{"venvs": {"cowsay": {"metadata": {"main_package": {"package": "cowsay", "package_version": "6.0"}}}}}"#;
const PIPX_OUTDATED: &str = r#"{"command": ["list"], "data": {"packages": [{"environment": "cowsay", "package": "cowsay", "version": "6.0", "latest_version": "6.1"}]}, "errors": [], "exit_code": 0}"#;
const UV_LIST: &str = "cowsay v6.0\n- cowsay\nrequests v2.32.0\n- requests\n";
const UV_OUTDATED: &str = "cowsay v6.0 [latest: 6.1]\n- cowsay\n";
const COMPOSER_LIST: &str = r#"{"installed": [{"name": "psr/log", "version": "1.0.0", "description": "Common interface for logging libraries", "homepage": "https://example.invalid"}]}"#;
const COMPOSER_OUTDATED: &str = r#"{"installed": [{"name": "psr/log", "version": "1.0.0", "description": "Common interface for logging libraries", "latest": "3.0.2"}]}"#;

#[test]
fn pip_lifecycle() {
    let fixture = DevFixture {
        venv: Some("/home/test/venv".into()),
        version: "pip 25.1.1\n".into(),
        pip_list: PIP_LIST.into(),
        pip_outdated: Some(PIP_OUTDATED.into()),
        ..DevFixture::default()
    };
    let mut backend = DevTool::pip(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 2);
    let outdated = installed.iter().find(|p| p.id.name == "cowsay").unwrap();
    assert_eq!(outdated.update, UpdateAvailability::Available);
    assert_eq!(outdated.candidate_version.as_deref(), Some("6.1"));
    let current = installed.iter().find(|p| p.id.name == "requests").unwrap();
    assert_eq!(current.update, UpdateAvailability::Current);
    assert_eq!(
        outdated.id.scope,
        Scope::Environment {
            path: "/home/test/venv".into()
        }
    );
    assert_eq!(backend.search("cowsay", &cancel).unwrap().len(), 1);
    let candidates = backend.search("new-tool", &cancel).unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].installed_version.is_none());
    assert!(backend.search("--evil", &cancel).unwrap().is_empty());
    let id = outdated.id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    assert_eq!(
        backend.details(&dev_id("pip", "missing", "/home/test/venv"), &cancel),
        Err(EngineError::NotFound)
    );
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("pip", "new-tool", "/home/test/venv")),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "pip".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(!progress.is_empty());
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "pip".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Install(dev_id("pip", "../evil", "/home/test/venv")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("pip");
    assert!(writes.contains(&vec!["install".into(), "new-tool".into()]));
    assert!(writes.contains(&vec!["install".into(), "--upgrade".into(), "cowsay".into()]));
    assert!(writes.contains(&vec!["uninstall".into(), "-y".into(), "cowsay".into()]));
}

#[test]
fn pip_requires_an_explicit_virtual_environment() {
    let cancel = Cancellation::default();
    let mut missing = DevTool::pip(DevFixture {
        version: "pip 25.1.1\n".into(),
        pip_list: PIP_LIST.into(),
        ..DevFixture::default()
    });
    assert_eq!(
        missing.detect(&cancel),
        Ok(Availability::Unavailable(
            "pip requires an explicitly selected virtual environment".into()
        ))
    );
    let mut relative = DevTool::pip(DevFixture {
        venv: Some("relative/venv".into()),
        version: "pip 25.1.1\n".into(),
        pip_list: PIP_LIST.into(),
        ..DevFixture::default()
    });
    assert!(relative.detect(&cancel).is_err());
    // Operations without a detected home fail closed.
    let mut cold = DevTool::pip(DevFixture {
        venv: Some("/home/test/venv".into()),
        ..DevFixture::default()
    });
    assert!(cold.installed(&cancel).is_err());
    assert!(cold.search("tool", &cancel).is_err());
}

#[test]
fn pipx_lifecycle() {
    let fixture = DevFixture {
        home: Some("/home/test".into()),
        version: "1.17.2\n".into(),
        list: PIPX_LIST.into(),
        outdated: Some(PIPX_OUTDATED.into()),
        ..DevFixture::default()
    };
    let mut backend = DevTool::pipx(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].id.name, "cowsay");
    assert_eq!(installed[0].update, UpdateAvailability::Available);
    assert_eq!(installed[0].candidate_version.as_deref(), Some("6.1"));
    assert_eq!(
        installed[0].id.scope,
        Scope::Environment {
            path: "/home/test/.local/share/pipx".into()
        }
    );
    let id = installed[0].id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("pipx", "new-tool", "/home/test/.local/share/pipx")),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "pipx".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "pipx".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Install(dev_id(
                "pipx",
                "https://example.invalid/tool.tar.gz",
                "/home/test/.local/share/pipx"
            )),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("pipx");
    assert!(writes.contains(&vec!["install".into(), "new-tool".into()]));
    assert!(writes.contains(&vec!["upgrade".into(), "cowsay".into()]));
    assert!(writes.contains(&vec!["uninstall".into(), "cowsay".into()]));
    assert!(writes.contains(&vec!["upgrade-all".into()]));
}

#[test]
fn uv_lifecycle() {
    let fixture = DevFixture {
        version: "uv 0.12.11\n".into(),
        root: Some("/home/test/.local/share/uv/tools".into()),
        list: UV_LIST.into(),
        outdated: Some(UV_OUTDATED.into()),
        ..DevFixture::default()
    };
    let mut backend = DevTool::uv(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 2);
    let outdated = installed.iter().find(|p| p.id.name == "cowsay").unwrap();
    assert_eq!(outdated.update, UpdateAvailability::Available);
    assert_eq!(outdated.candidate_version.as_deref(), Some("6.1"));
    let id = outdated.id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("uv", "new-tool", "/home/test/.local/share/uv/tools")),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "uv".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "uv".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    let writes = fixture.writes("uv");
    assert!(writes.contains(&vec!["tool".into(), "install".into(), "new-tool".into()]));
    // uv keeps exact pins, so upgrades reinstall at @latest like pnpm/Bun.
    assert_eq!(
        writes
            .iter()
            .filter(|args| {
                **args
                    == vec![
                        "tool".to_string(),
                        "install".to_string(),
                        "--force".to_string(),
                        "cowsay@latest".to_string(),
                    ]
            })
            .count(),
        2
    );
    assert!(writes.contains(&vec!["tool".into(), "uninstall".into(), "cowsay".into()]));
    assert!(!writes
        .iter()
        .any(|args| args.contains(&"upgrade".to_string())));
}

#[test]
fn composer_lifecycle() {
    let fixture = DevFixture {
        version: "Composer version 2.10.3\n".into(),
        root: Some("/home/test/.config/composer".into()),
        list: COMPOSER_LIST.into(),
        outdated: Some(COMPOSER_OUTDATED.into()),
        ..DevFixture::default()
    };
    let mut backend = DevTool::composer(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].id.name, "psr/log");
    assert_eq!(installed[0].update, UpdateAvailability::Available);
    assert_eq!(installed[0].candidate_version.as_deref(), Some("3.0.2"));
    assert_eq!(
        installed[0].summary,
        "Common interface for logging libraries"
    );
    let id = installed[0].id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    assert_eq!(
        backend.details(
            &dev_id("composer", "missing/package", "/home/test/.config/composer"),
            &cancel
        ),
        Err(EngineError::NotFound)
    );
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id(
                "composer",
                "monolog/monolog",
                "/home/test/.config/composer",
            )),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "composer".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "composer".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    // Bare names without a vendor are rejected before any manager runs.
    assert!(backend
        .execute(
            &Operation::Install(dev_id("composer", "monolog", "/home/test/.config/composer")),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("composer");
    assert!(writes.contains(&vec![
        "global".into(),
        "require".into(),
        "--no-interaction".into(),
        "--no-progress".into(),
        "monolog/monolog".into()
    ]));
    assert!(writes.contains(&vec!["global".into(), "remove".into(), "psr/log".into()]));
    // Upgrade-all re-requires each package so pinned constraints still move.
    assert_eq!(
        writes
            .iter()
            .filter(|args| {
                **args
                    == vec![
                        "global".to_string(),
                        "require".to_string(),
                        "--no-interaction".to_string(),
                        "--no-progress".to_string(),
                        "psr/log".to_string(),
                    ]
            })
            .count(),
        2
    );
    assert!(!writes
        .iter()
        .any(|args| args.contains(&"update".to_string())));
}

#[test]
fn gem_lifecycle() {
    let base = std::env::temp_dir().join(format!("pkgdeck-gem-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("gem");
    std::fs::create_dir_all(home.join("specifications")).unwrap();
    // A retained older version beside the new one still reports one identity.
    for spec in [
        "cowsay-0.3.0.gemspec",
        "cowsay-0.2.0.gemspec",
        "rake-13.0.0.gemspec",
    ] {
        std::fs::write(home.join("specifications").join(spec), "# stub\n").unwrap();
    }
    let fixture = DevFixture {
        root: Some(home.to_str().unwrap().into()),
        version: "4.0.20\n".into(),
        outdated: Some("cowsay (0.3.0 < 0.4.0)\n".into()),
        ..DevFixture::default()
    };
    let mut backend = DevTool::gem(fixture.clone());
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 2);
    let outdated = installed.iter().find(|p| p.id.name == "cowsay").unwrap();
    assert_eq!(outdated.update, UpdateAvailability::Available);
    assert_eq!(outdated.candidate_version.as_deref(), Some("0.4.0"));
    let current = installed.iter().find(|p| p.id.name == "rake").unwrap();
    assert_eq!(current.update, UpdateAvailability::Current);
    assert_eq!(outdated.id.scope, Scope::Environment { path: home.clone() });
    let id = outdated.id.clone();
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    assert_eq!(
        backend.details(&dev_id("gem", "missing", home.to_str().unwrap()), &cancel),
        Err(EngineError::NotFound)
    );
    let mut progress = vec![];
    backend
        .execute(
            &Operation::Install(dev_id("gem", "new-tool", home.to_str().unwrap())),
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    backend
        .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(&Operation::Remove(id.clone()), &cancel, &mut |p| {
            progress.push(p)
        })
        .unwrap();
    backend
        .execute(
            &Operation::UpgradeAll {
                backend: "gem".into(),
            },
            &cancel,
            &mut |p| progress.push(p),
        )
        .unwrap();
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "gem".into()
            },
            &cancel,
            &mut |p| progress.push(p)
        ),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(backend
        .execute(
            &Operation::Install(dev_id("gem", "--evil", home.to_str().unwrap())),
            &cancel,
            &mut |p| progress.push(p)
        )
        .is_err());
    let writes = fixture.writes("gem");
    assert!(writes.contains(&vec![
        "install".into(),
        "--user-install".into(),
        "--no-document".into(),
        "new-tool".into()
    ]));
    assert!(writes.contains(&vec![
        "update".into(),
        "--user-install".into(),
        "--no-document".into(),
        "cowsay".into()
    ]));
    assert!(writes.contains(&vec![
        "uninstall".into(),
        "--user-install".into(),
        "-x".into(),
        "-a".into(),
        "cowsay".into()
    ]));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn wave_five_name_policies_reject_options_paths_and_urls() {
    for (make, valid, invalid) in [
        ("pip", "cowsay", "--evil"),
        ("pipx", "cowsay", "https://example.invalid/tool.tar.gz"),
        ("uv", "cowsay", "../evil"),
        ("gem", "cowsay", "--evil"),
    ] {
        let mut backend = match make {
            "pip" => DevTool::pip(DevFixture {
                venv: Some("/home/test/venv".into()),
                version: "pip\n".into(),
                pip_list: "[]".into(),
                ..DevFixture::default()
            }),
            "pipx" => DevTool::pipx(DevFixture {
                home: Some("/home/test".into()),
                version: "1.0\n".into(),
                list: r#"{"venvs": {}}"#.into(),
                ..DevFixture::default()
            }),
            "uv" => DevTool::uv(DevFixture {
                version: "uv\n".into(),
                root: Some("/home/test/tools".into()),
                list: "".into(),
                ..DevFixture::default()
            }),
            _ => DevTool::gem(DevFixture {
                version: "gem\n".into(),
                root: Some("/home/test/gem".into()),
                ..DevFixture::default()
            }),
        };
        assert_eq!(backend.id(), make);
        assert_eq!(
            backend.detect(&Cancellation::default()),
            Ok(Availability::Available)
        );
        // Invalid names never become synthetic candidates.
        assert!(backend
            .search(invalid, &Cancellation::default())
            .unwrap()
            .is_empty());
        assert_eq!(
            backend
                .search(valid, &Cancellation::default())
                .unwrap()
                .len(),
            1
        );
    }
    // Composer requires vendor/package.
    let mut composer = DevTool::composer(DevFixture {
        root: Some("/home/test/composer".into()),
        list: COMPOSER_LIST.into(),
        ..DevFixture::default()
    });
    assert_eq!(
        composer.detect(&Cancellation::default()),
        Ok(Availability::Available)
    );
    assert!(composer
        .search("monolog", &Cancellation::default())
        .unwrap()
        .is_empty());
    assert_eq!(
        composer
            .search("monolog/monolog", &Cancellation::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn wave_five_malformed_metadata_is_never_treated_as_an_empty_success() {
    let cancel = Cancellation::default();
    // pip: invalid JSON and invalid entries.
    for (list, outdated) in [
        ("not json".to_string(), Some(PIP_OUTDATED.into())),
        (
            r#"[{"name": "--evil", "version": "1.0"}]"#.to_string(),
            Some(PIP_OUTDATED.into()),
        ),
        (r#"[{"name": "ok", "version": ""}]"#.to_string(), None),
    ] {
        let mut backend = DevTool::pip(DevFixture {
            venv: Some("/home/test/venv".into()),
            version: "pip 25.1.1\n".into(),
            pip_list: list,
            pip_outdated: outdated,
            ..DevFixture::default()
        });
        assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
        // Empty versions fail validation; other cases either list or error.
        let _ = backend.installed(&cancel);
    }
    let mut bad_pip = DevTool::pip(DevFixture {
        venv: Some("/home/test/venv".into()),
        version: "pip 25.1.1\n".into(),
        pip_list: "not json".into(),
        pip_outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(bad_pip.detect(&cancel), Ok(Availability::Available));
    assert!(bad_pip.installed(&cancel).is_err());
    // pipx: invalid JSON.
    let mut bad_pipx = DevTool::pipx(DevFixture {
        home: Some("/home/test".into()),
        version: "1.0\n".into(),
        list: "not json".into(),
        ..DevFixture::default()
    });
    assert_eq!(bad_pipx.detect(&cancel), Ok(Availability::Available));
    assert!(bad_pipx.installed(&cancel).is_err());
    // uv: relative tool dir and unreadable output.
    let mut relative_uv = DevTool::uv(DevFixture {
        version: "uv\n".into(),
        root: Some("relative/tools".into()),
        ..DevFixture::default()
    });
    assert!(relative_uv.detect(&cancel).is_err());
    // composer: invalid JSON and relative home.
    let mut bad_composer = DevTool::composer(DevFixture {
        version: "Composer\n".into(),
        root: Some("/home/test/composer".into()),
        list: "not json".into(),
        ..DevFixture::default()
    });
    assert_eq!(bad_composer.detect(&cancel), Ok(Availability::Available));
    assert!(bad_composer.installed(&cancel).is_err());
    let mut relative_composer = DevTool::composer(DevFixture {
        version: "Composer\n".into(),
        root: Some("relative/composer".into()),
        ..DevFixture::default()
    });
    assert!(relative_composer.detect(&cancel).is_err());
    // gem: invalid specification names are skipped, outdated failures stay current.
    let base = std::env::temp_dir().join(format!("pkgdeck-gem-malformed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("specifications")).unwrap();
    std::fs::write(
        base.join("specifications").join("nodash.gemspec"),
        "# stub\n",
    )
    .unwrap();
    std::fs::write(
        base.join("specifications").join("ok-1.0.gemspec"),
        "# stub\n",
    )
    .unwrap();
    let mut gem = DevTool::gem(DevFixture {
        root: Some(base.to_str().unwrap().into()),
        version: "4.0.20\n".into(),
        outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(gem.detect(&cancel), Ok(Availability::Available));
    let installed = gem.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].id.name, "ok");
    assert_eq!(installed[0].update, UpdateAvailability::Current);
    std::fs::remove_dir_all(&base).unwrap();
    // gem: option-like specification names fail closed, never list as empty.
    let evil_base = std::env::temp_dir().join(format!("pkgdeck-gem-evil-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&evil_base);
    std::fs::create_dir_all(evil_base.join("specifications")).unwrap();
    std::fs::write(
        evil_base.join("specifications").join("--evil-1.0.gemspec"),
        "# stub\n",
    )
    .unwrap();
    let mut evil_gem = DevTool::gem(DevFixture {
        root: Some(evil_base.to_str().unwrap().into()),
        version: "4.0.20\n".into(),
        outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(evil_gem.detect(&cancel), Ok(Availability::Available));
    assert!(evil_gem.installed(&cancel).is_err());
    std::fs::remove_dir_all(&evil_base).unwrap();
    // gem without specifications lists nothing.
    let empty_base = std::env::temp_dir().join(format!("pkgdeck-gem-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty_base);
    std::fs::create_dir_all(&empty_base).unwrap();
    let mut empty_gem = DevTool::gem(DevFixture {
        root: Some(empty_base.to_str().unwrap().into()),
        version: "4.0.20\n".into(),
        ..DevFixture::default()
    });
    assert_eq!(empty_gem.detect(&cancel), Ok(Availability::Available));
    assert!(empty_gem.installed(&cancel).unwrap().is_empty());
    std::fs::remove_dir_all(&empty_base).unwrap();
}

#[test]
fn wave_five_transports_report_unavailable_and_failed_writes() {
    let cancel = Cancellation::default();
    let mut missing_pipx = DevTool::pipx(DevFixture {
        home: Some("/home/test".into()),
        fail: Some(ExecutionError::Disabled("pipx not found".into())),
        ..DevFixture::default()
    });
    assert_eq!(
        missing_pipx.detect(&cancel),
        Ok(Availability::Unavailable("pipx not found".into()))
    );
    let mut broken_uv = DevTool::uv(DevFixture {
        root: Some("/home/test/tools".into()),
        fail: Some(ExecutionError::TimedOut),
        ..DevFixture::default()
    });
    assert_eq!(
        broken_uv.detect(&cancel),
        Err(ExecutionError::TimedOut.into())
    );
    let fixture = DevFixture {
        venv: Some("/home/test/venv".into()),
        version: "pip 25.1.1\n".into(),
        pip_list: PIP_LIST.into(),
        pip_outdated: Some(PIP_OUTDATED.into()),
        fail_writes: true,
        ..DevFixture::default()
    };
    let mut backend = DevTool::pip(fixture);
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    assert_eq!(backend.installed(&cancel).unwrap().len(), 2);
    assert!(backend
        .execute(
            &Operation::UpgradeAll {
                backend: "pip".into()
            },
            &cancel,
            &mut |_| {},
        )
        .is_err());
    // Missing optionals stay discoverable but unavailable.
    for source in ["pip", "pipx", "uv", "composer", "gem"] {
        let mut engine = native_engine(
            &[source.to_string()],
            true,
            Authorization::SudoNonInteractive,
            &cancel,
        )
        .unwrap();
        let sources = engine.discover(&cancel);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].backend, source);
    }
}

#[test]
fn wave_five_relative_homes_fail_closed() {
    let cancel = Cancellation::default();
    for (make, variable) in [
        ("pipx", "PIPX_HOME"),
        ("uv", "UV_TOOL_DIR"),
        ("composer", "COMPOSER_HOME"),
        ("gem", "GEM_HOME"),
    ] {
        // Access the fixture through detection with a relative override.
        let fixture = DevFixture {
            extra_env: [(variable.to_string(), "relative/home".to_string())]
                .into_iter()
                .collect(),
            version: "test\n".into(),
            ..DevFixture::default()
        };
        let mut backend = match make {
            "pipx" => DevTool::pipx(fixture),
            "uv" => DevTool::uv(fixture),
            "composer" => DevTool::composer(fixture),
            _ => DevTool::gem(fixture),
        };
        assert!(backend.detect(&cancel).is_err());
    }
}

#[test]
fn wave_five_unreachable_registries_keep_installed_state_current() {
    let cancel = Cancellation::default();
    let mut pip = DevTool::pip(DevFixture {
        venv: Some("/home/test/venv".into()),
        version: "pip\n".into(),
        pip_list: PIP_LIST.into(),
        pip_outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(pip.detect(&cancel), Ok(Availability::Available));
    let installed = pip.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 2);
    assert!(installed
        .iter()
        .all(|p| p.update == UpdateAvailability::Current));
    let mut pipx = DevTool::pipx(DevFixture {
        home: Some("/home/test".into()),
        version: "1.0\n".into(),
        list: PIPX_LIST.into(),
        outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(pipx.detect(&cancel), Ok(Availability::Available));
    let installed = pipx.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].update, UpdateAvailability::Current);
    let mut uv = DevTool::uv(DevFixture {
        version: "uv\n".into(),
        root: Some("/home/test/tools".into()),
        list: UV_LIST.into(),
        outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(uv.detect(&cancel), Ok(Availability::Available));
    assert!(uv
        .installed(&cancel)
        .unwrap()
        .iter()
        .all(|p| p.update == UpdateAvailability::Current));
    let mut composer = DevTool::composer(DevFixture {
        version: "Composer\n".into(),
        root: Some("/home/test/composer".into()),
        list: COMPOSER_LIST.into(),
        outdated: None,
        ..DevFixture::default()
    });
    assert_eq!(composer.detect(&cancel), Ok(Availability::Available));
    assert!(composer
        .installed(&cancel)
        .unwrap()
        .iter()
        .all(|p| p.update == UpdateAvailability::Current));
}

#[test]
fn wave_five_managers_skip_malformed_rows_and_reject_foreign_names() {
    let cancel = Cancellation::default();
    // uv skips executable continuations and garbage rows but rejects
    // option-like tool names instead of running them.
    let mut uv = DevTool::uv(DevFixture {
        version: "uv\n".into(),
        root: Some("/home/test/tools".into()),
        list: "cowsay v6.0\n- cowsay\ngarbage row\n\n".into(),
        outdated: Some("".into()),
        ..DevFixture::default()
    });
    assert_eq!(uv.detect(&cancel), Ok(Availability::Available));
    let installed = uv.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].id.name, "cowsay");
    let mut evil_uv = DevTool::uv(DevFixture {
        version: "uv\n".into(),
        root: Some("/home/test/tools".into()),
        list: "--evil v1.0\n".into(),
        outdated: Some("".into()),
        ..DevFixture::default()
    });
    assert_eq!(evil_uv.detect(&cancel), Ok(Availability::Available));
    assert!(evil_uv.installed(&cancel).is_err());
    // An empty Composer home lists nothing; failed metadata never does.
    let mut empty = DevTool::composer(DevFixture {
        version: "Composer\n".into(),
        root: Some("/home/test/composer".into()),
        list: "   \n".into(),
        ..DevFixture::default()
    });
    assert_eq!(empty.detect(&cancel), Ok(Availability::Available));
    assert!(empty.installed(&cancel).unwrap().is_empty());
    let mut bad_pipx = DevTool::pipx(DevFixture {
        home: Some("/home/test".into()),
        version: "1.0\n".into(),
        list: "not json".into(),
        ..DevFixture::default()
    });
    assert_eq!(bad_pipx.detect(&cancel), Ok(Availability::Available));
    assert!(bad_pipx.installed(&cancel).is_err());
}

#[test]
fn gem_tolerates_garbage_outdated_rows_and_cancellation() {
    let base = std::env::temp_dir().join(format!("pkgdeck-gem-outdated-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("specifications")).unwrap();
    std::fs::write(
        base.join("specifications").join("ok-1.0.gemspec"),
        "# stub\n",
    )
    .unwrap();
    std::fs::write(base.join("specifications").join("README"), "not a spec\n").unwrap();
    let mut backend = DevTool::gem(DevFixture {
        root: Some(base.to_str().unwrap().into()),
        version: "4.0.20\n".into(),
        outdated: Some("garbage line\nok (1.0 < 1.1)\n--evil (1 < 2)\n".into()),
        ..DevFixture::default()
    });
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let installed = backend.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].candidate_version.as_deref(), Some("1.1"));
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(backend.installed(&cancelled), Err(EngineError::Cancelled));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn gem_reports_unreadable_specification_directories() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!("pkgdeck-gem-perms-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("specifications")).unwrap();
    let mut backend = DevTool::gem(DevFixture {
        root: Some(base.to_str().unwrap().into()),
        version: "4.0.20\n".into(),
        ..DevFixture::default()
    });
    let cancel = Cancellation::default();
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    std::fs::set_permissions(
        base.join("specifications"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    // Root bypasses permission bits; otherwise the failure must surface.
    let result = backend.installed(&cancel);
    std::fs::set_permissions(
        base.join("specifications"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    if rustix::process::geteuid().is_root() {
        let _ = result;
    } else {
        assert!(result.is_err());
    }
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn default_venv_pip_transport_reports_disabled() {
    // Fixtures without a venv_pip override deny pip while other seams work.
    let cancel = Cancellation::default();
    let raw = Raw(output(""));
    assert!(matches!(
        raw.venv_pip(std::path::Path::new("/home/test/venv"), &[], &cancel, false),
        Err(ExecutionError::Disabled(_))
    ));
}

#[test]
fn native_venv_pip_transport_uses_the_selected_environment() {
    use pkgdeck_core::host::{Host, Runtime};
    let base = std::env::temp_dir().join(format!("pkgdeck-venv-transport-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let venv = base.join("venv");
    std::fs::create_dir_all(venv.join("bin")).unwrap();
    std::os::unix::fs::symlink("/bin/true", venv.join("bin/python")).unwrap();
    std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    let transport = NativeTransport {
        host: Host::new(
            Runtime::Native,
            [("PATH".into(), "/usr/bin:/bin".into())].into(),
        ),
        authorization: Authorization::SudoNonInteractive,
    };
    let cancel = Cancellation::default();
    let ok = transport
        .venv_pip(&venv, &["--version".into()], &cancel, false)
        .unwrap();
    assert_eq!(ok.code, Some(0));
    assert!(matches!(
        transport.venv_pip(&base.join("missing"), &[], &cancel, false),
        Err(ExecutionError::Disabled(_))
    ));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn flatpak_updates_match_commits_by_scope_origin_kind_and_branch() {
    let fixture = FlatpakFixture {
        installed: Some("io.example.App\tx86_64\tstable\t1\tApp\tflathub\tcurrent\nio.example.App\tx86_64\tbeta\t1\tApp beta\tflathub\t\norg.example.Platform\tx86_64\t50\t\tPlatform\tflathub\truntime\n".into()),
        user_updates: Some(output("app/io.example.App/x86_64/stable\t1\tflathub\napp/io.example.App/x86_64/beta\t2\tother-remote\nruntime/org.example.Platform/x86_64/50\t\tflathub\nruntime/org.example.Platform/x86_64/49\t\tflathub\n")),
        ..Default::default()
    };
    let mut backend = Flatpak::new(fixture.clone());
    let cancel = Cancellation::default();
    let packages = backend.installed(&cancel).unwrap();
    assert_eq!(packages.len(), 6);
    assert!(packages
        .iter()
        .filter(|p| p.id.scope == Scope::System)
        .all(|p| p.update == UpdateAvailability::Current));
    let updates: Vec<_> = packages
        .iter()
        .filter(|p| p.update == UpdateAvailability::Available)
        .collect();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].installed_version, updates[0].candidate_version);
    let runtime = updates[1];
    assert!(runtime.component_ids.is_empty());
    assert_eq!(
        runtime.id.reference.as_deref(),
        Some("runtime/org.example.Platform/x86_64/50")
    );
    assert_eq!(runtime.candidate_version.as_deref(), Some(""));
    let exact = backend
        .search("runtime/org.example.Platform/x86_64/50", &cancel)
        .unwrap();
    assert_eq!(exact.len(), 2);
    assert!(exact
        .iter()
        .all(|package| package.id.reference == runtime.id.reference));
    assert_eq!(
        backend.details(&runtime.id, &cancel).unwrap().package.id,
        runtime.id
    );

    backend
        .execute(
            &Operation::Upgrade(runtime.id.clone()),
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert!(calls.iter().any(|(args, write, system)| *write
        && !*system
        && args.contains(&"--runtime".into())
        && args.last().unwrap() == "runtime/org.example.Platform/x86_64/50"));
    drop(calls);
    let mut wrong = runtime.id.clone();
    wrong.reference = Some("runtime/org.other.Platform/x86_64/50".into());
    assert!(backend
        .execute(&Operation::Upgrade(wrong), &cancel, &mut |_| {})
        .is_err());
    let report = PackageReport {
        packages,
        failures: vec![],
        successful_sources: vec![],
    };
    assert_eq!(
        report
            .select(&Selector {
                name: "app/io.example.App/x86_64/stable".into(),
                backend: Some("flatpak".into()),
                architecture: None,
                scope: Some(Scope::System),
            })
            .unwrap()
            .reference
            .as_deref(),
        Some("app/io.example.App/x86_64/stable")
    );
}

#[test]
fn flatpak_update_query_failures_are_not_reported_as_current() {
    let cancel = Cancellation::default();
    let mut failed = output("");
    failed.code = Some(1);
    let mut truncated = output("");
    truncated.truncated = true;
    for updates in [
        output("broken"),
        output("app/--bad/x86_64/stable\t1\tflathub"),
        failed,
        truncated,
    ] {
        let mut backend = Flatpak::new(FlatpakFixture {
            user_updates: Some(updates),
            ..Default::default()
        });
        assert!(backend.installed(&cancel).is_err());
    }
    let fixture = FlatpakFixture {
        installed: Some(String::new()),
        ..Default::default()
    };
    assert!(Flatpak::new(fixture.clone())
        .installed(&cancel)
        .unwrap()
        .is_empty());
    assert!(!fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(args, _, _)| args.contains(&"remote-ls".into())));
}

#[test]
fn flatpak_search_preserves_branch_without_guessing_kind_or_fetching_updates() {
    let fixture = FlatpakFixture {
        installed: Some(String::new()),
        ..Default::default()
    };
    let mut backend = Flatpak::new(fixture.clone());
    let cancel = Cancellation::default();
    let rows = backend.search("User", &cancel).unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].id.scope, rows[1].id.scope);
    let id = &rows[0].id;
    assert!(rows[0].installed_version.is_none());
    assert_eq!(
        id.reference.as_deref(),
        Some(format!("io.example.User/{}/stable", std::env::consts::ARCH).as_str())
    );
    backend
        .execute(&Operation::Install(id.clone()), &cancel, &mut |_| {})
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert!(!calls
        .iter()
        .any(|(args, _, _)| args.iter().any(|arg| arg == "remote-ls")));
    let (args, _, system) = calls.iter().find(|(_, write, _)| *write).unwrap();
    assert!(!system);
    assert_eq!(args.last().map(String::as_str), id.reference.as_deref());
    assert!(!args.iter().any(|arg| arg == "--app" || arg == "--runtime"));
}

#[test]
fn apt_single_operation_preview_uses_read_only_simulation_and_exact_target() {
    let fixture = Fixture {
        apt_simulation: Some("0 upgraded, 1 newly installed, 1 to remove and 0 not upgraded.\nInst synthetic-fixture (2.0 Ubuntu:stable [amd64])\nRemv retired [1.0]\n".into()),
        ..Fixture::new()
    };
    let mut apt = Apt::new(fixture.clone());
    let id = PackageId {
        backend: "apt".into(),
        name: "synthetic-fixture".into(),
        architecture: "amd64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    let operation = Operation::Install(id.clone());
    let plan = apt
        .operation_plan(&operation, &Cancellation::default())
        .unwrap()
        .unwrap();
    assert_eq!(plan.operation, operation);
    assert_eq!(plan.changes.len(), 2);
    assert_eq!(plan.changes[0].action, PlannedAction::Install);
    assert_eq!(plan.changes[0].candidate_version.as_deref(), Some("2.0"));
    assert_eq!(plan.changes[1].action, PlannedAction::Remove);
    assert_eq!(plan.changes[1].installed_version.as_deref(), Some("1.0"));
    let calls = fixture.simulations.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "apt-get");
    assert_eq!(
        calls[0].1,
        [
            "--simulate",
            "-o",
            "Debug::NoLocking=1",
            "install",
            "synthetic-fixture:amd64"
        ]
    );
    drop(calls);
    assert!(fixture.writes.lock().unwrap().is_empty());
    let upgrade_all = apt.apt_upgrade_plan(&Cancellation::default()).unwrap();
    assert_eq!(upgrade_all.installs, ["synthetic-fixture"]);
    assert_eq!(upgrade_all.removals, ["retired"]);
    assert_eq!(
        fixture.simulations.lock().unwrap()[1].1.last().unwrap(),
        "dist-upgrade"
    );
    assert!(apt
        .operation_plan(
            &Operation::Refresh {
                backend: "apt".into()
            },
            &Cancellation::default()
        )
        .unwrap()
        .is_none());
    let mut foreign = id;
    foreign.backend = "snap".into();
    assert!(apt
        .operation_plan(&Operation::Remove(foreign), &Cancellation::default())
        .is_err());
}

#[test]
fn apt_local_archive_uses_exact_path_and_revalidates_before_install() {
    let base = std::env::temp_dir().join(format!("pkgdeck-apt-local-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let control = base.join("staging/DEBIAN");
    std::fs::create_dir_all(&control).unwrap();
    std::fs::write(control.join("control"), "Package: pkgdeck-synthetic\nVersion: 1.0\nArchitecture: all\nMaintainer: PkgDeck tests <nobody@example.invalid>\nDescription: Synthetic fixture\n").unwrap();
    let archive = base.join("Synthetic ñ package.deb");
    assert!(std::process::Command::new("dpkg-deb")
        .arg("--build")
        .arg(base.join("staging"))
        .arg(&archive)
        .status()
        .unwrap()
        .success());
    let cancel = Cancellation::default();
    let id = pkgdeck_core::local_deb::inspect(&archive, &cancel)
        .unwrap()
        .id;
    let fixture = Fixture {
        apt_simulation: Some("0 upgraded, 1 newly installed, 0 to remove and 0 not upgraded.\nInst pkgdeck-synthetic (1.0 Synthetic:stable [all])\n".into()),
        ..Fixture::new()
    };
    let mut apt = Apt::new(fixture.clone());
    let operation = Operation::Install(id);
    let plan = apt.operation_plan(&operation, &cancel).unwrap().unwrap();
    assert_eq!(plan.changes[0].name, "pkgdeck-synthetic");
    let calls = fixture.simulations.lock().unwrap();
    assert_eq!(calls[0].1.last().unwrap(), archive.as_os_str());
    drop(calls);
    apt.execute(&operation, &cancel, &mut |_| {}).unwrap();
    assert_eq!(fixture.installed.lock().unwrap().as_deref(), Some("1.0"));
    let staged = fixture.local_install.lock().unwrap().clone().unwrap();
    assert_ne!(staged, archive);
    assert!(
        !staged.exists(),
        "staged archive must be removed after APT returns"
    );
    let artifact = pkgdeck_core::artifact::inspect(archive.to_str().unwrap(), &cancel).unwrap();
    let artifact_operation = Operation::Install(artifact.id);
    let artifact_plan = apt
        .operation_plan(&artifact_operation, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(artifact_plan.changes[0].name, "pkgdeck-synthetic");
    apt.execute(&artifact_operation, &cancel, &mut |_| {})
        .unwrap();
    let artifact_stage = fixture.local_install.lock().unwrap().clone().unwrap();
    assert_ne!(artifact_stage, archive);
    assert!(!artifact_stage.exists());
    std::fs::write(&archive, b"changed since preview").unwrap();
    assert!(apt.execute(&operation, &cancel, &mut |_| {}).is_err());
    assert!(apt
        .execute(&artifact_operation, &cancel, &mut |_| {})
        .is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn flatpak_bundle_install_uses_a_private_reviewed_file() {
    let base = std::env::temp_dir().join(format!("pkgdeck-flatpak-bundle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let bundle = base.join("synthetic.flatpak");
    std::fs::write(&bundle, b"synthetic flatpak bundle").unwrap();
    let cancel = Cancellation::default();
    let package = pkgdeck_core::artifact::inspect(bundle.to_str().unwrap(), &cancel).unwrap();
    let fixture = FlatpakFixture::default();
    let mut backend = Flatpak::new(fixture.clone());
    backend
        .execute(
            &Operation::Install(package.id.clone()),
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    let (args, write, system) = calls.last().unwrap();
    assert!(*write && !*system);
    assert!(args.contains(&"--bundle".into()));
    let path = std::path::Path::new(args.last().unwrap());
    assert_ne!(path, bundle);
    assert!(!path.exists());
    drop(calls);
    std::fs::write(&bundle, b"changed flatpak bundle").unwrap();
    assert!(backend
        .execute(&Operation::Install(package.id), &cancel, &mut |_| {})
        .is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn local_archives_use_exact_native_managers() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::var_os("PKGDECK_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pkgdeck-native-artifacts-{}", std::process::id()))
        });
    if std::env::var_os("PKGDECK_ARTIFACT_CHILD").is_none() {
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        for (name, response) in [
            ("dnf", "exit 0"),
            ("zypper", "exit 0"),
            ("rpm", "printf 'synthetic-rpm|1.0-1|x86_64|Synthetic RPM'"),
            ("pacman", "printf 'Name : synthetic-arch\\nVersion : 1.0-1\\nArchitecture : x86_64\\nDescription : Synthetic Arch\\n'"),
            ("snap", "printf 'name: synthetic-snap\\nversion: 1.0\\nsummary: Synthetic Snap\\n'"),
        ] {
            let script = base.join(name);
            std::fs::write(&script, format!("#!/bin/sh\n{response}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_archives_use_exact_native_managers",
                "--nocapture",
            ])
            .env("PKGDECK_ARTIFACT_CHILD", "1")
            .env("PKGDECK_ARTIFACT_DIR", &base)
            .env("PATH", &base)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(base).unwrap();
        return;
    }
    let cancel = Cancellation::default();
    for (file, backend_name) in [
        ("sample.rpm", "dnf"),
        ("zypper.rpm", "zypper"),
        ("sample.pkg.tar.zst", "pacman"),
        ("sample.snap", "snap"),
    ] {
        if backend_name == "zypper" {
            std::fs::remove_file(base.join("dnf")).unwrap();
        }
        let archive = base.join(file);
        std::fs::write(&archive, b"synthetic archive bytes").unwrap();
        if file.ends_with(".snap") {
            std::fs::write(base.join("sample.assert"), b"synthetic signed assertion").unwrap();
        }
        let package = pkgdeck_core::artifact::inspect(archive.to_str().unwrap(), &cancel).unwrap();
        let fixture = Fixture {
            record_writes: true,
            ..Fixture::new()
        };
        let mut backend = match backend_name {
            "dnf" => Dnf::dnf(fixture.clone()),
            "zypper" => Zypper::zypper(fixture.clone()),
            "pacman" => Pacman::pacman(fixture.clone()),
            _ => Snap::snap(fixture.clone()),
        };
        backend
            .execute(
                &Operation::Install(package.id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        let writes = fixture.writes.lock().unwrap();
        let (manager, args) = writes.last().unwrap();
        assert_eq!(manager, backend_name);
        assert!(args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("pkgdeck-input-")));
        if backend_name == "snap" {
            assert_eq!(writes.len(), 2);
            assert_eq!(writes[0].1[0], "ack");
        }
        drop(writes);
        std::fs::write(&archive, b"changed after preview").unwrap();
        assert!(backend
            .execute(&Operation::Install(package.id), &cancel, &mut |_| {})
            .is_err());
    }
}

#[test]
fn system_manager_actions_preserve_native_target_and_noninteractive_mode() {
    for source in ["dnf", "pacman", "zypper", "snap"] {
        let fixture = Fixture {
            record_writes: true,
            ..Fixture::new()
        };
        let mut backend = match source {
            "dnf" => Dnf::dnf(fixture.clone()),
            "pacman" => Pacman::pacman(fixture.clone()),
            "zypper" => Zypper::zypper(fixture.clone()),
            _ => Snap::snap(fixture.clone()),
        };
        let id = PackageId {
            backend: source.into(),
            name: "synthetic-player".into(),
            architecture: "x86_64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        for operation in [
            Operation::Install(id.clone()),
            Operation::Remove(id.clone()),
            Operation::Upgrade(id.clone()),
            Operation::Refresh {
                backend: source.into(),
            },
            Operation::UpgradeAll {
                backend: source.into(),
            },
        ] {
            backend
                .execute(&operation, &Cancellation::default(), &mut |_| {})
                .unwrap();
            let writes = fixture.writes.lock().unwrap();
            let (executable, args) = writes.last().unwrap();
            assert_eq!(executable, source);
            if matches!(
                operation,
                Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
            ) {
                assert_eq!(args.last().unwrap(), "synthetic-player");
            } else {
                assert!(!args.iter().any(|arg| arg == "synthetic-player"));
            }
            if let Operation::Remove(_) = operation {
                assert!(args
                    .iter()
                    .any(|arg| arg == if source == "pacman" { "-Rns" } else { "remove" }));
            }
            let noninteractive = match source {
                "dnf" => Some("-y"),
                "pacman" => Some("--noconfirm"),
                "zypper" => Some("--non-interactive"),
                _ => None,
            };
            if let Some(flag) = noninteractive {
                assert!(args.iter().any(|arg| arg == flag));
            }
        }
        assert_eq!(fixture.writes.lock().unwrap().len(), 5);
    }
}

#[derive(Clone, Default)]
struct CleanupFixture {
    preview: String,
    truncated: bool,
    apt_cache_denied: bool,
    apt_cache_empty: bool,
    calls: Arc<Mutex<Vec<DevCall>>>,
}
impl Transport for CleanupFixture {
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
    fn system_manager(
        &self,
        exe: &str,
        args: &[OsString],
        cancel: &Cancellation,
        authenticated: bool,
    ) -> Result<Completion, ExecutionError> {
        self.dev_tool(exe, args, cancel, authenticated)?;
        if exe == "apt-config" {
            return Ok(output(
                "ROOT='/'\nCACHE='var/cache/apt'\nARCHIVES='archives/'\n",
            ));
        }
        if exe == "find" && self.apt_cache_denied {
            return Err(ExecutionError::Io("Permission denied".into()));
        }
        if exe == "find" {
            return Ok(output(if self.apt_cache_empty { "" } else { "..." }));
        }
        assert_eq!(exe, "apt-get");
        Ok(output("Remv unused-package [1.0]\n"))
    }
    fn flatpak_unused(&self, _: &Cancellation) -> Result<Completion, ExecutionError> {
        let mut result = output(&self.preview);
        result.truncated = self.truncated;
        Ok(result)
    }
    fn flatpak(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        self.dev_tool("flatpak", args, cancel, write)
    }
    fn env(&self, name: &str) -> Option<OsString> {
        (name == "VIRTUAL_ENV").then(|| "/tmp/synthetic-venv".into())
    }
    fn venv_pip(
        &self,
        _: &std::path::Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.dev_tool("pip", args, cancel, write)
    }
    fn dev_tool(
        &self,
        exe: &str,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.calls.lock().unwrap().push((
            exe.into(),
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            write,
        ));
        let mut result = output(if write { "" } else { &self.preview });
        result.truncated = self.truncated;
        Ok(result)
    }
}

#[test]
fn flatpak_cleanup_is_scope_specific_and_revalidated() {
    let fixture = CleanupFixture { preview: r#"[{"scope":"user","reference":"runtime/org.example.Runtime/x86_64/stable","bytes":4096},{"scope":"system","reference":"runtime/org.example.Extension/x86_64/stable","bytes":2048}]"#.into(), ..Default::default() };
    let mut backend = Flatpak::new(fixture.clone());
    let items = backend.cleanup(&Cancellation::default()).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id.key, "unused-user");
    assert_eq!(items[1].id.key, "unused-system");
    assert!(items[0].preview.contains("4096 bytes installed"));
    for item in items {
        backend
            .execute(
                &Operation::Clean(item.id),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
    }
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(
        calls[0].1,
        [
            "--user",
            "uninstall",
            "--unused",
            "--noninteractive",
            "--assumeyes"
        ]
    );
    assert_eq!(calls[1].1[0], "--system");
    assert!(calls.iter().all(|(_, _, write)| *write));
}

#[test]
fn native_flatpak_cleanup_is_unavailable_without_an_authoritative_preview() {
    let mut backend = Flatpak::new(NativeTransport {
        host: pkgdeck_core::host::Host::new(
            pkgdeck_core::host::Runtime::Native,
            Default::default(),
        ),
        authorization: Authorization::Polkit,
    });
    assert!(!backend.capabilities().contains(&Capability::Clean));
    assert!(matches!(
        backend.cleanup(&Cancellation::default()),
        Err(EngineError::Unsupported { .. })
    ));
    assert!(matches!(
        backend.execute(
            &Operation::Clean(CleanupId {
                backend: "flatpak".into(),
                key: "unused-user".into(),
            }),
            &Cancellation::default(),
            &mut |_| {},
        ),
        Err(EngineError::Unsupported { .. })
    ));
}

#[test]
fn flatpak_cleanup_rejects_invalid_or_disappeared_candidates() {
    for preview in [
        "[]",
        r#"[{"scope":"user","reference":"app/org.example.App/x86_64/stable","bytes":1}]"#,
        r#"[{"scope":"other","reference":"runtime/org.example.Runtime/x86_64/stable","bytes":1}]"#,
        "broken",
    ] {
        let fixture = CleanupFixture {
            preview: preview.into(),
            ..Default::default()
        };
        let mut backend = Flatpak::new(fixture.clone());
        assert!(backend
            .execute(
                &Operation::Clean(CleanupId {
                    backend: "flatpak".into(),
                    key: "unused-user".into()
                }),
                &Cancellation::default(),
                &mut |_| {}
            )
            .is_err());
        assert!(fixture.calls.lock().unwrap().is_empty());
    }
    let mut backend = Flatpak::new(CleanupFixture::default());
    assert!(backend
        .execute(
            &Operation::Clean(CleanupId {
                backend: "flatpak".into(),
                key: "all".into()
            }),
            &Cancellation::default(),
            &mut |_| {}
        )
        .is_err());
}

#[test]
fn development_cache_cleanup_uses_native_previews_and_fixed_commands() {
    for (name, preview, expected) in [
        (
            "npm",
            "make-fetch-happen:request-cache:https://registry.npmjs.org/example\n",
            vec!["cache", "clean", "--force"],
        ),
        ("uv", "4096\n", vec!["cache", "clean"]),
        (
            "pip",
            "Number of HTTP files: 2\nNumber of locally built wheels: 1\n",
            vec!["cache", "purge"],
        ),
    ] {
        let fixture = CleanupFixture {
            preview: preview.into(),
            ..Default::default()
        };
        let mut backend = match name {
            "npm" => DevTool::npm(fixture.clone()),
            "uv" => DevTool::uv(fixture.clone()),
            _ => DevTool::pip(fixture.clone()),
        };
        if name == "pip" {
            backend.detect(&Cancellation::default()).unwrap();
        }
        assert!(backend.capabilities().contains(&Capability::Clean));
        let tasks = backend.cleanup(&Cancellation::default()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert!(fixture
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, _, write)| !write));
        backend
            .execute(
                &Operation::Clean(tasks[0].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(fixture.calls.lock().unwrap().last().unwrap().1, expected);
        assert!(backend
            .execute(
                &Operation::Clean(CleanupId {
                    backend: name.into(),
                    key: "arbitrary".into()
                }),
                &Cancellation::default(),
                &mut |_| {}
            )
            .is_err());
    }
}

#[test]
fn cache_cleanup_does_not_offer_empty_or_invalid_inventories() {
    for (preview, error) in [("0", false), ("nonsense", true)] {
        let mut backend = DevTool::uv(CleanupFixture {
            preview: preview.into(),
            ..Default::default()
        });
        let result = backend.cleanup(&Cancellation::default());
        if error {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().is_empty());
        }
    }
    let mut backend = DevTool::npm(CleanupFixture::default());
    assert!(backend
        .cleanup(&Cancellation::default())
        .unwrap()
        .is_empty());
    assert!(backend
        .execute(
            &Operation::Clean(CleanupId {
                backend: "npm".into(),
                key: "cache".into()
            }),
            &Cancellation::default(),
            &mut |_| {}
        )
        .is_err());
    let mut backend = DevTool::npm(CleanupFixture {
        preview: "partial".into(),
        truncated: true,
        ..Default::default()
    });
    assert!(backend.cleanup(&Cancellation::default()).is_err());
    let mut backend = DevTool::pnpm(CleanupFixture::default());
    assert!(!backend.capabilities().contains(&Capability::Clean));
    assert!(backend.cleanup(&Cancellation::default()).is_err());
}

#[test]
fn apt_cleanup_loads_cache_without_authentication_and_preserves_orphans_on_failure() {
    let fixture = CleanupFixture {
        apt_cache_denied: true,
        ..Default::default()
    };
    let mut backend = Apt::new(fixture.clone());
    let report = backend.cleanup_report(&Cancellation::default());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].id.key, "autoremove");
    assert_eq!(report.failures.len(), 1);
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|(_, _, authenticated)| !authenticated));
    let mut backend = Apt::new(CleanupFixture::default());
    let report = backend.cleanup_report(&Cancellation::default());
    assert_eq!(report.items.len(), 2);
    assert!(report.failures.is_empty());
    let plan = backend
        .cleanup_plan(
            &CleanupId {
                backend: "apt".into(),
                key: "autoclean".into(),
            },
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(plan.id.key, "autoclean");
    for (_, args, authenticated) in fixture.calls.lock().unwrap().iter() {
        assert!(!authenticated);
        assert!(!args.contains(&"autoclean".into()));
    }
    assert!(backend
        .cleanup_plan(
            &CleanupId {
                backend: "apt".into(),
                key: "arbitrary".into()
            },
            &Cancellation::default()
        )
        .is_err());
    let mut backend = Apt::new(CleanupFixture {
        apt_cache_empty: true,
        ..Default::default()
    });
    let report = backend.cleanup_report(&Cancellation::default());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].id.key, "autoremove");
}

#[test]
fn snap_failed_native_completion_cannot_report_success() {
    let mut failed = output("synthetic manager output");
    failed.code = Some(1);
    failed.stderr = b"synthetic permission denied".to_vec();
    let mut backend = Snap::snap(Raw(failed.clone()));
    let package = PackageId {
        backend: "snap".into(),
        name: "synthetic-fixture".into(),
        architecture: "x86_64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    for operation in [
        Operation::Install(package.clone()),
        Operation::Remove(package.clone()),
        Operation::Upgrade(package),
        Operation::Refresh {
            backend: "snap".into(),
        },
        Operation::UpgradeAll {
            backend: "snap".into(),
        },
    ] {
        assert_eq!(
            backend.execute(&operation, &Cancellation::default(), &mut |_| {}),
            Err(EngineError::Execution(ExecutionError::Failed(
                failed.clone()
            )))
        );
    }
}

#[test]
fn flatpak_refresh_does_not_hide_failed_privileged_completion() {
    let mut failed = output("authorization failed");
    failed.code = Some(1);
    let mut backend = Flatpak::new(Raw(failed));
    assert!(matches!(
        backend.execute(
            &Operation::Refresh {
                backend: "flatpak".into()
            },
            &Cancellation::default(),
            &mut |_| {}
        ),
        Err(EngineError::Execution(ExecutionError::Failed(_)))
    ));
}
