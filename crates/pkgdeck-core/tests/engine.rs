use pkgdeck_core::{
    engine::*,
    package::*,
    process::{Cancellation, ExecutionError},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

const ALL: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Refresh,
    Capability::Upgrade,
];
const ALL_WITH_CLEAN: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Refresh,
    Capability::Upgrade,
    Capability::Clean,
];
fn id(backend: &str) -> PackageId {
    PackageId {
        backend: backend.into(),
        name: "fixture-tool".into(),
        architecture: "x86_64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    }
}
fn package(id: PackageId) -> Package {
    Package {
        display_name: "Synthetic tool".into(),
        summary: "Fixture only".into(),
        id,
        installed_version: None,
        candidate_version: Some("1.0".into()),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
    }
}
fn selector() -> Selector {
    Selector {
        name: "fixture-tool".into(),
        backend: None,
        architecture: None,
        scope: None,
    }
}
#[derive(Clone, Copy)]
enum Fault {
    None,
    Unavailable,
    Detect,
    Query,
    WrongIdentity,
    Duplicate,
    MissingInstalled,
    WrongDetails,
    Details,
    CancelDetect,
    Write,
    CancelWrite,
}
struct Synthetic {
    name: String,
    capabilities: &'static [Capability],
    packages: BTreeMap<PackageId, Package>,
    fault: Fault,
    calls: Arc<AtomicUsize>,
}
impl Synthetic {
    fn new(name: &str, fault: Fault) -> Self {
        let package = package(id(name));
        Self {
            name: name.into(),
            capabilities: ALL,
            packages: [(package.id.clone(), package)].into(),
            fault,
            calls: Arc::default(),
        }
    }
}
impl Backend for Synthetic {
    fn id(&self) -> &str {
        &self.name
    }
    fn capabilities(&self) -> &[Capability] {
        self.capabilities
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        match self.fault {
            Fault::Unavailable => Ok(Availability::Unavailable(
                "synthetic executable missing".into(),
            )),
            Fault::Detect => Err(ExecutionError::TimedOut.into()),
            Fault::CancelDetect => {
                cancel.cancel();
                Ok(Availability::Available)
            }
            _ => Ok(Availability::Available),
        }
    }
    fn search(&mut self, query: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if matches!(self.fault, Fault::Query) {
            return Err(ExecutionError::Io("synthetic read failure".into()).into());
        }
        let mut packages: Vec<_> = self
            .packages
            .values()
            .filter(|p| p.id.name.contains(query))
            .cloned()
            .collect();
        if !packages.is_empty() {
            match self.fault {
                Fault::WrongIdentity => packages[0].id.backend = "foreign".into(),
                Fault::Duplicate => packages.push(packages[0].clone()),
                _ => (),
            }
        }
        Ok(packages)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .search("", cancel)?
            .into_iter()
            .filter(|p| {
                p.installed_version.is_some() || matches!(self.fault, Fault::MissingInstalled)
            })
            .collect())
    }
    fn details(&mut self, id: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        if matches!(self.fault, Fault::Details) {
            return Err(EngineError::NotFound);
        }
        let mut package = self.packages.get(id).ok_or(EngineError::NotFound)?.clone();
        if matches!(self.fault, Fault::WrongDetails) {
            package.id.scope = Scope::User { uid: 1234 };
        }
        Ok(PackageDetails {
            package,
            description: "Synthetic package details".into(),
            homepage: Some("https://example.invalid/fixture".into()),
            dependencies: vec!["fixture-runtime".into()],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        progress(Progress::Message("Preparing synthetic operation".into()));
        if matches!(self.fault, Fault::Write) {
            return Err(ExecutionError::LockBusy.into());
        }
        match operation {
            Operation::Refresh { .. } => {
                for package in self.packages.values_mut() {
                    package.candidate_version = Some("2.0".into());
                    package.update = UpdateAvailability::Available;
                }
            }
            Operation::Install(id) | Operation::Upgrade(id) => {
                let package = self.packages.get_mut(id).ok_or(EngineError::NotFound)?;
                package.installed_version = package.candidate_version.clone();
                package.update = UpdateAvailability::Current;
            }
            Operation::UpgradeAll { backend } => {
                if backend != self.id() {
                    return Err(EngineError::NotFound);
                }
                for package in self.packages.values_mut() {
                    package.installed_version = package.candidate_version.clone();
                    package.update = UpdateAvailability::Current;
                }
            }
            Operation::Remove(id) => {
                let package = self.packages.get_mut(id).ok_or(EngineError::NotFound)?;
                package.installed_version = None;
                package.update = UpdateAvailability::Unknown;
            }
            Operation::Clean(_) => return Err(self.unsupported(Capability::Clean)),
        }
        progress(Progress::Transfer {
            completed: 1,
            total: Some(1),
        });
        if matches!(self.fault, Fault::CancelWrite) {
            cancel.cancel();
        }
        Ok(OperationOutcome {
            cancellation_deferred: cancel.requested(),
        })
    }
}
fn engine(fault: Fault) -> Engine {
    let mut engine = Engine::default();
    engine.register(Synthetic::new("synthetic", fault)).unwrap();
    engine
}

struct Cleaner;
impl Backend for Cleaner {
    fn id(&self) -> &str {
        "cleaner"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Clean]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Ok(vec![CleanupItem {
            id: CleanupId {
                backend: "cleaner".into(),
                key: "orphans".into(),
            },
            kind: CleanupKind::OrphanDependencies,
            title: "Synthetic orphans".into(),
            summary: "One synthetic dependency".into(),
            preview: "synthetic-runtime".into(),
        }])
    }
    fn execute(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        match operation {
            Operation::Clean(id) if id.backend == "cleaner" && id.key == "orphans" => {
                Ok(OperationOutcome::default())
            }
            _ => Err(EngineError::NotFound),
        }
    }
}

struct BrokenCleaner {
    name: &'static str,
    result: Result<Vec<CleanupItem>, EngineError>,
}
impl Backend for BrokenCleaner {
    fn id(&self) -> &str {
        self.name
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Clean]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        self.result.clone()
    }
}

