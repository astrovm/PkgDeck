//! PkgDeck's Installed page drawn with egui: a trial of a GUI without Qt.
//! It reads the same engine the Qt app does, on worker threads, and keeps
//! everything it shows in plain Rust state so tests can check it directly.
//! `view` draws that state; `theme` gives it PkgDeck's look.

mod icons;
pub mod theme;
mod view;

use eframe::egui;
use pkgdeck_core::{
    backends,
    engine::{BackendFailure, Engine, EngineError, PackageReport},
    host::Authorization,
    package::{Package, PackageDetails, PackageId, Scope, UpdateAvailability},
    process::Cancellation,
};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Instant,
};

pub use icons::NAMES as ICON_NAMES;

/// Builds an engine for some sources (all of them when empty), as
/// `pkgdeck_core::backends::native_engine` does. Tests pass their own.
pub type MakeEngine =
    fn(&[String], bool, Authorization, &Cancellation) -> Result<Engine, EngineError>;

/// What the list worker sends: the sources it asks, rows so far, every
/// row, or why none came.
enum ListReply {
    Asked(Vec<String>),
    Partial(PackageReport),
    Done(PackageReport),
    Failed(String),
}

/// A job on its own thread. Dropping it cancels the job; a reply that still
/// arrives goes nowhere.
struct Worker<T> {
    receiver: mpsc::Receiver<T>,
    cancel: Cancellation,
}
impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
/// Run `job` on a thread; each reply it sends is read by the next poll,
/// and wakes the window if there is one.
fn spawn<T: Send + 'static>(
    wake: Option<egui::Context>,
    job: impl FnOnce(&Cancellation, &mut dyn FnMut(T)) + Send + 'static,
) -> Worker<T> {
    let (sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    let token = cancel.clone();
    thread::spawn(move || {
        job(&token, &mut |reply| {
            let _ = sender.send(reply);
            if let Some(ctx) = &wake {
                ctx.request_repaint();
            }
        })
    });
    Worker { receiver, cancel }
}

/// How the list is sorted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortBy {
    Name,
    Version,
    Summary,
}

