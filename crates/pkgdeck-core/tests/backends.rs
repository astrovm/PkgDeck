use pkgdeck_core::{backends::*, engine::*, host::AptAction, package::*, process::*};
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
            AptAction::Install(_) | AptAction::Upgrade(_) => {
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
    assert_eq!(
        backend.installed(&cancel).unwrap()[0].update,
        UpdateAvailability::Available
    );
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