fn cleanup_item(backend: &str, key: &str, title: &str) -> CleanupItem {
    CleanupItem {
        id: CleanupId {
            backend: backend.into(),
            key: key.into(),
        },
        kind: CleanupKind::PackageCache,
        title: title.into(),
        summary: "Synthetic cache".into(),
        preview: "/tmp/synthetic-cache".into(),
    }
}

#[test]
fn cleanup_discovery_omits_unsupported_sources() {
    let mut engine = engine(Fault::None);
    engine.register(Cleaner).unwrap();
    let cancel = Cancellation::default();
    let report = engine.cleanup(&cancel);
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].id.key, "orphans");
    assert!(report.failures.is_empty());
    assert!(engine
        .execute(
            &Operation::Clean(report.items[0].id.clone()),
            &cancel,
            &mut |_| {}
        )
        .is_ok());
}

#[test]
fn cleanup_rejects_invalid_plans_and_preserves_backend_failures() {
    for (name, result) in [
        (
            "foreign",
            Ok(vec![cleanup_item("other", "cache", "Foreign cache")]),
        ),
        (
            "duplicate",
            Ok(vec![
                cleanup_item("duplicate", "cache", "Cache"),
                cleanup_item("duplicate", "cache", "Cache again"),
            ]),
        ),
        ("incomplete", Ok(vec![cleanup_item("incomplete", "", "")])),
        ("failed", Err(ExecutionError::TimedOut.into())),
    ] {
        let mut engine = Engine::default();
        engine
            .register(BrokenCleaner { name, result })
            .expect("unique synthetic backend");
        let report = engine.cleanup(&Cancellation::default());
        assert!(report.items.is_empty());
        assert_eq!(report.failures.len(), 1);
        if name == "failed" {
            assert_eq!(
                report.failures[0].error,
                EngineError::Execution(ExecutionError::TimedOut)
            );
        } else {
            assert!(matches!(
                report.failures[0].error,
                EngineError::InvalidResponse { .. }
            ));
        }
    }
}

