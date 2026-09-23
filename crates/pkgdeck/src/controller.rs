//! Thin Qt facade: native work runs on a worker; Qt properties change only on the GUI thread.
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use pkgdeck_core::activity::{History, Outcome, State};
use pkgdeck_core::background::{self, Schedule};
use pkgdeck_core::repositories::{self, Action as RepositoryAction};
use pkgdeck_core::{engine::*, host::Authorization, package::*, process::Cancellation};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    pin::Pin,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// CXX-Qt generates the FFI boundary; application code below uses safe Rust.
#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        include!("pkgdeck/native/controller.h");
        #[allow(dead_code)] // Test constructor; QML constructs production instances.
        fn create_controller() -> UniquePtr<PackageController>;
        type QString = cxx_qt_lib::QString;
    }
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, rows)]
        #[qproperty(QString, details)]
        #[qproperty(QString, status)]
        #[qproperty(QString, repositories)]
        #[qproperty(QString, source_catalog)]
        #[qproperty(QString, report_state)]
        #[qproperty(QString, activity)]
        #[qproperty(QString, background_state)]
        #[qproperty(QString, confirmation)]
        #[qproperty(QString, confirmation_data)]
        #[qproperty(QString, version)]
        #[qproperty(bool, busy)]
        #[qproperty(bool, writing)]
        #[qproperty(bool, upgradable)]
        type PackageController = super::Controller;
        #[qinvokable]
        fn load(
            self: Pin<&mut PackageController>,
            view: QString,
            query: QString,
            source: QString,
            sudo: bool,
            force: bool,
        );
        #[qinvokable]
        fn select(self: Pin<&mut PackageController>, index: i32);
        #[qinvokable]
        #[cxx_name = "retrySource"]
        fn retry_source(
            self: Pin<&mut PackageController>,
            view: QString,
            query: QString,
            source: QString,
        );
        #[qinvokable]
        fn propose(self: Pin<&mut PackageController>, action: QString, index: i32);
        #[qinvokable]
        #[cxx_name = "loadRepositories"]
        fn load_repositories(self: Pin<&mut PackageController>);
        #[qinvokable]
        #[cxx_name = "changeRepository"]
        fn change_repository(self: Pin<&mut PackageController>, action: QString);
        #[qinvokable]
        #[cxx_name = "proposeChecked"]
        fn propose_checked(self: Pin<&mut PackageController>, identities: QString);
        #[qinvokable]
        fn confirm(self: Pin<&mut PackageController>, approved: bool);
        #[qinvokable]
        fn cancel(self: Pin<&mut PackageController>);
        #[qinvokable]
        fn poll(self: Pin<&mut PackageController>);
        #[qinvokable]
        #[cxx_name = "refreshActivity"]
        fn refresh_activity(self: Pin<&mut PackageController>);
        #[qinvokable]
        #[cxx_name = "cancelQueued"]
        fn cancel_queued(self: Pin<&mut PackageController>);
        #[qinvokable]
        #[cxx_name = "checkUpdates"]
        fn check_updates(
            self: Pin<&mut PackageController>,
            sources: QString,
            enabled: bool,
            offline: bool,
            metered: bool,
            force: bool,
        );
        #[qinvokable]
        #[cxx_name = "setAutostart"]
        fn set_autostart(self: Pin<&mut PackageController>, enabled: bool) -> bool;
    }
}

#[derive(Clone)]
enum Job {
    Repositories(Option<RepositoryAction>),
    Load(String, String),
    BackgroundUpdates(Vec<String>),
    RetrySource(String, String, String),
    Details(PackageId),
    PlanOperation(Operation),
    PlanCleanAll(Vec<Operation>),
    Write(Operation, Option<Box<TransactionPlan>>),
    PlanUpgrade(Vec<Operation>, usize),
    UpgradeAll(Vec<Operation>, Option<AptUpgradePlan>),
    CleanAll(Vec<Operation>),
}
impl Job {
    fn writes(&self) -> bool {
        matches!(
            self,
            Self::Write(..)
                | Self::UpgradeAll(..)
                | Self::CleanAll(_)
                | Self::Repositories(Some(_))
        )
    }
    fn operations(&self) -> Vec<Operation> {
        match self {
            Self::Write(operation, _) => vec![operation.clone()],
            Self::UpgradeAll(operations, _) | Self::CleanAll(operations) => operations.clone(),
            _ => vec![],
        }
    }
}
struct Confirmed {
    job: Job,
    activity_id: Option<u64>,
    cleanup_preview: Vec<CleanupItem>,
}
enum Payload {
    Repositories(repositories::Report),
    Packages(PackageReport),
    BackgroundUpdates(PackageReport),
    RetryPackages(String, PackageReport),
    RetryCleanup(String, CleanupReport),
    RetrySources(String, Vec<Source>),
    Sources(Vec<Source>),
    Details(Box<PackageDetails>),
    Written(OperationOutcome),
    Batch(String, Vec<Outcome>),
    Cleanup(CleanupReport),
    UpgradePreview(Vec<Operation>, usize, Option<AptUpgradePlan>),
    OperationPreview(Operation, Option<Box<TransactionPlan>>),
    CleanPreview(Vec<Operation>, Vec<CleanupItem>),
}
enum Reply {
    Progress(String),
    Partial(PackageReport),
    Inventory(PackageReport),
    DetailsPreview(Box<PackageDetails>),
    Done(Result<Payload, EngineError>),
    Engine(Box<Engine>),
}
fn execute(engine: &mut Engine, job: Job, cancel: &Cancellation, send: &mut dyn FnMut(Reply)) {
    fn filter_updates(report: &mut PackageReport) {
        report
            .packages
            .retain(|p| p.update == UpdateAvailability::Available);
    }
    // The Installed view filters client-side by substring; other views
    // ignore the query so stale field text never narrows them. Failures
    // are preserved so partial results stay visible.
    fn filter_installed(report: &mut PackageReport, query: &str) {
        if !query.trim().is_empty() {
            let needle = query.trim().to_lowercase();
            report.packages.retain(|p| {
                p.id.name.to_lowercase().contains(&needle)
                    || p.summary.to_lowercase().contains(&needle)
            });
        }
    }
    let result = match job {
        Job::Repositories(_) => Err(EngineError::NotFound),
        Job::BackgroundUpdates(_) => {
            let mut report = engine.installed(cancel);
            filter_updates(&mut report);
            Ok(Payload::BackgroundUpdates(report))
        }
        Job::Load(view, query) => {
            if view == "Sources" {
                Ok(Payload::Sources(engine.discover(cancel)))
            } else if view == "Clean" {
                Ok(Payload::Cleanup(engine.cleanup(cancel)))
            } else if view == "Search" {
                // Stream cumulative partials so fast backends render while
                // slow ones still query; the terminal emission below carries
                // the same deterministic report as a synchronous query.
                let mut send_partial = |mut partial: PackageReport| {
                    partial.packages.retain(|p| !unverified_search_offer(p));
                    send(Reply::Partial(partial));
                };
                let mut report = engine.search_stream(&query, cancel, &mut send_partial);
                report.packages.retain(|p| !unverified_search_offer(p));
                Ok(Payload::Packages(report))
            } else {
                // Installed and Updates stream like Search so rows appear
                // while slow backends still query. Every partial carries
                // the same view filter as the terminal report below.
                // Upgrade gating still waits for the terminal report with
                // complete failures (see apply); partials only fill rows.
                let mut send_partial = |mut partial: PackageReport| {
                    if view == "Updates" {
                        filter_updates(&mut partial);
                    } else {
                        filter_installed(&mut partial, &query);
                    }
                    send(Reply::Partial(partial))
                };
                let mut report = engine.installed_stream(cancel, &mut send_partial);
                send(Reply::Inventory(report.clone()));
                if view == "Updates" {
                    filter_updates(&mut report);
                } else {
                    filter_installed(&mut report, &query);
                }
                Ok(Payload::Packages(report))
            }
        }
        Job::RetrySource(view, query, source) => {
            if view == "Sources" {
                Ok(Payload::RetrySources(source, engine.discover(cancel)))
            } else if view == "Clean" {
                Ok(Payload::RetryCleanup(source, engine.cleanup(cancel)))
            } else {
                let mut report = if view == "Search" { engine.search(&query, cancel) }
                    else { engine.installed(cancel) };
                if view == "Updates" {
                    filter_updates(&mut report);
                } else if view == "Installed" {
                    filter_installed(&mut report, &query);
                } else {
                    report.packages.retain(|p| !unverified_search_offer(p));
                }
                Ok(Payload::RetryPackages(source, report))
            }
        }
        Job::Details(id) => engine
            .details(&id, cancel)
            .map(|d| Payload::Details(Box::new(d))),
        Job::PlanUpgrade(operations, count) => {
            if operations.iter().any(|operation| {
                matches!(operation, Operation::UpgradeAll { backend } if backend == "apt")
            }) {
                engine
                    .plan_apt_upgrade(cancel)
                    .map(|plan| Payload::UpgradePreview(operations, count, Some(plan)))
            } else {
                Ok(Payload::UpgradePreview(operations, count, None))
            }
        }
        Job::PlanOperation(operation) => engine.plan_operation(&operation, cancel)
            .map(|plan| Payload::OperationPreview(operation, plan.map(Box::new))),
        Job::PlanCleanAll(operations) => {
            let report = engine.cleanup(cancel);
            Ok(Payload::CleanPreview(operations, report.items))
        }
        Job::UpgradeAll(operations, _) | Job::CleanAll(operations) => {
            let results = engine.execute_batch(&operations, cancel, &mut |event| {
                if let Event::Progress {
                    operation,
                    progress: Progress::Message(message),
                } = event
                {
                    send(Reply::Progress(format!(
                        "{}: {message}",
                        operation_label(&operation)
                    )));
                }
            });
            let completed = results.iter().filter(|r| r.is_ok()).count();
            let noun = if operations.iter().all(|op| matches!(op, Operation::Clean(_))) { "cleanup tasks" } else { "updates" };
            let mut status = format!("Completed {completed} of {} {noun}.", operations.len());
            let mut outcomes = Vec::new();
            for (operation, result) in operations.iter().zip(results) {
                outcomes.push(match &result {
                    Ok(_) => Outcome::Finished,
                    Err(EngineError::Cancelled) => Outcome::Cancelled,
                    Err(_) => Outcome::Failed,
                });
                let outcome = match result {
                    Ok(outcome) if outcome.cancellation_deferred => {
                        "Completed after cancellation; changes were not rolled back.".into()
                    }
                    Ok(_) => "Completed".into(),
                    Err(error) => error.to_string(),
                };
                status.push_str(&format!("\n{}: {outcome}", operation_label(operation)));
            }
            if operations.iter().any(|op| matches!(op, Operation::Upgrade(id) if id.backend == "fwupd")) {
                status.push_str("\nFirmware: follow the device restart or shutdown requirements shown before updating.");
            }
            Ok(Payload::Batch(status, outcomes))
        }
        Job::Write(op, _) => engine
            .execute(&op, cancel, &mut |event| {
                if let Event::Progress { progress, .. } = event {
                    send(Reply::Progress(match progress {
                        Progress::Message(message) => message,
                        Progress::Transfer { completed, total } => match total {
                            Some(total) => format!("Transferred {completed} of {total}"),
                            None => format!("Transferred {completed}"),
                        },
                    }));
                }
            })
            .map(|outcome| {
                if matches!(&op, Operation::Upgrade(id) if id.backend == "fwupd") {
                    Payload::Batch(if outcome.cancellation_deferred {
                        "Firmware completed after cancellation. Follow the device restart or shutdown requirements.".into()
                    } else {
                        "Firmware completed. Follow the device restart or shutdown requirements.".into()
                    }, vec![Outcome::Finished])
                } else { Payload::Written(outcome) }
            }),
    };
    send(Reply::Done(result));
}

