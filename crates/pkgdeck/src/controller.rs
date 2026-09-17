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
    Done(Result<Payload, EngineError>),
}
fn execute(engine: &mut Engine, job: Job, cancel: &Cancellation, send: &mut dyn FnMut(Reply)) {
    let result = match job {
        Job::Load(view, query) => {
            if view == "Sources" || view == "Discover" {
                Ok(Payload::Sources(engine.discover(cancel)))
            } else {
                let mut report = if view == "Search" {
                    engine.search(&query, cancel)
                } else {
                    engine.installed(cancel)
                };
                if view == "Updates" {
                    report
                        .packages
                        .retain(|p| p.update == UpdateAvailability::Available);
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
    busy: bool,
    upgradable: bool,
    updates_view: bool,
    packages: Vec<Package>,
    detail_cache: BTreeMap<PackageId, QString>,
    sources: Vec<Source>,
    pending: Option<Job>,
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
            busy: false,
            upgradable: false,
            updates_view: false,
            packages: vec![],
            detail_cache: BTreeMap::new(),
            sources: vec![],
            pending: None,
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
        let handle = thread::spawn(move || {
            let discover =
                matches!(&job, Job::Load(view, _) if view == "Sources" || view == "Discover");
            let mut send = |reply| {
                let _ = sender.send(reply);
            };
            match pkgdeck_core::backends::native_engine(
                source.as_deref(),
                discover,
                authorization,
                &token,
            ) {
                Ok(mut engine) => execute(&mut engine, job, &token, &mut send),
                Err(error) => send(Reply::Done(Err(error))),
            }
        });
        self.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel,
        });
        self.as_mut().set_status("Working…".into());
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
        if !["Search", "Installed", "Updates", "Sources", "Discover"].contains(&view.as_str())
            || (view == "Search" && query.trim().is_empty())
        {
            return;
        }
        if ![
            "", "apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak",
            "cargo", "npm", "pnpm", "bun",
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
        self.as_mut().rust_mut().sources.clear();
        self.as_mut().rust_mut().pending = None;
        self.as_mut().set_confirmation(QString::default());
        self.as_mut().set_rows("[]".into());
        self.as_mut().set_details("{}".into());
        self.start(Job::Load(view, query));
    }
    pub fn select(mut self: Pin<&mut Self>, index: i32) {
        if self.rust().worker.is_some() {
            return;
        }
        if let Some(package) = usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().packages.get(i))
            .cloned()
        {
            if let Some(details) = self.rust().detail_cache.get(&package.id).cloned() {
                self.as_mut().set_details(details);
                self.set_status("Package details loaded.".into());
                return;
            }
            self.as_mut().set_details(encoded(
                json!({"package": package_row(&package), "description": package.summary}),
            ));
            self.start(Job::Details(package.id));
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
        self.as_mut().set_confirmation(QString::default());
        if approved {
            if let Some(op) = pending {
                self.start(op);
            }
        }
    }
    pub fn cancel(mut self: Pin<&mut Self>) {
        if let Some(worker) = &self.rust().worker {
            worker.cancel.cancel();
            self.as_mut().set_status(
                "Cancellation requested; waiting for the native operation to finish safely.".into(),
            );
        }
    }
    fn apply(mut self: Pin<&mut Self>, result: Result<Payload, EngineError>) {
        match result {
            Err(e) => self.set_status(e.to_string().as_str().into()),
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
                self.as_mut().set_rows(encoded(rows));
                self.set_status(status.as_str().into());
            }
            Ok(Payload::Sources(sources)) => {
                let rows: Vec<_> = sources.iter().map(|s| json!({"kind": "source", "name": s.backend, "source": s.backend, "summary": source_status(s), "available": s.availability == Ok(Availability::Available)})).collect();
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
                self.as_mut().set_details(data);
                self.set_status("Package details loaded.".into());
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
                if let Reply::Done(result) = reply {
                    self.as_mut().apply(result);
                }
            }
            if joined.is_err() {
                self.as_mut().set_status("Backend worker failed.".into());
            }
            self.set_busy(false);
        } else {
            for reply in replies {
                if let Reply::Progress(message) = reply {
                    self.as_mut().set_status(message.as_str().into());
                }
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
                Job::Load("Discover".into(), "".into()),
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
