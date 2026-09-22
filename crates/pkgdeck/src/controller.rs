//! Thin Qt facade: native work runs on a worker; Qt properties change only on the GUI thread.
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use pkgdeck_core::repositories::{self, Action as RepositoryAction};
use pkgdeck_core::{engine::*, host::Authorization, package::*, process::Cancellation};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
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
        #[qproperty(QString, confirmation)]
        #[qproperty(QString, version)]
        #[qproperty(bool, busy)]
        #[qproperty(bool, writing)]
        #[qproperty(bool, inspecting)]
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
    }
}

#[derive(Clone)]
enum Job {
    Repositories(Option<RepositoryAction>),
    Load(String, String),
    Details(PackageId),
    Write(Operation),
    UpgradeAll(Vec<Operation>),
    CleanAll(Vec<Operation>),
    InspectCleanup,
}
impl Job {
    // Authenticated inspection also takes the non-preemptible foreground lock.
    fn writes(&self) -> bool {
        matches!(
            self,
            Self::Write(_)
                | Self::UpgradeAll(_)
                | Self::CleanAll(_)
                | Self::Repositories(Some(_))
                | Self::InspectCleanup
        )
    }
}
enum Payload {
    Repositories(repositories::Report),
    Packages(PackageReport),
    Sources(Vec<Source>),
    Details(Box<PackageDetails>),
    Written(OperationOutcome),
    Batch(String),
    Cleanup(CleanupReport),
}
enum Reply {
    Progress(String),
    Partial(PackageReport),
    Inventory(PackageReport),
    DetailsPreview(Box<PackageDetails>),
    Done(Result<Payload, EngineError>),
    Engine(Engine),
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
        Job::InspectCleanup => Ok(Payload::Cleanup(engine.cleanup_authenticated(cancel))),
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
        Job::Details(id) => engine
            .details(&id, cancel)
            .map(|d| Payload::Details(Box::new(d))),
        Job::UpgradeAll(operations) | Job::CleanAll(operations) => {
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
            for (operation, result) in operations.iter().zip(results) {
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
            Ok(Payload::Batch(status))
        }
        Job::Write(op) => engine
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
                    })
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
    repositories: QString,
    version: QString,
    busy: bool,
    writing: bool,
    inspecting: bool,
    upgradable: bool,
    updates_view: bool,
    packages: Vec<Package>,
    cleanup: Vec<CleanupItem>,
    failures: Vec<BackendFailure>,
    detail_cache: BTreeMap<PackageId, QString>,
    sources: Vec<Source>,
    pending: Option<Job>,
    queued: Option<Job>,
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
            repositories: "{}".into(),
            version: pkgdeck_core::VERSION.into(),
            busy: false,
            writing: false,
            inspecting: false,
            upgradable: false,
            updates_view: false,
            packages: vec![],
            cleanup: vec![],
            failures: vec![],
            detail_cache: BTreeMap::new(),
            sources: vec![],
            pending: None,
            queued: None,
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
        Job::Details(id) => vec![id.backend.clone()],
        _ => filter.to_owned(),
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
fn package_row(p: &Package, same_from: &[String], same_group: Option<&str>) -> Value {
    json!({"name": p.id.name, "display_name": p.display_name, "source": p.id.backend, "architecture": p.id.architecture,
        "remote": p.id.remote, "reference": p.id.reference, "scope": p.id.scope, "scope_label": scope_label(&p.id.scope), "summary": p.summary, "installed": p.installed_version,
        "candidate": p.candidate_version, "update": p.update, "kind": "package", "icon": p.icon,
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
        if job.writes() {
            self.as_mut().rust_mut().view_cache.clear();
            self.as_mut().rust_mut().prefetched.clear();
            self.as_mut().rust_mut().prefetch = prefetch_views();
            self.as_mut().rust_mut().detail_cache.clear();
        }
        let handle = thread::spawn(move || {
            let sources_view = matches!(&job, Job::Load(view, _) if view == "Sources");
            let mut send = |mut reply| {
                match &mut reply {
                    Reply::Partial(report)
                    | Reply::Inventory(report)
                    | Reply::Done(Ok(Payload::Packages(report))) => {
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
                        send(Reply::Engine(engine));
                        return;
                    }
                    Err(EngineError::UnknownBackend(_)) => {}
                    Err(error) => {
                        send(Reply::Done(Err(error)));
                        send(Reply::Engine(engine));
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
                    execute(&mut engine, job, &token, &mut send);
                    send(Reply::Engine(engine));
                }
                Err(error) => send(Reply::Done(Err(error))),
            }
        });
        let writing = worker_job.writes();
        self.as_mut()
            .set_inspecting(matches!(worker_job, Job::InspectCleanup));
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
        if self.rust().worker.as_ref().is_some_and(|w| w.job.writes()) {
            return;
        }
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
                        && self.rust().failures.is_empty()
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
        if self.rust().worker.is_some() && !self.rust().background {
            return;
        }
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
        self.as_mut().set_confirmation(
            format!("{}\n\nApply this repository change?", action.label())
                .as_str()
                .into(),
        );
        self.rust_mut().pending = Some(Job::Repositories(Some(action)));
    }
    pub fn propose(mut self: Pin<&mut Self>, action: QString, index: i32) {
        if self.rust().worker.is_some() && !self.rust().background {
            return;
        }
        let action = action.to_string();
        if action == "inspect-clean" {
            self.as_mut().set_confirmation("Authenticate to inspect protected APT cleanup candidates? This only previews changes; nothing will be removed.".into());
            self.rust_mut().pending = Some(Job::InspectCleanup);
            return;
        }
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
            let labels = operations
                .iter()
                .map(|operation| confirmation_label(operation, &self.rust().packages))
                .collect::<Vec<_>>()
                .join("\n\n");
            self.as_mut().set_confirmation(format!("Update all {count} listed packages?\n\n{labels}\n\nUpdates use each source’s native updater. Native dependency changes may follow. Successful updates are not rolled back if another fails. Continue?").as_str().into());
            self.rust_mut().pending = Some(Job::UpgradeAll(operations));
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
            self.as_mut().set_confirmation(format!("Run {} cleanup tasks?\n\n{labels}\n\nThe native managers selected these files and dependencies. Completed tasks are not rolled back if another fails. Continue?", operations.len()).as_str().into());
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
        if let Some(op) = &operation {
            let label = if let Operation::Clean(id) = op {
                self.rust()
                    .cleanup
                    .iter()
                    .find(|item| item.id == *id)
                    .map(|item| format!("{} ({})\n\n{}", item.title, id.backend, item.preview))
                    .unwrap_or_else(|| operation_label(op))
            } else {
                confirmation_label(op, &self.rust().packages)
            };
            self.as_mut().set_confirmation(
                format!(
                    "{}\n\nNative dependency changes may follow. Continue?",
                    label
                )
                .as_str()
                .into(),
            );
        } else {
            self.as_mut().set_confirmation(QString::default());
        }
        self.rust_mut().pending = operation.map(Job::Write);
    }
    /// Queue an upgrade of the checked Updates rows. Identities are
    /// re-resolved against the current package list: stale rows (moved on
    /// or vanished while streaming) are skipped, never guessed. An empty
    /// resolution clears any pending confirmation and says so in status.
    pub fn propose_checked(mut self: Pin<&mut Self>, identities: QString) {
        if self.rust().worker.is_some() && !self.rust().background {
            return;
        }
        let plan = plan_checked_upgrade(&self.rust().packages, &identities.to_string());
        if plan.operations.is_empty() {
            self.as_mut().rust_mut().pending = None;
            self.as_mut().set_confirmation(QString::default());
        } else {
            self.as_mut()
                .set_confirmation(plan.confirmation.as_str().into());
        }
        if let Some(status) = plan.status {
            self.as_mut().set_status(status.as_str().into());
        }
        if !plan.operations.is_empty() {
            self.rust_mut().pending = Some(Job::UpgradeAll(plan.operations));
        }
    }
    pub fn confirm(mut self: Pin<&mut Self>, approved: bool) {
        if self.rust().worker.is_some() && !self.rust().background {
            return;
        }
        let pending = self.as_mut().rust_mut().pending.take();
        self.as_mut().rust_mut().queued = None;
        self.as_mut().set_confirmation(QString::default());
        if approved {
            if let Some(op) = pending {
                self.start(op);
            }
        }
    }
    pub fn cancel(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().prefetch.clear();
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
        if matches!(self.rust().queued, Some(Job::Load(..)))
            && matches!(result, Ok(Payload::Cleanup(_)))
        {
            return;
        }
        match result {
            Err(e) => {
                // A superseded Details job ends cancelled once its replacement
                // is queued; that abort carries no news worth flashing.
                let superseded =
                    matches!(e, EngineError::Cancelled) && self.rust().queued.is_some();
                if !superseded {
                    self.set_status(e.to_string().as_str().into());
                }
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
                        && report.failures.is_empty()
                        && !upgrade_plan(&report.packages).is_empty();
                    self.as_mut().set_upgradable(upgradable);
                }
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
                        "summary": failure.error.to_string(), "available": false})
                }));
                let status = if report.failures.is_empty() {
                    format!("{} packages", rows.len())
                } else {
                    format!(
                        "{} packages\n{}",
                        rows.len(),
                        report
                            .failures
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
                let rows: Vec<_> = sources.iter().map(|s| json!({"kind": "source", "name": s.backend, "source": s.backend, "summary": source_status(s), "available": s.availability == Ok(Availability::Available), "capabilities": s.capabilities})).collect();
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
            Ok(Payload::Batch(status)) => {
                // Clear the entire snapshot even on partial failure: any native write may
                // have changed dependencies belonging to another listed package.
                self.as_mut()
                    .apply(Ok(Payload::Written(OperationOutcome::default())));
                self.set_status(status.as_str().into());
            }
            Ok(Payload::Written(outcome)) => {
                crate::metadata::invalidate();
                self.as_mut().set_upgradable(false);
                // Any native write may change dependencies belonging to
                // another listed package: drop every cached view with them.
                self.as_mut().rust_mut().view_cache.clear();
                self.as_mut().rust_mut().prefetched.clear();
                self.as_mut().rust_mut().prefetch = prefetch_views();
                self.as_mut().rust_mut().detail_cache.clear();
                self.as_mut().rust_mut().packages.clear();
                self.as_mut().rust_mut().cleanup.clear();
                self.as_mut().rust_mut().failures.clear();
                self.as_mut().rust_mut().sources.clear();
                self.as_mut().set_rows("[]".into());
                self.as_mut().set_details("{}".into());
                self.set_status(
                    if outcome.cancellation_deferred {
                        "Completed after cancellation; native changes were not rolled back."
                    } else {
                        "Completed. Reload to see current package state."
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
                        if let (Job::Load(view, query), Reply::Done(Ok(payload))) =
                            (&worker.job, reply)
                        {
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
                    continue;
                }
                match reply {
                    // The join above guarantees the thread finished sending.
                    Reply::Engine(engine) => {
                        self.as_mut().rust_mut().engine = Some(engine);
                    }
                    Reply::Partial(report) => self.as_mut().apply(Ok(Payload::Packages(report))),
                    Reply::DetailsPreview(details) => {
                        self.as_mut().apply(Ok(Payload::Details(details)))
                    }
                    Reply::Done(result) => {
                        // Snapshot terminal view reports for instant
                        // switching back, unless a newer load already
                        // superseded this one (its guard in apply skips it).
                        let key = match &worker.job {
                            Job::Load(view, query)
                                if !matches!(self.rust().queued, Some(Job::Load(..))) =>
                            {
                                Some(cache_key(
                                    view,
                                    query,
                                    &self.rust().source_filter.clone(),
                                    self.rust().sudo,
                                ))
                            }
                            Job::InspectCleanup => Some(cache_key(
                                "Clean",
                                "",
                                &self.rust().source_filter,
                                self.rust().sudo,
                            )),
                            _ => None,
                        };
                        let stashable = matches!(
                            result,
                            Ok(Payload::Packages(_))
                                | Ok(Payload::Sources(_))
                                | Ok(Payload::Cleanup(_))
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
            let queued = self.as_mut().rust_mut().queued.take();
            self.as_mut().set_writing(false);
            self.as_mut().set_inspecting(false);
            self.as_mut().set_busy(false);
            // A selection or view change that arrived while the worker was
            // busy starts now that the previous job has fully terminated.
            if let Some(job) = queued {
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
        controller.as_mut().rust_mut().pending = Some(Job::Write(Operation::Refresh {
            backend: "fixture".into(),
        }));
        controller.as_mut().confirm(true);
        // The write waits for the background worker to wind down, but the
        // UI must already report busy so confirmed work is never read as
        // idle before it starts.
        assert!(matches!(
            &controller.rust().queued,
            Some(Job::Write(Operation::Refresh { backend })) if backend == "fixture"
        ));
        assert!(*controller.busy());
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
    fn protected_cleanup_inspection_requires_explicit_confirmation() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().propose("inspect-clean".into(), -1);
        assert!(matches!(
            controller.rust().pending,
            Some(Job::InspectCleanup)
        ));
        assert!(controller
            .confirmation()
            .to_string()
            .contains("nothing will be removed"));
        assert!(controller.rust().worker.is_none());
        controller.as_mut().confirm(false);
        assert!(controller.rust().pending.is_none());
        assert!(controller.rust().worker.is_none());
        assert!(Job::InspectCleanup.writes()); // Locks navigation during authentication only.
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
            Some(Job::Write(Operation::Clean(_)))
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
        )));
        assert!(controller.status().to_string().contains("Restart required"));
        assert!(controller.rust().packages.is_empty());
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
            Some(Job::Write(Operation::Upgrade(_)))
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
            Job::Write(Operation::Upgrade(firmware.id.clone())),
            Job::UpgradeAll(vec![Operation::Upgrade(firmware.id.clone())]),
        ] {
            let mut replies = vec![];
            execute(
                &mut firmware_engine,
                job,
                &Cancellation::default(),
                &mut |reply| replies.push(reply),
            );
            let Reply::Done(Ok(Payload::Batch(status))) = replies.pop().unwrap() else {
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
                Job::Write(Operation::Upgrade(id.clone())),
                Job::UpgradeAll(vec![Operation::UpgradeAll {
                    backend: id.backend.clone(),
                }]),
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
                    Reply::Done(Ok(Payload::Batch(status))) => {
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