struct Worker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<Reply>,
    cancel: Cancellation,
    job: Job,
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.cancel.cancel();
            let _ = worker.handle.join();
        }
    }
}
pub struct Controller {
    rows: QString,
    details: QString,
    status: QString,
    confirmation: QString,
    confirmation_data: QString,
    repositories: QString,
    source_catalog: QString,
    report_state: QString,
    activity: QString,
    background_state: QString,
    version: QString,
    busy: bool,
    writing: bool,
    upgradable: bool,
    updates_view: bool,
    packages: Vec<Package>,
    cleanup: Vec<CleanupItem>,
    failures: Vec<BackendFailure>,
    last_success: BTreeMap<String, u64>,
    retrying_source: Option<String>,
    detail_cache: BTreeMap<PackageId, QString>,
    sources: Vec<Source>,
    pending: Option<Job>,
    queued: Option<Job>,
    confirmed_queue: VecDeque<Confirmed>,
    active_activity_id: Option<u64>,
    revalidating: Option<Confirmed>,
    discard_revalidation: bool,
    validated_confirmed: Option<Confirmed>,
    deferred_load: Option<Job>,
    activity_store: Option<History>,
    autostart_path: Option<PathBuf>,
    background_schedule: Schedule,
    selected: Option<PackageId>,
    engine: Option<Engine>,
    view_cache: ViewCache,
    prefetched: BTreeMap<String, (Instant, Payload)>,
    background: bool,
    prefetch: Vec<String>,
    source_filter: Vec<String>,
    sudo: bool,
    worker: Option<Worker>,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            rows: "[]".into(),
            details: "{}".into(),
            status: "Choose a view or search for a package.".into(),
            confirmation: QString::default(),
            confirmation_data: "{}".into(),
            repositories: "{}".into(),
            source_catalog: "[]".into(),
            report_state: r#"{"phase":"idle"}"#.into(),
            activity: "[]".into(),
            background_state: "{}".into(),
            version: pkgdeck_core::VERSION.into(),
            busy: false,
            writing: false,
            upgradable: false,
            updates_view: false,
            packages: vec![],
            cleanup: vec![],
            failures: vec![],
            last_success: BTreeMap::new(),
            retrying_source: None,
            detail_cache: BTreeMap::new(),
            sources: vec![],
            pending: None,
            queued: None,
            confirmed_queue: VecDeque::new(),
            active_activity_id: None,
            revalidating: None,
            discard_revalidation: false,
            validated_confirmed: None,
            deferred_load: None,
            activity_store: if cfg!(test) {
                None
            } else {
                History::default_store()
            },
            autostart_path: if cfg!(test) {
                None
            } else {
                background::autostart_path()
            },
            background_schedule: Schedule::default(),
            selected: None,
            engine: None,
            view_cache: ViewCache { entries: vec![] },
            prefetched: BTreeMap::new(),
            background: false,
            prefetch: prefetch_views(),
            source_filter: Vec::new(),
            sudo: false,
            worker: None,
        }
    }
}
fn upgrade_plan(packages: &[Package]) -> Vec<Operation> {
    let backends: std::collections::BTreeSet<_> = packages
        .iter()
        .filter(|p| p.installed_version.is_some() && p.update == UpdateAvailability::Available)
        .filter(|p| !pkgdeck_core::backends::update_only(&p.id.backend))
        .map(|p| p.id.backend.clone())
        .collect();
    backends
        .into_iter()
        .map(|backend| Operation::UpgradeAll { backend })
        .chain(
            packages
                .iter()
                .filter(|p| {
                    pkgdeck_core::backends::update_only(&p.id.backend)
                        && p.update == UpdateAvailability::Available
                })
                .map(|p| Operation::Upgrade(p.id.clone())),
        )
        .collect()
}
fn encoded(value: impl serde::Serialize) -> QString {
    serde_json::to_string(&value)
        .expect("serializable frontend data")
        .as_str()
        .into()
}
/// Engine scope for a job: Details jobs address one backend directly, so the
/// engine registers only that backend and skips every other backend's
/// detection roundtrip. All other jobs use the session source set, where an
/// empty filter means every available backend.
fn engine_source(job: &Job, filter: &[String]) -> Vec<String> {
    match job {
        Job::BackgroundUpdates(sources) => sources.clone(),
        Job::Write(operation, _) => vec![operation.backend().into()],
        Job::UpgradeAll(operations, _) | Job::CleanAll(operations) => operations.iter().map(|op| op.backend().to_owned()).collect(),
        Job::RetrySource(_, _, source) => vec![source.clone()],
        Job::PlanOperation(operation) => vec![operation.backend().into()],
        Job::PlanCleanAll(operations) => operations.iter().map(|op| op.backend().to_owned()).collect(),
        // The picker needs to explain disabled and unavailable managers too.
        Job::Load(view, _) if view == "Sources" => vec![],
        Job::Details(id) => vec![id.backend.clone()],
        Job::PlanUpgrade(operations, _)
            if operations.iter().any(|operation| {
                matches!(operation, Operation::UpgradeAll { backend } if backend == "apt")
            }) => vec!["apt".into()],
        _ => filter.to_owned(),
    }
}
fn same_target(a: &Operation, b: &Operation) -> bool {
    match (a, b) {
        (
            Operation::Install(x) | Operation::Remove(x) | Operation::Upgrade(x),
            Operation::Install(y) | Operation::Remove(y) | Operation::Upgrade(y),
        ) => x == y,
        (Operation::Clean(x), Operation::Clean(y)) => x == y,
        (Operation::UpgradeAll { backend }, other) | (other, Operation::UpgradeAll { backend }) => {
            backend == other.backend()
        }
        _ => false,
    }
}
fn queue_conflict(
    candidate: &Job,
    active: Option<&Job>,
    queued: &VecDeque<Confirmed>,
    revalidating: Option<&Confirmed>,
    validated: Option<&Confirmed>,
) -> Option<&'static str> {
    let operations = candidate.operations();
    for existing in active
        .into_iter()
        .chain(queued.iter().map(|entry| &entry.job))
        .chain(revalidating.map(|entry| &entry.job))
        .chain(validated.map(|entry| &entry.job))
    {
        for operation in &operations {
            for other in existing.operations() {
                if operation == &other {
                    return Some("This operation is already running or queued.");
                }
                if same_target(operation, &other) {
                    return Some("A conflicting operation is already running or queued.");
                }
            }
        }
    }
    None
}
fn repreview_changed_plan(
    job: &Job,
    result: &Result<Payload, EngineError>,
    packages: &[Package],
) -> Option<Job> {
    if !matches!(result, Err(EngineError::InvalidResponse { reason, .. }) if reason.contains("plan changed"))
    {
        return None;
    }
    match job {
        Job::Write(operation, Some(_)) => Some(Job::PlanOperation(operation.clone())),
        Job::UpgradeAll(operations, Some(_)) => {
            let count = packages
                .iter()
                .filter(|package| {
                    package.installed_version.is_some()
                        && package.update == UpdateAvailability::Available
                })
                .count();
            Some(Job::PlanUpgrade(operations.clone(), count))
        }
        _ => None,
    }
}
/// A finished details reply is fresh only while its identity is still the
/// current selection; rapid navigation supersedes slower replies.
fn details_fresh(selected: Option<&PackageId>, details: &PackageDetails) -> bool {
    selected.is_some_and(|id| *id == details.package.id)
}
fn scope_label(scope: &Scope) -> String {
    match scope {
        Scope::System => "System".into(),
        Scope::User { uid } => format!("User {uid}"),
        Scope::Environment { path } => path.display().to_string(),
    }
}
fn source_status(source: &Source) -> String {
    match &source.availability {
        Ok(Availability::Available) => "Available".into(),
        Ok(Availability::Unavailable(reason)) => format!("Unavailable: {reason}"),
        Err(error) => error.to_string(),
    }
}
fn source_row(source: &Source) -> Value {
    let state = match &source.availability {
        Ok(Availability::Available) => "available",
        Ok(Availability::Unavailable(reason))
            if reason.contains("restricted") || reason.contains("disabled by") =>
        {
            "restricted"
        }
        Ok(Availability::Unavailable(_)) => "unavailable",
        Err(error) => failure_kind(error),
    };
    json!({"kind": "source", "name": source.backend, "source": source.backend,
        "summary": source_status(source), "available": state == "available",
        "availability_kind": state, "check_failed": source.availability.is_err(), "capabilities": source.capabilities})
}
fn failure_kind(error: &EngineError) -> &'static str {
    match error {
        EngineError::Unsupported { .. } => "unsupported",
        EngineError::Unavailable { .. } => "unavailable",
        EngineError::Cancelled
        | EngineError::Execution(pkgdeck_core::process::ExecutionError::Cancelled) => "cancelled",
        EngineError::Execution(
            pkgdeck_core::process::ExecutionError::AuthorizationCancelled
            | pkgdeck_core::process::ExecutionError::AuthorizationDenied,
        ) => "authorization",
        EngineError::Execution(pkgdeck_core::process::ExecutionError::LockBusy) => "locked",
        _ => "failed",
    }
}
fn read_failed(failure: &BackendFailure) -> bool {
    !matches!(failure.error, EngineError::Unsupported { .. })
}
/// Details for a failed source row. Served from the stored report without a
/// backend roundtrip: the query already failed, re-querying cannot help.
fn failure_details(failure: &BackendFailure) -> QString {
    encoded(json!({
        "failure": {"backend": failure.backend, "error": failure.error.to_string()},
        "hint": format!("Check the {} source in the Sources view, or run the manager directly in a terminal for complete output.", failure.backend),
    }))
}
/// Upgrade operations for checked identities that still resolve to an
/// installed package with an available update. Stale identities (rows that
/// moved on or vanished) are skipped, never guessed.
fn checked_upgrades(packages: &[Package], identities: &str) -> Vec<Operation> {
    let Ok(raw) = serde_json::from_str::<Vec<Vec<serde_json::Value>>>(identities) else {
        return vec![];
    };
    raw.into_iter()
        .filter_map(|parts| {
            let [backend, name, architecture, remote, scope, rest @ ..] = parts.as_slice() else {
                return None;
            };
            let id = PackageId {
                backend: backend.as_str()?.to_owned(),
                name: name.as_str()?.to_owned(),
                architecture: architecture.as_str()?.to_owned(),
                scope: serde_json::from_value(scope.clone()).ok()?,
                remote: remote.as_str().map(str::to_owned),
                reference: match rest {
                    [] | [Value::Null] => None,
                    [Value::String(reference)] => Some(reference.clone()),
                    _ => return None,
                },
            };
            packages
                .iter()
                .find(|package| package.id == id)
                .filter(|package| {
                    package.installed_version.is_some()
                        && package.update == UpdateAvailability::Available
                })
                .map(|package| Operation::Upgrade(package.id.clone()))
        })
        .collect()
}
/// The resolved outcome of an Upgrade-selected request: the operations to
/// queue plus the exact confirmation and status text to show. Pure so the
/// resolution rules stay covered without a Qt object.
struct CheckedPlan {
    operations: Vec<Operation>,
    confirmation: String,
    status: Option<String>,
}
fn plan_checked_upgrade(packages: &[Package], identities: &str) -> CheckedPlan {
    let operations = checked_upgrades(packages, identities);
    if operations.is_empty() {
        return CheckedPlan {
            operations: vec![],
            confirmation: String::new(),
            status: Some("No selected packages can be updated.".into()),
        };
    }
    let count = operations.len();
    let labels = operations
        .iter()
        .map(|operation| confirmation_label(operation, packages))
        .collect::<Vec<_>>()
        .join("\n\n");
    CheckedPlan {
        operations,
        confirmation: format!("Update {count} selected packages?\n\n{labels}\n\nUpdates use each source’s native updater. Native dependency changes may follow. Successful updates are not rolled back if another fails. Continue?"),
        status: None,
    }
}
/// Cache key for one view snapshot: view, search text, checked sources,
/// and elevation. The Installed filter is client-side, so it stays out of
/// the key and shares the loaded rows while typing.
fn prefetch_views() -> Vec<String> {
    ["Installed", "Updates", "Sources", "Clean"]
        .into_iter()
        .rev()
        .map(str::to_owned)
        .collect()
}
const VIEW_TTL: Duration = Duration::from_secs(60);
fn cache_key(view: &str, query: &str, sources: &[String], sudo: bool) -> String {
    format!("{view}\0{query}\0{}\0{sudo}", sources.join(","))
}
/// Per-view snapshots so switching sections is instant after the first
/// visit. Small Vec, oldest evicted: views are few and searches share keys
/// only when repeated exactly.
struct ViewCache {
    entries: Vec<(String, CachedView)>,
}
#[derive(Clone)]
struct CachedView {
    loaded: Instant,
    cleanup: Vec<CleanupItem>,
    packages: Vec<Package>,
    failures: Vec<BackendFailure>,
    sources: Vec<Source>,
    rows: QString,
    status: QString,
    report_state: QString,
    upgradable: bool,
    updates_view: bool,
}
impl ViewCache {
    const CAPACITY: usize = 8;
    fn get(&self, key: &str) -> Option<&CachedView> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, view)| view)
    }
    fn insert(&mut self, key: String, view: CachedView) {
        self.entries.retain(|(candidate, _)| candidate != &key);
        if self.entries.len() >= Self::CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push((key, view));
    }
    fn clear(&mut self) {
        self.entries.clear();
    }
}
fn confirmation_label(operation: &Operation, packages: &[Package]) -> String {
    let mut label = operation_label(operation);
    if operation.backend() == "fwupd" {
        for package in packages.iter().filter(|package| {
            package.id.backend == "fwupd" && package.update == UpdateAvailability::Available
        }) {
            if matches!(operation, Operation::Upgrade(id) if *id != package.id) {
                continue;
            }
            label.push_str(&format!("\n{}: {}", package.display_name, package.summary));
        }
    }
    label
}
fn operation_label(operation: &Operation) -> String {
    let (action, id) = match operation {
        Operation::Install(id) => ("Install", id),
        Operation::Remove(id) => ("Remove", id),
        Operation::Upgrade(id) => ("Update", id),
        Operation::Refresh { backend } => return format!("Refresh metadata for {backend}"),
        Operation::UpgradeAll { backend } => return format!("Update all packages from {backend}"),
        Operation::Clean(id) => return format!("Clean {} with {}", id.key, id.backend),
    };
    format!(
        "{action} {}\nSource: {}\nArchitecture: {}\nScope: {}",
        id.reference.as_deref().unwrap_or(&id.name),
        id.backend,
        id.architecture,
        scope_label(&id.scope)
    )
}
fn action_name(operation: &Operation) -> &'static str {
    match operation {
        Operation::Install(_) => "Install",
        Operation::Remove(_) => "Remove",
        Operation::Upgrade(_) | Operation::UpgradeAll { .. } => "Update",
        Operation::Refresh { .. } => "Refresh",
        Operation::Clean(_) => "Clean",
    }
}
fn confirmation_preview(
    operation: &Operation,
    packages: &[Package],
    cleanup: &[CleanupItem],
    plan: Option<&TransactionPlan>,
) -> Value {
    let mut lines = vec![operation_label(operation)];
    let mut title = action_name(operation).to_owned();
    if let Some(package) = packages.iter().find(|package| match operation {
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
            package.id == *id
        }
        _ => false,
    }) {
        title = format!(
            "{} {}",
            title,
            if package.display_name.is_empty() {
                &package.id.name
            } else {
                &package.display_name
            }
        );
        if !package.display_name.is_empty() && package.display_name != package.id.name {
            lines.insert(0, package.display_name.clone());
        }
        if let Some(version) = &package.installed_version {
            lines.push(format!("Installed: {version}"));
        }
        if let Some(version) = &package.candidate_version {
            lines.push(format!("Available: {version}"));
        }
        if package.id.backend == "fwupd" {
            lines.push(package.summary.clone());
        }
    }
    if let Operation::Clean(id) = operation {
        if let Some(item) = cleanup.iter().find(|item| item.id == *id) {
            title = format!("Clean {}", item.title);
            lines.push(item.preview.clone());
        }
    }
    if let Operation::Refresh { backend } = operation {
        title = format!("Refresh {backend}");
    }
    if let Some(plan) = plan {
        let requested_name = match operation {
            Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
                Some(id.name.as_str())
            }
            _ => None,
        };
        let requested_action = match operation {
            Operation::Install(_) => Some(PlannedAction::Install),
            Operation::Remove(_) => Some(PlannedAction::Remove),
            Operation::Upgrade(_) => Some(PlannedAction::Upgrade),
            _ => None,
        };
        let describe = |change: &PlannedChange| {
            let action = match change.action {
                PlannedAction::Install => "Install",
                PlannedAction::Remove => "Remove",
                PlannedAction::Upgrade => "Update",
            };
            let version = match (&change.installed_version, &change.candidate_version) {
                (Some(old), Some(new)) => format!(" ({old} → {new})"),
                (Some(old), None) => format!(" ({old})"),
                (None, Some(new)) => format!(" ({new})"),
                (None, None) => String::new(),
            };
            format!("{action} {}{version}", change.name)
        };
        let requested = plan
            .changes
            .iter()
            .filter(|change| {
                Some(change.name.as_str()) == requested_name
                    && Some(change.action) == requested_action
            })
            .map(describe)
            .collect::<Vec<_>>();
        if !requested.is_empty() {
            lines.push(format!("Requested change: {}", requested.join(", ")));
        }
        let changes = plan
            .changes
            .iter()
            .filter(|change| {
                Some(change.name.as_str()) != requested_name
                    || Some(change.action) != requested_action
            })
            .map(describe)
            .collect::<Vec<_>>();
        lines.push(if changes.is_empty() {
            "Native plan: no additional packages".into()
        } else {
            format!("Native plan — additional changes:\n{}", changes.join("\n"))
        });
        lines.push(format!(
            "Download: {} · Disk impact: {} · Restart: {}",
            plan.download_bytes
                .map_or_else(|| "unknown".into(), |n| format!("{n} bytes")),
            plan.disk_bytes
                .map_or_else(|| "unknown".into(), |n| format!("{n:+} bytes")),
            plan.restart_required.map_or("unknown", |needed| if needed {
                "required"
            } else {
                "not indicated"
            })
        ));
    } else if matches!(
        operation,
        Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
    ) {
        lines.push("Transaction preview unavailable; additional changes are unknown.".into());
    }
    if matches!(
        operation,
        Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
    ) {
        lines.push("Data retention: manager-specific; details unavailable".into());
    } else if matches!(operation, Operation::Clean(_)) {
        lines.push("Data removal: see native cleanup preview".into());
    }
    if matches!(
        operation,
        Operation::Install(_)
            | Operation::Remove(_)
            | Operation::Upgrade(_)
            | Operation::UpgradeAll { .. }
            | Operation::Refresh { .. }
    ) {
        lines.push("System authorization may be requested.".into());
    }
    if title.chars().count() > 36 {
        title = format!("{}…", title.chars().take(35).collect::<String>());
    }
    json!({"action": title, "body": lines.join("\n\n")})
}
fn package_row(p: &Package, same_from: &[String], same_group: Option<&str>) -> Value {
    json!({"name": p.id.name, "display_name": p.display_name, "source": p.id.backend, "architecture": p.id.architecture,
        "remote": p.id.remote, "reference": p.id.reference, "scope": p.id.scope, "scope_label": scope_label(&p.id.scope), "summary": p.summary, "installed": p.installed_version,
        "candidate": p.candidate_version, "update": p.update, "kind": "package", "icon": p.icon,
        "component_ids": p.component_ids,
        "same_app_from": same_from, "same_app_group": same_group})
}
fn update_detail_name(
    packages: &mut [Package],
    rows: &str,
    detail: &Package,
) -> Option<Vec<Value>> {
    if detail.display_name.is_empty() {
        return None;
    }
    let index = packages.iter().position(|package| {
        package.id == detail.id && package.display_name != detail.display_name
    })?;
    let mut rows: Vec<Value> = serde_json::from_str(rows).ok()?;
    rows.get_mut(index)?
        .as_object_mut()?
        .insert("display_name".into(), json!(detail.display_name));
    packages[index]
        .display_name
        .clone_from(&detail.display_name);
    Some(rows)
}

