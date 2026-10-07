//! PkgDeck's Installed page drawn with egui: a trial of a GUI without Qt.
//! It reads the same engine the Qt app does, on worker threads, and keeps
//! everything it shows in plain Rust state so tests can check it directly.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use pkgdeck_core::{
    backends,
    engine::{BackendFailure, Engine, EngineError, PackageReport},
    host::Authorization,
    package::{Package, PackageDetails, PackageId, Scope, UpdateAvailability},
    process::Cancellation,
};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// Builds an engine for some sources (all of them when empty), as
/// `pkgdeck_core::backends::native_engine` does. Tests pass their own.
pub type MakeEngine =
    fn(&[String], bool, Authorization, &Cancellation) -> Result<Engine, EngineError>;

/// What the list worker sends: rows so far, every row, or why none came.
enum ListReply {
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
/// Run `job` on a thread; each reply it sends is read by the next poll.
fn spawn<T: Send + 'static>(
    job: impl FnOnce(&Cancellation, &mut dyn FnMut(T)) + Send + 'static,
) -> Worker<T> {
    let (sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    let token = cancel.clone();
    thread::spawn(move || {
        job(&token, &mut |reply| {
            let _ = sender.send(reply);
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
/// "Update 2.0 available" when there is one.
pub fn update_line(package: &Package) -> Option<String> {
    (package.update == UpdateAvailability::Available).then(|| match &package.candidate_version {
        Some(version) if package.installed_version.as_ref() != Some(version) => {
            format!("Update {version} available")
        }
        _ => "Update available".to_owned(),
    })
}

/// The Installed page: every installed package, a filter, sortable
/// columns, and the open package's details below the list.
pub struct Installed {
    make_engine: MakeEngine,
    packages: Vec<Package>,
    failures: Vec<BackendFailure>,
    /// Why nothing could be read at all.
    error: Option<String>,
    list: Option<Worker<ListReply>>,
    pub filter: String,
    sort: SortBy,
    descending: bool,
    /// Indexes into `packages`, filtered and sorted; rebuilt when stale.
    order: Vec<usize>,
    stale: bool,
    selected: Option<PackageId>,
    details: Option<Result<PackageDetails, String>>,
    details_worker: Option<Worker<(PackageId, Result<PackageDetails, String>)>>,
}

impl Installed {
    /// A page that starts reading what's installed right away.
    pub fn new(make_engine: MakeEngine) -> Self {
        let mut page = Self {
            make_engine,
            packages: vec![],
            failures: vec![],
            error: None,
            list: None,
            filter: String::new(),
            sort: SortBy::Name,
            descending: false,
            order: vec![],
            stale: true,
            selected: None,
            details: None,
            details_worker: None,
        };
        page.reload();
        page
    }
    /// Read the installed packages again. Rows stream in as each source
    /// answers.
    pub fn reload(&mut self) {
        let make = self.make_engine;
        self.error = None;
        self.list = Some(spawn(move |cancel, send| {
            let last = match make(&[], false, Authorization::Polkit, cancel) {
                Ok(mut engine) => ListReply::Done(
                    engine.installed_stream(cancel, &mut |report| send(ListReply::Partial(report))),
                ),
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
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
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
                    ListReply::Partial(report) => self.show_report(report),
                    ListReply::Done(report) => {
                        self.show_report(report);
                        self.list = None;
                    }
                    ListReply::Failed(error) => {
                        self.error = Some(error);
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
    fn show_report(&mut self, report: PackageReport) {
        self.packages = report.packages;
        self.failures = report.failures;
        self.stale = true;
        // The open package keeps its page only while it's still installed.
        if let Some(id) = &self.selected {
            if !self.packages.iter().any(|package| package.id == *id) {
                self.close_details();
            }
        }
    }
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_owned();
        self.stale = true;
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
        self.stale = false;
    }
    /// Open a package's details. They load on their own thread; the same
    /// package again changes nothing.
    pub fn open(&mut self, id: &PackageId) {
        if self.selected.as_ref() == Some(id) {
            return;
        }
        self.selected = Some(id.clone());
        self.details = None;
        let make = self.make_engine;
        let id = id.clone();
        self.details_worker = Some(spawn(move |cancel, send| {
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

    /// Draw the page. Workers are polled first, and while they run the
    /// window asks to be drawn again soon.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll();
        if self.list.is_some() || self.details_worker.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.close_details();
        }
        egui::Panel::top("installed_header").show(ui, |ui| self.header(ui));
        if self.selected.is_some() {
            egui::Panel::bottom("installed_details")
                .resizable(true)
                .default_size(ui.available_height() * 0.45)
                .size_range(120.0..=ui.available_height().max(120.0) - 80.0)
                .show(ui, |ui| self.details_ui(ui));
        }
        egui::CentralPanel::default_margins().show(ui, |ui| self.list_ui(ui));
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading(egui::RichText::new("Installed").strong().size(24.0));
            if self.list.is_some() {
                ui.add(egui::Spinner::new())
                    .on_hover_text("Reading installed packages");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.list.is_none(), egui::Button::new("Reload"))
                    .clicked()
                {
                    self.reload();
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let mut filter = self.filter.clone();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text("Filter installed packages")
                    .desired_width(320.0),
            );
            if edit.changed() {
                self.set_filter(&filter);
            }
            let shown = self.visible().len();
            let count = if shown == self.packages.len() {
                format!("{shown} packages")
            } else {
                format!("{shown} of {} packages", self.packages.len())
            };
            ui.label(egui::RichText::new(count).weak());
        });
        ui.add_space(8.0);
    }
    fn list_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Couldn't read installed packages. {error}"),
            );
            return;
        }
        for failure in &self.failures {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!(
                    "{} couldn't be read. {}",
                    backends::display_name(&failure.backend),
                    failure.error
                ),
            );
        }
        if self.packages.is_empty() {
            ui.label(if self.list.is_some() {
                "Reading installed packages…"
            } else {
                "Nothing installed was found."
            });
            return;
        }
        self.refresh_order();
        if self.order.is_empty() {
            ui.label(format!(
                "Nothing installed matches “{}”.",
                self.filter.trim()
            ));
            return;
        }
        let mut clicked = None;
        let mut sort = None;
        let row_height = ui.text_style_height(&egui::TextStyle::Body) * 2.0 + 14.0;
        // Rows open on a click, so their labels must not take it for
        // selecting text.
        ui.style_mut().interaction.selectable_labels = false;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            // Every cell starts at the top, so a row's first lines line up.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Min))
            .column(Column::initial(280.0).at_least(140.0).clip(true))
            .column(Column::initial(150.0).at_least(80.0).clip(true))
            .column(Column::remainder().at_least(120.0).clip(true))
            .header(28.0, |mut header| {
                for (column, title) in [
                    (SortBy::Name, "Name"),
                    (SortBy::Version, "Version"),
                    (SortBy::Summary, "Summary"),
                ] {
                    header.col(|ui| {
                        let arrow = match self.sorting() {
                            (current, false) if current == column => " ⏶",
                            (current, true) if current == column => " ⏷",
                            _ => "",
                        };
                        if ui.button(format!("{title}{arrow}")).clicked() {
                            sort = Some(column);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.order.len(), |mut row| {
                    let package = &self.packages[self.order[row.index()]];
                    row.set_selected(self.selected.as_ref() == Some(&package.id));
                    row.col(|ui| {
                        if let Some(icon) = &package.icon {
                            ui.add(
                                egui::Image::new(format!("file://{}", icon.display()))
                                    .fit_to_exact_size(egui::vec2(28.0, 28.0)),
                            );
                        }
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(shown_name(package)).strong());
                            ui.label(egui::RichText::new(source_line(&package.id)).small().weak());
                        });
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.label(package.installed_version.as_deref().unwrap_or(""));
                            if let Some(update) = update_line(package) {
                                ui.label(
                                    egui::RichText::new(update)
                                        .small()
                                        .color(ui.visuals().hyperlink_color),
                                );
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(&package.summary);
                    });
                    if row.response().clicked() {
                        clicked = Some(package.id.clone());
                    }
                });
            });
        if let Some(column) = sort {
            self.sort_by(column);
        }
        if let Some(id) = clicked {
            self.open(&id);
        }
    }
    fn details_ui(&mut self, ui: &mut egui::Ui) {
        let Some(package) = self
            .selected
            .as_ref()
            .and_then(|id| self.packages.iter().find(|package| package.id == *id))
            .cloned()
        else {
            return;
        };
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if let Some(icon) = &package.icon {
                ui.add(
                    egui::Image::new(format!("file://{}", icon.display()))
                        .fit_to_exact_size(egui::vec2(48.0, 48.0)),
                );
            }
            ui.vertical(|ui| {
                ui.heading(egui::RichText::new(shown_name(&package)).strong());
                ui.label(egui::RichText::new(source_line(&package.id)).weak());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    self.close_details();
                }
            });
        });
        if self.selected.is_none() {
            return;
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                if let Some(version) = &package.installed_version {
                    ui.label(format!("Version {version}"));
                }
                if let Some(update) = update_line(&package) {
                    ui.colored_label(ui.visuals().hyperlink_color, update);
                }
                ui.add_space(6.0);
                match &self.details {
                    None => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(&package.summary);
                        });
                    }
                    Some(Err(error)) => {
                        ui.label(&package.summary);
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("Details couldn't be loaded. {error}"),
                        );
                    }
                    Some(Ok(details)) => {
                        ui.label(if details.description.trim().is_empty() {
                            &package.summary
                        } else {
                            &details.description
                        });
                        if let Some(homepage) = &details.homepage {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Homepage").weak());
                                ui.hyperlink(homepage);
                            });
                        }
                        if !details.dependencies.is_empty() {
                            ui.add_space(6.0);
                            egui::CollapsingHeader::new(format!(
                                "Dependencies ({})",
                                details.dependencies.len()
                            ))
                            .default_open(false)
                            .show(ui, |ui| {
                                for dependency in &details.dependencies {
                                    ui.label(dependency);
                                }
                            });
                        }
                    }
                }
            });
    }
}

