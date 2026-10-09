//! The window's logic: native work runs on workers; properties change only
//! on the window's thread, when it polls.
use crate::qt::{QString, QUrl};
use pkgdeck_core::activity::{History, Outcome, State};
use pkgdeck_core::backends::AppImage;
use pkgdeck_core::background::{self, Schedule};
use pkgdeck_core::repositories::{self, Action as RepositoryAction};
use pkgdeck_core::repository_input::{self, Import as RepositoryImport};
use pkgdeck_core::{
    engine::*,
    failed_updates::FailedUpdates,
    host::Authorization,
    manifest,
    needs_password::NeedsPassword,
    package::*,
    process::{Cancellation, ExecutionError},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    pin::Pin,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Most search results the list shows. A short query can match tens of
/// thousands of packages, and laying them all out froze the window.
const SEARCH_ROW_LIMIT: usize = 500;
/// Keep only the best [`SEARCH_ROW_LIMIT`] matches, ranked like the list
/// ranks them, and return how many there were.
fn keep_best_matches(report: &mut PackageReport, query: &str) -> usize {
    let total = report.packages.len();
    if total > SEARCH_ROW_LIMIT {
        rank_search_matches(&mut report.packages, query);
        report.packages.truncate(SEARCH_ROW_LIMIT);
    }
    total
}

fn local_input_path(input: &str) -> Result<PathBuf, String> {
    let input = input.trim();
    let path = if let Some(encoded) = input.strip_prefix("file://") {
        let encoded = encoded
            .strip_prefix("localhost/")
            .map_or(encoded, |rest| rest);
        let encoded = if input.starts_with("file://localhost/") {
            format!("/{encoded}")
        } else {
            encoded.to_owned()
        };
        if !encoded.starts_with('/') {
            return Err("Only local file URLs are supported.".into());
        }
        let mut bytes = Vec::with_capacity(encoded.len());
        let mut chars = encoded.as_bytes().iter().copied();
        while let Some(byte) = chars.next() {
            if byte == b'%' {
                let high = chars.next().and_then(|value| (value as char).to_digit(16));
                let low = chars.next().and_then(|value| (value as char).to_digit(16));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err("This file link is not encoded correctly.".into());
                };
                let decoded = ((high << 4) | low) as u8;
                if decoded == 0 {
                    return Err("This file link contains invalid characters.".into());
                }
                bytes.push(decoded);
            } else {
                bytes.push(byte);
            }
        }
        String::from_utf8(bytes)
            .map_err(|_| "This file link contains invalid characters.".to_owned())?
    } else if input.contains("://") {
        return Err("Unsupported link. Use an HTTPS package or repository link.".into());
    } else {
        input.to_owned()
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("Choose an absolute local file path.".into());
    }
    Ok(path)
}

/// The controller as a front end holds it: its properties, read and set on
/// the window's thread, with listeners told when one changes. These are the
/// properties and calls the Qt window had; the logic below is unchanged.
pub mod ffi {
    use super::Controller;
    use crate::qt::{QString, UniquePtr};
    use std::pin::Pin;

    pub struct PackageController {
        rust: Controller,
        listeners: Vec<(&'static str, Listener)>,
    }
    type Listener = Box<dyn FnMut(Pin<&mut PackageController>)>;
    /// A listener stays until the controller goes; `release` keeps Qt's
    /// spelling for that.
    pub struct Connection;
    impl Connection {
        pub fn release(self) {}
    }
    pub fn create_controller() -> UniquePtr<PackageController> {
        let mut rust = Controller::default();
        rust.open_view_store(super::ViewStore::default_path());
        UniquePtr::new(PackageController {
            rust,
            listeners: Vec::new(),
        })
    }
    /// A controller that never reaches the running system, for a front
    /// end's tests: `engine` stands in for the package managers, and there
    /// is no activity log, login item, relaunch, password prompt or web
    /// lookup.
    pub fn create_synthetic_controller(engine: super::EngineMaker) -> UniquePtr<PackageController> {
        let mut rust = Controller::default();
        rust.natives = super::synthetic_natives(engine);
        rust.activity_store = None;
        rust.autostart_path = None;
        rust.install = None;
        rust.needs_password = super::NeedsPassword::default();
        rust.failed_updates = super::FailedUpdates::default();
        UniquePtr::new(PackageController {
            rust,
            listeners: Vec::new(),
        })
    }
    impl PackageController {
        pub fn rust(&self) -> &Controller {
            &self.rust
        }
        pub fn rust_mut(self: Pin<&mut Self>) -> &mut Controller {
            &mut self.get_mut().rust
        }
        fn changed(mut self: Pin<&mut Self>, property: &'static str) {
            let mut listeners = std::mem::take(&mut self.listeners);
            for (name, listener) in &mut listeners {
                if *name == property {
                    listener(self.as_mut());
                }
            }
            listeners.append(&mut self.listeners);
            self.listeners = listeners;
        }
        fn listen(
            mut self: Pin<&mut Self>,
            property: &'static str,
            listener: Listener,
        ) -> Connection {
            self.listeners.push((property, listener));
            Connection
        }
    }
    macro_rules! properties {
        ($($name:ident, $set:ident, $on:ident: $type:ty;)*) => {
            impl PackageController {
                $(
                    pub fn $name(&self) -> &$type {
                        &self.rust.$name
                    }
                    pub fn $set(mut self: Pin<&mut Self>, value: $type) {
                        if self.rust.$name != value {
                            self.as_mut().get_mut().rust.$name = value;
                            self.changed(stringify!($name));
                        }
                    }
                    pub fn $on(
                        self: Pin<&mut Self>,
                        listener: impl FnMut(Pin<&mut PackageController>) + 'static,
                    ) -> Connection {
                        self.listen(stringify!($name), Box::new(listener))
                    }
                )*
            }
        };
    }
    properties! {
        rows, set_rows, on_rows_changed: QString;
        details, set_details, on_details_changed: QString;
        status, set_status, on_status_changed: QString;
        notice, set_notice, on_notice_changed: QString;
        progress, set_progress, on_progress_changed: QString;
        repositories, set_repositories, on_repositories_changed: QString;
        source_catalog, set_source_catalog, on_source_catalog_changed: QString;
        report_state, set_report_state, on_report_state_changed: QString;
        pending_sources, set_pending_sources, on_pending_sources_changed: QString;
        manifest_preview, set_manifest_preview, on_manifest_preview_changed: QString;
        activity, set_activity, on_activity_changed: QString;
        background_state, set_background_state, on_background_state_changed: QString;
        notification_history, set_notification_history, on_notification_history_changed: QString;
        system_approval, set_system_approval, on_system_approval_changed: QString;
        approval_error, set_approval_error, on_approval_error_changed: QString;
        auto_update_result, set_auto_update_result, on_auto_update_result_changed: QString;
        held_updates, set_held_updates, on_held_updates_changed: QString;
        self_update, set_self_update, on_self_update_changed: QString;
        confirmation, set_confirmation, on_confirmation_changed: QString;
        confirmation_data, set_confirmation_data, on_confirmation_data_changed: QString;
        opened, set_opened, on_opened_changed: QString;
        version, set_version, on_version_changed: QString;
        busy, set_busy, on_busy_changed: bool;
        writing, set_writing, on_writing_changed: bool;
        reading, set_reading, on_reading_changed: bool;
        upgradable, set_upgradable, on_upgradable_changed: bool;
        needs_poll, set_needs_poll, on_needs_poll_changed: bool;
        refreshing, set_refreshing, on_refreshing_changed: bool;
    }
}

#[derive(Clone)]
enum Job {
    OpenInput(String),
    ImportRepository(RepositoryImport),
    Repositories(Option<RepositoryAction>),
    Load(String, String),
    BackgroundUpdates(Vec<String>),
    RetrySource(String, String, String),
    RetryFailedUpdates(Vec<String>),
    Details(PackageId),
    PlanOperation(Operation),
    /// Resolve the Homebrew cask that can take over a macOS app row and
    /// preview that adoption.
    PlanAdoption(Box<Package>),
    /// Check every AppImage installed some other way before PkgDeck moves
    /// them all in, with one review.
    PlanAdoptAll(Vec<Package>),
    PlanCleanAll(Vec<Operation>),
    Write(Operation, Option<Box<TransactionPlan>>),
    PlanUpgrade(Vec<Operation>, usize),
    UpgradeAll(Vec<Operation>, Option<AptUpgradePlan>),
    /// Updates a background check found, applied with nobody watching:
    /// system managers only through the upgrade-only helper. With `true`,
    /// an APT update may remove packages.
    AutoUpgrade(Vec<Operation>, bool),
    CleanAll(Vec<Operation>),
    ManifestExport(PathBuf, Vec<PackageId>),
    ManifestPreview(PathBuf),
}
impl Job {
    fn writes(&self) -> bool {
        matches!(
            self,
            Self::Write(..)
                | Self::UpgradeAll(..)
                | Self::AutoUpgrade(..)
                | Self::CleanAll(_)
                | Self::Repositories(Some(_))
                | Self::ImportRepository(_)
        )
    }
    /// Previews that decide whether a confirmed change may run.
    fn reviews(&self) -> bool {
        matches!(
            self,
            Self::PlanOperation(_) | Self::PlanUpgrade(..) | Self::PlanCleanAll(_)
        )
    }
    fn operations(&self) -> Vec<Operation> {
        match self {
            Self::Write(operation, _) => vec![operation.clone()],
            Self::UpgradeAll(operations, _)
            | Self::AutoUpgrade(operations, _)
            | Self::CleanAll(operations) => operations.clone(),
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
    /// An opened file or link, with what it says about itself.
    OpenPackage(Box<PackageDetails>),
    OpenRepository(RepositoryImport),
    Repositories(repositories::Report),
    Packages(PackageReport),
    BackgroundUpdates(PackageReport),
    RetryPackages(String, PackageReport),
    RetryFailedUpdates(Vec<String>, PackageReport),
    RetryCleanup(String, CleanupReport),
    RetrySources(String, Vec<Source>),
    Sources(Vec<Source>),
    Details(Box<PackageDetails>),
    Written(OperationOutcome),
    Batch(String, Vec<Outcome>),
    Cleanup(CleanupReport),
    UpgradePreview(Vec<Operation>, usize, Option<AptUpgradePlan>),
    OperationPreview(Operation, Option<Box<TransactionPlan>>),
    /// The cask package that adopts an app, its install and the adoption plan.
    AdoptionPreview(Box<Package>, Operation, Box<TransactionPlan>),
    /// The AppImages that can be moved in, each with its plan, and the
    /// names of those that can't any more.
    AdoptAllPreview(Vec<(Package, Operation, Box<TransactionPlan>)>, Vec<String>),
    ManifestExport(usize),
    ManifestPreview(manifest::Preview),
    CleanPreview(Vec<Operation>, Vec<CleanupItem>),
}
enum Reply {
    Progress(String),
    ProgressEvent(Event),
    /// A line of output from an Update all step that names a package.
    Output(String),
    /// What the failed steps of a batch printed, for the banner's details.
    FailureOutput(String),
    /// A cask whose update stopped for the administrator password.
    NeedsPassword(PackageId),
    /// The sources a streaming load asked, sent before its first partial.
    Asked(Vec<String>),
    /// The sources a preload's latest partial heard from: a preload keeps
    /// its rows for the end, but the section waiting on it names the rest.
    Answered(Vec<String>),
    Partial(PackageReport),
    /// How many packages the search matched, sent before each search
    /// report that may carry only the best of them.
    Matches(usize),
    /// The rows for a report's packages, made on the worker before the
    /// report is sent, so the window never encodes thousands of them.
    Rows(Box<PreparedRows>),
    /// Every installed package, and whether update indexes were refreshed
    /// first. Only a refreshed inventory may stand in for Updates.
    Inventory(PackageReport, bool),
    DetailsPreview(Box<PackageDetails>),
    Done(Result<Payload, EngineError>),
    Engine(Box<Engine>),
}
/// Progress of the running change, for the progress bar and for marking
/// the rows it changes. See `snapshot` for the fields the frontend reads.
struct ProgressState {
    activity_id: Option<u64>,
    label: String,
    done: usize,
    total: usize,
    transferred: u64,
    transfer_total: Option<u64>,
    operations: Vec<Operation>,
    /// The operation running now; before the first one starts, the job's
    /// only operation, if it has just one.
    current: Option<Operation>,
    /// Rows a whole-source step is expected to update, by source, from
    /// the Updates list (see `with_rows`).
    rows: BTreeMap<String, Vec<PackageId>>,
    /// The package a whole-source step reported it is working on now.
    item: Option<PackageId>,
    /// Packages already done, successfully or not.
    finished: Vec<PackageId>,
    names: Names,
}
impl ProgressState {
    /// Counts each package a whole-source step updates as a step of its
    /// own, using the rows with updates in `packages`, so "Update all"
    /// reads "3 of 12" packages instead of "1 of 2" sources.
    fn with_rows(mut self, packages: &[Package]) -> Self {
        for operation in &self.operations {
            if let Operation::UpgradeAll { backend } = operation {
                let rows: Vec<_> = packages
                    .iter()
                    .filter(|p| {
                        p.id.backend == *backend
                            && p.installed_version.is_some()
                            && p.update == UpdateAvailability::Available
                    })
                    .map(|p| p.id.clone())
                    .collect();
                for package in packages.iter().filter(|p| rows.contains(&p.id)) {
                    self.names
                        .entry(package.id.clone())
                        .or_insert_with(|| package.display_name.clone());
                }
                if !rows.is_empty() {
                    self.total += rows.len() - 1;
                }
                self.rows.insert(backend.clone(), rows);
            }
        }
        self
    }
    /// A whole-source step moved on to the package `name` (as its source
    /// names it: the row's name or the name people see). The package
    /// before it is done. False when no row of that source matches.
    fn move_to(&mut self, backend: &str, name: &str) -> bool {
        let Some(id) = self.rows.get(backend).and_then(|rows| {
            rows.iter()
                .find(|id| {
                    id.name == name || self.names.get(*id).is_some_and(|shown| shown == name)
                })
                .cloned()
        }) else {
            return false;
        };
        if let Some(previous) = self.item.take().filter(|previous| *previous != id) {
            self.finish(previous);
        }
        self.label = format!(
            "Update {}",
            self.names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| id.name.clone())
        );
        self.item = Some(id);
        true
    }
    /// Follows a line of output from the running whole-source step.
    fn observe(&mut self, line: &str) -> bool {
        let Some(Operation::UpgradeAll { backend }) = self.current.clone() else {
            return false;
        };
        pkgdeck_core::backends::output_package(&backend, line)
            .is_some_and(|name| self.move_to(&backend, &name))
    }
    /// Marks a package done and counts it, once.
    fn finish(&mut self, id: PackageId) {
        if !self.finished.contains(&id) {
            self.finished.push(id);
            self.done = (self.done + 1).min(self.total);
        }
    }
    fn new(job: &Job, activity_id: Option<u64>, names: &Names) -> Self {
        let operations = job.operations();
        let label = operations
            .first()
            .map(|operation| operation_title_named(operation, names))
            .unwrap_or_else(|| match job {
                Job::ImportRepository(import) => format!("Add {} repository", import.name),
                Job::Repositories(Some(action)) => match &action.change {
                    repositories::Change::Add { .. } => format!("Add {} repository", action.name),
                    repositories::Change::Remove => format!("Remove {} repository", action.name),
                    repositories::Change::SetEnabled { enabled: true } => {
                        format!("Enable {} repository", action.name)
                    }
                    repositories::Change::SetEnabled { enabled: false } => {
                        format!("Disable {} repository", action.name)
                    }
                    repositories::Change::SetPriority { .. } => {
                        format!("Change {} priority", action.name)
                    }
                    repositories::Change::OpenEditor => "Open software sources".into(),
                },
                _ => "Working".into(),
            });
        let current = match operations.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        };
        Self {
            activity_id,
            label,
            done: 0,
            total: operations.len().max(1),
            transferred: 0,
            transfer_total: None,
            operations,
            current,
            rows: BTreeMap::new(),
            item: None,
            finished: vec![],
            names: names.clone(),
        }
    }
    fn apply(&mut self, event: &Event) {
        match event {
            Event::Started(operation) => {
                self.label = operation_title_named(operation, &self.names);
                self.current = Some(operation.clone());
                self.item = None;
                self.transferred = 0;
                self.transfer_total = None;
            }
            Event::Progress {
                operation: Operation::UpgradeAll { backend },
                progress: Progress::Package(name),
            } => {
                self.move_to(backend, name);
            }
            Event::Progress {
                progress: Progress::Transfer { completed, total },
                ..
            } => {
                self.transferred = *completed;
                self.transfer_total = (*total).filter(|total| *total > 0);
            }
            Event::Finished { operation, .. } => {
                let rows = match operation {
                    Operation::UpgradeAll { backend } => {
                        self.rows.get(backend).cloned().unwrap_or_default()
                    }
                    _ => package_target(operation).cloned().into_iter().collect(),
                };
                if rows.is_empty() {
                    self.done = (self.done + 1).min(self.total);
                }
                for id in rows {
                    self.finish(id);
                }
                self.item = None;
                self.transferred = 0;
                self.transfer_total = None;
            }
            Event::Progress { .. } => {}
        }
    }
    /// How far the whole change is, from 0 to 1, when that is known: from
    /// finished steps of a batch plus the running step's transfer. A single
    /// step without a transfer size has no known fraction.
    fn fraction(&self) -> Option<f64> {
        let step = self
            .transfer_total
            .map(|total| (self.transferred as f64 / total as f64).min(1.0));
        if self.done >= self.total {
            Some(1.0)
        } else if self.total > 1 {
            Some((self.done as f64 + step.unwrap_or(0.0)) / self.total as f64)
        } else {
            step
        }
    }
    fn snapshot(&self) -> QString {
        let targets: Vec<_> = self
            .operations
            .iter()
            .flat_map(|operation| match operation {
                Operation::UpgradeAll { backend } => {
                    self.rows.get(backend).cloned().unwrap_or_default()
                }
                _ => package_target(operation).cloned().into_iter().collect(),
            })
            .map(|id| target_row(&id))
            .collect();
        let sources: Vec<_> = self
            .operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::UpgradeAll { backend } => Some(backend.clone()),
                _ => None,
            })
            .collect();
        let current = self.current.as_ref();
        encoded(json!({
            "activity_id": self.activity_id,
            "label": self.label,
            "done": self.done,
            "total": self.total,
            "transferred": self.transferred,
            "transfer_total": self.transfer_total,
            "fraction": self.fraction(),
            "targets": targets,
            "sources": sources,
            "action": current.map_or("", operation_kind),
            "current": self.item.as_ref().or_else(|| current.and_then(package_target)).map(target_row),
            "finished": self.finished.iter().map(target_row).collect::<Vec<_>>(),
            "current_source": current.map(Operation::backend),
        }))
    }
}
/// A Type 2 AppImage, whatever its file is called.
fn is_appimage(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut header = [0_u8; 11];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| header.starts_with(b"\x7fELF") && &header[8..11] == b"AI\x02")
}
/// An opened package that has nothing more to say than its own fields.
fn opened_payload(package: Package) -> Payload {
    Payload::OpenPackage(Box::new(PackageDetails {
        description: String::new(),
        homepage: package.homepages.first().cloned(),
        dependencies: vec![],
        package,
    }))
}
fn inspect_open_input(input: &str, cancel: &Cancellation) -> Result<Payload, EngineError> {
    if let Some(url) = input.strip_prefix("flatpak+") {
        return pkgdeck_core::flatpak_ref::inspect(url, cancel).map(opened_payload);
    }
    if input.starts_with("https://") {
        if repository_input::supported(input) {
            return repository_input::inspect(input, cancel).map(Payload::OpenRepository);
        }
        if input
            .split(['?', '#'])
            .next()
            .is_some_and(|url| url.ends_with(".flatpakref"))
        {
            return pkgdeck_core::flatpak_ref::inspect(input, cancel).map(opened_payload);
        }
        return pkgdeck_core::artifact::inspect(input, cancel).map(opened_payload);
    }
    let path = local_input_path(input).map_err(|reason| EngineError::InvalidResponse {
        backend: "open".into(),
        reason,
    })?;
    if repository_input::supported(&path.to_string_lossy()) {
        return repository_input::inspect(&path.to_string_lossy(), cancel)
            .map(Payload::OpenRepository);
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let package = match extension {
        _ if extension.eq_ignore_ascii_case("appimage") || is_appimage(&path) => {
            use pkgdeck_core::engine::Backend;
            AppImage::native()
                .search(&path.to_string_lossy(), cancel)
                .and_then(|mut packages| packages.pop().ok_or(EngineError::NotFound))
        }
        "deb" => {
            return pkgdeck_core::local_deb::inspect_details(&path, cancel)
                .map(|details| Payload::OpenPackage(Box::new(details)));
        }
        "flatpakref" => pkgdeck_core::flatpak_ref::inspect(&path.to_string_lossy(), cancel),
        _ => pkgdeck_core::artifact::inspect(&path.to_string_lossy(), cancel),
    };
    package.map(opened_payload)
}
/// Resolve the cask a macOS app row names through the real Homebrew Casks
/// source, as `pkd install --from homebrew-cask` does, and preview installing
/// it. Only a plan that adopts this very copy is offered: an installed cask,
/// or an install that would add a second copy elsewhere, is refused.
fn plan_adoption(
    engine: &mut Engine,
    app: &Package,
    cancel: &Cancellation,
) -> Result<Payload, EngineError> {
    if app.id.backend == "appimage" {
        return plan_appimage_adoption(engine, app, cancel);
    }
    const CASK: &str = "homebrew-cask";
    let refuse = |reason: String| EngineError::InvalidResponse {
        backend: CASK.into(),
        reason: format!("{reason} Nothing was changed."),
    };
    let token = app
        .adopt_with
        .as_deref()
        .filter(|_| app.id.backend == "macos-apps")
        .ok_or(EngineError::NotFound)?;
    let report = engine.lookup_for_mutation(token, cancel);
    if cancel.requested() {
        return Err(EngineError::Cancelled);
    }
    let id = report.select(&Selector {
        name: token.into(),
        backend: Some(CASK.into()),
        architecture: None,
        scope: None,
    })?;
    let cask = report
        .packages
        .into_iter()
        .find(|package| package.id == id)
        .ok_or(EngineError::NotFound)?;
    let name = if app.display_name.is_empty() {
        token
    } else {
        app.display_name.as_str()
    };
    if cask.installed_version.is_some() {
        return Err(refuse(format!(
            "Homebrew already has the {token} cask installed, so it can't take over this copy of {name}."
        )));
    }
    let operation = Operation::Install(cask.id.clone());
    let plan = engine.plan_operation(&operation, cancel)?;
    let same = |a: &std::path::Path, b: &std::path::Path| {
        a == b
            || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
    };
    match plan {
        Some(plan)
            if plan
                .adopts
                .as_deref()
                .is_some_and(|adopted| same(adopted, std::path::Path::new(&app.id.name))) =>
        {
            Ok(Payload::AdoptionPreview(
                Box::new(cask),
                operation,
                Box::new(plan),
            ))
        }
        _ => Err(refuse(format!(
            "The {token} cask would install another copy instead of managing {}.",
            app.id.name
        ))),
    }
}
/// Preview PkgDeck taking over an AppImage installed some other way: the
/// file moves into PkgDeck's folder and gets PkgDeck's own menu entry. The
/// AppImage source plans the move, and checks it again before it happens.
fn plan_appimage_adoption(
    engine: &mut Engine,
    app: &Package,
    cancel: &Cancellation,
) -> Result<Payload, EngineError> {
    if app.adopt_with.as_deref() != Some("appimage") {
        return Err(EngineError::NotFound);
    }
    let mut report = engine.lookup_for_mutation(&app.id.name, cancel);
    if let Some(failure) = report.failures.pop() {
        return Err(failure.error);
    }
    let package = report
        .packages
        .into_iter()
        .find(|package| package.id.backend == "appimage" && package.id.name == app.id.name)
        .ok_or(EngineError::NotFound)?;
    let operation = Operation::Install(package.id.clone());
    match engine.plan_operation(&operation, cancel)? {
        Some(plan) if plan.adopts.is_some() => Ok(Payload::AdoptionPreview(
            Box::new(package),
            operation,
            Box::new(plan),
        )),
        _ => Err(EngineError::InvalidResponse {
            backend: "appimage".into(),
            reason: format!(
                "{} isn't installed some other way anymore. Nothing was changed.",
                app.id.name
            ),
        }),
    }
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
        Job::OpenInput(input) => inspect_open_input(&input, cancel),
        Job::Repositories(_) | Job::ImportRepository(_) => Err(EngineError::NotFound),
        Job::ManifestExport(path, selected) => {
            let installed = engine.installed(cancel);
            if installed.failures.is_empty() {
                manifest::export(&installed.packages, &selected)
                    .and_then(|document| {
                        manifest::write_new(&path, &document)?;
                        Ok(Payload::ManifestExport(document.packages.len()))
                    })
                    .map_err(|error| EngineError::Execution(ExecutionError::Invalid(error.to_string())))
            } else {
                Err(EngineError::Execution(ExecutionError::Invalid(
                    "some package sources could not be read; retry the export".into(),
                )))
            }
        }
        Job::ManifestPreview(path) => manifest::read(&path)
            .and_then(|document| manifest::inspect(engine, &document, cancel))
            .map(Payload::ManifestPreview)
            .map_err(|error| EngineError::Execution(ExecutionError::Invalid(error.to_string()))),
        Job::BackgroundUpdates(_) => {
            let mut report = engine.installed_for_updates(cancel);
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
                send(Reply::Asked(engine.source_ids()));
                let mut send_partial = |mut partial: PackageReport| {
                    partial.packages.retain(|p| !unverified_search_offer(p));
                    send(Reply::Matches(keep_best_matches(&mut partial, &query)));
                    send(Reply::Partial(partial));
                };
                let mut report = engine.search_stream(&query, cancel, &mut send_partial);
                report.packages.retain(|p| !unverified_search_offer(p));
                send(Reply::Matches(keep_best_matches(&mut report, &query)));
                Ok(Payload::Packages(report))
            } else {
                // Installed and Updates stream like Search so rows appear
                // while slow backends still query. Every partial carries
                // the same view filter as the terminal report below.
                // Upgrade gating still waits for the terminal report with
                // complete failures (see apply); partials only fill rows.
                send(Reply::Asked(engine.source_ids()));
                let mut send_partial = |mut partial: PackageReport| {
                    if view == "Updates" {
                        filter_updates(&mut partial);
                    } else {
                        filter_installed(&mut partial, &query);
                    }
                    send(Reply::Partial(partial))
                };
                let mut report = if view == "Updates" {
                    // Homebrew's outdated flag is the local tap. Fetch it
                    // before listing, or a new cask (including PkgDeck) never
                    // appears. Other sources still stream while that runs.
                    engine.installed_for_updates_stream(cancel, &mut send_partial)
                } else {
                    engine.installed_stream(cancel, &mut send_partial)
                };
                send(Reply::Inventory(report.clone(), view == "Updates"));
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
                let mut report = if view == "Search" {
                    engine.search(&query, cancel)
                } else if view == "Updates" {
                    engine.installed_for_updates(cancel)
                } else {
                    engine.installed(cancel)
                };
                if view == "Updates" {
                    filter_updates(&mut report);
                } else if view == "Installed" {
                    filter_installed(&mut report, &query);
                } else {
                    report.packages.retain(|p| !unverified_search_offer(p));
                    keep_best_matches(&mut report, &query);
                }
                Ok(Payload::RetryPackages(source, report))
            }
        }
        Job::RetryFailedUpdates(sources) => {
            let mut report = engine.installed_for_updates(cancel);
            filter_updates(&mut report);
            Ok(Payload::RetryFailedUpdates(sources, report))
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
        Job::PlanAdoption(app) => plan_adoption(engine, &app, cancel),
        Job::PlanAdoptAll(apps) => {
            let mut plans = Vec::new();
            let mut skipped = Vec::new();
            for app in apps {
                if cancel.requested() {
                    return send(Reply::Done(Err(EngineError::Cancelled)));
                }
                match plan_appimage_adoption(engine, &app, cancel) {
                    Ok(Payload::AdoptionPreview(package, operation, plan)) => {
                        plans.push((*package, operation, plan))
                    }
                    _ => skipped.push(app.display_name.clone()),
                }
            }
            Ok(Payload::AdoptAllPreview(plans, skipped))
        }
        Job::PlanCleanAll(operations) => {
            let report = engine.cleanup(cancel);
            Ok(Payload::CleanPreview(operations, report.items))
        }
        Job::UpgradeAll(operations, _) | Job::CleanAll(operations) => {
            Ok(run_batch(engine, &operations, Vec::new(), cancel, send))
        }
        Job::AutoUpgrade(operations, removals) => {
            // Nobody reviews this run. Unless removals are allowed, APT goes
            // ahead only when its dry run removes nothing; otherwise it waits
            // for Update all.
            let apt = |op: &Operation| matches!(op, Operation::UpgradeAll { backend } if backend == "apt");
            let held = if operations.iter().any(apt) {
                match engine.plan_apt_upgrade(cancel) {
                    Ok(plan) if removals || plan.removals.is_empty() => None,
                    Ok(plan) => Some(format!(
                        "Not updated automatically: it would remove {} {}. Review it with Update all.",
                        plan.removals.len(),
                        if plan.removals.len() == 1 { "package" } else { "packages" }
                    )),
                    Err(error) => Some(plain_error(&error, Some("apt"), false)),
                }
            } else {
                None
            };
            let held: Vec<_> = held
                .map(|reason| {
                    operations
                        .iter()
                        .filter(|op| apt(op))
                        .map(|op| (op.clone(), reason.clone()))
                        .collect()
                })
                .unwrap_or_default();
            Ok(run_batch(engine, &operations, held, cancel, send))
        }
        Job::Write(op, _) => engine
            .execute(&op, cancel, &mut |event| {
                if let Event::Progress { progress, .. } = &event {
                    send(Reply::Progress(match progress {
                        Progress::Message(message) => message.clone(),
                        Progress::Transfer { completed, total } => match total {
                            Some(total) => format!("Transferred {completed} of {total}"),
                            None => format!("Transferred {completed}"),
                        },
                        Progress::Package(name) => format!("Updating {name}"),
                    }));
                }
                if !matches!(&event, Event::Progress { progress: Progress::Message(_), .. }) {
                    send(Reply::ProgressEvent(event));
                }
            })
            .map(|outcome| {
                if matches!(&op, Operation::Upgrade(id) if id.backend == "fwupd") {
                    Payload::Batch(if outcome.cancellation_deferred {
                        "Firmware update finished before it could be cancelled. Restart or shut down the device if required.".into()
                    } else {
                        "Firmware update finished. Restart or shut down the device if required.".into()
                    }, vec![Outcome::Finished])
                } else { Payload::Written(outcome) }
            }),
    };
    send(Reply::Done(result));
}

/// Run a confirmed batch and summarize it. `held` changes are reported as
/// failed with their reason and never run.
fn run_batch(
    engine: &mut Engine,
    operations: &[Operation],
    held: Vec<(Operation, String)>,
    cancel: &Cancellation,
    send: &mut dyn FnMut(Reply),
) -> Payload {
    let runnable: Vec<_> = operations
        .iter()
        .filter(|op| !held.iter().any(|(held, _)| held == *op))
        .cloned()
        .collect();
    let ran = engine.execute_batch(&runnable, cancel, &mut |event| {
        if let Event::Progress {
            operation,
            progress: Progress::Message(message),
        } = &event
        {
            send(Reply::Progress(format!(
                "{}: {message}",
                operation_title(operation)
            )));
        }
        if !matches!(
            &event,
            Event::Progress {
                progress: Progress::Message(_),
                ..
            }
        ) {
            send(Reply::ProgressEvent(event));
        }
    });
    let mut ran = ran.into_iter();
    let results: Vec<Result<OperationOutcome, (EngineError, Option<String>)>> = operations
        .iter()
        .map(|op| match held.iter().find(|(held, _)| held == op) {
            Some((_, reason)) => Err((EngineError::Cancelled, Some(reason.clone()))),
            None => ran
                .next()
                .unwrap_or(Err(EngineError::Cancelled))
                .map_err(|error| (error, None)),
        })
        .collect();
    let completed = results.iter().filter(|r| r.is_ok()).count();
    let noun = if operations
        .iter()
        .all(|op| matches!(op, Operation::Clean(_)))
    {
        "cleanup tasks"
    } else {
        "updates"
    };
    let mut status = format!("Completed {completed} of {} {noun}.", operations.len());
    let mut outcomes = Vec::new();
    let mut failure_output = Vec::new();
    for (operation, result) in operations.iter().zip(results) {
        if let Err((error, None)) = &result {
            if let Some(output) = raw_failure_output(error) {
                failure_output.push(format!("{}\n{output}", operation_title(operation)));
            }
            if let Operation::Upgrade(id) = operation {
                if cask_needed_password(error, Some(&id.backend)) {
                    send(Reply::NeedsPassword(id.clone()));
                }
            }
        }
        outcomes.push(match &result {
            Ok(_) => Outcome::Finished,
            Err((EngineError::Cancelled, None)) => Outcome::Cancelled,
            Err(_) => Outcome::Failed,
        });
        let outcome = match result {
            Ok(outcome) if outcome.cancellation_deferred => {
                "Finished before it could be cancelled. Changes were kept.".into()
            }
            Ok(_) => "Completed".into(),
            Err((_, Some(reason))) => reason,
            // A denial keeps the engine's words; the frontend
            // explains it for the chosen permission option. The App
            // Store has its own explanation, independent of it.
            Err((error, None)) if is_denied(&error) && operation.backend() != "mas" => {
                error.to_string()
            }
            Err((error, None)) => plain_error(&error, Some(operation.backend()), false),
        };
        status.push_str(&format!("\n{}: {outcome}", operation_title(operation)));
    }
    if operations
        .iter()
        .any(|op| matches!(op, Operation::Upgrade(id) if id.backend == "fwupd"))
    {
        status.push_str("\nFirmware: Restart or shut down the device if the update asked for it.");
    }
    if !failure_output.is_empty() {
        send(Reply::FailureOutput(failure_output.join("\n\n")));
    }
    Payload::Batch(status, outcomes)
}

/// The upgrade-only helper mode the saved approval covers, for automatic
/// runs only. Changes the person starts, Update all included, keep the
/// reviewed mode and its password prompt.
fn batch_mode(job: &Job) -> Option<pkgdeck_core::batch::BatchMode> {
    match job {
        Job::AutoUpgrade(_, removals) => Some(pkgdeck_core::batch::BatchMode::UpgradeOnly {
            removals: *removals,
        }),
        _ => None,
    }
}
/// Why saving or removing the system update approval failed, in a sentence.
fn approval_error(error: &ExecutionError) -> String {
    match error {
        ExecutionError::AuthorizationCancelled => "The password prompt was cancelled.".into(),
        ExecutionError::AuthorizationDenied => {
            "Your password wasn't accepted, so nothing changed.".into()
        }
        ExecutionError::Disabled(reason) | ExecutionError::Invalid(reason) => reason.clone(),
        ExecutionError::Failed(completion) => {
            let stderr = String::from_utf8_lossy(&completion.stderr);
            stderr
                .lines()
                .rev()
                .map(|line| line.trim().trim_start_matches("pkgdeck-host-runner: "))
                .find(|line| !line.is_empty())
                .map_or_else(
                    || "PkgDeck's helper could not save the setting.".into(),
                    str::to_owned,
                )
        }
        other => other.to_string(),
    }
}
/// How workers reach the running system: its package managers, and the host
/// and root that source lists are read from. Tests substitute synthetic ones.
#[derive(Clone, Copy)]
struct Natives {
    engine: fn(&[String], bool, Authorization, &Cancellation) -> Result<Engine, EngineError>,
    host: fn() -> pkgdeck_core::host::Host,
    /// Saves or removes the system update approval, behind a password.
    approve: fn(&pkgdeck_core::host::Host, bool, &Cancellation) -> Result<String, ExecutionError>,
    root: &'static str,
    metadata: crate::metadata::Sources,
    /// Load the local app catalog in the background when sections load.
    warm_catalog: bool,
}
/// Builds an engine for some sources, as `native_engine` does.
pub type EngineMaker =
    fn(&[String], bool, Authorization, &Cancellation) -> Result<Engine, EngineError>;
/// A machine whose only package managers are `engine`'s.
fn synthetic_natives(engine: EngineMaker) -> Natives {
    fn bare_host() -> pkgdeck_core::host::Host {
        pkgdeck_core::host::Host::new(
            pkgdeck_core::host::Runtime::Native,
            BTreeMap::from([("PATH".into(), "/nonexistent/pkgdeck".into())]),
        )
    }
    fn no_approval(
        _: &pkgdeck_core::host::Host,
        _: bool,
        _: &Cancellation,
    ) -> Result<String, ExecutionError> {
        Err(ExecutionError::Disabled(
            "PkgDeck's system helper is not installed".into(),
        ))
    }
    fn no_catalog() -> crate::metadata::Catalog {
        crate::metadata::Catalog::default()
    }
    fn offline(_: &str, _: &Cancellation) -> String {
        String::new()
    }
    Natives {
        engine,
        host: bare_host,
        approve: no_approval,
        root: "/nonexistent/pkgdeck",
        metadata: crate::metadata::Sources {
            catalog: no_catalog,
            fetch: offline,
        },
        warm_catalog: false,
    }
}
const NATIVES: Natives = Natives {
    engine: pkgdeck_core::backends::native_engine,
    host: pkgdeck_core::host::Host::current,
    approve: pkgdeck_core::unattended::set_approval,
    root: "/",
    metadata: crate::metadata::SYSTEM,
    warm_catalog: true,
};
/// Unit tests never detect the machine's own package managers.
#[cfg(not(test))]
const DEFAULT_NATIVES: Natives = NATIVES;
#[cfg(test)]
const DEFAULT_NATIVES: Natives = tests::NO_MANAGERS;
struct Worker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<Reply>,
    cancel: Cancellation,
    job: Job,
}
/// Preloads one section on its own thread so opening it is instant and a
/// foreground search never waits behind it.
struct PrefetchWorker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<Reply>,
    cancel: Cancellation,
    view: String,
    key: String,
    /// Sources or elevation changed, or a write started: drop its result.
    stale: bool,
    /// The sources it asked, and those that answered so far.
    asked: Vec<String>,
    answered: Vec<String>,
    /// What its engine checked, so the next preload or search can reuse it.
    scope: (Vec<String>, bool, Instant),
}
/// Loads one package's details while a search or section still streams,
/// so a selection never waits for the slowest package manager.
struct DetailsWorker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<Reply>,
    cancel: Cancellation,
    id: PackageId,
}
/// Saving or removing the system update approval: the reply is the
/// approval key, or why it failed.
struct ApprovalWorker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<Result<String, String>>,
}
struct CatalogWorker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<Vec<Source>>,
    cancel: Cancellation,
}
/// The operation history and where it lives, so the GUI can read it with a
/// shared lock on its own thread while changes still use `History`.
#[derive(Clone)]
struct ActivityStore {
    history: History,
    path: PathBuf,
}
impl ActivityStore {
    fn at(path: PathBuf) -> Self {
        Self {
            history: History::new(path.clone()),
            path,
        }
    }
    fn default_store() -> Option<Self> {
        pkgdeck_core::activity::default_path().map(Self::at)
    }
    /// Read the history without writing it: a shared lock lets `pkd` and
    /// other readers go on, and nothing is synced to disk. Only when an
    /// entry's owner has died is the history rewritten to mark it
    /// interrupted, and then read again.
    fn load(&self) -> std::io::Result<Vec<pkgdeck_core::activity::Entry>> {
        let entries = read_activity(&self.path)?;
        if entries.iter().any(|entry| {
            !matches!(
                entry.state,
                State::Finished | State::Failed | State::Cancelled | State::Interrupted
            ) && !std::path::Path::new(&format!("/proc/{}", entry.owner_pid)).exists()
        }) {
            self.history.recover_dead()?;
            return read_activity(&self.path);
        }
        Ok(entries)
    }
}
impl std::ops::Deref for ActivityStore {
    type Target = History;
    fn deref(&self) -> &History {
        &self.history
    }
}
/// Read the activity file under a shared lock. A missing file or lock
/// means no history yet.
fn read_activity(path: &std::path::Path) -> std::io::Result<Vec<pkgdeck_core::activity::Entry>> {
    use rustix::fs::{flock, FlockOperation};
    use std::io::ErrorKind;
    let lock = match std::fs::File::open(path.with_extension("lock")) {
        Ok(lock) => Some(lock),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(lock) = &lock {
        flock(lock, FlockOperation::LockShared)?;
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error),
    }
}
/// Activity entries as stored, plus "labels": one line per operation in
/// people's words ("Install Firefox (Flatpak)"), in operation order.
fn activity_rows(entries: &[pkgdeck_core::activity::Entry], names: &Names) -> QString {
    let rows: Vec<Value> = entries
        .iter()
        .map(|entry| {
            let mut row = serde_json::to_value(entry).unwrap_or_else(|_| json!({}));
            row["labels"] = json!(entry
                .operations
                .iter()
                .map(|operation| operation_title_named(operation, names))
                .collect::<Vec<_>>());
            row
        })
        .collect();
    encoded(rows)
}
/// Reads the Activity history off the GUI thread.
struct ActivityWorker {
    handle: thread::JoinHandle<()>,
    receiver: mpsc::Receiver<std::io::Result<Vec<pkgdeck_core::activity::Entry>>>,
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(worker) = self.activity_worker.take() {
            let _ = worker.handle.join();
        }
        if let Some(worker) = self.worker.take() {
            worker.cancel.cancel();
            let _ = worker.handle.join();
        }
        if let Some(worker) = self.catalog_worker.take() {
            worker.cancel.cancel();
            let _ = worker.handle.join();
        }
    }
}
pub struct Controller {
    rows: QString,
    details: QString,
    status: QString,
    notice: QString,
    progress: QString,
    confirmation: QString,
    confirmation_data: QString,
    opened: QString,
    /// Where tests keep the AppImages launch settings read and change.
    appimage_data: Option<PathBuf>,
    /// The package `opened` shows.
    opened_package: Option<Package>,
    repositories: QString,
    source_catalog: QString,
    report_state: QString,
    pending_sources: QString,
    /// Sources the visible load asked that have not answered yet.
    asked: Vec<String>,
    manifest_preview: QString,
    activity: QString,
    background_state: QString,
    system_approval: QString,
    approval_error: QString,
    auto_update_result: QString,
    held_updates: QString,
    self_update: QString,
    /// The running copy, to tell after a change whether it was replaced.
    install: Option<pkgdeck_core::relaunch::Install>,
    notification_history: QString,
    version: QString,
    busy: bool,
    writing: bool,
    reading: bool,
    upgradable: bool,
    needs_poll: bool,
    refreshing: bool,
    updates_view: bool,
    packages: Vec<Package>,
    cleanup: Vec<CleanupItem>,
    failures: Vec<BackendFailure>,
    last_success: BTreeMap<String, u64>,
    retrying_sources: Option<Vec<String>>,
    detail_cache: BTreeMap<PackageId, QString>,
    sources: Vec<Source>,
    pending: Option<Job>,
    queued: Option<Job>,
    confirmed_queue: VecDeque<Confirmed>,
    active_activity_id: Option<u64>,
    progress_state: Option<ProgressState>,
    revalidating: Option<Confirmed>,
    discard_revalidation: bool,
    validated_confirmed: Option<Confirmed>,
    deferred_load: Option<Job>,
    activity_store: Option<ActivityStore>,
    activity_worker: Option<ActivityWorker>,
    /// Activity changed while it was being read; read it once more after.
    activity_again: bool,
    autostart_path: Option<PathBuf>,
    background_schedule: Schedule,
    selected: Option<PackageId>,
    engine: Option<Engine>,
    view_cache: ViewCache,
    view_store: Option<ViewStore>,
    prefetched: BTreeMap<String, (Instant, Payload)>,
    background: bool,
    prefetch: Vec<String>,
    prefetch_worker: Option<PrefetchWorker>,
    details_worker: Option<DetailsWorker>,
    /// The visible section is waiting for the prefetch already loading it.
    awaiting_prefetch: bool,
    /// Rows already on screen stay until the refresh finishes instead of
    /// being replaced by each streamed partial.
    hold_partials: bool,
    /// What the failed steps of the running batch printed, until its notice
    /// takes it.
    failure_output: String,
    /// Casks of the running batch that stopped for the password.
    password_casks: Vec<PackageId>,
    /// Casks automatic runs skip until a newer version.
    needs_password: NeedsPassword,
    /// Updates that failed, which automatic runs and Update all skip until
    /// a newer version.
    failed_updates: FailedUpdates,
    /// The version each cask of the queued automatic run updates to.
    cask_versions: BTreeMap<PackageId, String>,
    /// The change the banner's Retry runs again.
    retry_job: Option<Job>,
    last_rewarm: Instant,
    /// Scope, elevation, and detection time of `engine`, so searches can
    /// reuse detected managers instead of rediscovering on every query.
    engine_scope: Option<(Vec<String>, bool, Instant)>,
    reused_engine_born: Option<Instant>,
    active_view: String,
    /// How many packages the running or last search matched; its rows
    /// carry only the best of them.
    search_matches: usize,
    prepared_rows: RowsCache,
    /// Set while a details preview is applied: the quick first look sent
    /// before slower lookups finish.
    details_preview: bool,
    /// More changes the review on screen covers, run after `pending`, with
    /// the operations of the `pending` they belong to.
    pending_more: (Vec<Operation>, Vec<Job>),
    /// The package whose details load because the pointer rests on its row.
    warming: Option<PackageId>,
    catalog_checked: bool,
    catalog_worker: Option<CatalogWorker>,
    approval_worker: Option<ApprovalWorker>,
    /// Install updates that background checks find.
    auto_update: bool,
    /// Let Update all remove packages when APT's plan does.
    allow_removals: bool,
    source_filter: Vec<String>,
    sudo: bool,
    worker: Option<Worker>,
    /// Display names of confirmed changes' packages (see `Names`).
    names: Names,
    natives: Natives,
}
impl Controller {
    /// Keep the slow sections between runs at `path`, and show what the last
    /// run kept until they load again.
    pub(crate) fn open_view_store(&mut self, path: Option<PathBuf>) {
        let Some(path) = path else { return };
        let store = ViewStore::open(path);
        for (key, view) in store.cached() {
            self.view_cache.insert(key, view);
        }
        self.view_store = Some(store);
    }
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            rows: "[]".into(),
            details: "{}".into(),
            status: "Choose a view or search for a package.".into(),
            notice: "{}".into(),
            progress: "{}".into(),
            confirmation: QString::default(),
            confirmation_data: "{}".into(),
            opened: QString::default(),
            appimage_data: None,
            opened_package: None,
            repositories: "{}".into(),
            source_catalog: "[]".into(),
            report_state: r#"{"phase":"idle"}"#.into(),
            pending_sources: "[]".into(),
            asked: vec![],
            manifest_preview: "{}".into(),
            activity: "[]".into(),
            background_state: "{}".into(),
            system_approval: QString::default(),
            approval_error: QString::default(),
            auto_update_result: "{}".into(),
            held_updates: "[]".into(),
            self_update: QString::default(),
            notification_history: "{}".into(),
            version: pkgdeck_core::VERSION.into(),
            busy: false,
            writing: false,
            reading: false,
            upgradable: false,
            // Sections preload from startup, which poll() starts.
            needs_poll: true,
            refreshing: false,
            updates_view: false,
            packages: vec![],
            cleanup: vec![],
            failures: vec![],
            last_success: BTreeMap::new(),
            retrying_sources: None,
            detail_cache: BTreeMap::new(),
            sources: vec![],
            pending: None,
            queued: None,
            confirmed_queue: VecDeque::new(),
            active_activity_id: None,
            progress_state: None,
            revalidating: None,
            discard_revalidation: false,
            validated_confirmed: None,
            deferred_load: None,
            activity_store: if cfg!(test) {
                None
            } else {
                ActivityStore::default_store()
            },
            activity_worker: None,
            activity_again: false,
            autostart_path: if cfg!(test) {
                None
            } else {
                background::autostart_path()
            },
            install: if cfg!(test) {
                None
            } else {
                pkgdeck_core::relaunch::Install::current()
            },
            background_schedule: Schedule::default(),
            selected: None,
            engine: None,
            view_cache: ViewCache { entries: vec![] },
            view_store: None,
            prefetched: BTreeMap::new(),
            background: false,
            prefetch: prefetch_views(),
            prefetch_worker: None,
            details_worker: None,
            awaiting_prefetch: false,
            hold_partials: false,
            failure_output: String::new(),
            password_casks: Vec::new(),
            needs_password: if cfg!(test) {
                NeedsPassword::default()
            } else {
                NeedsPassword::default_store()
            },
            failed_updates: if cfg!(test) {
                FailedUpdates::default()
            } else {
                FailedUpdates::default_store()
            },
            cask_versions: BTreeMap::new(),
            retry_job: None,
            last_rewarm: Instant::now(),
            engine_scope: None,
            reused_engine_born: None,
            active_view: "Search".into(),
            search_matches: 0,
            prepared_rows: RowsCache::default(),
            details_preview: false,
            pending_more: (Vec::new(), Vec::new()),
            warming: None,
            catalog_checked: false,
            catalog_worker: None,
            approval_worker: None,
            auto_update: false,
            allow_removals: true,
            source_filter: Vec::new(),
            sudo: false,
            worker: None,
            names: Names::new(),
            natives: DEFAULT_NATIVES,
        }
    }
}
impl Controller {
    /// Sections preload on their own worker from startup, Search included.
    fn next_prefetch(&mut self) -> Option<String> {
        self.prefetch.pop()
    }
    /// Forget cached details, and drop a details lookup still running so
    /// its older reply can't refill the cache after a reload or a write.
    fn invalidate_details(&mut self) {
        self.detail_cache.clear();
        if let Some(worker) = self.details_worker.take() {
            worker.cancel.cancel();
        }
    }
    /// A preload already loading this section, so a visible load can wait
    /// for it instead of querying every manager a second time.
    /// Installed does not stand in for Updates: Updates refreshes update
    /// indexes before listing.
    fn awaits_prefetch(&self, key: &str) -> bool {
        self.prefetch_worker
            .as_ref()
            .is_some_and(|worker| !worker.stale && worker.key == key)
    }
    /// Whether poll() may still deliver something or start queued work:
    /// any worker thread, a section waiting for its preload, or preloads
    /// that are due and not held back by a change awaiting confirmation.
    /// See the `reading` property.
    fn only_reading(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| matches!(worker.job, Job::Load(..) | Job::Details(_)))
            && self.confirmed_queue.is_empty()
            && self.validated_confirmed.is_none()
            && !matches!(
                self.queued,
                Some(
                    Job::PlanOperation(..)
                        | Job::PlanAdoption(..)
                        | Job::PlanAdoptAll(..)
                        | Job::PlanUpgrade(..)
                )
            )
    }
    fn outstanding(&self) -> bool {
        let held = self.pending.is_some() || !self.confirmed_queue.is_empty();
        self.worker.is_some()
            || self.prefetch_worker.is_some()
            || self.details_worker.is_some()
            || self.catalog_worker.is_some()
            || self.approval_worker.is_some()
            || self.activity_worker.is_some()
            || self.awaiting_prefetch
            || !self.prefetch.is_empty() && !held
    }
    /// Whether the saved approval still covers the helper this app would
    /// start, so an unattended update can't stop at a password prompt.
    fn approval_current(&self) -> bool {
        let saved = self.system_approval.to_string();
        if saved.is_empty() {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            saved == pkgdeck_core::unattended::macos::KEY
                && pkgdeck_core::unattended::macos::approved()
        }
        #[cfg(not(target_os = "macos"))]
        {
            pkgdeck_core::unattended::current_runner(&(self.natives.host)()).as_ref()
                == Some(&saved)
        }
    }
    /// A read the user is waiting for owns the worker. A quiet refresh
    /// behind cached rows, or background work, gives way to a new request.
    fn foreground_worker(&self) -> bool {
        self.worker.is_some() && !self.background && !self.refreshing
    }
    /// Display names for the packages these operations change: remembered
    /// from confirmation, else from the rows on screen.
    fn names_for(&self, operations: &[Operation]) -> Names {
        operations
            .iter()
            .filter_map(package_target)
            .filter_map(|id| {
                self.names
                    .get(id)
                    .cloned()
                    .or_else(|| {
                        self.packages
                            .iter()
                            .find(|package| package.id == *id)
                            .map(|package| package.display_name.clone())
                    })
                    .filter(|name| !name.trim().is_empty())
                    .map(|name| (id.clone(), name))
            })
            .collect()
    }
    /// Forget preloaded sections and load them again from scratch.
    fn invalidate_prefetch(&mut self) {
        self.prefetched.clear();
        self.prefetch = prefetch_views();
        self.awaiting_prefetch = false;
        if let Some(worker) = &mut self.prefetch_worker {
            worker.cancel.cancel();
            worker.stale = true;
        }
    }
}
/// Update all: one command for each source that can update everything at
/// once, and one update per package for sources that can't (rustup, Nix,
/// firmware, ...). Progress still names each package: sources report which
/// one they are on (see `Progress::Package`).
fn upgrade_plan(packages: &[Package]) -> Vec<Operation> {
    let (each, whole): (Vec<_>, Vec<_>) = packages
        .iter()
        .filter(|p| p.installed_version.is_some() && p.update == UpdateAvailability::Available)
        .partition(|p| pkgdeck_core::backends::per_package_upgrades(&p.id.backend));
    let backends: std::collections::BTreeSet<_> =
        whole.iter().map(|p| p.id.backend.clone()).collect();
    // A package listed twice is still updated once, in list order.
    let mut seen = std::collections::BTreeSet::new();
    backends
        .into_iter()
        .map(|backend| Operation::UpgradeAll { backend })
        .chain(
            each.into_iter()
                .filter(|p| seen.insert(&p.id))
                .map(|p| Operation::Upgrade(p.id.clone())),
        )
        .collect()
}
/// Whether this is an update that already failed at the version it would
/// go to now.
fn failed_before(operation: &Operation, packages: &[Package], failed: &FailedUpdates) -> bool {
    let Operation::Upgrade(id) = operation else {
        return false;
    };
    packages
        .iter()
        .find(|package| package.id == *id)
        .is_some_and(|package| failed.skips(id, cask_version(package)))
}
/// Update all leaves out updates that failed before, so one broken update
/// doesn't fail every run. When nothing else is left it tries them again:
/// that's what the person asked for.
fn without_failed(
    operations: Vec<Operation>,
    packages: &[Package],
    failed: &FailedUpdates,
) -> Vec<Operation> {
    let kept: Vec<Operation> = operations
        .iter()
        .filter(|operation| !failed_before(operation, packages, failed))
        .cloned()
        .collect();
    if kept.is_empty() {
        operations
    } else {
        kept
    }
}
/// [`upgrade_plan`] for an automatic run, which only downloads macOS
/// updates, with the one command the saved approval names. Casks update
/// one at a time, so one that needs the password stops only itself, and
/// those that needed it before wait for a newer version, like any update
/// that failed.
fn automatic_plan(
    packages: &[Package],
    needs_password: &NeedsPassword,
    failed: &FailedUpdates,
) -> Vec<Operation> {
    let mut seen = std::collections::BTreeSet::new();
    let casks: Vec<Operation> = packages
        .iter()
        .filter(|p| p.id.backend == "homebrew-cask")
        .filter(|p| p.installed_version.is_some() && p.update == UpdateAvailability::Available)
        .filter(|p| !needs_password.skips(&p.id.name, cask_version(p)))
        .filter(|p| seen.insert(&p.id))
        .map(|p| Operation::Upgrade(p.id.clone()))
        .collect();
    let mut operations: Vec<Operation> = upgrade_plan(packages)
        .into_iter()
        .flat_map(|operation| match operation {
            Operation::UpgradeAll { backend } if backend == "homebrew-cask" => casks.clone(),
            operation => vec![operation],
        })
        .filter(|operation| !failed_before(operation, packages, failed))
        .collect();
    if operations
        .iter()
        .any(|operation| operation.backend() == "macos-updates")
    {
        operations.retain(|operation| operation.backend() != "macos-updates");
        operations.push(Operation::UpgradeAll {
            backend: "macos-updates".into(),
        });
    }
    operations
}
/// The version a cask update goes to, which an automatic run remembers
/// when the update needs the password.
fn cask_version(package: &Package) -> &str {
    package.candidate_version.as_deref().unwrap_or_default()
}
/// Passes on the lines of output that name a package one of `backends` is
/// updating; everything else a manager prints stays in the worker.
fn output_follower(
    backends: Vec<String>,
    sender: crate::wake::Sender<Reply>,
) -> pkgdeck_core::process::OutputObserver {
    std::sync::Arc::new(move |line: &str| {
        if backends
            .iter()
            .any(|backend| pkgdeck_core::backends::output_package(backend, line).is_some())
        {
            let _ = sender.send(Reply::Output(line.to_owned()));
        }
    })
}
fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
/// Wall-clock seconds at a monotonic instant in the past.
fn epoch_seconds_at(instant: Instant) -> u64 {
    epoch_seconds().saturating_sub(instant.elapsed().as_secs())
}
/// The sources a streamed report heard from, as a success or a failure.
fn answered(report: &PackageReport) -> Vec<String> {
    let failed = report.failures.iter().map(|f| f.backend.clone());
    report
        .successful_sources
        .iter()
        .cloned()
        .chain(failed)
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
        Job::OpenInput(_) | Job::ImportRepository(_) => vec![],
        Job::RetrySource(_, _, source) => vec![source.clone()],
        Job::RetryFailedUpdates(sources) => sources.clone(),
        Job::PlanOperation(operation) => vec![operation.backend().into()],
        // AppImages are taken over by the AppImage source itself.
        Job::PlanAdoption(app) if app.id.backend == "appimage" => vec!["appimage".into()],
        Job::PlanAdoptAll(_) => vec!["appimage".into()],
        Job::PlanAdoption(_) => vec!["homebrew-cask".into()],
        Job::Write(operation, _) => vec![operation.backend().into()],
        Job::BackgroundUpdates(sources) => sources.clone(),
        Job::UpgradeAll(operations, _) | Job::AutoUpgrade(operations, _) | Job::CleanAll(operations) => operations.iter().map(|op| op.backend().to_owned()).collect(),
        Job::PlanCleanAll(operations) => operations.iter().map(|op| op.backend().to_owned()).collect(),
        // The picker needs to explain disabled and unavailable managers too.
        Job::Load(view, _) if view == "Sources" => vec![],
        Job::Details(id) => vec![id.backend.clone()],
        Job::ManifestPreview(_) => vec![],
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
/// The name people know a package source by, matching the GUI's source list.
// TODO(merge): use pkgdeck_core display_name
fn source_display_name(id: &str) -> String {
    match id {
        "apt" => "APT",
        "dnf" => "DNF",
        "pacman" => "Pacman",
        "zypper" => "Zypper",
        "snap" => "Snap",
        "homebrew" => "Homebrew",
        "homebrew-cask" => "Homebrew Casks",
        "macos-apps" => "macOS Applications",
        "mas" => "Mac App Store",
        "aur" => "AUR",
        "apk" => "apk",
        "xbps" => "XBPS",
        "system-image" => "System image",
        "macports" => "MacPorts",
        "macos-updates" => "macOS Updates",
        "rustup" => "rustup",
        "nix" => "Nix",
        "go" => "Go",
        "dotnet" => ".NET tools",
        "appimage" => "AppImage",
        "flatpak" => "Flatpak",
        "docker" => "Docker images",
        "podman" => "Podman images",
        "toolbox" => "Toolbx containers",
        "distrobox" => "Distrobox containers",
        "cargo" => "Cargo",
        "npm" => "npm",
        "pnpm" => "pnpm",
        "bun" => "Bun",
        "pip" => "pip",
        "pipx" => "pipx",
        "uv" => "uv",
        "pixi" => "pixi",
        "conda" => "Conda",
        "composer" => "Composer",
        "gem" => "RubyGems",
        "oh-my-zsh" => "Oh My Zsh",
        "fwupd" => "Firmware",
        "codex" => "Codex (standalone)",
        "claude" => "Claude Code (standalone)",
        "grok" => "Grok (standalone)",
        "opencode" => "OpenCode (standalone)",
        "cursor" => "Cursor CLI (standalone)",
        "copilot" => "GitHub Copilot CLI (standalone)",
        "kiro" => "Kiro CLI (standalone)",
        "antigravity" => "Antigravity CLI (standalone)",
        "amp" => "Amp (standalone)",
        "droid" => "Factory Droid (standalone)",
        "solana" => "Solana CLI (Agave)",
        "anchor" => "Anchor (AVM)",
        "foundry" => "Foundry",
        other => other,
    }
    .to_owned()
}
/// "System" or "User" for people; an environment shows its location.
fn scope_word(scope: &Scope) -> String {
    match scope {
        Scope::System => "System".into(),
        Scope::User { .. } => "User".into(),
        Scope::Environment { path } => path.display().to_string(),
    }
}
/// "APT, System": where a change happens, in people's words.
fn source_and_scope(backend: &str, scope: &Scope) -> String {
    format!("{}, {}", source_display_name(backend), scope_word(scope))
}
/// Capitalize and end with a period, so every message reads as a sentence.
fn sentence(text: &str) -> String {
    let text = text.trim().trim_end_matches([':', ';', ',']).trim_end();
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}
/// Drop Rust debug formatting that leaks from lower layers, such as
/// "(Some(1))" or "Some(\"x\")", keeping the meaningful value.
fn strip_debug(text: &str) -> String {
    let mut out = text.replace("(None)", "").replace(" None:", ":");
    while let Some(start) = out.find("Some(") {
        let inner = start + "Some(".len();
        let Some(end) = out[inner..].find(')').map(|at| inner + at) else {
            out.replace_range(start..inner, "");
            break;
        };
        let value = out[inner..end].trim_matches('"').to_owned();
        out.replace_range(start..=end, &value);
    }
    out
}
/// The most output shown under a failure. The end matters most: it is where
/// tools print what went wrong.
const FAILURE_OUTPUT_LIMIT: usize = 6000;
/// Everything a failed command printed, for the banner's details, so the
/// reason is readable without running it again in a terminal.
fn raw_failure_output(error: &EngineError) -> Option<String> {
    let EngineError::Execution(pkgdeck_core::process::ExecutionError::Failed(result)) = error
    else {
        return None;
    };
    let stderr = String::from_utf8_lossy(&result.stderr);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let text = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return None;
    }
    let mut start = text.len().saturating_sub(FAILURE_OUTPUT_LIMIT);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(if start > 0 {
        format!("…{}", &text[start..])
    } else {
        text
    })
}
/// Whether a failed OpenSSL build says its headers or pkg-config are
/// missing, the only case installing them fixes.
fn missing_openssl_files(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    [
        "pkg-config",
        "pkg_config",
        "opensslconf.h",
        "could not find openssl",
        "could not find directory of openssl",
    ]
    .iter()
    .any(|marker| output.contains(marker))
}
/// The first line of tool output worth showing: an error line if there is
/// one, else the first non-empty line. Prefixes such as "error:" and "E:"
/// are removed because the sentence around it already says it failed.
fn meaningful_line(output: &str) -> Option<String> {
    const PREFIXES: &[&str] = &["error:", "Error:", "ERROR:", "E:", "fatal:", "FATAL:"];
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("WARNING: apt does not have a stable CLI"))
        .collect();
    let line = lines
        .iter()
        .find(|line| PREFIXES.iter().any(|prefix| line.starts_with(prefix)))
        .or_else(|| lines.first())?;
    let mut line = *line;
    for prefix in PREFIXES {
        if let Some(rest) = line.strip_prefix(prefix) {
            line = rest.trim_start();
        }
    }
    // Well-known messages people hit often, said plainly.
    if line.contains("rustup could not choose a version")
        || line.contains("no default toolchain")
        || line.contains("no default is configured")
    {
        return Some("rustup has no default toolchain".into());
    }
    // Cargo builds a dependency's C bindings while installing. Say which
    // one failed, and the usual reason for the one people hit most.
    if let Some(krate) = line
        .strip_prefix("failed to run custom build command for `")
        .and_then(|rest| rest.split('`').next())
    {
        return Some(
            if krate.starts_with("openssl-sys ") && missing_openssl_files(output) {
                format!("building `{krate}` failed. It needs OpenSSL's development files (libssl-dev or openssl-devel) and pkg-config")
            } else {
                format!(
                    "building `{krate}` failed. Run the update in a terminal to read the build log"
                )
            },
        );
    }
    // Flatpak names the remote and the URL; people need the remote and why.
    if let Some(rest) = line.strip_prefix("Unable to load summary from remote ") {
        let remote = rest.split(':').next().unwrap_or_default();
        let reason = rest.rsplit(": ").next().unwrap_or_default();
        let reason = reason
            .split_once("] ")
            .filter(|(code, _)| code.starts_with('['))
            .map_or(reason, |(_, reason)| reason);
        return Some(format!("it can't reach the remote {remote} ({reason})"));
    }
    Some(line.trim_end_matches(['.', ':', ';']).to_owned())
}
const ROOT_MESSAGE: &str =
    "PkgDeck can't make changes while running as root. Start it as your normal user.";
/// Whether engine text is the refusal to make changes as root, in the
/// core's current wording or an older one.
fn refuses_root(text: &str) -> bool {
    text.contains("unprivileged user")
        || text.contains("running as root")
        || text.contains("runs as root")
}
/// One plain sentence for text that came from the engine, whatever its
/// wording: internal prefixes and debug formatting are removed, and the
/// tool's first meaningful output line is kept.
fn plain_text(raw: &str, backend: Option<&str>) -> String {
    if refuses_root(raw) {
        return ROOT_MESSAGE.into();
    }
    let tool = backend.map(source_display_name);
    let failed = |output: &str| match (meaningful_line(output), &tool) {
        (Some(line), Some(tool)) => format!("{tool} couldn't run: {}", sentence_tail(&line)),
        (Some(line), None) => sentence(&line),
        (None, Some(tool)) => format!("{tool} reported an error without details."),
        (None, None) => "The package manager reported an error without details.".into(),
    };
    if let Some(at) = raw.find("host command failed") {
        let rest = &raw[at..];
        return failed(rest.find("):").map_or("", |end| &rest[end + 2..]));
    }
    // Core's own wording: "the package manager exited with code N: reason".
    if let Some(rest) = raw.trim_start().strip_prefix("the package manager ") {
        return failed(rest.split_once(": ").map_or("", |(_, reason)| reason));
    }
    let cleaned = strip_debug(raw);
    let line = meaningful_line(&cleaned).unwrap_or_default();
    if line.is_empty() {
        return "Something went wrong.".into();
    }
    sentence(&line)
}
/// The part after "X couldn't run:" keeps its own case (tool names such as
/// rustup are lowercase) and ends with a period.
fn sentence_tail(line: &str) -> String {
    let mut line = line.trim().to_owned();
    if !line.ends_with(['.', '!', '?']) {
        line.push('.');
    }
    line
}
/// Lowercase a sentence's first letter to continue another sentence,
/// unless it starts an acronym or a name such as "APT".
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => {
            first.to_lowercase().chain(text.chars().skip(1)).collect()
        }
        _ => text.to_owned(),
    }
}
/// The macOS inventory lists every app it can read and reports each folder
/// or bundle it had to skip. That is a partial result, not a failed source.
fn skipped_app_entry(error: &EngineError) -> Option<(&str, &str)> {
    let EngineError::InvalidResponse { backend, reason } = error else {
        return None;
    };
    if backend != "macos-apps" {
        return None;
    }
    reason
        .strip_prefix("skipped folder ")
        .or_else(|| reason.strip_prefix("skipped entry in "))
        .or_else(|| reason.strip_prefix("skipped entry "))?
        .rsplit_once(": ")
}
/// A cask script ran sudo and got no password: the dialog was cancelled,
/// the run was automatic, or this build has no dialog to show.
fn cask_needed_password(error: &EngineError, backend: Option<&str>) -> bool {
    matches!(
        error,
        EngineError::Execution(pkgdeck_core::process::ExecutionError::Failed(result))
            if matches!(backend, Some("homebrew" | "homebrew-cask"))
                && String::from_utf8_lossy(&result.stderr).contains("/usr/bin/sudo")
    )
}
/// A plain explanation of an engine error, naming the tool when known.
fn plain_error(error: &EngineError, backend: Option<&str>, sudo: bool) -> String {
    use pkgdeck_core::process::ExecutionError as E;
    if let Some((path, why)) = skipped_app_entry(error) {
        let why = why.split(" (os error ").next().unwrap_or(why);
        return format!(
            "Couldn't read {path} ({}). The other apps are still listed.",
            lower_first(why)
        );
    }
    match error {
        // mas asks for the Mac password through sudo, which needs a terminal.
        EngineError::Execution(E::AuthorizationDenied) if backend == Some("mas") => "App Store updates need your Mac password, which mas can only ask for in a terminal. PkgDeck opened the App Store's Updates page so you can update there, or run `pkd upgrade --from mas` in Terminal.".into(),
        EngineError::Execution(E::AuthorizationDenied) if sudo => "PkgDeck couldn't get administrator access. The sudo option needs a recent sudo login in the same terminal.".into(),
        EngineError::Execution(E::AuthorizationDenied) => "PkgDeck couldn't get administrator access. Make sure your desktop's password prompt is running, then try again.".into(),
        EngineError::Execution(E::AuthorizationCancelled) => "The password prompt was closed. Nothing was changed.".into(),
        EngineError::Execution(E::LockBusy) => "Another package manager is running. Wait for it to finish, then try again.".into(),
        EngineError::Execution(E::Interrupted) => "The package manager was interrupted. Check its state before trying again.".into(),
        EngineError::Execution(E::TimedOut) => "The package manager took too long to answer. Try again.".into(),
        EngineError::Cancelled | EngineError::Execution(E::Cancelled) => "Cancelled.".into(),
        error if cask_needed_password(error, backend) => {
            "Homebrew needed your administrator password to finish this change and didn't get one. Update it again from Updates and enter your password when asked.".into()
        }
        EngineError::Execution(E::Failed(result)) => {
            let output = String::from_utf8_lossy(&result.stderr);
            let output = if output.trim().is_empty() {
                String::from_utf8_lossy(&result.stdout).into_owned()
            } else {
                output.into_owned()
            };
            match (meaningful_line(&output), backend.map(source_display_name)) {
                (Some(line), Some(tool)) => format!("{tool} couldn't run: {}", sentence_tail(&line)),
                (Some(line), None) => sentence(&line),
                (None, Some(tool)) => format!("{tool} reported an error without details."),
                (None, None) => "The package manager reported an error.".into(),
            }
        }
        EngineError::Unavailable { backend, reason } => {
            if refuses_root(reason) {
                return ROOT_MESSAGE.into();
            }
            format!(
                "{} isn't available: {}",
                source_display_name(backend),
                lower_first(&plain_text(reason, None))
            )
        }
        EngineError::InvalidResponse { backend: source, reason }
            if matches!(source.as_str(), "open" | "check") || reason.contains("host command failed") =>
        {
            plain_text(reason, Some(source.as_str()).filter(|s| !matches!(*s, "open" | "check")))
        }
        EngineError::InvalidResponse { backend: source, reason } => {
            if refuses_root(reason) {
                return ROOT_MESSAGE.into();
            }
            match meaningful_line(&strip_debug(reason)) {
                Some(line) => format!(
                    "{} reported a problem: {}",
                    source_display_name(source),
                    sentence_tail(&line)
                ),
                None => format!("{} reported a problem.", source_display_name(source)),
            }
        }
        EngineError::Unsupported { backend, .. } => format!(
            "{} can't do this.",
            source_display_name(backend)
        ),
        EngineError::NotFound => "No package matches the selection.".into(),
        other => plain_text(&other.to_string(), backend),
    }
}
/// A repository change in people's words, such as
/// "Remove repository\nflathub, Flatpak, System".
fn repository_label(action: &RepositoryAction) -> String {
    let change = match &action.change {
        repositories::Change::Add { url } => format!("Add repository from {url}"),
        repositories::Change::Remove => "Remove repository".into(),
        repositories::Change::SetEnabled { enabled: true } => "Enable repository".into(),
        repositories::Change::SetEnabled { enabled: false } => "Disable repository".into(),
        repositories::Change::SetPriority { priority } => {
            format!("Set repository priority to {priority}")
        }
        repositories::Change::OpenEditor => "Open Software Sources".into(),
    };
    format!(
        "{change}\n{}, {}",
        action.name,
        source_and_scope(&action.backend, &action.scope)
    )
}
/// A repository listing error such as "flatpak: host command failed…",
/// with the source named as people know it.
fn repository_error(error: &str) -> String {
    match error.split_once(": ") {
        Some((source, rest)) if pkgdeck_core::backends::BACKEND_IDS.contains(&source) => {
            let text = plain_text(rest, Some(source));
            let name = source_display_name(source);
            if text.starts_with(&name) {
                text
            } else {
                format!("{name}: {text}")
            }
        }
        _ => plain_text(error, None),
    }
}
/// The repositories report, with each repository's source and scope in
/// people's words ("where": "Flatpak, System") and errors as sentences.
/// Every original field stays for the frontend's own logic.
fn repositories_json(report: &repositories::Report) -> QString {
    let mut value = serde_json::to_value(report).unwrap_or_else(|_| json!({}));
    let rows = value["repositories"].as_array_mut().into_iter().flatten();
    for (row, repository) in rows.zip(&report.repositories) {
        row["source_name"] = json!(source_display_name(&repository.backend));
        row["where"] = json!(source_and_scope(&repository.backend, &repository.scope));
    }
    value["errors"] = json!(report
        .errors
        .iter()
        .map(|error| repository_error(error))
        .collect::<Vec<_>>());
    encoded(value)
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
        Err(error) => plain_error(error, Some(&source.backend), false),
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
        _ if skipped_app_entry(error).is_some() => "partial",
        _ => "failed",
    }
}
fn read_failed(failure: &BackendFailure) -> bool {
    !matches!(failure.error, EngineError::Unsupported { .. })
}
fn merge_retried_packages(
    current: &[Package],
    failures: &[BackendFailure],
    successful_sources: Vec<String>,
    retried: &[String],
    retry: PackageReport,
) -> PackageReport {
    let mut packages: Vec<_> = current
        .iter()
        .filter(|package| !retried.contains(&package.id.backend))
        .cloned()
        .collect();
    packages.extend(retry.packages);
    packages.sort_by(|a, b| a.id.cmp(&b.id));
    let mut failures: Vec<_> = failures
        .iter()
        .filter(|failure| !retried.contains(&failure.backend))
        .cloned()
        .collect();
    failures.extend(retry.failures);
    let mut successful_sources: Vec<_> = successful_sources
        .into_iter()
        .filter(|source| !retried.contains(source))
        .collect();
    successful_sources.extend(retry.successful_sources);
    successful_sources.sort();
    successful_sources.dedup();
    PackageReport {
        packages,
        failures,
        successful_sources,
    }
}
/// Details for a failed source row. Served from the stored report without a
/// backend roundtrip: the query already failed, re-querying cannot help.
/// A plain-language reason a change failed, with what to do next.
fn write_error_text(error: &EngineError, backend: Option<&str>, sudo: bool) -> String {
    plain_error(error, backend, sudo)
}
/// Display names of the packages a change targets, remembered when the
/// change is confirmed so its progress and result can name them.
type Names = BTreeMap<PackageId, String>;
fn is_denied(error: &EngineError) -> bool {
    matches!(
        error,
        EngineError::Execution(pkgdeck_core::process::ExecutionError::AuthorizationDenied)
    )
}
/// The one source a job changes, when it changes only one.
fn job_backend(operations: &[Operation]) -> Option<&str> {
    let first = operations.first()?.backend();
    operations
        .iter()
        .all(|operation| operation.backend() == first)
        .then_some(first)
}
/// What the banner says when a change can't be prepared, for example when
/// APT's dry run fails before the confirmation opens.
fn preflight_notice(job: &Job, error: &EngineError, sudo: bool, names: &Names) -> Option<Value> {
    let (title, operations) = match job {
        Job::PlanOperation(operation) => (
            operation_title_named(operation, names),
            vec![operation.clone()],
        ),
        Job::PlanUpgrade(operations, _) => ("The update".to_owned(), operations.clone()),
        Job::PlanAdoption(app) => (
            format!(
                "Managing {} with {}",
                app.display_name,
                if app.id.backend == "appimage" {
                    "PkgDeck"
                } else {
                    "Homebrew"
                }
            ),
            vec![],
        ),
        Job::PlanCleanAll(operations) => ("The cleanup".to_owned(), operations.clone()),
        Job::PlanAdoptAll(_) => ("Managing your AppImages with PkgDeck".to_owned(), vec![]),
        _ => return None,
    };
    if matches!(
        error,
        EngineError::Cancelled
            | EngineError::Execution(pkgdeck_core::process::ExecutionError::Cancelled)
    ) {
        return None;
    }
    Some(json!({
        "kind": "error",
        "title": format!("{title} couldn't be prepared"),
        "detail": write_error_text(error, job_backend(&operations), sudo),
    }))
}
/// A batch reports each error as text on its own line. A denial keeps the
/// engine's words so the right advice for the chosen permission option can
/// be given here; every other line is already plain.
const DENIED: &str = ": authorization denied or unavailable";
fn batch_status_text(status: &str, sudo: bool) -> String {
    status
        .lines()
        .map(|line| {
            line.strip_suffix(DENIED).map_or_else(
                || line.to_owned(),
                |label| {
                    format!(
                        "{label}: {}",
                        plain_error(
                            &EngineError::Execution(
                                pkgdeck_core::process::ExecutionError::AuthorizationDenied,
                            ),
                            None,
                            sudo,
                        )
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
/// The inverse change a toast may offer as Undo. Only an install and a
/// removal undo each other; updates and cleanups cannot be reversed, and
/// a package removed from a local file or a direct download can't be
/// installed again without that file. Nor can one removed from a source
/// PkgDeck never installs from, such as the macOS Applications inventory.
fn undo_action(operation: &Operation) -> Option<&'static str> {
    match operation {
        Operation::Install(id) if !pkgdeck_core::backends::never_installs(&id.backend) => {
            Some("remove")
        }
        Operation::Remove(id)
            if !pkgdeck_core::backends::never_installs(&id.backend)
                && !pkgdeck_core::backends::inventory_only(&id.backend)
                && id.backend != "appimage"
                && !id.reference.as_deref().is_some_and(|reference| {
                    ["local-deb:", "artifact:", "flatpakref:"]
                        .iter()
                        .any(|prefix| reference.starts_with(prefix))
                }) =>
        {
            Some("install")
        }
        _ => None,
    }
}
/// Whether a row of `backend` may offer removal. Sources PkgDeck never
/// installs from, and inventories, remove only when the source catalog
/// (the page's list of sources and what each can do) says they can; the
/// engine still refuses anything a backend doesn't support.
fn removable(catalog: &str, backend: &str) -> bool {
    if !pkgdeck_core::backends::never_installs(backend)
        && !pkgdeck_core::backends::inventory_only(backend)
    {
        return true;
    }
    serde_json::from_str::<Vec<Value>>(catalog).is_ok_and(|rows| {
        rows.iter().any(|row| {
            row["source"] == backend
                && row["capabilities"]
                    .as_array()
                    .is_some_and(|capabilities| capabilities.contains(&json!(Capability::Remove)))
        })
    })
}
/// A machine-readable name for what a change does, for the frontend.
fn operation_kind(operation: &Operation) -> &'static str {
    match operation {
        Operation::Install(_) => "install",
        Operation::Remove(_) => "remove",
        Operation::Upgrade(_) => "update",
        Operation::UpgradeAll { .. } => "update_all",
        Operation::Refresh { .. } => "refresh",
        Operation::Clean(_) => "clean",
    }
}
/// The row a package change targets, with the same fields as a package
/// row so the frontend can compute its row identity from it.
fn target_row(id: &PackageId) -> Value {
    json!({"source": id.backend, "name": id.name, "architecture": id.architecture,
        "remote": id.remote, "scope": id.scope, "reference": id.reference})
}
fn package_target(operation: &Operation) -> Option<&PackageId> {
    match operation {
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => Some(id),
        _ => None,
    }
}
/// What a notice says about the change itself, for a toast: the action,
/// the package and source in people's words, and whether Undo is safe.
fn notice_subject(operations: &[Operation], names: &Names, succeeded: bool) -> Value {
    let [operation] = operations else {
        return json!({"operation": if operations.is_empty() { "" } else { "batch" }, "undo": false, "undo_action": ""});
    };
    let undo = undo_action(operation).filter(|_| succeeded);
    let mut subject = json!({
        "operation": operation_kind(operation),
        "source": operation.backend(),
        "source_name": source_display_name(operation.backend()),
        "undo": undo.is_some(),
        "undo_action": undo.unwrap_or_default(),
    });
    if let Some(id) = package_target(operation) {
        subject["package"] = json!(display_name_for(id, names));
        subject["target"] = target_row(id);
    }
    subject
}
fn with_subject(mut notice: Value, subject: Value) -> Value {
    if let (Some(notice), Value::Object(subject)) = (notice.as_object_mut(), subject) {
        notice.extend(subject);
    }
    notice
}
/// What Retry runs again after a failure: the one change, or only the
/// steps of a batch that failed. A plan reviewed for the whole batch no
/// longer fits a part of it.
fn retry_job(job: &Job, result: &Result<Payload, EngineError>) -> Option<Job> {
    let failed: Vec<bool> = match result {
        Err(EngineError::Cancelled)
        | Err(EngineError::Execution(pkgdeck_core::process::ExecutionError::Cancelled)) => {
            return None
        }
        Err(_) => job.operations().iter().map(|_| true).collect(),
        Ok(Payload::Batch(_, outcomes)) => outcomes.iter().map(|o| *o == Outcome::Failed).collect(),
        Ok(_) => return None,
    };
    let keep = |operations: &[Operation]| -> Vec<Operation> {
        operations
            .iter()
            .zip(&failed)
            .filter(|(_, failed)| **failed)
            .map(|(operation, _)| operation.clone())
            .collect()
    };
    match job {
        Job::Write(..) if failed.iter().any(|f| *f) => Some(job.clone()),
        Job::UpgradeAll(operations, plan) => {
            let retry = keep(operations);
            let all = retry.len() == operations.len();
            (!retry.is_empty())
                .then(|| Job::UpgradeAll(retry, if all { plan.clone() } else { None }))
        }
        Job::CleanAll(operations) => {
            let retry = keep(operations);
            (!retry.is_empty()).then_some(Job::CleanAll(retry))
        }
        _ => None,
    }
}
/// An automatic run whose only failures are casks that need the password:
/// nothing is broken, the person just updates them by hand.
fn password_notice(
    job: &Job,
    result: &Result<Payload, EngineError>,
    password_casks: &[PackageId],
    names: &Names,
) -> Option<Value> {
    let (Job::AutoUpgrade(operations, _), Ok(Payload::Batch(_, outcomes))) = (job, result) else {
        return None;
    };
    let failed: Vec<&PackageId> = operations
        .iter()
        .zip(outcomes)
        .filter(|(_, outcome)| **outcome != Outcome::Finished)
        .map(|(operation, _)| match operation {
            Operation::Upgrade(id) if password_casks.contains(id) => Some(id),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let (title, it) = match failed.as_slice() {
        [] => return None,
        [one] => (
            format!(
                "{} needs your password to update",
                display_name_for(one, names)
            ),
            "it",
        ),
        many => (
            format!("{} apps need your password to update", many.len()),
            "them",
        ),
    };
    Some(json!({
        "kind": "info",
        "title": title,
        "detail": format!("Update {it} here and enter your password when asked. Automatic updates skip {it} until there's a newer version."),
    }))
}
/// The name a failure line starts with: the package, or the source for a
/// whole-source step.
fn short_name(operation: &Operation, names: &Names) -> String {
    match operation {
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
            display_name_for(id, names)
        }
        Operation::UpgradeAll { backend } => source_display_name(backend),
        _ => operation_title_named(operation, names),
    }
}
/// The banner title for one change that failed, such as "Firefox didn't
/// update".
fn failed_title(operation: &Operation, names: &Names) -> String {
    match operation {
        Operation::Upgrade(_) | Operation::UpgradeAll { .. } => {
            format!("{} didn't update", short_name(operation, names))
        }
        _ => format!("{} failed", operation_title_named(operation, names)),
    }
}
/// What the banner says once a change finishes.
fn write_notice(
    job: &Job,
    result: &Result<Payload, EngineError>,
    sudo: bool,
    names: &Names,
) -> Value {
    let operations = job.operations();
    let title = match operations.as_slice() {
        [one] => operation_title_named(one, names),
        [] => "The change".to_owned(),
        many => format!("{} changes", many.len()),
    };
    let succeeded = match result {
        Ok(Payload::Batch(_, outcomes)) => outcomes.iter().all(|o| *o == Outcome::Finished),
        Ok(_) => true,
        Err(_) => false,
    };
    let mut subject = notice_subject(&operations, names, succeeded);
    // Removing an adopted cask would delete the app the user already had,
    // so a finished adoption offers no Undo.
    if matches!(job, Job::Write(_, Some(plan)) if plan.adopts.is_some()) {
        subject["undo"] = json!(false);
        subject["undo_action"] = json!("");
    }
    let notice = match result {
        Err(
            EngineError::Cancelled
            | EngineError::Execution(pkgdeck_core::process::ExecutionError::Cancelled),
        ) => {
            json!({"kind": "info", "title": "Cancelled. Nothing else was changed."})
        }
        Err(error) => json!({
            "kind": "error",
            "title": match operations.as_slice() {
                [one] => failed_title(one, names),
                _ => format!("{title} failed"),
            },
            "detail": write_error_text(error, job_backend(&operations), sudo),
        }),
        Ok(Payload::Batch(_, outcomes))
            if !outcomes.contains(&Outcome::Failed) && outcomes.contains(&Outcome::Cancelled) =>
        {
            let done = outcomes.iter().filter(|o| **o == Outcome::Finished).count();
            json!({
                "kind": "info",
                "title": format!("{done} of {} changes finished. The rest were cancelled.", outcomes.len()),
            })
        }
        Ok(Payload::Batch(status, outcomes)) if outcomes.contains(&Outcome::Failed) => {
            // One line per failure, named the way people know the package.
            let failed: Vec<(&Operation, String)> = operations
                .iter()
                .zip(outcomes)
                .zip(status.lines().skip(1))
                .filter(|((_, outcome), _)| **outcome == Outcome::Failed)
                .map(|((operation, _), line)| {
                    let reason = line
                        .strip_prefix(&format!("{}: ", operation_title(operation)))
                        .unwrap_or(line);
                    let name = short_name(operation, names);
                    let line = if reason.starts_with(&name) {
                        reason.to_owned()
                    } else {
                        format!("{name}: {reason}")
                    };
                    (operation, line)
                })
                .collect();
            let detail = batch_status_text(
                &failed
                    .iter()
                    .map(|(_, line)| line.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                sudo,
            );
            let title = match failed.as_slice() {
                [(one, _)] => failed_title(one, names),
                many => format!("{} of {} changes failed", many.len(), outcomes.len()),
            };
            json!({"kind": "error", "title": title, "detail": detail})
        }
        Ok(payload) => {
            // Keep what the user must still do or know, such as a firmware
            // restart or a cancellation that came too late.
            let detail = match payload {
                Payload::Batch(status, _) => status
                    .lines()
                    .filter(|line| {
                        !line.starts_with("Completed ") && !line.ends_with(": Completed")
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Payload::Written(outcome) if outcome.cancellation_deferred => {
                    "Finished before it could be cancelled. Changes were kept.".into()
                }
                _ => String::new(),
            };
            if detail.is_empty() {
                json!({"kind": "success", "title": format!("{title} finished")})
            } else {
                json!({"kind": "success", "title": format!("{title} finished"), "detail": detail})
            }
        }
    };
    with_subject(notice, subject)
}
fn failure_details(failure: &BackendFailure, sudo: bool) -> QString {
    encoded(json!({
        "failure": {"backend": failure.backend, "error": plain_error(&failure.error, Some(&failure.backend), sudo)},
        "hint": format!("Check {} in the Sources view, or run it directly in a terminal for complete output.", source_display_name(&failure.backend)),
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
    details: String,
    status: Option<String>,
}
fn plan_checked_upgrade(
    packages: &[Package],
    identities: &str,
    failed: &FailedUpdates,
) -> CheckedPlan {
    let operations = without_failed(checked_upgrades(packages, identities), packages, failed);
    if operations.is_empty() {
        return CheckedPlan {
            operations: vec![],
            confirmation: String::new(),
            details: String::new(),
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
        confirmation: format!("Update {count} selected {}?\n\n{labels}\n\nOther packages may also change. If one update fails, the others can still finish.", if count == 1 { "package" } else { "packages" }),
        details: format!("{labels}\n\nOther packages may change."),
        status: None,
    }
}
/// Cache key for one view snapshot: view, search text, checked sources,
/// and elevation. The Installed filter is client-side, so it stays out of
/// the key and shares the loaded rows while typing.
fn prefetch_views() -> Vec<String> {
    ["Installed", "Updates", "Clean", "Sources"]
        .into_iter()
        .rev()
        .map(str::to_owned)
        .collect()
}
const VIEW_TTL: Duration = Duration::from_secs(60);
/// Preloaded sections older than this are loaded again in the background
/// while the app is open, so switching stays instant.
const REWARM_AFTER: Duration = Duration::from_secs(300);
fn cache_key(view: &str, query: &str, sources: &[String], sudo: bool) -> String {
    // Sources always lists every manager, whatever the filter.
    let sources = if view == "Sources" {
        String::new()
    } else {
        sources.join(",")
    };
    format!("{view}\0{query}\0{sources}\0{sudo}")
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
    /// A reload or write made this snapshot out of date. It still shows at
    /// once, marked stale, while the section loads again.
    expired: bool,
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
    const CAPACITY: usize = 32;
    fn get(&self, key: &str) -> Option<&CachedView> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, view)| view)
    }
    fn insert(&mut self, key: String, view: CachedView) {
        self.entries.retain(|(candidate, _)| candidate != &key);
        if self.entries.len() >= Self::CAPACITY {
            // Typing a search creates many entries; evict those first so
            // they never push out a preloaded section.
            let oldest = self
                .entries
                .iter()
                .position(|(candidate, _)| candidate.starts_with("Search\0"))
                .unwrap_or(0);
            self.entries.remove(oldest);
        }
        self.entries.push((key, view));
    }
    /// Mark every snapshot out of date without dropping it, so switching
    /// sections keeps showing the last rows while they load again.
    fn expire(&mut self) {
        for (_, view) in &mut self.entries {
            view.expired = true;
        }
    }
}
impl CachedView {
    fn stale(&self) -> bool {
        self.expired || self.loaded.elapsed() >= VIEW_TTL
    }
}
/// The slow sections' last snapshots, kept between runs so the next launch
/// shows them at once, marked stale, while they load again.
struct ViewStore {
    path: PathBuf,
    views: Vec<StoredView>,
    /// Writes run on their own threads; the newest one wins.
    written: std::sync::Arc<std::sync::Mutex<u64>>,
    generation: u64,
}
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct StoredView {
    key: String,
    packages: Vec<Package>,
    cleanup: Vec<CleanupItem>,
    rows: String,
    status: String,
    report_state: String,
    upgradable: bool,
    updates_view: bool,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredViews {
    /// Rows from another version may not read the same way.
    version: String,
    views: Vec<StoredView>,
}
impl ViewStore {
    /// Searches are too many and Sources is quick; the rest is worth keeping.
    fn keeps(key: &str) -> bool {
        ["Installed\0", "Updates\0", "Clean\0"]
            .iter()
            .any(|view| key.starts_with(view))
    }
    fn default_path() -> Option<PathBuf> {
        if cfg!(test)
            || rustix::process::geteuid().is_root()
            || std::env::var_os("PKGDECK_NO_CACHE").is_some()
        {
            return None;
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|p| std::path::Path::new(p).is_absolute())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(base.join("pkgdeck/views.json"))
    }
    /// Reads what the last run kept. A missing, damaged or older file is
    /// the same as none.
    fn open(path: PathBuf) -> Self {
        let views = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StoredViews>(&bytes).ok())
            .filter(|stored| stored.version == pkgdeck_core::VERSION)
            .map(|stored| stored.views)
            .unwrap_or_default();
        Self {
            path,
            views,
            written: Default::default(),
            generation: 0,
        }
    }
    /// The kept snapshots, out of date until they load again.
    fn cached(&self) -> Vec<(String, CachedView)> {
        let loaded = Instant::now()
            .checked_sub(VIEW_TTL)
            .unwrap_or_else(Instant::now);
        self.views
            .iter()
            .map(|view| {
                (
                    view.key.clone(),
                    CachedView {
                        loaded,
                        expired: true,
                        cleanup: view.cleanup.clone(),
                        packages: view.packages.clone(),
                        failures: vec![],
                        sources: vec![],
                        rows: view.rows.as_str().into(),
                        status: view.status.as_str().into(),
                        report_state: view.report_state.as_str().into(),
                        upgradable: view.upgradable,
                        updates_view: view.updates_view,
                    },
                )
            })
            .collect()
    }
    fn keep(&mut self, view: StoredView) {
        self.views.retain(|old| old.key != view.key);
        self.views.push(view);
        self.generation += 1;
        let (generation, written) = (self.generation, self.written.clone());
        let (path, stored) = (
            self.path.clone(),
            StoredViews {
                version: pkgdeck_core::VERSION.into(),
                views: self.views.clone(),
            },
        );
        thread::spawn(move || write_views(&path, &stored, generation, &written));
    }
}
/// Writes the kept snapshots unless a newer generation already was: writes
/// run on their own threads and may finish out of order. Whole, then
/// renamed, so a crash never leaves half a file.
fn write_views(
    path: &std::path::Path,
    stored: &StoredViews,
    generation: u64,
    written: &std::sync::Mutex<u64>,
) -> bool {
    // A writer that panicked left a generation that's still worth comparing.
    let mut last = written
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *last > generation {
        return false;
    }
    *last = generation;
    let temporary = path.with_extension("json.tmp");
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    serde_json::to_vec(stored)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            std::fs::create_dir_all(dir)?;
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(&temporary, path)
        })
        .is_ok()
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
/// One line for progress and results, such as "Install htop (APT)".
fn operation_title(operation: &Operation) -> String {
    operation_title_named(operation, &Names::new())
}
/// The name people know a package by: its display name when the change
/// was confirmed from a row, otherwise the name in its identity.
fn display_name_for(id: &PackageId, names: &Names) -> String {
    names
        .get(id)
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| package_name(id).to_owned())
}
/// Like `operation_title`, with the package's display name when known.
fn operation_title_named(operation: &Operation, names: &Names) -> String {
    match operation {
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => format!(
            "{} {} ({})",
            action_name(operation),
            display_name_for(id, names),
            source_display_name(&id.backend)
        ),
        _ => operation_label(operation)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}
/// The package name shown for an identity. A downloaded AppImage shows its
/// file name, and a reference such as a Flatpak ref wins over the name
/// (except an App Store ID).
fn package_name(id: &PackageId) -> &str {
    match id.reference.as_deref() {
        // The App Store ID only selects the app; people know it by name.
        Some(_) if id.backend == "mas" => &id.name,
        // A local AppImage's reference is its checksum; show the file.
        Some(_) if id.backend == "appimage" => id
            .name
            .split(['?', '#'])
            .next()
            .unwrap_or(&id.name)
            .rsplit('/')
            .next()
            .unwrap_or(&id.name),
        Some(reference) if !reference.starts_with("artifact:") => reference,
        _ => &id.name,
    }
}
fn operation_label(operation: &Operation) -> String {
    let (action, id) = match operation {
        Operation::Install(id) => ("Install", id),
        Operation::Remove(id) => ("Remove", id),
        Operation::Upgrade(id) => ("Update", id),
        Operation::Refresh { backend } => {
            return format!("Refresh {} package lists", source_display_name(backend))
        }
        Operation::UpgradeAll { backend } => {
            return format!("Update all {} packages", source_display_name(backend))
        }
        Operation::Clean(id) => {
            return format!("Clean {} ({})", id.key, source_display_name(&id.backend))
        }
    };
    format!(
        "{action} {}\nSource: {}\nArchitecture: {}\nScope: {}",
        package_name(id),
        source_display_name(&id.backend),
        id.architecture,
        scope_word(&id.scope)
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
    let mut summary = vec![operation_label(operation)];
    let mut title = action_name(operation).to_owned();
    let mut icon = None;
    // Installing or updating one app needs no second look. Anything that
    // removes, changes more than that app, or moves files is reviewed.
    let mut review = !matches!(operation, Operation::Install(_) | Operation::Upgrade(_));
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
        let source = source_display_name(&package.id.backend);
        let place = match &package.id.scope {
            Scope::System | Scope::User { .. } => {
                source_and_scope(&package.id.backend, &package.id.scope)
            }
            Scope::Environment { path } => {
                lines.push(format!("Location: {}", path.display()));
                source
            }
        };
        summary[0] = format!("{title}\n{place}");
        if !package.display_name.is_empty() && package.display_name != package.id.name {
            lines.insert(1, package.display_name.clone());
        }
        if let Some(version) = &package.installed_version {
            lines.push(format!("Installed: {version}"));
        }
        if let Some(version) = &package.candidate_version {
            lines.push(format!("Available: {version}"));
        }
        // The version line says what changes: old → new for an update, the
        // version that goes for a removal, the one that arrives for an install.
        match (
            operation,
            &package.installed_version,
            &package.candidate_version,
        ) {
            (Operation::Remove(_), Some(old), _) => summary.push(old.clone()),
            (Operation::Install(_), _, Some(new)) => summary.push(new.clone()),
            (_, Some(old), Some(new)) if old != new => summary.push(format!("{old} → {new}")),
            (_, Some(version), _) | (_, None, Some(version)) => summary.push(version.clone()),
            _ => {}
        }
        if package.id.backend == "fwupd" {
            lines.push(package.summary.clone());
            summary.push(package.summary.clone());
        }
        // An AppImage being installed shows what it is, like a store page.
        if package.id.backend == "appimage" && matches!(operation, Operation::Install(_)) {
            if !package.summary.is_empty() {
                summary.push(package.summary.clone());
            }
            icon.clone_from(&package.icon);
        }
        if (package.id.backend == "appimage"
            || package.id.reference.as_deref().is_some_and(|value| {
                value.starts_with("local-deb:")
                    || value.starts_with("flatpakref:")
                    || value.starts_with("artifact:")
            }))
            && matches!(operation, Operation::Install(_))
        {
            lines.push(package.summary.clone());
        }
    }
    if let Operation::Clean(id) = operation {
        if let Some(item) = cleanup.iter().find(|item| item.id == *id) {
            title = format!("Clean {}", item.title);
            lines.push(item.preview.clone());
            summary.push(item.preview.clone());
        }
    }
    if let Operation::Refresh { backend } = operation {
        title = format!("Refresh {}", source_display_name(backend));
    }
    if !matches!(
        operation,
        Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
    ) {
        summary[0] = title.clone();
    }
    // What follows the name, source and version is worth saying on an app
    // page too, which already shows those.
    let notes_from = summary.len();
    let mut other_changes = Vec::new();
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
        // Managing an AppImage moves the very file in; nothing is downloaded.
        let moving = plan.adopts.is_some() && operation.backend() == "appimage";
        let describe = |change: &PlannedChange| {
            let action = match change.action {
                PlannedAction::Install => "Install",
                PlannedAction::Remove => "Remove",
                PlannedAction::Upgrade => "Update",
            };
            let version = match (&change.installed_version, &change.candidate_version) {
                (Some(old), Some(new)) if old == new || change.action == PlannedAction::Remove => {
                    format!(" ({old})")
                }
                (Some(old), Some(new)) => format!(" ({old} → {new})"),
                (Some(old), None) => format!(" ({old})"),
                (None, Some(new)) => format!(" ({new})"),
                (None, None) => String::new(),
            };
            if moving && change.action == PlannedAction::Install {
                let version = change
                    .candidate_version
                    .as_deref()
                    .map(|version| format!(" {version}"))
                    .unwrap_or_default();
                return format!("Move {}{version} into PkgDeck", change.name);
            }
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
            lines.push(format!("Selected: {}", requested.join(", ")));
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
        review |= !changes.is_empty() || plan.adopts.is_some();
        other_changes.clone_from(&changes);
        if !changes.is_empty() {
            let removals = plan
                .changes
                .iter()
                .filter(|change| {
                    change.action == PlannedAction::Remove
                        && (Some(change.name.as_str()) != requested_name
                            || Some(change.action) != requested_action)
                })
                .map(|change| change.name.as_str())
                .collect::<Vec<_>>();
            summary.push(format!(
                "{} other {} will change",
                changes.len(),
                if changes.len() == 1 {
                    "package"
                } else {
                    "packages"
                }
            ));
            if !removals.is_empty() {
                summary.push(format!("Removes: {}", removals.join(", ")));
            }
        }
        if plan.restart_required == Some(true) {
            summary.push("Restart required".into());
        }
        if !changes.is_empty() {
            lines.push(format!("Other changes:\n{}", changes.join("\n")));
        }
        let mut impact = Vec::new();
        if let Some(bytes) = plan.download_bytes {
            impact.push(format!("Download: {bytes} bytes"));
        }
        if let Some(bytes) = plan.disk_bytes {
            impact.push(format!("Disk: {bytes:+} bytes"));
        }
        if !impact.is_empty() {
            lines.push(impact.join(", "));
        }
    } else if matches!(
        operation,
        Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
    ) && operation.backend() != "appimage"
    {
        // AppImages carry everything they need.
        summary.push("Other changes may be required.".into());
    }
    if matches!(operation, Operation::Remove(_)) {
        summary.push("App data may remain after removal.".into());
    }
    if title.chars().count() > 36 {
        title = format!("{}…", title.chars().take(35).collect::<String>());
    }
    let flatpak_ref_scope = match operation {
        Operation::Install(id)
            if id.backend == "flatpak"
                && id
                    .reference
                    .as_deref()
                    .is_some_and(|value| value.starts_with("flatpakref:")) =>
        {
            match &id.scope {
                Scope::System => "system",
                Scope::User { .. } => "user",
                _ => "",
            }
        }
        _ => "",
    };
    json!({"action": title, "body": lines.join("\n\n"), "summary": summary.join("\n"), "details": lines.iter().skip(1).filter(|line| !summary.contains(line)).cloned().collect::<Vec<_>>().join("\n\n"), "flatpak_ref_scope": flatpak_ref_scope, "icon": icon, "review": review || !flatpak_ref_scope.is_empty(), "notes": summary.get(notes_from..).unwrap_or_default(), "changes": other_changes})
}
/// The app page of an opened file or link: the `details` shape, plus where
/// it came from and what its button does.
fn opened_page(details: &PackageDetails, action: &str) -> Value {
    let package = &details.package;
    let info = crate::metadata::cached_info(package);
    let summary = package.summary.lines().next().unwrap_or_default();
    let description = info
        .as_ref()
        .map(|info| info.description.clone())
        .filter(|description| !description.is_empty())
        .unwrap_or_else(|| details.description.clone());
    let location = match package.id.reference.as_deref() {
        Some(reference) if reference.starts_with("local-deb:") => reference
            .splitn(3, ':')
            .nth(2)
            .unwrap_or_default()
            .to_owned(),
        Some(reference) if reference.starts_with("flatpakref:") => package.id.name.clone(),
        _ => package.id.name.clone(),
    };
    let mut row = package_row(package, &[], None);
    row["summary"] = json!(summary);
    json!({
        "package": row,
        "description": description,
        "homepage": details.homepage.clone().or_else(|| info.as_ref().and_then(|i| i.homepage.clone())),
        "publisher": info.as_ref().and_then(|i| i.publisher.clone()),
        "license": info.as_ref().and_then(|i| i.license.clone()),
        "dependencies": details.dependencies,
        "screenshots": info.as_ref().map(|i| i.screenshots.clone()).unwrap_or_default(),
        "location": location,
        "action": action,
    })
}
/// The list rows for `packages`, in order, with the apps each one shares.
fn package_rows(packages: &[Package]) -> Vec<Value> {
    let same = same_app_sources_all(packages);
    let groups = same_app_group_keys_all(packages);
    packages
        .iter()
        .zip(same.into_iter().zip(groups))
        .map(|(p, (from, group))| package_row(p, &from, group.as_deref()))
        .collect()
}
/// Encoded rows for exactly these packages.
struct PreparedRows {
    packages: Vec<Package>,
    json: String,
}
impl PreparedRows {
    fn new(packages: &[Package]) -> Box<Self> {
        Box::new(Self {
            packages: packages.to_vec(),
            json: serde_json::to_string(&package_rows(packages)).expect("serializable rows"),
        })
    }
}
/// Rows made on a worker, newest last. A few are kept: a preload's rows are
/// used when its section opens, which may be after other loads. The cache
/// saves the window the work of encoding rows, so it holds only a few of them
/// and never more than its budget: a list of tens of thousands of packages
/// must not be kept four times over.
#[derive(Default)]
struct RowsCache(Vec<PreparedRows>);
impl RowsCache {
    const CAPACITY: usize = 4;
    /// What every snapshot kept here may add up to. The packages count by
    /// their own size, which leaves out the strings behind them, so this
    /// bounds the order of a few large lists rather than an exact figure.
    const BUDGET: usize = 64 * 1024 * 1024;
    fn size(rows: &PreparedRows) -> usize {
        rows.json.len() + rows.packages.len() * std::mem::size_of::<Package>()
    }
    fn insert(&mut self, rows: PreparedRows) {
        self.0.retain(|kept| kept.packages != rows.packages);
        while !self.0.is_empty()
            && (self.0.len() >= Self::CAPACITY
                || Self::size(&rows) + self.0.iter().map(Self::size).sum::<usize>() > Self::BUDGET)
        {
            self.0.remove(0);
        }
        self.0.push(rows);
    }
    /// The encoded rows for `packages`, made on a worker or here.
    fn json(&self, packages: &[Package]) -> String {
        self.0
            .iter()
            .rev()
            .find(|kept| kept.packages == packages)
            .map(|kept| kept.json.clone())
            .unwrap_or_else(|| {
                serde_json::to_string(&package_rows(packages)).expect("serializable rows")
            })
    }
}
/// `rows`, an encoded array, with `more` rows appended.
fn append_rows(mut rows: String, more: &[Value]) -> String {
    if more.is_empty() {
        return rows;
    }
    let more = serde_json::to_string(more).expect("serializable rows");
    rows.pop();
    if rows.len() > 1 {
        rows.push(',');
    }
    rows.push_str(&more[1..]);
    rows
}
fn package_row(p: &Package, same_from: &[String], same_group: Option<&str>) -> Value {
    json!({"name": p.id.name, "display_name": p.display_name, "source": p.id.backend, "architecture": p.id.architecture,
        "remote": p.id.remote, "reference": p.id.reference, "scope": p.id.scope, "scope_label": scope_label(&p.id.scope), "summary": p.summary, "installed": p.installed_version,
        "candidate": p.candidate_version, "update": p.update, "kind": "package", "icon": p.icon,
        "same_app_from": same_from, "same_app_group": same_group, "adopt_with": p.adopt_with})
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
    pub fn open_input(mut self: Pin<&mut Self>, input: QString) {
        if input.to_string().trim().is_empty() {
            self.as_mut()
                .set_status("Choose an installation file.".into());
            return;
        }
        if self.rust().foreground_worker() {
            self.as_mut().rust_mut().queued = Some(Job::OpenInput(input.to_string()));
            self.as_mut()
                .set_status("The file will open when the current operation finishes.".into());
            return;
        }
        self.start(Job::OpenInput(input.to_string()));
    }
    /// The AppImage source: the person's own, or a test's data folder.
    fn appimages(&self) -> AppImage {
        match &self.rust().appimage_data {
            Some(data) => AppImage::with_data(data),
            None => AppImage::native(),
        }
    }
    /// The installed AppImage row at `index`.
    fn installed_appimage(&self, index: i32) -> Option<Package> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().packages.get(i))
            .filter(|p| p.id.backend == "appimage" && p.installed_version.is_some())
            .cloned()
    }
    pub fn app_launch_settings(self: Pin<&mut Self>, index: i32) -> QString {
        let Some(package) = self.installed_appimage(index) else {
            return "{}".into();
        };
        encoded(match self.appimages().launch_settings(&package.id) {
            Ok(settings) => json!({
                "arguments": settings.arguments,
                "environment": settings.environment.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>(),
                "editable": settings.editable,
            }),
            Err(error) => json!({"error": plain_error(&error, None, false)}),
        })
    }
    pub fn save_app_launch_settings(
        self: Pin<&mut Self>,
        index: i32,
        arguments: QString,
        environment: QString,
    ) -> QString {
        let Some(package) = self.installed_appimage(index) else {
            return "Nothing to change.".into();
        };
        let environment: Vec<(String, String)> =
            serde_json::from_str::<Vec<Value>>(&environment.to_string())
                .unwrap_or_default()
                .iter()
                .map(|pair| {
                    (
                        pair["name"].as_str().unwrap_or_default().trim().to_owned(),
                        pair["value"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .filter(|(name, _)| !name.is_empty())
                .collect();
        match self.appimages().set_launch_settings(
            &package.id,
            &arguments.to_string(),
            &environment,
        ) {
            Ok(()) => QString::default(),
            Err(error) => plain_error(&error, None, false).as_str().into(),
        }
    }
    pub fn app_update_source(self: Pin<&mut Self>, index: i32) -> QString {
        let Some(package) = self.installed_appimage(index) else {
            return "{}".into();
        };
        encoded(match self.appimages().update_source(&package.id) {
            Ok(source) => json!(source),
            Err(error) => json!({"error": plain_error(&error, None, false)}),
        })
    }
    pub fn save_app_update_source(
        mut self: Pin<&mut Self>,
        index: i32,
        github: QString,
    ) -> QString {
        let Some(package) = self.installed_appimage(index) else {
            return "Nothing to change.".into();
        };
        match self
            .appimages()
            .set_update_source(&package.id, &github.to_string())
        {
            Ok(()) => {
                // Sections loaded before don't know about the new source.
                self.as_mut().rust_mut().view_cache.expire();
                QString::default()
            }
            Err(error) => plain_error(&error, None, false).as_str().into(),
        }
    }
    pub fn app_file(self: Pin<&mut Self>, index: i32) -> QString {
        let Some(package) = self.installed_appimage(index) else {
            return "{}".into();
        };
        encoded(match self.appimages().file(&package.id) {
            Ok(file) => {
                let folder = file
                    .path
                    .parent()
                    .map(|folder| folder.display().to_string());
                let mut value = json!(file);
                value["folder"] = json!(folder);
                value
            }
            Err(_) => json!({}),
        })
    }
    /// The installed AppImage an opened file became: PkgDeck keeps it under
    /// its digest, which the page's preview already has.
    fn opened_appimage(&self) -> Option<PackageId> {
        let package = self.rust().opened_package.as_ref()?;
        let digest = package.id.reference.as_deref()?;
        (package.id.backend == "appimage").then(|| {
            self.appimages()
                .installed_id(digest, &package.id.architecture)
        })
    }
    pub fn launch_opened(self: Pin<&mut Self>) -> QString {
        let Some(id) = self.opened_appimage() else {
            return "Nothing to start.".into();
        };
        match self.appimages().launch(&id) {
            Ok(()) => QString::default(),
            Err(error) => plain_error(&error, None, false).as_str().into(),
        }
    }
    pub fn launch_app(self: Pin<&mut Self>, index: i32) -> QString {
        let Some(package) = self.installed_appimage(index) else {
            return "Nothing to start.".into();
        };
        match self.appimages().launch(&package.id) {
            Ok(()) => QString::default(),
            Err(error) => plain_error(&error, None, false).as_str().into(),
        }
    }
    /// Install (or Manage) what the open app page shows: preview the change
    /// as opening a file did before it had a page.
    pub fn install_opened(mut self: Pin<&mut Self>) {
        let Some(package) = self.rust().opened_package.clone() else {
            return;
        };
        if package.adopt_with.as_deref() == Some("appimage") {
            self.as_mut().start(Job::PlanAdoption(Box::new(package)));
            return;
        }
        let operation = Operation::Install(package.id.clone());
        if package.id.backend == "apt" {
            self.as_mut().start(Job::PlanOperation(operation));
        } else {
            self.as_mut()
                .apply(Ok(Payload::OperationPreview(operation, None)));
        }
    }
    /// Once the open page's own install finished, it shows the app as
    /// installed, with nothing left to press.
    fn mark_opened_installed(
        mut self: Pin<&mut Self>,
        job: &Job,
        result: &Result<Payload, EngineError>,
    ) {
        let Some(package) = self.rust().opened_package.clone() else {
            return;
        };
        let installed = result.is_ok()
            && job
                .operations()
                .iter()
                .any(|operation| matches!(operation, Operation::Install(id) if *id == package.id));
        if !installed {
            return;
        }
        // The page was written from this same package just before.
        let mut data: Value = serde_json::from_str(&self.opened().to_string()).unwrap_or_default();
        data["action"] = json!("");
        data["package"]["installed"] = data["package"]["candidate"].clone();
        self.as_mut().set_opened(encoded(data));
    }
    pub fn close_opened(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().opened_package = None;
        self.as_mut().set_opened(QString::default());
    }
    pub fn set_open_flatpak_scope(mut self: Pin<&mut Self>, system: bool) {
        let Some(Job::Write(Operation::Install(previous), None)) = &self.rust().pending else {
            return;
        };
        if previous.backend != "flatpak"
            || !previous
                .reference
                .as_deref()
                .is_some_and(|value| value.starts_with("flatpakref:"))
        {
            return;
        }
        let previous = previous.clone();
        let scope = if system {
            Scope::System
        } else {
            Scope::User {
                uid: rustix::process::getuid().as_raw(),
            }
        };
        if previous.scope == scope {
            return;
        }
        {
            let rust = self.as_mut().rust_mut();
            let Some(package) = rust
                .packages
                .iter_mut()
                .find(|package| package.id == previous)
            else {
                return;
            };
            package.id.scope = scope.clone();
        }
        let mut selected = previous;
        selected.scope = scope;
        self.as_mut().apply(Ok(Payload::OperationPreview(
            Operation::Install(selected),
            None,
        )));
    }
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
                    format!(
                        "Autostart couldn't be changed: {}",
                        sentence_tail(&plain_text(&error.to_string(), None))
                    )
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
    pub fn set_auto_update(mut self: Pin<&mut Self>, enabled: bool) {
        self.as_mut().rust_mut().auto_update = enabled;
    }
    pub fn set_allow_removals(mut self: Pin<&mut Self>, allowed: bool) {
        self.as_mut().rust_mut().allow_removals = allowed;
    }
    pub fn restart_app(mut self: Pin<&mut Self>, background: bool) -> bool {
        let Some(install) = self.rust().install.clone() else {
            return false;
        };
        match install.relaunch(background) {
            Ok(()) => true,
            Err(error) => {
                self.as_mut().set_status(
                    format!(
                        "PkgDeck couldn't restart: {}",
                        sentence_tail(&plain_text(&error.to_string(), None))
                    )
                    .as_str()
                    .into(),
                );
                false
            }
        }
    }
    pub fn restore_system_approval(self: Pin<&mut Self>, approval: QString) {
        // The MacPorts-only entry of earlier versions covers too little to
        // keep showing as on; turning it on again replaces it.
        #[cfg(target_os = "macos")]
        let approval = if approval.to_string() == "sudoers:macports" {
            QString::default()
        } else {
            approval
        };
        self.set_system_approval(approval);
    }
    pub fn allow_system_updates(mut self: Pin<&mut Self>, allow: bool) {
        if self.rust().approval_worker.is_some() {
            return;
        }
        self.as_mut().set_approval_error(QString::default());
        let natives = self.rust().natives;
        let host = (natives.host)();
        let (sender, receiver) = crate::wake::channel();
        let handle = thread::spawn(move || {
            let result = (natives.approve)(&host, allow, &Cancellation::default())
                .map(|key| if allow { key } else { String::new() })
                .map_err(|error| approval_error(&error));
            let _ = sender.send(result);
        });
        self.as_mut().rust_mut().approval_worker = Some(ApprovalWorker { handle, receiver });
        self.sync_needs_poll();
    }
    fn poll_approval(mut self: Pin<&mut Self>) {
        let Some(worker) = &self.rust().approval_worker else {
            return;
        };
        let Ok(result) = worker.receiver.try_recv() else {
            return;
        };
        let worker = self.as_mut().rust_mut().approval_worker.take().unwrap();
        let _ = worker.handle.join();
        match result {
            Ok(key) => self.as_mut().set_system_approval(key.as_str().into()),
            Err(error) => self.as_mut().set_approval_error(error.as_str().into()),
        }
    }
    /// After a background check: queue the updates it found as one
    /// automatic run, when that is on and nothing else is waiting. Returns
    /// whether a run was queued.
    fn schedule_auto_update(mut self: Pin<&mut Self>, report: &PackageReport) -> bool {
        use pkgdeck_core::unattended::{unattended, Unattended};
        let rust = self.rust();
        if !rust.auto_update
            || rust.worker.is_some()
            || rust.pending.is_some()
            || rust.queued.is_some()
            || rust.validated_confirmed.is_some()
            || !rust.confirmed_queue.is_empty()
        {
            return false;
        }
        let approved = rust.approval_current();
        // A source whose check failed may have more to update than it said.
        let packages: Vec<Package> = report
            .packages
            .iter()
            .filter(|p| !report.failures.iter().any(|f| f.backend == p.id.backend))
            .filter(|p| !pkgdeck_core::backends::inventory_only(&p.id.backend))
            .filter(|p| match unattended(&p.id.backend) {
                Unattended::User => true,
                Unattended::Approved => approved,
                Unattended::Never => false,
            })
            .cloned()
            .collect();
        let casks: Vec<(String, String)> = packages
            .iter()
            .filter(|p| p.id.backend == "homebrew-cask")
            .map(|p| (p.id.name.clone(), cask_version(p).to_owned()))
            .collect();
        // Updated by hand or replaced by a newer version: try it again.
        if !report.failures.iter().any(|f| f.backend == "homebrew-cask") {
            self.as_mut()
                .rust_mut()
                .needs_password
                .remember(&[], &casks);
        }
        let operations = automatic_plan(
            &packages,
            &self.rust().needs_password,
            &self.rust().failed_updates,
        );
        if operations.is_empty() {
            return false;
        }
        let versions = packages
            .iter()
            .filter(|p| p.id.backend == "homebrew-cask")
            .map(|p| (p.id.clone(), cask_version(p).to_owned()))
            .collect();
        self.as_mut().rust_mut().cask_versions = versions;
        let names = self.rust().names_for(&operations);
        self.as_mut().rust_mut().names.extend(names);
        let activity_id = self
            .rust()
            .activity_store
            .clone()
            .and_then(|store| store.begin("auto", operations.clone(), State::Queued).ok());
        self.as_mut().rust_mut().validated_confirmed = Some(Confirmed {
            job: Job::AutoUpgrade(operations, self.rust().allow_removals),
            activity_id,
            cleanup_preview: vec![],
        });
        self.sync_needs_poll();
        true
    }
    pub fn set_check_interval(mut self: Pin<&mut Self>, minutes: i32) {
        let seconds = u64::try_from(minutes).unwrap_or(0).saturating_mul(60);
        self.as_mut()
            .rust_mut()
            .background_schedule
            .set_interval(seconds);
    }
    pub fn restore_notification_history(mut self: Pin<&mut Self>, history: QString) {
        self.as_mut()
            .rust_mut()
            .background_schedule
            .restore_notifications(&history.to_string());
        let saved = self.rust().background_schedule.notification_history();
        self.as_mut()
            .set_notification_history(saved.as_str().into());
    }
    pub fn acknowledge_notification(mut self: Pin<&mut Self>) {
        self.as_mut()
            .rust_mut()
            .background_schedule
            .acknowledge_notification();
        let saved = self.rust().background_schedule.notification_history();
        self.as_mut()
            .set_notification_history(saved.as_str().into());
    }
    /// Remember which updates of a finished change failed, so automatic
    /// runs and Update all skip them, and forget the ones that worked.
    /// Casks that stopped for the password are remembered apart.
    fn remember_update_outcomes(
        mut self: Pin<&mut Self>,
        job: &Job,
        result: &Result<Payload, EngineError>,
        password_casks: &[PackageId],
    ) {
        let operations = job.operations();
        let failed: Vec<bool> = match result {
            Ok(Payload::Batch(_, outcomes)) => {
                outcomes.iter().map(|o| *o == Outcome::Failed).collect()
            }
            Ok(_) => vec![false; operations.len()],
            Err(
                EngineError::Cancelled
                | EngineError::Execution(pkgdeck_core::process::ExecutionError::Cancelled),
            ) => return,
            // One change on its own failed as a whole.
            Err(_) if operations.len() == 1 => vec![true],
            Err(_) => return,
        };
        let version = |id: &PackageId| -> Option<String> {
            let rust = self.rust();
            rust.packages
                .iter()
                .find(|package| package.id == *id)
                .and_then(|package| package.candidate_version.clone())
                .or_else(|| rust.cask_versions.get(id).cloned())
        };
        let mut failures = Vec::new();
        let mut worked = Vec::new();
        let mut needed = Vec::new();
        for (operation, failed) in operations.iter().zip(failed) {
            let Operation::Upgrade(id) = operation else {
                continue;
            };
            if !failed {
                worked.push(id.clone());
            } else if password_casks.contains(id) {
                needed.extend(version(id).map(|version| (id.name.clone(), version)));
            } else if let Some(version) = version(id) {
                failures.push((id.clone(), version));
            }
        }
        if failures.is_empty() && worked.is_empty() && needed.is_empty() {
            return;
        }
        let worked_casks: Vec<String> = worked
            .iter()
            .filter(|id| id.backend == "homebrew-cask")
            .map(|id| id.name.clone())
            .collect();
        let rust = self.as_mut().rust_mut();
        rust.failed_updates.remember(&failures, &worked);
        // Automatic runs remember theirs once the run ends.
        if !matches!(job, Job::AutoUpgrade(..)) {
            rust.needs_password.update(&needed, &worked_casks);
        }
        self.publish_held_updates();
    }
    /// A failure banner goes away once every update it reports is no longer
    /// waiting, such as after updating them some other way.
    fn clear_settled_failure(self: Pin<&mut Self>) {
        let Some(job) = &self.rust().retry_job else {
            return;
        };
        let operations = job.operations();
        let settled = !operations.is_empty()
            && operations.iter().all(|operation| match operation {
                Operation::Upgrade(id) => !self
                    .rust()
                    .packages
                    .iter()
                    .any(|p| p.id == *id && p.update == UpdateAvailability::Available),
                _ => false,
            });
        if settled {
            self.dismiss_notice();
        }
    }
    /// The updates the Updates page marks: ones that failed last time, and
    /// casks that need the password. Each still waits at that version.
    fn publish_held_updates(self: Pin<&mut Self>) {
        let rust = self.rust();
        let candidate = |backend: &str, name: &str, id: Option<&PackageId>| {
            rust.packages
                .iter()
                .find(|p| {
                    id.map_or(p.id.backend == backend && p.id.name == name, |id| {
                        p.id == *id
                    })
                })
                .map(|p| cask_version(p).to_owned())
        };
        let mut held: Vec<Value> = rust
            .failed_updates
            .updates()
            .iter()
            .filter(|update| {
                candidate("", "", Some(&update.id)).is_none_or(|version| version == update.version)
            })
            .map(|update| {
                let id = &update.id;
                json!({"reason": "failed", "source": id.backend, "name": id.name,
                    "identity": [id.backend, id.name, id.architecture, id.remote, id.scope, id.reference]})
            })
            .collect();
        held.extend(
            rust.needs_password
                .casks()
                .filter(|(name, version)| {
                    candidate("homebrew-cask", name, None).is_none_or(|current| current == *version)
                })
                .map(|(name, _)| json!({"reason": "password", "source": "homebrew-cask", "name": name})),
        );
        self.set_held_updates(encoded(json!(held)));
    }
    fn finish_background_check(mut self: Pin<&mut Self>, report: PackageReport) {
        // Updated by hand or replaced by a newer version: try it again.
        let answered: Vec<String> = self
            .rust()
            .failed_updates
            .updates()
            .iter()
            .map(|update| update.id.backend.clone())
            .filter(|backend| !report.failures.iter().any(|f| f.backend == *backend))
            .collect();
        let waiting: Vec<(PackageId, String)> = report
            .packages
            .iter()
            .filter(|p| p.update == UpdateAvailability::Available)
            .map(|p| (p.id.clone(), cask_version(p).to_owned()))
            .collect();
        let answered: Vec<&str> = answered.iter().map(String::as_str).collect();
        self.as_mut()
            .rust_mut()
            .failed_updates
            .keep_waiting(&answered, &waiting);
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
        // An automatic update tells what it did when it finishes, instead
        // of announcing updates it is about to install.
        let automatic = self.as_mut().schedule_auto_update(&report);
        if automatic {
            self.as_mut()
                .rust_mut()
                .background_schedule
                .acknowledge_notification();
        }
        let saved = self.rust().background_schedule.notification_history();
        self.as_mut()
            .set_notification_history(saved.as_str().into());
        self.as_mut().set_background_state(encoded(json!({"last_check": checked, "available": result.count, "failures": failures, "notify": result.notify && !automatic})));
    }
    fn finish_background_error(mut self: Pin<&mut Self>, error: &EngineError) {
        let available = serde_json::from_str::<Value>(&self.background_state().to_string())
            .ok()
            .and_then(|state| state["available"].as_u64())
            .unwrap_or(0);
        let checked = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let failures: Vec<_> = match error {
            EngineError::Incomplete(sources) => sources
                .iter()
                .map(
                    |source| json!({"source": source.backend, "kind": failure_kind(&source.error)}),
                )
                .collect(),
            _ => vec![json!({"source": "check", "kind": failure_kind(error)})],
        };
        self.as_mut().set_background_state(encoded(json!({"last_check": checked, "available": available, "failures": failures, "notify": false})));
    }
    pub fn dismiss_notice(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().retry_job = None;
        self.set_notice("{}".into());
    }
    /// Run again what the banner reports as failed. The change was already
    /// confirmed, and it is reviewed again before it runs, like any other.
    pub fn retry_change(mut self: Pin<&mut Self>) {
        let Some(job) = self.as_mut().rust_mut().retry_job.take() else {
            return;
        };
        self.as_mut().set_notice("{}".into());
        self.as_mut().accept_confirmed(job);
        self.sync_needs_poll();
    }
    /// Read the Activity history on a worker; poll() publishes it. A read
    /// requested while one runs starts again after it, so the newest state
    /// always lands last.
    pub fn refresh_activity(mut self: Pin<&mut Self>) {
        let Some(store) = self.rust().activity_store.clone() else {
            return;
        };
        if self.rust().activity_worker.is_some() {
            self.as_mut().rust_mut().activity_again = true;
            return;
        }
        let (sender, receiver) = crate::wake::channel();
        let handle = thread::spawn(move || {
            let _ = sender.send(store.load());
        });
        self.as_mut().rust_mut().activity_worker = Some(ActivityWorker { handle, receiver });
        self.sync_needs_poll();
    }
    fn poll_activity(mut self: Pin<&mut Self>) {
        let Some(worker) = &self.rust().activity_worker else {
            return;
        };
        if !worker.handle.is_finished() {
            return;
        }
        let worker = self.as_mut().rust_mut().activity_worker.take().unwrap();
        let _ = worker.handle.join();
        if let Ok(Ok(entries)) = worker.receiver.try_recv() {
            let rows = activity_rows(&entries, &self.rust().names);
            self.as_mut().set_activity(rows);
        }
        if std::mem::take(&mut self.as_mut().rust_mut().activity_again) {
            self.refresh_activity();
        }
    }
    /// Keep `needs_poll` true exactly while poll() has something to do.
    fn sync_needs_poll(mut self: Pin<&mut Self>) {
        let outstanding = self.rust().outstanding();
        self.as_mut().set_needs_poll(outstanding);
        let reading = self.rust().only_reading();
        self.as_mut().set_reading(reading);
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
        let names = self.rust().names_for(&operations);
        if self.rust().names.len() > 256 {
            self.as_mut().rust_mut().names.clear();
        }
        self.as_mut().rust_mut().names.extend(names);
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
        self.sync_needs_poll();
    }
    fn successful_sources(&self) -> Vec<String> {
        let state: Value =
            serde_json::from_str(&self.report_state().to_string()).unwrap_or_default();
        serde_json::from_value(state["successful_sources"].clone()).unwrap_or_default()
    }
    /// When the rows on screen were read, as `report_state.checked_at`.
    fn set_checked_at(mut self: Pin<&mut Self>, epoch: u64) {
        let mut state: Value =
            serde_json::from_str(&self.report_state().to_string()).unwrap_or_else(|_| json!({}));
        state["checked_at"] = json!(epoch);
        self.as_mut().set_report_state(encoded(state));
    }
    fn set_phase(mut self: Pin<&mut Self>, phase: &str) {
        let mut state: Value =
            serde_json::from_str(&self.report_state().to_string()).unwrap_or_else(|_| json!({}));
        state["phase"] = json!(phase);
        self.as_mut().set_report_state(encoded(state));
    }
    fn set_package_report_state(mut self: Pin<&mut Self>, report: &PackageReport, loading: bool) {
        let sudo = self.rust().sudo;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let retried = self.rust().retrying_sources.clone();
        for source in report
            .successful_sources
            .iter()
            .filter(|source| retried.as_ref().is_none_or(|ids| ids.contains(source)))
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
                    "detail": plain_error(&failure.error, Some(&failure.backend), sudo)
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
        } else if report.successful_sources.is_empty() && report.packages.is_empty() {
            "failed"
        } else {
            "partial"
        };
        let last_success = self.rust().last_success.clone();
        let mut state = json!({
            "phase": phase, "failures": failures,
            "successful_sources": report.successful_sources,
            "last_success": last_success
        });
        if self.rust().active_view == "Search" && self.rust().search_matches > report.packages.len()
        {
            state["matches"] = json!(self.rust().search_matches);
        }
        // Only a finished read says when the section was checked.
        if !loading {
            state["checked_at"] = json!(now);
        }
        self.as_mut().set_report_state(encoded(state));
    }
    fn start(mut self: Pin<&mut Self>, job: Job) {
        self.as_mut().start_job(job);
        self.sync_needs_poll();
    }
    fn start_job(mut self: Pin<&mut Self>, job: Job) {
        if matches!(&job, Job::Load(view, _) if view == "Search") {
            self.as_mut().rust_mut().search_matches = 0;
        }
        // The quiet reload of the section on screen, behind its cached rows.
        let quiet = self.rust().refreshing
            && matches!(&job, Job::Load(view, _) if *view == self.rust().active_view);
        if let Some(worker) = &self.rust().worker {
            if self.rust().background || self.rust().refreshing {
                worker.cancel.cancel();
                // The cancelled read's replies are dropped. A request of
                // the user's replaces a quiet reload; the cached rows
                // stay on screen, marked stale.
                self.as_mut().rust_mut().background = true;
                if !quiet {
                    self.as_mut().set_refreshing(false);
                }
                self.as_mut().rust_mut().queued = Some(job);
                // A confirmed foreground request preempts background
                // loading: report busy at once so the UI waits for the
                // queued job instead of reading idle state while the
                // background worker winds down.
                if !quiet {
                    self.as_mut().set_busy(true);
                }
            }
            return;
        }
        if matches!(job, Job::OpenInput(_)) {
            self.as_mut().rust_mut().pending = None;
            self.as_mut().set_confirmation(QString::default());
            self.as_mut().set_confirmation_data("{}".into());
        }
        let source_filter = self.rust().source_filter.clone();
        let batch_mode = batch_mode(&job);
        // An automatic run never shows a password dialog: Linux uses the
        // polkit rule, macOS sudo -n, which works only under the sudoers
        // entry. Casks get no password dialog either, so without the entry
        // one that needs sudo fails at once instead of waiting.
        let authorization = if self.rust().sudo || batch_mode.is_some() && cfg!(target_os = "macos")
        {
            Authorization::SudoNonInteractive
        } else {
            Authorization::Polkit
        };
        if job.writes() {
            // A write changes what a preload would show; load it again after.
            let controller = self.as_mut().rust_mut();
            if let Some(worker) = &mut controller.prefetch_worker {
                worker.cancel.cancel();
                worker.stale = true;
            }
            controller.prefetch = prefetch_views();
        }
        let cancel = Cancellation::default();
        let (sender, receiver) = crate::wake::channel();
        // Update all runs one command per source; its output says which
        // package each one is on, so pass those lines to the progress.
        let followed: Vec<String> = job
            .operations()
            .into_iter()
            .filter_map(|operation| match operation {
                Operation::UpgradeAll { backend } => Some(backend),
                _ => None,
            })
            .collect();
        let token = if followed.is_empty() {
            cancel.clone()
        } else {
            cancel.with_output(output_follower(followed, sender.clone()))
        };
        let worker_job = job.clone();
        // Loads rebuild for fresh discovery; the previous engine is dropped.
        // Details can reuse a warm engine; mutation jobs always rediscover.
        // Searches reuse a recent engine with the same scope, so typing
        // does not detect every package manager again for each query.
        let mut cached = self.as_mut().rust_mut().engine.take();
        let scope = self.as_mut().rust_mut().engine_scope.take();
        let search_scope = engine_source(&job, &source_filter);
        let warm_search = matches!(&job, Job::Load(view, _) if view == "Search")
            && scope.as_ref().is_some_and(|(sources, sudo, born)| {
                *sources == search_scope && *sudo == self.rust().sudo && born.elapsed() < VIEW_TTL
            });
        self.as_mut().rust_mut().reused_engine_born =
            scope.filter(|_| warm_search).map(|(_, _, born)| born);
        let cleanup = self.rust().cleanup.clone();
        let natives = self.rust().natives;
        // Keep read snapshots while a native write owns the worker. The UI
        // can browse them, clearly marked stale, until the queue drains.
        let handle = thread::spawn(move || {
            let sources_view = matches!(&job, Job::Load(view, _) | Job::RetrySource(view, ..) if view == "Sources");
            let mut send = |mut reply| {
                match &mut reply {
                    // Rows are made here, off the window's thread.
                    Reply::Partial(report) | Reply::Done(Ok(Payload::Packages(report))) => {
                        for package in &mut report.packages {
                            crate::metadata::enrich_cached(package);
                        }
                        let _ = sender.send(Reply::Rows(PreparedRows::new(&report.packages)));
                    }
                    Reply::Done(Ok(
                        Payload::RetryPackages(_, report) | Payload::RetryFailedUpdates(_, report),
                    )) => {
                        for package in &mut report.packages {
                            crate::metadata::enrich_cached(package);
                        }
                    }
                    Reply::Done(Ok(Payload::Details(details))) => {
                        crate::metadata::enrich(&mut details.package, natives.metadata);
                        let _ = sender.send(Reply::DetailsPreview(details.clone()));
                        crate::metadata::details(&mut details.package, &token, natives.metadata)
                    }
                    _ => {}
                }
                let _ = sender.send(reply);
            };
            let root = std::path::Path::new(natives.root);
            if let Job::Repositories(action) = &job {
                let transport = pkgdeck_core::backends::NativeTransport {
                    host: (natives.host)(),
                    authorization,
                };
                let result = action
                    .as_ref()
                    .map(|action| repositories::apply(&transport, action, &token))
                    .transpose()
                    .map(|_| Payload::Repositories(repositories::list(&transport, root, &token)));
                send(Reply::Done(result));
                return;
            }
            if let Job::ImportRepository(import) = &job {
                let transport = pkgdeck_core::backends::NativeTransport {
                    host: (natives.host)(),
                    authorization,
                };
                let result = repository_input::apply(import, authorization, &token)
                    .map(|_| Payload::Repositories(repositories::list(&transport, root, &token)));
                send(Reply::Done(result));
                return;
            }
            if let Job::OpenInput(input) = &job {
                send(Reply::Done(inspect_open_input(input, &token)));
                return;
            }
            // A warm search always has the engine its scope describes.
            if let Some(mut engine) = cached.take_if(|_| warm_search) {
                execute(&mut engine, job, &token, &mut send);
                send(Reply::Engine(Box::new(engine)));
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
            match (natives.engine)(
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
                    if let Some(mode) = batch_mode {
                        engine.set_batch_mode(mode);
                    }
                    engine.set_unattended(matches!(job, Job::AutoUpgrade(..)));
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
        let progress = writing.then(|| {
            ProgressState::new(
                &worker_job,
                self.rust().active_activity_id,
                &self.rust().names_for(&worker_job.operations()),
            )
            .with_rows(&self.rust().packages)
        });
        self.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel,
            job: worker_job,
        });
        if let Some(progress) = progress {
            self.as_mut().set_progress(progress.snapshot());
            self.as_mut().rust_mut().progress_state = Some(progress);
        }
        let foreground = !self.rust().background && !quiet;
        self.as_mut().set_busy(foreground);
        self.set_writing(writing);
    }
    /// Remember the sources a visible load still waits for and show them.
    fn set_asked(mut self: Pin<&mut Self>, sources: Vec<String>) {
        self.as_mut().rust_mut().asked = sources;
        self.show_pending();
    }
    /// Show what the visible section still waits for: its preload's
    /// sources while it waits on one, else its own load's.
    fn show_pending(mut self: Pin<&mut Self>) {
        let rust = self.rust();
        let pending: Vec<&String> = if rust.awaiting_prefetch {
            rust.prefetch_worker
                .iter()
                .filter(|worker| !worker.stale && worker.view == rust.active_view)
                .flat_map(|worker| {
                    let answered = &worker.answered;
                    worker.asked.iter().filter(|id| !answered.contains(id))
                })
                .collect()
        } else {
            rust.asked.iter().collect()
        };
        let pending = encoded(pending);
        if self.pending_sources() != &pending {
            self.as_mut().set_pending_sources(pending);
        }
    }
    fn follow_output(mut self: Pin<&mut Self>, line: &str) {
        if let Some(state) = self.as_mut().rust_mut().progress_state.as_mut() {
            if state.observe(line) {
                let snapshot = state.snapshot();
                self.as_mut().set_progress(snapshot);
            }
        }
    }
    fn update_progress(mut self: Pin<&mut Self>, event: &Event) {
        if let Some(state) = self.as_mut().rust_mut().progress_state.as_mut() {
            state.apply(event);
            let snapshot = state.snapshot();
            self.as_mut().set_progress(snapshot);
        }
    }
    /// Show a view. Without force, rows loaded in the last minute publish
    /// before this returns and nothing reloads. Older rows also publish at
    /// once, and a quiet reload runs behind them with `refreshing` true and
    /// `busy` false.
    pub fn load(
        mut self: Pin<&mut Self>,
        view: QString,
        query: QString,
        sources: QString,
        sudo: bool,
        force: bool,
    ) {
        self.as_mut().load_view(view, query, sources, sudo, force);
        self.as_mut().rewarm_if_due();
        if self.rust().natives.warm_catalog {
            crate::metadata::warm(self.rust().natives.metadata);
        }
        self.sync_needs_poll();
    }
    fn load_view(
        mut self: Pin<&mut Self>,
        view: QString,
        query: QString,
        sources: QString,
        sudo: bool,
        force: bool,
    ) {
        let writing = self.rust().worker.as_ref().is_some_and(|w| w.job.writes());
        let view = view.to_string();
        // Only Search takes a query. Sections ignore any leftover search
        // text, so it can never split their cache or miss a preload.
        let query = if view == "Search" {
            query.to_string()
        } else {
            String::new()
        };
        // Comma-joined checked source ids; empty means every available source.
        let sources: Vec<String> = sources
            .to_string()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !["Search", "Installed", "Updates", "Sources", "Clean"].contains(&view.as_str()) {
            return;
        }
        const KNOWN: &[&str] = pkgdeck_core::backends::BACKEND_IDS;
        if sources.iter().any(|s| !KNOWN.contains(&s.as_str())) {
            self.set_status("Unknown source.".into());
            return;
        }
        // Sources lists every manager; opening it must not reset the filter
        // the other sections and their preloads use.
        let sources = if view == "Sources" {
            self.rust().source_filter.clone()
        } else {
            sources
        };
        // Reloading the section on screen keeps its rows until the new ones
        // are complete (see hold_partials). Reloading again while that
        // refresh runs finds no rows here, yet the page still shows them.
        let refreshing = force
            && view != "Search"
            && self.rust().active_view == view
            && (self.rust().rows.to_string() != "[]"
                || self.rust().hold_partials && self.rust().worker.is_some());
        let was_refreshing = self.rust().refreshing;
        self.as_mut().set_refreshing(false);
        self.as_mut().rust_mut().active_view = view.clone();
        self.as_mut().rust_mut().awaiting_prefetch = false;
        self.as_mut().show_pending();
        self.as_mut().rust_mut().hold_partials = refreshing;
        if view == "Search" && query.trim().is_empty() {
            // Seed the selected sources before the first query starts.
            if self.rust().worker.is_none() {
                self.as_mut().rust_mut().source_filter = sources;
                self.as_mut().rust_mut().sudo = sudo;
            }
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
            self.as_mut().rust_mut().invalidate_prefetch();
            if changed {
                self.as_mut().rust_mut().engine_scope = None;
            }
        }
        if !force {
            // A preloaded section shows at once. An old one still shows at
            // once and is refreshed below, like any stale snapshot.
            if let Some((loaded, payload)) = self.as_mut().rust_mut().prefetched.remove(&key) {
                self.as_mut().rust_mut().updates_view = view == "Updates";
                self.as_mut().rust_mut().queued = None;
                self.as_mut().rust_mut().packages.clear();
                self.as_mut().rust_mut().cleanup.clear();
                self.as_mut().rust_mut().sources.clear();
                self.as_mut().rust_mut().failures.clear();
                self.as_mut().apply(Ok(payload));
                let upgradable =
                    view == "Updates" && !upgrade_plan(&self.rust().packages).is_empty();
                self.as_mut().set_upgradable(upgradable);
                // The rows were read when the preload finished, not now.
                self.as_mut().set_checked_at(epoch_seconds_at(loaded));
                self.as_mut().stash_current_at(key.clone(), loaded);
            }
            if let Some(cached) = self.rust().view_cache.get(&key).cloned() {
                let stale = cached.stale();
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
                    .set_phase(if stale { "stale" } else { "cached" });
                self.as_mut().set_busy(false);
                // Asking again for the section already reloading behind
                // its snapshot keeps that reload.
                let same_reload = was_refreshing
                    && stale
                    && !self.rust().background
                    && self.rust().worker.as_ref().is_some_and(|worker| {
                        matches!(&worker.job, Job::Load(v, q) if *v == view && *q == query)
                    });
                if let Some(worker) = self.rust().worker.as_ref().filter(|_| !same_reload) {
                    worker.cancel.cancel();
                    // Suppress every reply from the previous section.
                    self.as_mut().rust_mut().background = true;
                }
                if stale {
                    // Keep the snapshot on screen until the whole section
                    // arrives; streamed partials would blank most of it.
                    // The reload is quiet: busy stays false and refreshing
                    // tells the page to show a small spinner.
                    self.as_mut().rust_mut().hold_partials = true;
                    self.as_mut().set_refreshing(true);
                    if same_reload {
                    } else if self.as_mut().rust_mut().awaits_prefetch(&key) {
                        self.as_mut().rust_mut().awaiting_prefetch = true;
                    } else {
                        self.as_mut().start(Job::Load(view, query));
                    }
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
            self.as_mut().rust_mut().view_cache.expire();
            self.as_mut().rust_mut().invalidate_prefetch();
            self.as_mut().rust_mut().engine_scope = None;
            self.as_mut().rust_mut().invalidate_details();
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
        // The section is already preloading: wait for that result instead
        // of querying every manager a second time.
        if !force && self.rust().awaits_prefetch(&key) {
            self.as_mut().rust_mut().awaiting_prefetch = true;
            if let Some(worker) = &self.rust().worker {
                worker.cancel.cancel();
                self.as_mut().rust_mut().background = true;
            }
            self.as_mut().set_busy(true);
            self.as_mut().show_pending();
            return;
        }
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
        if self.rust().foreground_worker() || self.rust().pending.is_some() {
            return;
        }
        let (view, source) = (view.to_string(), source.to_string());
        let query = if view == "Search" {
            query.to_string()
        } else {
            String::new()
        };
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
        self.stash_current_at(key, Instant::now());
    }
    fn stash_current_at(self: Pin<&mut Self>, key: String, loaded: Instant) {
        let view = CachedView {
            loaded,
            expired: false,
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
        let mut this = self;
        // Failures aren't kept; the reload behind the snapshot reports them.
        if ViewStore::keeps(&key) {
            if let Some(store) = this.as_mut().rust_mut().view_store.as_mut() {
                store.keep(StoredView {
                    key: key.clone(),
                    packages: view.packages.clone(),
                    cleanup: view.cleanup.clone(),
                    rows: view.rows.to_string(),
                    status: view.status.to_string(),
                    report_state: view.report_state.to_string(),
                    upgradable: view.upgradable,
                    updates_view: view.updates_view,
                });
            }
        }
        this.rust_mut().view_cache.insert(key, view);
    }
    /// Look up the details of the row at `index` in the background, as the
    /// pointer rests on it, so opening it a moment later is instant. Never
    /// replaces the lookup for the open package or delays other work.
    pub fn warm_details(mut self: Pin<&mut Self>, index: i32) {
        let Some(package) = usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().packages.get(i))
            .cloned()
        else {
            return;
        };
        let busy = self.rust().details_worker.as_ref().is_some_and(|worker| {
            worker.id == package.id || self.rust().warming.as_ref() != Some(&worker.id)
        });
        if busy
            || self.rust().detail_cache.contains_key(&package.id)
            || self.rust().selected.as_ref() == Some(&package.id)
            || self.rust().worker.as_ref().is_some_and(|w| w.job.writes())
        {
            return;
        }
        self.as_mut().rust_mut().warming = Some(package.id.clone());
        self.start_details(package.id);
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
            // The open package is already loading, perhaps since the pointer
            // rested on its row. Arrowing onto it again must not start a
            // second native query.
            if self
                .rust()
                .details_worker
                .as_ref()
                .is_some_and(|worker| worker.id == package.id)
            {
                if self.rust().warming.as_ref() == Some(&package.id) {
                    self.as_mut().rust_mut().warming = None;
                    let same = same_app_sources(&self.rust().packages, &package.id);
                    self.as_mut().set_details(encoded(json!({"package": package_row(&package, &same, None), "description": package.summary, "more": true})));
                }
                return;
            }
            self.as_mut().rust_mut().warming = None;
            // A running Details job is stale the moment the selection moves:
            // cancel it and queue the new identity. A selection that lands
            // while rows stream in queues behind the load instead. Writes
            // are never preempted; selections made mid-write highlight
            // without loading details until the write finishes.
            if let Some(worker) = &self.rust().worker {
                if let Job::Details(id) = &worker.job {
                    if *id == package.id {
                        return;
                    }
                    worker.cancel.cancel();
                    self.as_mut().rust_mut().queued = Some(Job::Details(package.id.clone()));
                    return;
                }
                if matches!(worker.job, Job::Load(..)) {
                    // Rows are still streaming or refreshing: load details
                    // alongside instead of stopping that read.
                    let same = same_app_sources(&self.rust().packages, &package.id);
                    self.as_mut().set_details(encoded(json!({"package": package_row(&package, &same, None), "description": package.summary, "more": true})));
                    self.start_details(package.id);
                    return;
                }
                // A write owns the worker: nothing more loads until it ends.
                let same = same_app_sources(&self.rust().packages, &package.id);
                self.as_mut().set_details(encoded(json!({"package": package_row(&package, &same, None), "description": package.summary})));
                return;
            }
            let same = same_app_sources(&self.rust().packages, &package.id);
            // What the row already says, until its details arrive.
            self.as_mut().set_details(encoded(
                json!({"package": package_row(&package, &same, None), "description": package.summary, "more": true}),
            ));
            // Details load on their own worker, so opening a package never
            // makes the page busy or disables its actions.
            self.start_details(package.id);
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
            let sudo = self.rust().sudo;
            self.as_mut().set_details(failure_details(&failure, sudo));
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
        if !self.rust().foreground_worker() {
            self.start(Job::Repositories(None));
        }
    }
    pub fn export_inventory(self: Pin<&mut Self>, url: QUrl, identities: QString) {
        if self.rust().foreground_worker() {
            return;
        }
        let Some(path) = url
            .to_local_file()
            .map(|path| PathBuf::from(path.to_string()))
        else {
            self.set_status("Choose a local inventory file.".into());
            return;
        };
        let selected: Vec<PackageId> = match serde_json::from_str(&identities.to_string()) {
            Ok(selected) => selected,
            Err(_) => {
                self.set_status("Invalid package selection.".into());
                return;
            }
        };
        self.start(Job::ManifestExport(path, selected));
    }
    pub fn preview_inventory(mut self: Pin<&mut Self>, url: QUrl) {
        if self.rust().foreground_worker() {
            return;
        }
        let Some(path) = url
            .to_local_file()
            .map(|path| PathBuf::from(path.to_string()))
        else {
            self.set_status("Choose a local inventory file.".into());
            return;
        };
        self.as_mut().set_manifest_preview("{}".into());
        self.start(Job::ManifestPreview(path));
    }
    pub fn change_repository(mut self: Pin<&mut Self>, action: QString) {
        let action = match repositories::parse_action(&action.to_string()) {
            Ok(action) => action,
            Err(error) => {
                self.set_status(
                    format!(
                        "This repository change isn't valid: {}",
                        sentence_tail(&plain_text(&error.to_string(), None))
                    )
                    .as_str()
                    .into(),
                );
                return;
            }
        };
        if let Err(error) = action.validate() {
            let sudo = self.rust().sudo;
            self.set_status(plain_error(&error, None, sudo).as_str().into());
            return;
        }
        if matches!(action.change, repositories::Change::OpenEditor) {
            self.start(Job::Repositories(Some(action)));
            return;
        }
        self.as_mut().set_confirmation_data(encoded(
            json!({"action":"Apply", "body": repository_label(&action)}),
        ));
        self.as_mut().set_confirmation(
            format!(
                "{}\n\nApply this repository change?",
                repository_label(&action)
            )
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
            let operations = upgrade_plan(&self.rust().packages);
            if !self.rust().updates_view || operations.is_empty() {
                return;
            }
            let all = operations.len();
            let operations = without_failed(
                operations,
                &self.rust().packages,
                &self.rust().failed_updates,
            );
            let skipped = all - operations.len();
            let mut failed_sources: Vec<_> = self
                .rust()
                .failures
                .iter()
                .filter(|failure| read_failed(failure))
                .map(|failure| failure.backend.clone())
                .collect();
            failed_sources.sort();
            failed_sources.dedup();
            if !failed_sources.is_empty() && !writing {
                self.as_mut()
                    .set_status("Retrying failed source checks before updating.".into());
                self.start(Job::RetryFailedUpdates(failed_sources));
                return;
            }
            let count = self
                .rust()
                .packages
                .iter()
                .filter(|package| {
                    package.installed_version.is_some()
                        && package.update == UpdateAvailability::Available
                })
                .count()
                - skipped;
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
                .map(|item| {
                    format!(
                        "{} ({})\n{}",
                        item.title,
                        source_display_name(&item.id.backend),
                        item.preview
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let count = operations.len();
            let noun = if count == 1 { "task" } else { "tasks" };
            self.as_mut().set_confirmation_data(encoded(json!({"action":format!("Clean {count} {noun}"), "body": format!("{labels}\n\nTasks run in order. Finished tasks cannot be undone."), "summary": format!("Clean {count} {noun}\nFinished tasks cannot be undone."), "details": labels})));
            self.as_mut().set_confirmation(format!("Run {count} cleanup {noun}?\n\n{labels}\n\nTasks run in order. Finished tasks cannot be undone.").as_str().into());
            self.rust_mut().pending = Some(Job::CleanAll(operations));
            return;
        }
        if action == "adopt-all" {
            // Every AppImage installed some other way that PkgDeck can move in.
            let apps: Vec<_> = self
                .rust()
                .packages
                .iter()
                .filter(|p| {
                    p.id.backend == "appimage"
                        && p.adopt_with.as_deref() == Some("appimage")
                        && p.installed_version.is_some()
                })
                .cloned()
                .collect();
            if apps.is_empty() {
                return;
            }
            self.start(Job::PlanAdoptAll(apps));
            return;
        }
        if action == "adopt" {
            // A macOS app a curated cask can take over. The cask's exact
            // identity and the adoption check come from Homebrew itself.
            let app = usize::try_from(index)
                .ok()
                .and_then(|i| self.rust().packages.get(i))
                // Only the macOS Applications inventory and AppImages
                // installed some other way produce such rows.
                .filter(|p| {
                    matches!(p.id.backend.as_str(), "macos-apps" | "appimage")
                        && p.adopt_with.is_some()
                })
                .cloned();
            match app {
                Some(app) => self.start(Job::PlanAdoption(Box::new(app))),
                None => {
                    self.as_mut().set_confirmation(QString::default());
                    self.as_mut().set_confirmation_data("{}".into());
                    self.rust_mut().pending = None;
                }
            }
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
                let catalog = self.source_catalog().to_string();
                self.rust()
                    .packages
                    .get(i)
                    // Inventory rows are never installed or updated here.
                    .filter(|p| {
                        action == "remove" || !pkgdeck_core::backends::inventory_only(&p.id.backend)
                    })
                    .and_then(|p| match action.as_str() {
                        "install"
                            if !pkgdeck_core::backends::never_installs(&p.id.backend)
                                && p.installed_version.is_none() =>
                        {
                            Some(Operation::Install(p.id.clone()))
                        }
                        "remove"
                            if p.installed_version.is_some()
                                && removable(&catalog, &p.id.backend) =>
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
                    && self.rust().worker.is_none()
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
        let plan = plan_checked_upgrade(
            &self.rust().packages,
            &identities.to_string(),
            &self.rust().failed_updates,
        );
        if plan.operations.is_empty() {
            self.as_mut().rust_mut().pending = None;
            self.as_mut().set_confirmation(QString::default());
            self.as_mut().set_confirmation_data("{}".into());
        } else {
            let count = plan.operations.len();
            let noun = if count == 1 { "package" } else { "packages" };
            let warning = if count > 1 {
                "\nIf one update fails, the others can still finish."
            } else {
                ""
            };
            self.as_mut().set_confirmation_data(encoded(
                json!({"action":"Update", "body": plan.confirmation, "summary": format!("Update {count} selected {noun}{warning}"), "details": plan.details}),
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
        let (owner, more) = std::mem::take(&mut self.as_mut().rust_mut().pending_more);
        // Only the review that listed them may run them.
        let more = if pending
            .as_ref()
            .is_some_and(|job| job.operations() == owner)
        {
            more
        } else {
            vec![]
        };
        self.as_mut().set_confirmation(QString::default());
        self.as_mut().set_confirmation_data("{}".into());
        if approved {
            if let Some(op) = pending {
                self.as_mut().accept_confirmed(op);
            }
            // The rest of one review that covers several changes.
            for job in more {
                self.as_mut().accept_confirmed(job);
            }
        }
        // Declining lets held-back preloads run.
        self.sync_needs_poll();
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
        // The details lookup runs beside the list load. Drop it here so a
        // reply already in its channel cannot refill the cache or replace
        // the cancellation status.
        let details = self.as_mut().rust_mut().details_worker.take();
        if let Some(worker) = &details {
            worker.cancel.cancel();
        }
        if self.rust().worker.is_some() || details.is_some() {
            if let Some(worker) = &self.rust().worker {
                worker.cancel.cancel();
            }
            self.as_mut().set_asked(vec![]);
            self.as_mut()
                .set_status("Cancelling… Waiting for the package manager to finish safely.".into());
        }
        self.sync_needs_poll();
    }
    fn apply(mut self: Pin<&mut Self>, result: Result<Payload, EngineError>) {
        // A newer view or query owns the next visible report. A cancelled
        // worker can still race one final partial into the channel, so none
        // of its successful payloads may replace the new query's empty state.
        // Details still warm the cache: the lookup already ran, and choosing
        // that package again must not start another one.
        if matches!(self.rust().queued, Some(Job::Load(..)))
            && result.is_ok()
            && !matches!(result, Ok(Payload::Details(_)))
        {
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
                    let sudo = self.rust().sudo;
                    self.set_status(plain_error(&e, None, sudo).as_str().into());
                }
            }
            // An opened file or link gets its own app page; Install there
            // previews the change.
            Ok(Payload::OpenPackage(details)) => {
                let package = details.package.clone();
                {
                    let rust = self.as_mut().rust_mut();
                    rust.packages.retain(|row| row.id != package.id);
                    rust.packages.push(package.clone());
                    rust.opened_package = Some(package.clone());
                }
                let action = if package.adopt_with.as_deref() == Some("appimage") {
                    "Manage"
                } else {
                    "Install"
                };
                let data = opened_page(&details, action);
                self.as_mut().set_opened(encoded(data));
            }
            Ok(Payload::OpenRepository(import)) => {
                let action = if import.suffix == "ymp" {
                    "Open native installer".to_owned()
                } else {
                    format!("Add {}", import.name)
                };
                let scope = if import.backend == "flatpak" {
                    "User"
                } else {
                    "System"
                };
                let summary = format!(
                    "{action}\n{}, {scope}",
                    source_display_name(&import.backend)
                );
                let details = format!("{}\n{}", import.description, import.source);
                self.as_mut().set_confirmation_data(encoded(json!({"action": action, "body": summary, "summary": summary, "details": details})));
                self.as_mut().set_confirmation(summary.as_str().into());
                self.rust_mut().pending = Some(Job::ImportRepository(import));
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
                // With removals turned off in Settings, APT is left out when
                // its plan removes anything.
                let apt_all = |op: &Operation| matches!(op, Operation::UpgradeAll { backend } if backend == "apt");
                let (operations, apt_plan, count, left_out) = match apt_plan {
                    Some(plan) if !self.rust().allow_removals && !plan.removals.is_empty() => {
                        let apt_packages = self
                            .rust()
                            .packages
                            .iter()
                            .filter(|p| {
                                p.id.backend == "apt" && p.update == UpdateAvailability::Available
                            })
                            .count();
                        let note = format!(
                            "\nAPT is left out: it would remove {}. Turn on \"Allow updates that remove packages\" in Settings to include it.",
                            plan.removals.join(", ")
                        );
                        let rest: Vec<_> =
                            operations.into_iter().filter(|op| !apt_all(op)).collect();
                        (rest, None, count.saturating_sub(apt_packages), note)
                    }
                    plan => (operations, plan, count, String::new()),
                };
                if operations.is_empty() {
                    self.as_mut().set_status(left_out.trim().into());
                    return;
                }
                let labels = operations
                    .iter()
                    .map(|operation| confirmation_label(operation, &self.rust().packages))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let apt = apt_plan.as_ref().map_or_else(String::new, |plan| {
                    format!("\n\nAPT changes:\n{}", plan.summary())
                });
                let removals = apt_plan.as_ref().map_or_else(String::new, |plan| {
                    if plan.removals.is_empty() {
                        String::new()
                    } else {
                        format!("\nRemoves: {}", plan.removals.join(", "))
                    }
                });
                let noun = if count == 1 { "package" } else { "packages" };
                let warning = if count > 1 {
                    "\nIf one update fails, the others can still finish."
                } else {
                    ""
                };
                let failed_sources = self
                    .rust()
                    .failures
                    .iter()
                    .filter(|failure| read_failed(failure))
                    .count();
                let incomplete = if failed_sources == 0 {
                    String::new()
                } else {
                    let (noun, them) = if failed_sources == 1 {
                        ("source", "it")
                    } else {
                        ("sources", "them")
                    };
                    format!("\n{failed_sources} {noun} could not be checked. Updates from {them} are not included.")
                };
                self.as_mut().set_confirmation_data(encoded(json!({"action":"Update", "body": format!("{count} listed {noun}{incomplete}{left_out}{apt}\n\n{labels}"), "summary": format!("Update {count} {noun}{removals}{warning}{incomplete}{left_out}"), "details": format!("{incomplete}{left_out}{apt}\n\n{labels}")})));
                self.as_mut().set_confirmation(
                    format!("Update all {count} listed {noun}?{incomplete}{left_out}{apt}\n\n{labels}\n\nContinue?")
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
            Ok(Payload::AdoptionPreview(cask, operation, plan)) => {
                let mut data = confirmation_preview(
                    &operation,
                    std::slice::from_ref(&*cask),
                    &[],
                    Some(&plan),
                );
                // Say what happens in people's words: Homebrew takes over the
                // checked copy where it is; PkgDeck moves an AppImage in.
                let app = self
                    .rust()
                    .packages
                    .iter()
                    .find(|row| {
                        matches!(row.id.backend.as_str(), "macos-apps" | "appimage")
                            && plan.adopts.as_deref() == Some(std::path::Path::new(&row.id.name))
                    })
                    .map(|row| row.display_name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| cask.display_name.clone());
                let manager = if cask.id.backend == "appimage" {
                    "PkgDeck"
                } else {
                    "Homebrew"
                };
                data["action"] = json!(format!("Manage with {manager}"));
                data["notes"] = json!([plan.native_preview]);
                data["body"] = json!(format!(
                    "{}\n\n{}",
                    plan.native_preview,
                    data["body"].as_str().unwrap_or_default()
                ));
                data["summary"] = json!(format!(
                    "Manage {app} with {manager}\n{}\n{}",
                    source_display_name(&cask.id.backend),
                    plan.native_preview
                ));
                let body = data["body"].as_str().unwrap_or_default().to_owned();
                self.as_mut().rust_mut().names.insert(cask.id.clone(), app);
                self.as_mut().set_confirmation_data(encoded(data));
                self.as_mut().set_confirmation(body.as_str().into());
                self.rust_mut().pending = Some(Job::Write(operation, Some(plan)));
            }
            Ok(Payload::AdoptAllPreview(mut plans, skipped)) => {
                if plans.len() == 1 && skipped.is_empty() {
                    let (package, operation, plan) = plans.remove(0);
                    return self.apply(Ok(Payload::AdoptionPreview(
                        Box::new(package),
                        operation,
                        plan,
                    )));
                }
                if plans.is_empty() {
                    self.set_status(
                        "No AppImage installed some other way can be moved in now.".into(),
                    );
                    return;
                }
                let count = plans.len();
                let moves: Vec<String> = plans
                    .iter()
                    .map(|(package, _, plan)| {
                        let version = plan
                            .changes
                            .first()
                            .and_then(|change| change.candidate_version.as_deref())
                            .map(|version| format!(" {version}"))
                            .unwrap_or_default();
                        format!("Move {}{version} into PkgDeck", package.display_name)
                    })
                    .collect();
                let mut notes = vec![
                    "Moves them into PkgDeck's folder and replaces their menu entries.".to_owned(),
                ];
                if !skipped.is_empty() {
                    notes.push(format!("Left as they are: {}", skipped.join(", ")));
                }
                let noun = if count == 1 { "AppImage" } else { "AppImages" };
                let summary = format!("Manage {count} {noun} with PkgDeck\n{}", notes.join("\n"));
                let body = format!("{}\n\n{}", notes.join("\n"), moves.join("\n"));
                for (package, operation, _) in &plans {
                    let name = package.display_name.clone();
                    let names = &mut self.as_mut().rust_mut().names;
                    if let Operation::Install(id) = operation {
                        names.insert(id.clone(), name.clone());
                    }
                    names.insert(package.id.clone(), name);
                }
                let mut writes = plans
                    .into_iter()
                    .map(|(_, operation, plan)| Job::Write(operation, Some(plan)));
                let first = writes.next();
                let owner = first.as_ref().map(Job::operations).unwrap_or_default();
                self.as_mut().rust_mut().pending_more = (owner, writes.collect());
                self.as_mut().set_confirmation_data(encoded(json!({
                    "action": "Manage all", "review": true, "summary": summary,
                    "body": body, "notes": notes, "changes": moves, "details": moves.join("\n")
                })));
                self.as_mut().set_confirmation(body.as_str().into());
                self.rust_mut().pending = first;
            }
            Ok(Payload::ManifestExport(count)) => {
                self.set_status(format!("Exported {count} packages.").as_str().into());
            }
            Ok(Payload::ManifestPreview(preview)) => {
                self.as_mut().set_manifest_preview(encoded(preview));
                self.set_status("Inventory preview ready.".into());
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
                            format!(
                                "{} ({})\n{}",
                                item.title,
                                source_display_name(&item.id.backend),
                                item.preview
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let noun = if operations.len() == 1 {
                        "task"
                    } else {
                        "tasks"
                    };
                    self.as_mut().set_confirmation_data(encoded(json!({"action": format!("Clean {} {noun}", operations.len()), "body": body, "summary": format!("Clean {} {noun}\nFinished tasks cannot be undone.", operations.len()), "details": body})));
                    self.as_mut().set_confirmation(body.as_str().into());
                    self.as_mut().rust_mut().pending = Some(Job::CleanAll(operations));
                }
            }
            Ok(Payload::RetryPackages(source, retry)) => {
                self.as_mut().rust_mut().view_cache.expire();
                self.as_mut().rust_mut().invalidate_prefetch();
                let report = merge_retried_packages(
                    &self.rust().packages,
                    &self.rust().failures,
                    self.successful_sources(),
                    std::slice::from_ref(&source),
                    retry,
                );
                self.as_mut().rust_mut().retrying_sources = Some(vec![source]);
                self.as_mut().apply(Ok(Payload::Packages(report)));
                self.as_mut().rust_mut().retrying_sources = None;
            }
            Ok(Payload::RetryFailedUpdates(sources, retry)) => {
                self.as_mut().rust_mut().view_cache.expire();
                self.as_mut().rust_mut().invalidate_prefetch();
                let report = merge_retried_packages(
                    &self.rust().packages,
                    &self.rust().failures,
                    self.successful_sources(),
                    &sources,
                    retry,
                );
                self.as_mut().rust_mut().retrying_sources = Some(sources);
                self.as_mut().apply(Ok(Payload::Packages(report)));
                self.as_mut().rust_mut().retrying_sources = None;
            }
            Ok(Payload::RetryCleanup(source, retry)) => {
                self.as_mut().rust_mut().view_cache.expire();
                self.as_mut().rust_mut().invalidate_prefetch();
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
                self.as_mut().rust_mut().view_cache.expire();
                self.as_mut().rust_mut().invalidate_prefetch();
                let mut sources = self.rust().sources.clone();
                sources.retain(|row| row.backend != source);
                sources.extend(retry);
                sources.sort_by(|a, b| a.backend.cmp(&b.backend));
                self.as_mut().apply(Ok(Payload::Sources(sources)));
            }
            Ok(Payload::Packages(report)) => {
                let sudo = self.rust().sudo;
                // Only the terminal report enables the upgrade action.
                // Partial rows and failures can still change while reading.
                if self.rust().worker.is_none() {
                    let upgradable =
                        self.rust().updates_view && !upgrade_plan(&report.packages).is_empty();
                    self.as_mut().set_upgradable(upgradable);
                }
                let loading = self.rust().worker.is_some();
                self.as_mut().set_package_report_state(&report, loading);
                let failure_rows: Vec<_> = report.failures.iter().map(|failure| {
                    json!({"kind": "failure", "name": failure.backend, "source": failure.backend,
                        "display_name": source_display_name(&failure.backend),
                        "summary": plain_error(&failure.error, Some(&failure.backend), sudo), "failure_kind": failure_kind(&failure.error), "available": false})
                }).collect();
                let rows = append_rows(
                    self.rust().prepared_rows.json(&report.packages),
                    &failure_rows,
                );
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
                            .map(|f| format!(
                                "{}: {}",
                                source_display_name(&f.backend),
                                plain_error(&f.error, Some(&f.backend), sudo)
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                self.as_mut().rust_mut().packages = report.packages;
                self.as_mut().rust_mut().failures = report.failures;
                self.as_mut().set_rows(rows.as_str().into());
                self.as_mut().set_status(status.as_str().into());
                self.as_mut().publish_held_updates();
                if self.rust().updates_view && self.rust().worker.is_none() {
                    self.clear_settled_failure();
                }
            }
            Ok(Payload::Cleanup(report)) => {
                let sudo = self.rust().sudo;
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
                        "display_name": source_display_name(&failure.backend),
                        "summary": plain_error(&failure.error, Some(&failure.backend), sudo),
                        "available": false
                    })
                }));
                let incomplete = !report.failures.is_empty();
                let failures: Vec<_> = report.failures.iter().map(|failure| json!({"source": failure.backend, "kind": failure_kind(&failure.error), "detail": plain_error(&failure.error, Some(&failure.backend), sudo)})).collect();
                let last_success = self.rust().last_success.clone();
                self.as_mut().set_report_state(encoded(json!({"phase": if incomplete { if empty { "failed" } else { "partial" } } else { "complete" }, "failures": failures, "last_success": last_success, "checked_at": epoch_seconds()})));
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
                self.as_mut().rust_mut().view_cache.expire();
                self.as_mut().rust_mut().invalidate_prefetch();
                self.as_mut().rust_mut().invalidate_details();
                crate::metadata::invalidate();
                self.as_mut().set_repositories(repositories_json(&report));
                self.set_status("Repositories loaded.".into());
            }
            Ok(Payload::Sources(sources)) => {
                self.as_mut().rust_mut().catalog_checked = true;
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
                self.as_mut().set_report_state(encoded(json!({
                    "phase": phase, "failures": failures, "last_success": last_success,
                    "checked_at": epoch_seconds()
                })));
                self.as_mut().set_source_catalog(encoded(&rows));
                self.as_mut().rust_mut().sources = sources;
                self.as_mut().set_rows(encoded(rows));
                self.set_status("Source availability checked. Select a source for details.".into());
            }
            Ok(Payload::Details(details)) => {
                // A queued load is about to replace the rows. Keep the lookup
                // in the cache, and leave the panel until that load lands.
                let superseded = matches!(self.rust().queued, Some(Job::Load(..)));
                // Provider metadata may improve a name after opening details.
                // Preserve inventory state and only update presentation fields.
                let rows = self.rows().to_string();
                if !superseded {
                    if let Some(rows) = update_detail_name(
                        &mut self.as_mut().rust_mut().packages,
                        &rows,
                        &details.package,
                    ) {
                        self.as_mut().set_rows(encoded(rows));
                        // Patch the name in cached sections instead of dropping
                        // them, so opening details never makes sections reload.
                        for (_, cached) in &mut self.as_mut().rust_mut().view_cache.entries {
                            if let Some(rows) = update_detail_name(
                                &mut cached.packages,
                                &cached.rows.to_string(),
                                &details.package,
                            ) {
                                cached.rows = encoded(rows);
                            }
                        }
                    }
                }
                let info = crate::metadata::cached_info(&details.package);
                // A first look whose provider is still being asked online.
                let more =
                    self.rust().details_preview && crate::metadata::will_fetch(&details.package);
                let data = encoded(
                    json!({"package": package_row(&details.package, &same_app_sources(&self.rust().packages, &details.package.id), None), "description": info.as_ref().filter(|i| !i.description.is_empty()).map(|i| &i.description).unwrap_or(&details.description), "homepage": details.homepage.as_ref().or_else(|| info.as_ref().and_then(|i| i.homepage.as_ref())), "publisher": info.as_ref().and_then(|i| i.publisher.as_ref()), "license": info.as_ref().and_then(|i| i.license.as_ref()), "dependencies": details.dependencies, "screenshots": info.as_ref().map(|i| &i.screenshots), "more": more}),
                );
                // Bound memory use for large searches; reload and writes invalidate this snapshot.
                if self.rust().detail_cache.len() >= 128 {
                    self.as_mut().rust_mut().detail_cache.clear();
                }
                // Only complete details are reused when the row opens again.
                if !more {
                    self.as_mut()
                        .rust_mut()
                        .detail_cache
                        .insert(details.package.id.clone(), data.clone());
                }
                // A superseded reply still warms the cache, but only the
                // current selection may take over the details panel.
                if !superseded && details_fresh(self.rust().selected.as_ref(), &details) {
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
                self.as_mut().rust_mut().invalidate_prefetch();
                self.as_mut().rust_mut().engine_scope = None;
                self.as_mut().rust_mut().invalidate_details();
                self.as_mut().set_phase("stale");
                self.set_status(
                    if outcome.cancellation_deferred {
                        "Finished before it could be cancelled. Changes were kept."
                    } else {
                        "Done. Refreshing the package list."
                    }
                    .into(),
                );
            }
        }
    }
    /// An inventory warms Installed. It warms Updates only when it was read
    /// after refreshing update indexes; otherwise Updates would show the
    /// stale Homebrew tap and skip its own `brew update`. A refreshed read
    /// with failures may carry a failed fetch, which a plain Installed read
    /// would not hit, so Installed loads on its own then.
    fn warm_inventory(mut self: Pin<&mut Self>, mut report: PackageReport, refreshed: bool) {
        let sources = self.rust().source_filter.clone();
        let sudo = self.rust().sudo;
        if !refreshed || report.failures.is_empty() {
            let key = cache_key("Installed", "", &sources, sudo);
            self.as_mut()
                .rust_mut()
                .prefetched
                .insert(key, (Instant::now(), Payload::Packages(report.clone())));
        }
        if !refreshed {
            return;
        }
        report
            .packages
            .retain(|p| p.update == UpdateAvailability::Available);
        let key = cache_key("Updates", "", &sources, sudo);
        self.as_mut()
            .rust_mut()
            .prefetched
            .insert(key, (Instant::now(), Payload::Packages(report)));
    }
    pub fn check_sources(mut self: Pin<&mut Self>) {
        let engine = self.rust().natives.engine;
        self.as_mut().begin_catalog_check(move |token| {
            engine(&[], true, Authorization::Polkit, token)
                .map(|mut engine| engine.discover(token))
                .unwrap_or_default()
        });
    }
    fn begin_catalog_check(
        mut self: Pin<&mut Self>,
        discover: impl FnOnce(&Cancellation) -> Vec<Source> + Send + 'static,
    ) {
        if self.rust().catalog_checked || self.rust().catalog_worker.is_some() {
            return;
        }
        // Source availability uses a separate worker only when requested, so
        // startup discovery cannot slow the first search.
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (sender, receiver) = crate::wake::channel();
        let handle = thread::spawn(move || {
            let _ = sender.send(discover(&token));
        });
        self.as_mut().rust_mut().catalog_worker = Some(CatalogWorker {
            handle,
            receiver,
            cancel,
        });
        self.sync_needs_poll();
    }
    fn start_details(mut self: Pin<&mut Self>, id: PackageId) {
        if let Some(worker) = self.as_mut().rust_mut().details_worker.take() {
            // Its reply is for an older selection; let it finish unread.
            worker.cancel.cancel();
        }
        let authorization = if self.rust().sudo {
            Authorization::SudoNonInteractive
        } else {
            Authorization::Polkit
        };
        let job = Job::Details(id.clone());
        let scope = engine_source(&job, &self.rust().source_filter);
        let natives = self.rust().natives;
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (sender, receiver) = crate::wake::channel();
        let handle = thread::spawn(move || {
            let mut send = |mut reply| {
                if let Reply::Done(Ok(Payload::Details(details))) = &mut reply {
                    crate::metadata::enrich(&mut details.package, natives.metadata);
                    let _ = sender.send(Reply::DetailsPreview(details.clone()));
                    crate::metadata::details(&mut details.package, &token, natives.metadata);
                }
                if matches!(reply, Reply::Done(_) | Reply::DetailsPreview(_)) {
                    let _ = sender.send(reply);
                }
            };
            match (natives.engine)(&scope, false, authorization, &token) {
                Ok(mut engine) => execute(&mut engine, job, &token, &mut send),
                Err(error) => send(Reply::Done(Err(error))),
            }
        });
        self.as_mut().rust_mut().details_worker = Some(DetailsWorker {
            handle,
            receiver,
            cancel,
            id,
        });
        self.sync_needs_poll();
    }
    fn poll_details(mut self: Pin<&mut Self>) {
        let Some(worker) = &self.rust().details_worker else {
            return;
        };
        let id = worker.id.clone();
        let finished = worker.handle.is_finished();
        let mut replies: Vec<_> = worker.receiver.try_iter().collect();
        if finished {
            let worker = self.as_mut().rust_mut().details_worker.take().unwrap();
            let _ = worker.handle.join();
            replies.extend(worker.receiver.try_iter());
        }
        for reply in replies {
            // Only the current selection may fill the details pane (see
            // apply). A failure replaces the row preview with the reason,
            // unless the user already cancelled or a newer load is waiting.
            match reply {
                Reply::DetailsPreview(details) => {
                    self.as_mut().rust_mut().details_preview = true;
                    self.as_mut().apply(Ok(Payload::Details(details)));
                    self.as_mut().rust_mut().details_preview = false;
                }
                Reply::Done(Ok(Payload::Details(details))) => {
                    self.as_mut().apply(Ok(Payload::Details(details)));
                }
                Reply::Done(Err(error)) => self.as_mut().show_details_error(&id, &error),
                _ => {}
            }
        }
    }
    /// The parallel details lookup failed. Keep the open package selected
    /// and say why the rest of its details are missing. Do not cache the
    /// failure: choosing the package again tries the lookup once more.
    fn show_details_error(mut self: Pin<&mut Self>, id: &PackageId, error: &EngineError) {
        if failure_kind(error) == "cancelled"
            || matches!(self.rust().queued, Some(Job::Load(..)))
            || self.rust().selected.as_ref() != Some(id)
        {
            return;
        }
        let Some(package) = self
            .rust()
            .packages
            .iter()
            .find(|package| package.id == *id)
            .cloned()
        else {
            return;
        };
        let message = write_error_text(error, Some(&id.backend), self.rust().sudo);
        let same = same_app_sources(&self.rust().packages, id);
        self.as_mut().set_details(encoded(json!({
            "package": package_row(&package, &same, None),
            "description": format!("Details couldn't be loaded. {message}"),
        })));
        self.set_status(message.as_str().into());
    }
    fn start_prefetch(mut self: Pin<&mut Self>, view: String, key: String) {
        let sources = self.rust().source_filter.clone();
        let authorization = if self.rust().sudo {
            Authorization::SudoNonInteractive
        } else {
            Authorization::Polkit
        };
        let job = Job::Load(view.clone(), String::new());
        let scope = engine_source(&job, &sources);
        let scope_for_worker = scope.clone();
        let sources_view = view == "Sources";
        let natives = self.rust().natives;
        let sudo = self.rust().sudo;
        // Installed, Updates and Clean read the same sources: the engine the
        // last preload (or search) checked is reused for a minute, instead
        // of checking every package manager again for each section.
        let warm = !sources_view
            && self
                .rust()
                .engine_scope
                .as_ref()
                .is_some_and(|(sources, was_sudo, born)| {
                    *sources == scope && *was_sudo == sudo && born.elapsed() < VIEW_TTL
                });
        let (cached, born) = if warm {
            let born = self.as_mut().rust_mut().engine_scope.take().map(|s| s.2);
            (self.as_mut().rust_mut().engine.take(), born)
        } else {
            (None, None)
        };
        let born = born.unwrap_or_else(Instant::now);
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (sender, receiver) = crate::wake::channel();
        let handle = thread::spawn(move || {
            let mut send = |mut reply| {
                if let Reply::Done(Ok(Payload::Packages(report))) = &mut reply {
                    for package in &mut report.packages {
                        crate::metadata::enrich_cached(package);
                    }
                    let _ = sender.send(Reply::Rows(PreparedRows::new(&report.packages)));
                }
                // Rows stream only for a visible load; a section waiting
                // on this preload only needs who answered.
                let reply = match reply {
                    Reply::Partial(report) => Reply::Answered(answered(&report)),
                    reply => reply,
                };
                let _ = sender.send(reply);
            };
            let engine = match cached {
                Some(engine) => Ok(engine),
                None => (natives.engine)(&scope_for_worker, sources_view, authorization, &token),
            };
            match engine {
                Ok(mut engine) => {
                    execute(&mut engine, job, &token, &mut send);
                    // Sources checks every manager, even unused ones.
                    if !sources_view {
                        send(Reply::Engine(Box::new(engine)));
                    }
                }
                Err(error) => send(Reply::Done(Err(error))),
            }
        });
        self.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
            handle,
            receiver,
            cancel,
            view,
            key,
            stale: false,
            asked: vec![],
            answered: vec![],
            scope: (scope, sudo, born),
        });
        self.sync_needs_poll();
    }
    fn poll_prefetch(mut self: Pin<&mut Self>) {
        if let Some(worker) = &self.rust().prefetch_worker {
            let finished = worker.handle.is_finished();
            let replies: Vec<_> = worker.receiver.try_iter().collect();
            let (stale, key, view) = (worker.stale, worker.key.clone(), worker.view.clone());
            let scope = worker.scope.clone();
            let replies = if finished {
                let worker = self.as_mut().rust_mut().prefetch_worker.take().unwrap();
                let _ = worker.handle.join();
                replies
                    .into_iter()
                    .chain(worker.receiver.try_iter())
                    .collect()
            } else {
                replies
            };
            if !stale {
                for reply in replies {
                    match reply {
                        Reply::Inventory(report, refreshed) => {
                            self.as_mut().warm_inventory(report, refreshed)
                        }
                        Reply::Done(Ok(payload)) => {
                            if let Payload::Sources(sources) = &payload {
                                let rows: Vec<_> = sources.iter().map(source_row).collect();
                                self.as_mut().set_source_catalog(encoded(rows));
                                self.as_mut().rust_mut().catalog_checked = true;
                            }
                            self.as_mut()
                                .rust_mut()
                                .prefetched
                                .insert(key.clone(), (Instant::now(), payload));
                        }
                        // Kept for the next preload or search, unless a
                        // running job will leave its own.
                        Reply::Engine(engine) if self.rust().worker.is_none() => {
                            self.as_mut().rust_mut().engine = Some(*engine);
                            self.as_mut().rust_mut().engine_scope = Some(scope.clone());
                        }
                        Reply::Asked(sources) => {
                            if let Some(worker) = &mut self.as_mut().rust_mut().prefetch_worker {
                                worker.asked = sources;
                            }
                        }
                        Reply::Answered(sources) => {
                            if let Some(worker) = &mut self.as_mut().rust_mut().prefetch_worker {
                                worker.answered = sources;
                            }
                        }
                        Reply::Rows(rows) => self.as_mut().rust_mut().prepared_rows.insert(*rows),
                        _ => {}
                    }
                }
            }
            // Show the result if the visible section was waiting for it,
            // or load it directly if the preload failed.
            let active = self.rust().active_view.clone();
            if finished && !stale && self.rust().awaiting_prefetch && active == view {
                self.as_mut().rust_mut().awaiting_prefetch = false;
                let view = active;
                let sources = self.rust().source_filter.join(",");
                let sudo = self.rust().sudo;
                let busy = self.rust().worker.is_some();
                self.as_mut().set_busy(busy);
                self.as_mut().load(
                    view.as_str().into(),
                    QString::default(),
                    sources.as_str().into(),
                    sudo,
                    false,
                );
            }
            if !finished {
                return;
            }
        }
        self.as_mut().rewarm_if_due();
        let writing = self.rust().worker.as_ref().is_some_and(|w| w.job.writes())
            || !self.rust().confirmed_queue.is_empty()
            || self.rust().pending.is_some();
        if writing {
            return;
        }
        while let Some(view) = self.as_mut().rust_mut().next_prefetch() {
            let key = cache_key(&view, "", &self.rust().source_filter, self.rust().sudo);
            let fresh = self.rust().view_cache.get(&key).is_some_and(|v| !v.stale())
                || self
                    .rust()
                    .prefetched
                    .get(&key)
                    .is_some_and(|(loaded, _)| loaded.elapsed() < VIEW_TTL);
            if !fresh {
                self.start_prefetch(view, key);
                break;
            }
        }
    }
    /// Keep sections warm while the app stays open: queue preloads of
    /// sections read long ago. poll() runs them. When nothing else is
    /// outstanding the page stops polling, so opening a section checks too.
    fn rewarm_if_due(mut self: Pin<&mut Self>) {
        if self.rust().prefetch.is_empty()
            && self.rust().last_rewarm.elapsed() >= Duration::from_secs(30)
        {
            self.as_mut().rust_mut().last_rewarm = Instant::now();
            let (sources, sudo) = (self.rust().source_filter.clone(), self.rust().sudo);
            // Updates runs `brew update` first, so it refreshes no more often
            // than background checks do.
            let updates_after = REWARM_AFTER.max(Duration::from_secs(
                self.rust().background_schedule.interval(),
            ));
            // prefetch_views() is in pop order, so Installed still loads first.
            let old: Vec<String> = prefetch_views()
                .into_iter()
                .filter(|view| {
                    let key = cache_key(view, "", &sources, sudo);
                    let cached = self.rust().view_cache.get(&key).map(|v| v.loaded);
                    let preloaded = self.rust().prefetched.get(&key).map(|(loaded, _)| *loaded);
                    let after = if view == "Updates" {
                        updates_after
                    } else {
                        REWARM_AFTER
                    };
                    cached
                        .max(preloaded)
                        .is_none_or(|loaded| loaded.elapsed() >= after)
                })
                .collect();
            self.as_mut().rust_mut().prefetch = old;
        }
    }
    /// Publish what background threads delivered and start queued work.
    /// Afterwards `needs_poll` says whether anything is still outstanding.
    pub fn poll(mut self: Pin<&mut Self>) {
        self.as_mut().poll_approval();
        self.as_mut().poll_work();
        self.as_mut().poll_activity();
        self.sync_needs_poll();
    }
    fn poll_work(mut self: Pin<&mut Self>) {
        if let Some(worker) = &self.rust().catalog_worker {
            if let Ok(sources) = worker.receiver.try_recv() {
                let worker = self.as_mut().rust_mut().catalog_worker.take().unwrap();
                let _ = worker.handle.join();
                if !self.rust().catalog_checked {
                    let rows: Vec<_> = sources.iter().map(source_row).collect();
                    self.as_mut().set_source_catalog(encoded(rows));
                    self.as_mut().rust_mut().catalog_checked = true;
                }
            }
        }
        self.as_mut().poll_prefetch();
        self.as_mut().show_pending();
        self.as_mut().poll_details();
        let Some(worker) = &self.rust().worker else {
            return;
        };
        let mut replies: Vec<_> = worker.receiver.try_iter().collect();
        let complete =
            replies.iter().any(|r| matches!(r, Reply::Done(_))) || worker.handle.is_finished();
        let cancelled = worker.cancel.requested();
        // A load another request replaced no longer waits on screen.
        if (cancelled || self.rust().background) && !self.rust().asked.is_empty() {
            self.as_mut().set_asked(vec![]);
        }
        // A finished worker's thread may still have sent its last replies.
        let finished = complete.then(|| {
            let Worker {
                handle,
                receiver,
                cancel,
                job,
            } = self.as_mut().rust_mut().worker.take().unwrap();
            let joined = handle.join();
            replies.extend(receiver.try_iter());
            (job, cancel, joined)
        });
        let replies: Vec<_> = replies
            .into_iter()
            .filter_map(|reply| match reply {
                Reply::Inventory(report, refreshed) => {
                    if !cancelled {
                        self.as_mut().warm_inventory(report, refreshed);
                    }
                    None
                }
                // Only a visible load shows what it still waits for.
                Reply::Asked(sources) => {
                    if !cancelled && !self.rust().background {
                        self.as_mut().set_asked(sources);
                    }
                    None
                }
                // Partials are cumulative: a source counts as answered once
                // it succeeded or failed.
                Reply::Partial(report) => {
                    if !self.rust().asked.is_empty() {
                        let answered = answered(&report);
                        let mut pending = self.rust().asked.clone();
                        pending.retain(|id| !answered.contains(id));
                        self.as_mut().set_asked(pending);
                    }
                    Some(Reply::Partial(report))
                }
                // Sent just before Done, which reads it, so it is kept
                // whether or not the worker has finished by now.
                Reply::FailureOutput(output) => {
                    self.as_mut().rust_mut().failure_output = output;
                    None
                }
                Reply::NeedsPassword(id) => {
                    self.as_mut().rust_mut().password_casks.push(id);
                    None
                }
                Reply::Matches(count) => {
                    self.as_mut().rust_mut().search_matches = count;
                    None
                }
                Reply::Rows(rows) => {
                    if !cancelled {
                        self.as_mut().rust_mut().prepared_rows.insert(*rows);
                    }
                    None
                }
                reply => Some(reply),
            })
            .collect();
        // Partials are cumulative: only the newest of a batch is shown.
        let newest = replies.iter().rposition(|reply| {
            matches!(
                reply,
                Reply::Partial(_) | Reply::Done(Ok(Payload::Packages(_)))
            )
        });
        let replies: Vec<_> = replies
            .into_iter()
            .enumerate()
            .filter(|(at, reply)| !matches!(reply, Reply::Partial(_)) || Some(*at) == newest)
            .map(|(_, reply)| reply)
            .collect();
        if let Some((job, cancel, joined)) = finished {
            // A source that never answered is not waited for any more.
            self.as_mut().set_asked(vec![]);
            let password_casks = std::mem::take(&mut self.as_mut().rust_mut().password_casks);
            let done = replies.iter().find_map(|reply| match reply {
                Reply::Done(result) => Some(result),
                _ => None,
            });
            if let Some(done) = done {
                self.as_mut()
                    .remember_update_outcomes(&job, done, &password_casks);
            }
            if matches!(job, Job::AutoUpgrade(..)) && !password_casks.is_empty() {
                let needed: Vec<(String, String)> = password_casks
                    .iter()
                    .filter_map(|id| {
                        let version = self.rust().cask_versions.get(id)?.clone();
                        Some((id.name.clone(), version))
                    })
                    .collect();
                let waiting: Vec<(String, String)> = self
                    .rust()
                    .cask_versions
                    .iter()
                    .map(|(id, version)| (id.name.clone(), version.clone()))
                    .collect();
                self.as_mut()
                    .rust_mut()
                    .needs_password
                    .remember(&needed, &waiting);
            }
            if let Job::AutoUpgrade(operations, _) = &job {
                if let Some(Reply::Done(Ok(Payload::Batch(_, outcomes)))) =
                    replies.iter().find(|reply| matches!(reply, Reply::Done(_)))
                {
                    let count =
                        |outcome: Outcome| outcomes.iter().filter(|o| **o == outcome).count();
                    self.as_mut().set_auto_update_result(encoded(json!({
                        "updated": count(Outcome::Finished),
                        "failed": count(Outcome::Failed) + count(Outcome::Cancelled),
                        "total": operations.len(),
                    })));
                }
            }
            // A source that just refreshed was checked successfully, even
            // before any page has listed it.
            let finished: Vec<Operation> =
                match replies.iter().find(|reply| matches!(reply, Reply::Done(_))) {
                    Some(Reply::Done(Ok(Payload::Batch(_, outcomes)))) => job
                        .operations()
                        .into_iter()
                        .zip(outcomes)
                        .filter(|(_, outcome)| **outcome == Outcome::Finished)
                        .map(|(operation, _)| operation)
                        .collect(),
                    Some(Reply::Done(Ok(_))) => job.operations(),
                    _ => vec![],
                };
            let refreshed: Vec<String> = finished
                .iter()
                .filter(|operation| matches!(operation, Operation::Refresh { .. }))
                .map(|operation| operation.backend().to_string())
                .collect();
            if !refreshed.is_empty() {
                let now = epoch_seconds();
                for backend in refreshed {
                    self.as_mut().rust_mut().last_success.insert(backend, now);
                }
                let mut state: Value = serde_json::from_str(&self.report_state().to_string())
                    .unwrap_or_else(|_| json!({}));
                state["last_success"] = json!(self.rust().last_success);
                self.as_mut().set_report_state(encoded(state));
            }
            // A change can replace PkgDeck itself; the new copy runs once
            // this one restarts.
            if job.writes()
                && self.rust().self_update.is_empty()
                && replies
                    .iter()
                    .any(|reply| matches!(reply, Reply::Done(Ok(_))))
                && self
                    .rust()
                    .install
                    .as_ref()
                    .is_some_and(pkgdeck_core::relaunch::Install::updated)
            {
                let how = if matches!(job, Job::AutoUpgrade(..)) {
                    "automatic"
                } else {
                    "manual"
                };
                self.as_mut().set_self_update(how.into());
            }
            for reply in replies {
                if self.rust().background {
                    if !cancel.requested() {
                        match reply {
                            Reply::Done(Ok(Payload::BackgroundUpdates(report))) => {
                                self.as_mut().finish_background_check(report)
                            }
                            Reply::Done(Err(error)) => {
                                self.as_mut().finish_background_error(&error)
                            }
                            Reply::Done(Ok(payload)) => {
                                if let Job::Load(view, query) = &job {
                                    if let Payload::Sources(sources) = &payload {
                                        let rows: Vec<_> = sources.iter().map(source_row).collect();
                                        self.as_mut().set_source_catalog(encoded(rows));
                                        self.as_mut().rust_mut().catalog_checked = true;
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
                        // Remember what this engine detected so the next
                        // search can reuse it. Only reads leave a reusable one.
                        let born = self.as_mut().rust_mut().reused_engine_born.take();
                        let scope = matches!(job, Job::Load(ref view, _) if view != "Sources")
                            .then(|| {
                                (
                                    engine_source(&job, &self.rust().source_filter),
                                    self.rust().sudo,
                                    born.unwrap_or_else(Instant::now),
                                )
                            });
                        self.as_mut().rust_mut().engine_scope = scope;
                        self.as_mut().rust_mut().engine = Some(*engine);
                    }
                    Reply::Partial(report) => {
                        if !self.rust().hold_partials {
                            self.as_mut().apply(Ok(Payload::Packages(report)));
                        }
                    }
                    Reply::DetailsPreview(details) => {
                        self.as_mut().rust_mut().details_preview = true;
                        self.as_mut().apply(Ok(Payload::Details(details)));
                        self.as_mut().rust_mut().details_preview = false;
                    }
                    Reply::Done(result) => {
                        if self.rust().discard_revalidation && job.reviews() {
                            continue;
                        }
                        if let Err(error) = &result {
                            if let Some(notice) = preflight_notice(
                                &job,
                                error,
                                self.rust().sudo,
                                &self.rust().names_for(&job.operations()),
                            ) {
                                self.as_mut().set_notice(encoded(notice));
                            }
                        }
                        if job.writes() {
                            self.as_mut().mark_opened_installed(&job, &result);
                            // A changed plan is not a failure: the new plan
                            // opens for review right after this.
                            let mut notice = if repreview_changed_plan(
                                &job,
                                &result,
                                &self.rust().packages,
                            )
                            .is_some()
                            {
                                json!({"kind": "info", "title": "The planned changes are different now. Review them again."})
                            } else {
                                let names = self.rust().names_for(&job.operations());
                                password_notice(&job, &result, &password_casks, &names)
                                    .unwrap_or_else(|| {
                                        write_notice(&job, &result, self.rust().sudo, &names)
                                    })
                            };
                            let batch_output =
                                std::mem::take(&mut self.as_mut().rust_mut().failure_output);
                            let retry = if notice["kind"] == "error" {
                                let output = if batch_output.is_empty() {
                                    result
                                        .as_ref()
                                        .err()
                                        .and_then(raw_failure_output)
                                        .unwrap_or_default()
                                } else {
                                    batch_output
                                };
                                if !output.is_empty() {
                                    notice["output"] = json!(output);
                                }
                                retry_job(&job, &result)
                            } else {
                                None
                            };
                            notice["retry"] = json!(retry.is_some());
                            self.as_mut().rust_mut().retry_job = retry;
                            self.as_mut().set_notice(encoded(notice));
                            if let Some(id) = self.as_mut().rust_mut().active_activity_id.take() {
                                if let Some(store) = self.rust().activity_store.clone() {
                                    let outcomes = match &result {
                                        Ok(Payload::Batch(_, outcomes)) => outcomes.clone(),
                                        Ok(_) => job
                                            .operations()
                                            .iter()
                                            .map(|_| Outcome::Finished)
                                            .collect(),
                                        Err(EngineError::Cancelled) => job
                                            .operations()
                                            .iter()
                                            .map(|_| Outcome::Cancelled)
                                            .collect(),
                                        Err(_) => job
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
                            repreview_changed_plan(&job, &result, &self.rust().packages)
                        {
                            self.as_mut().rust_mut().queued = Some(next);
                        }
                        // Snapshot terminal view reports for instant
                        // switching back, unless a newer load already
                        // superseded this one (its guard in apply skips it).
                        let key = match &job {
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
                            // Update all retried failed sources; the merged
                            // report is the current Updates section.
                            Job::RetryFailedUpdates(_)
                                if !matches!(self.rust().queued, Some(Job::Load(..))) =>
                            {
                                Some(cache_key(
                                    "Updates",
                                    "",
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
                                | Ok(Payload::RetryFailedUpdates(..))
                                | Ok(Payload::RetryCleanup(..))
                                | Ok(Payload::RetrySources(..))
                        );
                        self.as_mut().apply(result);
                        if stashable {
                            if let Some(key) = key {
                                self.as_mut().stash_current(key);
                            }
                        }
                        if matches!(job, Job::RetryFailedUpdates(_))
                            && self.rust().queued.is_none()
                            && !cancel.requested()
                            && self.rust().active_view == "Updates"
                        {
                            let operations = upgrade_plan(&self.rust().packages);
                            if operations.is_empty() {
                                self.as_mut().set_status(
                                    "No available updates after retrying source checks.".into(),
                                );
                            } else {
                                let count = self
                                    .rust()
                                    .packages
                                    .iter()
                                    .filter(|package| {
                                        package.installed_version.is_some()
                                            && package.update == UpdateAvailability::Available
                                    })
                                    .count();
                                self.as_mut().rust_mut().queued =
                                    Some(Job::PlanUpgrade(operations, count));
                            }
                        }
                    }
                    Reply::Progress(_)
                    | Reply::ProgressEvent(_)
                    | Reply::Output(_)
                    | Reply::FailureOutput(_)
                    | Reply::NeedsPassword(_)
                    | Reply::Asked(_)
                    | Reply::Answered(_)
                    | Reply::Matches(_)
                    | Reply::Rows(_)
                    | Reply::Inventory(..) => {}
                }
            }
            if joined.is_err() && !self.rust().background {
                self.as_mut()
                    .set_status("A background task failed. Try again.".into());
            } else if joined.is_err() {
                self.as_mut()
                    .finish_background_error(&EngineError::InvalidResponse {
                        backend: "check".into(),
                        reason: "Backend worker failed".into(),
                    });
            }
            // The quiet reload of the section on screen has landed.
            if !self.rust().background
                && matches!(&job, Job::Load(view, _) if *view == self.rust().active_view)
            {
                self.as_mut().set_refreshing(false);
            }
            self.as_mut().rust_mut().background = false;
            self.as_mut().rust_mut().discard_revalidation = false;
            let queued = self.as_mut().rust_mut().queued.take();
            self.as_mut().rust_mut().progress_state = None;
            self.as_mut().set_progress("{}".into());
            self.as_mut().set_writing(false);
            let awaiting = self.rust().awaiting_prefetch && !self.rust().refreshing;
            self.as_mut().set_busy(awaiting);
            if let Some(entry) = self.as_mut().rust_mut().validated_confirmed.take() {
                self.as_mut().rust_mut().queued = queued;
                self.start_confirmed(entry);
            } else if let Some(entry) = self.as_mut().rust_mut().confirmed_queue.pop_front() {
                self.as_mut().rust_mut().queued = queued;
                self.validate_confirmed(entry);
            } else if matches!(
                &queued,
                Some(
                    Job::PlanOperation(..)
                        | Job::PlanAdoption(..)
                        | Job::PlanAdoptAll(..)
                        | Job::PlanUpgrade(..)
                )
            ) {
                self.start(queued.expect("review job"));
            } else if let Some(job) = self.as_mut().rust_mut().deferred_load.take() {
                self.start(job);
            } else if let Some(job) = queued {
                self.start(job);
            }
        } else if !self.rust().background {
            for reply in replies {
                match reply {
                    Reply::Partial(report) => {
                        if !self.rust().hold_partials {
                            self.as_mut().apply(Ok(Payload::Packages(report)));
                        }
                    }
                    Reply::DetailsPreview(details) => {
                        self.as_mut().rust_mut().details_preview = true;
                        self.as_mut().apply(Ok(Payload::Details(details)));
                        self.as_mut().rust_mut().details_preview = false;
                    }
                    Reply::Progress(message) => {
                        self.as_mut().set_status(message.as_str().into());
                    }
                    Reply::ProgressEvent(event) => self.as_mut().update_progress(&event),
                    Reply::Output(line) => self.as_mut().follow_output(&line),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// What `$pattern` binds in `$value`; any other value fails the test.
    macro_rules! expect {
        ($value:expr, $pattern:pat => $bound:expr) => {
            match $value {
                $pattern => $bound,
                _ => panic!("expected {}", stringify!($pattern)),
            }
        };
    }
    #[test]
    fn write_progress_tracks_real_steps_and_transfer_totals() {
        let first = Operation::Refresh {
            backend: "apt".into(),
        };
        let second = Operation::Refresh {
            backend: "flatpak".into(),
        };
        let mut state = ProgressState::new(
            &Job::UpgradeAll(vec![first.clone(), second.clone()], None),
            Some(42),
            &Names::new(),
        );
        state.apply(&Event::Started(first.clone()));
        state.apply(&Event::Progress {
            operation: first.clone(),
            progress: Progress::Transfer {
                completed: 50,
                total: Some(100),
            },
        });
        let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
        assert_eq!(snapshot["activity_id"], 42);
        assert_eq!(snapshot["done"], 0);
        assert_eq!(snapshot["total"], 2);
        assert_eq!(snapshot["transfer_total"], 100);
        state.apply(&Event::Finished {
            operation: first,
            result: Ok(OperationOutcome::default()),
        });
        state.apply(&Event::Started(second));
        let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
        assert_eq!(snapshot["done"], 1);
        assert_eq!(snapshot["transfer_total"], Value::Null);
        assert_eq!(snapshot["label"], "Refresh Flatpak package lists");
        let repository = RepositoryAction {
            backend: "flatpak".into(),
            name: "synthetic".into(),
            scope: Scope::System,
            change: repositories::Change::Remove,
        };
        let state = ProgressState::new(
            &Job::Repositories(Some(repository.clone())),
            None,
            &Names::new(),
        );
        assert_eq!(state.label, "Remove synthetic repository");
        for (change, label) in [
            (
                repositories::Change::Add {
                    url: "https://example.invalid/repo".into(),
                },
                "Add synthetic repository",
            ),
            (
                repositories::Change::SetEnabled { enabled: true },
                "Enable synthetic repository",
            ),
            (
                repositories::Change::SetEnabled { enabled: false },
                "Disable synthetic repository",
            ),
            (
                repositories::Change::SetPriority { priority: 5 },
                "Change synthetic priority",
            ),
            (repositories::Change::OpenEditor, "Open software sources"),
        ] {
            let action = RepositoryAction {
                change,
                ..repository.clone()
            };
            let state = ProgressState::new(&Job::Repositories(Some(action)), None, &Names::new());
            assert_eq!(state.label, label);
        }
        let mut state = ProgressState::new(
            &Job::Load("installed".into(), String::new()),
            None,
            &Names::new(),
        );
        assert_eq!(state.label, "Working");
        let before = state.snapshot().to_string();
        state.apply(&Event::Progress {
            operation: Operation::Refresh {
                backend: "apt".into(),
            },
            progress: Progress::Message("Reading lists".into()),
        });
        assert_eq!(state.snapshot().to_string(), before);
    }
    #[test]
    fn file_urls_preserve_spaces_and_unicode_and_reject_remote_hosts() {
        assert_eq!(
            local_input_path("file:///tmp/Sample%20%C3%B1.AppImage").unwrap(),
            PathBuf::from("/tmp/Sample ñ.AppImage")
        );
        assert_eq!(
            local_input_path("file://localhost/tmp/Sample%20App.deb").unwrap(),
            PathBuf::from("/tmp/Sample App.deb")
        );
        assert!(local_input_path("file://remote/tmp/package.deb").is_err());
        assert!(local_input_path("file:///tmp/bad%00.deb").is_err());
        assert!(local_input_path("file:///tmp/bad%2Z.deb").is_err());
        assert!(local_input_path("file:///tmp/bad%FF.deb").is_err());
        assert!(local_input_path("relative.deb").is_err());
        assert!(local_input_path("https://example.org/package.deb").is_err());
        assert!(inspect_open_input("file:///tmp/bad%2Z.deb", &Cancellation::default()).is_err());
    }
    #[test]
    fn direct_https_links_route_to_their_native_previews() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::var_os("PKGDECK_OPEN_LINK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("pkgdeck-open-links-{}", std::process::id()))
            });
        if std::env::var_os("PKGDECK_OPEN_LINK_CHILD").is_none() {
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let curl = base.join("curl");
            let script = "#!/bin/sh\nout=''\nfor arg; do\n  if [ \"$previous\" = '--output' ]; then out=\"$arg\"; fi\n  previous=\"$arg\"\ndone\ncase \"$arg\" in\n  *.flatpakref) printf '[Flatpak Ref]\\nName=org.example.Synthetic\\nUrl=https://example.invalid/repo\\n' ;;\n  *.flatpakrepo) printf '[Flatpak Repo]\\nName=synthetic\\nUrl=https://example.invalid/repo\\nGPGKey=c3ludGhldGlj\\n' ;;\n  *.flatpak) printf 'synthetic bundle bytes' > \"$out\" ;;\nesac\n";
            std::fs::write(&curl, script).unwrap();
            std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "controller::tests::direct_https_links_route_to_their_native_previews",
                    "--nocapture",
                ])
                .env("PKGDECK_OPEN_LINK_CHILD", "1")
                .env("PKGDECK_OPEN_LINK_DIR", &base)
                .env("PATH", &base)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            std::fs::remove_dir_all(base).unwrap();
            return;
        }
        let cancel = Cancellation::default();
        for link in [
            "https://example.invalid/synthetic.flatpakref",
            "flatpak+https://example.invalid/synthetic.flatpakref",
        ] {
            let preview = inspect_open_input(link, &cancel).unwrap();
            let opened = expect!(preview, Payload::OpenPackage(opened) => opened);
            assert_eq!(opened.package.id.name, "org.example.Synthetic");
        }
        let preview =
            inspect_open_input("https://example.invalid/synthetic.flatpakrepo", &cancel).unwrap();
        let import = expect!(preview, Payload::OpenRepository(import) => import);
        assert_eq!(import.name, "synthetic");
        let preview =
            inspect_open_input("https://example.invalid/synthetic.flatpak", &cancel).unwrap();
        let opened = expect!(preview, Payload::OpenPackage(opened) => opened);
        assert_eq!(opened.package.id.backend, "flatpak");
        assert!(inspect_open_input("https://example.invalid/unsupported.bin", &cancel).is_err());
    }
    #[test]
    fn remote_appimage_confirmation_uses_the_filename() {
        let id = PackageId {
            backend: "appimage".into(),
            name: "https://example.invalid/downloads/Synthetic.AppImage?version=1".into(),
            architecture: "x86_64".into(),
            scope: Scope::Environment {
                path: PathBuf::from("/tmp/synthetic-appimages"),
            },
            remote: None,
            reference: Some("artifact:appimage:synthetic".into()),
        };
        let label = operation_label(&Operation::Install(id));
        assert!(label.contains("Install Synthetic.AppImage"));
        assert!(!label.contains("?version=1"));
    }
    #[test]
    fn opening_repository_file_previews_before_any_write() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-open-repository-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let source = base.join("synthetic.sources");
        std::fs::write(&source, "Types: deb\nURIs: https://example.invalid/repo\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/synthetic.gpg\n").unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .open_input(source.to_str().unwrap().into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && controller.confirmation().is_empty() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(controller.confirmation().to_string().contains("APT source"));
        assert!(matches!(
            controller.rust().pending,
            Some(Job::ImportRepository(_))
        ));
        controller.as_mut().confirm(false);
        assert!(source.exists());
        controller
            .as_mut()
            .open_input(source.to_str().unwrap().into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && controller.confirmation().is_empty() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(&source, "Types: deb\nURIs: https://example.invalid/changed\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/synthetic.gpg\n").unwrap();
        controller.as_mut().confirm(true);
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && *controller.busy() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(controller
            .status()
            .to_string()
            .contains("changed since preview"));
        let one_click = base.join("synthetic.ymp");
        std::fs::write(&one_click, "<metapackage><group/></metapackage>\n").unwrap();
        controller
            .as_mut()
            .open_input(one_click.to_str().unwrap().into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && controller.confirmation().is_empty() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Open native installer"));
        controller.as_mut().confirm(false);
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn confirmed_flatpak_repository_reaches_the_native_manager() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::var_os("PKGDECK_CONTROLLER_REPO_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("pkgdeck-controller-repo-{}", std::process::id()))
            });
        if std::env::var_os("PKGDECK_CONTROLLER_REPO_CHILD").is_none() {
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let flatpak = base.join("flatpak");
            std::fs::write(
                &flatpak,
                format!(
                    "#!/bin/sh\ncase \"$*\" in\n *remote-add*) printf 'added' > '{}' ;;\nesac\n",
                    base.join("record").display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&flatpak, std::fs::Permissions::from_mode(0o755)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "controller::tests::confirmed_flatpak_repository_reaches_the_native_manager",
                    "--nocapture",
                ])
                .env("PKGDECK_CONTROLLER_REPO_CHILD", "1")
                .env("PKGDECK_CONTROLLER_REPO_DIR", &base)
                .env("PATH", &base)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            std::fs::remove_dir_all(base).unwrap();
            return;
        }
        let source = base.join("synthetic.flatpakrepo");
        std::fs::write(&source, "[Flatpak Repo]\nName=synthetic\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n").unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .open_input(source.to_str().unwrap().into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && controller.confirmation().is_empty() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Add synthetic"));
        controller.as_mut().confirm(true);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !base.join("record").exists() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(base.join("record")).unwrap(),
            "added"
        );
        assert!(source.exists());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn opening_local_deb_reads_metadata_without_installing_it() {
        let base = std::env::temp_dir().join(format!("pkgdeck-open-deb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let control = base.join("staging/DEBIAN");
        std::fs::create_dir_all(&control).unwrap();
        std::fs::write(
            control.join("control"),
            "Package: pkgdeck-open-synthetic\nVersion: 1.2.3\nArchitecture: all\nMaintainer: PkgDeck tests <nobody@example.invalid>\nDescription: Synthetic local archive\n",
        )
        .unwrap();
        let archive = base.join("Synthetic package.deb");
        let status = std::process::Command::new("dpkg-deb")
            .arg("--build")
            .arg(base.join("staging"))
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        let preview = inspect_open_input(archive.to_str().unwrap(), &Cancellation::default());
        let opened = expect!(preview.unwrap(), Payload::OpenPackage(opened) => opened);
        let package = &opened.package;
        assert_eq!(package.id.backend, "apt");
        assert_eq!(package.id.name, "pkgdeck-open-synthetic");
        assert_eq!(package.candidate_version.as_deref(), Some("1.2.3"));
        assert!(package
            .id
            .reference
            .as_deref()
            .unwrap()
            .starts_with("local-deb:"));
        assert!(archive.exists());
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn opening_appimage_previews_without_importing_until_confirmation() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-open-appimage-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let source = base.join("Sample ñ.AppImage");
        let mut elf = [0_u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4..7].copy_from_slice(&[2, 1, 1]);
        elf[8..11].copy_from_slice(b"AI\x02");
        // ELF machine: AArch64 (183) or x86-64 (62).
        let machine: u16 = [62, 183][usize::from(std::env::consts::ARCH == "aarch64")];
        elf[18..20].copy_from_slice(&machine.to_le_bytes());
        elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
        elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
        std::fs::write(&source, elf).unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .open_input(source.to_str().unwrap().into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && controller.opened().is_empty() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        // It opens as a page first; nothing is asked until Install.
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["action"], "Install", "{page}");
        assert_eq!(page["location"], source.to_str().unwrap());
        assert!(controller.confirmation().is_empty());
        controller.as_mut().install_opened();
        let preview = controller.confirmation().to_string();
        let status = controller.status();
        assert!(
            preview.contains("Sample ñ"),
            "preview: {preview}; status: {status}"
        );
        assert!(preview.contains("Location:"), "{preview}");
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert!(
            !data["summary"].as_str().unwrap().contains("Other changes"),
            "{data}"
        );
        assert!(matches!(
            controller.rust().pending,
            Some(Job::Write(Operation::Install(_), _))
        ));
        controller.as_mut().confirm(false);
        assert!(source.exists());
        assert!(controller.confirmation().is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn opening_flatpak_reference_uses_confirmation_and_rejects_unsupported_input() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-open-flatpakref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let source = base.join("Synthetic ñ.flatpakref");
        std::fs::write(
            &source,
            "[Flatpak Ref]\nName=org.example.Synthetic\nUrl=https://example.invalid/repo\n",
        )
        .unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .open_input(base.join("queued-unsupported.txt").to_str().unwrap().into());
        controller
            .as_mut()
            .open_input(format!("file://{}", source.display()).into());
        assert!(controller
            .status()
            .to_string()
            .contains("The file will open when the current operation finishes"));
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && controller.opened().is_empty() {
            controller.as_mut().poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        controller.as_mut().install_opened();
        let preview = controller.confirmation().to_string();
        assert!(preview.contains("org.example.Synthetic"), "{preview}");
        assert!(
            preview.contains("Repository to add if needed:"),
            "{preview}"
        );
        assert!(matches!(
            controller.rust().pending,
            Some(Job::Write(Operation::Install(_), _))
        ));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["flatpak_ref_scope"], "user");
        controller.as_mut().set_open_flatpak_scope(true);
        assert!(controller.confirmation().to_string().contains("System"));
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Install(id), _)) if id.scope == Scope::System
        ));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["flatpak_ref_scope"], "system");
        controller.as_mut().set_open_flatpak_scope(false);
        assert!(controller.confirmation().to_string().contains("User"));
        controller.as_mut().confirm(false);
        assert!(source.exists());

        controller
            .as_mut()
            .open_input(base.join("unsupported.txt").to_str().unwrap().into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            controller.as_mut().poll();
            if !controller.busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(controller
            .status()
            .to_string()
            .to_lowercase()
            .contains("unsupported package format"));
        assert!(controller.confirmation().is_empty());
        controller
            .as_mut()
            .open_input("flatpak+http://example.invalid/app.flatpakref".into());
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            controller.as_mut().poll();
            if !controller.busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(controller.status().to_string().contains(".flatpakref"));
        assert!(controller.confirmation().is_empty());
        controller.as_mut().open_input(" ".into());
        assert_eq!(
            controller.status().to_string(),
            "Choose an installation file."
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn appimage_launch_settings_need_an_installed_appimage() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let mut apt = synthetic_package("tool", "Tool");
        apt.installed_version = Some("1".into());
        let mut gone = synthetic_package("/nonexistent/pkgdeck-test/demo.appimage", "Demo");
        gone.id.backend = "appimage".into();
        gone.id.scope = Scope::User { uid: 1000 };
        gone.installed_version = Some("1".into());
        controller.as_mut().rust_mut().packages = vec![apt, gone];
        // Anything else, or no row at all, has nothing to show or do.
        for index in [-1, 0, 5] {
            assert_eq!(
                controller.as_mut().app_launch_settings(index).to_string(),
                "{}"
            );
            assert_eq!(
                controller.as_mut().launch_app(index).to_string(),
                "Nothing to start."
            );
            assert_eq!(
                controller
                    .as_mut()
                    .save_app_launch_settings(index, "%U".into(), "[]".into())
                    .to_string(),
                "Nothing to change."
            );
        }
        // An AppImage that's gone says so instead.
        let settings: Value =
            serde_json::from_str(&controller.as_mut().app_launch_settings(1).to_string()).unwrap();
        assert!(settings["error"].is_string(), "{settings}");
        assert!(!controller.as_mut().launch_app(1).is_empty());
        assert!(!controller
            .as_mut()
            .save_app_launch_settings(
                1,
                "%U".into(),
                r#"[{"name":" A ","value":"1"},{"name":"","value":"x"}]"#.into()
            )
            .is_empty());
    }
    #[test]
    fn appimage_pages_read_change_and_launch_through_the_menu_entry() {
        let data = std::env::temp_dir().join(format!("pkgdeck-launch-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let root = data.join("pkgdeck/appimages");
        let applications = data.join("applications");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&applications).unwrap();
        // A managed AppImage that's a script, so launching it really runs.
        let digest = "ab".repeat(32);
        let name = format!("pkgdeck-{digest}.AppImage");
        let file = root.join(&name);
        let record = data.join("record");
        std::fs::write(
            &file,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$GREETING\" \"$*\" > '{}'\n",
                record.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let entry = applications.join(format!("pkgdeck-{digest}.desktop"));
        std::fs::write(
            &entry,
            format!(
                "[Desktop Entry]\nName=Demo\nExec=\"{}\" --flag %U\n",
                file.display()
            ),
        )
        .unwrap();
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().appimage_data = Some(data.clone());
        let mut row = synthetic_package(&name, "Demo");
        row.id.backend = "appimage".into();
        row.id.scope = Scope::User {
            uid: rustix::process::getuid().as_raw(),
        };
        row.installed_version = Some("2.0".into());
        controller.as_mut().rust_mut().packages = vec![row];
        let settings: Value =
            serde_json::from_str(&controller.as_mut().app_launch_settings(0).to_string()).unwrap();
        assert_eq!(
            settings,
            json!({"arguments": "--flag %U", "environment": [], "editable": true})
        );
        // Blank variable names are dropped; the rest are saved trimmed.
        assert_eq!(
            controller
                .as_mut()
                .save_app_launch_settings(
                    0,
                    "--other %U".into(),
                    r#"[{"name":" GREETING ","value":"hi"},{"name":"","value":"x"}]"#.into()
                )
                .to_string(),
            ""
        );
        let settings: Value =
            serde_json::from_str(&controller.as_mut().app_launch_settings(0).to_string()).unwrap();
        assert_eq!(settings["arguments"], "--other %U");
        assert_eq!(
            settings["environment"],
            json!([{"name": "GREETING", "value": "hi"}])
        );
        assert_eq!(controller.as_mut().launch_app(0).to_string(), "");
        let text = (0..200)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(10));
                std::fs::read_to_string(&record)
                    .ok()
                    .filter(|text| text.ends_with('\n'))
            })
            .expect("the AppImage never started");
        assert_eq!(text, "hi|--other\n");
        // Its page shows the file it starts from.
        let shown: Value =
            serde_json::from_str(&controller.as_mut().app_file(0).to_string()).unwrap();
        assert_eq!(shown["path"], file.display().to_string());
        assert_eq!(shown["folder"], root.display().to_string());
        assert_eq!(shown["bytes"], std::fs::metadata(&file).unwrap().len());
        assert_eq!(shown["managed"], true);
        assert_eq!(shown["without_fuse"], false);
        assert_eq!(controller.as_mut().app_file(5).to_string(), "{}");
        // The page of a file PkgDeck just installed starts that install.
        assert_eq!(
            controller.as_mut().launch_opened().to_string(),
            "Nothing to start."
        );
        let mut opened = controller.rust().packages[0].clone();
        opened.id.name = data.join("Demo.AppImage").display().to_string();
        opened.id.reference = Some(digest.clone());
        controller.as_mut().rust_mut().opened_package = Some(opened);
        std::fs::remove_file(&record).unwrap();
        assert_eq!(controller.as_mut().launch_opened().to_string(), "");
        let text = (0..200)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(10));
                std::fs::read_to_string(&record)
                    .ok()
                    .filter(|text| text.ends_with('\n'))
            })
            .expect("the AppImage never started");
        assert_eq!(text, "hi|--other\n");
        controller.as_mut().rust_mut().opened_package = None;
        // Where it updates from, saved for the next check.
        let source: Value =
            serde_json::from_str(&controller.as_mut().app_update_source(0).to_string()).unwrap();
        assert_eq!(
            source,
            json!({"github": null, "builtin": false, "editable": true})
        );
        assert!(!controller
            .as_mut()
            .save_app_update_source(0, "not a project".into())
            .is_empty());
        assert_eq!(
            controller
                .as_mut()
                .save_app_update_source(0, "example/demo".into())
                .to_string(),
            ""
        );
        let source: Value =
            serde_json::from_str(&controller.as_mut().app_update_source(0).to_string()).unwrap();
        assert_eq!(source["github"], "example/demo");
        assert_eq!(controller.as_mut().app_update_source(5).to_string(), "{}");
        assert_eq!(
            controller
                .as_mut()
                .save_app_update_source(5, "x/y".into())
                .to_string(),
            "Nothing to change."
        );
        // An AppImage that's gone says why.
        let mut gone = controller.rust().packages[0].clone();
        gone.id.name = "/nonexistent/pkgdeck-test/other.appimage".into();
        controller.as_mut().rust_mut().packages.push(gone);
        let gone: Value =
            serde_json::from_str(&controller.as_mut().app_update_source(1).to_string()).unwrap();
        assert!(gone["error"].is_string(), "{gone}");
        assert_eq!(controller.as_mut().app_file(1).to_string(), "{}");
        // Opened from a file that isn't an AppImage: nothing to start.
        let mut opened = controller.rust().packages[0].clone();
        opened.id.backend = "apt".into();
        opened.id.reference = Some(digest);
        controller.as_mut().rust_mut().opened_package = Some(opened);
        assert_eq!(
            controller.as_mut().launch_opened().to_string(),
            "Nothing to start."
        );
        // One the install didn't keep can't start.
        let mut opened = controller.rust().packages[0].clone();
        opened.id.reference = Some("cd".repeat(32));
        controller.as_mut().rust_mut().opened_package = Some(opened);
        assert!(!controller.as_mut().launch_opened().is_empty());
        std::fs::remove_dir_all(data).unwrap();
    }
    #[test]
    fn previews_say_when_a_change_needs_a_second_look() {
        let package = synthetic_package("synthetic", "Synthetic");
        let install = Operation::Install(package.id.clone());
        let preview = |operation: &Operation, plan: Option<&TransactionPlan>| {
            confirmation_preview(operation, std::slice::from_ref(&package), &[], plan)
        };
        // Installing or updating just that app runs without asking.
        assert_eq!(preview(&install, None)["review"], false);
        assert_eq!(
            preview(&Operation::Upgrade(package.id.clone()), None)["review"],
            false
        );
        // Removing always asks, and says what may stay behind.
        let removal = preview(&Operation::Remove(package.id.clone()), None);
        assert_eq!(removal["review"], true);
        assert_eq!(
            removal["notes"],
            json!([
                "Other changes may be required.",
                "App data may remain after removal."
            ])
        );
        // A plan that changes other packages lists them.
        let change = |name: &str| PlannedChange {
            action: PlannedAction::Install,
            name: name.into(),
            installed_version: None,
            candidate_version: Some("1".into()),
        };
        let mut plan = TransactionPlan {
            operation: install.clone(),
            native_preview: String::new(),
            changes: vec![change("synthetic")],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: None,
        };
        let alone = preview(&install, Some(&plan));
        assert_eq!(alone["review"], false);
        assert_eq!(alone["changes"], json!([]));
        plan.changes.push(change("synthetic-library"));
        let more = preview(&install, Some(&plan));
        assert_eq!(more["review"], true);
        assert_eq!(more["notes"], json!(["1 other package will change"]));
        assert_eq!(more["changes"], json!(["Install synthetic-library (1)"]));
        // Moving an app already in place is reviewed too.
        plan.changes.truncate(1);
        plan.adopts = Some("/Applications/Synthetic.app".into());
        assert_eq!(preview(&install, Some(&plan))["review"], true);
        // So is anything that isn't one app.
        assert_eq!(
            confirmation_preview(
                &Operation::UpgradeAll {
                    backend: "apt".into()
                },
                &[],
                &[],
                None
            )["review"],
            true
        );
    }
    #[test]
    fn opened_files_show_a_page_until_back_and_installed_after_their_install() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        // Nothing open: Install does nothing.
        controller.as_mut().install_opened();
        assert!(controller.confirmation().is_empty());
        let mut package = synthetic_package("demo", "Demo");
        package.id.backend = "fixture".into();
        package.installed_version = None;
        package.candidate_version = Some("2.1.7".into());
        package.summary =
            "Flash images safely\nLocal archive: /home/user/Downloads/demo.deb".into();
        package.id.reference = Some(format!(
            "local-deb:{}:/home/user/Downloads/demo.deb",
            "ab".repeat(32)
        ));
        let details = PackageDetails {
            description: "A longer description.".into(),
            homepage: Some("https://example.invalid/demo".into()),
            dependencies: vec!["libc6".into()],
            package: package.clone(),
        };
        controller
            .as_mut()
            .apply(Ok(Payload::OpenPackage(Box::new(details))));
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["action"], "Install");
        assert_eq!(page["location"], "/home/user/Downloads/demo.deb");
        assert_eq!(page["package"]["summary"], "Flash images safely");
        assert_eq!(page["description"], "A longer description.");
        assert_eq!(page["homepage"], "https://example.invalid/demo");
        assert_eq!(page["dependencies"], json!(["libc6"]));
        // Nothing is asked until Install.
        assert!(controller.confirmation().is_empty());
        controller.as_mut().install_opened();
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["review"], false);
        controller.as_mut().confirm(false);
        // Another app's install leaves the page as it is; its own marks it installed.
        let job = |id: &PackageId| Job::Write(Operation::Install(id.clone()), None);
        let other = synthetic_package("other", "Other").id;
        controller.as_mut().mark_opened_installed(
            &job(&other),
            &Ok(Payload::Written(OperationOutcome::default())),
        );
        controller
            .as_mut()
            .mark_opened_installed(&job(&package.id), &Err(EngineError::NotFound));
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["action"], "Install");
        controller.as_mut().mark_opened_installed(
            &job(&package.id),
            &Ok(Payload::Written(OperationOutcome::default())),
        );
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["action"], "");
        assert_eq!(page["package"]["installed"], "2.1.7");
        // Back closes it.
        controller.as_mut().close_opened();
        assert!(controller.opened().is_empty());
        controller.as_mut().install_opened();
        assert!(controller.confirmation().is_empty());
        controller.as_mut().mark_opened_installed(
            &job(&package.id),
            &Ok(Payload::Written(OperationOutcome::default())),
        );
        assert!(controller.opened().is_empty());
        // Links and references show where they came from.
        let mut link = package.clone();
        link.id.name = "https://example.invalid/demo.flatpakref".into();
        link.id.reference = Some("flatpakref:app/org.example.Demo/x86_64/stable".into());
        assert_eq!(
            opened_page(
                &PackageDetails {
                    description: String::new(),
                    homepage: None,
                    dependencies: vec![],
                    package: link
                },
                "Install"
            )["location"],
            "https://example.invalid/demo.flatpakref"
        );
    }
    #[test]
    fn appimage_install_previews_show_icon_and_description() {
        let mut package = synthetic_package("/home/user/Downloads/demo.appimage", "Demo App");
        package.id.backend = "appimage".into();
        package.id.scope = Scope::Environment {
            path: "/home/user/.local/share/pkgdeck/appimages".into(),
        };
        package.summary = "Does demo things".into();
        package.installed_version = None;
        package.candidate_version = Some("2.0".into());
        package.icon = Some("/icons/demo.png".into());
        let data = confirmation_preview(
            &Operation::Install(package.id.clone()),
            std::slice::from_ref(&package),
            &[],
            None,
        );
        assert_eq!(data["icon"], "/icons/demo.png");
        let summary = data["summary"].as_str().unwrap();
        assert!(summary.starts_with("Install Demo App\nAppImage"));
        assert!(summary.contains("\n2.0\nDoes demo things"));
        // AppImages carry everything they need.
        assert!(!summary.contains("Other changes may be required"));
    }

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
            adopts: None,
        };
        let preview =
            confirmation_preview(&operation, std::slice::from_ref(&package), &[], Some(&plan));
        assert_eq!(preview["action"], "Update Anonymous App");
        assert!(preview["summary"]
            .as_str()
            .unwrap()
            .starts_with("Update Anonymous App\nAPT, System\n1 → 2"));
        assert!(!preview["summary"]
            .as_str()
            .unwrap()
            .contains("Architecture:"));
        assert!(preview["summary"]
            .as_str()
            .unwrap()
            .contains("1 other package will change"));
        assert!(preview["summary"]
            .as_str()
            .unwrap()
            .contains("Removes: old-library"));
        assert!(preview["details"]
            .as_str()
            .unwrap()
            .contains("Remove old-library (1)"));
        let body = preview["body"].as_str().unwrap();
        assert!(body.contains("Source: APT"));
        assert!(body.contains("Scope: System"));
        assert!(body.contains("Remove old-library (1)"));
        assert!(!body.contains("Install anonymous"));
        assert!(!body.contains("unknown"));
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
        assert!(body.contains("Selected: Update anonymous (1 → 2)"));
        assert!(body.contains("Remove anonymous (1)"));
        assert!(body.contains("Download: 2048 bytes, Disk: -512 bytes"));
        assert!(preview["summary"]
            .as_str()
            .unwrap()
            .contains("Restart required"));
        let mut environment_package = package.clone();
        environment_package.id.scope = Scope::Environment {
            path: std::path::PathBuf::from("/synthetic/env"),
        };
        let environment_operation = Operation::Upgrade(environment_package.id.clone());
        let environment =
            confirmation_preview(&environment_operation, &[environment_package], &[], None);
        assert!(environment["details"]
            .as_str()
            .unwrap()
            .contains("Location: /synthetic/env"));
        assert!(!environment["summary"]
            .as_str()
            .unwrap()
            .contains("/synthetic/env"));
        let unavailable = confirmation_preview(&operation, &[package], &[], None);
        assert!(!unavailable["body"].as_str().unwrap().contains("unknown"));
        assert!(unavailable["summary"]
            .as_str()
            .unwrap()
            .contains("Other changes may be required"));
        let long = serde_json::from_value::<Package>(json!({
            "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
            "display_name":"An extremely long synthetic application name for narrow windows", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        let preview = confirmation_preview(&operation, std::slice::from_ref(&long), &[], None);
        assert!(preview["action"].as_str().unwrap().chars().count() <= 36);
        assert!(preview["summary"]
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
    fn partial_inventory_from_one_source_keeps_rows_and_reports_partial_state() {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"macos-apps", "name":"/Applications/Visible.app", "architecture":"unknown", "scope":"system"},
            "display_name":"Visible", "summary":"Application", "installed_version":"1", "update":"unknown"
        })).unwrap();
        let failure = BackendFailure {
            backend: "macos-apps".into(),
            error: EngineError::InvalidResponse {
                backend: "macos-apps".into(),
                reason: "skipped folder /Applications/Restricted: permission denied".into(),
            },
        };
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport {
                packages: vec![package.clone()],
                failures: vec![failure],
                successful_sources: vec![],
            })));
        assert_eq!(controller.rust().packages, vec![package]);
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["phase"], "partial");
        assert!(state["failures"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("/Applications/Restricted"));
        assert!(!controller.rust().last_success.contains_key("macos-apps"));
    }
    #[test]
    fn failed_update_check_keeps_known_updates_and_retry_merges_new_ones() {
        let package = |backend: &str| -> Package {
            serde_json::from_value(json!({
                "id": {"backend":backend, "name":"synthetic-tool", "architecture":"all", "scope":"system"},
                "display_name":"Synthetic tool", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
            }))
            .unwrap()
        };
        let failure = BackendFailure {
            backend: "codex".into(),
            error: EngineError::InvalidResponse {
                backend: "codex".into(),
                reason: "synthetic timeout".into(),
            },
        };
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().updates_view = true;
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport {
                packages: vec![package("apt")],
                failures: vec![failure.clone()],
                successful_sources: vec!["apt".into()],
            })));
        assert!(*controller.upgradable());
        assert_eq!(upgrade_plan(&controller.rust().packages).len(), 1);
        let apt_checked = controller.rust().last_success["apt"];

        controller.as_mut().apply(Ok(Payload::RetryFailedUpdates(
            vec!["codex".into()],
            PackageReport {
                packages: vec![],
                failures: vec![failure],
                successful_sources: vec![],
            },
        )));
        assert!(*controller.upgradable());
        assert_eq!(controller.rust().packages, vec![package("apt")]);
        assert_eq!(controller.rust().last_success["apt"], apt_checked);
        controller.as_mut().apply(Ok(Payload::UpgradePreview(
            vec![Operation::UpgradeAll {
                backend: "apt".into(),
            }],
            1,
            None,
        )));
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Updates from it are not included"));
        controller.as_mut().confirm(false);

        controller.as_mut().apply(Ok(Payload::RetryFailedUpdates(
            vec!["codex".into()],
            PackageReport {
                packages: vec![package("codex")],
                failures: vec![],
                successful_sources: vec!["codex".into()],
            },
        )));
        assert_eq!(controller.rust().packages.len(), 2);
        assert_eq!(upgrade_plan(&controller.rust().packages).len(), 2);
        assert!(controller.rust().failures.is_empty());
        assert_eq!(controller.rust().last_success["apt"], apt_checked);
        assert!(controller.rust().last_success.contains_key("codex"));
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
            let (source, report) = expect!(
                replies.pop().unwrap(),
                Reply::Done(Ok(Payload::RetryPackages(source, report))) => (source, report)
            );
            assert_eq!(source, "fixture");
            assert_eq!(report.packages.len(), count);
            assert_eq!(report.successful_sources, ["fixture"]);
        }
        let mut replies = Vec::new();
        execute(
            &mut engine,
            Job::RetryFailedUpdates(vec!["fixture".into()]),
            &cancel,
            &mut |reply| replies.push(reply),
        );
        assert!(
            matches!(replies.pop(), Some(Reply::Done(Ok(Payload::RetryFailedUpdates(sources, report))))
            if sources == ["fixture"] && report.packages == [package.clone()])
        );
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
            adopts: None,
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
        let preview: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert!(preview["summary"]
            .as_str()
            .unwrap()
            .contains("Removes: retired"));
        assert!(preview["details"]
            .as_str()
            .unwrap()
            .contains("Remove (1): retired"));
        assert!(
            confirmation.find("Remove (1): retired") < confirmation.find("Update all APT packages")
        );
        assert!(
            matches!(&controller.rust().pending, Some(Job::UpgradeAll(_, Some(saved))) if *saved == plan)
        );
    }
    #[test]
    fn background_inventory_populates_both_sections_without_foreground_changes() {
        // An Updates read refreshes indexes first, so it can fill Installed
        // too. A plain Installed read cannot stand in for Updates.
        for (read, updates_cached) in [("Updates", true), ("Installed", false)] {
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            controller.as_mut().rust_mut().prefetch.clear();
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
                    Job::Load(read.into(), "".into()),
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
                job: Job::Load(read.into(), "".into()),
            });
            controller.as_mut().poll();
            assert_eq!(controller.rows().to_string(), "original");
            assert!(!controller.busy());
            assert_eq!(
                controller
                    .rust()
                    .prefetched
                    .contains_key(&cache_key("Updates", "", &[], false)),
                updates_cached,
                "{read}"
            );
            // Installed opens from the cache and keeps native updates.
            controller
                .as_mut()
                .load("Installed".into(), "".into(), "".into(), false, false);
            assert!(controller.rust().worker.is_none(), "{read}");
            assert_eq!(controller.rust().packages.len(), 1);
            assert_eq!(
                controller.rust().packages[0].update,
                UpdateAvailability::Available
            );
        }
    }

    #[test]
    fn a_failed_update_refresh_does_not_warm_installed() {
        let installed = cache_key("Installed", "", &[], false);
        let updates = cache_key("Updates", "", &[], false);
        let mut report = PackageReport::default();
        report.failures.push(BackendFailure {
            backend: "homebrew".into(),
            error: EngineError::Cancelled,
        });
        for (refreshed, installed_cached) in [(true, false), (false, true)] {
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            controller
                .as_mut()
                .warm_inventory(report.clone(), refreshed);
            let cached = &controller.rust().prefetched;
            assert_eq!(cached.contains_key(&installed), installed_cached);
            assert_eq!(cached.contains_key(&updates), refreshed);
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
    fn stale_snapshot_stays_whole_while_its_refresh_streams() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Installed", "", &[], false);
        let mut cached = cached_view("cached package");
        cached.expired = true;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key, cached);
        let (_sender, receiver) = mpsc::channel();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Sources".into(), "".into()),
        });
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        assert!(controller.rows().to_string().contains("cached package"));
        assert!(controller.rust().hold_partials);
        // The refresh streams one backend's rows before the rest.
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Partial(PackageReport::default()))
            .unwrap();
        controller.as_mut().rust_mut().background = false;
        controller.as_mut().rust_mut().queued = None;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| thread::sleep(Duration::from_millis(200))),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Installed".into(), "".into()),
        });
        controller.as_mut().poll();
        assert!(controller.rows().to_string().contains("cached package"));
        // A fresh visit streams partials as usual.
        controller
            .as_mut()
            .load("Search".into(), "query".into(), "".into(), false, false);
        assert!(!controller.rust().hold_partials);
    }

    /// A fresh directory for one test, removed when it ends.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pkgdeck-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for_file(path: &std::path::Path, contains: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(path).is_ok_and(|text| text.contains(contains)) {
            assert!(Instant::now() < deadline, "never held {contains}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn installed(controller: &mut Pin<&mut ffi::PackageController>, name: &str) -> String {
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":name, "architecture":"all", "scope":"system"},
            "display_name":name, "summary":"Kept package", "installed_version":"1", "update":"current"
        }))
        .unwrap();
        controller.as_mut().rust_mut().packages = vec![package];
        controller
            .as_mut()
            .set_rows(json!([{"kind":"package","name":name}]).to_string().into());
        cache_key("Installed", "", &[], false)
    }

    #[test]
    fn kept_sections_show_at_once_on_the_next_launch() {
        let dir = Scratch::new("kept-views");
        let path = dir.path().join("pkgdeck/views.json");
        let mut first = ffi::create_controller();
        let mut first = first.pin_mut();
        first
            .as_mut()
            .rust_mut()
            .open_view_store(Some(path.clone()));
        let key = installed(&mut first, "kept-tool");
        first.as_mut().stash_current(key.clone());
        wait_for_file(&path, "kept-tool");
        assert!(!path.with_extension("json.tmp").exists());

        let mut next = ffi::create_controller();
        let mut next = next.pin_mut();
        next.as_mut().rust_mut().open_view_store(Some(path));
        let cached = next.rust().view_cache.get(&key).unwrap();
        assert!(cached.stale());
        assert_eq!(cached.packages[0].id.name, "kept-tool");
        next.as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        assert!(next.rows().to_string().contains("kept-tool"));
        assert!(next
            .report_state()
            .to_string()
            .contains(r#""phase":"stale""#));
    }

    #[test]
    fn searches_and_sources_are_not_kept() {
        let dir = Scratch::new("unkept-views");
        let path = dir.path().join("views.json");
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .rust_mut()
            .open_view_store(Some(path.clone()));
        installed(&mut controller, "searched-tool");
        controller
            .as_mut()
            .stash_current(cache_key("Search", "searched", &[], false));
        controller
            .as_mut()
            .stash_current(cache_key("Sources", "", &[], false));
        // A section with a failed source keeps its rows, not the failure.
        let key = installed(&mut controller, "partly-checked-tool");
        controller.as_mut().rust_mut().failures = vec![BackendFailure {
            backend: "snap".into(),
            error: EngineError::Execution(ExecutionError::Cancelled),
        }];
        controller.as_mut().stash_current(key);
        wait_for_file(&path, "partly-checked-tool");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("searched-tool"));
        assert!(!text.contains("Sources"));
        assert!(!text.contains("snap"));
    }

    #[test]
    fn an_older_write_never_replaces_a_newer_one() {
        let dir = Scratch::new("ordered-views");
        let path = dir.path().join("views.json");
        let written = std::sync::Mutex::new(0);
        let stored = |name: &str| StoredViews {
            version: pkgdeck_core::VERSION.into(),
            views: vec![StoredView {
                key: name.into(),
                packages: vec![],
                cleanup: vec![],
                rows: "[]".into(),
                status: String::new(),
                report_state: String::new(),
                upgradable: false,
                updates_view: false,
            }],
        };
        assert!(write_views(&path, &stored("newer"), 2, &written));
        assert!(!write_views(&path, &stored("older"), 1, &written));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("newer") && !text.contains("older"));
        // A file in the way of the directory is a failed write, not a crash.
        let blocked = dir.path().join("views.json").join("views.json");
        assert!(!write_views(&blocked, &stored("blocked"), 3, &written));
    }

    #[test]
    fn a_damaged_or_older_store_is_ignored() {
        let dir = Scratch::new("damaged-views");
        let key = cache_key("Installed", "", &[], false);
        for text in [
            "not json".to_owned(),
            json!({"version": "0.0.1", "views": [{"key": key, "packages": [], "cleanup": [], "rows": "[]", "status": "", "report_state": "", "upgradable": false, "updates_view": false}]}).to_string(),
        ] {
            let path = dir.path().join("views.json");
            std::fs::write(&path, text).unwrap();
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            controller.as_mut().rust_mut().open_view_store(Some(path));
            assert!(controller.rust().view_cache.get(&key).is_none());
        }
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().open_view_store(None);
        assert!(controller.rust().view_store.is_none());
    }

    #[test]
    fn update_all_retry_replaces_the_saved_updates_section() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Updates", "", &[], false);
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key.clone(), cached_view("before retry"));
        controller.as_mut().rust_mut().view_cache.expire();
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":"retried-tool", "architecture":"all", "scope":"system"},
            "display_name":"retried-tool", "summary":"Retried package", "installed_version":"1", "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Ok(Payload::RetryFailedUpdates(
                vec!["apt".into()],
                PackageReport {
                    packages: vec![package],
                    failures: vec![],
                    successful_sources: vec!["apt".into()],
                },
            ))))
            .unwrap();
        let handle = thread::spawn(|| {});
        while !handle.is_finished() {
            thread::yield_now();
        }
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel: Cancellation::default(),
            job: Job::RetryFailedUpdates(vec!["apt".into()]),
        });
        controller.as_mut().poll();
        let saved = controller.rust().view_cache.get(&key).unwrap();
        assert!(!saved.stale());
        assert!(saved.rows.to_string().contains("retried-tool"));
        assert!(!saved.rows.to_string().contains("before retry"));
    }

    #[test]
    fn forced_reload_of_the_open_section_keeps_its_rows_until_complete() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().active_view = "Updates".into();
        controller.as_mut().set_rows(r#"[{"name":"shown"}]"#.into());
        let (_sender, receiver) = mpsc::channel();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Updates".into(), "".into()),
        });
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, true);
        assert!(controller.rust().hold_partials);
        // Reloading again before the refresh finishes still keeps them.
        assert_eq!(controller.rows().to_string(), "[]");
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, true);
        assert!(controller.rust().hold_partials);
        controller.as_mut().rust_mut().active_view = "Installed".into();
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, true);
        assert!(!controller.rust().hold_partials);
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
        assert_eq!(state["notify"], true);
        assert_eq!(controller.rows().to_string(), "current view");
        controller.as_mut().acknowledge_notification();
        let acknowledged = controller.notification_history().to_string();
        // A restart restores what was already announced.
        let mut restarted = ffi::create_controller();
        let mut restarted = restarted.pin_mut();
        restarted
            .as_mut()
            .restore_notification_history(acknowledged.as_str().into());
        assert_eq!(restarted.notification_history().to_string(), acknowledged);
        restarted.as_mut().finish_background_check(report.clone());
        let state: Value = serde_json::from_str(&restarted.background_state().to_string()).unwrap();
        assert_eq!(state["notify"], false);
        controller.as_mut().finish_background_check(report.clone());
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["notify"], false);

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
    fn background_worker_error_updates_failure_state_without_losing_known_count() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_background_state(encoded(json!({
            "last_check": 1, "available": 3, "failures": [], "notify": true
        })));
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Err(EngineError::Unavailable {
                backend: "fixture".into(),
                reason: "temporarily offline".into(),
            })))
            .unwrap();
        drop(sender);
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::BackgroundUpdates(vec!["fixture".into()]),
        });
        controller.as_mut().poll();
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["available"], 3);
        assert_eq!(state["notify"], false);
        assert_eq!(state["failures"][0]["kind"], "unavailable");
        assert!(state["last_check"].as_u64().unwrap() > 1);
        assert!(!controller.busy());

        controller
            .as_mut()
            .finish_background_error(&EngineError::Incomplete(vec![
                BackendFailure {
                    backend: "apt".into(),
                    error: EngineError::Unavailable {
                        backend: "apt".into(),
                        reason: "offline".into(),
                    },
                },
                BackendFailure {
                    backend: "flatpak".into(),
                    error: EngineError::Cancelled,
                },
            ]));
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["available"], 3);
        assert_eq!(state["failures"][0]["source"], "apt");
        assert_eq!(state["failures"][1]["kind"], "cancelled");
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
            .contains("Autostart couldn't be changed"));
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
        controller.as_mut().rust_mut().activity_store =
            Some(ActivityStore::at(path.join("activity.json")));
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
        // Activity is read on its own thread and published by poll().
        assert!(*controller.needs_poll());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !controller.activity().to_string().contains("queued") {
            controller.as_mut().poll_activity();
            thread::sleep(Duration::from_millis(2));
        }
        assert!(controller.activity().to_string().contains("queued"));
        assert!(controller
            .activity()
            .to_string()
            .contains("Update first (fixture)"));
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
        controller.as_mut().rust_mut().activity_store =
            Some(ActivityStore::at(path.join("activity.json")));
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
        controller.as_mut().rust_mut().activity_store =
            Some(ActivityStore::at(path.join("activity.json")));
        controller.as_mut().start_confirmed(Confirmed {
            job: Job::Write(operation, None),
            activity_id: Some(id),
            cleanup_preview: vec![],
        });
        assert_eq!(store.entries().unwrap()[0].state, State::Running);
        wait_until(&mut controller, |controller| {
            controller.rust().worker.is_none()
        });
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
            adopts: None,
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
        // The banner asks for another review instead of reporting a failure.
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["kind"], "info");
        assert!(notice["title"]
            .as_str()
            .unwrap()
            .contains("Review them again"));
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
        assert_eq!(source_status(&failed), "Cancelled.");
        let clean = Operation::Clean(CleanupId {
            backend: "apt".into(),
            key: "autoremove".into(),
        });
        assert_eq!(operation_label(&clean), "Clean autoremove (APT)");
        assert!(confirmation_label(&clean, &[]).contains("Clean autoremove (APT)"));
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
            adopt_with: None,
        };
        let other = firmware("other-device");
        let target = firmware("target-device");
        let label = confirmation_label(&Operation::Upgrade(target.id.clone()), &[other, target]);
        assert!(label.contains("target-device: synthetic firmware"));
        assert!(!label.contains("other-device: synthetic firmware"));
    }
    #[test]
    fn jobs_route_to_their_own_sources_and_errors_have_a_kind() {
        let filter = vec!["filtered".to_owned()];
        let id = synthetic_package("editor", "Editor").id;
        let upgrade = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        let clean = Operation::Clean(CleanupId {
            backend: "flatpak".into(),
            key: "unused".into(),
        });
        for (job, sources) in [
            (Job::RetryFailedUpdates(vec!["snap".into()]), vec!["snap"]),
            (Job::BackgroundUpdates(vec!["dnf".into()]), vec!["dnf"]),
            (Job::PlanCleanAll(vec![clean.clone()]), vec!["flatpak"]),
            (
                Job::ManifestPreview(PathBuf::from("inventory.json")),
                vec![],
            ),
            (
                Job::PlanUpgrade(vec![upgrade("flatpak"), upgrade("apt")], 2),
                vec!["apt"],
            ),
            (
                Job::PlanUpgrade(vec![upgrade("flatpak")], 1),
                vec!["filtered"],
            ),
            (Job::Details(id.clone()), vec!["apt"]),
        ] {
            assert_eq!(engine_source(&job, &filter), sources);
        }

        let install = Operation::Install(id.clone());
        assert!(same_target(&install, &Operation::Remove(id.clone())));
        assert!(same_target(&clean, &clean.clone()));
        assert!(!same_target(
            &clean,
            &Operation::Clean(CleanupId {
                backend: "flatpak".into(),
                key: "other".into(),
            })
        ));
        assert!(same_target(&upgrade("apt"), &install));
        assert!(!same_target(&clean, &install));

        let source = |availability| Source {
            backend: "flatpak".into(),
            capabilities: vec![],
            availability,
        };
        let restricted = source_row(&source(Ok(Availability::Unavailable(
            "disabled by the administrator".into(),
        ))));
        assert_eq!(restricted["availability_kind"], "restricted");
        let missing = source_row(&source(Ok(Availability::Unavailable(
            "not installed".into(),
        ))));
        assert_eq!(missing["availability_kind"], "unavailable");
        assert_eq!(restricted["available"], false);
        for (error, kind) in [
            (
                EngineError::Unsupported {
                    backend: "flatpak".into(),
                    capability: Capability::Search,
                },
                "unsupported",
            ),
            (
                EngineError::Execution(ExecutionError::AuthorizationDenied),
                "authorization",
            ),
            (
                EngineError::Execution(ExecutionError::AuthorizationCancelled),
                "authorization",
            ),
            (EngineError::Execution(ExecutionError::LockBusy), "locked"),
        ] {
            assert_eq!(failure_kind(&error), kind);
            let row = source_row(&source(Err(error)));
            assert_eq!(row["availability_kind"], kind);
            assert_eq!(row["check_failed"], true);
        }
        assert_eq!(
            plain_text("host command failed (exit 1):", None),
            "The package manager reported an error without details."
        );
        assert_eq!(
            plain_text("host command failed (exit 1):", Some("snap")),
            "Snap reported an error without details."
        );
        assert_eq!(
            plain_text("host command failed (exit 1): E: broken index", None),
            "Broken index."
        );
    }
    #[test]
    fn removal_confirmation_lists_the_selected_and_other_native_changes() {
        let change = |action, name: &str, old: Option<&str>, new: Option<&str>| PlannedChange {
            action,
            name: name.into(),
            installed_version: old.map(Into::into),
            candidate_version: new.map(Into::into),
        };
        let mut editor = synthetic_package("editor", "");
        editor.candidate_version = Some("2".into());
        let remove = Operation::Remove(editor.id.clone());
        let plan = TransactionPlan {
            operation: remove.clone(),
            native_preview: String::new(),
            changes: vec![
                change(PlannedAction::Remove, "editor", Some("1"), Some("2")),
                change(PlannedAction::Install, "libnew", None, Some("3")),
                change(PlannedAction::Upgrade, "libsame", Some("4"), Some("4")),
                change(PlannedAction::Remove, "libgone", None, None),
            ],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: None,
        };
        let preview = confirmation_preview(&remove, &[editor.clone()], &[], Some(&plan));
        assert_eq!(preview["action"], "Remove editor");
        assert_eq!(
            preview["summary"],
            "Remove editor\nAPT, System\n1\n3 other packages will change\nRemoves: libgone\nApp data may remain after removal."
        );
        let body = preview["body"].as_str().unwrap();
        assert!(body.contains("Selected: Remove editor (1)"), "{body}");
        assert!(
            body.contains("Other changes:\nInstall libnew (3)\nUpdate libsame (4)\nRemove libgone"),
            "{body}"
        );
        // A source-wide change has no selected package among the plan.
        let clean = Operation::Clean(CleanupId {
            backend: "apt".into(),
            key: "autoremove".into(),
        });
        let preview = confirmation_preview(&clean, &[editor.clone()], &[], Some(&plan));
        assert!(preview["summary"]
            .as_str()
            .unwrap()
            .contains("4 other packages will change"));
        assert!(!preview["body"].as_str().unwrap().contains("Selected:"));
        // A Flatpak reference only offers a system or user scope.
        let mut reference = editor;
        reference.id.backend = "flatpak".into();
        reference.id.reference = Some("flatpakref:https://example.invalid/app.flatpakref".into());
        reference.id.scope = Scope::Environment {
            path: "/opt/flatpak".into(),
        };
        let install = Operation::Install(reference.id.clone());
        let preview = confirmation_preview(&install, &[reference], &[], None);
        assert_eq!(preview["flatpak_ref_scope"], "");
        assert!(preview["body"]
            .as_str()
            .unwrap()
            .contains("Location: /opt/flatpak"));
    }
    #[test]
    fn activity_reads_report_an_unreadable_lock() {
        let temp = pkgdeck_tools::Temp::new();
        let file = temp.0.join("not-a-directory");
        std::fs::write(&file, "").unwrap();
        let error = read_activity(&file.join("activity.json")).unwrap_err();
        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(read_activity(&temp.0.join("activity.json"))
            .unwrap()
            .is_empty());
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
    fn sections_preload_from_startup_in_order() {
        let mut controller = Controller::default();
        assert_eq!(controller.active_view, "Search");
        let order: Vec<_> = std::iter::from_fn(|| controller.next_prefetch()).collect();
        assert_eq!(order, ["Installed", "Updates", "Clean", "Sources"]);
        assert!(controller.prefetch.is_empty());
        controller.invalidate_prefetch();
        assert_eq!(controller.prefetch.len(), 4);
    }
    fn synthetic_package(name: &str, display_name: &str) -> Package {
        Package {
            id: PackageId {
                backend: "apt".into(),
                name: name.into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: display_name.into(),
            summary: "Synthetic package".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }
    fn wait_until(
        controller: &mut Pin<&mut ffi::PackageController>,
        done: impl Fn(&ffi::PackageController) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !done(controller) {
            controller.as_mut().poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(done(controller));
    }
    fn fake_prefetch(view: &str, key: String, replies: Vec<Reply>, stale: bool) -> PrefetchWorker {
        let (sender, receiver) = mpsc::channel();
        for reply in replies {
            sender.send(reply).unwrap();
        }
        PrefetchWorker {
            handle: thread::spawn(move || drop(sender)),
            receiver,
            cancel: Cancellation::default(),
            view: view.into(),
            key,
            stale,
            asked: vec![],
            answered: vec![],
            scope: (vec![], false, Instant::now()),
        }
    }
    #[test]
    fn finished_changes_explain_what_happened() {
        use pkgdeck_core::process::{Completion, ExecutionError as E};
        let install = Operation::Install(synthetic_package("htop", "htop").id);
        let job = Job::Write(install.clone(), None);
        assert_eq!(operation_title(&install), "Install htop (APT)");
        assert_eq!(
            operation_title(&Operation::Refresh {
                backend: "apt".into()
            }),
            "Refresh APT package lists"
        );
        let success = write_notice(
            &job,
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &Names::new(),
        );
        assert_eq!(
            success,
            json!({"kind": "success", "title": "Install htop (APT) finished",
                "operation": "install", "package": "htop", "source": "apt", "source_name": "APT",
                "undo": true, "undo_action": "remove",
                "target": {"source": "apt", "name": "htop", "architecture": "all",
                    "remote": null, "scope": "system", "reference": null}})
        );
        // A remembered display name is what people see.
        let names = Names::from([(
            synthetic_package("htop", "htop").id,
            "Process Viewer".to_owned(),
        )]);
        let named = write_notice(
            &job,
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &names,
        );
        assert_eq!(named["title"], "Install Process Viewer (APT) finished");
        assert_eq!(named["package"], "Process Viewer");
        // Undo is offered only for a finished install or removal.
        let failed = write_notice(&job, &Err(EngineError::NotFound), false, &names);
        assert_eq!(failed["undo"], false);
        let removal = write_notice(
            &Job::Write(
                Operation::Remove(synthetic_package("htop", "htop").id),
                None,
            ),
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &names,
        );
        assert_eq!(removal["undo_action"], "install");
        let update = write_notice(
            &Job::Write(
                Operation::Upgrade(synthetic_package("htop", "htop").id),
                None,
            ),
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &names,
        );
        assert_eq!(update["operation"], "update");
        assert_eq!(update["undo"], false);
        let denied = write_notice(
            &job,
            &Err(E::AuthorizationDenied.into()),
            false,
            &Names::new(),
        );
        assert_eq!(denied["kind"], "error");
        assert_eq!(denied["title"], "Install htop (APT) failed");
        assert!(denied["detail"]
            .as_str()
            .unwrap()
            .contains("password prompt"));
        let sudo = write_notice(
            &job,
            &Err(E::AuthorizationDenied.into()),
            true,
            &Names::new(),
        );
        assert!(sudo["detail"].as_str().unwrap().contains("sudo login"));
        for (error, expected) in [
            (E::AuthorizationCancelled, "prompt was closed"),
            (E::LockBusy, "Another package manager is running"),
            (E::Interrupted, "interrupted"),
            (
                E::Failed(Completion {
                    code: Some(100),
                    signal: None,
                    stdout: vec![],
                    stderr: b"E: one\n\nE: two\n".to_vec(),
                    truncated: false,
                    cancellation_deferred: false,
                }),
                "APT couldn't run: one.",
            ),
            (
                E::Failed(Completion {
                    code: Some(1),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                }),
                "APT reported an error without details.",
            ),
            (E::TimedOut, "took too long"),
        ] {
            let notice = write_notice(&job, &Err(error.into()), false, &Names::new());
            assert!(
                notice["detail"].as_str().unwrap().contains(expected),
                "{notice}"
            );
        }
        let cancelled = write_notice(&job, &Err(EngineError::Cancelled), false, &Names::new());
        assert_eq!(cancelled["kind"], "info");
        let batch = Job::UpgradeAll(
            vec![
                Operation::UpgradeAll {
                    backend: "apt".into(),
                },
                Operation::UpgradeAll {
                    backend: "flatpak".into(),
                },
            ],
            None,
        );
        let partial = write_notice(
            &batch,
            &Ok(Payload::Batch(
                "Completed 1 of 2 updates.\nUpdate all APT packages: Completed\nUpdate all Flatpak packages: busy".into(),
                vec![Outcome::Finished, Outcome::Failed],
            )),
            false, &Names::new(),
        );
        // One failure names the source, without the step's long title.
        assert_eq!(partial["title"], "Flatpak didn't update");
        assert_eq!(partial["detail"], "Flatpak: busy");
        let cancelled_batch = write_notice(
            &batch,
            &Ok(Payload::Batch(
                String::new(),
                vec![Outcome::Finished, Outcome::Cancelled],
            )),
            false,
            &Names::new(),
        );
        assert_eq!(cancelled_batch["kind"], "info");
        assert_eq!(
            cancelled_batch["title"],
            "1 of 2 changes finished. The rest were cancelled."
        );
        let mixed = write_notice(
            &batch,
            &Ok(Payload::Batch(
                "Completed 0 of 2 updates.\nx: busy".into(),
                vec![Outcome::Failed, Outcome::Cancelled],
            )),
            false,
            &Names::new(),
        );
        assert_eq!(mixed["kind"], "error");
        let denied_batch = write_notice(
            &batch,
            &Ok(Payload::Batch(
                "Completed 0 of 2 updates.\nUpdate all APT packages: authorization denied or unavailable\nUpdate all Flatpak packages: Completed".into(),
                vec![Outcome::Failed, Outcome::Finished],
            )),
            false, &Names::new(),
        );
        assert_eq!(denied_batch["kind"], "error");
        assert!(denied_batch["detail"]
            .as_str()
            .unwrap()
            .contains("password prompt"));
        let denied_sudo = write_notice(
            &batch,
            &Ok(Payload::Batch(
                "Completed 0 of 1 updates.\nUpdate all packages from apt: authorization denied or unavailable".into(),
                vec![Outcome::Failed],
            )),
            true, &Names::new(),
        );
        assert!(denied_sudo["detail"]
            .as_str()
            .unwrap()
            .contains("sudo login"));
        // A failed preview says so; a superseded one stays quiet.
        let plan = Job::PlanOperation(install.clone());
        let prepared = preflight_notice(&plan, &E::LockBusy.into(), false, &Names::new()).unwrap();
        assert_eq!(prepared["title"], "Install htop (APT) couldn't be prepared");
        let denied_plan = preflight_notice(
            &Job::PlanUpgrade(vec![], 0),
            &E::AuthorizationDenied.into(),
            false,
            &Names::new(),
        )
        .unwrap();
        assert_eq!(denied_plan["title"], "The update couldn't be prepared");
        assert_eq!(
            preflight_notice(
                &Job::PlanCleanAll(vec![]),
                &E::TimedOut.into(),
                false,
                &Names::new()
            )
            .unwrap()["title"],
            "The cleanup couldn't be prepared"
        );
        assert!(preflight_notice(&plan, &EngineError::Cancelled, false, &Names::new()).is_none());
        assert!(preflight_notice(&job, &E::LockBusy.into(), false, &Names::new()).is_none());
        let all = write_notice(
            &batch,
            &Ok(Payload::Batch(
                "Completed 2 of 2 updates.".into(),
                vec![Outcome::Finished; 2],
            )),
            false,
            &Names::new(),
        );
        assert_eq!(all["title"], "2 changes finished");
        let firmware = write_notice(
            &job,
            &Ok(Payload::Batch(
                "Firmware update finished. Restart or shut down the device if required.".into(),
                vec![Outcome::Finished],
            )),
            false,
            &Names::new(),
        );
        assert_eq!(firmware["kind"], "success");
        assert!(firmware["detail"]
            .as_str()
            .unwrap()
            .contains("Restart or shut down"));
        let batch_done = write_notice(
            &batch,
            &Ok(Payload::Batch(
                "Completed 2 of 2 updates.\nUpdate all packages from apt: Completed\nUpdate all packages from flatpak: Finished before it could be cancelled. Changes were kept.".into(),
                vec![Outcome::Finished; 2],
            )),
            false, &Names::new(),
        );
        assert_eq!(
            batch_done["detail"],
            "Update all packages from flatpak: Finished before it could be cancelled. Changes were kept."
        );
        let deferred = write_notice(
            &job,
            &Ok(Payload::Written(OperationOutcome {
                cancellation_deferred: true,
            })),
            false,
            &Names::new(),
        );
        assert!(deferred["detail"]
            .as_str()
            .unwrap()
            .contains("before it could be cancelled"));
        let repository = write_notice(
            &Job::Repositories(None),
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &Names::new(),
        );
        assert_eq!(repository["title"], "The change finished");
        // The banner clears on request.
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_notice(encoded(denied));
        controller.as_mut().dismiss_notice();
        assert_eq!(controller.notice().to_string(), "{}");
    }
    #[test]
    fn opening_a_package_never_makes_the_page_busy() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
        let mut package = synthetic_package("grok", "Grok");
        package.id.backend = "grok".into();
        controller.as_mut().rust_mut().packages = vec![package];
        controller.as_mut().select(0);
        // Details load beside the page: actions stay enabled meanwhile.
        assert!(controller.details().to_string().contains("Grok"));
        assert!(controller.rust().details_worker.is_some());
        assert!(controller.rust().worker.is_none());
        assert!(!*controller.busy());
    }
    #[test]
    fn selecting_during_a_search_loads_details_alongside_it() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        // A standalone tool keeps both jobs cheap and read-only.
        controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
        let mut package = synthetic_package("grok", "Grok");
        package.id.backend = "grok".into();
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        controller
            .as_mut()
            .start(Job::Load("Search".into(), "zzz".into()));
        controller.as_mut().select(0);
        // The row shows at once; full details load on their own worker.
        assert!(controller.details().to_string().contains("Grok"));
        assert!(controller.rust().details_worker.is_some());
        assert!(controller.rust().queued.is_none());
        // Choosing that row again waits for the lookup already running.
        let cancel = controller
            .rust()
            .details_worker
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        controller.as_mut().select(0);
        assert!(!cancel.requested());
        assert!(controller
            .rust()
            .worker
            .as_ref()
            .is_some_and(|worker| { matches!(worker.job, Job::Load(..)) }));
        assert!(controller.rust().queued.is_none());
        // A reply that arrives while the next search is queued is kept, so
        // opening the package again does not query the manager a second time.
        controller.as_mut().rust_mut().queued = Some(Job::Load("Search".into(), "next".into()));
        controller.as_mut().set_details("{}".into());
        controller
            .as_mut()
            .apply(Ok(Payload::Details(Box::new(PackageDetails {
                package: package.clone(),
                description: "Full Grok details".into(),
                homepage: None,
                dependencies: vec![],
            }))));
        assert_eq!(controller.details().to_string(), "{}");
        assert!(controller
            .rust()
            .detail_cache
            .get(&package.id)
            .is_some_and(|cached| cached.to_string().contains("Full Grok details")));
        controller.as_mut().rust_mut().queued = None;
        controller.as_mut().select(0);
        assert!(controller
            .details()
            .to_string()
            .contains("Full Grok details"));
        // A reload drops the lookup so its reply can't refill the cache.
        controller.as_mut().rust_mut().invalidate_details();
        assert!(controller.rust().details_worker.is_none());
        // Selecting again replaces the pending lookup.
        controller.as_mut().select(0);
        wait_until(&mut controller, |c| {
            c.rust().details_worker.is_none() && c.rust().worker.is_none()
        });
        assert_eq!(controller.rust().selected.as_ref(), Some(&package.id));
    }
    #[test]
    fn cancel_stops_the_parallel_details_lookup() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let package = synthetic_package("fixture", "Fixture");
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        controller.as_mut().rust_mut().selected = Some(package.id.clone());
        let (sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        sender
            .send(Reply::DetailsPreview(Box::new(PackageDetails {
                package: package.clone(),
                description: "should not land".into(),
                homepage: None,
                dependencies: vec![],
            })))
            .unwrap();
        controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            id: package.id.clone(),
        });
        controller.as_mut().cancel();
        assert!(cancel.requested());
        assert!(controller.rust().details_worker.is_none());
        controller.as_mut().poll();
        assert!(controller.rust().detail_cache.is_empty());
        assert!(!controller.details().to_string().contains("should not land"));
        assert!(controller.status().to_string().contains("Cancelling"));
        // The channel was dropped with the worker, so nothing further can land.
        assert!(sender
            .send(Reply::Done(Ok(Payload::Details(Box::new(
                PackageDetails {
                    package,
                    description: "still should not land".into(),
                    homepage: None,
                    dependencies: vec![],
                }
            )))))
            .is_err());
    }
    #[test]
    fn resting_on_a_row_loads_its_details_before_it_opens() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let first = fixture_row();
        let mut second = fixture_row();
        second.id.name = "other".into();
        controller.as_mut().rust_mut().packages = vec![first.clone(), second.clone()];
        let loading = |controller: &ffi::PackageController| {
            controller
                .rust()
                .details_worker
                .as_ref()
                .map(|worker| worker.id.clone())
        };
        controller.as_mut().warm_details(-1);
        assert_eq!(loading(&controller), None);
        controller.as_mut().warm_details(0);
        assert_eq!(loading(&controller), Some(first.id.clone()));
        controller.as_mut().warm_details(0);
        assert_eq!(loading(&controller), Some(first.id.clone()));
        // Moving on to another row looks that one up instead.
        controller.as_mut().warm_details(1);
        assert_eq!(loading(&controller), Some(second.id.clone()));
        // Opening it shows what the row says while the same lookup finishes.
        controller.as_mut().select(1);
        assert_eq!(loading(&controller), Some(second.id.clone()));
        let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
        assert_eq!(details["more"], true);
        // The open package's lookup is never replaced.
        controller.as_mut().warm_details(0);
        assert_eq!(loading(&controller), Some(second.id.clone()));
        let started = Instant::now();
        while controller.rust().details_worker.is_some() {
            assert!(started.elapsed() < Duration::from_secs(10));
            controller.as_mut().poll();
            thread::sleep(Duration::from_millis(5));
        }
        let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
        assert_ne!(details["more"], true);
        // Known, open, or behind a change: nothing to look up.
        controller
            .as_mut()
            .rust_mut()
            .detail_cache
            .insert(second.id.clone(), "{}".into());
        controller.as_mut().rust_mut().selected = None;
        controller.as_mut().warm_details(1);
        assert_eq!(loading(&controller), None);
        controller.as_mut().rust_mut().detail_cache.clear();
        controller.as_mut().rust_mut().selected = Some(second.id.clone());
        controller.as_mut().warm_details(1);
        assert_eq!(loading(&controller), None);
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Write(Operation::Install(first.id.clone()), None),
            vec![],
        ));
        controller.as_mut().warm_details(0);
        assert_eq!(loading(&controller), None);
        controller.as_mut().rust_mut().worker = None;
    }
    #[test]
    fn parallel_details_failure_replaces_the_row_preview() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let package = synthetic_package("fixture", "Fixture");
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        controller.as_mut().rust_mut().selected = Some(package.id.clone());
        controller.as_mut().set_details(encoded(json!({
            "package": package_row(&package, &[], None),
            "description": package.summary,
        })));
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Err(EngineError::Unavailable {
                backend: "homebrew".into(),
                reason: "brew is not installed".into(),
            })))
            .unwrap();
        controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            id: package.id.clone(),
        });
        controller.as_mut().poll();
        let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
        assert!(details["description"]
            .as_str()
            .unwrap()
            .contains("Details couldn't be loaded"));
        assert!(details["description"]
            .as_str()
            .unwrap()
            .contains("brew is not installed"));
        assert_eq!(details["package"]["name"], "fixture");
        assert!(controller.rust().detail_cache.is_empty());
        // A cancelled lookup stays quiet and leaves the preview in place.
        controller.as_mut().set_details("{}".into());
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Err(EngineError::Cancelled)))
            .unwrap();
        controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            id: package.id.clone(),
        });
        controller.as_mut().poll();
        assert_eq!(controller.details().to_string(), "{}");
        // A failure for a package that left the list has no row to explain.
        controller.as_mut().rust_mut().packages.clear();
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Reply::Done(Err(EngineError::NotFound)))
            .unwrap();
        controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            id: package.id,
        });
        let status = controller.status().to_string();
        controller.as_mut().poll();
        assert_eq!(controller.details().to_string(), "{}");
        assert_eq!(controller.status().to_string(), status);
    }
    #[test]
    fn finished_preloads_fill_sections_and_the_source_catalog() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        // A pending review blocks new preloads, so only these fakes run.
        controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
        let mut inventory = PackageReport::default();
        inventory.packages.push(synthetic_package("warm", "warm"));
        let sources = vec![Source {
            backend: "apt".into(),
            capabilities: vec![Capability::Search],
            availability: Ok(Availability::Available),
        }];
        let key = cache_key("Sources", "", &[], false);
        controller.as_mut().rust_mut().prefetch_worker = Some(fake_prefetch(
            "Sources",
            key.clone(),
            vec![
                Reply::Inventory(inventory, true),
                Reply::Done(Ok(Payload::Sources(sources))),
            ],
            false,
        ));
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        assert!(controller.rust().prefetched.contains_key(&key));
        assert!(controller
            .rust()
            .prefetched
            .contains_key(&cache_key("Updates", "", &[], false)));
        assert!(controller.rust().catalog_checked);
        assert!(controller.source_catalog().to_string().contains("apt"));
        // A preload made stale by a write or filter change is dropped.
        controller.as_mut().rust_mut().prefetched.clear();
        let key = cache_key("Clean", "", &[], false);
        controller.as_mut().rust_mut().prefetch_worker = Some(fake_prefetch(
            "Clean",
            key.clone(),
            vec![Reply::Done(Ok(Payload::Cleanup(CleanupReport::default())))],
            true,
        ));
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        assert!(controller.rust().prefetched.is_empty());
    }
    #[test]
    fn open_sections_are_reloaded_when_their_preload_gets_old() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
        controller.as_mut().rust_mut().prefetch.clear();
        let fresh = cached_view("fresh");
        let mut old = cached_view("old");
        old.loaded = Instant::now() - REWARM_AFTER;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(cache_key("Installed", "", &[], false), old);
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(cache_key("Clean", "", &[], false), fresh);
        controller.as_mut().rust_mut().last_rewarm = Instant::now() - Duration::from_secs(60);
        controller.as_mut().poll();
        // Installed (old), Updates and Sources (never loaded) reload,
        // Installed first because it also fills Updates.
        let mut order = controller.rust().prefetch.clone();
        order.reverse();
        assert_eq!(order, ["Installed", "Updates", "Sources"]);
        // Checked at most every 30 seconds.
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().poll();
        assert!(controller.rust().prefetch.is_empty());
    }
    #[test]
    fn updates_refresh_in_the_background_only_as_often_as_checks_run() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
        controller.as_mut().set_check_interval(60);
        let key = cache_key("Updates", "", &[], false);
        for view in ["Installed", "Clean", "Sources"] {
            controller
                .as_mut()
                .rust_mut()
                .view_cache
                .insert(cache_key(view, "", &[], false), cached_view("fresh"));
        }
        // Older than the usual rewarm, younger than the hourly check.
        for (age, due) in [(REWARM_AFTER, false), (Duration::from_secs(60 * 60), true)] {
            let mut updates = cached_view("updates");
            updates.loaded = Instant::now() - age;
            controller
                .as_mut()
                .rust_mut()
                .view_cache
                .insert(key.clone(), updates);
            controller.as_mut().rust_mut().prefetch.clear();
            controller.as_mut().rust_mut().last_rewarm = Instant::now() - Duration::from_secs(60);
            controller.as_mut().poll();
            assert_eq!(
                controller.rust().prefetch,
                if due {
                    vec!["Updates".to_owned()]
                } else {
                    vec![]
                },
                "{age:?}"
            );
        }
    }
    #[test]
    fn preloads_start_on_their_own_worker_and_report_back() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        // A standalone tool keeps detection cheap and read-only.
        controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
        controller.as_mut().rust_mut().prefetch = vec!["Clean".into()];
        controller.as_mut().poll();
        assert!(controller.rust().prefetch_worker.is_some());
        assert!(controller.rust().worker.is_none());
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        assert!(
            controller.rust().prefetched.contains_key(&cache_key(
                "Clean",
                "",
                &["grok".into()],
                false
            )) || controller.rust().prefetch_worker.is_none()
        );
    }
    #[test]
    fn searches_reuse_a_recent_engine_and_keep_its_detection_time() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let job = Job::Load("Search".into(), "anything".into());
        let born = Instant::now() - Duration::from_secs(5);
        let scope = engine_source(&job, &[]);
        controller.as_mut().rust_mut().engine = Some(Engine::default());
        controller.as_mut().rust_mut().engine_scope = Some((scope.clone(), false, born));
        controller.as_mut().start(job);
        wait_until(&mut controller, |c| c.rust().worker.is_none());
        let (sources, sudo, kept) = controller.rust().engine_scope.clone().unwrap();
        assert_eq!((sources, sudo, kept), (scope, false, born));
        assert!(controller.rust().engine.is_some());
        // Changing the sources forgets it.
        controller.as_mut().rust_mut().view_cache.insert(
            cache_key("Installed", "", &["apt".into()], false),
            cached_view("apt only"),
        );
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "apt".into(), false, false);
        assert!(controller.rust().engine_scope.is_none());
        assert!(controller.rows().to_string().contains("apt only"));
    }
    #[test]
    fn rows_stay_actionable_only_while_rows_load() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let worker = |job| Worker {
            handle: thread::spawn(|| {}),
            receiver: mpsc::channel().1,
            cancel: Cancellation::default(),
            job,
        };
        let refresh = Operation::Refresh {
            backend: "apt".into(),
        };
        assert!(!controller.rust().only_reading());
        controller.as_mut().rust_mut().worker =
            Some(worker(Job::Load("Search".into(), "x".into())));
        controller.as_mut().sync_needs_poll();
        assert!(*controller.reading());
        // A change being prepared or queued blocks rows again.
        controller.as_mut().rust_mut().queued = Some(Job::PlanOperation(refresh.clone()));
        assert!(!controller.rust().only_reading());
        controller.as_mut().rust_mut().queued = None;
        controller.as_mut().rust_mut().worker = Some(worker(Job::Write(refresh, None)));
        controller.as_mut().sync_needs_poll();
        assert!(!*controller.reading());
        controller.as_mut().rust_mut().worker = None;
    }
    #[test]
    fn preloads_reuse_one_checked_engine_and_pass_it_on() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let scope = engine_source(&Job::Load("Installed".into(), String::new()), &[]);
        let born = Instant::now() - Duration::from_secs(5);
        controller.as_mut().rust_mut().engine = Some(Engine::default());
        controller.as_mut().rust_mut().engine_scope = Some((scope.clone(), false, born));
        controller
            .as_mut()
            .start_prefetch("Installed".into(), cache_key("Installed", "", &[], false));
        assert!(controller.rust().engine.is_none());
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        // Handed back with its first check time, for the next preload or search.
        assert_eq!(
            controller.rust().engine_scope,
            Some((scope.clone(), false, born))
        );
        assert!(controller.rust().engine.is_some());
        // An engine checked over a minute ago is checked again.
        let old = Instant::now() - VIEW_TTL;
        controller.as_mut().rust_mut().engine_scope = Some((scope.clone(), false, old));
        controller
            .as_mut()
            .start_prefetch("Updates".into(), cache_key("Updates", "", &[], false));
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        let (_, _, checked) = controller.rust().engine_scope.clone().unwrap();
        assert!(checked > old);
        // A preload made stale by a source change keeps nothing.
        controller.as_mut().rust_mut().engine = None;
        controller.as_mut().rust_mut().engine_scope = None;
        controller.as_mut().rust_mut().prefetch_worker = Some(fake_prefetch(
            "Clean",
            cache_key("Clean", "", &[], false),
            vec![Reply::Engine(Box::default())],
            true,
        ));
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        assert!(controller.rust().engine.is_none());
    }
    #[test]
    fn a_better_name_from_details_updates_cached_sections_too() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let package = synthetic_package("org.example.app", "org.example.app");
        let rows = encoded(vec![
            json!({"name": "org.example.app", "display_name": "org.example.app"}),
        ]);
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        controller.as_mut().set_rows(rows.clone());
        let mut cached = cached_view("unused");
        cached.packages = vec![package.clone()];
        cached.rows = rows;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(cache_key("Installed", "", &[], false), cached);
        let mut named = package;
        named.display_name = "Example App".into();
        controller
            .as_mut()
            .apply(Ok(Payload::Details(Box::new(PackageDetails {
                package: named,
                description: String::new(),
                homepage: None,
                dependencies: vec![],
            }))));
        assert!(controller.rows().to_string().contains("Example App"));
        let cached = controller
            .rust()
            .view_cache
            .get(&cache_key("Installed", "", &[], false))
            .unwrap();
        assert!(cached.rows.to_string().contains("Example App"));
        assert_eq!(cached.packages[0].display_name, "Example App");
    }
    #[test]
    fn updates_wait_for_an_installed_preload_and_cancel_older_reads() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let (release, gate) = mpsc::channel::<()>();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = gate.recv();
            let mut report = PackageReport::default();
            let mut package = synthetic_package("pending", "pending");
            package.update = UpdateAvailability::Available;
            package.candidate_version = Some("2".into());
            report.packages.push(package);
            report.successful_sources.push("apt".into());
            let _ = sender.send(Reply::Inventory(report.clone(), true));
            let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
        });
        controller.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
            handle,
            receiver,
            cancel: Cancellation::default(),
            view: "Updates".into(),
            key: cache_key("Updates", "", &[], false),
            stale: false,
            asked: vec![],
            answered: vec![],
            scope: (vec![], false, Instant::now()),
        });
        // An older foreground read is cancelled and its reply ignored.
        controller
            .as_mut()
            .start(Job::Load("Search".into(), "zzz-no-such-package".into()));
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, false);
        assert!(controller.rust().awaiting_prefetch);
        assert!(controller.rust().background);
        release.send(()).unwrap();
        wait_until(&mut controller, |c| !*c.busy() && c.rust().worker.is_none());
        assert!(controller.rows().to_string().contains("pending"));
    }
    #[test]
    fn pending_sources_shrink_as_partials_arrive_and_clear_when_the_load_ends() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let pending = |c: &ffi::PackageController| -> Vec<String> {
            serde_json::from_str(&c.pending_sources().to_string()).unwrap()
        };
        assert!(pending(&controller).is_empty());
        let (step, gate) = mpsc::channel::<()>();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let asked = vec!["apt".to_string(), "gem".into(), "npm".into()];
            let _ = sender.send(Reply::Asked(asked));
            let _ = gate.recv();
            let mut report = PackageReport::default();
            report.packages.push(synthetic_package("fast", "fast"));
            report.successful_sources.push("apt".into());
            let _ = sender.send(Reply::Partial(report.clone()));
            let _ = gate.recv();
            report.failures.push(BackendFailure {
                backend: "npm".into(),
                error: EngineError::Cancelled,
            });
            let _ = sender.send(Reply::Partial(report.clone()));
            let _ = gate.recv();
            // Gem never answers, as a source skipped quietly would not.
            let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
        });
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load("Search".into(), "fast".into()),
        });
        controller.as_mut().set_busy(true);
        wait_until(&mut controller, |c| !c.rust().asked.is_empty());
        assert_eq!(pending(&controller), ["apt", "gem", "npm"]);
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().asked.len() == 2);
        assert_eq!(pending(&controller), ["gem", "npm"]);
        assert!(controller.rows().to_string().contains("fast"));
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().asked.len() == 1);
        assert_eq!(pending(&controller), ["gem"]);
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().worker.is_none());
        assert!(pending(&controller).is_empty());
    }
    #[test]
    fn pending_sources_clear_on_cancel_and_skip_background_loads() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let pending = |c: &ffi::PackageController| c.pending_sources().to_string();
        let worker = |job: Job| {
            let (step, gate) = mpsc::channel::<()>();
            let (sender, receiver) = mpsc::channel();
            let handle = thread::spawn(move || {
                let _ = sender.send(Reply::Asked(vec!["apt".into(), "npm".into()]));
                let _ = gate.recv();
                let mut report = PackageReport::default();
                report.successful_sources.push("apt".into());
                let _ = sender.send(Reply::Partial(report.clone()));
                let _ = gate.recv();
                let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
            });
            let worker = Worker {
                handle,
                receiver,
                cancel: Cancellation::default(),
                job,
            };
            (worker, step)
        };
        // Cancelling stops showing what the load waited for, and a partial
        // that still arrives does not bring it back.
        let (running, step) = worker(Job::Load("Installed".into(), "".into()));
        controller.as_mut().rust_mut().worker = Some(running);
        wait_until(&mut controller, |c| !c.rust().asked.is_empty());
        assert_eq!(pending(&controller), r#"["apt","npm"]"#);
        controller.as_mut().cancel();
        assert_eq!(pending(&controller), "[]");
        step.send(()).unwrap();
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().worker.is_none());
        assert_eq!(pending(&controller), "[]");
        // Another request replacing the load hides it at the next poll,
        // before the replaced load winds down.
        let (running, step) = worker(Job::Load("Search".into(), "vim".into()));
        controller.as_mut().rust_mut().worker = Some(running);
        wait_until(&mut controller, |c| !c.rust().asked.is_empty());
        controller.rust().worker.as_ref().unwrap().cancel.cancel();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().poll();
        assert_eq!(pending(&controller), "[]");
        assert!(controller.rust().worker.is_some());
        step.send(()).unwrap();
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().worker.is_none());
        assert_eq!(pending(&controller), "[]");
        // A section reloading out of sight never lists its sources.
        let (running, step) = worker(Job::Load("Updates".into(), "".into()));
        controller.as_mut().rust_mut().worker = Some(running);
        controller.as_mut().rust_mut().background = true;
        step.send(()).unwrap();
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().worker.is_none());
        assert_eq!(pending(&controller), "[]");
        assert!(controller.rust().asked.is_empty());
    }
    #[test]
    fn a_section_waiting_on_its_preload_names_the_sources_still_loading() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let pending = |c: &ffi::PackageController| c.pending_sources().to_string();
        let (step, gate) = mpsc::channel::<()>();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = sender.send(Reply::Asked(vec!["apt".into(), "brew".into()]));
            let _ = gate.recv();
            let _ = sender.send(Reply::Answered(vec!["brew".into()]));
            let _ = gate.recv();
            let mut report = PackageReport::default();
            report.successful_sources.push("brew".into());
            let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
        });
        controller.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
            handle,
            receiver,
            cancel: Cancellation::default(),
            view: "Updates".into(),
            key: cache_key("Updates", "", &[], false),
            stale: false,
            asked: vec![],
            answered: vec![],
            scope: (vec![], false, Instant::now()),
        });
        // A preload nobody waits for stays quiet.
        wait_until(&mut controller, |c| {
            c.rust().prefetch_worker.as_ref().unwrap().asked.len() == 2
        });
        assert_eq!(pending(&controller), "[]");
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, false);
        assert!(controller.rust().awaiting_prefetch);
        assert_eq!(pending(&controller), r#"["apt","brew"]"#);
        step.send(()).unwrap();
        wait_until(&mut controller, |c| {
            c.pending_sources().to_string() == r#"["apt"]"#
        });
        // Leaving the section stops naming them; coming back resumes.
        controller
            .as_mut()
            .load("Search".into(), "".into(), "".into(), false, false);
        assert_eq!(pending(&controller), "[]");
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, false);
        assert_eq!(pending(&controller), r#"["apt"]"#);
        step.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
        assert!(!controller.rust().awaiting_prefetch);
        assert_eq!(pending(&controller), "[]");
    }
    #[test]
    fn a_stale_preload_names_nothing() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let key = cache_key("Installed", "", &[], false);
        let mut worker = fake_prefetch("Installed", key, vec![], false);
        worker.asked = vec!["apt".into()];
        controller.as_mut().rust_mut().prefetch_worker = Some(worker);
        controller.as_mut().rust_mut().active_view = "Installed".into();
        controller.as_mut().rust_mut().awaiting_prefetch = true;
        controller.as_mut().show_pending();
        assert_eq!(controller.pending_sources().to_string(), r#"["apt"]"#);
        controller.as_mut().rust_mut().invalidate_prefetch();
        controller.as_mut().rust_mut().awaiting_prefetch = true;
        controller.as_mut().show_pending();
        assert_eq!(controller.pending_sources().to_string(), "[]");
    }
    #[test]
    fn preloads_report_which_sources_answered_instead_of_rows() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .start_prefetch("Installed".into(), cache_key("Installed", "", &[], false));
        let worker = controller
            .as_mut()
            .rust_mut()
            .prefetch_worker
            .take()
            .unwrap();
        worker.handle.join().unwrap();
        let replies: Vec<_> = worker.receiver.try_iter().collect();
        assert!(matches!(&replies[0], Reply::Asked(asked) if asked == &["fixture"]));
        assert!(matches!(&replies[1], Reply::Answered(answered) if answered == &["fixture"]));
        assert!(!replies.iter().any(|r| matches!(r, Reply::Partial(_))));
    }
    #[test]
    fn retrying_a_source_in_a_section_ignores_search_text() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
        controller
            .as_mut()
            .retry_source("Clean".into(), "leftover text".into(), "grok".into());
        assert!(matches!(
            controller.rust().worker.as_ref().map(|worker| &worker.job),
            Some(Job::RetrySource(view, query, _)) if view == "Clean" && query.is_empty()
        ));
        wait_until(&mut controller, |c| c.rust().worker.is_none());
    }
    #[test]
    fn opening_a_preloading_section_waits_for_that_preload() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let key = cache_key("Installed", "", &[], false);
        let (sender, receiver) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let _ = gate.recv();
            let mut report = PackageReport::default();
            report.packages.push(Package {
                id: PackageId {
                    backend: "apt".into(),
                    name: "preloaded".into(),
                    architecture: "all".into(),
                    scope: Scope::System,
                    remote: None,
                    reference: None,
                },
                display_name: "preloaded".into(),
                summary: "Preloaded package".into(),
                installed_version: Some("1".into()),
                candidate_version: Some("1".into()),
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
                adopt_with: None,
            });
            report.successful_sources.push("apt".into());
            let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
        });
        controller.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
            handle,
            receiver,
            cancel: Cancellation::default(),
            view: "Installed".into(),
            key,
            stale: false,
            asked: vec![],
            answered: vec![],
            scope: (vec![], false, Instant::now()),
        });
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        // No second query: the section waits for the preload in flight.
        assert!(controller.rust().worker.is_none());
        assert!(controller.rust().awaiting_prefetch);
        assert!(*controller.busy());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && *controller.busy() {
            controller.as_mut().poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!*controller.busy());
        assert!(controller.rows().to_string().contains("preloaded"));
        assert!(controller.rust().worker.is_none());
    }
    #[test]
    fn typed_searches_never_evict_preloaded_sections() {
        let mut cache = ViewCache { entries: vec![] };
        cache.insert(
            cache_key("Installed", "", &[], false),
            cached_view("installed"),
        );
        for index in 0..100 {
            cache.insert(
                cache_key("Search", &format!("query {index}"), &[], false),
                cached_view("search"),
            );
        }
        assert!(cache.get(&cache_key("Installed", "", &[], false)).is_some());
        assert!(cache
            .get(&cache_key("Search", "query 99", &[], false))
            .is_some());
        assert_eq!(
            cache_key("Sources", "", &["apt".into()], false),
            cache_key("Sources", "", &[], false)
        );
    }

    #[test]
    fn source_catalog_check_is_lazy_and_single_flight() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        assert!(controller.rust().catalog_worker.is_none());
        controller.as_mut().begin_catalog_check(|_| {
            vec![Source {
                backend: "apt".into(),
                capabilities: vec![Capability::Search],
                availability: Ok(Availability::Available),
            }]
        });
        controller
            .as_mut()
            .begin_catalog_check(|_| unreachable!("the check is single-flight"));
        for _ in 0..100 {
            controller.as_mut().poll();
            if controller.rust().catalog_checked {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(controller.rust().catalog_checked);
        let catalog: Value =
            serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
        assert_eq!(catalog.as_array().unwrap().len(), 1);
        assert_eq!(catalog[0]["source"], "apt");
        assert_eq!(catalog[0]["availability_kind"], "available");
        assert!(!*controller.busy());
        assert!(controller.rust().worker.is_none());
        controller.as_mut().begin_catalog_check(|_| Vec::new());
        assert!(controller.rust().catalog_worker.is_none());
    }

    #[test]
    fn dropping_controller_cancels_source_catalog_check() {
        let cancel = Cancellation::default();
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || drop(sender));
        let mut controller = Controller::default();
        controller.catalog_worker = Some(CatalogWorker {
            handle,
            receiver,
            cancel: cancel.clone(),
        });
        drop(controller);
        assert!(cancel.requested());
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
            .contains("Run 1 cleanup task?"));
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
    fn repository_confirmation_and_refresh_invalidate_cached_state() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().change_repository("broken".into());
        assert!(controller
            .status()
            .to_string()
            .contains("This repository change isn't valid"));
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
                features: repositories::Features::default(),
            })));
        let report: Value = serde_json::from_str(&controller.repositories().to_string()).unwrap();
        assert_eq!(report["repositories"][0]["scope"], "system");
        assert_eq!(report["errors"][0], "Synthetic partial failure.");
        assert_eq!(report["repositories"][0]["where"], "Flatpak, System");
        assert!(controller
            .rust()
            .view_cache
            .get("old")
            .is_some_and(CachedView::stale));
    }
    #[test]
    fn restricted_sources_propose_removal_only_when_their_source_can_remove() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let mut app = synthetic_package("/Applications/Obsidian.app", "Obsidian");
        app.id.backend = "macos-apps".into();
        app.installed_version = Some("1.2.3".into());
        app.update = UpdateAvailability::Available;
        let mut tool = synthetic_package("codex", "Codex");
        tool.id.backend = "codex".into();
        tool.installed_version = Some("1.0".into());
        let mut missing = tool.clone();
        missing.installed_version = None;
        controller.as_mut().rust_mut().packages = vec![app, tool, missing];
        let proposes =
            |controller: &mut Pin<&mut ffi::PackageController>, action: &str, index: i32| {
                controller.as_mut().propose(action.into(), index);
                let proposed = controller.rust().pending.is_some();
                assert_eq!(proposed, !controller.confirmation().is_empty());
                proposed
            };
        // Until the catalog says a source can remove, nothing is proposed.
        for catalog in [
            "[]",
            "not json",
            r#"[{"source":"macos-apps","capabilities":["search","installed"]},
                {"source":"codex","capabilities":["installed","upgrade"]}]"#,
        ] {
            controller.as_mut().set_source_catalog(catalog.into());
            for index in [0, 1] {
                assert!(!proposes(&mut controller, "remove", index));
            }
        }
        controller.as_mut().set_source_catalog(
            r#"[{"source":"macos-apps","capabilities":["installed","remove"]},
                {"source":"codex","capabilities":["installed","remove","upgrade"]}]"#
                .into(),
        );
        // Inventory rows are still never installed or updated, and a source
        // PkgDeck never installs from offers no install.
        for action in ["install", "upgrade"] {
            assert!(!proposes(&mut controller, action, 0));
        }
        assert!(!proposes(&mut controller, "install", 2));
        // Only installed rows can be removed.
        assert!(!proposes(&mut controller, "remove", 2));
        for (index, backend) in [(0, "macos-apps"), (1, "codex")] {
            assert!(proposes(&mut controller, "remove", index));
            assert!(
                matches!(&controller.rust().pending, Some(Job::Write(Operation::Remove(id), _)) if id.backend == backend)
            );
        }
    }
    #[test]
    fn ordinary_sources_offer_removal_without_asking_the_catalog() {
        assert!(removable("[]", "apt"));
        assert!(removable("not json", "homebrew"));
        assert!(!removable("[]", "fwupd"));
        assert!(!removable(r#"[{"source":"mas"}]"#, "mas"));
        assert!(!removable(
            r#"[{"source":"conda","capabilities":["remove"]}]"#,
            "mas"
        ));
        assert!(removable(
            r#"[{"source":"mas","capabilities":["remove"]}]"#,
            "mas"
        ));
    }
    #[test]
    fn firmware_actions_require_confirmation_and_never_offer_removal() {
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
            summary: "Firmware, AC power required, Restart required".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
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
    fn container_pull_is_explicit_and_requires_a_stored_tag() {
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
            summary: "Tags: example/app:latest. 42MB".into(),
            installed_version: Some("0123456789ab".into()),
            candidate_version: None,
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
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
    fn source_failures_and_busy_requests_preserve_the_active_operation() {
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
            .contains("Firmware isn't available: synthetic daemon offline."));
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
            .contains("Waiting for the package manager to finish"));
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
    fn streamed_details_preserve_progress_and_finish_cleanly() {
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
            adopt_with: None,
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
            .contains("Finished before it could be cancelled"));
    }
    #[test]
    fn new_view_discards_superseded_reports_and_cancellation_errors() {
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
            .contains("No package matches"));
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
    #[test]
    fn inventory_jobs_export_and_preview_exact_installed_identity() {
        let package: Package = serde_json::from_value(json!({
            "id":{"backend":"fixture", "name":"synthetic-editor", "architecture":"x86_64", "scope":"system"},
            "display_name":"Synthetic Editor", "summary":"Fixture", "installed_version":"1", "candidate_version":null, "update":"current"
        })).unwrap();
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-gui-inventory-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package.clone(),
                fail: false,
            })
            .unwrap();
        let mut replies = Vec::new();
        execute(
            &mut engine,
            Job::ManifestExport(path.clone(), vec![package.id.clone()]),
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        assert!(matches!(
            replies.pop(),
            Some(Reply::Done(Ok(Payload::ManifestExport(1))))
        ));
        let saved = manifest::read(&path).unwrap();
        assert_eq!(saved.packages[0].name, "synthetic-editor");
        execute(
            &mut engine,
            Job::ManifestPreview(path.clone()),
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        let preview = expect!(
            replies.pop(),
            Some(Reply::Done(Ok(Payload::ManifestPreview(preview)))) => preview
        );
        assert_eq!(
            preview.packages[0].status,
            manifest::PreviewStatus::AlreadyInstalled
        );
        execute(
            &mut engine,
            Job::ManifestExport(path.clone(), vec![]),
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        assert!(matches!(replies.pop(), Some(Reply::Done(Err(_)))));
        std::fs::remove_file(path).unwrap();
    }
    /// A source whose installed list always fails and whose writes report
    /// the given progress before succeeding.
    struct Scripted {
        id: &'static str,
        progress: Vec<Progress>,
    }
    impl Backend for Scripted {
        fn id(&self) -> &str {
            self.id
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Installed, Capability::Upgrade]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn execute(
            &mut self,
            _: &Operation,
            _: &Cancellation,
            progress: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            self.progress.drain(..).for_each(progress);
            Ok(OperationOutcome::default())
        }
    }
    /// Homebrew Casks where updating `locked` stops at sudo's password.
    struct PasswordCask {
        locked: &'static str,
    }
    impl Backend for PasswordCask {
        fn id(&self) -> &str {
            "homebrew-cask"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Installed, Capability::Upgrade]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn execute(
            &mut self,
            operation: &Operation,
            _: &Cancellation,
            _: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            match operation {
                Operation::Upgrade(id) if id.name == self.locked => Err(
                    pkgdeck_core::process::ExecutionError::Failed(
                        pkgdeck_core::process::Completion {
                            code: Some(1),
                            signal: None,
                            stdout: vec![],
                            stderr: format!("sudo: a password is required\nError: {}: Failure while executing; `/usr/bin/sudo -E -- /bin/rm` exited with 1.\n", id.name).into_bytes(),
                            truncated: false,
                            cancellation_deferred: false,
                        },
                    )
                    .into(),
                ),
                _ => Ok(OperationOutcome::default()),
            }
        }
    }
    #[test]
    fn an_automatic_run_remembers_casks_that_stopped_for_the_password() {
        let cask = |name: &str| {
            let mut package = synthetic_package(name, name);
            package.id.backend = "homebrew-cask".into();
            package
        };
        let (mail, editor) = (cask("mail-app"), cask("editor-app"));
        let job = Job::AutoUpgrade(
            vec![
                Operation::Upgrade(mail.id.clone()),
                Operation::Upgrade(editor.id.clone()),
            ],
            false,
        );
        let mut engine = Engine::default();
        engine
            .register(PasswordCask { locked: "mail-app" })
            .unwrap();
        let replies = run_job(&mut engine, job.clone(), &Cancellation::default());
        assert!(replies
            .iter()
            .any(|reply| matches!(reply, Reply::NeedsPassword(id) if *id == mail.id)));

        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().cask_versions = BTreeMap::from([
            (mail.id.clone(), "2".to_owned()),
            (editor.id.clone(), "5".to_owned()),
        ]);
        let replies = replies
            .into_iter()
            .filter(|reply| matches!(reply, Reply::NeedsPassword(_) | Reply::Done(_)))
            .collect();
        controller.as_mut().rust_mut().worker = Some(fake_worker(job, replies));
        controller.as_mut().poll();
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["kind"], "info");
        assert_eq!(notice["title"], "mail-app needs your password to update");
        assert!(controller.rust().needs_password.skips("mail-app", "2"));
        assert!(!controller.rust().needs_password.skips("editor-app", "5"));
        assert!(controller.rust().password_casks.is_empty());
    }
    /// A standalone tool whose updates fail while `broken`.
    struct BrokenTool {
        broken: bool,
    }
    impl Backend for BrokenTool {
        fn id(&self) -> &str {
            "anchor"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Installed, Capability::Upgrade]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn execute(
            &mut self,
            _: &Operation,
            _: &Cancellation,
            _: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            if self.broken {
                Err(EngineError::Execution(
                    pkgdeck_core::process::ExecutionError::Io(
                        "No such file or directory (os error 2)".into(),
                    ),
                ))
            } else {
                Ok(OperationOutcome::default())
            }
        }
    }
    #[test]
    fn update_all_skips_updates_that_failed_until_they_work() {
        let available = |backend: &str, name: &str, display: &str, version: &str| {
            let mut package = synthetic_package(name, display);
            package.id.backend = backend.into();
            package.installed_version = Some("1".into());
            package.candidate_version = Some(version.into());
            package.update = UpdateAvailability::Available;
            package
        };
        let mail = available("homebrew-cask", "mail-app", "Mail App", "2");
        let mut tool = available("anchor", "anchor", "Anchor (AVM)", "2.0");
        tool.id.reference = Some("/home/fixture/.avm/bin/avm".into());
        let upgrade = |package: &Package| Operation::Upgrade(package.id.clone());
        let run = |controller: &mut Pin<&mut ffi::PackageController>, job: Job, broken: bool| {
            let mut engine = Engine::default();
            engine
                .register(PasswordCask { locked: "mail-app" })
                .unwrap();
            engine.register(BrokenTool { broken }).unwrap();
            let replies = run_job(&mut engine, job.clone(), &Cancellation::default())
                .into_iter()
                .filter(|reply| matches!(reply, Reply::NeedsPassword(_) | Reply::Done(_)))
                .collect();
            controller.as_mut().rust_mut().worker = Some(fake_worker(job, replies));
            controller.as_mut().poll();
        };
        let held = |controller: &Pin<&mut ffi::PackageController>| -> Vec<(String, String)> {
            serde_json::from_str::<Vec<Value>>(controller.held_updates().as_str())
                .unwrap()
                .iter()
                .map(|held| {
                    (
                        held["name"].as_str().unwrap().to_owned(),
                        held["reason"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        };
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().packages = vec![mail.clone(), tool.clone()];
        controller.as_mut().rust_mut().updates_view = true;
        run(
            &mut controller,
            Job::UpgradeAll(vec![upgrade(&mail), upgrade(&tool)], None),
            true,
        );
        // Each failure is one short line named the way people know it.
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["title"], "2 of 2 changes failed", "{notice}");
        let detail = notice["detail"].as_str().unwrap();
        assert!(!detail.contains("/home/fixture"), "{detail}");
        assert!(detail.contains("Anchor (AVM): "), "{detail}");
        assert!(detail.contains("Mail App: "), "{detail}");
        // The tool waits as a failure, the cask for the password.
        assert!(controller.rust().failed_updates.skips(&tool.id, "2.0"));
        assert!(!controller.rust().failed_updates.skips(&mail.id, "2"));
        assert!(controller.rust().needs_password.skips("mail-app", "2"));
        assert_eq!(
            held(&controller),
            [
                ("anchor".into(), "failed".into()),
                ("mail-app".into(), "password".into())
            ]
        );
        // Update all leaves the failed one out, and asks for the cask.
        controller.as_mut().propose("upgrade-all".into(), -1);
        let Some(Job::UpgradeAll(operations, _)) = &controller.rust().pending else {
            panic!("Update all asks first");
        };
        assert!(!operations.contains(&upgrade(&tool)), "{operations:?}");
        assert!(!operations.is_empty());
        // With nothing else left, it tries it again.
        controller.as_mut().confirm(false);
        controller.as_mut().rust_mut().packages = vec![tool.clone()];
        controller.as_mut().propose("upgrade-all".into(), -1);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::UpgradeAll(operations, _)) if operations == &[upgrade(&tool)]
        ));
        controller.as_mut().confirm(false);
        // A newer version is tried again and isn't marked.
        let newer = available("anchor", "anchor", "Anchor (AVM)", "2.1");
        let newer = Package {
            id: tool.id.clone(),
            ..newer
        };
        controller.as_mut().rust_mut().packages = vec![newer.clone()];
        controller.as_mut().publish_held_updates();
        assert!(held(&controller).iter().all(|(name, _)| name != "anchor"));
        controller.as_mut().rust_mut().packages = vec![mail.clone(), tool.clone()];
        // Once it works, it's forgotten, and so is the cask.
        run(&mut controller, Job::Write(upgrade(&tool), None), false);
        assert!(!controller.rust().failed_updates.skips(&tool.id, "2.0"));
        let mut engine = Engine::default();
        engine.register(PasswordCask { locked: "other" }).unwrap();
        let job = Job::Write(upgrade(&mail), None);
        let replies = run_job(&mut engine, job.clone(), &Cancellation::default());
        controller.as_mut().rust_mut().worker = Some(fake_worker(job, replies));
        controller.as_mut().poll();
        assert!(!controller.rust().needs_password.skips("mail-app", "2"));
        assert!(held(&controller).is_empty());
    }
    #[test]
    fn one_failed_update_is_named_without_its_path() {
        let mut tool = synthetic_package("anchor", "Anchor (AVM)");
        tool.id.backend = "anchor".into();
        tool.id.reference = Some("/home/fixture/.avm/bin/avm".into());
        let operation = Operation::Upgrade(tool.id.clone());
        let names = Names::from([(tool.id.clone(), "Anchor (AVM)".to_owned())]);
        let line = format!(
            "{}: Anchor (AVM) couldn't run: No such file or directory (os error 2).",
            operation_title(&operation)
        );
        assert!(line.contains("/home/fixture"), "the old line named the path");
        let notice = write_notice(
            &Job::AutoUpgrade(vec![operation.clone()], false),
            &Ok(Payload::Batch(
                format!("Completed 0 of 1 updates.\n{line}"),
                vec![Outcome::Failed],
            )),
            false,
            &names,
        );
        assert_eq!(notice["title"], "Anchor (AVM) didn't update");
        assert_eq!(
            notice["detail"],
            "Anchor (AVM) couldn't run: No such file or directory (os error 2)."
        );
        // A single change that fails on its own reads the same way.
        let single = write_notice(
            &Job::Write(operation, None),
            &Err(EngineError::NotFound),
            false,
            &names,
        );
        assert_eq!(single["title"], "Anchor (AVM) didn't update");
        // Anything else keeps saying what failed.
        let install = write_notice(
            &Job::Write(Operation::Install(tool.id.clone()), None),
            &Err(EngineError::NotFound),
            false,
            &names,
        );
        assert_eq!(install["title"], "Install Anchor (AVM) (Anchor (AVM)) failed");
    }
    #[test]
    fn automatic_runs_skip_updates_that_failed() {
        let mut tool = synthetic_package("anchor", "Anchor (AVM)");
        tool.id.backend = "anchor".into();
        tool.installed_version = Some("1".into());
        tool.candidate_version = Some("2.0".into());
        tool.update = UpdateAvailability::Available;
        let dir = std::env::temp_dir().join(format!("pkgdeck-auto-failed-{}", std::process::id()));
        let mut failed = FailedUpdates::at(&dir.join("failed-updates.json"));
        let plan = |failed: &FailedUpdates, packages: &[Package]| {
            automatic_plan(packages, &NeedsPassword::default(), failed)
        };
        assert_eq!(
            plan(&failed, &[tool.clone()]),
            [Operation::Upgrade(tool.id.clone())]
        );
        failed.remember(&[(tool.id.clone(), "2.0".into())], &[]);
        // Automatic runs never fall back to trying it again.
        assert!(plan(&failed, &[tool.clone()]).is_empty());
        let mut newer = tool.clone();
        newer.candidate_version = Some("2.1".into());
        assert_eq!(
            plan(&failed, &[newer]),
            [Operation::Upgrade(tool.id.clone())]
        );
        // An update that failed to the same version isn't run when checked
        // with others either.
        let identity = |p: &Package| {
            json!([
                p.id.backend,
                p.id.name,
                p.id.architecture,
                p.id.remote,
                p.id.scope
            ])
        };
        let mut other = tool.clone();
        other.id.name = "foundry".into();
        let checked = plan_checked_upgrade(
            &[tool.clone(), other.clone()],
            &json!([identity(&tool), identity(&other)]).to_string(),
            &failed,
        );
        assert_eq!(checked.operations, [Operation::Upgrade(other.id.clone())]);
        let _ = std::fs::remove_dir_all(dir);
    }
    /// APT whose dry run removes `removals`, and whose writes succeed.
    struct AptPlanFixture {
        removals: Vec<String>,
    }
    impl Backend for AptPlanFixture {
        fn id(&self) -> &str {
            "apt"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Installed, Capability::Upgrade]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn apt_upgrade_plan(&mut self, _: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
            Ok(AptUpgradePlan {
                preview: "Inst synthetic".into(),
                upgrades: vec!["synthetic".into()],
                installs: vec![],
                removals: self.removals.clone(),
            })
        }
        fn execute(
            &mut self,
            _: &Operation,
            _: &Cancellation,
            _: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            Ok(OperationOutcome::default())
        }
    }
    #[test]
    fn automatic_updates_hold_apt_back_when_it_would_remove_packages() {
        let all = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        for (removals, allowed, apt_outcome) in [
            (vec!["old-kernel".to_owned()], false, Outcome::Failed),
            (vec!["old-kernel".to_owned()], true, Outcome::Finished),
            (vec![], false, Outcome::Finished),
        ] {
            let mut engine = Engine::default();
            engine
                .register(AptPlanFixture {
                    removals: removals.clone(),
                })
                .unwrap();
            engine
                .register(Scripted {
                    id: "homebrew",
                    progress: vec![],
                })
                .unwrap();
            let replies = run_job(
                &mut engine,
                Job::AutoUpgrade(vec![all("apt"), all("homebrew")], allowed),
                &Cancellation::default(),
            );
            let (status, outcomes) = expect!(
                replies.into_iter().last(),
                Some(Reply::Done(Ok(Payload::Batch(status, outcomes)))) => (status, outcomes)
            );
            assert_eq!(outcomes, [apt_outcome.clone(), Outcome::Finished]);
            assert_eq!(
                status.contains("it would remove 1 package. Review it with Update all."),
                apt_outcome == Outcome::Failed,
                "{status}"
            );
        }
    }
    #[test]
    fn only_automatic_runs_skip_the_password() {
        use pkgdeck_core::batch::BatchMode;
        let all = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        // Update all is started by the person, so it asks.
        let manual = Job::UpgradeAll(vec![all("apt"), all("homebrew")], None);
        assert_eq!(batch_mode(&manual), None);
        let mut id = synthetic_package("synthetic", "Synthetic").id;
        id.backend = "dnf".into();
        assert_eq!(batch_mode(&Job::Write(Operation::Upgrade(id), None)), None);
        // An automatic run always uses the mode it was queued with.
        for removals in [false, true] {
            let auto = Job::AutoUpgrade(vec![all("apt")], removals);
            assert_eq!(batch_mode(&auto), Some(BatchMode::UpgradeOnly { removals }));
        }
    }
    #[test]
    fn update_all_leaves_apt_out_when_removals_are_off() {
        let all = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        let plan = AptUpgradePlan {
            preview: "Remv old-kernel".into(),
            upgrades: vec!["linux".into()],
            installs: vec![],
            removals: vec!["old-kernel".into()],
        };
        let available = |backend: &str, name: &str| {
            let mut package = synthetic_package(name, name);
            package.id.backend = backend.into();
            package.update = UpdateAvailability::Available;
            package
        };
        for allowed in [true, false] {
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            controller.as_mut().set_allow_removals(allowed);
            controller.as_mut().rust_mut().packages =
                vec![available("apt", "linux"), available("homebrew", "wget")];
            controller.as_mut().apply(Ok(Payload::UpgradePreview(
                vec![all("apt"), all("homebrew")],
                2,
                Some(plan.clone()),
            )));
            let confirmation = controller.confirmation().to_string();
            let pending = expect!(
                &controller.rust().pending,
                Some(Job::UpgradeAll(operations, apt)) => (operations.clone(), apt.clone())
            );
            if allowed {
                assert_eq!(
                    pending,
                    (vec![all("apt"), all("homebrew")], Some(plan.clone()))
                );
                assert!(confirmation.starts_with("Update all 2 listed packages?"));
            } else {
                assert_eq!(pending, (vec![all("homebrew")], None));
                assert!(confirmation.starts_with("Update all 1 listed package?"));
                assert!(confirmation.contains("APT is left out: it would remove old-kernel."));
            }
        }
        // With nothing else to update, nothing waits for confirmation.
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_allow_removals(false);
        controller
            .as_mut()
            .apply(Ok(Payload::UpgradePreview(vec![all("apt")], 1, Some(plan))));
        assert!(controller.rust().pending.is_none());
        assert!(controller
            .status()
            .to_string()
            .starts_with("APT is left out"));
    }
    #[test]
    fn automatic_runs_download_macos_updates_in_one_step() {
        let available = |backend: &str, name: &str| {
            let mut package = synthetic_package(name, name);
            package.id.backend = backend.into();
            package.update = UpdateAvailability::Available;
            package
        };
        let all = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        assert_eq!(
            automatic_plan(
                &[
                    available("macos-updates", "Safari"),
                    available("homebrew", "wget"),
                    available("macos-updates", "macOS Tahoe 26.1"),
                ],
                &NeedsPassword::default(),
                &FailedUpdates::default()
            ),
            [all("homebrew"), all("macos-updates")]
        );
        assert_eq!(
            automatic_plan(
                &[available("homebrew", "wget")],
                &NeedsPassword::default(),
                &FailedUpdates::default()
            ),
            [all("homebrew")]
        );
    }
    #[test]
    fn automatic_runs_skip_casks_that_needed_the_password() {
        let cask = |name: &str, version: &str| {
            let mut package = synthetic_package(name, name);
            package.id.backend = "homebrew-cask".into();
            package.installed_version = Some("1".into());
            package.candidate_version = Some(version.into());
            package.update = UpdateAvailability::Available;
            package
        };
        let (mail, editor) = (cask("mail-app", "2"), cask("editor-app", "5"));
        let upgrade = |package: &Package| Operation::Upgrade(package.id.clone());
        let dir = std::env::temp_dir().join(format!("pkgdeck-auto-casks-{}", std::process::id()));
        let mut needs = NeedsPassword::at(&dir.join("needs-password.json"));
        // One at a time, so one cask's password stops only that cask.
        assert_eq!(
            automatic_plan(
                &[mail.clone(), editor.clone()],
                &needs,
                &FailedUpdates::default()
            ),
            [upgrade(&mail), upgrade(&editor)]
        );
        needs.remember(&[("mail-app".into(), "2".into())], &[]);
        assert_eq!(
            automatic_plan(
                &[mail.clone(), editor.clone()],
                &needs,
                &FailedUpdates::default()
            ),
            [upgrade(&editor)]
        );
        assert_eq!(
            automatic_plan(&[cask("mail-app", "3")], &needs, &FailedUpdates::default()),
            [upgrade(&cask("mail-app", "3"))]
        );

        let job = Job::AutoUpgrade(vec![upgrade(&mail), upgrade(&editor)], false);
        let names = Names::from([(mail.id.clone(), "Mail App".to_owned())]);
        let batch = |outcomes: Vec<Outcome>| Ok(Payload::Batch(String::new(), outcomes));
        let notice = password_notice(
            &job,
            &batch(vec![Outcome::Failed, Outcome::Finished]),
            std::slice::from_ref(&mail.id),
            &names,
        )
        .unwrap();
        assert_eq!(notice["kind"], "info");
        assert_eq!(notice["title"], "Mail App needs your password to update");
        // Any other failure keeps the error banner.
        assert!(password_notice(
            &job,
            &batch(vec![Outcome::Failed, Outcome::Failed]),
            std::slice::from_ref(&mail.id),
            &names,
        )
        .is_none());
        let both = password_notice(
            &job,
            &batch(vec![Outcome::Failed, Outcome::Failed]),
            &[mail.id.clone(), editor.id.clone()],
            &names,
        )
        .unwrap();
        assert_eq!(both["title"], "2 apps need your password to update");
        assert!(password_notice(
            &job,
            &batch(vec![Outcome::Finished, Outcome::Finished]),
            &[],
            &names,
        )
        .is_none());
        let manual = Job::UpgradeAll(vec![upgrade(&mail)], None);
        assert!(password_notice(
            &manual,
            &batch(vec![Outcome::Failed]),
            std::slice::from_ref(&mail.id),
            &names
        )
        .is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn background_checks_queue_only_what_may_update_unattended() {
        let available = |backend: &str, name: &str| {
            let mut package = synthetic_package(name, name);
            package.id.backend = backend.into();
            package.update = UpdateAvailability::Available;
            package.candidate_version = Some("2".into());
            package
        };
        let report = PackageReport {
            packages: vec![
                available("homebrew", "wget"),
                // No saved approval here, so system managers wait.
                available("apt", "synthetic"),
                available("fwupd", "firmware"),
                // Its check failed, so it may have more than it listed.
                available("cargo", "ripgrep"),
            ],
            failures: vec![BackendFailure {
                backend: "cargo".into(),
                error: EngineError::Cancelled,
            }],
            successful_sources: vec!["homebrew".into(), "apt".into(), "fwupd".into()],
        };
        for automatic in [false, true] {
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            controller.as_mut().rust_mut().prefetch.clear();
            controller.as_mut().set_auto_update(automatic);
            controller.as_mut().set_allow_removals(automatic);
            controller.as_mut().finish_background_check(report.clone());
            let state: Value =
                serde_json::from_str(&controller.background_state().to_string()).unwrap();
            assert_eq!(state["notify"], !automatic);
            let queued = controller
                .rust()
                .validated_confirmed
                .as_ref()
                .map(|entry| entry.job.operations());
            assert!(controller.rust().validated_confirmed.as_ref().is_none_or(
                |entry| matches!(entry.job, Job::AutoUpgrade(_, removals) if removals)
            ));
            if automatic {
                assert_eq!(
                    queued,
                    Some(vec![Operation::UpgradeAll {
                        backend: "homebrew".into()
                    }])
                );
            } else {
                assert_eq!(queued, None);
            }
        }
        // Nothing starts while a change waits for confirmation.
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_auto_update(true);
        controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
        controller.as_mut().finish_background_check(report);
        assert!(controller.rust().validated_confirmed.is_none());
    }
    fn run_job(engine: &mut Engine, job: Job, cancel: &Cancellation) -> Vec<Reply> {
        let mut replies = Vec::new();
        execute(engine, job, cancel, &mut |reply| replies.push(reply));
        replies
    }
    #[test]
    fn jobs_without_a_worker_engine_path_answer_directly() {
        let mut engine = Engine::default();
        engine
            .register(Scripted {
                id: "fwupd",
                progress: vec![
                    Progress::Transfer {
                        completed: 5,
                        total: Some(10),
                    },
                    Progress::Transfer {
                        completed: 7,
                        total: None,
                    },
                    Progress::Package("synthetic-firmware".into()),
                ],
            })
            .unwrap();
        let cancel = Cancellation::default();
        let mut firmware = synthetic_package("synthetic-firmware", "Firmware").id;
        firmware.backend = "fwupd".into();
        let replies = run_job(
            &mut engine,
            Job::Write(Operation::Upgrade(firmware.clone()), None),
            &cancel,
        );
        let texts: Vec<_> = replies
            .iter()
            .filter_map(|reply| match reply {
                Reply::Progress(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [
                "Transferred 5 of 10",
                "Transferred 7",
                "Updating synthetic-firmware"
            ]
        );
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Ok(Payload::Batch(status, outcomes))))
                if status == "Firmware update finished. Restart or shut down the device if required."
                    && outcomes == &[Outcome::Finished]
        ));

        // A failed source makes an export incomplete, so nothing is written.
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-gui-partial-export-{}.json",
            std::process::id()
        ));
        let replies = run_job(
            &mut engine,
            Job::ManifestExport(path.clone(), vec![]),
            &cancel,
        );
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Err(EngineError::Execution(ExecutionError::Invalid(reason)))))
                if reason == "some package sources could not be read; retry the export"
        ));
        assert!(!path.exists());

        let upgrade = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        let replies = run_job(
            &mut engine,
            Job::PlanUpgrade(vec![upgrade("fwupd")], 3),
            &cancel,
        );
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Ok(Payload::UpgradePreview(operations, 3, None))))
                if operations == &[upgrade("fwupd")]
        ));
        let replies = run_job(
            &mut engine,
            Job::PlanUpgrade(vec![upgrade("apt")], 1),
            &cancel,
        );
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Err(EngineError::UnknownBackend(backend)))) if backend == "apt"
        ));
        let clean = vec![Operation::Clean(CleanupId {
            backend: "fwupd".into(),
            key: "cache".into(),
        })];
        let replies = run_job(&mut engine, Job::PlanCleanAll(clean.clone()), &cancel);
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Ok(Payload::CleanPreview(operations, items))))
                if operations == &clean && items.is_empty()
        ));
        for job in [
            Job::Repositories(None),
            Job::OpenInput("relative.deb".into()),
        ] {
            let replies = run_job(&mut engine, job, &cancel);
            assert!(matches!(
                replies.as_slice(),
                [Reply::Done(Err(
                    EngineError::NotFound | EngineError::InvalidResponse { .. }
                ))]
            ));
        }

        let replies = run_job(
            &mut engine,
            Job::UpgradeAll(vec![Operation::Upgrade(firmware.clone())], None),
            &cancel,
        );
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Ok(Payload::Batch(status, outcomes))))
                if status.contains(": Completed") && outcomes == &[Outcome::Finished]
        ));
        cancel.cancel();
        let replies = run_job(
            &mut engine,
            Job::UpgradeAll(vec![Operation::Upgrade(firmware)], None),
            &cancel,
        );
        assert!(matches!(
            replies.last(),
            Some(Reply::Done(Ok(Payload::Batch(_, outcomes)))) if outcomes == &[Outcome::Cancelled]
        ));
    }
    #[test]
    fn inventory_picker_rejects_remote_paths_and_bad_selection() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .export_inventory(QUrl::from("https://example.invalid/list.json"), "[]".into());
        assert!(controller.status().to_string().contains("local inventory"));
        controller
            .as_mut()
            .preview_inventory(QUrl::from("https://example.invalid/list.json"));
        assert!(controller.status().to_string().contains("local inventory"));
        let file = QUrl::from_local_file(&"/tmp/synthetic-list.json".into());
        controller.as_mut().export_inventory(file, "broken".into());
        assert!(controller
            .status()
            .to_string()
            .contains("Invalid package selection"));
        controller
            .as_mut()
            .apply(Ok(Payload::ManifestPreview(manifest::Preview {
                schema_version: manifest::SCHEMA_VERSION,
                packages: vec![],
            })));
        assert!(controller
            .manifest_preview()
            .to_string()
            .contains("packages"));
        controller.as_mut().apply(Ok(Payload::ManifestExport(2)));
        assert!(controller
            .status()
            .to_string()
            .contains("Exported 2 packages"));

        // A foreground picker request can preempt a background read without
        // touching the host while it waits for the worker to wind down.
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let cancellation = Cancellation::default();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancellation.clone(),
            job: Job::Load("Installed".into(), String::new()),
        });
        let local = QUrl::from_local_file(&"/tmp/synthetic-list.json".into());
        controller.as_mut().preview_inventory(local.clone());
        assert!(matches!(
            controller.rust().queued,
            Some(Job::ManifestPreview(_))
        ));
        assert!(cancellation.requested());
        controller.as_mut().export_inventory(local, "[]".into());
        assert!(matches!(
            controller.rust().queued,
            Some(Job::ManifestExport(_, _))
        ));
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
    }
    fn cached_view(name: &str) -> CachedView {
        CachedView {
            loaded: Instant::now(),
            expired: false,
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
    fn view_cache_replaces_evicts_oldest_and_expires() {
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
        cache.expire();
        assert!(cache.entries.iter().all(|(_, view)| view.stale()));
        assert_eq!(cache.entries.len(), ViewCache::CAPACITY);
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
            expect!(replies.pop().unwrap(), Reply::Done(Ok(Payload::Packages(report))) => report)
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
            adopt_with: None,
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
                &mut |reply| {
                    if let Reply::Partial(report) | Reply::Done(Ok(Payload::Packages(report))) =
                        reply
                    {
                        reports.push(report);
                    }
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
                adopt_with: None,
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
        // A reference that is not text, or trailing parts, never matches.
        let mut numbered = identity.clone();
        numbered[5] = serde_json::json!(7);
        let mut longer = identity.clone();
        longer.push(serde_json::json!("extra"));
        let malformed = serde_json::to_string(&vec![numbered, longer]).unwrap();
        assert!(checked_upgrades(&[runtime.clone()], &malformed).is_empty());
        identity[5] = serde_json::json!("runtime/org.example.Platform/all/beta");
        assert!(
            checked_upgrades(&[runtime], &serde_json::to_string(&vec![identity]).unwrap())
                .is_empty()
        );
    }
    #[test]
    fn rows_made_on_the_worker_match_rows_made_here() {
        let package = |name: &str| Package {
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
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Current,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        };
        let packages = vec![package("one"), package("two")];
        let made_here = serde_json::to_string(&package_rows(&packages)).unwrap();
        let mut cache = RowsCache::default();
        assert_eq!(cache.json(&packages), made_here);
        // Rows from a worker are used only for exactly the same packages.
        let mut prepared = PreparedRows::new(&packages);
        prepared.json = "[\"from the worker\"]".into();
        cache.insert(*prepared);
        assert_eq!(cache.json(&packages), "[\"from the worker\"]");
        let mut changed = packages.clone();
        changed[1].installed_version = Some("2".into());
        assert_ne!(cache.json(&changed), "[\"from the worker\"]");
        // Only the newest few are kept.
        for name in ["a", "b", "c", "d"] {
            cache.insert(*PreparedRows::new(&[package(name)]));
        }
        assert_eq!(cache.json(&packages), made_here);
        // A snapshot over the budget pushes every older one out, so one huge
        // list is held once and never four times.
        let mut huge = PreparedRows::new(&[package("huge")]);
        huge.json = "x".repeat(RowsCache::BUDGET);
        cache.insert(*huge);
        let mut small = PreparedRows::new(&[package("small")]);
        small.json = "[\"small\"]".into();
        cache.insert(*small);
        assert_eq!(cache.json(&[package("small")]), "[\"small\"]");
        assert_eq!(cache.0.len(), 1);
        // Failure rows go after the package rows.
        let failure = [json!({"kind": "failure"})];
        assert_eq!(
            append_rows("[]".into(), &failure),
            r#"[{"kind":"failure"}]"#
        );
        assert_eq!(
            append_rows("[1]".into(), &failure),
            r#"[1,{"kind":"failure"}]"#
        );
        assert_eq!(append_rows("[1]".into(), &[]), "[1]");
    }
    #[test]
    fn a_batch_of_partials_shows_only_the_newest() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = |name: &str| {
            let mut row = fixture_row();
            row.id.name = name.into();
            row
        };
        let report = |names: &[&str]| PackageReport {
            packages: names.iter().map(|name| row(name)).collect(),
            failures: vec![],
            successful_sources: vec![],
        };
        let (worker, gate) = held_worker(
            Job::Load("Installed".into(), String::new()),
            vec![
                Reply::Rows(PreparedRows::new(&report(&["first"]).packages)),
                Reply::Partial(report(&["first"])),
                Reply::Rows(PreparedRows::new(&report(&["first", "second"]).packages)),
                Reply::Partial(report(&["first", "second"])),
            ],
        );
        controller.as_mut().rust_mut().worker = Some(worker);
        let shown = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = shown.clone();
        controller
            .as_mut()
            .on_rows_changed(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .release();
        controller.as_mut().poll();
        assert_eq!(shown.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(controller.rows().to_string().contains("second"));
        drop(gate);
        settle(&mut controller);

        // A read that ended without its final report still shows its
        // newest rows.
        let worker = fake_worker(
            Job::Load("Installed".into(), String::new()),
            vec![
                Reply::Partial(report(&["third"])),
                Reply::Partial(report(&["third", "fourth"])),
            ],
        );
        while !worker.handle.is_finished() {
            thread::yield_now();
        }
        controller.as_mut().rust_mut().worker = Some(worker);
        controller.as_mut().poll();
        assert!(controller.rust().worker.is_none());
        assert!(controller.rows().to_string().contains("fourth"));
    }
    #[test]
    fn huge_searches_keep_only_the_best_matches() {
        let package = |name: String| Package {
            id: PackageId {
                backend: "fixture".into(),
                name,
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: String::new(),
            summary: "Fixture".into(),
            installed_version: None,
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        };
        let mut report = PackageReport {
            packages: (0..2000)
                .map(|i| package(format!("lib-{i:04}-v")))
                .chain([package("v".into()), package("vlc".into())])
                .collect(),
            ..PackageReport::default()
        };
        assert_eq!(keep_best_matches(&mut report, "V "), 2002);
        assert_eq!(report.packages.len(), SEARCH_ROW_LIMIT);
        // The exact name and a prefix match come first, as the list ranks.
        let names: Vec<_> = report.packages.iter().map(|p| p.id.name.as_str()).collect();
        assert_eq!(names[..3], ["v", "vlc", "lib-0000-v"]);
        // Smaller searches are left as they are.
        let mut small = PackageReport {
            packages: vec![package("vlc".into()), package("v".into())],
            ..PackageReport::default()
        };
        assert_eq!(keep_best_matches(&mut small, "v"), 2);
        let names: Vec<_> = small.packages.iter().map(|p| p.id.name.as_str()).collect();
        assert_eq!(names, ["vlc", "v"]);

        // The page says how many matched in all.
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().natives = Natives {
            warm_catalog: true,
            ..SYNTHETIC
        };
        controller
            .as_mut()
            .load("Search".into(), "v".into(), "".into(), false, false);
        settle(&mut controller);
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Load("Search".into(), "v".into()),
            vec![
                Reply::Matches(2002),
                Reply::Done(Ok(Payload::Packages(report))),
            ],
        ));
        controller.as_mut().poll();
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["matches"], 2002);
        // A new search starts counting again.
        controller
            .as_mut()
            .start(Job::Load("Search".into(), "vl".into()));
        assert_eq!(controller.rust().search_matches, 0);
        settle(&mut controller);
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
                adopt_with: None,
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
        let empty = plan_checked_upgrade(
            &packages,
            &format!("[{}]", row("vanished")),
            &FailedUpdates::default(),
        );
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
            &FailedUpdates::default(),
        );
        assert_eq!(plan.operations.len(), 1);
        assert!(matches!(&plan.operations[0], Operation::Upgrade(id) if id.name == "upgradable"));
        assert!(plan.status.is_none());
        assert!(plan.confirmation.contains("Update 1 selected package?"));
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
        let text = failure_details(&failure, false).to_string();
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
                adopt_with: None,
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
            adopt_with: None,
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
        assert_eq!(replies.len(), 5);
        let asked = expect!(replies.remove(0), Reply::Asked(sources) => sources);
        assert_eq!(asked, ["fixture"]);
        // Each report says how many packages matched before it arrives.
        assert_eq!(
            expect!(replies.remove(0), Reply::Matches(count) => count),
            1
        );
        let partial = expect!(replies.remove(0), Reply::Partial(report) => report);
        assert_eq!(
            expect!(replies.remove(0), Reply::Matches(count) => count),
            1
        );
        let terminal =
            expect!(replies.remove(0), Reply::Done(Ok(Payload::Packages(report))) => report);
        assert_eq!(terminal, partial);
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
            adopt_with: None,
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
            assert_eq!(replies.len(), 4);
            let asked = expect!(replies.remove(0), Reply::Asked(sources) => sources);
            assert_eq!(asked, ["fixture"]);
            let partial = expect!(replies.remove(0), Reply::Partial(report) => report);
            assert_eq!(partial.packages, vec![package.clone()]);
            assert!(matches!(
                replies.remove(0),
                Reply::Inventory(_, refreshed) if refreshed == (view == "Updates")
            ));
            let terminal =
                expect!(replies.remove(0), Reply::Done(Ok(Payload::Packages(report))) => report);
            assert_eq!(terminal, partial);
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
            adopt_with: None,
        };
        assert_eq!(package_row(&package, &[], None)["source"], "fixture");
        assert!(encoded(package_row(&package, &[], None))
            .to_string()
            .contains("synthetic"));
        let mut firmware = package.clone();
        firmware.id.backend = "fwupd".into();
        firmware.display_name = "Synthetic BIOS".into();
        firmware.summary = "Firmware, AC power required, Restart required".into();
        for tool in pkgdeck_core::backends::StandaloneTool::ALL {
            let mut standalone = package.clone();
            standalone.id.backend = tool.id().into();
            standalone.id.reference = Some("/synthetic/bin/tool".into());
            assert_eq!(
                upgrade_plan(&[standalone.clone()]),
                vec![Operation::Upgrade(standalone.id)]
            );
        }
        for backend in ["rustup", "pixi", "nix"] {
            let mut toolchain = package.clone();
            toolchain.id.backend = backend.into();
            assert_eq!(
                upgrade_plan(&[toolchain.clone()]),
                vec![Operation::Upgrade(toolchain.id)]
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
            &FailedUpdates::default(),
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
            let status = expect!(
                replies.pop().unwrap(),
                Reply::Done(Ok(Payload::Batch(status, _))) => status
            );
            assert!(status.contains("Restart or shut down"));
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
        // One command for each source, however many packages it updates.
        assert_eq!(
            plan,
            vec![
                Operation::UpgradeAll {
                    backend: id.backend.clone()
                },
                Operation::UpgradeAll {
                    backend: other.id.backend.clone()
                }
            ]
        );
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
                        assert!(status.contains("Update all fixture packages"));
                        assert!(status.contains(if fail { "authorization" } else { "cancel" }));
                    }
                    Reply::Done(Ok(Payload::Written(outcome))) => {
                        assert!(outcome.cancellation_deferred)
                    }
                    reply => {
                        let expected = if matches!(job, Job::Details(_)) {
                            EngineError::NotFound
                        } else {
                            pkgdeck_core::process::ExecutionError::AuthorizationDenied.into()
                        };
                        assert!(
                            matches!(&reply, Reply::Done(Err(error)) if *error == expected),
                            "missing terminal reply"
                        );
                    }
                }
            }
        }
    }
    fn fake_worker(job: Job, replies: Vec<Reply>) -> Worker {
        let (sender, receiver) = mpsc::channel();
        for reply in replies {
            sender.send(reply).unwrap();
        }
        Worker {
            handle: thread::spawn(move || drop(sender)),
            receiver,
            cancel: Cancellation::default(),
            job,
        }
    }
    fn idle_controller() -> crate::qt::UniquePtr<ffi::PackageController> {
        let mut controller = ffi::create_controller();
        controller.pin_mut().rust_mut().prefetch.clear();
        controller
    }
    #[test]
    fn source_and_scope_names_read_as_people_know_them() {
        for (id, name) in [
            ("apt", "APT"),
            ("dnf", "DNF"),
            ("pacman", "Pacman"),
            ("zypper", "Zypper"),
            ("snap", "Snap"),
            ("homebrew", "Homebrew"),
            ("homebrew-cask", "Homebrew Casks"),
            ("macos-apps", "macOS Applications"),
            ("mas", "Mac App Store"),
            ("aur", "AUR"),
            ("apk", "apk"),
            ("xbps", "XBPS"),
            ("system-image", "System image"),
            ("macports", "MacPorts"),
            ("rustup", "rustup"),
            ("nix", "Nix"),
            ("go", "Go"),
            ("dotnet", ".NET tools"),
            ("appimage", "AppImage"),
            ("flatpak", "Flatpak"),
            ("docker", "Docker images"),
            ("podman", "Podman images"),
            ("toolbox", "Toolbx containers"),
            ("distrobox", "Distrobox containers"),
            ("cargo", "Cargo"),
            ("npm", "npm"),
            ("pnpm", "pnpm"),
            ("bun", "Bun"),
            ("pip", "pip"),
            ("pipx", "pipx"),
            ("uv", "uv"),
            ("pixi", "pixi"),
            ("conda", "Conda"),
            ("composer", "Composer"),
            ("gem", "RubyGems"),
            ("fwupd", "Firmware"),
            ("codex", "Codex (standalone)"),
            ("claude", "Claude Code (standalone)"),
            ("grok", "Grok (standalone)"),
            ("opencode", "OpenCode (standalone)"),
            ("cursor", "Cursor CLI (standalone)"),
            ("copilot", "GitHub Copilot CLI (standalone)"),
            ("kiro", "Kiro CLI (standalone)"),
            ("antigravity", "Antigravity CLI (standalone)"),
            ("amp", "Amp (standalone)"),
            ("droid", "Factory Droid (standalone)"),
            ("solana", "Solana CLI (Agave)"),
            ("anchor", "Anchor (AVM)"),
            ("foundry", "Foundry"),
            ("fixture", "fixture"),
        ] {
            assert_eq!(source_display_name(id), name);
        }
        // Every source the engine knows has a name of its own.
        for id in pkgdeck_core::backends::BACKEND_IDS {
            assert!(source_display_name(id) != *id || id.chars().all(|c| c.is_lowercase()));
        }
        assert_eq!(
            source_and_scope("flatpak", &Scope::User { uid: 1000 }),
            "Flatpak, User"
        );
        assert_eq!(source_and_scope("apt", &Scope::System), "APT, System");
        assert_eq!(
            scope_word(&Scope::Environment {
                path: PathBuf::from("/tmp/env")
            }),
            "/tmp/env"
        );
        let upgrade_all = Operation::UpgradeAll {
            backend: "flatpak".into(),
        };
        assert_eq!(operation_title(&upgrade_all), "Update all Flatpak packages");
        let mut id = synthetic_package("tool", "Tool").id;
        id.scope = Scope::User { uid: 1000 };
        let label = operation_label(&Operation::Remove(id.clone()));
        assert!(label.contains("Source: APT"));
        assert!(label.contains("Scope: User"));
        assert!(!label.contains("1000"));
        // A downloaded AppImage shows its file name, a Flatpak ref its ref.
        id.backend = "appimage".into();
        id.name = "https://example.invalid/dl/Tool.AppImage?x=1".into();
        id.reference = Some("artifact:sha".into());
        assert_eq!(package_name(&id), "Tool.AppImage");
        // A local AppImage's reference is its checksum, never shown.
        id.name = "/home/user/Downloads/tool.appimage".into();
        id.reference = Some("ab".repeat(32));
        assert_eq!(package_name(&id), "tool.appimage");
        id.backend = "flatpak".into();
        id.reference = Some("app/org.example.Tool/x86_64/stable".into());
        assert_eq!(package_name(&id), "app/org.example.Tool/x86_64/stable");
        // An App Store app shows its name, not its App Store ID.
        id.backend = "mas".into();
        id.name = "iMovie".into();
        id.reference = Some("408981434".into());
        assert_eq!(package_name(&id), "iMovie");
        assert_eq!(
            operation_title(&Operation::Upgrade(id)),
            "Update iMovie (Mac App Store)"
        );
    }
    #[test]
    fn repository_text_names_sources_and_scopes() {
        let action = RepositoryAction {
            backend: "flatpak".into(),
            name: "flathub".into(),
            scope: Scope::System,
            change: repositories::Change::Remove,
        };
        assert_eq!(
            repository_label(&action),
            "Remove repository\nflathub, Flatpak, System"
        );
        for (change, text) in [
            (
                repositories::Change::Add {
                    url: "https://example.invalid".into(),
                },
                "Add repository from https://example.invalid",
            ),
            (
                repositories::Change::SetEnabled { enabled: true },
                "Enable repository",
            ),
            (
                repositories::Change::SetEnabled { enabled: false },
                "Disable repository",
            ),
            (
                repositories::Change::SetPriority { priority: 5 },
                "Set repository priority to 5",
            ),
            (repositories::Change::OpenEditor, "Open Software Sources"),
        ] {
            let action = RepositoryAction {
                change,
                ..action.clone()
            };
            assert!(repository_label(&action).starts_with(text));
        }
        assert_eq!(
            repository_error("flatpak: host command failed (Some(1)): error: No remote refs\n"),
            "Flatpak couldn't run: No remote refs."
        );
        assert_eq!(
            repository_error("apt: listing failed"),
            "APT: Listing failed."
        );
        assert_eq!(
            repository_error("/etc/apt/sources.list: unreadable"),
            "/etc/apt/sources.list: unreadable."
        );
    }
    #[test]
    fn failed_commands_keep_their_output_for_the_banner() {
        use pkgdeck_core::process::{Completion, ExecutionError as E};
        let failed = |stdout: &str, stderr: &str| {
            EngineError::Execution(E::Failed(Completion {
                code: Some(101),
                signal: None,
                stdout: stdout.as_bytes().to_vec(),
                stderr: stderr.as_bytes().to_vec(),
                truncated: false,
                cancellation_deferred: false,
            }))
        };
        assert_eq!(
            raw_failure_output(&failed("built 3 crates\n", "error: linker failed\n")).as_deref(),
            Some("built 3 crates\nerror: linker failed")
        );
        assert_eq!(raw_failure_output(&failed("", "  \n")), None);
        assert_eq!(raw_failure_output(&EngineError::NotFound), None);
        // A long log keeps its end, where the reason is.
        let long = format!("{}\nthe reason", "x".repeat(FAILURE_OUTPUT_LIMIT * 2));
        let kept = raw_failure_output(&failed("", &long)).unwrap();
        assert!(kept.starts_with('…') && kept.ends_with("the reason"));
        // The cut moves forward off the middle of a character.
        let accents = format!("{}x", "é".repeat(FAILURE_OUTPUT_LIMIT));
        let kept = raw_failure_output(&failed("", &accents)).unwrap();
        assert!(kept.starts_with("…é") && kept.ends_with('x'));
    }
    #[test]
    fn retry_runs_only_the_steps_that_failed() {
        let names = |job: &Option<Job>| match job {
            Some(Job::UpgradeAll(operations, _) | Job::CleanAll(operations)) => operations.len(),
            _ => 0,
        };
        let upgrade = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        let job = Job::UpgradeAll(vec![upgrade("cargo"), upgrade("npm")], None);
        let partial = Ok(Payload::Batch(
            String::new(),
            vec![Outcome::Finished, Outcome::Failed],
        ));
        assert_eq!(names(&retry_job(&job, &partial)), 1);
        let none_failed = Ok(Payload::Batch(
            String::new(),
            vec![Outcome::Finished, Outcome::Finished],
        ));
        assert!(retry_job(&job, &none_failed).is_none());
        assert_eq!(names(&retry_job(&job, &Err(EngineError::NotFound))), 2);
        assert!(retry_job(&job, &Err(EngineError::Cancelled)).is_none());
        // A step that finished has nothing to retry.
        assert!(retry_job(&job, &Ok(Payload::Written(OperationOutcome::default()))).is_none());
        assert_eq!(names(&None), 0);
        // Cleanups retry the same way.
        let cache = |backend: &str| {
            Operation::Clean(CleanupId {
                backend: backend.into(),
                key: "cache".into(),
            })
        };
        let clean = Job::CleanAll(vec![cache("apt"), cache("npm")]);
        let one_failed = Ok(Payload::Batch(
            String::new(),
            vec![Outcome::Failed, Outcome::Finished],
        ));
        assert_eq!(names(&retry_job(&clean, &one_failed)), 1);
        assert!(retry_job(&clean, &none_failed).is_none());
    }
    #[test]
    fn a_failed_change_offers_its_output_and_retry_once() {
        use pkgdeck_core::process::{Completion, ExecutionError as E};
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        let job = Job::Write(Operation::Upgrade(row.id.clone()), None);
        let failure = EngineError::Execution(E::Failed(Completion {
            code: Some(1),
            signal: None,
            stdout: vec![],
            stderr: b"the tool said no".to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }));
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            job,
            vec![
                Reply::FailureOutput("Upgrade\nthe batch log".into()),
                Reply::Done(Err(failure)),
            ],
        ));
        controller.as_mut().poll();
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["kind"], "error");
        assert_eq!(notice["output"], "Upgrade\nthe batch log");
        assert_eq!(notice["retry"], true);
        // Retry clears the banner and runs the change again, once.
        controller.as_mut().retry_change();
        assert_eq!(controller.notice().to_string(), "{}");
        assert!(controller.rust().worker.is_some());
        settle(&mut controller);
        let finished = controller.notice().to_string();
        assert!(finished.contains("\"success\""));
        controller.as_mut().retry_change();
        assert_eq!(controller.notice().to_string(), finished);
    }
    #[test]
    fn engine_errors_become_one_plain_sentence() {
        use pkgdeck_core::process::{Completion, ExecutionError as E};
        let failed = |stderr: &str| {
            EngineError::Execution(E::Failed(Completion {
                code: Some(1),
                signal: None,
                stdout: vec![],
                stderr: stderr.as_bytes().to_vec(),
                truncated: false,
                cancellation_deferred: false,
            }))
        };
        let rustup = failed("error: rustup could not choose a version of cargo to run, because one wasn't specified explicitly, and no default is configured.\nhelp: run 'rustup default stable'\n");
        assert_eq!(
            plain_error(&rustup, Some("cargo"), false),
            "Cargo couldn't run: rustup has no default toolchain."
        );
        // Cargo names the dependency whose build failed.
        let openssl = failed("error: failed to run custom build command for `openssl-sys v0.9.100`\n\nCaused by:\n  Could not find openssl via pkg-config\n");
        assert_eq!(
            plain_error(&openssl, Some("cargo"), false),
            "Cargo couldn't run: building `openssl-sys v0.9.100` failed. It needs OpenSSL's development files (libssl-dev or openssl-devel) and pkg-config."
        );
        // The same crate failing for another reason gets no install advice.
        let old_openssl = failed("error: failed to run custom build command for `openssl-sys v0.6.7`\n\nCaused by:\n  process didn't exit successfully\n");
        assert_eq!(
            plain_error(&old_openssl, Some("cargo"), false),
            "Cargo couldn't run: building `openssl-sys v0.6.7` failed. Run the update in a terminal to read the build log."
        );
        let other = failed("error: failed to run custom build command for `ring v0.17.0`\n");
        assert_eq!(
            plain_error(&other, Some("cargo"), false),
            "Cargo couldn't run: building `ring v0.17.0` failed. Run the update in a terminal to read the build log."
        );
        // Flatpak names the remote and the reason, not the URL.
        let remote = failed("error: Unable to load summary from remote astrovm: While fetching https://flatpak.4st.li/repo/summary.idx: [35] SSL connect error\n");
        assert_eq!(
            plain_error(&remote, Some("flatpak"), false),
            "Flatpak couldn't run: it can't reach the remote astrovm (SSL connect error)."
        );
        // The same text reached through Display, with the debug exit code.
        assert_eq!(
            plain_text(&rustup.to_string(), Some("cargo")),
            "Cargo couldn't run: rustup has no default toolchain."
        );
        assert_eq!(
            plain_text(
                "host command failed (Some(100)): \nE: Unable to locate package x\n",
                None
            ),
            "Unable to locate package x."
        );
        assert_eq!(
            plain_text("host command failed (None): ", Some("apt")),
            "APT reported an error without details."
        );
        assert_eq!(
            plain_text("host command failed (None): ", None),
            "The package manager reported an error without details."
        );
        // Stdout is used when a tool reports only there.
        let quiet = EngineError::Execution(E::Failed(Completion {
            code: Some(1),
            signal: None,
            stdout: b"npm ERR! 404 Not Found\n".to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }));
        assert_eq!(
            plain_error(&quiet, Some("npm"), false),
            "npm couldn't run: npm ERR! 404 Not Found."
        );
        assert_eq!(
            plain_error(&failed("   \n"), None, false),
            "The package manager reported an error."
        );
        assert_eq!(plain_error(&failed("boom"), None, false), "Boom.");
        let sudo_failed = failed(
            "Error: Failure while executing; `/usr/bin/sudo -E -- /bin/rm -r -f --` exited with 1.",
        );
        assert!(plain_error(&sudo_failed, Some("homebrew-cask"), false)
            .starts_with("Homebrew needed your administrator password"));
        assert!(plain_error(&sudo_failed, Some("npm"), false).starts_with("npm couldn't run"));
        // Running as root is explained, whatever layer said it.
        assert_eq!(
            plain_error(
                &E::Disabled("run the frontend as an unprivileged user".into()).into(),
                Some("apt"),
                false
            ),
            ROOT_MESSAGE
        );
        assert_eq!(
            plain_error(
                &EngineError::Unavailable {
                    backend: "apt".into(),
                    reason: "run the frontend as an unprivileged user".into()
                },
                None,
                false
            ),
            ROOT_MESSAGE
        );
        assert_eq!(
            plain_error(
                &EngineError::InvalidResponse {
                    backend: "snap".into(),
                    reason: "run the frontend as an unprivileged user".into()
                },
                None,
                false
            ),
            ROOT_MESSAGE
        );
        assert_eq!(
            plain_error(
                &EngineError::InvalidResponse {
                    backend: "snap".into(),
                    reason: "unexpected output Some(\"x\")".into()
                },
                None,
                false
            ),
            "Snap reported a problem: unexpected output x."
        );
        assert_eq!(
            plain_error(
                &EngineError::InvalidResponse {
                    backend: "snap".into(),
                    reason: " ".into()
                },
                None,
                false
            ),
            "Snap reported a problem."
        );
        let skipped = EngineError::InvalidResponse {
            backend: "macos-apps".into(),
            reason: "skipped folder /Users/me/Applications/Locked: Permission denied (os error 13)"
                .into(),
        };
        assert_eq!(
    plain_error(&skipped, Some("macos-apps"), false),
    "Couldn't read /Users/me/Applications/Locked (permission denied). The other apps are still listed."
);
        assert_eq!(failure_kind(&skipped), "partial");
        // mas asks for the password itself, whatever PkgDeck's setting is.
        let denied =
            EngineError::Execution(pkgdeck_core::process::ExecutionError::AuthorizationDenied);
        for sudo in [false, true] {
            assert!(plain_error(&denied, Some("mas"), sudo).contains("pkd upgrade --from mas"));
        }
        let elsewhere = EngineError::InvalidResponse {
            backend: "snap".into(),
            reason: "skipped folder /x: denied".into(),
        };
        assert_eq!(failure_kind(&elsewhere), "failed");
        assert_eq!(
            plain_error(
                &EngineError::InvalidResponse {
                    backend: "open".into(),
                    reason: "Only local file URLs are supported.".into()
                },
                None,
                false
            ),
            "Only local file URLs are supported."
        );
        assert_eq!(
            plain_error(
                &EngineError::Unavailable {
                    backend: "apt".into(),
                    reason: "APT lists are missing".into()
                },
                None,
                false
            ),
            "APT isn't available: APT lists are missing."
        );
        assert_eq!(
            plain_error(
                &EngineError::Unsupported {
                    backend: "fwupd".into(),
                    capability: Capability::Install
                },
                None,
                false
            ),
            "Firmware can't do this."
        );
        assert_eq!(
            plain_error(&EngineError::UnknownBackend("x".into()), None, false),
            "Unknown backend: x."
        );
        assert_eq!(plain_error(&E::Cancelled.into(), None, false), "Cancelled.");
        assert_eq!(plain_text("", None), "Something went wrong.");
        assert_eq!(sentence("  "), "");
        assert_eq!(sentence("done!"), "Done!");
        assert_eq!(strip_debug("code Some(3 and more"), "code 3 and more");
        assert_eq!(strip_debug("exit (None) now"), "exit  now");
        assert_eq!(lower_first("APT lists"), "APT lists");
        assert_eq!(lower_first("Broken"), "broken");
        assert_eq!(meaningful_line("\n\n"), None);
        assert_eq!(
            meaningful_line(
                "WARNING: apt does not have a stable CLI interface.\nReading\nE: Held packages"
            ),
            Some("Held packages".into())
        );
        // Batch lines keep the engine's denial until it is explained here.
        assert_eq!(
            batch_status_text(
                "Update x (APT): authorization denied or unavailable\nok",
                true
            ),
            format!(
                "Update x (APT): {}\nok",
                plain_error(&E::AuthorizationDenied.into(), None, true)
            )
        );
    }
    #[test]
    fn errors_shown_by_the_controller_are_plain() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().apply(Err(EngineError::Execution(
            pkgdeck_core::process::ExecutionError::Disabled(
                "run the frontend as an unprivileged user".into(),
            ),
        )));
        assert_eq!(controller.status().to_string(), ROOT_MESSAGE);
        let failure = BackendFailure {
            backend: "cargo".into(),
            error: EngineError::InvalidResponse {
                backend: "cargo".into(),
                reason: "host command failed (Some(1)): error: rustup could not choose a version of cargo to run".into(),
            },
        };
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport {
                packages: vec![synthetic_package("htop", "htop")],
                failures: vec![failure.clone()],
                successful_sources: vec!["apt".into()],
            })));
        let expected = "Cargo couldn't run: rustup has no default toolchain.";
        assert!(controller
            .status()
            .to_string()
            .contains(&format!("Cargo: {expected}")));
        let rows: Value = serde_json::from_str(&controller.rows().to_string()).unwrap();
        assert_eq!(rows[1]["summary"], expected);
        assert_eq!(rows[1]["display_name"], "Cargo");
        assert_eq!(rows[1]["source"], "cargo");
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["failures"][0]["detail"], expected);
        controller
            .as_mut()
            .apply(Ok(Payload::Cleanup(CleanupReport {
                items: vec![],
                failures: vec![failure],
            })));
        let rows: Value = serde_json::from_str(&controller.rows().to_string()).unwrap();
        assert_eq!(rows[0]["summary"], expected);
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["failures"][0]["detail"], expected);
        assert!(state["checked_at"].as_u64().unwrap() > 0);
    }
    #[test]
    fn activity_reads_share_the_lock_and_never_write() {
        use rustix::fs::{flock, FlockOperation};
        let path =
            std::env::temp_dir().join(format!("pkgdeck-activity-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let store = ActivityStore::at(path.join("activity.json"));
        // No history yet reads as empty, without creating anything.
        assert!(store.load().unwrap().is_empty());
        assert!(!path.exists());
        let operation = Operation::Refresh {
            backend: "apt".into(),
        };
        let id = store
            .begin("gui", vec![operation.clone()], State::Finished)
            .unwrap();
        // Another reader holding the lock shared does not block this read.
        let lock = std::fs::File::open(path.join("activity.lock")).unwrap();
        flock(&lock, FlockOperation::LockShared).unwrap();
        let modified = std::fs::metadata(path.join("activity.json"))
            .unwrap()
            .modified()
            .unwrap();
        let entries = store.load().unwrap();
        assert_eq!(entries[0].id, id);
        assert_eq!(
            std::fs::metadata(path.join("activity.json"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        drop(lock);
        // An entry whose owner died is marked interrupted once.
        store.begin("gui", vec![operation], State::Running).unwrap();
        let mut raw: Vec<Value> =
            serde_json::from_slice(&std::fs::read(path.join("activity.json")).unwrap()).unwrap();
        raw[1]["owner_pid"] = json!(u32::MAX - 1);
        std::fs::write(
            path.join("activity.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        assert_eq!(store.load().unwrap()[1].state, State::Interrupted);
        // A broken file reads as no history rather than failing.
        std::fs::write(path.join("activity.json"), b"not json").unwrap();
        assert!(read_activity(&path.join("activity.json"))
            .unwrap()
            .is_empty());
        // A directory where the file should be is an error.
        std::fs::remove_file(path.join("activity.json")).unwrap();
        std::fs::create_dir(path.join("activity.json")).unwrap();
        assert!(read_activity(&path.join("activity.json")).is_err());
        std::fs::remove_dir(path.join("activity.json")).unwrap();
        std::fs::remove_file(path.join("activity.lock")).unwrap();
        std::fs::create_dir(path.join("activity.lock")).unwrap();
        assert!(read_activity(&path.join("activity.json")).is_ok());
        std::fs::remove_dir_all(path).unwrap();
        let _ = Some(&store).map(|store| store.history.clone());
        assert!(ActivityStore::default_store().is_some() || std::env::var_os("HOME").is_none());
    }
    #[test]
    fn activity_refreshes_off_the_gui_thread_and_repeats_when_asked_again() {
        let path =
            std::env::temp_dir().join(format!("pkgdeck-activity-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let store = ActivityStore::at(path.join("activity.json"));
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        // Without a store nothing starts.
        controller.as_mut().refresh_activity();
        assert!(controller.rust().activity_worker.is_none());
        controller.as_mut().poll();
        assert!(!*controller.needs_poll());
        controller.as_mut().rust_mut().activity_store = Some(store.clone());
        controller.as_mut().refresh_activity();
        assert!(*controller.needs_poll());
        // A second request while reading reads again afterwards.
        store
            .begin(
                "gui",
                vec![Operation::Refresh {
                    backend: "flatpak".into(),
                }],
                State::Queued,
            )
            .unwrap();
        controller.as_mut().refresh_activity();
        assert!(controller.rust().activity_again);
        wait_until(&mut controller, |controller| {
            controller
                .activity()
                .to_string()
                .contains("Refresh Flatpak package lists")
                && controller.rust().activity_worker.is_none()
        });
        assert!(!controller.rust().activity_again);
        assert!(!*controller.needs_poll());
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn needs_poll_is_true_exactly_while_work_is_outstanding() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        // Startup preloads wait for poll().
        assert!(*controller.needs_poll());
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().poll();
        assert!(!*controller.needs_poll());
        // A confirmation holds preloads back; declining lets them run.
        controller.as_mut().rust_mut().prefetch = vec!["Clean".into()];
        controller.as_mut().rust_mut().pending = Some(Job::CleanAll(vec![]));
        controller.as_mut().sync_needs_poll();
        assert!(!*controller.needs_poll());
        controller.as_mut().confirm(false);
        assert!(*controller.needs_poll());
        controller.as_mut().rust_mut().prefetch.clear();
        // Details start synchronously and flip it at once.
        controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
        let mut package = synthetic_package("grok", "Grok");
        package.id.backend = "grok".into();
        controller.as_mut().rust_mut().packages = vec![package];
        controller.as_mut().poll();
        assert!(!*controller.needs_poll());
        controller.as_mut().select(0);
        assert!(*controller.needs_poll());
        wait_until(&mut controller, |controller| {
            controller.rust().details_worker.is_none()
        });
        controller.as_mut().poll();
        assert!(!*controller.needs_poll());
        // Cancelling with nothing running keeps it false.
        controller.as_mut().cancel();
        assert!(!*controller.needs_poll());
        // Each worker kind counts.
        let mut rust = Controller::default();
        rust.prefetch.clear();
        assert!(!rust.outstanding());
        rust.awaiting_prefetch = true;
        assert!(rust.outstanding());
        rust.awaiting_prefetch = false;
        rust.catalog_worker = Some(CatalogWorker {
            handle: thread::spawn(|| {}),
            receiver: mpsc::channel().1,
            cancel: Cancellation::default(),
        });
        assert!(rust.outstanding());
        let worker = rust.catalog_worker.take().unwrap();
        worker.handle.join().unwrap();
        rust.worker = Some(fake_worker(Job::Load("Clean".into(), "".into()), vec![]));
        assert!(rust.outstanding());
        assert!(rust.foreground_worker());
        rust.refreshing = true;
        assert!(!rust.foreground_worker());
        rust.worker.take().unwrap().handle.join().unwrap();
    }
    #[test]
    fn returning_to_a_recent_section_publishes_it_at_once() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Updates", "", &[], false);
        let mut cached = cached_view("recent update");
        cached.report_state = r#"{"phase":"complete","checked_at":1234}"#.into();
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key, cached);
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, false);
        assert!(controller.rows().to_string().contains("recent update"));
        assert!(!*controller.busy());
        assert!(!*controller.refreshing());
        assert!(controller.rust().worker.is_none());
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["phase"], "cached");
        assert_eq!(state["checked_at"], 1234);
    }
    #[test]
    fn returning_to_an_old_section_refreshes_quietly_behind_it() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Installed", "", &[], false);
        let mut cached = cached_view("old package");
        cached.loaded = Instant::now() - VIEW_TTL;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key.clone(), cached);
        // The previous section still reads in the background.
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker =
            Some(fake_worker(Job::Load("Sources".into(), "".into()), vec![]));
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        assert!(controller.rows().to_string().contains("old package"));
        assert!(!*controller.busy());
        assert!(*controller.refreshing());
        assert!(*controller.needs_poll());
        assert!(
            matches!(&controller.rust().queued, Some(Job::Load(view, _)) if view == "Installed")
        );
        // Asking for it again keeps the one reload.
        controller.as_mut().rust_mut().background = false;
        controller.as_mut().rust_mut().queued = None;
        let report = PackageReport {
            packages: vec![synthetic_package("fresh", "Fresh")],
            failures: vec![],
            successful_sources: vec!["apt".into()],
        };
        let reload = fake_worker(
            Job::Load("Installed".into(), "".into()),
            vec![Reply::Done(Ok(Payload::Packages(report)))],
        );
        let cancel = reload.cancel.clone();
        controller.as_mut().rust_mut().worker = Some(reload);
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        assert!(!cancel.requested());
        assert!(*controller.refreshing());
        assert!(!*controller.busy());
        // The reload lands: fresh rows, spinner off, checked time recorded.
        wait_until(&mut controller, |controller| {
            controller.rust().worker.is_none()
        });
        assert!(controller.rows().to_string().contains("fresh"));
        assert!(!*controller.refreshing());
        assert!(!*controller.busy());
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert!(state["checked_at"].as_u64().unwrap() > 0);
    }
    #[test]
    fn a_users_request_replaces_a_quiet_refresh() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().active_view = "Installed".into();
        controller.as_mut().set_refreshing(true);
        let refresh = fake_worker(Job::Load("Installed".into(), "".into()), vec![]);
        let cancel = refresh.cancel.clone();
        controller.as_mut().rust_mut().worker = Some(refresh);
        let operation = Operation::Install(synthetic_package("htop", "htop").id);
        controller
            .as_mut()
            .start(Job::PlanOperation(operation.clone()));
        assert!(cancel.requested());
        assert!(!*controller.refreshing());
        assert!(*controller.busy());
        assert!(controller.rust().background);
        assert!(
            matches!(&controller.rust().queued, Some(Job::PlanOperation(op)) if *op == operation)
        );
        // Guards that refused while a foreground read ran let requests
        // through to start() while only a quiet refresh runs.
        controller.as_mut().rust_mut().background = false;
        controller.as_mut().set_refreshing(true);
        controller.as_mut().load_repositories();
        assert!(matches!(
            &controller.rust().queued,
            Some(Job::Repositories(None))
        ));
    }
    #[test]
    fn a_waited_for_preload_of_an_old_section_is_quiet() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Clean", "", &[], false);
        let mut cached = cached_view("old cleanup");
        cached.expired = true;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key.clone(), cached);
        controller.as_mut().rust_mut().prefetch_worker =
            Some(fake_prefetch("Clean", key, vec![], false));
        controller
            .as_mut()
            .load("Clean".into(), "".into(), "".into(), false, false);
        assert!(controller.rust().awaiting_prefetch);
        assert!(*controller.refreshing());
        assert!(!*controller.busy());
    }
    #[test]
    fn a_preloaded_section_says_when_it_was_read() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let key = cache_key("Updates", "", &[], false);
        let loaded = Instant::now() - Duration::from_secs(20);
        controller
            .as_mut()
            .rust_mut()
            .prefetched
            .insert(key, (loaded, Payload::Packages(PackageReport::default())));
        controller
            .as_mut()
            .load("Updates".into(), "".into(), "".into(), false, false);
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        let checked = state["checked_at"].as_u64().unwrap();
        assert!(epoch_seconds() - checked >= 19);
        assert!(!*controller.busy());
        assert!(!*controller.refreshing());
        assert_eq!(epoch_seconds_at(Instant::now()), epoch_seconds());
        // A loading partial carries no checked time.
        controller
            .as_mut()
            .set_package_report_state(&PackageReport::default(), true);
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert!(state.get("checked_at").is_none());
        controller.as_mut().apply(Ok(Payload::Sources(vec![])));
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert!(state["checked_at"].as_u64().is_some());
    }
    #[test]
    fn progress_names_the_rows_it_changes_and_how_far_it_is() {
        let first = synthetic_package("first", "First App").id;
        let second = synthetic_package("second", "Second App").id;
        let names = Names::from([(first.clone(), "First App".to_owned())]);
        let job = Job::UpgradeAll(
            vec![
                Operation::Upgrade(first.clone()),
                Operation::Upgrade(second.clone()),
                Operation::UpgradeAll {
                    backend: "flatpak".into(),
                },
            ],
            None,
        );
        let mut state = ProgressState::new(&job, Some(7), &names);
        let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
        assert_eq!(snapshot["label"], "Update First App (APT)");
        assert_eq!(snapshot["targets"][0], target_row(&first));
        assert_eq!(snapshot["targets"][1]["name"], "second");
        assert_eq!(snapshot["sources"], json!(["flatpak"]));
        assert_eq!(snapshot["fraction"], 0.0);
        assert_eq!(snapshot["current"], Value::Null);
        assert_eq!(snapshot["action"], "");
        state.apply(&Event::Started(Operation::Upgrade(second.clone())));
        state.apply(&Event::Progress {
            operation: Operation::Upgrade(second.clone()),
            progress: Progress::Transfer {
                completed: 50,
                total: Some(100),
            },
        });
        let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
        assert_eq!(snapshot["current"], target_row(&second));
        assert_eq!(snapshot["current_source"], "apt");
        assert_eq!(snapshot["action"], "update");
        assert!((snapshot["fraction"].as_f64().unwrap() - 0.5 / 3.0).abs() < 1e-9);
        assert_eq!(snapshot["finished"], json!([]));
        // A finished package step is listed, a whole-source one is not.
        state.apply(&Event::Finished {
            operation: Operation::Upgrade(second.clone()),
            result: Ok(OperationOutcome::default()),
        });
        state.apply(&Event::Finished {
            operation: Operation::UpgradeAll {
                backend: "flatpak".into(),
            },
            result: Ok(OperationOutcome::default()),
        });
        let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
        assert_eq!(snapshot["finished"], json!([target_row(&second)]));
        // One change: its transfer is the fraction, unknown without one.
        let single = Job::Write(Operation::Install(first.clone()), None);
        let mut state = ProgressState::new(&single, None, &names);
        let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
        assert_eq!(snapshot["fraction"], Value::Null);
        assert_eq!(snapshot["current"], target_row(&first));
        assert_eq!(snapshot["action"], "install");
        state.apply(&Event::Progress {
            operation: Operation::Install(first.clone()),
            progress: Progress::Transfer {
                completed: 30,
                total: Some(40),
            },
        });
        assert_eq!(state.fraction(), Some(0.75));
        state.apply(&Event::Finished {
            operation: Operation::Install(first),
            result: Ok(OperationOutcome::default()),
        });
        assert_eq!(state.fraction(), Some(1.0));
    }
    #[test]
    fn update_all_counts_and_follows_each_package_of_a_source() {
        let cask = |name: &str, shown: &str| {
            let mut package = synthetic_package(name, shown);
            package.id.backend = "homebrew-cask".into();
            package.update = UpdateAvailability::Available;
            package
        };
        let (alpha, beta, gamma) = (
            cask("alpha", "Alpha App"),
            cask("beta", "Beta App"),
            cask("gamma", "Gamma App"),
        );
        let mut current = cask("current", "Current");
        current.update = UpdateAvailability::Current;
        let firmware = {
            let mut package = synthetic_package("bios", "BIOS");
            package.id.backend = "fwupd".into();
            package.id
        };
        let casks = Operation::UpgradeAll {
            backend: "homebrew-cask".into(),
        };
        let flatpak = Operation::UpgradeAll {
            backend: "flatpak".into(),
        };
        let job = Job::UpgradeAll(
            vec![
                casks.clone(),
                flatpak.clone(),
                Operation::Upgrade(firmware.clone()),
            ],
            None,
        );
        let packages = [alpha.clone(), beta.clone(), gamma.clone(), current];
        let mut state = ProgressState::new(&job, None, &Names::new()).with_rows(&packages);
        let snapshot = |state: &ProgressState| -> Value {
            serde_json::from_str(&state.snapshot().to_string()).unwrap()
        };
        // Three casks, one Flatpak step without rows, and the firmware.
        assert_eq!(snapshot(&state)["total"], 5);
        assert_eq!(snapshot(&state)["targets"].as_array().unwrap().len(), 4);
        assert!(!state.observe("==> Upgrading beta"), "nothing runs yet");
        state.apply(&Event::Started(casks.clone()));
        assert!(!state.observe("==> Upgrading 3 outdated packages:"));
        assert!(!state.observe("==> Upgrading unlisted"));
        assert!(state.observe("==> Upgrading beta"));
        let now = snapshot(&state);
        assert_eq!(now["label"], "Update Beta App");
        assert_eq!(now["current"], target_row(&beta.id));
        assert_eq!(now["done"], 0);
        assert!(state.observe("==> Upgrading gamma"));
        assert_eq!(snapshot(&state)["finished"], json!([target_row(&beta.id)]));
        // Sources PkgDeck updates one by one report packages directly, by
        // the name people see as well.
        state.apply(&Event::Progress {
            operation: casks.clone(),
            progress: Progress::Package("Alpha App".into()),
        });
        assert_eq!(snapshot(&state)["done"], 2);
        state.apply(&Event::Finished {
            operation: casks,
            result: Ok(OperationOutcome::default()),
        });
        assert_eq!(snapshot(&state)["done"], 3);
        assert_eq!(snapshot(&state)["finished"].as_array().unwrap().len(), 3);
        state.apply(&Event::Started(flatpak.clone()));
        state.apply(&Event::Finished {
            operation: flatpak,
            result: Ok(OperationOutcome::default()),
        });
        assert_eq!(snapshot(&state)["done"], 4);
        state.apply(&Event::Started(Operation::Upgrade(firmware.clone())));
        assert!(
            !state.observe("==> Upgrading alpha"),
            "not a whole-source step"
        );
        state.apply(&Event::Finished {
            operation: Operation::Upgrade(firmware),
            result: Ok(OperationOutcome::default()),
        });
        assert_eq!(snapshot(&state)["done"], 5);
        assert_eq!(state.fraction(), Some(1.0));

        // The worker passes on only lines that name a package.
        let (sender, receiver) = crate::wake::channel();
        let follow = output_follower(vec!["homebrew-cask".into()], sender);
        follow("==> Downloading https://example.invalid/beta.zip");
        follow("==> Upgrading beta");
        assert!(matches!(
            receiver.try_iter().collect::<Vec<_>>().as_slice(),
            [Reply::Output(line)] if line == "==> Upgrading beta"
        ));
    }
    #[test]
    fn followed_output_moves_the_running_update_all_on() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let mut package = synthetic_package("beta", "Beta App");
        package.id.backend = "homebrew-cask".into();
        package.update = UpdateAvailability::Available;
        let casks = Operation::UpgradeAll {
            backend: "homebrew-cask".into(),
        };
        let job = Job::UpgradeAll(vec![casks.clone()], None);
        let mut state =
            ProgressState::new(&job, None, &Names::new()).with_rows(std::slice::from_ref(&package));
        state.apply(&Event::Started(casks));
        controller.as_mut().rust_mut().progress_state = Some(state);
        let (sender, receiver) = mpsc::channel();
        let (release, hold) = mpsc::channel::<()>();
        sender
            .send(Reply::Output("==> Upgrading beta".into()))
            .unwrap();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(move || {
                let _ = hold.recv();
                drop(sender);
            }),
            receiver,
            cancel: Cancellation::default(),
            job,
        });
        controller.as_mut().poll();
        let progress: Value = serde_json::from_str(&controller.progress().to_string()).unwrap();
        assert_eq!(progress["label"], "Update Beta App");
        assert_eq!(progress["current"], target_row(&package.id));
        drop(release);
        controller.as_mut().rust_mut().progress_state = None;
        controller.as_mut().follow_output("==> Upgrading beta");
        settle(&mut controller);
    }
    #[test]
    fn undo_is_offered_only_where_it_is_safe() {
        let id = synthetic_package("tool", "Tool").id;
        assert_eq!(undo_action(&Operation::Install(id.clone())), Some("remove"));
        assert_eq!(undo_action(&Operation::Remove(id.clone())), Some("install"));
        assert_eq!(undo_action(&Operation::Upgrade(id.clone())), None);
        let mut local = id.clone();
        local.reference = Some("local-deb:/tmp/tool.deb".into());
        assert_eq!(undo_action(&Operation::Remove(local)), None);
        let mut appimage = id.clone();
        appimage.backend = "appimage".into();
        assert_eq!(undo_action(&Operation::Remove(appimage.clone())), None);
        assert_eq!(undo_action(&Operation::Install(appimage)), Some("remove"));
        // Removing from a source PkgDeck can't install from can't be undone.
        for backend in ["codex", "mas", "macos-apps"] {
            let mut restricted = id.clone();
            restricted.backend = backend.into();
            assert_eq!(undo_action(&Operation::Remove(restricted)), None);
        }
        let mut firmware = id;
        firmware.backend = "fwupd".into();
        assert_eq!(undo_action(&Operation::Install(firmware)), None);
        assert_eq!(
            undo_action(&Operation::Clean(CleanupId {
                backend: "apt".into(),
                key: "autoremove".into()
            })),
            None
        );
        // Several changes or none are never undone as one.
        let batch = notice_subject(
            &[
                Operation::Refresh {
                    backend: "apt".into(),
                },
                Operation::Refresh {
                    backend: "snap".into(),
                },
            ],
            &Names::new(),
            true,
        );
        assert_eq!(batch["operation"], "batch");
        assert_eq!(batch["undo"], false);
        assert_eq!(notice_subject(&[], &Names::new(), true)["operation"], "");
        let refresh = notice_subject(
            &[Operation::Refresh {
                backend: "snap".into(),
            }],
            &Names::new(),
            true,
        );
        assert_eq!(refresh["source_name"], "Snap");
        assert!(refresh.get("target").is_none());
        assert_eq!(job_backend(&[]), None);
        assert_eq!(
            operation_kind(&Operation::UpgradeAll {
                backend: "apt".into()
            }),
            "update_all"
        );
        assert_eq!(
            operation_kind(&Operation::Clean(CleanupId {
                backend: "apt".into(),
                key: "autoremove".into()
            })),
            "clean"
        );
    }
    #[test]
    fn confirmed_changes_remember_display_names_for_their_results() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let package = synthetic_package("htop", "Process Viewer");
        controller.as_mut().rust_mut().packages = vec![package.clone()];
        // The remembered names stay bounded.
        for index in 0..300 {
            let id = synthetic_package(&format!("old-{index}"), "Old").id;
            controller
                .as_mut()
                .rust_mut()
                .names
                .insert(id, "Old".into());
        }
        // A busy worker keeps the confirmed change queued.
        controller.as_mut().rust_mut().worker =
            Some(fake_worker(Job::Load("Sources".into(), "".into()), vec![]));
        controller
            .as_mut()
            .accept_confirmed(Job::Write(Operation::Remove(package.id.clone()), None));
        controller.as_mut().rust_mut().packages.clear();
        assert_eq!(controller.rust().names.len(), 1);
        assert_eq!(
            controller.rust().names.get(&package.id).map(String::as_str),
            Some("Process Viewer")
        );
        let notice = write_notice(
            &Job::Write(Operation::Remove(package.id.clone()), None),
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &controller
                .rust()
                .names_for(&[Operation::Remove(package.id)]),
        );
        assert_eq!(notice["title"], "Remove Process Viewer (APT) finished");
        assert_eq!(notice["undo_action"], "install");
        controller.as_mut().cancel_queued();
        let worker = controller.as_mut().rust_mut().worker.take().unwrap();
        worker.handle.join().unwrap();
    }
    #[test]
    fn opened_repositories_and_cleanups_confirm_in_plain_words() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().cleanup = vec![CleanupItem {
            id: CleanupId {
                backend: "apt".into(),
                key: "autoremove".into(),
            },
            title: "Unused packages".into(),
            summary: "Synthetic".into(),
            preview: "Removes 2 packages".into(),
            kind: CleanupKind::OrphanDependencies,
        }];
        controller.as_mut().propose("clean-all".into(), -1);
        assert!(controller
            .confirmation()
            .to_string()
            .contains("Unused packages (APT)"));
        controller.as_mut().change_repository(
            r#"{"backend":"flatpak","name":"flathub","scope":"system","action":"remove"}"#.into(),
        );
        assert!(controller
            .confirmation()
            .to_string()
            .contains("flathub, Flatpak, System"));
    }

    /// A Homebrew Casks stand-in: `adopts` is the app its install would
    /// take over, `None` for an ordinary install.
    struct CaskFixture {
        package: Package,
        adopts: Option<PathBuf>,
    }
    impl Backend for CaskFixture {
        fn id(&self) -> &str {
            "homebrew-cask"
        }
        fn capabilities(&self) -> &[Capability] {
            &[
                Capability::Search,
                Capability::Installed,
                Capability::Install,
            ]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(vec![self.package.clone()])
        }
        fn operation_plan(
            &mut self,
            operation: &Operation,
            _: &Cancellation,
        ) -> Result<Option<TransactionPlan>, EngineError> {
            Ok(self.adopts.clone().map(|app| TransactionPlan {
                operation: operation.clone(),
                native_preview:
                    "Obsidian is already in /Applications (version 1.2.3, signed by TEAM).".into(),
                changes: vec![],
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
                adopts: Some(app),
            }))
        }
    }
    /// An AppImage source stand-in: the file it finds, and the copy its
    /// install would move in (`None` once it's no longer installed elsewhere).
    struct AppImageFixture {
        package: Package,
        adopts: Option<PathBuf>,
    }
    impl Backend for AppImageFixture {
        fn id(&self) -> &str {
            "appimage"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Search, Capability::Install]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn search(&mut self, query: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            if query == "/fail" {
                return Err(EngineError::NotFound);
            }
            Ok(vec![self.package.clone()])
        }
        fn operation_plan(
            &mut self,
            operation: &Operation,
            _: &Cancellation,
        ) -> Result<Option<TransactionPlan>, EngineError> {
            Ok(self.adopts.clone().map(|original| TransactionPlan {
                operation: operation.clone(),
                native_preview: "Moves ~/AppImages/demo.appimage into PkgDeck's folder and replaces its menu entries.".into(),
                changes: vec![],
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
                adopts: Some(original),
            }))
        }
    }
    fn external_appimage() -> Package {
        let mut app = synthetic_package("/home/user/AppImages/demo.appimage", "Demo");
        app.id.backend = "appimage".into();
        app.id.scope = Scope::User { uid: 1000 };
        app.summary = "~/AppImages/demo.appimage".into();
        app.adopt_with = Some("appimage".into());
        app
    }
    fn plan_appimage_adoption_with(adopts: Option<&str>, app: Package) -> Reply {
        let mut file = app.clone();
        file.id.scope = Scope::Environment {
            path: "/home/user/.local/share/pkgdeck/appimages".into(),
        };
        file.id.reference = Some("ab".repeat(32));
        file.summary = "Does demo things".into();
        let mut engine = Engine::default();
        engine
            .register(AppImageFixture {
                package: file,
                adopts: adopts.map(PathBuf::from),
            })
            .unwrap();
        let mut replies = Vec::new();
        execute(
            &mut engine,
            Job::PlanAdoption(Box::new(app)),
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        replies.pop().unwrap()
    }
    #[test]
    fn appimage_adoption_previews_the_move_the_appimage_source_plans() {
        let app = external_appimage();
        assert!(matches!(
            plan_appimage_adoption_with(Some(&app.id.name), app.clone()),
            Reply::Done(Ok(Payload::AdoptionPreview(file, Operation::Install(id), plan)))
                if file.id == id
                    && id.name == app.id.name
                    && plan.adopts.as_deref() == Some(std::path::Path::new(&app.id.name))
        ));
        // No longer installed elsewhere: nothing to take over.
        assert!(matches!(
            plan_appimage_adoption_with(None, app.clone()),
            Reply::Done(Err(EngineError::InvalidResponse { reason, .. })) if reason.contains("isn't installed some other way anymore")
        ));
        // A failed read of the file is reported as is.
        let mut failing = app.clone();
        failing.id.name = "/fail".into();
        assert!(matches!(
            plan_appimage_adoption_with(Some("/fail"), failing),
            Reply::Done(Err(EngineError::NotFound))
        ));
        // Only rows PkgDeck can take over are planned.
        let mut managed = app;
        managed.adopt_with = None;
        assert!(matches!(
            plan_appimage_adoption_with(Some("/x"), managed),
            Reply::Done(Err(EngineError::NotFound))
        ));
    }
    #[test]
    fn managing_every_appimage_reviews_them_once() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let demo = external_appimage();
        let mut other = external_appimage();
        other.id.name = "/home/user/AppImages/other.appimage".into();
        other.display_name = "Other".into();
        // Planned on a worker: what can move, and what can't any more.
        let mut file = demo.clone();
        file.id.reference = Some("ab".repeat(32));
        let mut engine = Engine::default();
        engine
            .register(AppImageFixture {
                package: file,
                adopts: Some(PathBuf::from(&demo.id.name)),
            })
            .unwrap();
        let plan_all = |engine: &mut Engine, cancel: &Cancellation| {
            let mut replies = Vec::new();
            execute(
                engine,
                Job::PlanAdoptAll(vec![demo.clone(), other.clone()]),
                cancel,
                &mut |reply| replies.push(reply),
            );
            replies.pop().unwrap()
        };
        assert!(matches!(
            plan_all(&mut engine, &Cancellation::default()),
            Reply::Done(Ok(Payload::AdoptAllPreview(plans, skipped)))
                if plans.len() == 1 && skipped == ["Other"]
        ));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            plan_all(&mut engine, &cancelled),
            Reply::Done(Err(EngineError::Cancelled))
        ));
        // One review lists every move, then runs them all.
        let plan = |app: &Package, version: Option<&str>| {
            let operation = Operation::Install(app.id.clone());
            let plan = TransactionPlan {
                operation: operation.clone(),
                native_preview: "Moves it into PkgDeck's folder.".into(),
                changes: vec![PlannedChange {
                    action: PlannedAction::Install,
                    name: app.display_name.clone(),
                    installed_version: None,
                    candidate_version: version.map(str::to_owned),
                }],
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
                adopts: Some(PathBuf::from(&app.id.name)),
            };
            (app.clone(), operation, Box::new(plan))
        };
        controller.as_mut().rust_mut().packages = vec![demo.clone(), other.clone()];
        controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
            vec![plan(&demo, Some("2.0")), plan(&other, None)],
            vec![],
        )));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["action"], "Manage all");
        assert_eq!(
            data["changes"],
            json!(["Move Demo 2.0 into PkgDeck", "Move Other into PkgDeck"])
        );
        assert!(data["summary"]
            .as_str()
            .unwrap()
            .starts_with("Manage 2 AppImages with PkgDeck"));
        controller.as_mut().confirm(true);
        let queued: Vec<Vec<Operation>> = controller
            .rust()
            .worker
            .iter()
            .map(|worker| worker.job.operations())
            .chain(
                controller
                    .rust()
                    .confirmed_queue
                    .iter()
                    .map(|entry| entry.job.operations()),
            )
            .collect();
        assert_eq!(queued.len(), 2, "{}", queued.len());
        settle(&mut controller);
        // One that couldn't move is named, and the count reads right.
        controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
            vec![plan(&demo, Some("2.0"))],
            vec!["Other".into()],
        )));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert!(data["summary"]
            .as_str()
            .unwrap()
            .starts_with("Manage 1 AppImage with PkgDeck"));
        assert!(data["notes"][1].as_str().unwrap().contains("Other"));
        controller.as_mut().confirm(false);
        // A later review never runs the moves an earlier one listed.
        controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
            vec![plan(&demo, Some("2.0")), plan(&other, None)],
            vec![],
        )));
        controller.as_mut().rust_mut().pending = Some(Job::PlanCleanAll(vec![]));
        controller.as_mut().confirm(true);
        assert!(controller.rust().confirmed_queue.is_empty());
        assert!(!controller
            .rust()
            .worker
            .as_ref()
            .is_some_and(|worker| matches!(worker.job, Job::Write(..))));
        settle(&mut controller);
        // Just one: the usual review, which says it moves.
        controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
            vec![plan(&demo, Some("2.0"))],
            vec![],
        )));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["action"], "Manage with PkgDeck");
        assert!(
            data.to_string().contains("Move Demo 2.0 into PkgDeck"),
            "{data}"
        );
        controller.as_mut().confirm(false);
        // None left to move.
        controller
            .as_mut()
            .apply(Ok(Payload::AdoptAllPreview(vec![], vec!["Demo".into()])));
        assert_eq!(controller.confirmation().to_string(), "");
        assert!(controller.status().to_string().contains("No AppImage"));
        // The button plans every AppImage installed some other way.
        controller.as_mut().propose("adopt-all".into(), -1);
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanAdoptAll(apps) if apps.len() == 2)
        ));
        settle(&mut controller);
        controller.as_mut().rust_mut().packages = vec![];
        controller.as_mut().propose("adopt-all".into(), -1);
        assert!(controller.rust().worker.is_none());
        // A failed plan says what it was for.
        let notice = preflight_notice(
            &Job::PlanAdoptAll(vec![demo]),
            &EngineError::NotFound,
            false,
            &Names::default(),
        )
        .unwrap();
        assert!(notice["title"]
            .as_str()
            .unwrap()
            .starts_with("Managing your AppImages with PkgDeck"));
    }
    #[test]
    fn appimage_adoption_confirms_as_manage_with_pkgdeck() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let app = external_appimage();
        controller.as_mut().rust_mut().packages = vec![app.clone()];
        let reply = plan_appimage_adoption_with(Some(&app.id.name), app.clone());
        assert!(matches!(
            &reply,
            Reply::Done(Ok(Payload::AdoptionPreview(..)))
        ));
        if let Reply::Done(result) = reply {
            controller.as_mut().apply(result);
        }
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["action"], "Manage with PkgDeck");
        assert!(
            data["summary"]
                .as_str()
                .unwrap()
                .starts_with("Manage Demo with PkgDeck\nAppImage\nMoves ~/AppImages/demo.appimage"),
            "{data}"
        );
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Install(_), Some(plan))) if plan.adopts.is_some()
        ));
        controller.as_mut().confirm(false);
        // Opening a file that's installed some other way previews the move.
        let mut opened = app.clone();
        opened.id.scope = Scope::Environment {
            path: "/home/user/.local/share/pkgdeck/appimages".into(),
        };
        controller
            .as_mut()
            .apply(Ok(opened_payload(opened.clone())));
        // Its page offers Manage, which previews the move.
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["action"], "Manage");
        controller.as_mut().install_opened();
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanAdoption(found) if found.id == opened.id)
        ));
        settle(&mut controller);
        // A failed plan names PkgDeck, not Homebrew.
        let notice = preflight_notice(
            &Job::PlanAdoption(Box::new(app)),
            &EngineError::NotFound,
            false,
            &Names::default(),
        )
        .unwrap();
        assert!(
            notice["title"]
                .as_str()
                .unwrap()
                .starts_with("Managing Demo with PkgDeck"),
            "{notice}"
        );
    }
    fn adoptable_app() -> Package {
        let mut app = synthetic_package("/Applications/Obsidian.app", "Obsidian");
        app.id.backend = "macos-apps".into();
        app.id.architecture = "unknown".into();
        app.id.reference = Some(app.id.name.clone());
        app.installed_version = Some("1.2.3".into());
        app.adopt_with = Some("obsidian".into());
        app
    }
    fn obsidian_cask() -> Package {
        let mut cask = synthetic_package("obsidian", "Obsidian");
        cask.id.backend = "homebrew-cask".into();
        cask.id.architecture = std::env::consts::ARCH.into();
        cask.id.scope = Scope::Environment {
            path: "/opt/homebrew".into(),
        };
        cask.installed_version = None;
        cask.candidate_version = Some("1.2.3".into());
        cask
    }
    fn plan_adoption_with(cask: Package, adopts: Option<&str>, app: Package) -> Reply {
        let mut engine = Engine::default();
        engine
            .register(CaskFixture {
                package: cask,
                adopts: adopts.map(PathBuf::from),
            })
            .unwrap();
        let mut replies = Vec::new();
        execute(
            &mut engine,
            Job::PlanAdoption(Box::new(app)),
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        assert_eq!(replies.len(), 1);
        replies.pop().unwrap()
    }
    #[test]
    fn adoption_resolves_the_real_cask_and_previews_only_this_copy() {
        let cask = obsidian_cask();
        // The identity, scope and prefix come from the cask source.
        assert!(matches!(
            plan_adoption_with(
                cask.clone(),
                Some("/Applications/Obsidian.app"),
                adoptable_app(),
            ),
            Reply::Done(Ok(Payload::AdoptionPreview(found, Operation::Install(id), plan)))
                if *found == cask
                    && id == cask.id
                    && plan.adopts.as_deref()
                        == Some(std::path::Path::new("/Applications/Obsidian.app"))
        ));
        let refused = |reply: Reply, words: &str| {
            assert!(
                matches!(
                    &reply,
                    Reply::Done(Err(EngineError::InvalidResponse { backend, reason }))
                        if backend == "homebrew-cask"
                            && reason.contains(words)
                            && reason.ends_with("Nothing was changed.")
                ),
                "expected a refusal naming {words:?}"
            );
        };
        // An install that would add a copy elsewhere, or no copy at all.
        refused(
            plan_adoption_with(
                cask.clone(),
                Some("/Applications/Other/Obsidian.app"),
                adoptable_app(),
            ),
            "would install another copy instead of managing /Applications/Obsidian.app",
        );
        refused(
            plan_adoption_with(cask.clone(), None, adoptable_app()),
            "would install another copy",
        );
        // Homebrew already has the cask: nothing to hand over.
        let mut installed = cask.clone();
        installed.installed_version = Some("1.2.3".into());
        refused(
            plan_adoption_with(
                installed,
                Some("/Applications/Obsidian.app"),
                adoptable_app(),
            ),
            "already has the obsidian cask installed",
        );
        // An unnamed app is called by its cask.
        let mut installed = cask.clone();
        installed.installed_version = Some("1.2.3".into());
        let mut unnamed = adoptable_app();
        unnamed.display_name.clear();
        refused(
            plan_adoption_with(installed, Some("/Applications/Obsidian.app"), unnamed),
            "can't take over this copy of obsidian.",
        );
        // A cancelled lookup stops before any plan.
        let mut engine = Engine::default();
        engine
            .register(CaskFixture {
                package: cask.clone(),
                adopts: None,
            })
            .unwrap();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            plan_adoption(&mut engine, &adoptable_app(), &cancel),
            Err(EngineError::Cancelled)
        ));
        // Only macOS inventory rows that name a cask are resolved.
        let mut plain = adoptable_app();
        plain.adopt_with = None;
        let mut foreign = adoptable_app();
        foreign.id.backend = "apt".into();
        for app in [plain, foreign] {
            assert!(matches!(
                plan_adoption_with(cask.clone(), Some("/Applications/Obsidian.app"), app),
                Reply::Done(Err(EngineError::NotFound))
            ));
        }
        assert_eq!(
            engine_source(&Job::PlanAdoption(Box::new(adoptable_app())), &[]),
            ["homebrew-cask"]
        );
        // The AppImage source takes over AppImages itself.
        let mut appimage = adoptable_app();
        appimage.id.backend = "appimage".into();
        appimage.adopt_with = Some("appimage".into());
        assert_eq!(
            engine_source(&Job::PlanAdoption(Box::new(appimage)), &[]),
            ["appimage"]
        );
    }

    #[test]
    fn appimages_open_whatever_their_file_is_called() {
        let dir =
            std::env::temp_dir().join(format!("pkgdeck-appimage-magic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut header = vec![0_u8; 64];
        header[..4].copy_from_slice(b"\x7fELF");
        header[8..11].copy_from_slice(b"AI\x02");
        let named = dir.join("tool");
        std::fs::write(&named, &header).unwrap();
        assert!(is_appimage(&named));
        header[8] = 0;
        std::fs::write(&named, &header).unwrap();
        assert!(!is_appimage(&named));
        assert!(!is_appimage(&dir.join("missing")));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn adoption_is_proposed_only_for_rows_a_cask_can_manage() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        let mut plain = adoptable_app();
        plain.adopt_with = None;
        let mut other = synthetic_package("obsidian", "Obsidian");
        other.adopt_with = Some("obsidian".into());
        controller.as_mut().rust_mut().packages = vec![adoptable_app(), plain, other];
        assert_eq!(
            package_row(&adoptable_app(), &[], None)["adopt_with"],
            "obsidian"
        );
        assert!(package_row(&controller.rust().packages[1], &[], None)["adopt_with"].is_null());
        // A quiet background read keeps the job queued, so no real Homebrew
        // is asked.
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Load("Installed".into(), "".into()),
            vec![],
        ));
        for index in [1, 2, 3, -1] {
            controller.as_mut().propose("adopt".into(), index);
            assert!(controller.rust().pending.is_none());
            assert!(!matches!(
                controller.rust().queued,
                Some(Job::PlanAdoption(_))
            ));
        }
        controller.as_mut().propose("adopt".into(), 0);
        assert!(matches!(
            &controller.rust().queued,
            Some(Job::PlanAdoption(app)) if **app == adoptable_app()
        ));
        // The read-only row still never offers its own writes.
        controller.as_mut().rust_mut().queued = None;
        for action in ["install", "remove", "upgrade"] {
            controller.as_mut().propose(action.into(), 0);
            assert!(controller.rust().pending.is_none());
            assert!(controller.rust().queued.is_none());
        }
        controller.as_mut().rust_mut().worker = None;
    }
    #[test]
    fn adoption_confirms_in_plain_words_and_offers_no_undo() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().packages = vec![adoptable_app()];
        let cask = obsidian_cask();
        let operation = Operation::Install(cask.id.clone());
        let plan = TransactionPlan {
            operation: operation.clone(),
            native_preview: "Obsidian is already in /Applications (version 1.2.3, signed by TEAM)."
                .into(),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: Some("/Applications/Obsidian.app".into()),
        };
        controller.as_mut().apply(Ok(Payload::AdoptionPreview(
            Box::new(cask.clone()),
            operation.clone(),
            Box::new(plan.clone()),
        )));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["action"], "Manage with Homebrew");
        let summary = data["summary"].as_str().unwrap();
        assert!(
            summary.starts_with("Manage Obsidian with Homebrew\nHomebrew Casks\n"),
            "{summary}"
        );
        assert!(summary.contains("already in /Applications"));
        assert!(controller
            .confirmation()
            .to_string()
            .contains("already in /Applications"));
        assert!(
            matches!(&controller.rust().pending, Some(Job::Write(op, Some(reviewed))) if *op == operation && **reviewed == plan)
        );
        assert_eq!(
            controller.rust().names.get(&cask.id).map(String::as_str),
            Some("Obsidian")
        );
        // Removing the cask afterwards would delete the app the user had.
        let job = Job::Write(operation.clone(), Some(Box::new(plan)));
        let notice = write_notice(
            &job,
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &controller.rust().names,
        );
        assert_eq!(notice["kind"], "success");
        assert_eq!(notice["undo"], false);
        assert_eq!(notice["undo_action"], "");
        let plain = write_notice(
            &Job::Write(operation, None),
            &Ok(Payload::Written(OperationOutcome::default())),
            false,
            &controller.rust().names,
        );
        assert_eq!(plain["undo"], true);
        // A refused adoption explains itself before anything runs.
        let refusal = preflight_notice(
            &Job::PlanAdoption(Box::new(adoptable_app())),
            &EngineError::InvalidResponse {
                backend: "homebrew-cask".into(),
                reason: "The obsidian cask would install another copy instead of managing /Applications/Obsidian.app. Nothing was changed.".into(),
            },
            false,
            &Names::new(),
        )
        .unwrap();
        assert_eq!(
            refusal["title"],
            "Managing Obsidian with Homebrew couldn't be prepared"
        );
        assert!(refusal["detail"].as_str().unwrap().contains("another copy"));
        controller.as_mut().confirm(false);
        assert!(controller.rust().pending.is_none());
    }

    /// A package from the synthetic system: installed, with an update.
    fn fixture_row() -> Package {
        let mut package = synthetic_package("synthetic", "Synthetic");
        package.id.backend = "fixture".into();
        package.candidate_version = Some("2".into());
        package.update = UpdateAvailability::Available;
        package
    }
    fn fixture_engine(
        _: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut engine = Engine::default();
        engine.register(Fixture {
            package: fixture_row(),
            fail: false,
        })?;
        Ok(engine)
    }
    fn missing_engine(
        sources: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        Err(EngineError::Unavailable {
            backend: sources.join(","),
            reason: "synthetic system has no managers".into(),
        })
    }
    /// A host with no tools on its PATH, so nothing native can run.
    fn bare_host() -> pkgdeck_core::host::Host {
        pkgdeck_core::host::Host::new(
            pkgdeck_core::host::Runtime::Native,
            BTreeMap::from([("PATH".into(), "/nonexistent/pkgdeck-tests".into())]),
        )
    }
    /// A machine without package managers: every known source is absent,
    /// and an unknown one is refused as the real engine refuses it.
    fn no_engine(
        sources: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        match sources
            .iter()
            .find(|source| !pkgdeck_core::backends::BACKEND_IDS.contains(&source.as_str()))
        {
            Some(unknown) => Err(EngineError::UnknownBackend(unknown.clone())),
            None => Ok(Engine::default()),
        }
    }
    fn no_catalog() -> crate::metadata::Catalog {
        crate::metadata::Catalog::default()
    }
    fn offline(_: &str, _: &Cancellation) -> String {
        String::new()
    }
    /// No local catalog and no provider lookups, so details never leave the test.
    const NO_METADATA: crate::metadata::Sources = crate::metadata::Sources {
        catalog: no_catalog,
        fetch: offline,
    };
    /// A machine without the helper: approving never reaches a password prompt.
    fn no_approval(
        _: &pkgdeck_core::host::Host,
        _: bool,
        _: &Cancellation,
    ) -> Result<String, ExecutionError> {
        Err(ExecutionError::Disabled(
            "PkgDeck's system helper is not installed".into(),
        ))
    }
    /// The password was given: the approval names a synthetic helper.
    fn synthetic_approval(
        _: &pkgdeck_core::host::Host,
        grant: bool,
        _: &Cancellation,
    ) -> Result<String, ExecutionError> {
        Ok(if grant { "synthetic-helper" } else { "" }.into())
    }
    pub(super) const NO_MANAGERS: Natives = Natives {
        engine: no_engine,
        host: bare_host,
        approve: no_approval,
        root: "/nonexistent/pkgdeck-tests",
        metadata: NO_METADATA,
        warm_catalog: false,
    };
    #[test]
    fn synthetic_controllers_stay_off_the_system() {
        let mut controller = ffi::create_synthetic_controller(fixture_engine);
        let natives = controller.rust().natives;
        assert_eq!(natives.root, "/nonexistent/pkgdeck");
        assert!(!natives.warm_catalog);
        assert!(
            (natives.metadata.fetch)("https://flathub.org/x", &Cancellation::default()).is_empty()
        );
        let _ = (natives.metadata.catalog)();
        assert!((natives.approve)(&(natives.host)(), true, &Cancellation::default()).is_err());
        assert!(controller.rust().activity_store.is_none());
        let mut controller = controller.pin_mut();
        controller.as_mut().check_sources();
        settle(&mut controller);
        let catalog: Value =
            serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
        assert_eq!(catalog[0]["source"], "fixture");
    }
    #[test]
    fn production_workers_use_the_running_system() {
        assert_eq!(NATIVES.root, "/");
        let cancel = Cancellation::default();
        assert!(matches!(
            (NO_MANAGERS.engine)(&["missing-fixture".into()], false, Authorization::Polkit, &cancel),
            Err(EngineError::UnknownBackend(name)) if name == "missing-fixture"
        ));
        let empty = (NO_MANAGERS.engine)(&["apt".into()], true, Authorization::Polkit, &cancel);
        assert!(empty.unwrap().discover(&cancel).is_empty());
        // Every provider lookup from a test answers nothing.
        assert!((NO_MANAGERS.metadata.fetch)(
            "https://flathub.org/api/v2/appstream/org.example.App",
            &cancel
        )
        .is_empty());
    }
    const SYNTHETIC: Natives = Natives {
        engine: fixture_engine,
        host: bare_host,
        approve: no_approval,
        root: "/nonexistent/pkgdeck-tests",
        metadata: NO_METADATA,
        warm_catalog: false,
    };
    fn synthetic_controller() -> crate::qt::UniquePtr<ffi::PackageController> {
        let mut controller = idle_controller();
        controller.pin_mut().rust_mut().natives = SYNTHETIC;
        controller
    }
    fn settle(controller: &mut Pin<&mut ffi::PackageController>) {
        wait_until(controller, |controller| {
            controller.rust().worker.is_none()
                && controller.rust().details_worker.is_none()
                && controller.rust().catalog_worker.is_none()
        });
    }
    #[test]
    fn background_checks_repositories_and_source_catalogs_use_the_system_workers() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .check_updates("apt,,flatpak".into(), true, false, false, true);
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::BackgroundUpdates(sources) if sources == &["apt", "flatpak"])
        ));
        assert!(!controller.busy());
        settle(&mut controller);
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["available"], 1);

        controller.as_mut().load_repositories();
        settle(&mut controller);
        let repositories: Value =
            serde_json::from_str(&controller.repositories().to_string()).unwrap();
        assert_eq!(repositories["repositories"], json!([]));
        assert_eq!(controller.status().to_string(), "Repositories loaded.");

        controller.as_mut().check_sources();
        settle(&mut controller);
        let catalog: Value =
            serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
        assert_eq!(catalog[0]["source"], "fixture");
        assert_eq!(catalog[0]["availability_kind"], "available");
        assert!(controller.rust().catalog_checked);
    }
    /// Answers details for its one package.
    struct Described(Package);
    impl Backend for Described {
        fn id(&self) -> &str {
            &self.0.id.backend
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Details]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn details(
            &mut self,
            _: &PackageId,
            _: &Cancellation,
        ) -> Result<PackageDetails, EngineError> {
            Ok(PackageDetails {
                package: self.0.clone(),
                description: "Described by the warm engine".into(),
                homepage: None,
                dependencies: vec!["libsynthetic".into()],
            })
        }
    }
    #[test]
    fn details_reuse_a_warm_engine_and_fall_back_to_a_fresh_one() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let mut row = fixture_row();
        row.id.backend = "described".into();
        let mut warm = Engine::default();
        warm.register(Described(row.clone())).unwrap();
        assert_eq!(
            warm.discover(&Cancellation::default())[0].availability,
            Ok(Availability::Available)
        );
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        controller.as_mut().rust_mut().selected = Some(row.id.clone());
        controller.as_mut().rust_mut().engine = Some(warm);
        controller.as_mut().start(Job::Details(row.id.clone()));
        settle(&mut controller);
        let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
        assert_eq!(details["description"], "Described by the warm engine");
        assert_eq!(details["dependencies"], json!(["libsynthetic"]));
        // The warm engine goes back for the next lookup.
        assert!(controller.rust().engine.is_some());

        // The warm engine's own failure is reported as is.
        let mut warm = Engine::default();
        warm.register(Fixture {
            package: fixture_row(),
            fail: false,
        })
        .unwrap();
        controller.as_mut().rust_mut().engine = Some(warm);
        controller.as_mut().start(Job::Details(fixture_row().id));
        settle(&mut controller);
        assert_eq!(
            controller.status().to_string(),
            plain_error(&EngineError::NotFound, None, false)
        );
        assert!(controller.rust().engine.is_some());

        // A source it never detected needs a fresh engine.
        controller.as_mut().rust_mut().natives.engine = missing_engine;
        controller.as_mut().start(Job::Details(row.id.clone()));
        settle(&mut controller);
        assert!(controller
            .status()
            .to_string()
            .contains("synthetic system has no managers"));
    }

    /// A worker whose thread keeps running until the returned gate drops.
    fn held_worker(job: Job, replies: Vec<Reply>) -> (Worker, mpsc::Sender<()>) {
        let (sender, receiver) = mpsc::channel();
        for reply in replies {
            sender.send(reply).unwrap();
        }
        let (gate, held) = mpsc::channel::<()>();
        let worker = Worker {
            handle: thread::spawn(move || {
                let _ = held.recv();
                drop(sender);
            }),
            receiver,
            cancel: Cancellation::default(),
            job,
        };
        (worker, gate)
    }
    fn failing_worker(job: Job) -> Worker {
        Worker {
            handle: thread::spawn(|| panic!("synthetic worker failure")),
            receiver: mpsc::channel().1,
            cancel: Cancellation::default(),
            job,
        }
    }
    fn fixture_details(description: &str) -> Box<PackageDetails> {
        Box::new(PackageDetails {
            package: fixture_row(),
            description: description.into(),
            homepage: None,
            dependencies: vec![],
        })
    }
    #[test]
    fn background_workers_keep_catalogs_and_report_their_own_failure() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let source = Source {
            backend: "fixture".into(),
            capabilities: vec![],
            availability: Ok(Availability::Available),
        };
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Load("Sources".into(), String::new()),
            vec![
                Reply::Engine(Box::default()),
                Reply::Done(Ok(Payload::Sources(vec![source]))),
            ],
        ));
        controller.as_mut().poll();
        assert!(controller.rust().worker.is_none());
        assert!(controller.rust().catalog_checked);
        assert!(controller.source_catalog().to_string().contains("fixture"));
        // A quiet read never leaves its engine for later searches.
        assert!(controller.rust().engine.is_none());
        assert!(controller
            .rust()
            .prefetched
            .contains_key(&cache_key("Sources", "", &[], false)));

        // A quiet result for anything but a section is not kept.
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Details(fixture_row().id),
            vec![Reply::Done(Ok(Payload::Details(fixture_details("Quiet"))))],
        ));
        controller.as_mut().poll();
        assert!(!controller.details().to_string().contains("Quiet"));
        assert_eq!(controller.rust().prefetched.len(), 1);

        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker =
            Some(failing_worker(Job::BackgroundUpdates(vec!["apt".into()])));
        wait_until(&mut controller, |controller| {
            controller.rust().worker.is_none()
        });
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(
            state["failures"],
            json!([{"source": "check", "kind": "failed"}])
        );

        let status = controller.status().to_string();
        controller.as_mut().rust_mut().worker =
            Some(failing_worker(Job::Load("Installed".into(), String::new())));
        wait_until(&mut controller, |controller| {
            controller.rust().worker.is_none()
        });
        assert_ne!(controller.status().to_string(), status);
        assert_eq!(
            controller.status().to_string(),
            "A background task failed. Try again."
        );
    }
    #[test]
    fn running_and_finished_workers_show_details_previews() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        controller.as_mut().rust_mut().selected = Some(row.id.clone());
        let (worker, gate) = held_worker(
            Job::Load("Installed".into(), String::new()),
            vec![
                Reply::Partial(PackageReport {
                    packages: vec![row.clone()],
                    failures: vec![],
                    successful_sources: vec![],
                }),
                Reply::DetailsPreview(fixture_details("While loading")),
                Reply::Engine(Box::default()),
            ],
        );
        controller.as_mut().rust_mut().worker = Some(worker);
        controller.as_mut().poll();
        assert!(controller.rust().worker.is_some());
        assert!(controller.details().to_string().contains("While loading"));
        assert!(controller.rows().to_string().contains("synthetic"));
        // A running worker's engine is not taken early.
        assert!(controller.rust().engine.is_none());
        drop(gate);
        settle(&mut controller);

        controller.as_mut().rust_mut().detail_cache.clear();
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Details(row.id.clone()),
            vec![
                Reply::DetailsPreview(fixture_details("Preview")),
                Reply::Done(Ok(Payload::Details(fixture_details("Finished")))),
            ],
        ));
        controller.as_mut().poll();
        assert!(controller.rust().worker.is_none());
        assert!(controller.details().to_string().contains("Finished"));

        // Flathub's description and screenshots are fetched after the first
        // look, which says more is coming and isn't kept for reopening.
        let mut flathub = fixture_details("");
        flathub.package.id.backend = "flatpak".into();
        flathub.package.id.name = "io.example.PreviewOnly".into();
        flathub.package.id.remote = Some("flathub".into());
        let more = |controller: &Pin<&mut ffi::PackageController>| {
            serde_json::from_str::<Value>(&controller.details().to_string()).unwrap()["more"]
                == true
        };
        controller.as_mut().rust_mut().packages = vec![flathub.package.clone()];
        controller.as_mut().rust_mut().selected = Some(flathub.package.id.clone());
        controller.as_mut().rust_mut().detail_cache.clear();
        let (worker, gate) = held_worker(
            Job::Details(flathub.package.id.clone()),
            vec![Reply::DetailsPreview(flathub.clone())],
        );
        controller.as_mut().rust_mut().worker = Some(worker);
        controller.as_mut().poll();
        assert!(more(&controller));
        assert!(controller.rust().detail_cache.is_empty());
        drop(gate);
        settle(&mut controller);
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Details(flathub.package.id.clone()),
            vec![Reply::Done(Ok(Payload::Details(flathub)))],
        ));
        controller.as_mut().poll();
        assert!(!more(&controller));
        assert_eq!(controller.rust().detail_cache.len(), 1);
    }
    #[test]
    fn finished_reviews_notices_and_activity_follow_the_job() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        let install = Operation::Install(row.id.clone());

        // Only reviews are dropped after their confirmation was cancelled.
        controller.as_mut().rust_mut().discard_revalidation = true;
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Load("Installed".into(), String::new()),
            vec![Reply::Done(Ok(Payload::Packages(PackageReport {
                packages: vec![row.clone()],
                failures: vec![],
                successful_sources: vec!["fixture".into()],
            })))],
        ));
        controller.as_mut().poll();
        assert!(controller.rows().to_string().contains("synthetic"));
        assert!(!controller.rust().discard_revalidation);
        controller.as_mut().rust_mut().discard_revalidation = true;
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::PlanOperation(install.clone()),
            vec![Reply::Done(Ok(Payload::OperationPreview(
                install.clone(),
                None,
            )))],
        ));
        controller.as_mut().poll();
        assert!(controller.rust().pending.is_none());
        assert!(!controller.rust().discard_revalidation);

        // A review that cannot be prepared says so.
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::PlanOperation(install.clone()),
            vec![Reply::Done(Err(ExecutionError::AuthorizationDenied.into()))],
        ));
        controller.as_mut().poll();
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["kind"], "error");
        assert_eq!(
            notice["title"],
            "Install synthetic (fixture) couldn't be prepared"
        );

        // A cancelled write records a cancelled outcome.
        let temp = pkgdeck_tools::Temp::new();
        let store = ActivityStore::at(temp.0.join("activity.json"));
        let upgrade = Operation::Upgrade(row.id.clone());
        let id = store
            .begin("gui", vec![upgrade.clone()], State::Running)
            .unwrap();
        controller.as_mut().rust_mut().activity_store = Some(store.clone());
        controller.as_mut().rust_mut().active_activity_id = Some(id);
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Write(upgrade.clone(), None),
            vec![Reply::Done(Err(EngineError::Cancelled))],
        ));
        controller.as_mut().poll();
        let entry = store.entries().unwrap().pop().unwrap();
        assert_eq!(entry.outcomes, [Outcome::Cancelled]);
        let finished = store
            .begin("gui", vec![upgrade.clone()], State::Running)
            .unwrap();
        controller.as_mut().rust_mut().active_activity_id = Some(finished);
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Write(upgrade.clone(), None),
            vec![Reply::Done(Ok(Payload::Written(
                OperationOutcome::default(),
            )))],
        ));
        controller.as_mut().poll();
        let entry = store.entries().unwrap().pop().unwrap();
        assert_eq!(
            (entry.id, entry.outcomes),
            (finished, vec![Outcome::Finished])
        );
        assert!(controller.rust().active_activity_id.is_none());

        // Without a history the finished write only forgets its entry.
        controller.as_mut().rust_mut().activity_store = None;
        controller.as_mut().rust_mut().active_activity_id = Some(finished + 1);
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Write(upgrade, None),
            vec![Reply::Done(Ok(Payload::Written(
                OperationOutcome::default(),
            )))],
        ));
        controller.as_mut().poll();
        assert!(controller.rust().active_activity_id.is_none());
        assert_eq!(store.entries().unwrap().len(), 2);
    }
    #[test]
    fn retried_update_sources_plan_the_update_they_were_retried_for() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().active_view = "Updates".into();
        controller.as_mut().rust_mut().updates_view = true;
        let retried = |packages: Vec<Package>| {
            fake_worker(
                Job::RetryFailedUpdates(vec!["fixture".into()]),
                vec![Reply::Done(Ok(Payload::RetryFailedUpdates(
                    vec!["fixture".into()],
                    PackageReport {
                        packages,
                        failures: vec![],
                        successful_sources: vec!["fixture".into()],
                    },
                )))],
            )
        };
        controller.as_mut().rust_mut().worker = Some(retried(vec![]));
        controller.as_mut().poll();
        assert_eq!(
            controller.status().to_string(),
            "No available updates after retrying source checks."
        );
        assert!(controller.rust().worker.is_none());

        controller.as_mut().rust_mut().worker = Some(retried(vec![fixture_row()]));
        controller.as_mut().poll();
        let upgrade = Operation::UpgradeAll {
            backend: "fixture".into(),
        };
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanUpgrade(operations, 1) if operations == std::slice::from_ref(&upgrade))
        ));
        settle(&mut controller);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::UpgradeAll(operations, None)) if operations == &[upgrade]
        ));
    }
    #[test]
    fn finished_workers_start_confirmed_reviewed_and_deferred_jobs() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        let upgrade = Operation::Upgrade(row.id.clone());
        let done = || {
            fake_worker(
                Job::Load("Search".into(), "synthetic".into()),
                vec![Reply::Done(Err(EngineError::Cancelled))],
            )
        };
        let confirmed = || Confirmed {
            job: Job::Write(upgrade.clone(), None),
            activity_id: None,
            cleanup_preview: vec![],
        };

        controller.as_mut().rust_mut().validated_confirmed = Some(confirmed());
        controller.as_mut().rust_mut().worker = Some(done());
        controller.as_mut().poll();
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::Write(operation, None) if *operation == upgrade)
        ));
        assert!(controller.writing());
        settle(&mut controller);
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["kind"], "success");

        controller
            .as_mut()
            .rust_mut()
            .confirmed_queue
            .push_back(confirmed());
        controller.as_mut().rust_mut().worker = Some(done());
        controller.as_mut().poll();
        assert!(controller.rust().revalidating.is_some());
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanOperation(operation) if *operation == upgrade)
        ));
        settle(&mut controller);

        controller.as_mut().rust_mut().deferred_load =
            Some(Job::Load("Installed".into(), String::new()));
        controller.as_mut().rust_mut().worker = Some(done());
        controller.as_mut().poll();
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::Load(view, _) if view == "Installed")
        ));
        settle(&mut controller);
        assert!(controller.rows().to_string().contains("synthetic"));
    }

    fn cleanup_item(key: &str) -> CleanupItem {
        CleanupItem {
            id: CleanupId {
                backend: "fixture".into(),
                key: key.into(),
            },
            kind: CleanupKind::OrphanDependencies,
            title: format!("Remove {key}"),
            summary: "Synthetic cleanup".into(),
            preview: format!("would remove {key}"),
        }
    }
    #[test]
    fn open_reference_scope_changes_only_a_pending_flatpak_reference_install() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_open_flatpak_scope(false);
        assert!(controller.rust().pending.is_none());
        let mut native = fixture_row();
        native.id.backend = "apt".into();
        controller.as_mut().rust_mut().pending =
            Some(Job::Write(Operation::Install(native.id.clone()), None));
        controller.as_mut().set_open_flatpak_scope(false);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Install(id), None)) if *id == native.id
        ));
        let mut reference = native.clone();
        reference.id.backend = "flatpak".into();
        reference.id.reference = Some("flatpakref:https://example.invalid/app.flatpakref".into());
        controller.as_mut().rust_mut().pending =
            Some(Job::Write(Operation::Install(reference.id.clone()), None));
        // The same scope, or a row that is no longer listed, keeps the plan.
        controller.as_mut().set_open_flatpak_scope(true);
        controller.as_mut().set_open_flatpak_scope(false);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Install(id), None)) if id.scope == Scope::System
        ));
    }
    #[test]
    fn confirmed_changes_queue_behind_running_work_and_can_be_cancelled() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let items = vec![cleanup_item("orphans"), cleanup_item("cache")];
        controller.as_mut().rust_mut().cleanup = items.clone();
        let upgrade = Operation::Upgrade(fixture_row().id);
        // A running write is never cancelled for a queued change.
        controller.as_mut().rust_mut().worker =
            Some(fake_worker(Job::Write(upgrade.clone(), None), vec![]));
        controller
            .as_mut()
            .accept_confirmed(Job::CleanAll(vec![Operation::Clean(items[1].id.clone())]));
        assert!(!controller
            .rust()
            .worker
            .as_ref()
            .unwrap()
            .cancel
            .requested());
        assert_eq!(
            controller.rust().confirmed_queue[0].cleanup_preview,
            [items[1].clone()]
        );
        assert_eq!(controller.status().to_string(), "Operation queued.");
        controller.as_mut().rust_mut().worker = None;
        // Changes wait behind earlier queued ones even with nothing running.
        controller
            .as_mut()
            .accept_confirmed(Job::CleanAll(vec![Operation::Clean(items[0].id.clone())]));
        assert_eq!(controller.rust().confirmed_queue.len(), 2);
        assert!(controller.rust().worker.is_none());
        controller.as_mut().rust_mut().validated_confirmed = Some(Confirmed {
            job: Job::Write(upgrade.clone(), None),
            activity_id: None,
            cleanup_preview: vec![],
        });
        controller.as_mut().cancel_queued();
        assert!(controller.rust().confirmed_queue.is_empty());
        assert!(controller.rust().validated_confirmed.is_none());

        // A running write keeps a new request waiting, not queued.
        controller.as_mut().rust_mut().worker =
            Some(fake_worker(Job::Write(upgrade, None), vec![]));
        controller
            .as_mut()
            .start(Job::Load("Installed".into(), String::new()));
        assert!(controller.rust().queued.is_none());
        controller.as_mut().rust_mut().worker = None;

        // Source changes run without a separate review. The bare host has
        // no editor, so nothing opens.
        let action = RepositoryAction {
            backend: "apt".into(),
            name: "sources".into(),
            scope: Scope::System,
            change: repositories::Change::OpenEditor,
        };
        controller.as_mut().validate_confirmed(Confirmed {
            job: Job::Repositories(Some(action)),
            activity_id: None,
            cleanup_preview: vec![],
        });
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(worker.job, Job::Repositories(Some(_)))
        ));
        settle(&mut controller);
        assert!(controller.status().to_string().contains("unavailable"));
    }
    #[test]
    fn reads_report_unsupported_sources_and_ignore_invalid_requests() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_package_report_state(
            &PackageReport {
                packages: vec![],
                failures: vec![BackendFailure {
                    backend: "fixture".into(),
                    error: EngineError::Unsupported {
                        backend: "fixture".into(),
                        capability: Capability::Installed,
                    },
                }],
                successful_sources: vec![],
            },
            false,
        );
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["phase"], "unsupported");
        controller.as_mut().set_package_report_state(
            &PackageReport {
                packages: vec![],
                failures: vec![BackendFailure {
                    backend: "fixture".into(),
                    error: EngineError::Cancelled,
                }],
                successful_sources: vec![],
            },
            false,
        );
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["phase"], "failed");

        let status = controller.status().to_string();
        controller.as_mut().load(
            "Bogus".into(),
            String::new().as_str().into(),
            String::new().as_str().into(),
            false,
            false,
        );
        assert!(controller.rust().worker.is_none());
        assert_eq!(controller.status().to_string(), status);

        // Nothing starts beside a foreground read.
        controller.as_mut().rust_mut().worker = Some(fake_worker(
            Job::Load("Installed".into(), String::new()),
            vec![],
        ));
        controller
            .as_mut()
            .retry_source("Installed".into(), "".into(), "flatpak".into());
        let path = std::env::temp_dir().join("pkgdeck-never-written.json");
        let url = QUrl::from_local_file(&path.to_string_lossy().as_ref().into());
        controller
            .as_mut()
            .export_inventory(url.clone(), "[]".into());
        controller.as_mut().preview_inventory(url);
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(worker.job, Job::Load(..))
        ));
        assert!(controller.rust().queued.is_none());
        controller.as_mut().rust_mut().worker = None;

        controller
            .as_mut()
            .retry_source("Search".into(), "synthetic".into(), "flatpak".into());
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::RetrySource(view, query, source)
                if view == "Search" && query == "synthetic" && source == "flatpak")
        ));
        settle(&mut controller);
        assert!(controller.rows().to_string().contains("synthetic"));
    }
    #[test]
    fn reviewed_plans_travel_to_the_write_engine() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        controller.as_mut().rust_mut().cleanup = vec![cleanup_item("orphans")];
        let upgrade_all = Operation::UpgradeAll {
            backend: "fixture".into(),
        };
        let apt_plan = AptUpgradePlan {
            preview: "Inst synthetic".into(),
            upgrades: vec!["synthetic".into()],
            installs: vec![],
            removals: vec![],
        };
        controller
            .as_mut()
            .start(Job::UpgradeAll(vec![upgrade_all], Some(apt_plan)));
        settle(&mut controller);
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        assert_eq!(notice["kind"], "success");
        let upgrade = Operation::Upgrade(row.id.clone());
        let plan = TransactionPlan {
            operation: upgrade.clone(),
            native_preview: "Upgrade synthetic".into(),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: None,
        };
        controller
            .as_mut()
            .start(Job::Write(upgrade, Some(Box::new(plan))));
        settle(&mut controller);
        let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
        // The fixture now plans nothing, so the reviewed plan is stale.
        assert!(notice["detail"]
            .as_str()
            .unwrap()
            .contains("The planned changes are different now."));
    }
    #[test]
    fn automatic_runs_use_their_batch_mode_and_report_what_they_did() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let operations = vec![Operation::UpgradeAll {
            backend: "fixture".into(),
        }];
        controller
            .as_mut()
            .start(Job::AutoUpgrade(operations.clone(), false));
        settle(&mut controller);
        let result: Value =
            serde_json::from_str(&controller.auto_update_result().to_string()).unwrap();
        assert_eq!(result, json!({"updated": 1, "failed": 0, "total": 1}));
        // A run that never reached its batch leaves the last result alone.
        controller.as_mut().set_auto_update_result("{}".into());
        controller.as_mut().rust_mut().natives = Natives {
            engine: missing_engine,
            ..SYNTHETIC
        };
        controller
            .as_mut()
            .start(Job::AutoUpgrade(operations, false));
        settle(&mut controller);
        assert_eq!(controller.auto_update_result().to_string(), "{}");
    }
    #[test]
    fn a_finished_refresh_counts_as_a_successful_check() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let refresh = Operation::Refresh {
            backend: "fixture".into(),
        };
        controller.as_mut().start(Job::Write(refresh.clone(), None));
        settle(&mut controller);
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert!(
            state["last_success"]["fixture"].as_u64().unwrap() > 0,
            "{state}"
        );
        // A batch records only the refreshes that finished.
        controller.as_mut().rust_mut().last_success.clear();
        controller
            .as_mut()
            .start(Job::UpgradeAll(vec![refresh], None));
        settle(&mut controller);
        assert!(controller.rust().last_success.contains_key("fixture"));
    }
    #[test]
    fn a_change_that_replaces_pkgdeck_asks_for_a_restart() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-self-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("pkgdeck");
        let operations = vec![Operation::UpgradeAll {
            backend: "fixture".into(),
        }];
        for (job, expected) in [
            (Job::UpgradeAll(operations.clone(), None), "manual"),
            (Job::AutoUpgrade(operations.clone(), false), "automatic"),
        ] {
            std::fs::write(&program, "old").unwrap();
            let mut controller = synthetic_controller();
            let mut controller = controller.pin_mut();
            let install = pkgdeck_core::relaunch::Install::program(&program);
            controller.as_mut().rust_mut().install = Some(install.clone());
            // Nothing replaced yet.
            controller.as_mut().start(job.clone());
            settle(&mut controller);
            assert_eq!(controller.self_update().to_string(), "");
            std::fs::write(dir.join("new"), "new").unwrap();
            std::fs::rename(dir.join("new"), &program).unwrap();
            controller.as_mut().start(job);
            settle(&mut controller);
            assert_eq!(controller.self_update().to_string(), expected);
            // The updated copy starts once this test process exits.
            assert!(controller.as_mut().restart_app(true));
        }
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        assert!(!controller.as_mut().restart_app(false));
        // A Flatpak restarts through flatpak-spawn, missing outside one.
        controller.as_mut().rust_mut().install = pkgdeck_core::relaunch::Install::detect(
            &Default::default(),
            Some("[Instance]\napp-path=/var/lib/flatpak/app/io.github.astrovm.PkgDeck/x86_64/stable/abc/files\n"),
            std::path::Path::new("/app/bin/pkgdeck"),
        );
        assert!(!controller.as_mut().restart_app(false));
        assert!(controller
            .status()
            .to_string()
            .starts_with("PkgDeck couldn't restart:"));
    }
    #[test]
    fn system_update_approval_is_saved_removed_and_explained() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().natives = Natives {
            approve: synthetic_approval,
            ..SYNTHETIC
        };
        let approval_idle =
            |controller: &ffi::PackageController| controller.rust().approval_worker.is_none();
        controller.as_mut().allow_system_updates(true);
        assert!(controller.needs_poll());
        wait_until(&mut controller, approval_idle);
        assert_eq!(controller.system_approval().to_string(), "synthetic-helper");
        assert_eq!(controller.approval_error().to_string(), "");
        controller.as_mut().allow_system_updates(false);
        wait_until(&mut controller, approval_idle);
        assert_eq!(controller.system_approval().to_string(), "");

        // Without the helper, the reason is shown and nothing is saved.
        controller.as_mut().rust_mut().natives = SYNTHETIC;
        controller.as_mut().allow_system_updates(true);
        wait_until(&mut controller, approval_idle);
        assert_eq!(controller.system_approval().to_string(), "");
        assert_eq!(
            controller.approval_error().to_string(),
            "PkgDeck's system helper is not installed"
        );

        // While the password prompt is open, another toggle waits and the
        // answer is read once it arrives.
        let (sender, receiver) = mpsc::channel();
        controller.as_mut().rust_mut().approval_worker = Some(ApprovalWorker {
            handle: thread::spawn(|| {}),
            receiver,
        });
        controller.as_mut().allow_system_updates(false);
        controller.as_mut().poll();
        assert!(controller.rust().approval_worker.is_some());
        assert_eq!(
            controller.approval_error().to_string(),
            "PkgDeck's system helper is not installed"
        );
        sender.send(Ok("later-helper".into())).unwrap();
        controller.as_mut().poll();
        assert!(controller.rust().approval_worker.is_none());
        assert_eq!(controller.system_approval().to_string(), "later-helper");
    }
    #[test]
    fn approval_failures_read_as_one_sentence() {
        let failed = |stderr: &str| {
            ExecutionError::Failed(pkgdeck_core::process::Completion {
                code: Some(1),
                signal: None,
                stdout: vec![],
                stderr: stderr.as_bytes().to_vec(),
                truncated: false,
                cancellation_deferred: false,
            })
        };
        for (error, sentence) in [
            (
                ExecutionError::AuthorizationCancelled,
                "The password prompt was cancelled.",
            ),
            (
                ExecutionError::AuthorizationDenied,
                "Your password wasn't accepted, so nothing changed.",
            ),
            (ExecutionError::Disabled("Not installed".into()), "Not installed"),
            (ExecutionError::Invalid("Unexpected user".into()), "Unexpected user"),
            (
                failed("polkit noise\npkgdeck-host-runner: this system's polkit has no rules folder\n\n"),
                "this system's polkit has no rules folder",
            ),
            (
                failed(" \n"),
                "PkgDeck's helper could not save the setting.",
            ),
            (ExecutionError::TimedOut, "host read timed out"),
        ] {
            assert_eq!(approval_error(&error), sentence);
        }
    }
    #[test]
    fn a_saved_approval_counts_only_for_the_helper_it_names() {
        let mut controller = idle_controller();
        let mut controller = controller.pin_mut();
        assert!(!controller.rust().approval_current());
        controller
            .as_mut()
            .restore_system_approval("some-other-helper".into());
        let current = controller.rust().approval_current();
        // The test binary has no root-owned helper beside it on Linux.
        #[cfg(not(target_os = "macos"))]
        assert!(!current);
        #[cfg(target_os = "macos")]
        let _ = current;
        // Nothing that may update unattended: no run is queued.
        controller.as_mut().set_auto_update(true);
        let mut firmware = synthetic_package("firmware", "Firmware");
        firmware.id.backend = "fwupd".into();
        firmware.update = UpdateAvailability::Available;
        firmware.candidate_version = Some("2".into());
        let report = PackageReport {
            packages: vec![firmware],
            failures: vec![],
            successful_sources: vec!["fwupd".into()],
        };
        assert!(!controller.as_mut().schedule_auto_update(&report));
        assert!(controller.rust().validated_confirmed.is_none());
    }
    #[test]
    fn automatic_runs_without_apt_skip_its_plan_and_hold_it_when_planning_fails() {
        let all = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        let mut engine = Engine::default();
        engine
            .register(Scripted {
                id: "homebrew",
                progress: vec![],
            })
            .unwrap();
        let cancel = Cancellation::default();
        let replies = run_job(
            &mut engine,
            Job::AutoUpgrade(vec![all("homebrew")], false),
            &cancel,
        );
        let outcomes = expect!(
            replies.into_iter().last(),
            Some(Reply::Done(Ok(Payload::Batch(_, outcomes)))) => outcomes
        );
        assert_eq!(outcomes, [Outcome::Finished]);
        // APT isn't there to plan, so it is held with the reason and the
        // rest still runs.
        let replies = run_job(
            &mut engine,
            Job::AutoUpgrade(vec![all("apt"), all("homebrew")], true),
            &cancel,
        );
        let outcomes = expect!(
            replies.into_iter().last(),
            Some(Reply::Done(Ok(Payload::Batch(_, outcomes)))) => outcomes
        );
        assert_eq!(outcomes, [Outcome::Failed, Outcome::Finished]);
        // A cleanup batch names its tasks as such.
        let replies = run_job(
            &mut engine,
            Job::CleanAll(vec![Operation::Clean(CleanupId {
                backend: "fwupd".into(),
                key: "cache".into(),
            })]),
            &cancel,
        );
        let status = expect!(
            replies.into_iter().last(),
            Some(Reply::Done(Ok(Payload::Batch(status, _)))) => status
        );
        assert!(
            status.starts_with("Completed 0 of 1 cleanup tasks."),
            "{status}"
        );
    }
    #[test]
    fn details_workers_replace_older_lookups_and_report_failures() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let first = fixture_row();
        let mut second = fixture_row();
        second.id.name = "second".into();
        controller.as_mut().rust_mut().packages = vec![first.clone(), second.clone()];
        controller.as_mut().select(0);
        let older = controller
            .rust()
            .details_worker
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        controller.as_mut().select(1);
        assert!(older.requested());
        // Choosing the package that is loading does not start it again.
        let current = controller
            .rust()
            .details_worker
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        controller.as_mut().select(1);
        assert!(!current.requested());
        settle(&mut controller);
        let (worker, gate) = held_worker(Job::Details(first.id.clone()), vec![]);
        controller.as_mut().rust_mut().worker = Some(worker);
        controller.as_mut().select(0);
        assert!(!controller
            .rust()
            .worker
            .as_ref()
            .unwrap()
            .cancel
            .requested());
        assert!(controller.rust().queued.is_none());
        assert!(controller.rust().details_worker.is_none());
        drop(gate);
        settle(&mut controller);
        // The fixture has no details; the row preview stays with the reason.
        let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
        assert_eq!(details["package"]["name"], "second");
        let status = controller.status().to_string();
        assert_eq!(status, "No package matches the selection.");

        controller.as_mut().rust_mut().natives.engine = missing_engine;
        controller.as_mut().select(0);
        settle(&mut controller);
        assert!(controller
            .status()
            .to_string()
            .contains("synthetic system has no managers"));

        // Replies other than details are ignored.
        let (sender, receiver) = mpsc::channel();
        sender.send(Reply::Engine(Box::default())).unwrap();
        controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
            handle: thread::spawn(move || drop(sender)),
            receiver,
            cancel: Cancellation::default(),
            id: first.id.clone(),
        });
        let before = controller.details().to_string();
        settle(&mut controller);
        assert_eq!(controller.details().to_string(), before);
    }
    #[test]
    fn failed_preloads_leave_their_section_unloaded() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().natives.engine = missing_engine;
        let key = cache_key("Installed", "", &[], false);
        controller
            .as_mut()
            .start_prefetch("Installed".into(), key.clone());
        wait_until(&mut controller, |controller| {
            controller.rust().prefetch_worker.is_none()
        });
        assert!(!controller.rust().prefetched.contains_key(&key));
    }

    #[test]
    fn proposals_review_native_plans_and_retry_failed_sources_first() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        let mut apt = fixture_row();
        apt.id.backend = "apt".into();
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        // Updating everything is offered only in the Updates section.
        controller.as_mut().propose("upgrade-all".into(), -1);
        assert!(controller.rust().worker.is_none());
        assert!(controller.rust().pending.is_none());

        controller.as_mut().rust_mut().updates_view = true;
        controller.as_mut().rust_mut().failures = vec![BackendFailure {
            backend: "fixture".into(),
            error: EngineError::Cancelled,
        }];
        controller.as_mut().propose("upgrade-all".into(), -1);
        assert_eq!(
            controller.status().to_string(),
            "Retrying failed source checks before updating."
        );
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::RetryFailedUpdates(sources) if sources == &["fixture"])
        ));
        settle(&mut controller);

        // APT updates and APT changes to one package get a native plan first.
        controller.as_mut().rust_mut().failures.clear();
        controller.as_mut().rust_mut().packages = vec![apt.clone()];
        controller.as_mut().propose("upgrade-all".into(), -1);
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanUpgrade(operations, 1)
                if operations == &[Operation::UpgradeAll { backend: "apt".into() }])
        ));
        settle(&mut controller);
        controller.as_mut().rust_mut().packages = vec![apt.clone()];
        controller.as_mut().propose("remove".into(), 0);
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanOperation(Operation::Remove(id)) if *id == apt.id)
        ));
        settle(&mut controller);

        // Other managers confirm without a native plan.
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        controller.as_mut().propose("remove".into(), 0);
        assert!(controller.rust().worker.is_none());
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Remove(id), None)) if *id == row.id
        ));

        let mut second = row.clone();
        second.id.name = "second".into();
        controller.as_mut().rust_mut().packages = vec![row, second];
        controller.as_mut().propose_checked(
            r#"[["fixture","synthetic","all",null,"system",null],["fixture","second","all",null,"system",null]]"#.into(),
        );
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(
            data["summary"],
            "Update 2 selected packages\nIf one update fails, the others can still finish."
        );

        // The Software Sources editor opens without a confirmation.
        controller.as_mut().rust_mut().pending = None;
        controller.as_mut().change_repository(
            r#"{"backend":"apt","name":"sources","scope":"system","action":"open_editor"}"#.into(),
        );
        assert!(controller.rust().pending.is_none());
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::Repositories(Some(action))
                if matches!(action.change, repositories::Change::OpenEditor))
        ));
        settle(&mut controller);
        assert!(controller.status().to_string().contains("unavailable"));
    }
    #[test]
    fn applied_results_change_only_what_they_describe() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let row = fixture_row();
        controller
            .as_mut()
            .apply(Ok(Payload::BackgroundUpdates(PackageReport {
                packages: vec![row.clone()],
                failures: vec![],
                successful_sources: vec!["fixture".into()],
            })));
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        assert_eq!(state["available"], 1);

        // An opened APT package is reviewed with its native plan.
        let mut apt = fixture_row();
        apt.id.backend = "apt".into();
        apt.installed_version = None;
        controller.as_mut().apply(Ok(opened_payload(apt.clone())));
        controller.as_mut().install_opened();
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::PlanOperation(Operation::Install(id)) if *id == apt.id)
        ));
        settle(&mut controller);

        // A queued update whose plan is unchanged may run.
        let operations = vec![Operation::UpgradeAll {
            backend: "fixture".into(),
        }];
        let queued = |job| Confirmed {
            job,
            activity_id: None,
            cleanup_preview: vec![],
        };
        controller.as_mut().rust_mut().revalidating =
            Some(queued(Job::UpgradeAll(operations.clone(), None)));
        controller
            .as_mut()
            .apply(Ok(Payload::UpgradePreview(operations.clone(), 1, None)));
        assert!(controller.rust().validated_confirmed.is_some());
        assert!(controller.rust().pending.is_none());
        controller.as_mut().rust_mut().validated_confirmed = None;

        // A changed one asks again, naming what could not be checked.
        controller.as_mut().rust_mut().revalidating = Some(queued(Job::UpgradeAll(vec![], None)));
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        controller.as_mut().rust_mut().failures = ["apt", "dnf"]
            .map(|backend| BackendFailure {
                backend: backend.into(),
                error: EngineError::Cancelled,
            })
            .into();
        let plan = AptUpgradePlan {
            preview: String::new(),
            upgrades: vec!["synthetic".into()],
            installs: vec![],
            removals: vec![],
        };
        controller.as_mut().apply(Ok(Payload::UpgradePreview(
            operations.clone(),
            1,
            Some(plan.clone()),
        )));
        assert!(controller.rust().revalidating.is_none());
        assert!(controller.rust().validated_confirmed.is_none());
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::UpgradeAll(pending, Some(reviewed))) if *pending == operations && *reviewed == plan
        ));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(
            data["summary"],
            "Update 1 package\n2 sources could not be checked. Updates from them are not included."
        );
        controller.as_mut().rust_mut().failures.clear();
        controller
            .as_mut()
            .apply(Ok(Payload::UpgradePreview(operations.clone(), 2, None)));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(
            data["summary"],
            "Update 2 packages\nIf one update fails, the others can still finish."
        );

        // A queued cleanup whose tasks changed is confirmed again.
        let items = vec![cleanup_item("orphans"), cleanup_item("cache")];
        let cleanups: Vec<_> = items
            .iter()
            .map(|item| Operation::Clean(item.id.clone()))
            .collect();
        controller.as_mut().rust_mut().pending = None;
        controller.as_mut().rust_mut().revalidating = Some(queued(Job::CleanAll(cleanups.clone())));
        controller
            .as_mut()
            .apply(Ok(Payload::CleanPreview(cleanups.clone(), items.clone())));
        let data: Value =
            serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
        assert_eq!(data["action"], "Clean 2 tasks");
        assert!(
            matches!(&controller.rust().pending, Some(Job::CleanAll(pending)) if *pending == cleanups)
        );
        // Without a queued cleanup a preview only refreshes nothing.
        controller.as_mut().rust_mut().pending = None;
        controller
            .as_mut()
            .apply(Ok(Payload::CleanPreview(cleanups, items)));
        assert!(controller.rust().pending.is_none());

        // Every source failing is a failed read.
        controller.as_mut().apply(Ok(Payload::Sources(vec![Source {
            backend: "fixture".into(),
            capabilities: vec![],
            availability: Err(EngineError::Cancelled),
        }])));
        let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
        assert_eq!(state["phase"], "failed");

        // The details cache stays bounded.
        for index in 0..128 {
            let mut id = row.id.clone();
            id.name = format!("cached-{index}");
            controller
                .as_mut()
                .rust_mut()
                .detail_cache
                .insert(id, "{}".into());
        }
        controller
            .as_mut()
            .apply(Ok(Payload::Details(fixture_details("Fresh"))));
        assert_eq!(controller.rust().detail_cache.len(), 1);
        assert!(controller.rust().detail_cache.contains_key(&row.id));
    }

    fn described_engine(
        _: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut row = fixture_row();
        row.id.backend = "described".into();
        let mut engine = Engine::default();
        engine.register(Described(row))?;
        Ok(engine)
    }
    #[test]
    fn elevated_reads_and_opened_packages_use_their_own_workers() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        // An empty search only remembers the chosen sources and elevation.
        controller
            .as_mut()
            .load("Search".into(), "  ".into(), "apt".into(), true, false);
        assert!(controller.rust().worker.is_none());
        assert_eq!(controller.rust().source_filter, ["apt"]);
        assert!(controller.rust().sudo);

        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), true, true);
        assert!(matches!(
            &controller.rust().worker,
            Some(worker) if matches!(&worker.job, Job::Load(view, _) if view == "Installed")
        ));
        settle(&mut controller);
        assert!(controller.rows().to_string().contains("synthetic"));
        controller
            .as_mut()
            .start_prefetch("Updates".into(), cache_key("Updates", "", &[], true));
        wait_until(&mut controller, |controller| {
            controller.rust().prefetch_worker.is_none()
        });
        assert!(controller
            .rust()
            .prefetched
            .contains_key(&cache_key("Updates", "", &[], true)));

        // Details of an opened package arrive from the details worker.
        let mut row = fixture_row();
        row.id.backend = "described".into();
        controller.as_mut().rust_mut().natives.engine = described_engine;
        controller.as_mut().rust_mut().packages = vec![row];
        controller.as_mut().select(0);
        assert!(controller.rust().details_worker.is_some());
        settle(&mut controller);
        let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
        assert_eq!(details["description"], "Described by the warm engine");
    }
    #[test]
    fn rows_offer_installs_and_source_refreshes() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let mut row = fixture_row();
        row.installed_version = None;
        controller.as_mut().rust_mut().packages = vec![row.clone()];
        controller.as_mut().propose("install".into(), 0);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Install(id), None)) if *id == row.id
        ));
        controller.as_mut().rust_mut().sources = vec![Source {
            backend: "fixture".into(),
            capabilities: vec![Capability::Refresh],
            availability: Ok(Availability::Available),
        }];
        controller.as_mut().propose("refresh".into(), 0);
        assert!(matches!(
            &controller.rust().pending,
            Some(Job::Write(Operation::Refresh { backend }, None)) if backend == "fixture"
        ));
    }

    #[test]
    fn running_worker_events_publish_progress_only_while_tracking_a_write() {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let operation = Operation::Refresh {
            backend: "apt".into(),
        };
        let job = Job::Write(operation.clone(), None);
        controller.as_mut().rust_mut().progress_state =
            Some(ProgressState::new(&job, Some(42), &Names::new()));
        let (sender, receiver) = mpsc::channel();
        let (release, hold) = mpsc::channel::<()>();
        // Keep the worker alive until after polling: a finished worker takes
        // a different path that clears progress instead of publishing it.
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(move || {
                let _ = hold.recv();
            }),
            receiver,
            cancel: Cancellation::default(),
            job,
        });
        sender
            .send(Reply::ProgressEvent(Event::Started(operation.clone())))
            .unwrap();
        sender
            .send(Reply::ProgressEvent(Event::Progress {
                operation: operation.clone(),
                progress: Progress::Transfer {
                    completed: 50,
                    total: Some(100),
                },
            }))
            .unwrap();
        controller.as_mut().poll();
        let progress: Value = serde_json::from_str(&controller.progress().to_string()).unwrap();
        assert_eq!(progress["activity_id"], 42);
        assert_eq!(progress["label"], "Refresh APT package lists");
        assert_eq!(progress["transferred"], 50);
        assert_eq!(progress["transfer_total"], 100);

        // A late event without a tracked write leaves the visible snapshot.
        controller.as_mut().rust_mut().progress_state = None;
        sender
            .send(Reply::ProgressEvent(Event::Finished {
                operation,
                result: Ok(OperationOutcome::default()),
            }))
            .unwrap();
        controller.as_mut().poll();
        assert_eq!(
            serde_json::from_str::<Value>(&controller.progress().to_string()).unwrap(),
            progress
        );
        drop(release);
        drop(sender);
        settle(&mut controller);
        assert_eq!(controller.progress().to_string(), "{}");
    }
}
