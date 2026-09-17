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
    for source in ["appimage", "flatpak", "dnf", "pacman", "zypper", "snap"] {
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