impl ffi::PackageController {
    pub fn set_autostart(mut self: Pin<&mut Self>, enabled: bool) -> bool {
        let result = self
            .rust()
            .autostart_path
            .clone()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No user configuration directory",
                )
            })
            .and_then(|path| background::set_autostart(&path, enabled));
        match result {
            Ok(()) => true,
            Err(error) => {
                self.as_mut().set_status(
                    format!("Autostart could not be changed: {error}")
                        .as_str()
                        .into(),
                );
                false
            }
        }
    }
    pub fn check_updates(
        mut self: Pin<&mut Self>,
        sources: QString,
        enabled: bool,
        offline: bool,
        metered: bool,
        force: bool,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let busy = self.rust().worker.is_some()
            || !self.rust().confirmed_queue.is_empty()
            || self.rust().pending.is_some();
        if !self
            .as_mut()
            .rust_mut()
            .background_schedule
            .ready(now, enabled, offline, metered, busy, force)
        {
            return;
        }
        let sources: Vec<String> = sources
            .to_string()
            .split(',')
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect();
        if sources
            .iter()
            .any(|id| !pkgdeck_core::backends::BACKEND_IDS.contains(&id.as_str()))
        {
            return;
        }
        self.as_mut().rust_mut().background = true;
        self.start(Job::BackgroundUpdates(sources));
    }
    fn finish_background_check(mut self: Pin<&mut Self>, report: PackageReport) {
        let result = self
            .as_mut()
            .rust_mut()
            .background_schedule
            .complete(&report);
        let checked = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let failures: Vec<_> = report
            .failures
            .iter()
            .map(|failure| json!({"source": failure.backend, "kind": failure_kind(&failure.error)}))
            .collect();
        self.as_mut().set_background_state(encoded(json!({"last_check": checked, "available": result.count, "failures": failures, "notify": result.changed && result.count > 0})));
    }
    pub fn refresh_activity(mut self: Pin<&mut Self>) {
        if let Some(store) = self.rust().activity_store.clone() {
            let _ = store.recover_dead();
            if let Ok(entries) = store.entries() {
                self.as_mut().set_activity(encoded(entries));
            }
        }
    }
    fn start_confirmed(mut self: Pin<&mut Self>, entry: Confirmed) {
        if let (Some(store), Some(id)) = (self.rust().activity_store.clone(), entry.activity_id) {
            let _ = store.state(id, State::Running);
        }
        self.as_mut().rust_mut().active_activity_id = entry.activity_id;
        self.as_mut().refresh_activity();
        self.start(entry.job);
    }
    fn accept_confirmed(mut self: Pin<&mut Self>, job: Job) {
        let active = self.rust().worker.as_ref().map(|worker| &worker.job);
        if let Some(message) = queue_conflict(
            &job,
            active,
            &self.rust().confirmed_queue,
            self.rust().revalidating.as_ref(),
            self.rust().validated_confirmed.as_ref(),
        ) {
            self.set_status(message.into());
            return;
        }
        let operations = job.operations();
        let activity_id = self
            .rust()
            .activity_store
            .clone()
            .and_then(|store| store.begin("gui", operations, State::Queued).ok());
        let cleanup_preview = if let Job::CleanAll(operations) = &job {
            self.rust()
                .cleanup
                .iter()
                .filter(|item| operations.contains(&Operation::Clean(item.id.clone())))
                .cloned()
                .collect()
        } else {
            vec![]
        };
        let entry = Confirmed {
            job,
            activity_id,
            cleanup_preview,
        };
        if self.rust().worker.is_some() || !self.rust().confirmed_queue.is_empty() {
            self.as_mut().rust_mut().confirmed_queue.push_back(entry);
            self.as_mut().refresh_activity();
            self.as_mut().set_status("Operation queued.".into());
            self.as_mut().set_busy(true);
            if let Some(worker) = &self.rust().worker {
                if !worker.job.writes() {
                    worker.cancel.cancel();
                }
            }
        } else {
            self.start_confirmed(entry);
        }
    }
    fn validate_confirmed(mut self: Pin<&mut Self>, entry: Confirmed) {
        let plan = match &entry.job {
            Job::Write(operation, _) => Job::PlanOperation(operation.clone()),
            Job::UpgradeAll(operations, _) => {
                Job::PlanUpgrade(operations.clone(), operations.len())
            }
            Job::CleanAll(operations) => Job::PlanCleanAll(operations.clone()),
            _ => {
                self.start_confirmed(entry);
                return;
            }
        };
        self.as_mut().rust_mut().revalidating = Some(entry);
        self.start(plan);
    }
    fn fail_revalidation(mut self: Pin<&mut Self>, entry: Confirmed) {
        if let (Some(store), Some(id)) = (self.rust().activity_store.clone(), entry.activity_id) {
            let outcomes = entry
                .job
                .operations()
                .iter()
                .map(|_| Outcome::Failed)
                .collect();
            let _ = store.finish(id, outcomes);
        }
        self.as_mut().refresh_activity();
    }
    pub fn cancel_queued(mut self: Pin<&mut Self>) {
        let mut cancelled = Vec::new();
        while let Some(entry) = self.as_mut().rust_mut().confirmed_queue.pop_front() {
            cancelled.push(entry);
        }
        if let Some(entry) = self.as_mut().rust_mut().revalidating.take() {
            cancelled.push(entry);
            self.as_mut().rust_mut().discard_revalidation = true;
            if let Some(worker) = &self.rust().worker {
                worker.cancel.cancel();
            }
        }
        if let Some(entry) = self.as_mut().rust_mut().validated_confirmed.take() {
            cancelled.push(entry);
        }
        for entry in cancelled {
            if let (Some(store), Some(id)) = (self.rust().activity_store.clone(), entry.activity_id)
            {
                let outcomes = entry
                    .job
                    .operations()
                    .iter()
                    .map(|_| Outcome::Cancelled)
                    .collect();
                let _ = store.finish(id, outcomes);
            }
        }
        self.as_mut().refresh_activity();
    }
    fn successful_sources(&self) -> Vec<String> {
        let state: Value =
            serde_json::from_str(&self.report_state().to_string()).unwrap_or_default();
        serde_json::from_value(state["successful_sources"].clone()).unwrap_or_default()
    }
    fn set_phase(mut self: Pin<&mut Self>, phase: &str) {
        let mut state: Value =
            serde_json::from_str(&self.report_state().to_string()).unwrap_or_else(|_| json!({}));
        state["phase"] = json!(phase);
        self.as_mut().set_report_state(encoded(state));
    }
    fn set_package_report_state(mut self: Pin<&mut Self>, report: &PackageReport, loading: bool) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let retried = self.rust().retrying_source.clone();
        for source in report
            .successful_sources
            .iter()
            .filter(|source| retried.as_ref().is_none_or(|id| id == *source))
        {
            self.as_mut()
                .rust_mut()
                .last_success
                .insert(source.clone(), now);
        }
        let failures: Vec<_> = report
            .failures
            .iter()
            .filter(|failure| read_failed(failure))
            .map(|failure| {
                json!({
                    "source": failure.backend, "kind": failure_kind(&failure.error),
                    "detail": failure.error.to_string()
                })
            })
            .collect();
        let phase = if loading {
            "loading"
        } else if failures.is_empty()
            && report.successful_sources.is_empty()
            && !report.failures.is_empty()
        {
            "unsupported"
        } else if failures.is_empty() {
            "complete"
        } else if report.successful_sources.is_empty() {
            "failed"
        } else {
            "partial"
        };
        let last_success = self.rust().last_success.clone();
        self.as_mut().set_report_state(encoded(json!({
            "phase": phase, "failures": failures,
            "successful_sources": report.successful_sources,
            "last_success": last_success
        })));
    }
    fn start(mut self: Pin<&mut Self>, job: Job) {
        if let Some(worker) = &self.rust().worker {
            if self.rust().background {
                worker.cancel.cancel();
                self.as_mut().rust_mut().queued = Some(job);
                // A confirmed foreground request preempts background
                // loading: report busy at once so the UI waits for the
                // queued job instead of reading idle state while the
                // background worker winds down.
                self.as_mut().set_busy(true);
            }
            return;
        }
        let source_filter = self.rust().source_filter.clone();
        let authorization = if self.rust().sudo {
            Authorization::SudoNonInteractive
        } else {
            Authorization::Polkit
        };
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        let worker_job = job.clone();
        // Loads rebuild for fresh discovery; the previous engine is dropped.
        // Details can reuse a warm engine; mutation jobs always rediscover.
        let cached = self.as_mut().rust_mut().engine.take();
        let cleanup = self.rust().cleanup.clone();
        // Keep read snapshots while a native write owns the worker. The UI
        // can browse them, clearly marked stale, until the queue drains.
        let handle = thread::spawn(move || {
            let sources_view = matches!(&job, Job::Load(view, _) | Job::RetrySource(view, ..) if view == "Sources");
            let mut send = |mut reply| {
                match &mut reply {
                    Reply::Partial(report)
                    | Reply::Inventory(report)
                    | Reply::Done(Ok(
                        Payload::Packages(report) | Payload::RetryPackages(_, report),
                    )) => {
                        for package in &mut report.packages {
                            crate::metadata::enrich(package);
                        }
                    }
                    Reply::Done(Ok(Payload::Details(details))) => {
                        crate::metadata::enrich(&mut details.package);
                        let _ = sender.send(Reply::DetailsPreview(details.clone()));
                        crate::metadata::details(&mut details.package, &token)
                    }
                    _ => {}
                }
                let _ = sender.send(reply);
            };
            if let Job::Repositories(action) = &job {
                let transport = pkgdeck_core::backends::NativeTransport {
                    host: pkgdeck_core::host::Host::current(),
                    authorization,
                };
                let result = if let Some(reason) = transport.host.runtime.disabled_reason() {
                    Err(pkgdeck_core::process::ExecutionError::Disabled(reason.into()).into())
                } else {
                    action
                        .as_ref()
                        .map(|action| repositories::apply(&transport, action, &token))
                        .transpose()
                        .map(|_| {
                            Payload::Repositories(repositories::list(
                                &transport,
                                std::path::Path::new("/"),
                                &token,
                            ))
                        })
                };
                send(Reply::Done(result));
                return;
            }
            // Fast path: Details against a warm engine reuse detected state
            // outright. A filter change since discovery surfaces as
            // UnknownBackend and falls through to the scoped rebuild below.
            if let (Job::Details(id), Some(mut engine)) = (&job, cached) {
                match engine.details_reuse(id, &token) {
                    Ok(details) => {
                        send(Reply::Done(Ok(Payload::Details(Box::new(details)))));
                        send(Reply::Engine(Box::new(engine)));
                        return;
                    }
                    Err(EngineError::UnknownBackend(_)) => {}
                    Err(error) => {
                        send(Reply::Done(Err(error)));
                        send(Reply::Engine(Box::new(engine)));
                        return;
                    }
                }
            }
            match pkgdeck_core::backends::native_engine(
                &engine_source(&job, &source_filter),
                sources_view,
                authorization,
                &token,
            ) {
                Ok(mut engine) => {
                    for item in cleanup {
                        engine.remember_cleanup_plan(item);
                    }
                    if let Job::UpgradeAll(_, Some(plan)) = &job {
                        engine.remember_apt_upgrade_plan(plan.clone());
                    }
                    if let Job::Write(_, Some(plan)) = &job {
                        engine.remember_operation_plan((**plan).clone());
                    }
                    execute(&mut engine, job, &token, &mut send);
                    send(Reply::Engine(Box::new(engine)));
                }
                Err(error) => send(Reply::Done(Err(error))),
            }
        });
        let writing = worker_job.writes();
        self.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel,
            job: worker_job,
        });
        let foreground = !self.rust().background;
        self.as_mut().set_busy(foreground);
        self.set_writing(writing);
    }
    pub fn load(
        mut self: Pin<&mut Self>,
        view: QString,
        query: QString,
        sources: QString,
        sudo: bool,
        force: bool,
    ) {
        let writing = self.rust().worker.as_ref().is_some_and(|w| w.job.writes());
        let view = view.to_string();
        let query = query.to_string();
        // Comma-joined checked source ids; empty means every available source.
        let sources: Vec<String> = sources
            .to_string()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !["Search", "Installed", "Updates", "Sources", "Clean"].contains(&view.as_str())
            || (view == "Search" && query.trim().is_empty())
        {
            return;
        }
        const KNOWN: &[&str] = pkgdeck_core::backends::BACKEND_IDS;
        if sources.iter().any(|s| !KNOWN.contains(&s.as_str())) {
            self.set_status("Unknown source.".into());
            return;
        }
        let key = cache_key(&view, &query, &sources, sudo);
        if writing {
            self.as_mut().rust_mut().source_filter = sources;
            self.as_mut().rust_mut().sudo = sudo;
            if let Some(cached) = self.rust().view_cache.get(&key).cloned() {
                self.as_mut().rust_mut().packages = cached.packages;
                self.as_mut().rust_mut().cleanup = cached.cleanup;
                self.as_mut().rust_mut().failures = cached.failures;
                self.as_mut().rust_mut().sources = cached.sources;
                self.as_mut().rust_mut().updates_view = cached.updates_view;
                self.as_mut().set_upgradable(cached.upgradable);
                self.as_mut().set_rows(cached.rows);
                self.as_mut().set_details("{}".into());
                self.as_mut().set_report_state(cached.report_state);
                self.as_mut().set_phase("stale");
            } else {
                self.as_mut().rust_mut().packages.clear();
                self.as_mut().rust_mut().cleanup.clear();
                self.as_mut().rust_mut().sources.clear();
                self.as_mut().set_rows("[]".into());
                self.as_mut().set_details("{}".into());
                self.as_mut().set_phase("loading");
            }
            self.as_mut().rust_mut().deferred_load = Some(Job::Load(view, query));
            return;
        }
        let changed = self.rust().source_filter != sources || self.rust().sudo != sudo;
        if force || changed {
            self.as_mut().rust_mut().prefetched.clear();
            self.as_mut().rust_mut().prefetch = prefetch_views();
        }
        if !force {
            if let Some((loaded, payload)) = self.as_mut().rust_mut().prefetched.remove(&key) {
                if loaded.elapsed() < VIEW_TTL {
                    self.as_mut().rust_mut().updates_view = view == "Updates";
                    self.as_mut().rust_mut().queued = None;
                    self.as_mut().rust_mut().packages.clear();
                    self.as_mut().rust_mut().cleanup.clear();
                    self.as_mut().rust_mut().sources.clear();
                    self.as_mut().rust_mut().failures.clear();
                    self.as_mut().apply(Ok(payload));
                    let upgradable = view == "Updates"
                        && self
                            .rust()
                            .failures
                            .iter()
                            .all(|failure| !read_failed(failure))
                        && !upgrade_plan(&self.rust().packages).is_empty();
                    self.as_mut().set_upgradable(upgradable);
                    self.as_mut().stash_current(key.clone());
                }
            }
            if let Some(cached) = self.rust().view_cache.get(&key).cloned() {
                self.as_mut().set_upgradable(cached.upgradable);
                self.as_mut().rust_mut().updates_view = cached.updates_view;
                self.as_mut().rust_mut().source_filter = sources;
                self.as_mut().rust_mut().sudo = sudo;
                self.as_mut().rust_mut().packages = cached.packages;
                self.as_mut().rust_mut().cleanup = cached.cleanup;
                self.as_mut().rust_mut().failures = cached.failures;
                self.as_mut().rust_mut().sources = cached.sources;
                self.as_mut().rust_mut().pending = None;
                self.as_mut().rust_mut().queued = None;
                self.as_mut().rust_mut().selected = None;
                self.as_mut().set_confirmation(QString::default());
                self.as_mut().set_rows(cached.rows);
                self.as_mut().set_details("{}".into());
                self.as_mut().set_status(cached.status);
                self.as_mut().set_report_state(cached.report_state);
                self.as_mut()
                    .set_phase(if cached.loaded.elapsed() >= VIEW_TTL {
                        "stale"
                    } else {
                        "cached"
                    });
                self.as_mut().set_busy(false);
                if let Some(worker) = &self.rust().worker {
                    worker.cancel.cancel();
                    // Suppress every reply from the previous section.
                    self.as_mut().rust_mut().background = true;
                }
                if cached.loaded.elapsed() >= VIEW_TTL {
                    self.as_mut().start(Job::Load(view, query));
                }
                return;
            }
        }
        self.as_mut().set_upgradable(false);
        self.as_mut().rust_mut().updates_view = view == "Updates";
        self.as_mut().rust_mut().source_filter = sources;
        self.as_mut().rust_mut().sudo = sudo;
        // Details survive view switches so re-selecting a package is
        // instant; only an explicit reload or a write may invalidate them.
        if force {
            crate::metadata::invalidate();
            self.as_mut().rust_mut().view_cache.clear();
            self.as_mut().rust_mut().prefetched.clear();
            self.as_mut().rust_mut().prefetch = prefetch_views();
            self.as_mut().rust_mut().detail_cache.clear();
        }
        self.as_mut().rust_mut().packages.clear();
        self.as_mut().rust_mut().cleanup.clear();
        self.as_mut().rust_mut().failures.clear();
        self.as_mut().rust_mut().sources.clear();
        self.as_mut().rust_mut().pending = None;
        self.as_mut().rust_mut().queued = None;
        self.as_mut().rust_mut().selected = None;
        self.as_mut().set_confirmation(QString::default());
        self.as_mut().set_rows("[]".into());
        self.as_mut().set_details("{}".into());
        self.as_mut().set_phase("loading");
        // Reads are preemptible: cancel the in-flight load or details
        // query and run this one next. Native writes keep the lock. Report
        // busy at once: the fresh rows are not on screen yet, and a silent
        // gap here reads as idle while the requested view is still pending.
        if self.rust().worker.is_some() {
            self.rust().worker.as_ref().unwrap().cancel.cancel();
            self.as_mut().rust_mut().queued = Some(Job::Load(view, query));
            self.as_mut().set_busy(true);
            return;
        }
        self.start(Job::Load(view, query));
    }
    pub fn retry_source(mut self: Pin<&mut Self>, view: QString, query: QString, source: QString) {
        if self.rust().worker.is_some() && !self.rust().background || self.rust().pending.is_some()
        {
            return;
        }
        let (view, query, source) = (view.to_string(), query.to_string(), source.to_string());
        if !["Search", "Installed", "Updates", "Clean", "Sources"].contains(&view.as_str())
            || !pkgdeck_core::backends::BACKEND_IDS.contains(&source.as_str())
            || !self.rust().source_filter.is_empty() && !self.rust().source_filter.contains(&source)
        {
            return;
        }
        self.as_mut().set_phase("loading");
        self.start(Job::RetrySource(view, query, source));
    }
    /// Snapshot the current table under `key`. Called after a terminal view
    /// report lands; the caller guarantees no newer load superseded it.
    fn stash_current(self: Pin<&mut Self>, key: String) {
        let view = CachedView {
            loaded: Instant::now(),
            cleanup: self.rust().cleanup.clone(),
            packages: self.rust().packages.clone(),
            failures: self.rust().failures.clone(),
            sources: self.rust().sources.clone(),
            rows: self.rust().rows.clone(),
            status: self.rust().status.clone(),
            report_state: self.rust().report_state.clone(),
            upgradable: self.rust().upgradable,
            updates_view: self.rust().updates_view,
        };
        self.rust_mut().view_cache.insert(key, view);
    }
    pub fn select(mut self: Pin<&mut Self>, index: i32) {
        if let Some(package) = usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().packages.get(i))
            .cloned()
        {
            self.as_mut().rust_mut().selected = Some(package.id.clone());
            if let Some(details) = self.rust().detail_cache.get(&package.id).cloned() {
                self.as_mut().set_details(details);
                self.set_status("Package details loaded.".into());
                return;
            }
            // A running Details job is stale the moment the selection moves:
            // cancel it and queue the new identity. A selection that lands
            // while rows stream in queues behind the load instead. Writes
            // are never preempted; selections made mid-write highlight
            // without loading details until the write finishes.
            if let Some(worker) = &self.rust().worker {
                if matches!(worker.job, Job::Details(_)) {
                    worker.cancel.cancel();
                    self.as_mut().rust_mut().queued = Some(Job::Details(package.id));
                    return;
                }
                if matches!(worker.job, Job::Load(..)) {
                    if self.rust().background {
                        worker.cancel.cancel();
                    }
                    self.as_mut().rust_mut().queued = Some(Job::Details(package.id));
                    return;
                }
                let same = same_app_sources(&self.rust().packages, &package.id);
                self.as_mut().set_details(encoded(json!({"package": package_row(&package, &same, None), "description": package.summary})));
                return;
            }
            let same = same_app_sources(&self.rust().packages, &package.id);
            self.as_mut().set_details(encoded(
                json!({"package": package_row(&package, &same, None), "description": package.summary}),
            ));
            self.start(Job::Details(package.id));
        } else if let Some(item) = usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().cleanup.get(i))
            .cloned()
        {
            self.as_mut().set_details(encoded(json!({"cleanup": item})));
            self.set_status("Cleanup preview loaded.".into());
        } else if let Some(failure) = usize::try_from(index)
            .ok()
            .and_then(|i| {
                i.checked_sub(self.rust().packages.len() + self.rust().cleanup.len())
                    .and_then(|j| self.rust().failures.get(j))
            })
            .cloned()
        {
            self.as_mut().set_details(failure_details(&failure));
            self.set_status("Source failure details.".into());
        } else if let Some(source) = usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().sources.get(i))
        {
            let detail = json!({"source": source.backend, "availability": source_status(source), "capabilities": source.capabilities});
            self.set_details(encoded(detail));
        }
    }
    pub fn load_repositories(self: Pin<&mut Self>) {
        if self.rust().worker.is_none() || self.rust().background {
            self.start(Job::Repositories(None));
        }
    }
    pub fn change_repository(mut self: Pin<&mut Self>, action: QString) {
        let action = match repositories::parse_action(&action.to_string()) {
            Ok(action) => action,
            Err(error) => {
                self.set_status(
                    format!("Invalid repository request: {error}")
                        .as_str()
                        .into(),
                );
                return;
            }
        };
        if let Err(error) = action.validate() {
            self.set_status(error.to_string().as_str().into());
            return;
        }
        if matches!(action.change, repositories::Change::OpenEditor) {
            self.start(Job::Repositories(Some(action)));
            return;
        }
        self.as_mut()
            .set_confirmation_data(encoded(json!({"action":"Apply", "body": action.label()})));
        self.as_mut().set_confirmation(
            format!("{}\n\nApply this repository change?", action.label())
                .as_str()
                .into(),
        );
        self.rust_mut().pending = Some(Job::Repositories(Some(action)));
    }
    pub fn propose(mut self: Pin<&mut Self>, action: QString, index: i32) {
        let writing = self
            .rust()
            .worker
            .as_ref()
            .is_some_and(|worker| worker.job.writes());
        let action = action.to_string();
        if action == "upgrade-all" {
            if !self.upgradable {
                return;
            }
            let operations = upgrade_plan(&self.rust().packages);
            let count = self
                .rust()
                .packages
                .iter()
                .filter(|package| {
                    package.installed_version.is_some()
                        && package.update == UpdateAvailability::Available
                })
                .count();
            if !writing && operations.iter().any(|operation| {
                matches!(operation, Operation::UpgradeAll { backend } if backend == "apt")
            }) {
                self.start(Job::PlanUpgrade(operations, count));
            } else {
                self.as_mut()
                    .apply(Ok(Payload::UpgradePreview(operations, count, None)));
            }
            return;
        }
        if action == "clean-all" {
            let operations: Vec<_> = self
                .rust()
                .cleanup
                .iter()
                .map(|item| Operation::Clean(item.id.clone()))
                .collect();
            if operations.is_empty() {
                return;
            }
            let labels = self
                .rust()
                .cleanup
                .iter()
                .map(|item| format!("{} ({})\n{}", item.title, item.id.backend, item.preview))
                .collect::<Vec<_>>()
                .join("\n\n");
            self.as_mut().set_confirmation_data(encoded(json!({"action":format!("Clean {} tasks", operations.len()), "body": format!("{labels}\n\nTasks run in order. Completed tasks cannot be undone.")})));
            self.as_mut().set_confirmation(format!("Run {} cleanup tasks?\n\n{labels}\n\nTasks run in order. Completed tasks cannot be undone.", operations.len()).as_str().into());
            self.rust_mut().pending = Some(Job::CleanAll(operations));
            return;
        }
        let operation = usize::try_from(index).ok().and_then(|i| {
            if action == "refresh" {
                self.rust()
                    .sources
                    .get(i)
                    .filter(|s| {
                        s.availability == Ok(Availability::Available)
                            && s.capabilities.contains(&Capability::Refresh)
                    })
                    .map(|s| Operation::Refresh {
                        backend: s.backend.clone(),
                    })
            } else if action == "clean" {
                self.rust()
                    .cleanup
                    .get(i)
                    .map(|item| Operation::Clean(item.id.clone()))
            } else {
                self.rust()
                    .packages
                    .get(i)
                    .and_then(|p| match action.as_str() {
                        "install"
                            if !pkgdeck_core::backends::update_only(&p.id.backend)
                                && p.installed_version.is_none() =>
                        {
                            Some(Operation::Install(p.id.clone()))
                        }
                        "remove"
                            if !pkgdeck_core::backends::update_only(&p.id.backend)
                                && p.installed_version.is_some() =>
                        {
                            Some(Operation::Remove(p.id.clone()))
                        }
                        "upgrade"
                            if p.update == UpdateAvailability::Available
                                || (matches!(p.id.backend.as_str(), "docker" | "podman")
                                    && p.installed_version.is_some()
                                    && p.id.reference.is_some()) =>
                        {
                            Some(Operation::Upgrade(p.id.clone()))
                        }
                        _ => None,
                    })
            }
        });
        match operation {
            Some(op)
                if op.backend() == "apt"
                    && !writing
                    && matches!(
                        op,
                        Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
                    ) =>
            {
                self.start(Job::PlanOperation(op));
            }
            Some(op) => self.as_mut().apply(Ok(Payload::OperationPreview(op, None))),
            None => {
                self.as_mut().set_confirmation(QString::default());
                self.as_mut().set_confirmation_data("{}".into());
                self.rust_mut().pending = None;
            }
        }
    }
    /// Queue an upgrade of the checked Updates rows. Identities are
    /// re-resolved against the current package list: stale rows (moved on
    /// or vanished while streaming) are skipped, never guessed. An empty
    /// resolution clears any pending confirmation and says so in status.
    pub fn propose_checked(mut self: Pin<&mut Self>, identities: QString) {
        let plan = plan_checked_upgrade(&self.rust().packages, &identities.to_string());
        if plan.operations.is_empty() {
            self.as_mut().rust_mut().pending = None;
            self.as_mut().set_confirmation(QString::default());
            self.as_mut().set_confirmation_data("{}".into());
        } else {
            self.as_mut().set_confirmation_data(encoded(
                json!({"action":format!("Update {} packages", plan.operations.len()), "body": plan.confirmation}),
            ));
            self.as_mut()
                .set_confirmation(plan.confirmation.as_str().into());
        }
        if let Some(status) = plan.status {
            self.as_mut().set_status(status.as_str().into());
        }
        if !plan.operations.is_empty() {
            self.rust_mut().pending = Some(Job::UpgradeAll(plan.operations, None));
        }
    }
    pub fn confirm(mut self: Pin<&mut Self>, approved: bool) {
        let pending = self.as_mut().rust_mut().pending.take();
        self.as_mut().set_confirmation(QString::default());
        self.as_mut().set_confirmation_data("{}".into());
        if approved {
            if let Some(op) = pending {
                self.accept_confirmed(op);
            }
        }
    }
    pub fn cancel(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().prefetch.clear();
        self.as_mut().cancel_queued();
        self.as_mut().rust_mut().deferred_load = None;
        if self.rust().worker.is_some() {
            // An explicit cancellation also drops any queued selection: the
            // user asked everything to stop, not to continue afterwards.
            self.as_mut().rust_mut().queued = None;
        }
        if let Some(worker) = &self.rust().worker {
            worker.cancel.cancel();
            self.as_mut().set_status(
                "Cancellation requested; waiting for the native operation to finish safely.".into(),
            );
        }
    }
    fn apply(mut self: Pin<&mut Self>, result: Result<Payload, EngineError>) {
        // A newer view or query owns the next visible report. A cancelled
        // worker can still race one final partial into the channel, so none
        // of its successful payloads may replace the new query's empty state.
        if matches!(self.rust().queued, Some(Job::Load(..))) && result.is_ok() {
            return;
        }
        match result {
            Ok(Payload::BackgroundUpdates(report)) => self.as_mut().finish_background_check(report),
            Err(e) => {
                if let Some(entry) = self.as_mut().rust_mut().revalidating.take() {
                    self.as_mut().fail_revalidation(entry);
                }
                // A superseded Details job ends cancelled once its replacement
                // is queued; that abort carries no news worth flashing.
                let superseded =
                    matches!(e, EngineError::Cancelled) && self.rust().queued.is_some();
                if !superseded {
                    self.set_status(e.to_string().as_str().into());
                }
            }
            Ok(Payload::UpgradePreview(operations, count, apt_plan)) => {
                if let Some(entry) = self.as_mut().rust_mut().revalidating.take() {
                    if matches!(&entry.job, Job::UpgradeAll(previous, plan) if previous == &operations && plan == &apt_plan)
                    {
                        self.as_mut().rust_mut().validated_confirmed = Some(entry);
                        return;
                    }
                    self.as_mut().fail_revalidation(entry);
                }
                let labels = operations
                    .iter()
                    .map(|operation| confirmation_label(operation, &self.rust().packages))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let apt = apt_plan.as_ref().map_or_else(String::new, |plan| {
                    format!("\n\nAPT transaction:\n{}", plan.summary())
                });
                self.as_mut().set_confirmation_data(encoded(json!({"action":format!("Update {count} packages"), "body": format!("{count} listed packages{apt}\n\n{labels}")})));
                self.as_mut().set_confirmation(
                    format!("Update all {count} listed packages?{apt}\n\n{labels}\n\nContinue?")
                        .as_str()
                        .into(),
                );
                self.rust_mut().pending = Some(Job::UpgradeAll(operations, apt_plan));
            }
            Ok(Payload::OperationPreview(operation, plan)) => {
                if let Some(entry) = self.as_mut().rust_mut().revalidating.take() {
                    if matches!(&entry.job, Job::Write(previous, reviewed) if previous == &operation && reviewed == &plan)
                    {
                        self.as_mut().rust_mut().validated_confirmed = Some(entry);
                        return;
                    }
                    self.as_mut().fail_revalidation(entry);
                }
                let data = confirmation_preview(
                    &operation,
                    &self.rust().packages,
                    &self.rust().cleanup,
                    plan.as_deref(),
                );
                let body = data["body"].as_str().unwrap_or_default().to_owned();
                self.as_mut().set_confirmation_data(encoded(data));
                self.as_mut().set_confirmation(body.as_str().into());
                self.rust_mut().pending = Some(Job::Write(operation, plan));
            }
            Ok(Payload::CleanPreview(operations, fresh)) => {
                if let Some(entry) = self.as_mut().rust_mut().revalidating.take() {
                    let selected: Vec<_> = fresh
                        .iter()
                        .filter(|item| operations.contains(&Operation::Clean(item.id.clone())))
                        .cloned()
                        .collect();
                    if selected == entry.cleanup_preview {
                        self.as_mut().rust_mut().cleanup = fresh;
                        self.as_mut().rust_mut().validated_confirmed = Some(entry);
                        return;
                    }
                    self.as_mut().fail_revalidation(entry);
                    self.as_mut().rust_mut().cleanup = fresh;
                    if selected.is_empty() {
                        self.as_mut()
                            .set_status("Queued cleanup is no longer available.".into());
                        return;
                    }
                    let operations: Vec<_> = selected
                        .iter()
                        .map(|item| Operation::Clean(item.id.clone()))
                        .collect();
                    let body = selected
                        .iter()
                        .map(|item| {
                            format!("{} ({})\n{}", item.title, item.id.backend, item.preview)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    self.as_mut().set_confirmation_data(encoded(json!({"action": format!("Clean {} tasks", operations.len()), "body": body})));
                    self.as_mut().set_confirmation(body.as_str().into());
                    self.as_mut().rust_mut().pending = Some(Job::CleanAll(operations));
                }
            }
            Ok(Payload::RetryPackages(source, retry)) => {
                self.as_mut().rust_mut().view_cache.clear();
                self.as_mut().rust_mut().prefetched.clear();
                let mut packages = self.rust().packages.clone();
                packages.retain(|package| package.id.backend != source);
                packages.extend(retry.packages);
                packages.sort_by(|a, b| a.id.cmp(&b.id));
                let mut failures = self.rust().failures.clone();
                failures.retain(|failure| failure.backend != source);
                failures.extend(retry.failures);
                let mut successful_sources = self.successful_sources();
                successful_sources.retain(|id| id != &source);
                successful_sources.extend(retry.successful_sources);
                successful_sources.sort();
                self.as_mut().rust_mut().retrying_source = Some(source.clone());
                self.as_mut().apply(Ok(Payload::Packages(PackageReport {
                    packages,
                    failures,
                    successful_sources,
                })));
                self.as_mut().rust_mut().retrying_source = None;
            }
            Ok(Payload::RetryCleanup(source, retry)) => {
                self.as_mut().rust_mut().view_cache.clear();
                self.as_mut().rust_mut().prefetched.clear();
                let mut items = self.rust().cleanup.clone();
                items.retain(|item| item.id.backend != source);
                items.extend(retry.items);
                items.sort_by(|a, b| a.id.cmp(&b.id));
                let mut failures = self.rust().failures.clone();
                failures.retain(|failure| failure.backend != source);
                failures.extend(retry.failures);
                self.as_mut()
                    .apply(Ok(Payload::Cleanup(CleanupReport { items, failures })));
            }
            Ok(Payload::RetrySources(source, retry)) => {
                self.as_mut().rust_mut().view_cache.clear();
                self.as_mut().rust_mut().prefetched.clear();
                let mut sources = self.rust().sources.clone();
                sources.retain(|row| row.backend != source);
                sources.extend(retry);
                sources.sort_by(|a, b| a.backend.cmp(&b.backend));
                self.as_mut().apply(Ok(Payload::Sources(sources)));
            }
            Ok(Payload::Packages(report)) => {
                if matches!(self.rust().queued, Some(Job::Load(..))) {
                    return;
                }
                // Upgrade gating waits for the terminal report, when no
                // worker remains: partials cannot promise complete
                // failures, so an early partial must not enable it. Rows,
                // status, and failures below update on every partial.
                if self.rust().worker.is_none() {
                    let upgradable = self.rust().updates_view
                        && report.failures.iter().all(|failure| !read_failed(failure))
                        && !upgrade_plan(&report.packages).is_empty();
                    self.as_mut().set_upgradable(upgradable);
                }
                let loading = self.rust().worker.is_some();
                self.as_mut().set_package_report_state(&report, loading);
                let same = same_app_sources_all(&report.packages);
                let groups = same_app_group_keys_all(&report.packages);
                let mut rows: Vec<_> = report
                    .packages
                    .iter()
                    .zip(same.into_iter().zip(groups))
                    .map(|(p, (from, group))| package_row(p, &from, group.as_deref()))
                    .collect();
                rows.extend(report.failures.iter().map(|failure| {
                    json!({"kind": "failure", "name": failure.backend, "source": failure.backend,
                        "summary": failure.error.to_string(), "failure_kind": failure_kind(&failure.error), "available": false})
                }));
                let failed: Vec<_> = report
                    .failures
                    .iter()
                    .filter(|failure| read_failed(failure))
                    .collect();
                let status = if failed.is_empty() {
                    format!("{} packages", report.packages.len())
                } else {
                    format!(
                        "{} packages\n{}",
                        report.packages.len(),
                        failed
                            .iter()
                            .map(|f| format!("{}: {}", f.backend, f.error))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                self.as_mut().rust_mut().packages = report.packages;
                self.as_mut().rust_mut().failures = report.failures;
                self.as_mut().set_rows(encoded(rows));
                self.set_status(status.as_str().into());
            }
            Ok(Payload::Cleanup(report)) => {
                let empty = report.items.is_empty();
                let mut rows: Vec<_> = report
                    .items
                    .iter()
                    .map(|item| {
                        json!({
                            "kind": "cleanup", "name": item.title, "display_name": item.title,
                            "source": item.id.backend, "summary": item.summary,
                            "cleanup_key": item.id.key, "cleanup_kind": item.kind,
                            "preview": item.preview
                        })
                    })
                    .collect();
                rows.extend(report.failures.iter().map(|failure| {
                    json!({
                        "kind": "failure", "name": failure.backend, "source": failure.backend,
                        "summary": failure.error.to_string(), "available": false
                    })
                }));
                let incomplete = !report.failures.is_empty();
                let failures: Vec<_> = report.failures.iter().map(|failure| json!({"source": failure.backend, "kind": failure_kind(&failure.error), "detail": failure.error.to_string()})).collect();
                let last_success = self.rust().last_success.clone();
                self.as_mut().set_report_state(encoded(json!({"phase": if incomplete { if empty { "failed" } else { "partial" } } else { "complete" }, "failures": failures, "last_success": last_success})));
                self.as_mut().rust_mut().cleanup = report.items;
                self.as_mut().rust_mut().failures = report.failures;
                self.as_mut().set_rows(encoded(rows));
                self.set_status(
                    if incomplete {
                        "Some cleanup sources could not be checked."
                    } else if empty {
                        "Nothing to clean."
                    } else {
                        "Review a cleanup task before running it."
                    }
                    .into(),
                );
            }
            Ok(Payload::Repositories(report)) => {
                self.as_mut().rust_mut().view_cache.clear();
                self.as_mut().rust_mut().prefetched.clear();
                self.as_mut().rust_mut().prefetch = prefetch_views();
                self.as_mut().rust_mut().detail_cache.clear();
                crate::metadata::invalidate();
                self.as_mut().set_repositories(encoded(report));
                self.set_status("Repositories loaded.".into());
            }
            Ok(Payload::Sources(sources)) => {
                if matches!(self.rust().queued, Some(Job::Load(..))) {
                    return;
                }
                let rows: Vec<_> = sources.iter().map(source_row).collect();
                let failures: Vec<_> = rows
                    .iter()
                    .filter(|row| row["check_failed"] == true)
                    .map(|row| json!({"source": row["source"], "kind": row["availability_kind"]}))
                    .collect();
                let last_success = self.rust().last_success.clone();
                let phase = if failures.is_empty() {
                    "complete"
                } else if failures.len() == rows.len() {
                    "failed"
                } else {
                    "partial"
                };
                self.as_mut().set_report_state(encoded(
                    json!({"phase": phase, "failures": failures, "last_success": last_success}),
                ));
                self.as_mut().set_source_catalog(encoded(&rows));
                self.as_mut().rust_mut().sources = sources;
                self.as_mut().set_rows(encoded(rows));
                self.set_status("Source availability checked. Select a source for details.".into());
            }
            Ok(Payload::Details(details)) => {
                if matches!(self.rust().queued, Some(Job::Load(..))) {
                    return;
                }
                // Provider metadata may improve a name after opening details.
                // Preserve inventory state and only update presentation fields.
                let rows = self.rows().to_string();
                if let Some(rows) = update_detail_name(
                    &mut self.as_mut().rust_mut().packages,
                    &rows,
                    &details.package,
                ) {
                    self.as_mut().set_rows(encoded(rows));
                    self.as_mut().rust_mut().view_cache.clear();
                    self.as_mut().rust_mut().prefetched.clear();
                    self.as_mut().rust_mut().prefetch = prefetch_views();
                }
                let info = crate::metadata::cached_info(&details.package);
                let data = encoded(
                    json!({"package": package_row(&details.package, &same_app_sources(&self.rust().packages, &details.package.id), None), "description": info.as_ref().filter(|i| !i.description.is_empty()).map(|i| &i.description).unwrap_or(&details.description), "homepage": details.homepage.as_ref().or_else(|| info.as_ref().and_then(|i| i.homepage.as_ref())), "dependencies": details.dependencies, "screenshots": info.as_ref().map(|i| &i.screenshots)}),
                );
                // Bound memory use for large searches; reload and writes invalidate this snapshot.
                if self.rust().detail_cache.len() >= 128 {
                    self.as_mut().rust_mut().detail_cache.clear();
                }
                self.as_mut()
                    .rust_mut()
                    .detail_cache
                    .insert(details.package.id.clone(), data.clone());
                // A superseded reply still warms the cache, but only the
                // current selection may take over the details panel.
                if details_fresh(self.rust().selected.as_ref(), &details) {
                    self.as_mut().set_details(data);
                    self.set_status("Package details loaded.".into());
                }
            }
            Ok(Payload::Batch(status, _)) => {
                // Native writes may change dependencies in other rows.
                self.as_mut()
                    .apply(Ok(Payload::Written(OperationOutcome::default())));
                self.set_status(status.as_str().into());
            }
            Ok(Payload::Written(outcome)) => {
                crate::metadata::invalidate();
                self.as_mut().set_upgradable(false);
                self.as_mut().rust_mut().prefetched.clear();
                self.as_mut().rust_mut().prefetch = prefetch_views();
                self.as_mut().rust_mut().detail_cache.clear();
                self.as_mut().set_phase("stale");
                self.set_status(
                    if outcome.cancellation_deferred {
                        "Completed after cancellation; native changes were not rolled back."
                    } else {
                        "Completed. Refreshing package state."
                    }
                    .into(),
                );
            }
        }
    }
    fn warm_inventory(mut self: Pin<&mut Self>, mut report: PackageReport) {
        let sources = self.rust().source_filter.clone();
        let sudo = self.rust().sudo;
        let key = cache_key("Installed", "", &sources, sudo);
        self.as_mut()
            .rust_mut()
            .prefetched
            .insert(key, (Instant::now(), Payload::Packages(report.clone())));
        report
            .packages
            .retain(|p| p.update == UpdateAvailability::Available);
        let key = cache_key("Updates", "", &sources, sudo);
        self.as_mut()
            .rust_mut()
            .prefetched
            .insert(key, (Instant::now(), Payload::Packages(report)));
    }
    pub fn poll(mut self: Pin<&mut Self>) {
        if self.rust().worker.is_none() && self.rust().pending.is_none() {
            while let Some(view) = self.as_mut().rust_mut().prefetch.pop() {
                let key = cache_key(&view, "", &self.rust().source_filter, self.rust().sudo);
                if self
                    .rust()
                    .view_cache
                    .get(&key)
                    .is_some_and(|v| v.loaded.elapsed() < VIEW_TTL)
                    || self.rust().prefetched.contains_key(&key)
                {
                    continue;
                }
                self.as_mut().rust_mut().background = true;
                self.as_mut().start(Job::Load(view, String::new()));
                break;
            }
        }
        let Some(worker) = &self.rust().worker else {
            return;
        };
        let replies: Vec<_> = worker.receiver.try_iter().collect();
        let complete =
            replies.iter().any(|r| matches!(r, Reply::Done(_))) || worker.handle.is_finished();
        let cancelled = worker.cancel.requested();
        let replies: Vec<_> = replies
            .into_iter()
            .filter_map(|reply| {
                if let Reply::Inventory(report) = reply {
                    if !cancelled {
                        self.as_mut().warm_inventory(report);
                    }
                    None
                } else {
                    Some(reply)
                }
            })
            .collect();
        if complete {
            let worker = self.as_mut().rust_mut().worker.take().unwrap();
            let joined = worker.handle.join();
            for reply in replies.into_iter().chain(worker.receiver.try_iter()) {
                if let Reply::Inventory(report) = reply {
                    if !worker.cancel.requested() {
                        self.as_mut().warm_inventory(report);
                    }
                    continue;
                }
                if self.rust().background {
                    if !worker.cancel.requested() {
                        match reply {
                            Reply::Done(Ok(Payload::BackgroundUpdates(report))) => {
                                self.as_mut().finish_background_check(report)
                            }
                            Reply::Done(Ok(payload)) => {
                                if let Job::Load(view, query) = &worker.job {
                                    if let Payload::Sources(sources) = &payload {
                                        let rows: Vec<_> = sources.iter().map(source_row).collect();
                                        self.as_mut().set_source_catalog(encoded(rows));
                                    }
                                    let key = cache_key(
                                        view,
                                        query,
                                        &self.rust().source_filter,
                                        self.rust().sudo,
                                    );
                                    self.as_mut()
                                        .rust_mut()
                                        .prefetched
                                        .insert(key, (Instant::now(), payload));
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                match reply {
                    // The join above guarantees the thread finished sending.
                    Reply::Engine(engine) => {
                        self.as_mut().rust_mut().engine = Some(*engine);
                    }
                    Reply::Partial(report) => self.as_mut().apply(Ok(Payload::Packages(report))),
                    Reply::DetailsPreview(details) => {
                        self.as_mut().apply(Ok(Payload::Details(details)))
                    }
                    Reply::Done(result) => {
                        if self.rust().discard_revalidation
                            && matches!(
                                worker.job,
                                Job::PlanOperation(..)
                                    | Job::PlanUpgrade(..)
                                    | Job::PlanCleanAll(..)
                            )
                        {
                            continue;
                        }
                        if worker.job.writes() {
                            if let Some(id) = self.as_mut().rust_mut().active_activity_id.take() {
                                if let Some(store) = self.rust().activity_store.clone() {
                                    let outcomes = match &result {
                                        Ok(Payload::Batch(_, outcomes)) => outcomes.clone(),
                                        Ok(_) => worker
                                            .job
                                            .operations()
                                            .iter()
                                            .map(|_| Outcome::Finished)
                                            .collect(),
                                        Err(EngineError::Cancelled) => worker
                                            .job
                                            .operations()
                                            .iter()
                                            .map(|_| Outcome::Cancelled)
                                            .collect(),
                                        Err(_) => worker
                                            .job
                                            .operations()
                                            .iter()
                                            .map(|_| Outcome::Failed)
                                            .collect(),
                                    };
                                    let _ = store.finish(id, outcomes);
                                }
                                self.as_mut().refresh_activity();
                            }
                        }
                        // A reviewed native plan can change between review and write.
                        // Stop the write, compute the new plan, and ask again.
                        if let Some(next) =
                            repreview_changed_plan(&worker.job, &result, &self.rust().packages)
                        {
                            self.as_mut().rust_mut().queued = Some(next);
                        }
                        // Snapshot terminal view reports for instant
                        // switching back, unless a newer load already
                        // superseded this one (its guard in apply skips it).
                        let key = match &worker.job {
                            Job::Load(view, query) | Job::RetrySource(view, query, _)
                                if !matches!(self.rust().queued, Some(Job::Load(..))) =>
                            {
                                Some(cache_key(
                                    view,
                                    query,
                                    &self.rust().source_filter.clone(),
                                    self.rust().sudo,
                                ))
                            }
                            _ => None,
                        };
                        let stashable = matches!(
                            result,
                            Ok(Payload::Packages(_))
                                | Ok(Payload::Sources(_))
                                | Ok(Payload::Cleanup(_))
                                | Ok(Payload::RetryPackages(..))
                                | Ok(Payload::RetryCleanup(..))
                                | Ok(Payload::RetrySources(..))
                        );
                        self.as_mut().apply(result);
                        if stashable {
                            if let Some(key) = key {
                                self.as_mut().stash_current(key);
                            }
                        }
                    }
                    Reply::Progress(_) | Reply::Inventory(_) => {}
                }
            }
            if joined.is_err() && !self.rust().background {
                self.as_mut().set_status("Backend worker failed.".into());
            }
            self.as_mut().rust_mut().background = false;
            self.as_mut().rust_mut().discard_revalidation = false;
            let queued = self.as_mut().rust_mut().queued.take();
            self.as_mut().set_writing(false);
            self.as_mut().set_busy(false);
            if let Some(entry) = self.as_mut().rust_mut().validated_confirmed.take() {
                self.as_mut().rust_mut().queued = queued;
                self.start_confirmed(entry);
            } else if let Some(entry) = self.as_mut().rust_mut().confirmed_queue.pop_front() {
                self.as_mut().rust_mut().queued = queued;
                self.validate_confirmed(entry);
            } else if matches!(&queued, Some(Job::PlanOperation(..) | Job::PlanUpgrade(..))) {
                self.start(queued.expect("review job"));
            } else if let Some(job) = self.as_mut().rust_mut().deferred_load.take() {
                self.start(job);
            } else if let Some(job) = queued {
                self.start(job);
            }
        } else if !self.rust().background {
            for reply in replies {
                match reply {
                    Reply::Partial(report) => self.as_mut().apply(Ok(Payload::Packages(report))),
                    Reply::DetailsPreview(details) => {
                        self.as_mut().apply(Ok(Payload::Details(details)))
                    }
                    Reply::Progress(message) => {
                        self.as_mut().set_status(message.as_str().into());
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_preview_names_target_and_extra_native_changes() {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
            "display_name":"Anonymous App", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        let operation = Operation::Upgrade(package.id.clone());
        let mut plan = TransactionPlan {
            operation: operation.clone(),
            native_preview: "synthetic".into(),
            changes: vec![
                PlannedChange {
                    action: PlannedAction::Upgrade,
                    name: "anonymous".into(),
                    installed_version: Some("1".into()),
                    candidate_version: Some("2".into()),
                },
                PlannedChange {
                    action: PlannedAction::Remove,
                    name: "old-library".into(),
                    installed_version: Some("1".into()),
                    candidate_version: None,
                },
            ],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
        };
        let preview =
            confirmation_preview(&operation, std::slice::from_ref(&package), &[], Some(&plan));
        assert_eq!(preview["action"], "Update Anonymous App");
        let body = preview["body"].as_str().unwrap();
        assert!(body.contains("Source: apt"));
        assert!(body.contains("Scope: System"));
        assert!(body.contains("Remove old-library (1)"));
        assert!(!body.contains("Install anonymous"));
        assert!(body.contains("Data retention: manager-specific"));
        plan.changes.push(PlannedChange {
            action: PlannedAction::Remove,
            name: "anonymous".into(),
            installed_version: Some("1".into()),
            candidate_version: None,
        });
        plan.download_bytes = Some(2048);
        plan.disk_bytes = Some(-512);
        plan.restart_required = Some(true);
        let preview =
            confirmation_preview(&operation, std::slice::from_ref(&package), &[], Some(&plan));
        let body = preview["body"].as_str().unwrap();
        assert!(body.contains("Requested change: Update anonymous (1 → 2)"));
        assert!(body.contains("Remove anonymous (1)"));
        assert!(body.contains("Download: 2048 bytes · Disk impact: -512 bytes · Restart: required"));
        let unavailable = confirmation_preview(&operation, &[package], &[], None);
        assert!(unavailable["body"]
            .as_str()
            .unwrap()
            .contains("Transaction preview unavailable"));
        let long = serde_json::from_value::<Package>(json!({
            "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
            "display_name":"An extremely long synthetic application name for narrow windows", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        let preview = confirmation_preview(&operation, std::slice::from_ref(&long), &[], None);
        assert!(preview["action"].as_str().unwrap().chars().count() <= 36);
        assert!(preview["body"]
            .as_str()
            .unwrap()
            .contains(&long.display_name));
    }
    #[test]
    fn retry_merges_only_target_and_preserves_other_check_times() {
        let package = |backend: &str| -> Package {
            serde_json::from_value(json!({
                "id": {"backend":backend, "name":"anonymous", "architecture":"all", "scope":"system"},
                "display_name":"Anonymous", "summary":"Synthetic", "installed_version":"1", "candidate_version":"1", "update":"current"
            })).unwrap()
        };
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().packages = vec![package("apt")];
        controller
            .as_mut()
            .rust_mut()
            .last_success
            .insert("apt".into(), 123);
        controller.as_mut().set_report_state(encoded(json!({"phase":"partial", "successful_sources":["apt"], "failures":[{"source":"npm", "kind":"failed"}], "last_success":{"apt":123}})));
        controller.as_mut().apply(Ok(Payload::RetryPackages(
            "npm".into(),
            PackageReport {
                packages: vec![package("npm")],
                failures: vec![],
                successful_sources: vec!["npm".into()],
            },
        )));
        assert_eq!(controller.rust().packages.len(), 2);
        assert_eq!(controller.rust().last_success["apt"], 123);
        assert!(controller.rust().last_success["npm"] > 123);
        assert_eq!(
            serde_json::from_str::<Value>(&controller.report_state().to_string()).unwrap()["phase"],
            "complete"
        );
    }
    #[test]
    fn retry_jobs_return_only_the_requested_view_and_preview() {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"fixture", "name":"anonymous", "architecture":"all", "scope":"system"},
            "display_name":"Anonymous", "summary":"Synthetic", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package.clone(),
                fail: false,
            })
            .unwrap();
        let cancel = Cancellation::default();
        for (view, query, count) in [
            ("Search", "anonymous", 1),
            ("Installed", "missing", 0),
            ("Updates", "missing", 1),
        ] {
            let mut replies = Vec::new();
            execute(
                &mut engine,
                Job::RetrySource(view.into(), query.into(), "fixture".into()),
                &cancel,
                &mut |reply| replies.push(reply),
            );
            assert_eq!(replies.len(), 1);
            match replies.pop().unwrap() {
                Reply::Done(Ok(Payload::RetryPackages(source, report))) => {
                    assert_eq!(source, "fixture");
                    assert_eq!(report.packages.len(), count);
                    assert_eq!(report.successful_sources, ["fixture"]);
                }
                _ => panic!("expected a scoped package retry"),
            }
        }
        let mut replies = Vec::new();
        execute(
            &mut engine,
            Job::RetrySource("Clean".into(), "".into(), "fixture".into()),
            &cancel,
            &mut |reply| replies.push(reply),
        );
        assert!(
            matches!(replies.pop(), Some(Reply::Done(Ok(Payload::RetryCleanup(source, _)))) if source == "fixture")
        );
        execute(
            &mut engine,
            Job::RetrySource("Sources".into(), "".into(), "fixture".into()),
            &cancel,
            &mut |reply| replies.push(reply),
        );
        assert!(
            matches!(replies.pop(), Some(Reply::Done(Ok(Payload::RetrySources(source, sources)))) if source == "fixture" && sources.len() == 1)
        );
        execute(
            &mut engine,
            Job::PlanOperation(Operation::Install(package.id)),
            &cancel,
            &mut |reply| replies.push(reply),
        );
        assert!(matches!(
            replies.pop(),
            Some(Reply::Done(Ok(Payload::OperationPreview(
                Operation::Install(_),
                None
            ))))
        ));
    }
    #[test]
    fn retry_cleanup_and_sources_replace_only_failed_source() {
        let item = |source: &str, name: &str| CleanupItem {
            id: CleanupId {
                backend: source.into(),
                key: name.into(),
            },
            kind: CleanupKind::OrphanDependencies,
            title: name.into(),
            summary: "Synthetic".into(),
            preview: "No changes".into(),
        };
        let source = |name: &str, available| Source {
            backend: name.into(),
            capabilities: vec![],
            availability: if available {
                Ok(Availability::Available)
            } else {
                Err(EngineError::Unavailable {
                    backend: name.into(),
                    reason: "Synthetic".into(),
                })
            },
        };
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .apply(Ok(Payload::Cleanup(CleanupReport {
                items: vec![item("apt", "keep"), item("npm", "old")],
                failures: vec![],
            })));
        controller.as_mut().apply(Ok(Payload::RetryCleanup(
            "npm".into(),
            CleanupReport {
                items: vec![item("npm", "new")],
                failures: vec![],
            },
        )));
        assert_eq!(
            controller
                .rust()
                .cleanup
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["keep", "new"]
        );
        controller.as_mut().apply(Ok(Payload::Sources(vec![
            source("apt", true),
            source("npm", false),
        ])));
        controller.as_mut().apply(Ok(Payload::RetrySources(
            "npm".into(),
            vec![source("npm", true)],
        )));
        assert_eq!(controller.rust().sources.len(), 2);
        assert!(controller
            .rust()
            .sources
            .iter()
            .all(|source| source.availability.is_ok()));
    }
    #[test]
    fn changed_plans_request_a_fresh_confirmation_for_the_same_targets() {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
            "display_name":"Anonymous", "summary":"Synthetic", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let operation = Operation::Upgrade(package.id.clone());
        let plan = TransactionPlan {
            operation: operation.clone(),
            native_preview: "Synthetic".into(),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
        };
        let drift = Err(EngineError::InvalidResponse {
            backend: "apt".into(),
            reason: "Transaction plan changed; review again".into(),
        });
        let single = Job::Write(operation.clone(), Some(Box::new(plan)));
        assert!(
            matches!(repreview_changed_plan(&single, &drift, std::slice::from_ref(&package)), Some(Job::PlanOperation(next)) if next == operation)
        );
        let batch = Job::UpgradeAll(
            vec![operation.clone()],
            Some(AptUpgradePlan {
                preview: "Synthetic".into(),
                upgrades: vec!["anonymous".into()],
                installs: vec![],
                removals: vec![],
            }),
        );
        assert!(
            matches!(repreview_changed_plan(&batch, &drift, &[package]), Some(Job::PlanUpgrade(next, 1)) if next == vec![operation])
        );
        assert!(repreview_changed_plan(&single, &Err(EngineError::NotFound), &[]).is_none());
        assert!(
            repreview_changed_plan(&Job::Load("Updates".into(), "".into()), &drift, &[]).is_none()
        );
    }
    #[test]
    fn retry_rejects_unknown_or_disabled_sources_without_starting_work() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .retry_source("Unknown".into(), "".into(), "apt".into());
        controller
            .as_mut()
            .retry_source("Updates".into(), "".into(), "unknown".into());
        controller.as_mut().rust_mut().source_filter = vec!["apt".into()];
        controller
            .as_mut()
            .retry_source("Updates".into(), "".into(), "npm".into());
        assert!(controller.rust().worker.is_none());
        assert!(controller.rust().queued.is_none());
    }
    #[test]
    fn apt_removals_appear_first_in_update_confirmation() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let plan = AptUpgradePlan {
            preview: "synthetic plan".into(),
            upgrades: vec!["synthetic".into()],
            installs: vec!["dependency".into()],
            removals: vec!["retired".into()],
        };
        controller.as_mut().apply(Ok(Payload::UpgradePreview(
            vec![Operation::UpgradeAll {
                backend: "apt".into(),
            }],
            1,
            Some(plan.clone()),
        )));
        let confirmation = controller.confirmation().to_string();
        assert!(confirmation.contains("Remove (1): retired"));
        assert!(
            confirmation.find("Remove (1): retired")
                < confirmation.find("Update all packages from apt")
        );
        assert!(
            matches!(&controller.rust().pending, Some(Job::UpgradeAll(_, Some(saved))) if *saved == plan)
        );
    }
    #[test]
    fn background_inventory_populates_both_sections_without_foreground_changes() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_rows("original".into());
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"snap", "name":"synthetic", "architecture":"all", "scope":"system"},
            "display_name":"Synthetic", "summary":"Fixture", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package,
                fail: false,
            })
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            execute(
                &mut engine,
                Job::Load("Installed".into(), "".into()),
                &Cancellation::default(),
                &mut |reply| {
                    sender.send(reply).unwrap();
                },
            )
        });
        handle.join().unwrap();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Installed".into(), "".into()),
        });
        controller.as_mut().poll();
        assert_eq!(controller.rows().to_string(), "original");
        assert!(!controller.busy());
        // A single Installed execution supplies both sections. Switching to
        // Updates and back starts no new worker and preserves native updates.
        for view in ["Updates", "Installed"] {
            controller
                .as_mut()
                .load(view.into(), "".into(), "".into(), false, false);
            assert!(controller.rust().worker.is_none());
            assert_eq!(controller.rust().packages.len(), 1);
            assert_eq!(
                controller.rust().packages[0].update,
                UpdateAvailability::Available
            );
        }
    }

    #[test]
    fn expired_snapshot_stays_visible_and_preempts_background_refresh() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Clean", "", &[], false);
        let mut cached = cached_view("cached cleanup");
        cached.loaded = Instant::now() - VIEW_TTL;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key, cached);
        let (_sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::Load("Sources".into(), "".into()),
        });
        controller
            .as_mut()
            .load("Clean".into(), "".into(), "".into(), false, false);
        assert!(controller.rows().to_string().contains("cached cleanup"));
        assert!(cancel.requested());
        assert!(matches!(&controller.rust().queued, Some(Job::Load(view, _)) if view == "Clean"));
    }

    #[test]
    fn confirmed_write_queued_over_background_worker_reports_busy() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let (_sender, receiver) = mpsc::channel();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Sources".into(), "".into()),
        });
        controller.as_mut().rust_mut().pending = Some(Job::Write(
            Operation::Refresh {
                backend: "fixture".into(),
            },
            None,
        ));
        controller.as_mut().confirm(true);
        // The write waits for the background worker to wind down, but the
        // UI must already report busy so confirmed work is never read as
        // idle before it starts.
        assert!(matches!(
            controller.rust().confirmed_queue.front().map(|entry| &entry.job),
            Some(Job::Write(Operation::Refresh { backend }, _)) if backend == "fixture"
        ));
        assert!(*controller.busy());
    }

    #[test]
    fn confirmed_queue_keeps_order_and_rejects_duplicate_or_conflicting_targets() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let (_sender, receiver) = mpsc::channel();
        let first = PackageId {
            backend: "fixture".into(),
            name: "first".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        let second = PackageId {
            name: "second".into(),
            ..first.clone()
        };
        let third = PackageId {
            name: "third".into(),
            ..first.clone()
        };
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Write(Operation::Install(first.clone()), None),
        });
        controller
            .as_mut()
            .accept_confirmed(Job::Write(Operation::Install(first.clone()), None));
        controller
            .as_mut()
            .accept_confirmed(Job::Write(Operation::Remove(first), None));
        assert!(controller.rust().confirmed_queue.is_empty());
        controller
            .as_mut()
            .accept_confirmed(Job::Write(Operation::Install(second.clone()), None));
        controller
            .as_mut()
            .accept_confirmed(Job::Write(Operation::Install(third.clone()), None));
        assert_eq!(controller.rust().confirmed_queue.len(), 2);
        assert!(
            matches!(&controller.rust().confirmed_queue[0].job, Job::Write(Operation::Install(id), _) if id == &second)
        );
        assert!(
            matches!(&controller.rust().confirmed_queue[1].job, Job::Write(Operation::Install(id), _) if id == &third)
        );
        controller.as_mut().cancel_queued();
        assert!(controller.rust().confirmed_queue.is_empty());
        let validating = Confirmed {
            job: Job::Write(Operation::Install(second.clone()), None),
            activity_id: None,
            cleanup_preview: vec![],
        };
        assert_eq!(
            queue_conflict(
                &Job::Write(Operation::Remove(second), None),
                None,
                &VecDeque::new(),
                Some(&validating),
                None,
            ),
            Some("A conflicting operation is already running or queued.")
        );
        let clean = CleanupId {
            backend: "fixture".into(),
            key: "cache".into(),
        };
        let active = Job::CleanAll(vec![Operation::Clean(clean.clone())]);
        assert_eq!(
            queue_conflict(
                &Job::Write(Operation::Clean(clean), None),
                Some(&active),
                &VecDeque::new(),
                None,
                None
            ),
            Some("This operation is already running or queued.")
        );
        let active = Job::UpgradeAll(
            vec![Operation::UpgradeAll {
                backend: "fixture".into(),
            }],
            None,
        );
        assert_eq!(
            queue_conflict(
                &Job::Write(Operation::Upgrade(third), None),
                Some(&active),
                &VecDeque::new(),
                None,
                None
            ),
            Some("A conflicting operation is already running or queued.")
        );
    }

    #[test]
    fn cancelling_queued_revalidation_discards_a_late_preview() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let operation = Operation::Refresh {
            backend: "fixture".into(),
        };
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Ok(Payload::OperationPreview(
                operation.clone(),
                None,
            ))))
            .unwrap();
        controller.as_mut().rust_mut().revalidating = Some(Confirmed {
            job: Job::Write(operation.clone(), None),
            activity_id: None,
            cleanup_preview: vec![],
        });
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::PlanOperation(operation),
        });
        controller.as_mut().cancel_queued();
        assert!(cancel.requested());
        controller.as_mut().poll();
        assert!(controller.rust().pending.is_none());
        assert!(controller.confirmation().is_empty());
    }

    #[test]
    fn background_check_policy_and_results_leave_the_current_view_intact() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_rows("current view".into());
        controller
            .as_mut()
            .check_updates("apt".into(), false, false, false, false);
        controller
            .as_mut()
            .check_updates("apt".into(), true, true, false, false);
        controller
            .as_mut()
            .check_updates("apt".into(), true, false, true, false);
        assert!(controller.rust().worker.is_none());
        controller
            .as_mut()
            .check_updates("unknown".into(), true, false, false, false);
        assert!(controller.rust().worker.is_none());

        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
            "display_name":"Synthetic", "summary":"Fixture", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let report = PackageReport {
            packages: vec![package],
            failures: vec![],
            successful_sources: vec!["apt".into()],
        };
        controller.as_mut().finish_background_check(report.clone());
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["available"], 1);
        assert_eq!(state["notify"], false);
        assert_eq!(controller.rows().to_string(), "current view");

        let mut changed = report;
        changed.packages[0].candidate_version = Some("3".into());
        controller.as_mut().finish_background_check(changed);
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["notify"], true);

        controller.as_mut().finish_background_check(PackageReport {
            packages: vec![],
            failures: vec![BackendFailure {
                backend: "fixture".into(),
                error: EngineError::Cancelled,
            }],
            successful_sources: vec![],
        });
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["notify"], false);
        assert_eq!(state["failures"][0]["source"], "fixture");
    }

    #[test]
    fn writes_serve_cached_navigation_and_defer_new_reads_without_cancelling() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().view_cache.insert(
            cache_key("Installed", "", &[], false),
            cached_view("retained package"),
        );
        let (_sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::Write(
                Operation::Refresh {
                    backend: "fixture".into(),
                },
                None,
            ),
        });
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        assert!(controller.rows().to_string().contains("retained package"));
        assert!(!cancel.requested());
        assert!(
            matches!(&controller.rust().deferred_load, Some(Job::Load(view, _)) if view == "Installed")
        );
        controller
            .as_mut()
            .load("Search".into(), "new query".into(), "".into(), false, false);
        assert_eq!(controller.rows().to_string(), "[]");
        assert!(
            matches!(&controller.rust().deferred_load, Some(Job::Load(view, query)) if view == "Search" && query == "new query")
        );
        assert!(!cancel.requested());
    }

    #[test]
    fn queued_previews_only_reuse_an_unchanged_confirmation() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let operation = Operation::Refresh {
            backend: "fixture".into(),
        };
        let queued = || Confirmed {
            job: Job::Write(operation.clone(), None),
            activity_id: None,
            cleanup_preview: vec![],
        };
        controller.as_mut().rust_mut().revalidating = Some(queued());
        controller
            .as_mut()
            .apply(Ok(Payload::OperationPreview(operation.clone(), None)));
        assert!(controller.rust().validated_confirmed.is_some());
        assert!(controller.confirmation().is_empty());
        controller.as_mut().rust_mut().validated_confirmed = None;
        controller.as_mut().rust_mut().revalidating = Some(queued());
        controller.as_mut().apply(Ok(Payload::OperationPreview(
            Operation::Refresh {
                backend: "changed".into(),
            },
            None,
        )));
        assert!(controller.rust().validated_confirmed.is_none());
        assert!(controller.confirmation().to_string().contains("changed"));
    }

    #[test]
    fn queued_cleanup_requires_new_consent_when_its_native_preview_changes() {
        let item = CleanupItem {
            id: CleanupId {
                backend: "fixture".into(),
                key: "cache".into(),
            },
            kind: CleanupKind::OrphanDependencies,
            title: "Synthetic cleanup".into(),
            summary: "Fixture".into(),
            preview: "Remove one cached file".into(),
        };
        let operations = vec![Operation::Clean(item.id.clone())];
        let queued = || Confirmed {
            job: Job::CleanAll(operations.clone()),
            activity_id: None,
            cleanup_preview: vec![item.clone()],
        };
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().revalidating = Some(queued());
        controller.as_mut().apply(Ok(Payload::CleanPreview(
            operations.clone(),
            vec![item.clone()],
        )));
        assert!(controller.rust().validated_confirmed.is_some());
        assert!(controller.confirmation().is_empty());

        controller.as_mut().rust_mut().validated_confirmed = None;
        controller.as_mut().rust_mut().revalidating = Some(queued());
        let mut changed = item.clone();
        changed.preview = "Remove two cached files".into();
        controller
            .as_mut()
            .apply(Ok(Payload::CleanPreview(operations.clone(), vec![changed])));
        assert!(controller.rust().validated_confirmed.is_none());
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Remove two cached files"));
        assert!(matches!(controller.rust().pending, Some(Job::CleanAll(_))));

        controller.as_mut().rust_mut().pending = None;
        controller.as_mut().set_confirmation(QString::default());
        controller.as_mut().rust_mut().revalidating = Some(queued());
        controller
            .as_mut()
            .apply(Ok(Payload::CleanPreview(operations, vec![])));
        assert!(controller.rust().pending.is_none());
        assert!(controller
            .status()
            .to_string()
            .contains("no longer available"));
    }

    #[test]
    fn queued_writes_wait_for_a_read_worker_then_revalidate_each_job_kind() {
        let clean = Operation::Clean(CleanupId {
            backend: "fixture".into(),
            key: "cache".into(),
        });
        let jobs = [
            Job::Write(
                Operation::Refresh {
                    backend: "fixture".into(),
                },
                None,
            ),
            Job::UpgradeAll(
                vec![Operation::UpgradeAll {
                    backend: "fixture".into(),
                }],
                None,
            ),
            Job::CleanAll(vec![clean]),
        ];
        for (index, job) in jobs.into_iter().enumerate() {
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            let (_sender, receiver) = mpsc::channel();
            let cancel = Cancellation::default();
            controller.as_mut().rust_mut().background = true;
            controller.as_mut().rust_mut().worker = Some(Worker {
                handle: thread::spawn(|| {}),
                receiver,
                cancel: cancel.clone(),
                job: Job::Load("Sources".into(), "".into()),
            });
            controller.as_mut().validate_confirmed(Confirmed {
                job,
                activity_id: None,
                cleanup_preview: vec![],
            });
            assert!(controller.rust().revalidating.is_some());
            assert!(cancel.requested());
            assert!(matches!(
                (index, &controller.rust().queued),
                (0, Some(Job::PlanOperation(_)))
                    | (1, Some(Job::PlanUpgrade(_, _)))
                    | (2, Some(Job::PlanCleanAll(_)))
            ));
        }
    }

    #[test]
    fn background_update_job_reads_only_available_updates() {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"fixture", "name":"synthetic", "architecture":"all", "scope":"system"},
            "display_name":"Synthetic", "summary":"Fixture", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package,
                fail: false,
            })
            .unwrap();
        let mut replies = vec![];
        execute(
            &mut engine,
            Job::BackgroundUpdates(vec!["fixture".into()]),
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        assert!(matches!(
            replies.pop(),
            Some(Reply::Done(Ok(Payload::BackgroundUpdates(report))))
                if report.packages.len() == 1 && report.packages[0].id.name == "synthetic"
        ));
    }

    #[test]
    fn autostart_only_persists_after_a_successful_file_change() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-controller-autostart-{}",
            std::process::id()
        ));
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().autostart_path = Some(path.join("autostart/app.desktop"));
        assert!(controller.as_mut().set_autostart(true));
        assert!(std::fs::read_to_string(path.join("autostart/app.desktop"))
            .unwrap()
            .contains("--background"));
        assert!(controller.as_mut().set_autostart(false));
        assert!(!path.join("autostart/app.desktop").exists());
        std::fs::write(path.join("blocked"), "synthetic").unwrap();
        controller.as_mut().rust_mut().autostart_path = Some(path.join("blocked/app.desktop"));
        assert!(!controller.as_mut().set_autostart(true));
        assert!(controller
            .status()
            .to_string()
            .contains("Autostart could not"));
        controller.as_mut().rust_mut().autostart_path = None;
        assert!(!controller.as_mut().set_autostart(true));
        assert!(controller
            .status()
            .to_string()
            .contains("No user configuration directory"));
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn queued_activity_and_mixed_batch_results_use_exact_targets() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-controller-activity-{}",
            std::process::id()
        ));
        let store = History::new(path.join("activity.json"));
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().activity_store = Some(store.clone());
        let first = Operation::Upgrade(PackageId {
            backend: "fixture".into(),
            name: "first".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        });
        let second = Operation::Upgrade(PackageId {
            backend: "fixture".into(),
            name: "second".into(),
            architecture: "all".into(),
            scope: Scope::User { uid: 1234 },
            remote: None,
            reference: None,
        });
        let operations = vec![first, second];
        let (_sender, receiver) = mpsc::channel();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Sources".into(), "".into()),
        });
        controller
            .as_mut()
            .accept_confirmed(Job::UpgradeAll(operations.clone(), None));
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operations, operations);
        assert_eq!(entries[0].state, State::Queued);
        assert!(controller.activity().to_string().contains("queued"));
        controller.as_mut().cancel_queued();
        assert_eq!(store.entries().unwrap()[0].state, State::Cancelled);
        controller.as_mut().rust_mut().worker = None;

        let id = store
            .begin("gui", operations.clone(), State::Running)
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Ok(Payload::Batch(
                "One completed, one failed".into(),
                vec![Outcome::Finished, Outcome::Failed],
            ))))
            .unwrap();
        controller.as_mut().rust_mut().active_activity_id = Some(id);
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::UpgradeAll(operations.clone(), None),
        });
        controller.as_mut().poll();
        let entry = store
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == id)
            .unwrap();
        assert_eq!(entry.state, State::Failed);
        assert_eq!(entry.outcomes, [Outcome::Finished, Outcome::Failed]);
        assert_eq!(entry.operations, operations);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn failed_queued_revalidation_records_failure_without_starting_a_write() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-revalidation-activity-{}",
            std::process::id()
        ));
        let store = History::new(path.join("activity.json"));
        let operation = Operation::Refresh {
            backend: "fixture".into(),
        };
        let id = store
            .begin("gui", vec![operation.clone()], State::Queued)
            .unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().activity_store = Some(store.clone());
        controller.as_mut().rust_mut().revalidating = Some(Confirmed {
            job: Job::Write(operation, None),
            activity_id: Some(id),
            cleanup_preview: vec![],
        });
        controller.as_mut().apply(Err(EngineError::NotFound));
        assert!(controller.rust().revalidating.is_none());
        assert!(controller.rust().worker.is_none());
        let entry = store.entries().unwrap().pop().unwrap();
        assert_eq!(entry.state, State::Failed);
        assert_eq!(entry.outcomes, [Outcome::Failed]);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unavailable_confirmed_target_has_a_terminal_activity_result() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-unavailable-activity-{}",
            std::process::id()
        ));
        let store = History::new(path.join("activity.json"));
        let operation = Operation::Refresh {
            backend: "missing-fixture".into(),
        };
        let id = store
            .begin("gui", vec![operation.clone()], State::Queued)
            .unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().activity_store = Some(store.clone());
        controller.as_mut().start_confirmed(Confirmed {
            job: Job::Write(operation, None),
            activity_id: Some(id),
            cleanup_preview: vec![],
        });
        assert_eq!(store.entries().unwrap()[0].state, State::Running);
        for _ in 0..500 {
            if controller
                .rust()
                .worker
                .as_ref()
                .is_some_and(|worker| worker.handle.is_finished())
            {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        controller.as_mut().poll();
        let entry = store
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == id)
            .unwrap();
        assert_eq!(entry.state, State::Failed);
        assert_eq!(entry.outcomes, [Outcome::Failed]);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn changed_native_plan_is_reviewed_before_a_deferred_page_load() {
        let operation = Operation::Refresh {
            backend: "missing-fixture".into(),
        };
        let plan = TransactionPlan {
            operation: operation.clone(),
            native_preview: "synthetic".into(),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
        };
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Err(EngineError::InvalidResponse {
                backend: "missing-fixture".into(),
                reason: "Transaction plan changed".into(),
            })))
            .unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Write(operation.clone(), Some(Box::new(plan))),
        });
        controller.as_mut().rust_mut().deferred_load =
            Some(Job::Load("Search".into(), "query".into()));
        controller.as_mut().poll();
        assert!(
            matches!(controller.rust().worker.as_ref().map(|worker| &worker.job), Some(Job::PlanOperation(op)) if op == &operation)
        );
        assert!(
            matches!(&controller.rust().deferred_load, Some(Job::Load(view, query)) if view == "Search" && query == "query")
        );
    }

    #[test]
    fn selection_during_a_write_uses_retained_package_details() {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"fixture", "name":"selected", "architecture":"all", "scope":"system"},
            "display_name":"Selected", "summary":"Synthetic details", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().packages = vec![package];
        let (_sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::Write(
                Operation::Refresh {
                    backend: "fixture".into(),
                },
                None,
            ),
        });
        controller.as_mut().select(0);
        assert_eq!(
            controller.rust().selected.as_ref().unwrap().name,
            "selected"
        );
        assert!(controller
            .details()
            .to_string()
            .contains("Synthetic details"));
        assert!(!cancel.requested());
    }

    #[test]
    fn label_helpers_cover_scopes_sources_and_cleanup_operations() {
        use std::path::PathBuf;
        assert_eq!(scope_label(&Scope::System), "System");
        assert_eq!(scope_label(&Scope::User { uid: 1000 }), "User 1000");
        assert_eq!(
            scope_label(&Scope::Environment {
                path: PathBuf::from("/tmp/fixture")
            }),
            "/tmp/fixture"
        );
        let unavailable = Source {
            backend: "apt".into(),
            capabilities: vec![],
            availability: Ok(Availability::Unavailable("locked".into())),
        };
        assert_eq!(source_status(&unavailable), "Unavailable: locked");
        let failed = Source {
            backend: "apt".into(),
            capabilities: vec![],
            availability: Err(EngineError::Cancelled),
        };
        assert_eq!(source_status(&failed), "operation cancelled");
        let clean = Operation::Clean(CleanupId {
            backend: "apt".into(),
            key: "autoremove".into(),
        });
        assert_eq!(operation_label(&clean), "Clean autoremove with apt");
        assert!(confirmation_label(&clean, &[]).contains("Clean autoremove with apt"));
        let firmware = |name: &str| Package {
            id: PackageId {
                backend: "fwupd".into(),
                name: name.into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: "synthetic firmware".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        let other = firmware("other-device");
        let target = firmware("target-device");
        let label = confirmation_label(&Operation::Upgrade(target.id.clone()), &[other, target]);
        assert!(label.contains("target-device: synthetic firmware"));
        assert!(!label.contains("other-device: synthetic firmware"));
    }

    #[test]
    fn preempting_read_reports_busy_until_fresh_rows_land() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let (_sender, receiver) = mpsc::channel();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Installed".into(), "".into()),
        });
        controller
            .as_mut()
            .load("Search".into(), "fixture".into(), "".into(), false, true);
        // The new view cancels the in-flight load and waits behind it; the
        // table is empty until it runs, so idle must not be reported.
        assert!(matches!(
            &controller.rust().queued,
            Some(Job::Load(view, query)) if view == "Search" && query == "fixture"
        ));
        assert!(*controller.busy());
    }

    #[test]
    fn superseded_search_drops_late_successful_payloads() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_rows("new-query-pending".into());
        controller.as_mut().rust_mut().queued =
            Some(Job::Load("Search".into(), "new-query".into()));
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport::default())));
        assert_eq!(controller.rows().to_string(), "new-query-pending");
        controller.as_mut().apply(Ok(Payload::Sources(vec![])));
        assert_eq!(controller.rows().to_string(), "new-query-pending");
    }

    #[test]
    fn prefetch_order_is_bounded_and_never_searches() {
        let mut queue = prefetch_views();
        let order: Vec<_> = std::iter::from_fn(|| queue.pop()).collect();
        assert_eq!(order, ["Installed", "Updates", "Sources", "Clean"]);
        assert!(queue.is_empty());
    }

    #[test]
    fn cleanup_rows_show_preview_and_require_confirmation() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let item = CleanupItem {
            id: CleanupId {
                backend: "apt".into(),
                key: "autoremove".into(),
            },
            kind: CleanupKind::OrphanDependencies,
            title: "Unused dependencies".into(),
            summary: "One package".into(),
            preview: "Remv synthetic-runtime [1.0]".into(),
        };
        controller
            .as_mut()
            .apply(Ok(Payload::Cleanup(CleanupReport {
                items: vec![item],
                failures: vec![],
            })));
        controller
            .as_mut()
            .stash_current(cache_key("Clean", "", &[], false));
        controller.as_mut().rust_mut().cleanup.clear();
        controller
            .as_mut()
            .load("Clean".into(), "".into(), "".into(), false, false);
        assert_eq!(controller.rust().cleanup.len(), 1);
        assert!(controller.rust().worker.is_none());
        assert!(controller.rows().to_string().contains("cleanup"));
        controller.as_mut().select(0);
        assert!(controller
            .details()
            .to_string()
            .contains("synthetic-runtime"));
        controller.as_mut().propose("clean".into(), 0);
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Remv synthetic-runtime"));
        assert!(matches!(
            controller.rust().pending,
            Some(Job::Write(Operation::Clean(_), _))
        ));
        controller.as_mut().propose("clean-all".into(), 0);
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Run 1 cleanup tasks"));
        assert!(matches!(controller.rust().pending, Some(Job::CleanAll(_))));
        controller.as_mut().confirm(false);

        let failure = BackendFailure {
            backend: "homebrew".into(),
            error: EngineError::Unavailable {
                backend: "homebrew".into(),
                reason: "synthetic executable missing".into(),
            },
        };
        controller
            .as_mut()
            .apply(Ok(Payload::Cleanup(CleanupReport {
                items: vec![],
                failures: vec![failure],
            })));
        assert!(controller
            .status()
            .to_string()
            .contains("could not be checked"));
        assert!(controller.rows().to_string().contains("failure"));
        controller.as_mut().select(0);
        assert!(controller.details().to_string().contains("homebrew"));
        controller.as_mut().propose("clean-all".into(), 0);
        assert!(controller.confirmation().is_empty());
    }
    #[test]
    fn qt_repository_confirmation_and_refresh_invalidate_cached_state() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().change_repository("broken".into());
        assert!(controller
            .status()
            .to_string()
            .contains("Invalid repository request"));
        controller.as_mut().change_repository(
            r#"{"backend":"flatpak","name":"--all","scope":"system","action":"remove"}"#.into(),
        );
        assert!(controller
            .status()
            .to_string()
            .contains("invalid repository name"));
        let request =
            r#"{"backend":"flatpak","name":"fixture","scope":"system","action":"remove"}"#;
        controller.as_mut().change_repository(request.into());
        assert!(controller.confirmation().to_string().contains("System"));
        assert!(controller.confirmation().to_string().contains("fixture"));
        assert!(matches!(
            controller.rust().pending,
            Some(Job::Repositories(Some(_)))
        ));
        assert!(!*controller.busy());
        controller.as_mut().confirm(false);
        assert!(controller.rust().pending.is_none());
        assert!(controller.confirmation().to_string().is_empty());
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert("old".into(), cached_view("old"));
        let key = cache_key("Sources", "", &[], false);
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key, cached_view("cached sources"));
        controller
            .as_mut()
            .load("Sources".into(), "".into(), "".into(), false, false);
        assert!(!*controller.busy());
        assert!(controller.rows().to_string().contains("cached sources"));
        controller
            .as_mut()
            .apply(Ok(Payload::Repositories(repositories::Report {
                repositories: vec![repositories::Repository {
                    backend: "flatpak".into(),
                    name: "fixture".into(),
                    title: "Fixture".into(),
                    url: "https://example.invalid".into(),
                    scope: Scope::System,
                    enabled: true,
                    priority: Some(1),
                }],
                errors: vec!["Synthetic partial failure".into()],
            })));
        let report: Value = serde_json::from_str(&controller.repositories().to_string()).unwrap();
        assert_eq!(report["repositories"][0]["scope"], "system");
        assert_eq!(report["errors"][0], "Synthetic partial failure");
        assert!(controller.rust().view_cache.get("old").is_none());
    }
    #[test]
    fn qt_firmware_actions_require_confirmation_and_never_offer_removal() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let package = Package {
            id: PackageId {
                backend: "fwupd".into(),
                name: "synthetic-device".into(),
                architecture: "device".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "Synthetic BIOS".into(),
            summary: "Firmware · AC power required · Restart required".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        controller.as_mut().rust_mut().detail_cache.insert(
            package.id.clone(),
            encoded(json!({"description": "Synthetic cached firmware details"})),
        );
        controller.as_mut().select(0);
        assert!(controller
            .details()
            .to_string()
            .contains("Synthetic cached firmware details"));
        assert!(!*controller.busy());

        controller.as_mut().rust_mut().detail_cache.clear();
        let (_sender, receiver) = mpsc::channel();
        let cancelled = Cancellation::default();
        let mut previous = package.id.clone();
        previous.name = "previous-device".into();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancelled.clone(),
            job: Job::Details(previous),
        });
        controller.as_mut().select(0);
        assert!(cancelled.requested());
        assert!(matches!(&controller.rust().queued, Some(Job::Details(id)) if *id == package.id));
        controller
            .as_mut()
            .rust_mut()
            .worker
            .take()
            .unwrap()
            .handle
            .join()
            .unwrap();
        controller.as_mut().rust_mut().queued = None;
        controller.as_mut().rust_mut().updates_view = true;
        controller.as_mut().set_upgradable(true);
        for action in ["install", "remove"] {
            controller.as_mut().propose(action.into(), 0);
            assert!(controller.rust().pending.is_none());
        }
        for action in ["upgrade", "upgrade-all"] {
            controller.as_mut().propose(action.into(), 0);
            let confirmation = controller.confirmation().to_string();
            assert!(confirmation.contains("Synthetic BIOS"));
            assert!(confirmation.contains("AC power required"));
            assert!(confirmation.contains("Restart required"));
            assert!(!*controller.writing());
            controller.as_mut().confirm(false);
        }
        let identity = json!([[
            package.id.backend,
            package.id.name,
            package.id.architecture,
            null,
            package.id.scope
        ]])
        .to_string();
        controller
            .as_mut()
            .propose_checked(identity.as_str().into());
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Synthetic BIOS"));
        controller.as_mut().propose_checked("[]".into());
        assert!(controller.rust().pending.is_none());
        assert!(controller.status().to_string().contains("No selected"));
        controller.as_mut().apply(Ok(Payload::Batch(
            "Firmware completed. Restart required.".into(),
            vec![Outcome::Finished],
        )));
        assert!(controller.status().to_string().contains("Restart required"));
        assert!(!controller.rust().packages.is_empty());
        assert!(!*controller.upgradable());
    }
    #[test]
    fn qt_container_pull_is_explicit_and_requires_a_stored_tag() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let package = Package {
            id: PackageId {
                backend: "docker".into(),
                name: "sha256:0123456789abcdef".into(),
                architecture: "x86_64".into(),
                scope: Scope::System,
                remote: Some("Docker daemon".into()),
                reference: Some("example/app:latest".into()),
            },
            display_name: "example/app:latest".into(),
            summary: "Tags: example/app:latest · 42MB".into(),
            installed_version: Some("0123456789ab".into()),
            candidate_version: None,
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        controller.as_mut().rust_mut().packages = vec![package];
        controller.as_mut().propose("upgrade".into(), 0);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Upgrade(_), _))
        ));
        assert!(controller
            .confirmation()
            .to_string()
            .contains("example/app:latest"));
        controller.as_mut().confirm(false);
        controller.as_mut().rust_mut().packages[0].id.reference = None;
        controller.as_mut().propose("upgrade".into(), 0);
        assert!(controller.rust().pending.is_none());
    }
    #[test]
    fn qt_source_failures_and_busy_requests_preserve_the_active_operation() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .load("Sources".into(), "".into(), "unknown".into(), false, false);
        assert_eq!(controller.status().to_string(), "Unknown source.");
        assert!(!*controller.busy());
        controller
            .as_mut()
            .rust_mut()
            .failures
            .push(BackendFailure {
                backend: "fwupd".into(),
                error: EngineError::Unavailable {
                    backend: "fwupd".into(),
                    reason: "Synthetic daemon offline".into(),
                },
            });
        controller.as_mut().select(0);
        assert!(controller
            .details()
            .to_string()
            .contains("Synthetic daemon offline"));
        controller.as_mut().rust_mut().failures.clear();
        controller.as_mut().rust_mut().sources.push(Source {
            backend: "fwupd".into(),
            capabilities: vec![Capability::Refresh],
            availability: Ok(Availability::Unavailable("Synthetic unavailable".into())),
        });
        controller.as_mut().select(0);
        assert!(controller
            .details()
            .to_string()
            .contains("Synthetic unavailable"));
        controller.as_mut().propose("refresh".into(), 0);
        assert!(controller.rust().pending.is_none());
        let (sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::Repositories(Some(RepositoryAction {
                backend: "flatpak".into(),
                name: "fixture".into(),
                scope: Scope::System,
                change: repositories::Change::Remove,
            })),
        });
        controller.as_mut().load_repositories();
        controller.as_mut().change_repository("invalid".into());
        controller.as_mut().propose_checked("[]".into());
        controller
            .as_mut()
            .load("Sources".into(), "".into(), "".into(), false, true);
        assert!(!cancel.requested());
        assert!(controller.rust().queued.is_none());
        controller.as_mut().cancel();
        assert!(cancel.requested());
        assert!(controller
            .status()
            .to_string()
            .contains("waiting for the native operation"));
        sender
            .send(Reply::Done(Ok(Payload::Repositories(
                repositories::Report::default(),
            ))))
            .unwrap_or_else(|_| panic!("worker channel closed"));
        controller.as_mut().poll();
        assert!(controller.rust().worker.is_none());
        assert!(!*controller.busy());
    }
    #[test]
    fn qt_streamed_details_preserve_progress_and_finish_cleanly() {
        let mut object = ffi::create_controller();
        let mut controller = object.pin_mut();
        let package = Package {
            id: PackageId {
                backend: "fwupd".into(),
                name: "synthetic-device".into(),
                architecture: "device".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "Device".into(),
            summary: "Firmware".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        controller.as_mut().rust_mut().selected = Some(package.id.clone());
        controller
            .as_mut()
            .set_rows(encoded(vec![package_row(&package, &[], None)]));
        let mut details = PackageDetails {
            package: package.clone(),
            description: "Synthetic firmware details".into(),
            homepage: None,
            dependencies: vec![],
        };
        details.package.display_name = "Synthetic BIOS".into();
        let (sender, receiver) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(move || {
                wait.recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            }),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Details(package.id.clone()),
        });
        controller.as_mut().set_busy(true);
        sender
            .send(Reply::DetailsPreview(Box::new(details.clone())))
            .unwrap_or_else(|_| panic!("closed channel"));
        sender
            .send(Reply::Progress("Loading device metadata".into()))
            .unwrap_or_else(|_| panic!("closed channel"));
        controller.as_mut().poll();
        assert_eq!(controller.status().to_string(), "Loading device metadata");
        assert!(controller
            .details()
            .to_string()
            .contains("Synthetic firmware details"));
        assert!(controller.rows().to_string().contains("Synthetic BIOS"));
        assert!(*controller.busy());
        release.send(()).unwrap();
        sender
            .send(Reply::Done(Ok(Payload::Details(Box::new(details)))))
            .unwrap_or_else(|_| panic!("closed channel"));
        controller.as_mut().poll();
        assert!(!*controller.busy());
        assert!(controller.rust().worker.is_none());
        controller
            .as_mut()
            .apply(Ok(Payload::Written(OperationOutcome {
                cancellation_deferred: true,
            })));
        assert!(controller
            .status()
            .to_string()
            .contains("Completed after cancellation"));
    }
    #[test]
    fn qt_new_view_discards_superseded_reports_and_cancellation_errors() {
        let mut object = ffi::create_controller();
        let mut controller = object.pin_mut();
        let (_sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::Load("Sources".into(), "".into()),
        });
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, true);
        assert!(cancel.requested());
        assert!(matches!(&controller.rust().queued, Some(Job::Load(view, _)) if view == "Updates"));
        let status = controller.status().to_string();
        controller.as_mut().apply(Err(EngineError::Cancelled));
        assert_eq!(controller.status().to_string(), status);
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport::default())));
        controller.as_mut().apply(Ok(Payload::Sources(vec![Source {
            backend: "fwupd".into(),
            capabilities: vec![],
            availability: Ok(Availability::Available),
        }])));
        assert!(controller.rust().sources.is_empty());
        assert_eq!(controller.rows().to_string(), "[]");
        controller
            .as_mut()
            .rust_mut()
            .worker
            .take()
            .unwrap()
            .handle
            .join()
            .unwrap();
        controller.as_mut().rust_mut().queued = None;
        controller.as_mut().apply(Err(EngineError::NotFound));
        assert!(controller
            .status()
            .to_string()
            .contains("no package matches"));
    }
    struct Fixture {
        package: Package,
        fail: bool,
    }
    impl Backend for Fixture {
        fn id(&self) -> &str {
            &self.package.id.backend
        }
        fn capabilities(&self) -> &[Capability] {
            &[
                Capability::Search,
                Capability::Installed,
                Capability::Details,
                Capability::Install,
                Capability::Remove,
                Capability::Refresh,
                Capability::Upgrade,
            ]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(vec![self.package.clone()])
        }
        fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(vec![self.package.clone()])
        }
        fn details(
            &mut self,
            _: &PackageId,
            _: &Cancellation,
        ) -> Result<PackageDetails, EngineError> {
            Err(EngineError::NotFound)
        }
        fn execute(
            &mut self,
            _: &Operation,
            cancel: &Cancellation,
            progress: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            progress(Progress::Message("Synthetic progress".into()));
            if self.fail {
                Err(pkgdeck_core::process::ExecutionError::AuthorizationDenied.into())
            } else {
                cancel.cancel();
                Ok(OperationOutcome {
                    cancellation_deferred: true,
                })
            }
        }
    }
    fn cached_view(name: &str) -> CachedView {
        CachedView {
            loaded: Instant::now(),
            cleanup: vec![],
            packages: vec![],
            failures: vec![],
            sources: vec![],
            rows: format!(r#"[{{"name":"{name}"}}]"#).as_str().into(),
            status: "cached".into(),
            report_state: r#"{"phase":"cached"}"#.into(),
            upgradable: false,
            updates_view: false,
        }
    }
    #[test]
    fn cache_keys_separate_views_queries_sources_and_elevation() {
        let apt = vec!["apt".to_string()];
        let all: Vec<String> = vec![];
        assert_eq!(
            cache_key("Installed", "", &all, false),
            cache_key("Installed", "", &all, false)
        );
        assert_ne!(
            cache_key("Installed", "", &all, false),
            cache_key("Updates", "", &all, false)
        );
        assert_ne!(
            cache_key("Search", "fire", &all, false),
            cache_key("Search", "firefox", &all, false)
        );
        assert_ne!(
            cache_key("Installed", "", &all, false),
            cache_key("Installed", "", &apt, false)
        );
        assert_ne!(
            cache_key("Installed", "", &all, false),
            cache_key("Installed", "", &all, true)
        );
    }
    #[test]
    fn view_cache_replaces_evicts_oldest_and_clears() {
        let mut cache = ViewCache { entries: vec![] };
        assert!(cache.get("missing").is_none());
        cache.insert("a".into(), cached_view("a"));
        cache.insert("b".into(), cached_view("b"));
        assert!(cache.get("a").is_some());
        // Re-inserting a key replaces it without growing the cache.
        cache.insert("a".into(), cached_view("a2"));
        assert_eq!(cache.entries.len(), 2);
        assert!(cache.get("a").is_some());
        for i in 0..ViewCache::CAPACITY {
            cache.insert(format!("k{i}"), cached_view("x"));
        }
        assert_eq!(cache.entries.len(), ViewCache::CAPACITY);
        // Oldest ("b", then "a") evicted first.
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_none());
        cache.clear();
        assert!(cache.entries.is_empty());
    }
    #[test]
    fn friendly_detail_names_preserve_exact_identity_and_inventory_state() {
        let original: Package = serde_json::from_value(json!({
            "id": {"backend":"snap", "name":"player", "architecture":"all", "scope":"system"},
            "display_name":"player", "summary":"Local summary", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let mut packages = vec![original.clone()];
        let rows = serde_json::to_string(&vec![package_row(&original, &[], None)]).unwrap();
        let mut detail = original.clone();
        detail.display_name = "Friendly Player".into();
        detail.installed_version = None;
        detail.candidate_version = Some("99".into());
        let updated = update_detail_name(&mut packages, &rows, &detail).unwrap();
        assert_eq!(updated[0]["display_name"], "Friendly Player");
        assert_eq!(updated[0]["installed"], "1");
        assert_eq!(updated[0]["candidate"], "2");
        let mut expected = original.clone();
        expected.display_name = "Friendly Player".into();
        assert_eq!(packages, [expected]);
        assert!(update_detail_name(&mut packages, &rows, &detail).is_none());
        packages[0] = original.clone();
        detail.id.scope = Scope::User { uid: 1000 };
        assert!(update_detail_name(&mut packages, &rows, &detail).is_none());
        detail.id = original.id.clone();
        for malformed in ["broken", "[]", "[null]"] {
            assert!(update_detail_name(&mut packages, malformed, &detail).is_none());
            assert_eq!(packages[0], original);
        }
        detail.display_name.clear();
        assert!(update_detail_name(&mut packages, &rows, &detail).is_none());
    }
    #[test]
    fn controller_version_tracks_package_metadata() {
        assert_eq!(
            Controller::default().version.to_string(),
            pkgdeck_core::VERSION
        );
    }
    #[test]
    fn installed_view_filters_by_query_while_other_views_ignore_it() {
        fn load(engine: &mut Engine, view: &str, query: &str) -> PackageReport {
            let mut replies = vec![];
            execute(
                engine,
                Job::Load(view.into(), query.into()),
                &Cancellation::default(),
                &mut |r| replies.push(r),
            );
            match replies.pop().unwrap() {
                Reply::Done(Ok(Payload::Packages(report))) => report,
                _ => panic!("expected a package report"),
            }
        }
        let package = Package {
            id: PackageId {
                backend: "fixture".into(),
                name: "synthetic".into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package.clone(),
                fail: false,
            })
            .unwrap();
        assert_eq!(load(&mut engine, "Installed", "").packages.len(), 1);
        assert_eq!(
            load(&mut engine, "Installed", "synthetic").packages.len(),
            1
        );
        assert_eq!(load(&mut engine, "Installed", "FIXTURE").packages.len(), 1);
        assert!(load(&mut engine, "Installed", "missing")
            .packages
            .is_empty());
        // Updates never narrows by the query; stale field text is ignored.
        assert_eq!(load(&mut engine, "Updates", "missing").packages.len(), 1);
        // Search must exclude install placeholders in every streamed report,
        // but retain genuine catalog entries with an empty version label.
        for candidate in [None, Some(""), Some("2")] {
            let mut offer = package.clone();
            offer.installed_version = None;
            offer.candidate_version = candidate.map(str::to_owned);
            let mut engine = Engine::default();
            engine
                .register(Fixture {
                    package: offer,
                    fail: false,
                })
                .unwrap();
            let mut reports = Vec::new();
            execute(
                &mut engine,
                Job::Load("Search".into(), "synthetic".into()),
                &Cancellation::default(),
                &mut |reply| match reply {
                    Reply::Partial(report)
                    | Reply::Inventory(report)
                    | Reply::Done(Ok(Payload::Packages(report))) => reports.push(report),
                    _ => {}
                },
            );
            assert!(reports.len() >= 2);
            assert!(reports
                .iter()
                .all(|report| report.packages.len() == usize::from(candidate.is_some())));
        }
    }
    #[test]
    fn checked_identities_resolve_only_to_upgradable_packages() {
        fn package(name: &str, installed: bool, update: UpdateAvailability) -> Package {
            Package {
                id: PackageId {
                    backend: "fixture".into(),
                    name: name.into(),
                    architecture: "all".into(),
                    scope: Scope::System,
                    remote: None,
                    reference: None,
                },
                display_name: name.into(),
                summary: "Fixture".into(),
                installed_version: installed.then(|| "1".into()),
                candidate_version: Some("2".into()),
                update,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            }
        }
        let packages = vec![
            package("upgradable", true, UpdateAvailability::Available),
            package("current", true, UpdateAvailability::Current),
            package("uninstalled", false, UpdateAvailability::Available),
        ];
        // Identity shape mirrors QML rowIdentity: [source, name, arch, remote, scope, reference].
        let row = |name: &str| {
            vec![
                serde_json::json!("fixture"),
                serde_json::json!(name),
                serde_json::json!("all"),
                serde_json::json!(null),
                serde_json::json!("system"),
                serde_json::json!(null),
            ]
        };
        let identities = serde_json::to_string(&vec![
            row("upgradable"),
            row("current"),
            row("uninstalled"),
            row("vanished"),
            vec![serde_json::json!("bogus")],
        ])
        .unwrap();
        let operations = checked_upgrades(&packages, &identities);
        assert_eq!(operations.len(), 1);
        assert!(matches!(&operations[0], Operation::Upgrade(id) if id.name == "upgradable"));
        assert!(checked_upgrades(&packages, "not json").is_empty());
        let mut runtime = package("org.example.Platform", true, UpdateAvailability::Available);
        runtime.id.reference = Some("runtime/org.example.Platform/all/stable".into());
        let mut identity = row("org.example.Platform");
        identity[5] = serde_json::json!(runtime.id.reference);
        let encoded = serde_json::to_string(&vec![identity.clone()]).unwrap();
        assert_eq!(
            checked_upgrades(&[runtime.clone()], &encoded),
            vec![Operation::Upgrade(runtime.id.clone())]
        );
        identity[5] = serde_json::json!("runtime/org.example.Platform/all/beta");
        assert!(
            checked_upgrades(&[runtime], &serde_json::to_string(&vec![identity]).unwrap())
                .is_empty()
        );
    }
    #[test]
    fn checked_proposals_plan_selected_upgrades() {
        fn package(name: &str) -> Package {
            Package {
                id: PackageId {
                    backend: "fixture".into(),
                    name: name.into(),
                    architecture: "all".into(),
                    scope: Scope::System,
                    remote: None,
                    reference: None,
                },
                display_name: name.into(),
                summary: "Fixture".into(),
                installed_version: Some("1".into()),
                candidate_version: Some("2".into()),
                update: UpdateAvailability::Available,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            }
        }
        // Identity shape mirrors QML rowIdentity: [source, name, arch, remote, scope, reference].
        let row = |name: &str| {
            serde_json::to_string(&vec![
                serde_json::json!("fixture"),
                serde_json::json!(name),
                serde_json::json!("all"),
                serde_json::json!(null),
                serde_json::json!("system"),
                serde_json::json!(null),
            ])
            .unwrap()
        };
        let packages = vec![package("upgradable")];
        // Stale identities resolve to nothing: empty confirmation, status set.
        let empty = plan_checked_upgrade(&packages, &format!("[{}]", row("vanished")));
        assert!(empty.operations.is_empty());
        assert!(empty.confirmation.is_empty());
        assert_eq!(
            empty.status.as_deref(),
            Some("No selected packages can be updated.")
        );
        // One live identity plans a single-upgrade batch with confirmation.
        let plan = plan_checked_upgrade(
            &packages,
            &format!("[{}, {}]", row("upgradable"), row("vanished")),
        );
        assert_eq!(plan.operations.len(), 1);
        assert!(matches!(&plan.operations[0], Operation::Upgrade(id) if id.name == "upgradable"));
        assert!(plan.status.is_none());
        assert!(plan.confirmation.contains("Update 1 selected packages?"));
        assert!(plan.confirmation.contains("Update upgradable"));
    }
    #[test]
    fn failure_details_carry_backend_error_and_hint() {
        let failure = BackendFailure {
            backend: "npm".into(),
            error: EngineError::InvalidResponse {
                backend: "npm".into(),
                reason: "npm ls failed: boom".into(),
            },
        };
        let text = failure_details(&failure).to_string();
        assert!(text.contains("npm ls failed: boom"));
        assert!(text.contains("Sources view"));
    }
    #[test]
    fn details_jobs_scope_their_own_backend() {
        let id = PackageId {
            backend: "homebrew".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        assert_eq!(
            engine_source(&Job::Details(id.clone()), &[]),
            vec!["homebrew".to_string()]
        );
        assert_eq!(
            engine_source(&Job::Details(id), &["apt".to_string()]),
            vec!["homebrew".to_string()]
        );
        assert_eq!(
            engine_source(
                &Job::Load("Installed".into(), "".into()),
                &["apt".to_string()]
            ),
            vec!["apt".to_string()]
        );
        assert_eq!(
            engine_source(&Job::Load("Installed".into(), "".into()), &[]),
            Vec::<String>::new()
        );
        assert_eq!(
            engine_source(
                &Job::RetrySource("Updates".into(), "".into(), "apt".into()),
                &["apt".into(), "npm".into()]
            ),
            vec!["apt".to_string()]
        );
    }
    #[test]
    fn stale_details_replies_never_take_the_panel() {
        let id = PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        let mut other = id.clone();
        other.name = "other".into();
        let details = PackageDetails {
            package: Package {
                id: id.clone(),
                display_name: "Synthetic".into(),
                summary: "Fixture".into(),
                installed_version: None,
                candidate_version: None,
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            },
            description: String::new(),
            homepage: None,
            dependencies: vec![],
        };
        assert!(details_fresh(Some(&id), &details));
        assert!(!details_fresh(None, &details));
        assert!(!details_fresh(Some(&other), &details));
    }
    #[test]
    fn loads_stream_partials_before_the_terminal_report() {
        let package = Package {
            id: PackageId {
                backend: "fixture".into(),
                name: "synthetic".into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package.clone(),
                fail: false,
            })
            .unwrap();
        let mut replies = vec![];
        execute(
            &mut engine,
            Job::Load("Search".into(), "synthetic".into()),
            &Cancellation::default(),
            &mut |r| replies.push(r),
        );
        assert_eq!(replies.len(), 2);
        let partial = match replies.remove(0) {
            Reply::Partial(report) => report,
            _ => panic!("expected a streaming partial first"),
        };
        match replies.remove(0) {
            Reply::Done(Ok(Payload::Packages(report))) => assert_eq!(report, partial),
            _ => panic!("expected the terminal report last"),
        }
    }
    #[test]
    fn installed_and_updates_loads_stream_filtered_partials() {
        let package = Package {
            id: PackageId {
                backend: "fixture".into(),
                name: "synthetic".into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package.clone(),
                fail: false,
            })
            .unwrap();
        // Both views stream one partial per backend, each carrying the
        // same view filter as the terminal report.
        for view in ["Installed", "Updates"] {
            let mut replies = vec![];
            execute(
                &mut engine,
                Job::Load(view.into(), "".into()),
                &Cancellation::default(),
                &mut |r| replies.push(r),
            );
            assert_eq!(replies.len(), 3);
            let partial = match replies.remove(0) {
                Reply::Partial(report) => report,
                _ => panic!("expected a streaming partial first"),
            };
            assert_eq!(partial.packages, vec![package.clone()]);
            assert!(matches!(replies.remove(0), Reply::Inventory(_)));
            match replies.remove(0) {
                Reply::Done(Ok(Payload::Packages(report))) => assert_eq!(report, partial),
                _ => panic!("expected the terminal report last"),
            }
        }
    }
    #[test]
    fn jobs_keep_source_identity_native_updates_and_typed_failures() {
        let id = PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        let package = Package {
            id: id.clone(),
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        };
        assert_eq!(package_row(&package, &[], None)["source"], "fixture");
        assert!(encoded(package_row(&package, &[], None))
            .to_string()
            .contains("synthetic"));
        let mut firmware = package.clone();
        firmware.id.backend = "fwupd".into();
        firmware.display_name = "Synthetic BIOS".into();
        firmware.summary = "Firmware · AC power required · Restart required".into();
        for tool in pkgdeck_core::backends::StandaloneTool::ALL {
            let mut standalone = package.clone();
            standalone.id.backend = tool.id().into();
            standalone.id.reference = Some("/synthetic/bin/tool".into());
            assert_eq!(
                upgrade_plan(&[standalone.clone()]),
                vec![Operation::Upgrade(standalone.id)]
            );
        }
        let mixed = upgrade_plan(&[package.clone(), firmware.clone()]);
        assert!(mixed.contains(&Operation::Upgrade(firmware.id.clone())));
        let label = confirmation_label(
            &Operation::Upgrade(firmware.id.clone()),
            &[firmware.clone()],
        );
        assert!(label.contains("Synthetic BIOS"));
        assert!(label.contains("AC power required"));
        assert!(label.contains("Restart required"));
        let checked = plan_checked_upgrade(
            &[firmware.clone()],
            &json!([[
                firmware.id.backend,
                firmware.id.name,
                firmware.id.architecture,
                null,
                firmware.id.scope
            ]])
            .to_string(),
        );
        assert!(checked.confirmation.contains("Synthetic BIOS"));
        let mut firmware_engine = Engine::default();
        firmware_engine
            .register(Fixture {
                package: firmware.clone(),
                fail: false,
            })
            .unwrap();
        for job in [
            Job::Write(Operation::Upgrade(firmware.id.clone()), None),
            Job::UpgradeAll(vec![Operation::Upgrade(firmware.id.clone())], None),
        ] {
            let mut replies = vec![];
            execute(
                &mut firmware_engine,
                job,
                &Cancellation::default(),
                &mut |reply| replies.push(reply),
            );
            let Reply::Done(Ok(Payload::Batch(status, _))) = replies.pop().unwrap() else {
                panic!("firmware completion status missing")
            };
            assert!(status.contains("restart or shutdown"));
        }

        let mut other = package.clone();
        other.id.backend = "other-source".into();
        let mut current = package.clone();
        current.update = UpdateAvailability::Current;
        let mut uninstalled = package.clone();
        uninstalled.installed_version = None;
        let plan = upgrade_plan(&[
            package.clone(),
            package.clone(),
            other.clone(),
            current,
            uninstalled,
        ]);
        assert_eq!(plan.len(), 2);
        assert!(plan.contains(&Operation::UpgradeAll {
            backend: id.backend.clone()
        }));
        assert!(plan.contains(&Operation::UpgradeAll {
            backend: other.id.backend
        }));
        for fail in [false, true] {
            let mut engine = Engine::default();
            engine
                .register(Fixture {
                    package: package.clone(),
                    fail,
                })
                .unwrap();
            for job in [
                Job::Load("Search".into(), "synthetic".into()),
                Job::Load("Installed".into(), "".into()),
                Job::Load("Updates".into(), "".into()),
                Job::Load("Sources".into(), "".into()),
                Job::Details(id.clone()),
                Job::Write(Operation::Upgrade(id.clone()), None),
                Job::UpgradeAll(
                    vec![Operation::UpgradeAll {
                        backend: id.backend.clone(),
                    }],
                    None,
                ),
            ] {
                let mut replies = vec![];
                execute(
                    &mut engine,
                    job.clone(),
                    &Cancellation::default(),
                    &mut |r| replies.push(r),
                );
                match replies.pop().unwrap() {
                    Reply::Done(Ok(Payload::Packages(report))) => {
                        assert_eq!(report.packages[0].id, id)
                    }
                    Reply::Done(Ok(Payload::Sources(sources))) => {
                        assert_eq!(sources[0].backend, "fixture")
                    }
                    Reply::Done(Ok(Payload::Batch(status, _))) => {
                        assert!(status.contains(if fail {
                            "Completed 0 of 1"
                        } else {
                            "Completed 1 of 1"
                        }));
                        assert!(status.contains("Update all packages from fixture"));
                        assert!(status.contains(if fail { "authorization" } else { "cancel" }));
                    }
                    Reply::Done(Ok(Payload::Written(outcome))) => {
                        assert!(outcome.cancellation_deferred)
                    }
                    Reply::Done(Err(error)) => assert_eq!(
                        error,
                        if matches!(job, Job::Details(_)) {
                            EngineError::NotFound
                        } else {
                            pkgdeck_core::process::ExecutionError::AuthorizationDenied.into()
                        }
                    ),
                    _ => panic!("missing terminal reply"),
                }
            }
        }
    }
}
