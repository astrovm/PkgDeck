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

#[derive(Clone, Default)]
struct Fixture {
    installed: Arc<Mutex<Option<String>>>,
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
        match action {
            AptAction::Refresh => *self.candidate.lock().unwrap() = "2.0".into(),
            AptAction::Install(_) | AptAction::Upgrade(_) | AptAction::UpgradeAll => {
                *self.installed.lock().unwrap() = Some(self.candidate.lock().unwrap().clone())
            }
            AptAction::Remove(_) => *self.installed.lock().unwrap() = None,
        }
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
                _ => panic!("unexpected native operation"),
            }
            return Ok(output(""));
        }
        match args[0] {
            "--prefix" => Ok(output("/home/linuxbrew/.linuxbrew\n")),
            "formulae" => Ok(output("synthetic-fixture\n")),
            "info" => {
                let installed = self.installed.lock().unwrap().clone();
                let candidate = self.candidate.lock().unwrap().clone();
                let formula = json!({"full_name":"synthetic-fixture","desc":"Synthetic package","homepage":"https://example.invalid","versions":{"stable":candidate},"revision":0,"installed":installed.iter().map(|v| json!({"version":v})).collect::<Vec<_>>(),"outdated":installed.as_ref().is_some_and(|v| v != &candidate),"dependencies":[]});
                Ok(output(json!({"formulae": if args.contains(&"--installed") && installed.is_none() { vec![] } else { vec![formula] }}).to_string()))
            }
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
        assert_eq!(executable, "dnf");
        let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
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
    assert_eq!(backend.capabilities().len(), 7);
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
    lifecycle(Apt(Fixture::new()));
}
#[test]
fn homebrew_lifecycle() {
    lifecycle(Homebrew::new(Fixture::new()));
}
#[test]
fn dnf_lifecycle() {
    lifecycle(Dnf::dnf(Fixture::new()));
}

#[test]
fn wave_three_parsers_preserve_system_identities() {
    let cancel = Cancellation::default();
    let mut pacman = Pacman::pacman(Raw(output(
        "core/synthetic-fixture 1.0\nSynthetic package\n",
    )));
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
fn flatpak_lists_user_and_system_applications_without_collapsing_scope() {
    let cancel = Cancellation::default();
    let metadata = "io.example.User\tx86_64\tstable\t1.0\tUser app\nio.example.System\tx86_64\tstable\t2.0\tSystem app\n";
    let mut backend = Flatpak::new(Raw(output(metadata)));
    assert_eq!(backend.detect(&cancel), Ok(Availability::Available));
    let packages = backend.installed(&cancel).unwrap();
    assert_eq!(packages.len(), 4);
    assert!(packages
        .iter()
        .any(|package| package.id.scope == Scope::System));
    assert!(packages
        .iter()
        .any(|package| matches!(package.id.scope, Scope::User { .. })));
}

type FlatpakCall = (Vec<String>, bool, bool);

#[derive(Clone, Default)]
struct FlatpakFixture(Arc<Mutex<Vec<FlatpakCall>>>);
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
        self.0.lock().unwrap().push((args.clone(), write, system));
        if args.contains(&"list".into()) {
            let name = if system {
                "io.example.System"
            } else {
                "io.example.User"
            };
            return Ok(output(format!(
                "{name}\tx86_64\tstable\t1.0\tSynthetic app\n"
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
    assert_eq!(backend.search("io.example.User", &cancel).unwrap().len(), 1);
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
    let calls = fixture.0.lock().unwrap();
    assert!(calls
        .iter()
        .any(|(args, write, system)| *write && !*system && args.contains(&"flathub".into())));
    assert!(calls.iter().any(|(args, write, system)| *write
        && *system
        && args.first().is_some_and(|scope| scope == "--system")));
}

#[test]
fn flatpak_rejects_malformed_metadata_and_foreign_operations() {
    let cancel = Cancellation::default();
    let mut backend = Flatpak::new(Raw(output("bad\tmetadata\n")));
    assert!(backend.installed(&cancel).is_err());
    let foreign = PackageId {
        backend: "flatpak".into(),
        name: "io.example.App".into(),
        architecture: "x86_64".into(),
        scope: Scope::Environment {
            path: "/synthetic".into(),
        },
        remote: None,
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
        "appimage", "flatpak", "dnf", "pacman", "zypper", "snap", "cargo", "npm", "pnpm", "bun",
    ] {
        let mut engine = native_engine(
            Some(source),
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
            Apt(fixture.clone()).detect(&Cancellation::default()),
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
        assert!(Apt(Raw(result.clone())).search("fixture", &cancel).is_err());
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
        host: Host::new(Runtime::Snap, Default::default()),
        authorization: Authorization::Polkit,
    };
    assert!(sandbox.apt_query("detect", "", "", &cancel).is_err());
    assert!(sandbox.apt_write(AptAction::Refresh, &cancel).is_err());
    assert!(sandbox.brew(&[], &cancel, true).is_err());
    assert!(matches!(
        native_engine(Some("foreign"), false, Authorization::Polkit, &cancel),
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
    // The user and system catalog queries return the same remote entry;
    // it must collapse to one identity so engine selection is unambiguous.
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].id.scope, Scope::User { .. }));
    assert!(results
        .iter()
        .all(|p| p.id.remote.as_deref() == Some("flathub")));
    let id = results[0].id.clone();
    assert!(id.remote.is_some());
    assert_eq!(backend.details(&id, &cancel).unwrap().package.id, id);
    let mut missing = id.clone();
    missing.name = "io.example.Missing".into();
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
    let calls = fixture.0.lock().unwrap();
    assert!(calls
        .iter()
        .any(|(args, write, system)| *write && !*system && args.contains(&"update".into())));
    assert!(calls
        .iter()
        .any(|(args, write, system)| *write && *system && args.contains(&"update".into())));
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
fn flatpak_detect_reports_unavailable_backend() {
    let cancel = Cancellation::default();
    let mut backend = Flatpak::new(FailingFlatpak(ExecutionError::Disabled("missing".into())));
    assert_eq!(
        backend.detect(&cancel),
        Ok(Availability::Unavailable("missing".into()))
    );
}

#[test]
fn flatpak_remote_search_resolves_to_a_single_identity() {
    // Mirrors `pkd --from flatpak info <app>`: engine selection must see one
    // identity, not Ambiguous user/system duplicates, and details must follow.
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
            scope: None,
        })
        .unwrap();
    assert!(id.remote.is_some());
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
            _: &[OsString],
            _: &Cancellation,
            _: bool,
            system: bool,
        ) -> Result<Completion, ExecutionError> {
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
        if name == "HOME" {
            self.home.clone().map(OsString::from)
        } else {
            None
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
        "update".into(),
        "--global".into(),
        "typescript".into()
    ]));
    assert!(writes.contains(&vec![
        "remove".into(),
        "--global".into(),
        "typescript".into()
    ]));
    assert!(writes.contains(&vec!["update".into(), "--global".into()]));
}

#[test]
fn pnpm_outdated_newer_reports_available() {
    let fixture = DevFixture {
        version: "10.0.0\n".into(),
        root: Some("/home/test/.local/share/pnpm/global/9".into()),
        list: PNPM_LIST.into(),
        outdated: Some(
            r#"{"typescript": {"current": "7.0.2", "wanted": "7.1.0", "latest": "8.0.0"}}"#.into(),
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
    assert!(writes.contains(&vec!["update".into(), "--global".into(), "alpha".into()]));
    assert!(writes.contains(&vec!["remove".into(), "--global".into(), "alpha".into()]));
    assert!(writes.contains(&vec!["update".into(), "--global".into()]));
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
