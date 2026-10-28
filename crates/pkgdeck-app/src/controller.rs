//! The window's logic: native work runs on workers; properties change only
//! on the window's thread, when it polls.
use crate::qt::{QString, QUrl};
use pkgdeck_core::activity::{History, Outcome, State};
use pkgdeck_core::backends::AppImage;
use pkgdeck_core::background::{self, Schedule};
use pkgdeck_core::hold::{waits_for_builds, Hold, Waits};
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
mod wording;
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    pin::Pin,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wording::*;

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
        failure, set_failure, on_failure_changed: QString;
        failed_rows, set_failed_rows, on_failed_rows_changed: QString;
        progress, set_progress, on_progress_changed: QString;
        batch_finished, set_batch_finished, on_batch_finished_changed: QString;
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
    RefreshSources(Vec<String>),
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
    /// Cleanup that runs with nobody watching, once a day: what is found
    /// then, as long as [`CleanupItem::automatic`] allows it.
    AutoClean {
        caches: bool,
        worktrees: bool,
        preferences: CleanupPreferences,
    },
    CleanAll(Vec<Operation>),
    /// Cleanup tasks that waited for a build, tried again as they were
    /// reviewed.
    RetryCleans(Vec<CleanupItem>),
    /// Refresh the selected sources, then read their updates in the same worker.
    CheckUpdates(Vec<Operation>, Vec<String>),
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
                | Self::AutoClean { .. }
                | Self::CleanAll(_)
                | Self::CheckUpdates(..)
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
            | Self::CleanAll(operations)
            | Self::CheckUpdates(operations, _) => operations.clone(),
            Self::RetryCleans(items) => items
                .iter()
                .map(|item| Operation::Clean(item.id.clone()))
                .collect(),
            _ => vec![],
        }
    }
}
/// How each change in a batch ended, kept as data so the summary, the
/// notice, failed rows and Retry all read the same facts.
#[derive(Clone, Debug, Default, PartialEq)]
struct Batch {
    /// What the changes are, such as "updates" or "cleanup tasks".
    noun: &'static str,
    /// One per change, in the order they were asked for.
    steps: Vec<Ended>,
    /// What people must still do, such as restarting after firmware.
    note: Option<&'static str>,
}
/// How one change in a batch ended.
#[derive(Clone, Debug, PartialEq)]
struct Ended {
    /// The change as `operation_title` names it.
    title: String,
    outcome: Outcome,
    said: Said,
    /// Why it didn't finish, as Activity keeps it: what `said` says,
    /// without anything a tool printed.
    kept: Option<String>,
}
/// What a cleanup task waiting for a build says.
const WAITS_FOR_BUILD: &str = "Waits for a build that uses it to finish, then cleans on its own.";
/// How often cleanup tasks waiting for a build are tried again.
const RETRY_CLEANS_EVERY: u64 = 60;
/// A cleanup task waiting for a build: the plan it was reviewed as, and
/// which change of which Activity runs it is, to settle once it runs.
#[derive(Clone, Debug, PartialEq)]
struct WaitingClean {
    item: CleanupItem,
    activity: Vec<(u64, usize)>,
}
/// What a step has to say about how it ended.
#[derive(Clone, Debug, PartialEq)]
enum Said {
    Completed,
    /// It finished before the cancel reached it, so its changes stay.
    KeptAfterCancel,
    /// Administrator access was refused. The advice depends on the
    /// permission option, so it is worded only when shown.
    Denied,
    /// Why it failed or was skipped, already in plain words.
    Because(String),
}
impl Said {
    fn text(&self, sudo: bool) -> String {
        match self {
            Self::Completed => "Completed".into(),
            Self::KeptAfterCancel => {
                "Finished before it could be cancelled. Changes were kept.".into()
            }
            Self::Denied => plain_error(
                &EngineError::Execution(ExecutionError::AuthorizationDenied),
                None,
                sudo,
            ),
            Self::Because(reason) => reason.clone(),
        }
    }
}
impl Batch {
    fn outcomes(&self) -> impl Iterator<Item = Outcome> + '_ {
        self.steps.iter().map(|step| step.outcome)
    }
    /// "Title: why" for each step that waits for a build or was left alone.
    fn left_alone(&self, sudo: bool) -> impl Iterator<Item = String> + '_ {
        self.steps
            .iter()
            .filter(|step| matches!(step.outcome, Outcome::Waiting | Outcome::Skipped))
            .map(move |step| format!("{}: {}", step.title, step.said.text(sudo)))
    }
    /// Each step as Activity keeps it.
    fn ended(&self) -> Vec<(Outcome, Option<String>)> {
        self.steps
            .iter()
            .map(|step| (step.outcome, step.kept.clone()))
            .collect()
    }
    /// "Completed 1 of 2 updates.", then each step and the note.
    fn text(&self, sudo: bool) -> String {
        let done = self.outcomes().filter(|o| *o == Outcome::Finished).count();
        let mut text = format!("Completed {done} of {} {}.", self.steps.len(), self.noun);
        for step in &self.steps {
            text.push_str(&format!("\n{}: {}", step.title, step.said.text(sudo)));
        }
        if let Some(note) = self.note {
            text.push_str(&format!("\n{note}"));
        }
        text
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
    Batch(Batch),
    CheckedUpdates(Box<PackageReport>, Batch),
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
    /// What automatic cleaning did: tasks finished and failed, and about
    /// how much it freed. `tasks` is what it ran, in order, each as it ended,
    /// and `started_at` when, for Activity.
    AutoCleaned {
        cleaned: usize,
        failed: usize,
        bytes: u64,
        tasks: Vec<(CleanupItem, Outcome, Option<String>)>,
        started_at: u64,
    },
    /// How cleanup tasks that waited for a build ended when tried again.
    RetriedCleans(Batch),
}
enum Reply {
    Progress(String),
    ProgressEvent(Event),
    /// A line of output from an Update all step that names a package.
    Output(String),
    /// What the failed steps of a batch printed, for the failure's details.
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
    operations: Vec<Operation>,
    /// The operation that started last; before the first one starts, the
    /// job's only operation, if it has just one.
    current: Option<Operation>,
    /// The steps running now, in the order they started. Steps of
    /// different sources can run at once.
    running: Vec<Step>,
    /// Rows a whole-source step is expected to update, by source, from
    /// the Updates list (see `with_rows`).
    rows: BTreeMap<String, Vec<PackageId>>,
    /// Packages already done, successfully or not.
    finished: Vec<PackageId>,
    /// Packages whose change succeeded, in the order they finished.
    succeeded: Vec<PackageId>,
    names: Names,
}
/// A running step of a change.
struct Step {
    operation: Operation,
    label: String,
    /// The package a whole-source step reported it is working on now.
    item: Option<PackageId>,
    transferred: u64,
    transfer_total: Option<u64>,
}
impl Step {
    /// How far its transfer is, from 0 to 1, when it has one.
    fn fraction(&self) -> Option<f64> {
        self.transfer_total
            .map(|total| (self.transferred as f64 / total as f64).min(1.0))
    }
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
        let label = format!(
            "Update {}",
            self.names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| id.name.clone())
        );
        let step = self.step(&Operation::UpgradeAll {
            backend: backend.into(),
        });
        step.label.clone_from(&label);
        let previous = step.item.replace(id.clone());
        if let Some(previous) = previous.filter(|previous| *previous != id) {
            self.finish(previous, false);
        }
        self.label = label;
        true
    }
    /// The running step of `operation`, which started now if it wasn't
    /// running yet.
    fn step(&mut self, operation: &Operation) -> &mut Step {
        let index = match self
            .running
            .iter()
            .position(|step| step.operation == *operation)
        {
            Some(index) => index,
            None => {
                self.running.push(Step {
                    operation: operation.clone(),
                    label: operation_title_named(operation, &self.names),
                    item: None,
                    transferred: 0,
                    transfer_total: None,
                });
                self.running.len() - 1
            }
        };
        &mut self.running[index]
    }
    /// Follows a line of output from a running whole-source step. Steps
    /// running at once share their output, so each source checks the line.
    fn observe(&mut self, line: &str) -> bool {
        let sources: Vec<String> = self
            .running
            .iter()
            .filter_map(|step| match &step.operation {
                Operation::UpgradeAll { backend } => Some(backend.clone()),
                _ => None,
            })
            .collect();
        sources.iter().any(|backend| {
            pkgdeck_core::backends::output_package(backend, line)
                .is_some_and(|name| self.move_to(backend, &name))
        })
    }
    /// Marks a package done and counts it once. Success is confirmed only
    /// when the operation finishes, not when its output moves to another row.
    fn finish(&mut self, id: PackageId, ok: bool) {
        if ok && !self.succeeded.contains(&id) {
            self.succeeded.push(id.clone());
        }
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
            operations,
            current,
            running: vec![],
            rows: BTreeMap::new(),
            finished: vec![],
            succeeded: vec![],
            names: names.clone(),
        }
    }
    fn apply(&mut self, event: &Event) {
        match event {
            Event::Started(operation) => {
                self.running.retain(|step| step.operation != *operation);
                self.label = self.step(operation).label.clone();
                self.current = Some(operation.clone());
            }
            Event::Progress {
                operation: Operation::UpgradeAll { backend },
                progress: Progress::Package(name),
            } => {
                self.move_to(backend, name);
            }
            Event::Progress {
                operation,
                progress: Progress::Transfer { completed, total },
            } => {
                let step = self.step(operation);
                step.transferred = *completed;
                step.transfer_total = (*total).filter(|total| *total > 0);
            }
            Event::Finished { operation, result } => {
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
                    self.finish(id, result.is_ok());
                }
                self.running.retain(|step| step.operation != *operation);
                // What still runs, if anything, is what the card names.
                if let Some(last) = self.running.last() {
                    self.label.clone_from(&last.label);
                    self.current = Some(last.operation.clone());
                }
            }
            Event::Progress { .. } => {}
        }
    }
    /// How far the whole change is, from 0 to 1, when that is known: from
    /// finished steps of a batch plus the running step's transfer. A single
    /// step without a transfer size has no known fraction.
    fn fraction(&self) -> Option<f64> {
        if self.done >= self.total {
            Some(1.0)
        } else if self.total > 1 {
            let steps: f64 = self.running.iter().filter_map(Step::fraction).sum();
            Some(((self.done as f64 + steps) / self.total as f64).min(1.0))
        } else {
            self.running.last().and_then(Step::fraction)
        }
    }
    /// The step the card shows the transfer of: the only one running.
    fn transfer(&self) -> Option<&Step> {
        match self.running.as_slice() {
            [one] => Some(one),
            _ => None,
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
        let item = self
            .running
            .iter()
            .find(|step| Some(&step.operation) == current)
            .and_then(|step| step.item.as_ref());
        let running: Vec<_> = self
            .running
            .iter()
            .map(|step| {
                json!({
                    "current": step.item.as_ref().or_else(|| package_target(&step.operation)).map(target_row),
                    "source": step.operation.backend(),
                })
            })
            .collect();
        let label = match self.running.len() {
            0 | 1 => running_label(&self.label),
            count => format!(
                "{} and {} more",
                running_label(&self.label).trim_end_matches('…'),
                count - 1
            ),
        };
        let transfer = self.transfer();
        encoded(json!({
            "activity_id": self.activity_id,
            "label": label,
            "done": self.done,
            "total": self.total,
            "transferred": transfer.map_or(0, |step| step.transferred),
            "transfer_total": transfer.and_then(|step| step.transfer_total),
            "fraction": self.fraction(),
            "targets": targets,
            "sources": sources,
            "action": current.map_or("", operation_kind),
            "current": item.or_else(|| current.and_then(package_target)).map(target_row),
            "running": running,
            "finished": self.finished.iter().map(target_row).collect::<Vec<_>>(),
            "current_source": current.map(Operation::backend),
        }))
    }
    /// What the batch changed, for the Updates list: the sources it ran on
    /// and the rows that succeeded. Empty for a single change, which has no
    /// batch marks.
    fn batch_result(&self) -> QString {
        if self.total <= 1 {
            return encoded(json!({"sources": [], "finished": []}));
        }
        let mut sources: Vec<&str> = Vec::new();
        for operation in &self.operations {
            let backend = operation.backend();
            if !sources.contains(&backend) {
                sources.push(backend);
            }
        }
        let finished: Vec<Value> = self.succeeded.iter().map(target_row).collect();
        encoded(json!({"sources": sources, "finished": finished}))
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
/// An opened file or link, with the copy already installed, if any, so its
/// page can offer an update or a reinstall instead of an install.
fn inspect_open_input(input: &str, cancel: &Cancellation) -> Result<Payload, EngineError> {
    let mut payload = inspect_open_file(input, cancel)?;
    if let Payload::OpenPackage(details) = &mut payload {
        let package = &mut details.package;
        if package.id.backend != "appimage" && package.installed_version.is_none() {
            package.installed_version =
                pkgdeck_core::installed_copy::locate(&mut package.id, cancel);
        }
    }
    Ok(payload)
}
fn inspect_open_file(input: &str, cancel: &Cancellation) -> Result<Payload, EngineError> {
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
    let refuse = |reason: String| EngineError::Refused {
        backend: CASK.into(),
        refusal: Refusal::Explained(format!("{reason} Nothing was changed.")),
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
        _ => Err(EngineError::Refused {
            backend: "appimage".into(),
            refusal: Refusal::Explained(format!(
                "{} isn't installed some other way anymore. Nothing was changed.",
                app.id.name
            )),
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
                    .map_err(|error| {
                        EngineError::Execution(ExecutionError::Invalid(error.to_string()))
                    })
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
        Job::CheckUpdates(operations, _) => {
            // Index-only sources still need a fresh check. Sources with a
            // refresh operation fetch below, so don't fetch them twice.
            let refreshing: Vec<_> = operations
                .iter()
                .map(|op| op.backend().to_owned())
                .collect();
            let mut failures = engine.refresh_update_indexes(&refreshing, cancel);
            let ran = batch(engine, &operations, vec![], cancel, &mut |reply| {
                if let Reply::ProgressEvent(Event::Finished {
                    operation,
                    result: Err(error),
                }) = &reply
                {
                    failures.push(BackendFailure {
                        backend: operation.backend().into(),
                        error: error.clone(),
                    });
                }
                send(reply);
            });
            if cancel.requested() {
                Ok(Payload::Batch(ran))
            } else {
                send(Reply::Progress("Checking for updates…".into()));
                let mut report = engine.installed(cancel);
                if cancel.requested() {
                    Ok(Payload::Batch(ran))
                } else {
                    // A cached index can still be read after its refresh failed.
                    // Preserve that failure instead of claiming a fresh check.
                    report
                        .successful_sources
                        .retain(|source| !failures.iter().any(|f| &f.backend == source));
                    report
                        .failures
                        .retain(|failure| !failures.iter().any(|f| f.backend == failure.backend));
                    report.failures.extend(failures);
                    // A missing tool isn't a failed check. Broken installs and real errors stay.
                    report.failures.retain(|failure| {
                        !matches!(&failure.error, EngineError::Unavailable { reason, .. }
                            if pkgdeck_core::backends::is_missing_tool_reason(reason))
                    });
                    filter_updates(&mut report);
                    Ok(Payload::CheckedUpdates(Box::new(report), ran))
                }
            }
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
        Job::RetryFailedUpdates(sources) | Job::RefreshSources(sources) => {
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
        Job::PlanOperation(operation) => engine
            .plan_operation(&operation, cancel)
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
        Job::AutoClean {
            caches,
            worktrees,
            preferences,
        } => {
            let items: Vec<_> = engine
                .cleanup(cancel)
                .items
                .into_iter()
                .filter(|item| {
                    preferences.includes_item(item)
                        && ((caches && item.automatic())
                            || (worktrees
                                && item.kind == CleanupKind::Worktree
                                && item.id.backend == "projects"))
                })
                .collect();
            let operations: Vec<_> = items
                .iter()
                .map(|item| Operation::Clean(item.id.clone()))
                .collect();
            let started_at = epoch_seconds();
            let ended = match operations.is_empty() {
                true => vec![],
                false => batch(engine, &operations, Vec::new(), cancel, send).ended(),
            };
            let tasks: Vec<_> = items
                .into_iter()
                .zip(ended)
                .map(|(item, (outcome, kept))| (item, outcome, kept))
                .collect();
            let finished = |(_, outcome, _): &&(CleanupItem, Outcome, _)| {
                *outcome == Outcome::Finished
            };
            Ok(Payload::AutoCleaned {
                cleaned: tasks.iter().filter(finished).count(),
                failed: tasks
                    .iter()
                    .filter(|(_, outcome, _)| *outcome == Outcome::Failed)
                    .count(),
                bytes: tasks
                    .iter()
                    .filter(finished)
                    .filter_map(|(item, ..)| item.bytes)
                    .sum(),
                tasks,
                started_at,
            })
        }
        Job::RetryCleans(items) => {
            // Each runs as it was reviewed, however long ago that was.
            for item in &items {
                engine.remember_cleanup_plan(item.clone());
            }
            let operations = Job::RetryCleans(items).operations();
            Ok(Payload::RetriedCleans(batch(
                engine,
                &operations,
                Vec::new(),
                cancel,
                send,
            )))
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
                            Some(total) => format!("Transferring {completed} of {total}…"),
                            None => format!("Transferring {completed}…"),
                        },
                        Progress::Package(name) => format!("Updating {name}…"),
                    }));
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
            })
            .map(|outcome| match firmware_note(std::slice::from_ref(&op)) {
                Some(note) => Payload::Batch(Batch {
                    noun: "updates",
                    steps: vec![Ended {
                        title: operation_title(&op),
                        outcome: Outcome::Finished,
                        said: if outcome.cancellation_deferred {
                            Said::KeptAfterCancel
                        } else {
                            Said::Completed
                        },
                        kept: None,
                    }],
                    note: Some(note),
                }),
                None => Payload::Written(outcome),
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
    Payload::Batch(batch(engine, operations, held, cancel, send))
}
/// Run `operations` in order, skipping `held` ones with their reason.
fn batch(
    engine: &mut Engine,
    operations: &[Operation],
    held: Vec<(Operation, String)>,
    cancel: &Cancellation,
    send: &mut dyn FnMut(Reply),
) -> Batch {
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
    let noun = if operations
        .iter()
        .all(|op| matches!(op, Operation::Clean(_)))
    {
        "cleanup tasks"
    } else if operations
        .iter()
        .all(|op| matches!(op, Operation::Refresh { .. }))
    {
        "source refreshes"
    } else {
        "updates"
    };
    let mut steps = Vec::new();
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
        let (outcome, kept) = match &result {
            Ok(_) => (Outcome::Finished, None),
            Err((_, Some(reason))) => (Outcome::Failed, Some(reason.clone())),
            Err((error, None)) => ended_by(error, operation.backend()),
        };
        let said = match result {
            Ok(outcome) if outcome.cancellation_deferred => Said::KeptAfterCancel,
            Ok(_) => Said::Completed,
            Err((_, Some(reason))) => Said::Because(reason),
            Err((EngineError::Refused { refusal: Refusal::Building, .. }, None)) => {
                Said::Because(WAITS_FOR_BUILD.into())
            }
            // The App Store explains a denial its own way, whatever the
            // permission option is.
            Err((error, None)) if is_denied(&error) && operation.backend() != "mas" => Said::Denied,
            Err((error, None)) => {
                Said::Because(plain_error(&error, Some(operation.backend()), false))
            }
        };
        steps.push(Ended {
            title: operation_title(operation),
            outcome,
            said,
            kept,
        });
    }
    if !failure_output.is_empty() {
        send(Reply::FailureOutput(failure_output.join("\n\n")));
    }
    Batch {
        noun,
        steps,
        note: firmware_note(operations),
    }
}
/// How a change `error` stopped ended, as Activity keeps it. A cleanup task
/// waiting for a build, or left alone because it changed since it was
/// reviewed, didn't fail.
fn ended_by(error: &EngineError, backend: &str) -> (Outcome, Option<String>) {
    match error {
        EngineError::Cancelled => (Outcome::Cancelled, None),
        EngineError::Refused {
            refusal: Refusal::Building,
            ..
        } => (Outcome::Waiting, Some(WAITS_FOR_BUILD.into())),
        EngineError::Refused {
            refusal: refusal @ Refusal::CleanupChanged,
            ..
        } => (Outcome::Skipped, Some(refusal.to_string())),
        error => (Outcome::Failed, Some(kept_reason(error, backend))),
    }
}
/// A firmware update may need a restart before it takes effect.
fn firmware_note(operations: &[Operation]) -> Option<&'static str> {
    operations
        .iter()
        .any(|op| matches!(op, Operation::Upgrade(id) if id.backend == "fwupd"))
        .then_some("Firmware: Restart or shut down the device if the update asked for it.")
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
    /// Why automatic updates should wait right now, if they should.
    hold: fn(&pkgdeck_core::host::Host, Waits) -> Option<Hold>,
}
/// Never on battery, metered or building.
fn never_hold(_: &pkgdeck_core::host::Host, _: Waits) -> Option<Hold> {
    None
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
        hold: never_hold,
    }
}
const NATIVES: Natives = Natives {
    engine: pkgdeck_core::backends::native_engine,
    host: pkgdeck_core::host::Host::current,
    approve: pkgdeck_core::unattended::set_approval,
    root: "/",
    metadata: crate::metadata::SYSTEM,
    warm_catalog: true,
    hold: pkgdeck_core::hold::current,
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
    /// Stops every thread that runs commands and waits for them, so their
    /// commands get SIGTERM before PkgDeck exits. Left running, they lose
    /// their pipe when it does, and some tools (grok) crash on that. All are
    /// told first, so their grace periods run together.
    fn drop(&mut self) {
        let worker = self.worker.take();
        let prefetch = self.prefetch_worker.take();
        let details = self.details_worker.take();
        let catalog = self.catalog_worker.take();
        let cancels = [
            worker.as_ref().map(|w| &w.cancel),
            prefetch.as_ref().map(|w| &w.cancel),
            details.as_ref().map(|w| &w.cancel),
            catalog.as_ref().map(|w| &w.cancel),
        ];
        for cancel in cancels.into_iter().flatten() {
            cancel.cancel();
        }
        let handles = [
            self.activity_worker.take().map(|w| w.handle),
            worker.map(|w| w.handle),
            prefetch.map(|w| w.handle),
            details.map(|w| w.handle),
            catalog.map(|w| w.handle),
        ];
        for handle in handles.into_iter().flatten() {
            let _ = handle.join();
        }
    }
}
pub struct Controller {
    rows: QString,
    details: QString,
    status: QString,
    notice: QString,
    /// The last change that failed, as its notice said, until it is
    /// dismissed, retried, or a later look shows it fixed. Activity shows it.
    failure: QString,
    /// The rows a change failed on, with why: `[{target, reason}]`.
    failed_rows: QString,
    progress: QString,
    /// What the last batch changed, read when `writing` goes false. See
    /// `ProgressState::batch_result`.
    batch_finished: QString,
    confirmation: QString,
    confirmation_data: QString,
    opened: QString,
    /// Where tests keep the AppImages launch settings read and change.
    appimage_data: Option<PathBuf>,
    /// The package `opened` shows.
    opened_package: Option<Package>,
    /// Flatpak links install for the whole system unless the app is
    /// already installed: the last choice in their review.
    flatpak_links_for_system: bool,
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
    /// The change the failure's Retry runs again.
    retry_job: Option<Job>,
    /// Each step that failed on a row, with why, until it works or a later
    /// look shows it done anyway. See `failed_rows`.
    failed: Vec<(Operation, String)>,
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
    /// The sources automatic updates leave for you.
    manual_sources: Vec<String>,
    /// What automatic updates and cleaning wait for, besides builds.
    waits: Waits,
    /// Why the updates the last background check found wait, if they do.
    auto_held: Option<Hold>,
    /// The sources whose updates a build holds, by name.
    auto_held_sources: Vec<String>,
    /// Let Update all remove packages when APT's plan does.
    allow_removals: bool,
    /// Clean once a day after a background check.
    auto_clean: bool,
    auto_clean_worktrees: bool,
    cleanup_preferences: CleanupPreferences,
    /// When automatic cleaning last ran, in seconds since 1970.
    auto_cleaned_at: u64,
    /// Cleanup tasks waiting for a build, tried again until it's over.
    waiting_cleans: Vec<WaitingClean>,
    /// When waiting tasks were last tried, in seconds since 1970.
    cleans_tried_at: u64,
    /// A window shows the sections; see `set_window_open`.
    window_open: bool,
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
            failure: "{}".into(),
            failed_rows: "[]".into(),
            progress: "{}".into(),
            batch_finished: "{}".into(),
            confirmation: QString::default(),
            confirmation_data: "{}".into(),
            opened: QString::default(),
            appimage_data: None,
            opened_package: None,
            flatpak_links_for_system: false,
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
            failed: vec![],
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
            manual_sources: vec![],
            waits: Waits::default(),
            auto_held: None,
            auto_held_sources: Vec::new(),
            allow_removals: true,
            auto_clean: false,
            auto_clean_worktrees: false,
            cleanup_preferences: CleanupPreferences::default(),
            auto_cleaned_at: 0,
            waiting_cleans: Vec::new(),
            cleans_tried_at: 0,
            source_filter: Vec::new(),
            window_open: true,
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
            || self.window_open && !self.prefetch.is_empty() && !held
    }
    /// Keep `waiting` tasks waiting for a build. A task already waiting
    /// takes the newer review, and settles every run it waits in.
    fn keep_waiting(&mut self, waiting: Vec<WaitingClean>) {
        for mut new in waiting {
            if let Some(at) = self
                .waiting_cleans
                .iter()
                .position(|old| old.item.id == new.item.id)
            {
                let old = self.waiting_cleans.remove(at);
                new.activity.splice(0..0, old.activity);
            }
            self.waiting_cleans.push(new);
        }
    }
    /// The sources whose automatic updates wait for builds right now.
    fn held_for_builds(&self) -> &[String] {
        match self.auto_held {
            Some(Hold::Build) => &self.auto_held_sources,
            _ => &[],
        }
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
    /// Names for what these operations change: remembered from
    /// confirmation, else from the rows on screen.
    fn names_for(&self, operations: &[Operation]) -> Names {
        let packages = operations
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
            .collect();
        let cleanup = operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::Clean(id) => self
                    .names
                    .cleanup
                    .get(id)
                    .cloned()
                    .or_else(|| {
                        self.cleanup
                            .iter()
                            .find(|item| item.id == *id)
                            .map(|item| item.title.clone())
                    })
                    .filter(|title| !title.trim().is_empty())
                    .map(|title| (id.clone(), title)),
                _ => None,
            })
            .collect();
        Names { packages, cleanup }
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
        Job::OpenInput(_) | Job::ImportRepository(_) | Job::AutoClean { .. } => vec![],
        Job::RetrySource(_, _, source) => vec![source.clone()],
        Job::RetryFailedUpdates(sources) | Job::RefreshSources(sources) => sources.clone(),
        Job::PlanOperation(operation) => vec![operation.backend().into()],
        // AppImages are taken over by the AppImage source itself.
        Job::PlanAdoption(app) if app.id.backend == "appimage" => vec!["appimage".into()],
        Job::PlanAdoptAll(_) => vec!["appimage".into()],
        Job::PlanAdoption(_) => vec!["homebrew-cask".into()],
        Job::Write(operation, _) => vec![operation.backend().into()],
        Job::BackgroundUpdates(sources) => sources.clone(),
        Job::CheckUpdates(_, sources) => sources.clone(),
        Job::UpgradeAll(operations, _) | Job::AutoUpgrade(operations, _) | Job::CleanAll(operations) => operations.iter().map(|op| op.backend().to_owned()).collect(),
        Job::PlanCleanAll(operations) => operations.iter().map(|op| op.backend().to_owned()).collect(),
        Job::RetryCleans(items) => items.iter().map(|item| item.id.backend.clone()).collect(),
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
    if !matches!(
        result,
        Err(EngineError::Refused {
            refusal: Refusal::PlanChanged,
            ..
        })
    ) {
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
/// The name people know a package source by, as the source list shows it.
/// A CLI's own updater says "(standalone)", apart from that CLI from npm
/// or Homebrew, unless its name already says how it was installed.
fn source_display_name(id: &str) -> String {
    let name = pkgdeck_core::backends::display_name(id);
    let standalone = pkgdeck_core::backends::StandaloneTool::ALL
        .iter()
        .any(|tool| tool.id() == id);
    if standalone && !name.ends_with(')') {
        format!("{name} (standalone)")
    } else {
        name.to_owned()
    }
}
/// A cleanup task for a confirmation: what, where, how much, then exactly
/// what it does.
fn cleanup_label(item: &CleanupItem) -> String {
    let mut label = format!("{} ({})", item.title, source_display_name(&item.id.backend));
    if let Some(bytes) = item.bytes.filter(|bytes| *bytes > 0) {
        label.push_str(&format!(", about {}", pkgdeck_core::disk::size(bytes)));
    }
    format!("{label}\n{}", item.preview)
}
/// "Frees about 4.2 GB.", counting the tasks that can tell.
fn cleanup_frees(items: &[&CleanupItem]) -> Option<String> {
    let bytes: u64 = items.iter().filter_map(|item| item.bytes).sum();
    (bytes > 0).then(|| format!("Frees about {}.", pkgdeck_core::disk::size(bytes)))
}
/// The confirmation for running `items` together.
fn clean_confirmation(items: &[&CleanupItem]) -> Value {
    let count = items.len();
    let noun = if count == 1 { "task" } else { "tasks" };
    let labels = items
        .iter()
        .map(|item| cleanup_label(item))
        .collect::<Vec<_>>()
        .join("\n\n");
    let frees = cleanup_frees(items)
        .map(|frees| format!("\n{frees}"))
        .unwrap_or_default();
    json!({
        "action": format!("Clean {count} {noun}"),
        "body": format!("{labels}\n\nTasks run in order. Finished tasks cannot be undone."),
        "summary": format!("Clean {count} {noun}{frees}\nFinished tasks cannot be undone."),
        "labels": labels,
        "items": items.iter().map(|item| json!({
            "group": source_display_name(&item.id.backend),
            "icon_name": item.id.backend,
            "title": item.title,
            "size": item.bytes.filter(|bytes| *bytes > 0).map(pkgdeck_core::disk::size),
            "preview": item.preview,
        })).collect::<Vec<_>>(),
    })
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
        // The window and `change_repository` name scopes as plain words.
        row["scope"] = json!(match repository.scope {
            Scope::System => "system",
            _ => "user",
        });
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
/// What people call what a change targets, remembered when the change is
/// confirmed so its progress and result can name it: packages by display
/// name (the map itself), cleanup tasks by their row's title.
#[derive(Clone, Debug, Default, PartialEq)]
struct Names {
    packages: BTreeMap<PackageId, String>,
    cleanup: BTreeMap<CleanupId, String>,
}
impl Names {
    fn new() -> Self {
        Self::default()
    }
    fn len(&self) -> usize {
        self.packages.len() + self.cleanup.len()
    }
    fn clear(&mut self) {
        self.packages.clear();
        self.cleanup.clear();
    }
    fn extend(&mut self, other: Names) {
        self.packages.extend(other.packages);
        self.cleanup.extend(other.cleanup);
    }
}
impl std::ops::Deref for Names {
    type Target = BTreeMap<PackageId, String>;
    fn deref(&self) -> &Self::Target {
        &self.packages
    }
}
impl std::ops::DerefMut for Names {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.packages
    }
}
impl<const N: usize> From<[(PackageId, String); N]> for Names {
    fn from(packages: [(PackageId, String); N]) -> Self {
        Self {
            packages: BTreeMap::from(packages),
            ..Self::default()
        }
    }
}
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
/// What the notice says when a change can't be prepared, for example when
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
        "urgent": is_denied(error),
    }))
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
        Ok(Payload::Batch(ran)) => ran.outcomes().map(|o| o == Outcome::Failed).collect(),
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
        Job::CheckUpdates(operations, sources) => {
            let retry = keep(operations);
            (!retry.is_empty()).then(|| Job::CheckUpdates(retry, sources.clone()))
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
    let (Job::AutoUpgrade(operations, _), Ok(Payload::Batch(ran))) = (job, result) else {
        return None;
    };
    let failed: Vec<&PackageId> = operations
        .iter()
        .zip(ran.outcomes())
        .filter(|(_, outcome)| *outcome != Outcome::Finished)
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
        Operation::Clean(id) => names
            .cleanup
            .get(id)
            .cloned()
            .unwrap_or_else(|| operation_title_named(operation, names)),
        _ => operation_title_named(operation, names),
    }
}
/// The notice title for one change that failed, such as "Firefox didn't
/// update".
fn failed_title(operation: &Operation, names: &Names) -> String {
    match operation {
        Operation::Upgrade(_) | Operation::UpgradeAll { .. } => {
            format!("{} didn't update", short_name(operation, names))
        }
        _ => format!("{} failed", operation_title_named(operation, names)),
    }
}
/// What the notice says once a change finishes.
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
        Ok(Payload::Batch(ran)) => ran.outcomes().all(|o| o == Outcome::Finished),
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
        Err(EngineError::Refused {
            refusal: Refusal::Building,
            ..
        }) => json!({
            "kind": "info",
            "title": format!("{title} waits for a build"),
            "detail": WAITS_FOR_BUILD,
        }),
        Err(error) => json!({
            "kind": "error",
            "title": match operations.as_slice() {
                [one] => failed_title(one, names),
                _ => format!("{title} failed"),
            },
            "detail": write_error_text(error, job_backend(&operations), sudo),
        }),
        Ok(Payload::Batch(ran))
            if !ran.outcomes().any(|o| o == Outcome::Failed)
                && ran.outcomes().any(|o| o == Outcome::Cancelled) =>
        {
            let done = ran.outcomes().filter(|o| *o == Outcome::Finished).count();
            json!({
                "kind": "info",
                "title": format!("{done} of {} changes finished. The rest were cancelled.", ran.steps.len()),
            })
        }
        Ok(Payload::Batch(ran)) if ran.outcomes().any(|o| o == Outcome::Failed) => {
            // One line per reason, naming the changes it stopped the way
            // people know them.
            let failed: Vec<(&Operation, String)> = operations
                .iter()
                .zip(&ran.steps)
                .filter(|(_, step)| step.outcome == Outcome::Failed)
                .map(|(operation, step)| (operation, step.said.text(sudo)))
                .collect();
            let mut reasons: Vec<(Vec<String>, &str)> = vec![];
            for (operation, reason) in &failed {
                let reason = reason.as_str();
                let name = short_name(operation, names);
                if reason.starts_with(&name) {
                    reasons.push((vec![], reason));
                } else if let Some((named, _)) = reasons
                    .iter_mut()
                    .find(|(named, same)| !named.is_empty() && *same == reason)
                {
                    named.push(name);
                } else {
                    reasons.push((vec![name], reason));
                }
            }
            let detail = reasons
                .iter()
                .map(|(named, reason)| match named.as_slice() {
                    [] => (*reason).to_owned(),
                    [one] => format!("{one}: {reason}"),
                    [first, second] => format!("{first} and {second}: {reason}"),
                    [first, second, third] => {
                        format!("{first}, {second} and {third}: {reason}")
                    }
                    [first, second, rest @ ..] => {
                        format!("{first}, {second} and {} more: {reason}", rest.len())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let title = match failed.as_slice() {
                [(one, _)] => failed_title(one, names),
                many => format!("{} of {} changes failed", many.len(), ran.steps.len()),
            };
            // Then what waits or was left alone, which needs nothing now.
            let detail = std::iter::once(detail)
                .chain(ran.left_alone(sudo))
                .collect::<Vec<_>>()
                .join("\n");
            json!({"kind": "error", "title": title, "detail": detail})
        }
        Ok(payload) => {
            // Keep what the user must still do or know, such as a firmware
            // restart or a cancellation that came too late.
            let detail = match payload {
                Payload::Batch(ran) => ran
                    .steps
                    .iter()
                    .filter(|step| step.said != Said::Completed)
                    .map(|step| format!("{}: {}", step.title, step.said.text(sudo)))
                    .chain(ran.note.map(str::to_owned))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Payload::Written(outcome) if outcome.cancellation_deferred => {
                    "Finished before it could be cancelled. Changes were kept.".into()
                }
                _ => String::new(),
            };
            match payload {
                // Some wait for a build or were left alone: not done yet,
                // but nothing went wrong.
                Payload::Batch(ran) if ran.left_alone(sudo).next().is_some() => {
                    let done = ran.outcomes().filter(|o| *o == Outcome::Finished).count();
                    json!({
                        "kind": "info",
                        "title": format!("{done} of {} {} finished", ran.steps.len(), ran.noun),
                        "detail": detail,
                    })
                }
                _ if detail.is_empty() => {
                    json!({"kind": "success", "title": format!("{title} finished")})
                }
                _ => {
                    json!({"kind": "success", "title": format!("{title} finished"), "detail": detail})
                }
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
    items: Vec<Value>,
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
            items: vec![],
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
        confirmation: format!("Update {count} selected {}?\n\n{labels}\n\nOther packages may also change. If one update fails, the others can still finish.", if count == 1 { "package" } else { "packages" }),
        items: update_items(&operations, packages),
        operations,
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
/// The kept sections live only in the file: the cache holds what shows,
/// and a second copy here would double the memory they take.
struct ViewStore {
    path: PathBuf,
    /// Writes run on their own threads; for each section the newest wins.
    written: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u64>>>,
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
    fn open(path: PathBuf) -> Self {
        Self {
            path,
            written: Default::default(),
            generation: 0,
        }
    }
    /// The kept snapshots, out of date until they load again.
    fn cached(&self) -> Vec<(String, CachedView)> {
        let loaded = Instant::now()
            .checked_sub(VIEW_TTL)
            .unwrap_or_else(Instant::now);
        stored_views(&self.path)
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
        self.generation += 1;
        let (generation, written) = (self.generation, self.written.clone());
        let path = self.path.clone();
        thread::spawn(move || write_view(&path, view, generation, &written));
    }
}
/// What the last run kept at `path`. A missing, damaged or older file is
/// the same as none.
fn stored_views(path: &std::path::Path) -> Vec<StoredView> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StoredViews>(&bytes).ok())
        .filter(|stored| stored.version == pkgdeck_core::VERSION)
        .map(|stored| stored.views)
        .unwrap_or_default()
}
/// Puts `view` in the file at `path` in place of its older snapshot, unless
/// a newer one of the same section was written already: writes run on
/// their own threads and may finish out of order. Whole, then renamed, so
/// a crash never leaves half a file.
fn write_view(
    path: &std::path::Path,
    view: StoredView,
    generation: u64,
    written: &std::sync::Mutex<std::collections::HashMap<String, u64>>,
) -> bool {
    // A writer that panicked left generations that are still worth comparing.
    let mut last = written
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let newest = last.entry(view.key.clone()).or_default();
    if *newest > generation {
        return false;
    }
    *newest = generation;
    let mut views = stored_views(path);
    views.retain(|old| old.key != view.key);
    views.push(view);
    let stored = StoredViews {
        version: pkgdeck_core::VERSION.into(),
        views,
    };
    let temporary = path.with_extension("json.tmp");
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    serde_json::to_vec(&stored)
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
/// The name a confirmation row shows for `package`.
fn confirm_title(package: &Package) -> &str {
    if package.display_name.is_empty() {
        package_name(&package.id)
    } else {
        &package.display_name
    }
}
/// One row of a confirmation's list: grouped by source, with the version
/// change and a note when known.
fn confirm_item(package: &Package) -> Value {
    json!({
        "group": source_display_name(&package.id.backend),
        "icon_name": package.id.backend,
        "title": confirm_title(package),
        "from": package.installed_version,
        "to": package.candidate_version,
        "icon": package.icon,
        "note": if package.id.backend == "fwupd" { package.summary.as_str() } else { "" },
    })
}
/// What an APT plan changes beyond the updates the dialog's rows already
/// name: extra installs and removals, or nothing.
fn apt_extra(plan: &AptUpgradePlan) -> String {
    let extra = [("Install", &plan.installs), ("Remove", &plan.removals)]
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(label, names)| format!("{label} ({}): {}", names.len(), names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n");
    if extra.is_empty() {
        extra
    } else {
        format!("\n\n{extra}")
    }
}
/// The packages `operations` update, one row each. A name that appears
/// more than once (such as an i386 build next to the native one) gets its
/// architecture so the rows can be told apart.
fn update_items(operations: &[Operation], packages: &[Package]) -> Vec<Value> {
    let mut chosen: Vec<&Package> = Vec::new();
    for operation in operations {
        match operation {
            Operation::Upgrade(id) => chosen.extend(packages.iter().filter(|p| p.id == *id)),
            Operation::UpgradeAll { backend } => {
                chosen.extend(packages.iter().filter(|p| {
                    p.id.backend == *backend && p.update == UpdateAvailability::Available
                }))
            }
            _ => {}
        }
    }
    let mut seen: std::collections::HashMap<(&str, &str), usize> = std::collections::HashMap::new();
    for package in &chosen {
        *seen
            .entry((package.id.backend.as_str(), confirm_title(package)))
            .or_default() += 1;
    }
    chosen
        .iter()
        .map(|package| {
            let mut item = confirm_item(package);
            let title = confirm_title(package);
            let repeated = seen[&(package.id.backend.as_str(), title)] > 1;
            if repeated && !package.id.architecture.is_empty() {
                item["title"] = json!(format!("{title} ({})", package.id.architecture));
            }
            item
        })
        .collect()
}
/// One line for progress and results, such as "Install htop (APT)".
/// A change's title as it runs: "Update Beta App" reads "Updating Beta
/// App…" on its progress card.
fn running_label(title: &str) -> String {
    if title.is_empty() {
        return String::new();
    }
    let (verb, rest) = title.split_once(' ').unwrap_or((title, ""));
    let running = match verb {
        "Install" => "Installing",
        "Remove" => "Removing",
        "Update" => "Updating",
        "Refresh" => "Refreshing",
        "Clean" => "Cleaning",
        "Add" => "Adding",
        "Enable" => "Enabling",
        "Disable" => "Disabling",
        "Change" => "Changing",
        "Open" => "Opening",
        _ => return format!("{title}…"),
    };
    if rest.is_empty() {
        format!("{running}…")
    } else {
        format!("{running} {rest}…")
    }
}
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
        // The row's title already names what it cleans.
        Operation::Clean(id) if names.cleanup.contains_key(id) => {
            format!("Clean {}", names.cleanup[id])
        }
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
            lines.extend(cleanup_frees(&[item]));
            summary.extend(cleanup_frees(&[item]));
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
            impact.push(format!("Download: {}", pkgdeck_core::disk::size(bytes)));
        }
        if let Some(bytes) = plan.disk_bytes {
            impact.push(format!(
                "Disk: {}{}",
                if bytes < 0 { "-" } else { "+" },
                pkgdeck_core::disk::size(bytes.unsigned_abs())
            ));
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
    // A reference with no comment starts with where it came from, which
    // the page already shows.
    let summary = package
        .summary
        .lines()
        .next()
        .filter(|line| !line.starts_with("Reference: "))
        .unwrap_or_default();
    let description = info
        .as_ref()
        .map(|info| info.description.clone())
        .filter(|description| !description.is_empty())
        .unwrap_or_else(|| details.description.clone());
    let location = match package.id.reference.as_deref() {
        // After its hash, the file or link it came from.
        Some(reference)
            if reference.starts_with("local-deb:") || reference.starts_with("flatpakref:") =>
        {
            reference
                .splitn(3, ':')
                .nth(2)
                .unwrap_or_default()
                .to_owned()
        }
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
        "launchable": package.installed_version.is_some() && launchable_flatpak(package),
        "runnable": runnable_file(package).is_some(),
    })
}
/// An AppImage file that isn't installed yet, which can still start from
/// where it is.
fn runnable_file(package: &Package) -> Option<&std::path::Path> {
    let path = std::path::Path::new(&package.id.name);
    (package.id.backend == "appimage" && package.installed_version.is_none() && path.is_absolute())
        .then_some(path)
}
/// A Flatpak app an opened file or link names, not a runtime: it can start
/// once it's installed.
fn launchable_flatpak(package: &Package) -> bool {
    package.id.backend == "flatpak"
        && !package
            .summary
            .lines()
            .any(|line| line.starts_with("Runtime: "))
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
        let page: Value = serde_json::from_str(&self.opened().to_string()).unwrap_or_default();
        let file = self
            .rust()
            .opened_package
            .as_ref()
            .filter(|_| page["runnable"] == json!(true))
            .and_then(runnable_file);
        let started = if let Some(path) = file {
            self.appimages().run_file(path)
        } else if let Some(id) = self.opened_appimage() {
            self.appimages().launch(&id)
        } else {
            match self.opened_flatpak() {
                Some(id) => pkgdeck_core::backends::launch_flatpak(&id),
                None => return "Nothing to start.".into(),
            }
        };
        match started {
            Ok(()) => QString::default(),
            Err(error) => plain_error(&error, None, false).as_str().into(),
        }
    }
    /// The Flatpak app the open page shows, once its page says it's installed.
    fn opened_flatpak(&self) -> Option<PackageId> {
        let package = self.rust().opened_package.as_ref()?;
        let page: Value = serde_json::from_str(&self.opened().to_string()).ok()?;
        (launchable_flatpak(package) && page["launchable"] == json!(true))
            .then(|| package.id.clone())
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
        // A reference names no version, but the app is installed all the same.
        let version = data["package"]["candidate"].clone();
        data["package"]["installed"] = if version.is_null() {
            json!("")
        } else {
            version
        };
        data["launchable"] = json!(launchable_flatpak(&package));
        // The installed copy starts instead.
        data["runnable"] = json!(false);
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
            // The open page installs where the review now says, so it must
            // know the install it waits for when that finishes.
            if let Some(opened) = rust.opened_package.as_mut().filter(|p| p.id == previous) {
                opened.id.scope = scope.clone();
            }
        }
        if self
            .rust()
            .opened_package
            .as_ref()
            .is_some_and(|p| p.id.scope == scope)
        {
            let mut data: Value =
                serde_json::from_str(&self.opened().to_string()).unwrap_or_default();
            data["package"]["scope"] = json!(scope);
            data["package"]["scope_label"] = json!(scope_label(&scope));
            self.as_mut().set_opened(encoded(data));
        }
        let mut selected = previous;
        selected.scope = scope;
        self.as_mut().apply(Ok(Payload::OperationPreview(
            Operation::Install(selected),
            None,
        )));
    }
    /// Where the login item goes, for tests of the window.
    #[doc(hidden)]
    pub fn use_autostart_path(self: Pin<&mut Self>, path: PathBuf) {
        self.rust_mut().autostart_path = Some(path);
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
        // Updates waiting for power or for a build: check again as soon as
        // that's over, instead of a whole interval later.
        let retry = self.rust().auto_update
            && self.rust().auto_held.is_some()
            && !busy
            && self.as_mut().still_held().is_none();
        if !self.as_mut().rust_mut().background_schedule.ready(
            now,
            enabled,
            offline,
            metered,
            busy,
            force || retry,
        ) {
            // Cleaning is due once a day, whatever the checks find. Tied to
            // checks alone, an automatic update or a build at check time
            // would put it off again and again.
            if enabled && !busy {
                self.as_mut().schedule_auto_clean();
            }
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
        if !enabled {
            self.as_mut().rust_mut().auto_held = None;
        }
    }
    pub fn auto_update(&self) -> bool {
        self.rust().auto_update
    }
    /// Leave these sources' updates for you, even with automatic updates.
    pub fn set_manual_sources(mut self: Pin<&mut Self>, sources: Vec<String>) {
        self.as_mut().rust_mut().manual_sources = sources;
    }
    pub fn manual_sources(&self) -> &[String] {
        &self.rust().manual_sources
    }
    /// Whether automatic updates wait for the charger and for a connection
    /// that isn't metered. Cleaning waits for the charger too.
    /// Where Flatpak links install by default: for the whole system, or for
    /// this user.
    pub fn set_flatpak_links_for_system(mut self: Pin<&mut Self>, system: bool) {
        self.as_mut().rust_mut().flatpak_links_for_system = system;
    }
    pub fn flatpak_links_for_system(&self) -> bool {
        self.rust().flatpak_links_for_system
    }
    pub fn set_waits(mut self: Pin<&mut Self>, battery: bool, metered: bool) {
        self.as_mut().rust_mut().waits = Waits { battery, metered };
    }
    pub fn waits(&self) -> Waits {
        self.rust().waits
    }
    /// Why automatic updates wait now, if they still do. A new reason
    /// replaces the old one in the background state.
    fn still_held(mut self: Pin<&mut Self>) -> Option<Hold> {
        let natives = self.rust().natives;
        let hold = (natives.hold)(&(natives.host)(), self.rust().waits);
        if hold.is_some() && hold != self.rust().auto_held {
            self.as_mut().rust_mut().auto_held = hold;
            let mut state: Value =
                serde_json::from_str(&self.background_state().to_string()).unwrap_or_default();
            state["waiting"] = json!(hold.map(Hold::key));
            state["waiting_for"] = json!(self.rust().held_for_builds());
            state["notify"] = json!(false);
            self.as_mut().set_background_state(encoded(state));
        }
        hold
    }
    /// Carry over when the copy that ran before last checked, so this one
    /// waits out the interval instead of checking right away.
    pub fn resume_checks(mut self: Pin<&mut Self>, last_check: u64) {
        self.as_mut()
            .rust_mut()
            .background_schedule
            .resume(last_check);
    }
    /// Whether a window shows the sections. Without one, nothing preloads
    /// and what was preloaded goes, so PkgDeck stays small in the tray.
    pub fn set_window_open(mut self: Pin<&mut Self>, open: bool) {
        let rust = self.as_mut().rust_mut();
        if rust.window_open == open {
            return;
        }
        rust.window_open = open;
        rust.invalidate_prefetch();
        if open {
            // Show what was kept at once, while the sections load again.
            if rust.view_cache.entries.is_empty() {
                let kept = rust.view_store.as_ref().map(ViewStore::cached);
                for (key, view) in kept.into_iter().flatten() {
                    rust.view_cache.insert(key, view);
                }
            }
        } else {
            rust.prefetch.clear();
            rust.view_cache.entries.clear();
            rust.prepared_rows = RowsCache::default();
            rust.invalidate_details();
            if rust.worker.is_none() {
                rust.engine = None;
                rust.engine_scope = None;
            }
        }
        self.sync_needs_poll();
    }
    /// Clean automatically, and when that last happened, so a restart
    /// doesn't clean again the same day.
    pub fn set_auto_clean(mut self: Pin<&mut Self>, enabled: bool, cleaned_at: u64) {
        self.as_mut().rust_mut().auto_clean = enabled;
        self.as_mut().rust_mut().auto_cleaned_at = cleaned_at;
    }
    pub fn auto_clean(&self) -> bool {
        self.rust().auto_clean
    }
    pub fn set_auto_clean_worktrees(mut self: Pin<&mut Self>, enabled: bool) {
        self.as_mut().rust_mut().auto_clean_worktrees = enabled;
    }
    pub fn set_cleanup_preferences(mut self: Pin<&mut Self>, preferences: QString) {
        if let Ok(preferences) = serde_json::from_str(&preferences.to_string()) {
            self.as_mut().rust_mut().cleanup_preferences = preferences;
        }
    }
    pub fn auto_clean_worktrees(&self) -> bool {
        self.rust().auto_clean_worktrees
    }
    /// Clean when it's been a day, nothing else is waiting to run, and the
    /// computer is plugged in and not building: after a check that started
    /// no update, or between checks.
    fn schedule_auto_clean(mut self: Pin<&mut Self>) {
        const DAY: u64 = 24 * 60 * 60;
        let now = epoch_seconds();
        let rust = self.rust();
        if (!rust.auto_clean && !rust.auto_clean_worktrees)
            || now < rust.auto_cleaned_at.saturating_add(DAY)
            || rust.pending.is_some()
            || rust.queued.is_some()
            || rust.validated_confirmed.is_some()
            || !rust.confirmed_queue.is_empty()
        {
            return;
        }
        // Cleaning downloads nothing, so a metered connection doesn't count.
        // A build holds only the tasks that remove what it uses: they wait
        // for it on their own.
        let natives = rust.natives;
        let waits = Waits {
            metered: false,
            ..rust.waits
        };
        if (natives.hold)(&(natives.host)(), waits) == Some(Hold::Battery) {
            return;
        }
        let job = Job::AutoClean {
            caches: rust.auto_clean,
            worktrees: rust.auto_clean_worktrees,
            preferences: rust.cleanup_preferences.clone(),
        };
        self.as_mut().rust_mut().validated_confirmed = Some(Confirmed {
            job,
            activity_id: None,
            cleanup_preview: vec![],
        });
        self.sync_needs_poll();
    }
    /// Remember the cleanup tasks of `job` that wait for a build, as they
    /// were reviewed, to try them again until it's over. `activity` is the
    /// Activity run they belong to.
    fn wait_for_builds(
        mut self: Pin<&mut Self>,
        job: &Job,
        ended: &[(Outcome, Option<String>)],
        activity: Option<u64>,
    ) {
        let waiting: Vec<WaitingClean> = job
            .operations()
            .iter()
            .zip(ended)
            .enumerate()
            .filter(|(_, (_, (outcome, _)))| *outcome == Outcome::Waiting)
            .filter_map(|(index, (operation, _))| {
                let Operation::Clean(id) = operation else {
                    return None;
                };
                let item = self.rust().cleanup.iter().find(|item| item.id == *id)?;
                Some(WaitingClean {
                    item: item.clone(),
                    activity: activity.map(|activity| (activity, index)).into_iter().collect(),
                })
            })
            .collect();
        self.as_mut().rust_mut().keep_waiting(waiting);
    }
    /// Show what automatic cleaning ran in Activity, and keep the tasks
    /// that wait for a build waiting.
    fn record_auto_clean(
        mut self: Pin<&mut Self>,
        tasks: Vec<(CleanupItem, Outcome, Option<String>)>,
        started_at: u64,
    ) {
        if tasks.is_empty() {
            return;
        }
        for (item, ..) in &tasks {
            let title = item.title.clone();
            self.as_mut()
                .rust_mut()
                .names
                .cleanup
                .insert(item.id.clone(), title);
        }
        let operations = tasks
            .iter()
            .map(|(item, ..)| Operation::Clean(item.id.clone()))
            .collect();
        let ended = tasks
            .iter()
            .map(|(_, outcome, kept)| (*outcome, kept.clone()))
            .collect();
        let activity = self
            .rust()
            .activity_store
            .clone()
            .and_then(|store| store.record("auto", operations, started_at, ended).ok());
        let waiting = tasks
            .into_iter()
            .enumerate()
            .filter(|(_, (_, outcome, _))| *outcome == Outcome::Waiting)
            .map(|(index, (item, ..))| WaitingClean {
                item,
                activity: activity.map(|id| (id, index)).into_iter().collect(),
            })
            .collect();
        self.as_mut().rust_mut().keep_waiting(waiting);
        self.as_mut().refresh_activity();
    }
    /// Try the cleanup tasks waiting for a build again, at most once a
    /// minute and only while nothing else runs. The window calls this on a
    /// timer; a task still waiting just keeps waiting.
    pub fn retry_waiting_cleans(mut self: Pin<&mut Self>) {
        let now = epoch_seconds();
        let rust = self.rust();
        if rust.waiting_cleans.is_empty()
            || now < rust.cleans_tried_at.saturating_add(RETRY_CLEANS_EVERY)
            || rust.worker.is_some()
            || rust.pending.is_some()
            || rust.queued.is_some()
            || rust.validated_confirmed.is_some()
            || !rust.confirmed_queue.is_empty()
        {
            return;
        }
        let items = rust
            .waiting_cleans
            .iter()
            .map(|waiting| waiting.item.clone())
            .collect();
        self.as_mut().rust_mut().cleans_tried_at = now;
        // Behind anything the person asks for, like a background check.
        self.as_mut().rust_mut().background = true;
        self.start(Job::RetryCleans(items));
    }
    /// Settle the waiting tasks a retry ran in Activity. Those still waiting,
    /// or that the retry didn't reach, keep waiting.
    fn finish_retried_cleans(mut self: Pin<&mut Self>, items: &[CleanupItem], ran: &Batch) {
        let store = self.rust().activity_store.clone();
        let mut cleaned = false;
        for (item, step) in items.iter().zip(&ran.steps) {
            if matches!(step.outcome, Outcome::Waiting | Outcome::Cancelled) {
                continue;
            }
            let rust = self.as_mut().rust_mut();
            let Some(at) = rust
                .waiting_cleans
                .iter()
                .position(|waiting| waiting.item.id == item.id)
            else {
                continue;
            };
            let waiting = rust.waiting_cleans.remove(at);
            cleaned |= step.outcome == Outcome::Finished;
            if let Some(store) = &store {
                for (id, index) in waiting.activity {
                    let _ = store.settle(id, index, step.outcome, step.kept.clone());
                }
            }
        }
        if cleaned {
            // Cleaning may remove packages, images and caches lists show.
            self.as_mut()
                .apply(Ok(Payload::Written(OperationOutcome::default())));
        }
        self.as_mut().refresh_activity();
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
    /// how many updates the queued run leaves for you, or nothing when no
    /// run was queued.
    fn schedule_auto_update(mut self: Pin<&mut Self>, report: &PackageReport) -> Option<u64> {
        use pkgdeck_core::unattended::{unattended, Unattended};
        self.as_mut().rust_mut().auto_held = None;
        let rust = self.rust();
        if !rust.auto_update
            || rust.worker.is_some()
            || rust.pending.is_some()
            || rust.queued.is_some()
            || rust.validated_confirmed.is_some()
            || !rust.confirmed_queue.is_empty()
        {
            return None;
        }
        let approved = rust.approval_current();
        // A source whose check failed may have more to update than it said.
        let packages: Vec<Package> = report
            .packages
            .iter()
            .filter(|p| !report.failures.iter().any(|f| f.backend == p.id.backend))
            .filter(|p| !pkgdeck_core::backends::inventory_only(&p.id.backend))
            .filter(|p| !rust.manual_sources.contains(&p.id.backend))
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
            return None;
        }
        // On battery or a metered connection, wait: check_updates tries
        // again once that's over. A build holds only the updates that could
        // change what it runs; the rest go ahead.
        let natives = self.rust().natives;
        let hold = (natives.hold)(&(natives.host)(), self.rust().waits);
        let (operations, held): (Vec<_>, Vec<_>) = match hold {
            Some(Hold::Build) => operations
                .into_iter()
                .partition(|operation| !waits_for_builds(operation.backend())),
            _ => (operations, vec![]),
        };
        let mut held_sources: Vec<String> = held
            .iter()
            .map(|operation| source_display_name(operation.backend()))
            .collect();
        held_sources.sort();
        held_sources.dedup();
        self.as_mut().rust_mut().auto_held_sources = held_sources;
        self.as_mut().rust_mut().auto_held = match hold {
            Some(Hold::Build) if held.is_empty() => None,
            hold => hold,
        };
        if matches!(hold, Some(Hold::Battery | Hold::Metered)) || operations.is_empty() {
            return None;
        }
        let left = report
            .packages
            .iter()
            .filter(|p| p.installed_version.is_some() && p.update == UpdateAvailability::Available)
            .filter(|p| {
                !operations
                    .iter()
                    .chain(&held)
                    .any(|o| o.backend() == p.id.backend)
            })
            .count() as u64;
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
        Some(left)
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
            Ok(Payload::Batch(ran)) => ran.outcomes().map(|o| o == Outcome::Failed).collect(),
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
    /// Forgets the failed rows a complete look shows done anyway, such as
    /// an update made some other way or a cleanup task that is gone. The
    /// failure goes too once none of its steps is left.
    fn settle_failures(mut self: Pin<&mut Self>, cleanup: Option<&CleanupReport>) {
        let view = self.rust().active_view.clone();
        let updates = self.rust().updates_view;
        let packages = &self.rust().packages;
        let successful = self.successful_sources();
        let answered = |backend: &str| {
            (if cleanup.is_some() {
                self.rust().source_filter.is_empty()
                    || self
                        .rust()
                        .source_filter
                        .iter()
                        .any(|source| source == backend)
            } else {
                successful.iter().any(|source| source == backend)
            }) && !self
                .rust()
                .failures
                .iter()
                .any(|failure| failure.backend == backend)
        };
        let done = |operation: &Operation| match operation {
            Operation::Clean(id) => cleanup.is_some_and(|report| {
                answered(&id.backend) && !report.items.iter().any(|item| item.id == *id)
            }),
            _ if cleanup.is_some() => false,
            Operation::Upgrade(id) => {
                updates
                    && answered(&id.backend)
                    && !packages
                        .iter()
                        .any(|p| p.id == *id && p.update == UpdateAvailability::Available)
            }
            Operation::Install(id) => packages
                .iter()
                .any(|p| p.id == *id && p.installed_version.is_some()),
            Operation::Remove(id) => {
                view == "Installed"
                    && answered(&id.backend)
                    && !packages.iter().any(|p| p.id == *id)
            }
            _ => false,
        };
        let failed: Vec<_> = self
            .rust()
            .failed
            .iter()
            .filter(|(operation, _)| !done(operation))
            .cloned()
            .collect();
        let settled = self.rust().retry_job.as_ref().is_some_and(|job| {
            let operations = job.operations();
            !operations.is_empty() && operations.iter().all(&done)
        });
        if failed.len() != self.rust().failed.len() {
            self.as_mut().rust_mut().failed = failed;
            self.as_mut().publish_failed_rows();
        }
        if settled {
            self.dismiss_failure();
        }
    }
    fn publish_failed_rows(self: Pin<&mut Self>) {
        let rows: Vec<Value> = self
            .rust()
            .failed
            .iter()
            .filter_map(|(operation, reason)| {
                let target = match operation {
                    Operation::Clean(id) => {
                        json!({"source": id.backend, "cleanup_key": id.key})
                    }
                    _ => target_row(package_target(operation)?),
                };
                Some(json!({"target": target, "reason": reason}))
            })
            .collect();
        self.set_failed_rows(encoded(rows));
    }
    /// Which rows this finished change failed on, and why; the ones that
    /// worked are forgotten.
    fn remember_failed_rows(
        mut self: Pin<&mut Self>,
        job: &Job,
        result: &Result<Payload, EngineError>,
    ) {
        let operations = job.operations();
        let sudo = self.rust().sudo;
        let outcomes: Vec<(Operation, Option<String>)> = match result {
            Err(
                EngineError::Cancelled
                | EngineError::Execution(pkgdeck_core::process::ExecutionError::Cancelled),
            ) => return,
            Err(error) => operations
                .into_iter()
                .map(|operation| {
                    let reason = write_error_text(error, Some(operation.backend()), sudo);
                    (operation, Some(reason))
                })
                .collect(),
            Ok(Payload::Batch(ran)) => operations
                .into_iter()
                .zip(&ran.steps)
                .filter_map(|(operation, step)| match step.outcome {
                    Outcome::Finished => Some((operation, None)),
                    Outcome::Failed => Some((operation, Some(step.said.text(sudo)))),
                    _ => None,
                })
                .collect(),
            Ok(_) => operations
                .into_iter()
                .map(|operation| (operation, None))
                .collect(),
        };
        let mut failed = self.rust().failed.clone();
        for (operation, reason) in outcomes {
            if !matches!(
                operation,
                Operation::Install(_)
                    | Operation::Remove(_)
                    | Operation::Upgrade(_)
                    | Operation::Clean(_)
            ) {
                continue;
            }
            failed.retain(|(old, _)| *old != operation);
            if let Some(reason) = reason {
                failed.push((operation, reason));
            }
        }
        self.as_mut().rust_mut().failed = failed;
        self.publish_failed_rows();
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
        // of announcing updates it is about to install. The ones it leaves
        // for you are announced still.
        let left = self.as_mut().schedule_auto_update(&report);
        let automatic = left.is_some();
        if !automatic {
            self.as_mut().schedule_auto_clean();
        }
        if left == Some(0) {
            self.as_mut()
                .rust_mut()
                .background_schedule
                .acknowledge_notification();
        }
        let saved = self.rust().background_schedule.notification_history();
        self.as_mut()
            .set_notification_history(saved.as_str().into());
        let waiting = self.rust().auto_held.map(Hold::key);
        let waiting_for = self.rust().held_for_builds().to_vec();
        self.as_mut().set_background_state(encoded(json!({"last_check": checked, "available": result.count, "failures": failures, "notify": result.notify && left != Some(0), "manual": left, "waiting": waiting, "waiting_for": waiting_for})));
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
    /// Hides the notice. A failure stays in Activity, with its Retry.
    pub fn dismiss_notice(self: Pin<&mut Self>) {
        self.set_notice("{}".into());
    }
    /// Forgets the last failure, its Retry, and the notice still showing it.
    pub fn dismiss_failure(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().retry_job = None;
        let notice: Value = serde_json::from_str(&self.notice().to_string()).unwrap_or_default();
        if notice["kind"] == "error" {
            self.as_mut().set_notice("{}".into());
        }
        self.set_failure("{}".into());
    }
    /// Run again what the failure reports as failed. The change was already
    /// confirmed, and it is reviewed again before it runs, like any other.
    pub fn retry_change(mut self: Pin<&mut Self>) {
        let Some(job) = self.as_mut().rust_mut().retry_job.take() else {
            return;
        };
        self.as_mut().set_notice("{}".into());
        self.as_mut().set_failure("{}".into());
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
                .map(|_| (Outcome::Failed, None))
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
                    .map(|_| (Outcome::Cancelled, None))
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
                    Reply::Done(Ok(Payload::CheckedUpdates(report, _))) => {
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
                let mut result = inspect_open_input(input, &token);
                // The catalog names an opened app and gives its icon,
                // description and screenshots, as on a search result's page.
                if let Ok(Payload::OpenPackage(details)) = &mut result {
                    crate::metadata::enrich(&mut details.package, natives.metadata);
                    crate::metadata::details(&mut details.package, &token, natives.metadata);
                }
                send(Reply::Done(result));
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
                    engine.set_unattended(matches!(
                        job,
                        Job::AutoUpgrade(..) | Job::AutoClean { .. }
                    ));
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
            // An empty search shows nothing, so a section still loading,
            // or waiting to, must not land here once it's done.
            self.as_mut().rust_mut().deferred_load = None;
            let section = |job: &Job| {
                matches!(
                    job,
                    Job::Load(..)
                        | Job::RetrySource(..)
                        | Job::RetryFailedUpdates(..)
                        | Job::RefreshSources(..)
                )
            };
            if self.rust().queued.as_ref().is_some_and(section) {
                self.as_mut().rust_mut().queued = None;
            }
            if let Some(worker) = self.rust().worker.as_ref().filter(|w| section(&w.job)) {
                worker.cancel.cancel();
                self.as_mut().rust_mut().background = true;
            }
            // Busy only while something the person started still runs, not
            // for a section read or a preload the page waited on.
            let busy = self.rust().foreground_worker();
            self.as_mut().set_busy(busy);
            if self.rows().to_string() != "[]" {
                self.as_mut().rust_mut().packages.clear();
                self.as_mut().rust_mut().cleanup.clear();
                self.as_mut().rust_mut().sources.clear();
                self.as_mut().set_rows("[]".into());
            }
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
    /// Re-read the Updates rows of `sources` (a comma list) after a batch
    /// wrote to them. Unlike `retry_source`, it never plans upgrades.
    pub fn refresh_sources(mut self: Pin<&mut Self>, sources: QString) {
        if self.rust().foreground_worker() || self.rust().pending.is_some() {
            return;
        }
        let sources: Vec<String> = sources
            .to_string()
            .split(',')
            .filter(|source| {
                pkgdeck_core::backends::BACKEND_IDS.contains(source)
                    && (self.rust().source_filter.is_empty()
                        || self.rust().source_filter.iter().any(|f| f == source))
            })
            .map(str::to_owned)
            .collect();
        if sources.is_empty() {
            return;
        }
        self.as_mut().set_phase("loading");
        self.start(Job::RefreshSources(sources));
    }
    /// One explicit update check: refreshes every selected source that has
    /// package lists, then checks them all, registries without lists
    /// included. Refreshes go without a password where the system allows
    /// (PackageKit, DNF's own cache); the rest ask for it once.
    pub fn check_for_updates(mut self: Pin<&mut Self>, sources: QString) {
        if self.rust().foreground_worker() || self.rust().pending.is_some() {
            return;
        }
        let mut sources: Vec<String> = sources
            .to_string()
            .split(',')
            .map(str::trim)
            .filter(|source| !source.is_empty())
            .map(str::to_owned)
            .collect();
        if sources.is_empty()
            || sources
                .iter()
                .any(|source| !pkgdeck_core::backends::BACKEND_IDS.contains(&source.as_str()))
        {
            return;
        }
        sources.sort();
        sources.dedup();
        let catalog: Vec<Value> =
            serde_json::from_str(self.source_catalog().as_str()).unwrap_or_default();
        // A source that can't run here, like a macOS one on Linux, is
        // skipped, as on any Updates load: it hasn't failed a check.
        sources.retain(|source| {
            !catalog.iter().any(|row| {
                row["source"].as_str() == Some(source.as_str()) && row["available"] == false
            })
        });
        if sources.is_empty() {
            return;
        }
        let operations = sources
            .iter()
            .filter(|source| {
                // A check never asks for a password: system managers refresh
                // through PackageKit or the user's own copy of their index
                // instead, as the check reads them.
                !pkgdeck_core::backends::refresh_needs_root(source)
                    && catalog.iter().any(|row| {
                        row["source"].as_str() == Some(source.as_str())
                            && row["available"] == true
                            && row["capabilities"]
                                .as_array()
                                .is_some_and(|caps| caps.contains(&json!("refresh")))
                    })
            })
            .map(|backend| Operation::Refresh {
                backend: backend.clone(),
            })
            .collect();
        self.as_mut().rust_mut().active_view = "Updates".into();
        self.as_mut().rust_mut().source_filter = sources.clone();
        self.as_mut().rust_mut().updates_view = true;
        self.as_mut().set_phase("loading");
        let job = Job::CheckUpdates(operations, sources);
        if job.operations().is_empty() {
            self.start(job);
        } else {
            self.accept_confirmed(job);
        }
    }
    pub fn checking_updates(&self) -> bool {
        self.rust()
            .worker
            .as_ref()
            .is_some_and(|worker| matches!(worker.job, Job::CheckUpdates(..)))
            || self
                .rust()
                .confirmed_queue
                .iter()
                .any(|entry| matches!(entry.job, Job::CheckUpdates(..)))
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
                    .set_status("Retrying failed source checks before updating…".into());
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
    pub fn propose_clean_checked(mut self: Pin<&mut Self>, identities: QString) {
        let wanted: Vec<CleanupId> =
            serde_json::from_str(&identities.to_string()).unwrap_or_default();
        let cleanup = self.rust().cleanup.clone();
        let items: Vec<_> = cleanup
            .iter()
            .filter(|item| {
                wanted.contains(&item.id) && self.rust().cleanup_preferences.includes_item(item)
            })
            .collect();
        if items.is_empty() {
            self.as_mut().set_confirmation(QString::default());
            self.as_mut().set_confirmation_data("{}".into());
            self.rust_mut().pending = None;
            return;
        }
        let operations: Vec<_> = items
            .iter()
            .map(|item| Operation::Clean(item.id.clone()))
            .collect();
        let confirmation = clean_confirmation(&items);
        let count = operations.len();
        let noun = if count == 1 { "task" } else { "tasks" };
        let body = confirmation["body"].as_str().unwrap_or_default().to_owned();
        self.as_mut().set_confirmation_data(encoded(confirmation));
        self.as_mut().set_confirmation(
            format!("Run {count} cleanup {noun}?\n\n{body}")
                .as_str()
                .into(),
        );
        self.rust_mut().pending = Some(Job::CleanAll(operations));
    }

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
                json!({"action":"Update", "body": plan.confirmation, "summary": format!("Update {count} selected {noun}{warning}"), "items": plan.items}),
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
                .set_status("Cancelling once the package manager finishes safely…".into());
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
            // Settled where the retry's reply arrives, behind everything.
            Ok(Payload::RetriedCleans(_)) => {}
            Ok(Payload::AutoCleaned {
                cleaned,
                failed,
                bytes,
                tasks,
                started_at,
            }) => {
                let now = epoch_seconds();
                self.as_mut().rust_mut().auto_cleaned_at = now;
                self.as_mut().record_auto_clean(tasks, started_at);
                if cleaned > 0 {
                    // Cleaning may remove packages, images and caches lists show.
                    self.as_mut()
                        .apply(Ok(Payload::Written(OperationOutcome::default())));
                }
                let mut state: Value =
                    serde_json::from_str(&self.background_state().to_string()).unwrap_or_default();
                if !state.is_object() {
                    state = json!({});
                }
                state["cleaned_at"] = json!(now);
                state["cleaned"] = json!({"tasks": cleaned, "failed": failed, "bytes": bytes});
                state["notify"] = json!(false);
                self.as_mut().set_background_state(encoded(state));
            }
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
            Ok(Payload::OpenPackage(mut details)) => {
                let reference = details.package.id.reference.as_deref();
                if self.rust().flatpak_links_for_system
                    && details.package.installed_version.is_none()
                    && reference.is_some_and(|value| value.starts_with("flatpakref:"))
                {
                    details.package.id.scope = Scope::System;
                }
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
                    pkgdeck_core::installed_copy::action(&package)
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
                            "\nAPT is left out: it would remove {}. Turn on Allow removals in Settings to include it.",
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
                let apt = apt_plan.as_ref().map_or_else(String::new, apt_extra);
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
                let items = update_items(&operations, &self.rust().packages);
                self.as_mut().set_confirmation_data(encoded(json!({"action":"Update", "body": format!("{count} listed {noun}{incomplete}{left_out}{apt}\n\n{labels}"), "summary": format!("Update {count} {noun}{removals}{warning}{incomplete}{left_out}"), "details": format!("{incomplete}{left_out}{apt}").trim(), "items": items})));
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
                    let mut confirmation = clean_confirmation(&selected.iter().collect::<Vec<_>>());
                    let body = confirmation["labels"].clone();
                    confirmation["body"] = body.clone();
                    self.as_mut().set_confirmation_data(encoded(confirmation));
                    self.as_mut()
                        .set_confirmation(body.as_str().unwrap_or_default().into());
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
                if self.rust().worker.is_none() {
                    self.settle_failures(None);
                }
            }
            Ok(Payload::Cleanup(mut report)) => {
                // The biggest first; tasks that can't tell keep their order
                // at the end.
                report
                    .items
                    .sort_by_key(|item| std::cmp::Reverse(item.bytes.unwrap_or(0)));
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
                            "cleanup_opt_in": item.opt_in(),
                            "preview": item.preview, "bytes": item.bytes
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
                let frees: u64 = report.items.iter().filter_map(|item| item.bytes).sum();
                self.as_mut().rust_mut().failures = report.failures.clone();
                self.as_mut().settle_failures(Some(&report));
                self.as_mut().rust_mut().cleanup = report.items;
                self.as_mut().rust_mut().failures = report.failures;
                self.as_mut().set_rows(encoded(rows));
                let status = if incomplete {
                    "Some cleanup sources could not be checked.".to_owned()
                } else if empty {
                    "Nothing to clean.".to_owned()
                } else if frees > 0 {
                    format!(
                        "About {} can be freed. Review a cleanup task before running it.",
                        pkgdeck_core::disk::size(frees)
                    )
                } else {
                    "Review a cleanup task before running it.".to_owned()
                };
                self.set_status(status.as_str().into());
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
            Ok(Payload::Batch(ran)) => {
                // Native writes may change dependencies in other rows.
                self.as_mut()
                    .apply(Ok(Payload::Written(OperationOutcome::default())));
                let sudo = self.rust().sudo;
                self.set_status(ran.text(sudo).as_str().into());
            }
            Ok(Payload::CheckedUpdates(report, _)) => {
                self.apply(Ok(Payload::Packages(*report)));
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
        // Without a window nobody switches sections; keep nothing.
        if !self.rust().window_open {
            return;
        }
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
    /// Checks every source again, such as after one failed its check.
    pub fn recheck_sources(mut self: Pin<&mut Self>) {
        self.as_mut().forget_catalog();
        self.check_sources();
    }
    /// Lets the next check run. One running already stays the only one.
    fn forget_catalog(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().catalog_checked = false;
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
        // Nobody sees preloaded sections without a window.
        if !self.rust().window_open {
            return;
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
                if let Some(Reply::Done(Ok(Payload::Batch(ran)))) =
                    replies.iter().find(|reply| matches!(reply, Reply::Done(_)))
                {
                    let count = |outcome: Outcome| ran.outcomes().filter(|o| *o == outcome).count();
                    self.as_mut().set_auto_update_result(encoded(json!({
                        "updated": count(Outcome::Finished),
                        "failed": count(Outcome::Failed) + count(Outcome::Cancelled),
                        "total": operations.len(),
                    })));
                }
            }
            // A source that just refreshed was checked successfully, even
            // before any page has listed it.
            let finished: Vec<Operation> = match replies
                .iter()
                .find(|reply| matches!(reply, Reply::Done(_)))
            {
                Some(Reply::Done(Ok(Payload::Batch(ran) | Payload::CheckedUpdates(_, ran)))) => job
                    .operations()
                    .into_iter()
                    .zip(ran.outcomes())
                    .filter(|(_, outcome)| *outcome == Outcome::Finished)
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
                            Reply::Done(Ok(Payload::RetriedCleans(ran))) => {
                                if let Job::RetryCleans(items) = &job {
                                    self.as_mut().finish_retried_cleans(items, &ran);
                                }
                            }
                            // Tasks a retry couldn't start keep waiting.
                            Reply::Done(Err(_)) if matches!(job, Job::RetryCleans(_)) => {}
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
                        let (result, checked) = match result {
                            Ok(Payload::CheckedUpdates(report, ran)) => {
                                (Ok(Payload::Batch(ran)), Some(*report))
                            }
                            result => (result, None),
                        };
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
                                self.as_mut().rust_mut().retry_job = None;
                                self.as_mut().set_failure(encoded(notice.clone()));
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
                            // Only a failure replaces the last one, and its Retry.
                            if notice["kind"] == "error" {
                                // Only a password refused needs the person now.
                                notice["urgent"] =
                                    json!(matches!(&result, Err(error) if is_denied(error)));
                                self.as_mut().rust_mut().retry_job = retry;
                                self.as_mut().set_failure(encoded(notice.clone()));
                            }
                            self.as_mut().remember_failed_rows(&job, &result);
                            if !matches!(job, Job::CheckUpdates(..)) || notice["kind"] != "success"
                            {
                                self.as_mut().set_notice(encoded(notice));
                            }
                            let ended: Vec<_> = match &result {
                                Ok(Payload::Batch(ran)) => ran.ended(),
                                Ok(_) => job
                                    .operations()
                                    .iter()
                                    .map(|_| (Outcome::Finished, None))
                                    .collect(),
                                Err(error) => job
                                    .operations()
                                    .iter()
                                    .map(|operation| ended_by(error, operation.backend()))
                                    .collect(),
                            };
                            let activity = self.as_mut().rust_mut().active_activity_id.take();
                            self.as_mut().wait_for_builds(&job, &ended, activity);
                            if let Some(id) = activity {
                                if let Some(store) = self.rust().activity_store.clone() {
                                    let _ = store.finish(id, ended);
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
                            Job::RetryFailedUpdates(_) | Job::RefreshSources(_)
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
                        if let Some(report) = checked {
                            // A read for another page may have queued while the
                            // refresh ran. Its rows and source selection win.
                            if self.rust().active_view == "Updates"
                                && self.rust().queued.is_none()
                                && self.rust().deferred_load.is_none()
                            {
                                self.as_mut().apply(Ok(Payload::Packages(report)));
                                let key = cache_key(
                                    "Updates",
                                    "",
                                    &self.rust().source_filter,
                                    self.rust().sudo,
                                );
                                self.as_mut().stash_current(key);
                            }
                        } else if matches!(job, Job::CheckUpdates(..)) && cancel.requested() {
                            self.as_mut().set_phase("cancelled");
                        }
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
            // Set before the progress clears, so the frontend can read what
            // the batch changed when `writing` goes false.
            let batch = self
                .as_mut()
                .rust_mut()
                .progress_state
                .take()
                .map_or_else(|| "{}".into(), |state| state.batch_result());
            self.as_mut().set_batch_finished(batch);
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
mod tests;
