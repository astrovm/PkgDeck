//! Engine checks that stop a query or a batch before any write.
use pkgdeck_core::{
    engine::*,
    host::{Authorization, Host, Runtime},
    package::*,
    process::Cancellation,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// A backend with fixed capabilities that counts writes and native plan
/// reads, and whose plans change after the first `stable_reads` reads.
struct Fixture {
    id: &'static str,
    capabilities: &'static [Capability],
    writes: Arc<AtomicUsize>,
    reads: usize,
    stable_reads: usize,
}
impl Fixture {
    fn new(id: &'static str, capabilities: &'static [Capability]) -> Self {
        Self {
            id,
            capabilities,
            writes: Arc::default(),
            reads: 0,
            stable_reads: usize::MAX,
        }
    }
    fn preview(&mut self) -> String {
        self.reads += 1;
        if self.reads > self.stable_reads {
            "changed".into()
        } else {
            "reviewed".into()
        }
    }
}
impl Backend for Fixture {
    fn id(&self) -> &str {
        self.id
    }
    fn capabilities(&self) -> &[Capability] {
        self.capabilities
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn apt_upgrade_plan(&mut self, _: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        Ok(AptUpgradePlan {
            preview: self.preview(),
            upgrades: vec![],
            installs: vec![],
            removals: vec![],
        })
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        Ok(Some(TransactionPlan {
            operation: operation.clone(),
            native_preview: self.preview(),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: None,
        }))
    }
    fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Ok(vec![CleanupItem {
            id: CleanupId {
                backend: self.id.into(),
                key: "cache".into(),
            },
            kind: CleanupKind::PackageCache,
            title: "Cache".into(),
            summary: "Cached downloads".into(),
            preview: self.preview(),
        }])
    }
    fn execute_group(
        &mut self,
        operations: &[Operation],
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Some(Ok(vec![OperationOutcome::default(); operations.len()]))
    }
    fn execute(
        &mut self,
        _: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(OperationOutcome::default())
    }
}

const APT: &[Capability] = &[
    Capability::Install,
    Capability::Refresh,
    Capability::Upgrade,
    Capability::Clean,
];

fn apt_install(name: &str) -> Operation {
    Operation::Install(PackageId {
        backend: "apt".into(),
        name: name.into(),
        architecture: "amd64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    })
}
fn authorized(backend: Fixture) -> Engine {
    let mut engine = Engine::default();
    engine.register(backend).unwrap();
    engine.enable_batch_authorization(
        Host::new(Runtime::Native, Default::default()),
        Authorization::Polkit,
    );
    engine
}
fn invalid(result: &Result<OperationOutcome, EngineError>, expected: &str) {
    match result {
        Err(EngineError::InvalidResponse { reason, .. }) => {
            assert!(reason.contains(expected), "{reason}")
        }
        other => panic!("expected {expected:?}, got {other:?}"),
    }
}

#[test]
fn backends_without_an_upgrade_preview_cannot_plan_a_full_upgrade() {
    struct Plain;
    impl Backend for Plain {
        fn id(&self) -> &str {
            "apt"
        }
        fn capabilities(&self) -> &[Capability] {
            APT
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
    }
    let mut engine = Engine::default();
    engine.register(Plain).unwrap();
    let cancel = Cancellation::default();
    assert_eq!(
        engine.plan_apt_upgrade(&cancel),
        Err(EngineError::Unsupported {
            backend: "apt".into(),
            capability: Capability::Upgrade,
        })
    );
    // Nor a preview of a single change.
    assert_eq!(
        engine.plan_operation(&apt_install("tool"), &cancel),
        Ok(None)
    );
}

#[test]
fn cancelled_cleanup_reports_each_source() {
    let cancel = Cancellation::default();
    cancel.cancel();
    let mut engine = Engine::default();
    engine.register(Fixture::new("apt", APT)).unwrap();
    let report = engine.cleanup(&cancel);
    assert!(report.items.is_empty());
    assert_eq!(
        report.failures,
        [BackendFailure {
            backend: "apt".into(),
            error: EngineError::Cancelled,
        }]
    );
}

#[test]
fn sources_without_a_capability_report_it_as_unsupported() {
    let cancel = Cancellation::default();
    let mut engine = Engine::default();
    engine.register(Fixture::new("apt", APT)).unwrap();
    let report = engine.installed(&cancel);
    assert!(report.packages.is_empty());
    assert_eq!(
        report.failures,
        [BackendFailure {
            backend: "apt".into(),
            error: EngineError::Unsupported {
                backend: "apt".into(),
                capability: Capability::Installed,
            },
        }]
    );
    let id = match apt_install("tool") {
        Operation::Install(id) => id,
        _ => unreachable!(),
    };
    assert_eq!(
        engine.details_reuse(&id, &cancel),
        Err(EngineError::Unsupported {
            backend: "apt".into(),
            capability: Capability::Details,
        })
    );
}

#[test]
fn a_batch_rechecks_unreviewed_or_changed_plans_before_authorization() {
    let cancel = Cancellation::default();
    // A full APT upgrade needs a reviewed plan; this backend has none.
    let fixture = Fixture::new("apt", APT);
    let writes = fixture.writes.clone();
    let mut engine = authorized(fixture);
    let results = engine.execute_batch(
        &[Operation::UpgradeAll {
            backend: "apt".into(),
        }],
        &cancel,
        &mut |_| {},
    );
    invalid(&results[0], "not reviewed");
    // A cleanup task nobody reviewed is refused.
    let clean = Operation::Clean(CleanupId {
        backend: "apt".into(),
        key: "cache".into(),
    });
    let results = engine.execute_batch(std::slice::from_ref(&clean), &cancel, &mut |_| {});
    invalid(&results[0], "changed or expired");
    // So is an operation whose native plan changed since review.
    let install = apt_install("tool");
    let fixture = Fixture {
        stable_reads: 1,
        ..Fixture::new("apt", APT)
    };
    let writes_changed = fixture.writes.clone();
    let mut engine = authorized(fixture);
    engine.plan_operation(&install, &cancel).unwrap();
    let results = engine.execute_batch(std::slice::from_ref(&install), &cancel, &mut |_| {});
    invalid(&results[0], "different now");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(writes_changed.load(Ordering::SeqCst), 0);
}

#[test]
fn grouped_apt_changes_are_rechecked_right_before_the_transaction() {
    let cancel = Cancellation::default();
    let (first, second) = (apt_install("one"), apt_install("two"));
    // The plan is read at review, at the batch check, and again right
    // before the grouped write, where it has changed.
    let fixture = Fixture {
        stable_reads: 2,
        ..Fixture::new("apt", APT)
    };
    let writes = fixture.writes.clone();
    let mut engine = authorized(fixture);
    engine.plan_operation(&first, &cancel).unwrap();
    let mut finished = 0;
    let results = engine.execute_batch(&[first, second], &cancel, &mut |event| {
        if matches!(event, Event::Finished { .. }) {
            finished += 1;
        }
    });
    assert_eq!(results.len(), 2);
    for result in &results {
        invalid(result, "different now");
    }
    assert_eq!(finished, 2);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[test]
fn a_reviewed_cleanup_runs_in_a_batch() {
    let cancel = Cancellation::default();
    let fixture = Fixture::new("user", APT);
    let writes = fixture.writes.clone();
    let mut engine = authorized(fixture);
    let report = engine.cleanup(&cancel);
    assert_eq!(report.items.len(), 1);
    let clean = Operation::Clean(report.items[0].id.clone());
    let results = engine.execute_batch(&[clean], &cancel, &mut |_| {});
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[test]
fn apt_refreshes_in_a_batch_run_on_their_own() {
    let cancel = Cancellation::default();
    let fixture = Fixture::new("apt", APT);
    let writes = fixture.writes.clone();
    let mut engine = authorized(fixture);
    let refresh = Operation::Refresh {
        backend: "apt".into(),
    };
    let results = engine.execute_batch(&[refresh.clone(), refresh], &cancel, &mut |_| {});
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(writes.load(Ordering::SeqCst), 2);
}