#[test]
fn lifecycle_keeps_refresh_separate_from_upgrade_and_emits_ordered_events() {
    let mut engine = engine(Fault::None);
    let cancel = Cancellation::default();
    assert_eq!(
        engine.discover(&cancel)[0].availability,
        Ok(Availability::Available)
    );
    let report = engine.search("fixture", &cancel);
    assert!(report.failures.is_empty());
    let selected = report.select(&selector()).unwrap();
    assert_eq!(selected, id("synthetic"));
    assert!(engine.installed(&cancel).packages.is_empty());
    assert_eq!(
        engine.details(&selected, &cancel).unwrap().dependencies,
        ["fixture-runtime"]
    );
    let mut events = Vec::new();
    let install = Operation::Install(selected.clone());
    assert_eq!(
        engine.execute(&install, &cancel, &mut |e| events.push(e)),
        Ok(OperationOutcome::default())
    );
    assert_eq!(
        events,
        [
            Event::Started(install.clone()),
            Event::Progress {
                operation: install.clone(),
                progress: Progress::Message("Preparing synthetic operation".into())
            },
            Event::Progress {
                operation: install.clone(),
                progress: Progress::Transfer {
                    completed: 1,
                    total: Some(1)
                }
            },
            Event::Finished {
                operation: install,
                result: Ok(OperationOutcome::default())
            }
        ]
    );
    assert_eq!(
        engine.installed(&cancel).packages[0]
            .installed_version
            .as_deref(),
        Some("1.0")
    );
    engine
        .execute(
            &Operation::Refresh {
                backend: "synthetic".into(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let details = engine.details(&selected, &cancel).unwrap();
    assert_eq!(details.package.installed_version.as_deref(), Some("1.0"));
    assert_eq!(details.package.candidate_version.as_deref(), Some("2.0"));
    assert_eq!(details.package.update, UpdateAvailability::Available);
    engine
        .execute(&Operation::Upgrade(selected.clone()), &cancel, &mut |_| {})
        .unwrap();
    assert_eq!(
        engine.installed(&cancel).packages[0]
            .installed_version
            .as_deref(),
        Some("2.0")
    );
    engine
        .execute(&Operation::Remove(selected), &cancel, &mut |_| {})
        .unwrap();
    assert!(engine.installed(&cancel).packages.is_empty());
}

#[test]
fn matching_names_require_explicit_source_architecture_and_environment_selection() {
    let mut engine = engine(Fault::None);
    engine
        .register(Synthetic::new("other", Fault::None))
        .unwrap();
    let report = engine.search("fixture", &Cancellation::default());
    assert!(matches!(report.select(&selector()), Err(EngineError::Ambiguous(ids)) if ids.len()==2));
    let mut select = selector();
    select.backend = Some("synthetic".into());
    assert_eq!(report.select(&select).unwrap(), id("synthetic"));
    select.name = "Synthetic tool".into();
    assert_eq!(report.select(&select), Err(EngineError::NotFound));
    select.name = "fixture-tool".into();
    let mut variants = report;
    let mut arm = package(id("synthetic"));
    arm.id.architecture = "aarch64".into();
    let mut user = package(id("synthetic"));
    user.id.scope = Scope::User { uid: 42 };
    let mut environment = package(id("synthetic"));
    environment.id.scope = Scope::Environment {
        path: "/synthetic/venv".into(),
    };
    variants.packages.extend([arm, user, environment.clone()]);
    assert!(matches!(
        variants.select(&select),
        Err(EngineError::Ambiguous(_))
    ));
    select.architecture = Some("x86_64".into());
    select.scope = Some(environment.id.scope.clone());
    assert_eq!(variants.select(&select).unwrap(), environment.id);
    select.scope = Some(Scope::User { uid: 43 });
    assert_eq!(variants.select(&select), Err(EngineError::NotFound));
}

#[test]
fn partial_queries_preserve_successes_but_cannot_resolve_unseen_ambiguity() {
    let mut engine = engine(Fault::Query);
    engine
        .register(Synthetic::new("working", Fault::None))
        .unwrap();
    let report = engine.search("fixture", &Cancellation::default());
    assert_eq!(report.packages.len(), 1);
    assert_eq!(report.failures.len(), 1);
    assert!(matches!(
        report.select(&selector()),
        Err(EngineError::Incomplete(_))
    ));
    let mut select = selector();
    select.backend = Some("working".into());
    assert_eq!(report.select(&select).unwrap(), id("working"));
    assert_eq!(engine.installed(&Cancellation::default()).failures.len(), 1);
}

#[test]
fn availability_errors_and_unsupported_capabilities_prevent_dispatch() {
    let cancel = Cancellation::default();
    for fault in [Fault::Unavailable, Fault::Detect] {
        let mut engine = engine(fault);
        assert_ne!(
            engine.discover(&cancel)[0].availability,
            Ok(Availability::Available)
        );
        assert_eq!(engine.search("fixture", &cancel).failures.len(), 1);
        assert!(engine
            .execute(&Operation::Install(id("synthetic")), &cancel, &mut |_| {})
            .is_err());
    }
    let mut engine = Engine::default();
    let mut backend = Synthetic::new("limited", Fault::None);
    backend.capabilities = &[Capability::Search];
    engine.register(backend).unwrap();
    assert!(matches!(
        engine.details(&id("limited"), &cancel),
        Err(EngineError::Unsupported {
            capability: Capability::Details,
            ..
        })
    ));
    assert!(matches!(
        engine.execute(&Operation::Remove(id("limited")), &cancel, &mut |_| {}),
        Err(EngineError::Unsupported {
            capability: Capability::Remove,
            ..
        })
    ));
    assert!(matches!(
        engine.register(Synthetic::new("limited", Fault::None)),
        Err(EngineError::DuplicateBackend(_))
    ));
    assert!(matches!(
        engine.details(&id("missing"), &cancel),
        Err(EngineError::UnknownBackend(_))
    ));
}

#[test]
fn malformed_backend_responses_are_not_exposed_as_valid_packages() {
    for fault in [Fault::WrongIdentity, Fault::Duplicate] {
        let report = engine(fault).search("fixture", &Cancellation::default());
        assert!(report.packages.is_empty());
        assert!(matches!(
            report.failures[0].error,
            EngineError::InvalidResponse { .. }
        ));
    }
    assert!(matches!(
        engine(Fault::MissingInstalled)
            .installed(&Cancellation::default())
            .failures[0]
            .error,
        EngineError::InvalidResponse { .. }
    ));
    assert!(matches!(
        engine(Fault::WrongDetails).details(&id("synthetic"), &Cancellation::default()),
        Err(EngineError::InvalidResponse { .. })
    ));
    assert_eq!(
        engine(Fault::Details).details(&id("synthetic"), &Cancellation::default()),
        Err(EngineError::NotFound)
    );
}

#[test]
fn best_effort_batches_keep_successes_and_emit_terminal_failure_events() {
    let mut engine = engine(Fault::Write);
    engine
        .register(Synthetic::new("working", Fault::None))
        .unwrap();
    let operations = [
        Operation::Install(id("synthetic")),
        Operation::Install(id("working")),
        Operation::Refresh {
            backend: "missing".into(),
        },
    ];
    let mut events = Vec::new();
    let results = engine.execute_batch(&operations, &Cancellation::default(), &mut |e| {
        events.push(e)
    });
    assert_eq!(results[0], Err(ExecutionError::LockBusy.into()));
    assert!(results[1].is_ok());
    assert!(matches!(results[2], Err(EngineError::UnknownBackend(_))));
    let finished: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Finished { result, .. } => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(finished, results);
    assert_eq!(engine.installed(&Cancellation::default()).packages.len(), 1);
}

#[test]
fn cancellation_stops_pending_work_without_relabeling_completed_writes() {
    let mut engine = engine(Fault::CancelWrite);
    let cancel = Cancellation::default();
    let operations = [
        Operation::Install(id("synthetic")),
        Operation::Remove(id("synthetic")),
    ];
    let mut events = Vec::new();
    let result = engine.execute_batch(&operations, &cancel, &mut |e| events.push(e));
    assert_eq!(
        result,
        [
            Ok(OperationOutcome {
                cancellation_deferred: true
            }),
            Err(EngineError::Cancelled)
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Finished { .. }))
            .count(),
        2
    );
    assert_eq!(
        engine.discover(&cancel)[0].availability,
        Err(EngineError::Cancelled)
    );
    assert_eq!(
        engine.search("", &cancel).failures[0].error,
        EngineError::Cancelled
    );
    assert_eq!(engine.installed(&Cancellation::default()).packages.len(), 1);
    let mut engine = Engine::default();
    let backend = Synthetic::new("during-detect", Fault::CancelDetect);
    let calls = backend.calls.clone();
    engine.register(backend).unwrap();
    assert_eq!(
        engine.execute(
            &Operation::Install(id("during-detect")),
            &Cancellation::default(),
            &mut |_| {}
        ),
        Err(EngineError::Cancelled)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

struct Defaults;
impl Backend for Defaults {
    fn id(&self) -> &str {
        "defaults"
    }
    fn capabilities(&self) -> &[Capability] {
        ALL_WITH_CLEAN
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
}
#[test]
fn unimplemented_backend_methods_explicitly_report_unsupported() {
    let mut engine = Engine::default();
    engine.register(Defaults).unwrap();
    let cancel = Cancellation::default();
    assert!(matches!(
        engine.search("", &cancel).failures[0].error,
        EngineError::Unsupported {
            capability: Capability::Search,
            ..
        }
    ));
    assert!(matches!(
        engine.installed(&cancel).failures[0].error,
        EngineError::Unsupported {
            capability: Capability::Installed,
            ..
        }
    ));
    assert!(matches!(
        engine.details(&id("defaults"), &cancel),
        Err(EngineError::Unsupported {
            capability: Capability::Details,
            ..
        })
    ));
    assert!(matches!(
        engine.execute(&Operation::Upgrade(id("defaults")), &cancel, &mut |_| {}),
        Err(EngineError::Unsupported {
            capability: Capability::Upgrade,
            ..
        })
    ));
    let cleanup = engine.cleanup(&cancel);
    assert!(cleanup.items.is_empty());
    assert!(matches!(
        cleanup.failures.as_slice(),
        [BackendFailure {
            backend,
            error: EngineError::Unsupported {
                capability: Capability::Clean,
                ..
            }
        }] if backend == "defaults"
    ));
    assert!(matches!(
        engine.execute(
            &Operation::Clean(CleanupId {
                backend: "defaults".into(),
                key: "missing".into(),
            }),
            &cancel,
            &mut |_| {}
        ),
        Err(EngineError::Unsupported {
            capability: Capability::Clean,
            ..
        })
    ));
}

#[test]
fn errors_keep_typed_execution_causes_and_useful_diagnostics() {
    use std::error::Error;
    assert_eq!(
        EngineError::from(ExecutionError::Cancelled),
        EngineError::Cancelled
    );
    let wrapped = EngineError::from(ExecutionError::AuthorizationDenied);
    assert!(wrapped.source().is_some());
    for error in [
        EngineError::UnknownBackend("fixture".into()),
        EngineError::DuplicateBackend("fixture".into()),
        EngineError::Unavailable {
            backend: "fixture".into(),
            reason: "missing".into(),
        },
        EngineError::Unsupported {
            backend: "fixture".into(),
            capability: Capability::Search,
        },
        EngineError::InvalidResponse {
            backend: "fixture".into(),
            reason: "wrong identity".into(),
        },
        EngineError::NotFound,
        EngineError::Ambiguous(vec![id("one"), id("two")]),
        EngineError::Incomplete(vec![]),
        EngineError::Cancelled,
        wrapped,
    ] {
        assert!(!error.to_string().is_empty());
    }
    assert!(EngineError::NotFound.source().is_none());
}

#[test]
fn details_reuse_skips_detection_for_warm_engines() {
    let cancel = Cancellation::default();
    // No detect() call happens before details_reuse: a broken detector would
    // fail here if the engine re-validated availability.
    let mut broken = Engine::default();
    broken
        .register(Synthetic::new("synthetic", Fault::Detect))
        .unwrap();
    assert!(matches!(
        broken.details_reuse(&id("synthetic"), &cancel),
        Ok(details) if details.package.id == id("synthetic")
    ));
    // Unknown backends, missing capabilities, wrong identities, and prior
    // cancellation still report precisely.
    let mut ready = engine(Fault::None);
    assert_eq!(
        ready.details_reuse(&id("missing"), &cancel),
        Err(EngineError::UnknownBackend("missing".into()))
    );
    let mut bare = Engine::default();
    bare.register(Defaults).unwrap();
    assert!(matches!(
        bare.details_reuse(&id("defaults"), &cancel),
        Err(EngineError::Unsupported { .. })
    ));
    let mut wrong = engine(Fault::WrongDetails);
    assert!(matches!(
        wrong.details_reuse(&id("synthetic"), &cancel),
        Err(EngineError::InvalidResponse { .. })
    ));
    let missing = PackageId {
        backend: "synthetic".into(),
        ..id("synthetic")
    };
    let mut missing_id = missing.clone();
    missing_id.name = "absent".into();
    assert_eq!(
        engine(Fault::None).details_reuse(&missing_id, &cancel),
        Err(EngineError::NotFound)
    );
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        engine(Fault::None).details_reuse(&id("synthetic"), &cancelled),
        Err(EngineError::Cancelled)
    );
}

#[test]
fn streaming_reports_cumulative_partials_equal_to_the_sync_query() {
    let cancel = Cancellation::default();
    let mut live = Engine::default();
    live.register(Synthetic::new("one", Fault::None)).unwrap();
    live.register(Synthetic::new("two", Fault::Query)).unwrap();
    let mut partials = vec![];
    let final_report = live.search_stream("fixture", &cancel, &mut |partial| {
        assert!(partial.packages.windows(2).all(|w| w[0].id <= w[1].id));
        partials.push(partial);
    });
    // One emission per backend. Reports are cumulative, so a failure
    // reappears in later partials; assert on membership, never on sums.
    assert_eq!(partials.len(), 2);
    for partial in &partials {
        assert!(partial.failures.iter().all(|f| f.backend == "two"));
    }
    assert_eq!(final_report.failures.len(), 1);
    assert_eq!(partials.last().unwrap(), &final_report);
    // The terminal emission matches a synchronous query on the same state.
    assert_eq!(live.search("fixture", &cancel), final_report);
    // Cancellation surfaces per backend like the synchronous query.
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut dead = Engine::default();
    dead.register(Synthetic::new("one", Fault::None)).unwrap();
    let mut seen = 0;
    dead.search_stream("fixture", &cancelled, &mut |_| seen += 1);
    assert_eq!(seen, 1);
    // The engine reassembles itself and stays usable afterwards.
    let report = dead.search("fixture", &Cancellation::default());
    assert!(report.failures.is_empty());

    // Detection can cancel before a query starts. The cancellation is
    // preserved as a backend failure rather than being mistaken for an empty
    // successful report.
    let cancelled = Cancellation::default();
    let mut detecting = Engine::default();
    detecting
        .register(Synthetic::new("cancel-detect", Fault::CancelDetect))
        .unwrap();
    let report = detecting.search("fixture", &cancelled);
    assert!(matches!(
        report.failures.as_slice(),
        [BackendFailure {
            error: EngineError::Cancelled,
            ..
        }]
    ));
}

#[test]
fn installed_stream_reports_cumulative_partials_equal_to_the_sync_query() {
    let cancel = Cancellation::default();
    let mut live = Engine::default();
    live.register(Synthetic::new("one", Fault::None)).unwrap();
    live.register(Synthetic::new("two", Fault::Query)).unwrap();
    let mut partials = vec![];
    let final_report = live.installed_stream(&cancel, &mut |partial| {
        assert!(partial.packages.windows(2).all(|w| w[0].id <= w[1].id));
        partials.push(partial);
    });
    // One emission per backend. Reports are cumulative, so a failure
    // reappears in later partials; assert on membership, never on sums.
    assert_eq!(partials.len(), 2);
    for partial in &partials {
        assert!(partial.failures.iter().all(|f| f.backend == "two"));
    }
    assert_eq!(final_report.failures.len(), 1);
    assert_eq!(partials.last().unwrap(), &final_report);
    // The terminal emission matches a synchronous query on the same state.
    assert_eq!(live.installed(&cancel), final_report);
}

#[test]
fn cleanup_revalidates_confirmed_preview_before_writing() {
    let mut engine = Engine::default();
    engine.register(Cleaner).unwrap();
    let cancel = Cancellation::default();
    let mut plan = engine.cleanup(&cancel).items.remove(0);
    let operation = Operation::Clean(plan.id.clone());
    plan.preview = "a different previously confirmed target".into();
    engine.remember_cleanup_plan(plan);
    let mut events = Vec::new();
    assert!(matches!(
        engine.execute(&operation, &cancel, &mut |event| events.push(event)),
        Err(EngineError::InvalidResponse { reason, .. }) if reason.contains("reload")
    ));
    assert_eq!(events.len(), 2);
    let plan = engine.cleanup(&cancel).items.remove(0);
    assert!(engine
        .execute(&Operation::Clean(plan.id), &cancel, &mut |_| {})
        .is_ok());
    // A used confirmation cannot be replayed without a new preview.
    assert!(matches!(
        engine.execute(&operation, &cancel, &mut |_| {}),
        Err(EngineError::InvalidResponse { .. })
    ));
}
