//! Thin Qt facade: native work runs on a worker; Qt properties change only on the GUI thread.
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use pkgdeck_core::{engine::*, host::Authorization, package::*, process::Cancellation};
use serde_json::{json, Value};
use std::{collections::BTreeMap, pin::Pin, sync::mpsc, thread};

// CXX-Qt generates the FFI boundary; application code below uses safe Rust.
#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, rows)]
        #[qproperty(QString, details)]
        #[qproperty(QString, status)]
        #[qproperty(QString, confirmation)]
        #[qproperty(QString, version)]
        #[qproperty(bool, busy)]
        #[qproperty(bool, upgradable)]
        type PackageController = super::Controller;
        #[qinvokable]
        fn load(
            self: Pin<&mut PackageController>,
            view: QString,
            query: QString,
            source: QString,
            sudo: bool,
        );
        #[qinvokable]
        fn select(self: Pin<&mut PackageController>, index: i32);
        #[qinvokable]
        fn propose(self: Pin<&mut PackageController>, action: QString, index: i32);
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
    Load(String, String),
    Details(PackageId),
    Write(Operation),
    UpgradeAll(Vec<Operation>),
}
enum Payload {
    Packages(PackageReport),
    Sources(Vec<Source>),
    Details(Box<PackageDetails>),
    Written(OperationOutcome),
    Batch(String),
}
enum Reply {
    Progress(String),
    Partial(PackageReport),
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
        Job::Load(view, query) => {
            if view == "Sources" {
                Ok(Payload::Sources(engine.discover(cancel)))
            } else if view == "Search" {
                // Stream cumulative partials so fast backends render while
                // slow ones still query; the terminal emission below carries
                // the same deterministic report as a synchronous query.
                let mut send_partial = |partial| send(Reply::Partial(partial));
                let report = engine.search_stream(&query, cancel, &mut send_partial);
                Ok(Payload::Packages(report))
            } else {
                // Installed and Updates stay synchronous: upgrade gating
                // needs complete failures, which partials cannot promise.
                let mut report = engine.installed(cancel);
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
        Job::UpgradeAll(operations) => {
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
            let mut status = format!(
                "Completed {completed} of {} upgrades. Reload to see current package state.",
                operations.len()
            );
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
            .map(Payload::Written),
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
    version: QString,
    busy: bool,
    upgradable: bool,
    updates_view: bool,
    packages: Vec<Package>,
    failures: Vec<BackendFailure>,
    detail_cache: BTreeMap<PackageId, QString>,
    sources: Vec<Source>,
    pending: Option<Job>,
    queued: Option<Job>,
    selected: Option<PackageId>,
    engine: Option<Engine>,
    busy_since: Option<std::time::Instant>,
    progressed: bool,
    source: Option<String>,
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
            version: pkgdeck_core::VERSION.into(),
            busy: false,
            upgradable: false,
            updates_view: false,
            packages: vec![],
            failures: vec![],
            detail_cache: BTreeMap::new(),
            sources: vec![],
            pending: None,
            queued: None,
            selected: None,
            engine: None,
            busy_since: None,
            progressed: false,
            source: None,
            sudo: false,
            worker: None,
        }
    }
}
fn upgrade_plan(packages: &[Package]) -> Vec<Operation> {
    let backends: std::collections::BTreeSet<_> = packages
        .iter()
        .filter(|p| p.installed_version.is_some() && p.update == UpdateAvailability::Available)
        .map(|p| p.id.backend.clone())
        .collect();
    backends
        .into_iter()
        .map(|backend| Operation::UpgradeAll { backend })
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
/// detection roundtrip. All other jobs use the session source filter.
fn engine_source(job: &Job, filter: Option<&str>) -> Option<String> {
    match job {
        Job::Details(id) => Some(id.backend.clone()),
        _ => filter.map(str::to_owned),
    }
}
/// A finished details reply is fresh only while its identity is still the
/// current selection; rapid navigation supersedes slower replies.
fn details_fresh(selected: Option<&PackageId>, details: &PackageDetails) -> bool {
    selected.is_some_and(|id| *id == details.package.id)
}
/// Show the working status only for jobs that actually take time, so fast
/// selections never flash it. Progress messages bypass this entirely.
fn working_grace_exceeded(since: Option<std::time::Instant>, progressed: bool) -> bool {
    !progressed
        && since.is_some_and(|started| started.elapsed() > std::time::Duration::from_millis(200))
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
fn operation_label(operation: &Operation) -> String {
    let (action, id) = match operation {
        Operation::Install(id) => ("Install", id),
        Operation::Remove(id) => ("Remove", id),
        Operation::Upgrade(id) => ("Upgrade", id),
        Operation::Refresh { backend } => return format!("Refresh metadata for {backend}"),
        Operation::UpgradeAll { backend } => return format!("Upgrade all packages from {backend}"),
    };
    format!(
        "{action} {}\nSource: {}\nArchitecture: {}\nScope: {}",
        id.name,
        id.backend,
        id.architecture,
        scope_label(&id.scope)
    )
}
fn package_row(p: &Package) -> Value {
    json!({"name": p.id.name, "source": p.id.backend, "architecture": p.id.architecture,
        "remote": p.id.remote, "scope": p.id.scope, "scope_label": scope_label(&p.id.scope), "summary": p.summary, "installed": p.installed_version,
        "candidate": p.candidate_version, "update": p.update, "kind": "package"})
}
impl ffi::PackageController {
    fn start(mut self: Pin<&mut Self>, job: Job) {
        if self.rust().worker.is_some() {
            return;
        }
        let source = self.rust().source.clone();
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
        // Every other job inherits the warm engine when one survived.
        let cached = self.as_mut().rust_mut().engine.take();
        let handle = thread::spawn(move || {
            let sources_view = matches!(&job, Job::Load(view, _) if view == "Sources");
            let mut send = |reply| {
                let _ = sender.send(reply);
            };
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
                engine_source(&job, source.as_deref()).as_deref(),
                sources_view,
                authorization,
                &token,
            ) {
                Ok(mut engine) => {
                    execute(&mut engine, job, &token, &mut send);
                    send(Reply::Engine(engine));
                }
                Err(error) => send(Reply::Done(Err(error))),
            }
        });
        self.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel,
            job: worker_job,
        });
        self.as_mut().rust_mut().busy_since = Some(std::time::Instant::now());
        self.as_mut().rust_mut().progressed = false;
        self.set_busy(true);
    }
    pub fn load(
        mut self: Pin<&mut Self>,
        view: QString,
        query: QString,
        source: QString,
        sudo: bool,
    ) {
        if self.rust().worker.is_some() {
            return;
        }
        let view = view.to_string();
        let query = query.to_string();
        let source = source.to_string();
        if !["Search", "Installed", "Updates", "Sources"].contains(&view.as_str())
            || (view == "Search" && query.trim().is_empty())
        {
            return;
        }
        if ![
            "", "apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak",
            "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem",
        ]
        .contains(&source.as_str())
        {
            self.set_status("Unknown source.".into());
            return;
        }
        self.as_mut().set_upgradable(false);
        self.as_mut().rust_mut().updates_view = view == "Updates";
        self.as_mut().rust_mut().source = (!source.is_empty()).then_some(source);
        self.as_mut().rust_mut().sudo = sudo;
        self.as_mut().rust_mut().detail_cache.clear();
        self.as_mut().rust_mut().packages.clear();
        self.as_mut().rust_mut().failures.clear();
        self.as_mut().rust_mut().sources.clear();
        self.as_mut().rust_mut().pending = None;
        self.as_mut().rust_mut().queued = None;
        self.as_mut().rust_mut().selected = None;
        self.as_mut().set_confirmation(QString::default());
        self.as_mut().set_rows("[]".into());
        self.as_mut().set_details("{}".into());
        self.start(Job::Load(view, query));
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
            // are never preempted; their selection simply waits.
            if let Some(worker) = &self.rust().worker {
                if matches!(worker.job, Job::Details(_)) {
                    worker.cancel.cancel();
                    self.as_mut().rust_mut().queued = Some(Job::Details(package.id));
                    return;
                }
                if matches!(worker.job, Job::Load(..)) {
                    self.as_mut().rust_mut().queued = Some(Job::Details(package.id));
                    return;
                }
                return;
            }
            self.as_mut().set_details(encoded(
                json!({"package": package_row(&package), "description": package.summary}),
            ));
            self.start(Job::Details(package.id));
        } else if let Some(failure) = usize::try_from(index)
            .ok()
            .and_then(|i| {
                i.checked_sub(self.rust().packages.len())
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
    pub fn propose(mut self: Pin<&mut Self>, action: QString, index: i32) {
        if self.rust().worker.is_some() {
            return;
        }
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
            let labels = operations
                .iter()
                .map(operation_label)
                .collect::<Vec<_>>()
                .join("\n\n");
            self.as_mut().set_confirmation(format!("Upgrade all {count} listed packages?\n\n{labels}\n\nEach source runs as one transaction. Native dependency changes may follow. Successful upgrades are not rolled back if another fails. Continue?").as_str().into());
            self.rust_mut().pending = Some(Job::UpgradeAll(operations));
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
            } else {
                self.rust()
                    .packages
                    .get(i)
                    .and_then(|p| match action.as_str() {
                        "install" if p.installed_version.is_none() => {
                            Some(Operation::Install(p.id.clone()))
                        }
                        "remove" if p.installed_version.is_some() => {
                            Some(Operation::Remove(p.id.clone()))
                        }
                        "upgrade" if p.update == UpdateAvailability::Available => {
                            Some(Operation::Upgrade(p.id.clone()))
                        }
                        _ => None,
                    })
            }
        });
        if let Some(op) = &operation {
            self.as_mut().set_confirmation(
                format!(
                    "{}\n\nNative dependency changes may follow. Continue?",
                    operation_label(op)
                )
                .as_str()
                .into(),
            );
        } else {
            self.as_mut().set_confirmation(QString::default());
        }
        self.rust_mut().pending = operation.map(Job::Write);
    }
    pub fn confirm(mut self: Pin<&mut Self>, approved: bool) {
        if self.rust().worker.is_some() {
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
                let upgradable = self.rust().updates_view
                    && report.failures.is_empty()
                    && !upgrade_plan(&report.packages).is_empty();
                self.as_mut().set_upgradable(upgradable);
                let mut rows: Vec<_> = report.packages.iter().map(package_row).collect();
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
            Ok(Payload::Sources(sources)) => {
                let rows: Vec<_> = sources.iter().map(|s| json!({"kind": "source", "name": s.backend, "source": s.backend, "summary": source_status(s), "available": s.availability == Ok(Availability::Available), "capabilities": s.capabilities})).collect();
                self.as_mut().rust_mut().sources = sources;
                self.as_mut().set_rows(encoded(rows));
                self.set_status("Source availability checked. Select a source for details.".into());
            }
            Ok(Payload::Details(details)) => {
                let data = encoded(
                    json!({"package": package_row(&details.package), "description": details.description, "homepage": details.homepage, "dependencies": details.dependencies}),
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
                self.as_mut().set_upgradable(false);
                self.as_mut().rust_mut().detail_cache.clear();
                self.as_mut().rust_mut().packages.clear();
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
    pub fn poll(mut self: Pin<&mut Self>) {
        let Some(worker) = &self.rust().worker else {
            return;
        };
        let replies: Vec<_> = worker.receiver.try_iter().collect();
        let complete =
            replies.iter().any(|r| matches!(r, Reply::Done(_))) || worker.handle.is_finished();
        if complete {
            let worker = self.as_mut().rust_mut().worker.take().unwrap();
            let joined = worker.handle.join();
            for reply in replies.into_iter().chain(worker.receiver.try_iter()) {
                match reply {
                    // The join above guarantees the thread finished sending.
                    Reply::Engine(engine) => {
                        self.as_mut().rust_mut().engine = Some(engine);
                    }
                    Reply::Partial(report) => self.as_mut().apply(Ok(Payload::Packages(report))),
                    Reply::Done(result) => self.as_mut().apply(result),
                    Reply::Progress(_) => {}
                }
            }
            if joined.is_err() {
                self.as_mut().set_status("Backend worker failed.".into());
            }
            self.as_mut().rust_mut().busy_since = None;
            let queued = self.as_mut().rust_mut().queued.take();
            self.as_mut().set_busy(false);
            // A selection that arrived while the worker was busy starts now
            // that the previous job has fully terminated.
            if let Some(job) = queued {
                self.start(job);
            }
        } else {
            let mut progressed = false;
            for reply in replies {
                match reply {
                    Reply::Partial(report) => self.as_mut().apply(Ok(Payload::Packages(report))),
                    Reply::Progress(message) => {
                        progressed = true;
                        self.as_mut().set_status(message.as_str().into());
                    }
                    _ => {}
                }
            }
            if progressed {
                self.as_mut().rust_mut().progressed = true;
            } else if working_grace_exceeded(self.rust().busy_since, self.rust().progressed) {
                self.as_mut().set_status("Working…".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        package: Package,
        fail: bool,
    }
    impl Backend for Fixture {
        fn id(&self) -> &str {
            "fixture"
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
            },
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
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
        };
        assert_eq!(
            engine_source(&Job::Details(id.clone()), None),
            Some("homebrew".into())
        );
        assert_eq!(
            engine_source(&Job::Details(id), Some("apt")),
            Some("homebrew".into())
        );
        assert_eq!(
            engine_source(&Job::Load("Installed".into(), "".into()), Some("apt")),
            Some("apt".into())
        );
        assert_eq!(
            engine_source(&Job::Load("Installed".into(), "".into()), None),
            None
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
    fn working_status_waits_out_fast_jobs() {
        assert!(!working_grace_exceeded(None, false));
        assert!(!working_grace_exceeded(
            Some(std::time::Instant::now()),
            false
        ));
        assert!(!working_grace_exceeded(
            Some(std::time::Instant::now()),
            true
        ));
        assert!(working_grace_exceeded(
            Some(std::time::Instant::now() - std::time::Duration::from_millis(500)),
            false
        ));
        assert!(!working_grace_exceeded(
            Some(std::time::Instant::now() - std::time::Duration::from_millis(500)),
            true
        ));
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
            },
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
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
    fn jobs_keep_source_identity_native_updates_and_typed_failures() {
        let id = PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
        };
        let package = Package {
            id: id.clone(),
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
        };
        assert_eq!(package_row(&package)["source"], "fixture");
        assert!(encoded(package_row(&package))
            .to_string()
            .contains("synthetic"));
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
                        assert!(status.contains("Upgrade all packages from fixture"));
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