/// The name people know a package by.
pub fn shown_name(package: &Package) -> &str {
    if package.display_name.trim().is_empty() {
        &package.id.name
    } else {
        &package.display_name
    }
}
/// "Flatpak, flathub, User": where a package comes from.
pub fn source_line(id: &PackageId) -> String {
    let scope = match &id.scope {
        Scope::System => "System".to_owned(),
        Scope::User { .. } => "User".to_owned(),
        Scope::Environment { path } => path.display().to_string(),
    };
    [
        backends::display_name(&id.backend),
        id.remote.as_deref().unwrap_or(""),
        &scope,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}
/// The version an update brings, or "" when it isn't known.
pub fn update_version(package: &Package) -> Option<&str> {
    (package.update == UpdateAvailability::Available).then(|| match &package.candidate_version {
        Some(version) if package.installed_version.as_ref() != Some(version) => version,
        _ => "",
    })
}
/// "Update 2.0 available" when there is one.
pub fn update_line(package: &Package) -> Option<String> {
    update_version(package).map(|version| match version {
        "" => "Update available".to_owned(),
        version => format!("Update {version} available"),
    })
}
/// "Snap couldn't be read: snapd isn't running": a source that failed,
/// and why, without repeating its name.
pub fn failure_line(failure: &BackendFailure) -> String {
    let reason = match &failure.error {
        EngineError::Unavailable { reason, .. } => reason.clone(),
        error => error.to_string(),
    };
    format!(
        "{} couldn't be read: {reason}",
        backends::display_name(&failure.backend)
    )
}
/// What a screen reader says for a row: everything the row shows.
pub fn row_label(package: &Package) -> String {
    [
        shown_name(package).to_owned(),
        source_line(&package.id),
        package.installed_version.clone().unwrap_or_default(),
        update_line(package).unwrap_or_default(),
        package.summary.clone(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}

/// The Installed page: every installed package, a filter, sortable
/// columns, and the open package's details below the list.
pub struct Installed {
    make_engine: MakeEngine,
    /// Wakes the window when a worker sends something.
    wake: Option<egui::Context>,
    packages: Vec<Package>,
    failures: Vec<BackendFailure>,
    /// Sources asked that haven't answered yet.
    waiting: Vec<String>,
    /// Why nothing could be read at all.
    error: Option<String>,
    list: Option<Worker<ListReply>>,
    filter: String,
    sort: SortBy,
    descending: bool,
    /// Indexes into `packages`, filtered and sorted; rebuilt when stale.
    order: Vec<usize>,
    stale: bool,
    /// The highlighted row, which the keyboard moves.
    cursor: Option<PackageId>,
    /// The package whose details are open.
    selected: Option<PackageId>,
    details: Option<Result<PackageDetails, String>>,
    details_worker: Option<Worker<(PackageId, Result<PackageDetails, String>)>>,
    dependencies_open: bool,
    /// Bring the cursor's row into view on the next frame.
    reveal: bool,
}

impl Installed {
    /// A page that starts reading what's installed right away.
    pub fn new(make_engine: MakeEngine, wake: Option<egui::Context>) -> Self {
        let mut page = Self {
            make_engine,
            wake,
            packages: vec![],
            failures: vec![],
            waiting: vec![],
            error: None,
            list: None,
            filter: String::new(),
            sort: SortBy::Name,
            descending: false,
            order: vec![],
            stale: true,
            cursor: None,
            selected: None,
            details: None,
            details_worker: None,
            dependencies_open: false,
            reveal: false,
        };
        page.reload();
        page
    }
    /// Read the installed packages again. Rows stream in as each source
    /// answers.
    pub fn reload(&mut self) {
        let make = self.make_engine;
        self.error = None;
        self.list = Some(spawn(self.wake.clone(), move |cancel, send| {
            let last =
                match make(&[], false, Authorization::Polkit, cancel) {
                    Ok(mut engine) => {
                        send(ListReply::Asked(engine.source_ids()));
                        ListReply::Done(engine.installed_stream(cancel, &mut |report| {
                            send(ListReply::Partial(report))
                        }))
                    }
                    Err(error) => ListReply::Failed(error.to_string()),
                };
            send(last);
        }));
    }
    pub fn loading(&self) -> bool {
        self.list.is_some()
    }
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }
    pub fn failures(&self) -> &[BackendFailure] {
        &self.failures
    }
    /// The sources still being read, by the names people know them by.
    pub fn waiting(&self) -> Vec<&str> {
        self.waiting
            .iter()
            .map(|id| backends::display_name(id))
            .collect()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn filter(&self) -> &str {
        &self.filter
    }
    pub fn cursor(&self) -> Option<&PackageId> {
        self.cursor.as_ref()
    }
    pub fn selected(&self) -> Option<&PackageId> {
        self.selected.as_ref()
    }
    /// The open package's details: none while they load, or why they
    /// couldn't be read.
    pub fn details(&self) -> Option<&Result<PackageDetails, String>> {
        self.details.as_ref()
    }
    /// Take in what the workers sent. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(worker) = &self.list {
            let replies: Vec<_> = worker.receiver.try_iter().collect();
            for reply in replies {
                changed = true;
                match reply {
                    ListReply::Asked(sources) => self.waiting = sources,
                    ListReply::Partial(report) => self.show_report(report, false),
                    ListReply::Done(report) => {
                        self.show_report(report, true);
                        self.waiting.clear();
                        self.list = None;
                    }
                    ListReply::Failed(error) => {
                        self.error = Some(error);
                        self.waiting.clear();
                        self.list = None;
                    }
                }
            }
        }
        if let Some(worker) = &self.details_worker {
            if let Ok((id, details)) = worker.receiver.try_recv() {
                changed = true;
                self.details_worker = None;
                if self.selected.as_ref() == Some(&id) {
                    self.details = Some(details);
                }
            }
        }
        changed
    }
    fn show_report(&mut self, mut report: PackageReport, complete: bool) {
        // A source has answered once it succeeded or failed.
        self.waiting.retain(|id| {
            !report.successful_sources.contains(id)
                && !report.failures.iter().any(|failure| failure.backend == *id)
        });
        // During a refresh, keep the last rows from sources that haven't
        // answered yet. Their open details stay usable while others stream in.
        if !complete {
            report.packages.extend(
                self.packages
                    .iter()
                    .filter(|package| {
                        !report.successful_sources.contains(&package.id.backend)
                            && !report
                                .failures
                                .iter()
                                .any(|failure| failure.backend == package.id.backend)
                    })
                    .cloned(),
            );
        }
        self.packages = report.packages;
        self.failures = report.failures;
        self.stale = true;
        // Rows that are gone take their highlight and details with them.
        let present = |id: &Option<PackageId>| {
            id.as_ref()
                .is_some_and(|id| self.packages.iter().any(|package| package.id == *id))
        };
        if !present(&self.cursor) {
            self.cursor = None;
        }
        if !present(&self.selected) && self.selected.is_some() {
            self.close_details();
        }
    }
    pub fn set_filter(&mut self, filter: &str) {
        if self.filter != filter {
            self.filter = filter.to_owned();
            self.stale = true;
        }
    }
    /// Sort by `column`; the same column again flips the direction.
    pub fn sort_by(&mut self, column: SortBy) {
        if self.sort == column {
            self.descending = !self.descending;
        } else {
            self.sort = column;
            self.descending = false;
        }
        self.stale = true;
    }
    pub fn sorting(&self) -> (SortBy, bool) {
        (self.sort, self.descending)
    }
    /// The packages shown, in order.
    pub fn visible(&mut self) -> Vec<&Package> {
        self.refresh_order();
        self.order.iter().map(|&i| &self.packages[i]).collect()
    }
    fn refresh_order(&mut self) {
        if !self.stale {
            return;
        }
        let needle = self.filter.trim().to_lowercase();
        let packages = &self.packages;
        let mut order: Vec<usize> = (0..packages.len())
            .filter(|&i| {
                let package = &packages[i];
                needle.is_empty()
                    || [shown_name(package), &package.id.name, &package.summary]
                        .iter()
                        .any(|text| text.to_lowercase().contains(&needle))
            })
            .collect();
        let key = |package: &Package| match self.sort {
            SortBy::Name => shown_name(package).to_lowercase(),
            SortBy::Version => package.installed_version.clone().unwrap_or_default(),
            SortBy::Summary => package.summary.to_lowercase(),
        };
        order.sort_by(|&a, &b| {
            let (a, b) = (&packages[a], &packages[b]);
            key(a)
                .cmp(&key(b))
                .then_with(|| {
                    shown_name(a)
                        .to_lowercase()
                        .cmp(&shown_name(b).to_lowercase())
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        if self.descending {
            order.reverse();
        }
        self.order = order;
        if self
            .cursor
            .as_ref()
            .is_some_and(|id| !self.order.iter().any(|&i| self.packages[i].id == *id))
        {
            self.cursor = None;
        }
        self.stale = false;
    }
    /// Move the highlight `by` rows (negative is up), staying in the list.
    /// With details open, they follow the highlight.
    pub fn move_cursor(&mut self, by: isize) {
        self.refresh_order();
        if self.order.is_empty() {
            return;
        }
        let at = self
            .cursor
            .as_ref()
            .and_then(|id| self.order.iter().position(|&i| self.packages[i].id == *id));
        let last = self.order.len() as isize - 1;
        // From nowhere, down starts at the top and up at the bottom.
        let next = match at {
            Some(at) => at as isize + by,
            None if by < 0 => last + 1 + by,
            None => by - 1,
        }
        .clamp(0, last) as usize;
        let id = self.packages[self.order[next]].id.clone();
        self.cursor = Some(id.clone());
        self.reveal = true;
        if self.selected.is_some() {
            self.open(&id);
        }
    }
    /// Open the highlighted row's details.
    pub fn open_cursor(&mut self) {
        if let Some(id) = self.cursor.clone() {
            self.open(&id);
        }
    }
    /// Open a package's details. They load on their own thread; the same
    /// package again changes nothing.
    pub fn open(&mut self, id: &PackageId) {
        self.cursor = Some(id.clone());
        if self.selected.as_ref() == Some(id) {
            return;
        }
        self.selected = Some(id.clone());
        self.details = None;
        self.dependencies_open = false;
        let make = self.make_engine;
        let id = id.clone();
        self.details_worker = Some(spawn(self.wake.clone(), move |cancel, send| {
            let details = make(
                std::slice::from_ref(&id.backend),
                false,
                Authorization::Polkit,
                cancel,
            )
            .and_then(|mut engine| engine.details(&id, cancel))
            .map_err(|error| error.to_string());
            send((id, details));
        }));
    }
    pub fn close_details(&mut self) {
        self.selected = None;
        self.details = None;
        self.details_worker = None;
    }
    /// Esc: close the details, else clear the filter.
    pub fn escape(&mut self) {
        if self.selected.is_some() {
            self.close_details();
        } else {
            self.set_filter("");
        }
    }
}

/// Image loaders, the system fonts under `root`, and PkgDeck's look.
pub fn setup(ctx: &egui::Context, root: &Path) {
    egui_extras::install_image_loaders(ctx);
    ctx.set_fonts(theme::fonts(root));
    theme::apply(ctx);
}

/// The window: the sidebar and the Installed page. With `smoke_test` it
/// reports its first drawn frame, then closes.
pub struct Window {
    pub page: Installed,
    smoke_test: bool,
    started: Instant,
    /// Fonts and looks to set up on the first frame, from this root.
    setup: Option<PathBuf>,
}
impl Window {
    pub fn new(ctx: &egui::Context, make_engine: MakeEngine, smoke_test: bool) -> Self {
        Self::with_fonts(ctx, make_engine, smoke_test, Path::new("/"))
    }
    /// The window with the system fonts found under `root`.
    pub fn with_fonts(
        ctx: &egui::Context,
        make_engine: MakeEngine,
        smoke_test: bool,
        root: &Path,
    ) -> Self {
        Self {
            page: Installed::new(make_engine, Some(ctx.clone())),
            smoke_test,
            started: Instant::now(),
            setup: Some(root.to_owned()),
        }
    }
    /// Draw one frame. Returns whether the window should close.
    pub fn frame(&mut self, ui: &mut egui::Ui) -> bool {
        // New fonts take effect on the next frame, so this one only sets
        // them up and asks for that frame.
        if let Some(root) = self.setup.take() {
            setup(ui.ctx(), &root);
            ui.ctx().request_repaint();
            return false;
        }
        view::window(ui, &mut self.page);
        if self.smoke_test {
            eprintln!(
                "PKGDECK_EGUI_READY after {} ms",
                self.started.elapsed().as_millis()
            );
        }
        self.smoke_test
    }
}
impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.frame(ui) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests;