/// PkgDeck's colours (see the Qt app's Theme.qml) on egui's visuals.
pub fn apply_theme(ctx: &egui::Context) {
    for (theme, accent, canvas, surface) in [
        (
            egui::Theme::Light,
            egui::Color32::from_rgb(0x2f, 0x68, 0xd8),
            egui::Color32::from_rgb(0xf4, 0xf6, 0xf9),
            egui::Color32::WHITE,
        ),
        (
            egui::Theme::Dark,
            egui::Color32::from_rgb(0x7a, 0xa7, 0xff),
            egui::Color32::from_rgb(0x0e, 0x10, 0x15),
            egui::Color32::from_rgb(0x16, 0x1a, 0x21),
        ),
    ] {
        ctx.style_mut_of(theme, |style| {
            let visuals = &mut style.visuals;
            visuals.hyperlink_color = accent;
            visuals.selection.bg_fill = accent.gamma_multiply(0.25);
            visuals.selection.stroke.color = accent;
            visuals.panel_fill = canvas;
            visuals.window_fill = surface;
            visuals.extreme_bg_color = surface;
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
                &mut visuals.widgets.open,
            ] {
                widget.corner_radius = egui::CornerRadius::same(9);
            }
            style.spacing.button_padding = egui::vec2(12.0, 6.0);
        });
    }
}

/// The window: the Installed page, and with `smoke_test` a single frame
/// that reports it drew, then closes.
pub struct Window {
    pub page: Installed,
    smoke_test: bool,
    started: Instant,
}
impl Window {
    pub fn new(ctx: &egui::Context, make_engine: MakeEngine, smoke_test: bool) -> Self {
        egui_extras::install_image_loaders(ctx);
        apply_theme(ctx);
        Self {
            page: Installed::new(make_engine),
            smoke_test,
            started: Instant::now(),
        }
    }
    /// Draw one frame. Returns whether the window should close.
    pub fn frame(&mut self, ui: &mut egui::Ui) -> bool {
        self.page.ui(ui);
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
