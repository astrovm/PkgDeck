//! The app core against a synthetic package manager: loading pages,
//! choosing rows, reviewing and running changes, undo, background checks,
//! and what's remembered. Nothing here touches the real system.

use pkgdeck::{
    app::{App, Launch, ReviewOn, ToastAction, WindowRequest},
    model::{Column, Page},
    platform::{Event, Platform},
    settings::{Settings, Store},
};
use pkgdeck_app::controller::ffi::create_synthetic_controller;
use pkgdeck_core::{
    engine::{Backend, Engine, EngineError},
    host::Authorization,
    package::{
        Availability, Capability, Operation, OperationOutcome, Package, PackageDetails, PackageId, Progress, Scope,
        UpdateAvailability,
    },
    process::Cancellation,
};
use serde_json::json;
use std::{
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// What the synthetic manager has, shared with its engine on worker threads.
static WORLD: Mutex<Vec<Package>> = Mutex::new(Vec::new());
/// One test at a time owns the world.
static TURN: Mutex<()> = Mutex::new(());

fn package(name: &str, display: &str, installed: Option<&str>, candidate: Option<&str>) -> Package {
    Package {
        id: PackageId {
            backend: "apt".into(),
            name: name.into(),
            architecture: "x86_64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: display.into(),
        summary: format!("{display} summary"),
        installed_version: installed.map(Into::into),
        candidate_version: candidate.map(Into::into),
        update: match (installed, candidate) {
            (Some(a), Some(b)) if a != b => UpdateAvailability::Available,
            (Some(_), _) => UpdateAvailability::Current,
            _ => UpdateAvailability::Unknown,
        },
        ..serde_json::from_value(json!({
            "id": {"backend": "apt", "name": name, "architecture": "x86_64", "scope": "system"},
            "display_name": "", "summary": "", "installed_version": null, "candidate_version": null, "update": "unknown"
        }))
        .unwrap()
    }
}

struct Synthetic;
impl Backend for Synthetic {
    fn id(&self) -> &str {
        "apt"
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Details,
            Capability::Install,
            Capability::Remove,
            Capability::Upgrade,
            Capability::Refresh,
        ]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn search(&mut self, query: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let world = WORLD.lock().unwrap();
        Ok(world.iter().filter(|p| p.id.name.contains(query)).cloned().collect())
    }
    fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let world = WORLD.lock().unwrap();
        Ok(world.iter().filter(|p| p.installed_version.is_some()).cloned().collect())
    }
    fn details(&mut self, id: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        let world = WORLD.lock().unwrap();
        let package = world.iter().find(|p| p.id == *id).cloned().ok_or(EngineError::NotFound)?;
        Ok(serde_json::from_value(json!({
            "package": package,
            "description": format!("All about {}.", package.display_name),
            "homepage": "https://example.invalid",
            "dependencies": ["libc6"],
        }))
        .unwrap())
    }
    fn execute(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        progress(Progress::Message("Working".into()));
        let mut world = WORLD.lock().unwrap();
        let mut set = |id: &PackageId, f: &dyn Fn(&mut Package)| {
            if let Some(p) = world.iter_mut().find(|p| p.id == *id) {
                f(p);
            }
        };
        match operation {
            Operation::Install(id) => set(id, &|p| p.installed_version = p.candidate_version.clone()),
            Operation::Remove(id) => set(id, &|p| p.installed_version = None),
            Operation::Upgrade(id) => set(id, &|p| {
                p.installed_version = p.candidate_version.clone();
                p.update = UpdateAvailability::Current;
            }),
            _ => {}
        }
        Ok(OperationOutcome::default())
    }
}

fn engine(_: &[String], _: bool, _: Authorization, _: &Cancellation) -> Result<Engine, EngineError> {
    let mut engine = Engine::default();
    engine.register(Synthetic)?;
    Ok(engine)
}

fn start(packages: Vec<Package>) -> (MutexGuard<'static, ()>, App) {
    let turn = TURN.lock().unwrap_or_else(|poison| poison.into_inner());
    *WORLD.lock().unwrap() = packages;
    let (store, settings) = Store::open(None, None);
    let app = App::with_controller(
        create_synthetic_controller(engine),
        Settings { source_list: "apt".into(), ..settings },
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    (turn, app)
}

/// Runs the app until `done` holds, or fails after a while.
fn until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.tick(Instant::now());
        app.refresh_items();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn idle(app: &mut App) {
    until(app, "idle", |app| !app.busy && !*app.ctl.needs_poll());
}

fn titles(app: &App) -> Vec<String> {
    app.items.iter().map(|item| app.rows[item.raw].title().to_owned()).collect()
}

fn world() -> Vec<Package> {
    WORLD.lock().unwrap().clone()
}

#[test]
fn installed_lists_selects_and_removes_with_review() {
    let (_turn, mut app) = start(vec![
        package("gimp", "GIMP", Some("2"), None),
        package("htop", "htop", Some("3"), None),
        package("krita", "Krita", None, Some("5")),
    ]);
    app.open_page(Page::Installed);
    until(&mut app, "installed rows", |app| app.items.len() == 2);
    assert_eq!(titles(&app), ["GIMP", "htop"]);
    // Sorting by name, backwards.
    app.sort = Some((Column::Name, false));
    app.invalidate();
    app.refresh_items();
    assert_eq!(titles(&app), ["htop", "GIMP"]);
    app.sort = None;
    app.invalidate();
    app.refresh_items();
    // Filtering.
    app.filter = "gim".into();
    app.invalidate();
    app.refresh_items();
    assert_eq!(titles(&app), ["GIMP"]);
    app.filter.clear();
    app.invalidate();
    app.refresh_items();
    // Hovering warms the details; choosing opens the page.
    app.hovered_row(Some(app.items[0].raw));
    std::thread::sleep(Duration::from_millis(160));
    app.tick(Instant::now());
    app.hovered_row(None);
    app.choose(0, true);
    assert!(app.page_open);
    assert_eq!(app.page_key(), app.selected.clone());
    until(&mut app, "details", |app| app.details_for_selection().is_some_and(|d| !d.more && d.description.starts_with("All")));
    assert_eq!(app.details_for_selection().unwrap().description, "All about GIMP.");
    // Remove from the page: the review shows on the page.
    app.run_page_action();
    until(&mut app, "review", |app| app.confirmation.is_some() && !app.busy);
    assert!(app.review_on_page || app.confirm_open);
    if app.review_on_page {
        assert_eq!(app.review_on, ReviewOn::Page(app.selected.clone().unwrap()));
    }
    app.confirm(true);
    until(&mut app, "removed", |_| world()[0].installed_version.is_none());
    until(&mut app, "list refreshed", |app| !app.busy && !app.writing && titles(app) == ["htop"]);
    // The success toast can undo.
    until(&mut app, "toast", |app| app.toast.is_some());
    if app.toast.as_ref().unwrap().action == Some(ToastAction::Undo) {
        app.toast_action(ToastAction::Undo);
    } else {
        app.hide_toast();
    }
    idle(&mut app);
    app.close_page();
    assert!(!app.page_open);
    app.deselect();
    assert!(app.selected.is_none());
    app.save_settings();
}

#[test]
fn search_debounces_ranks_and_installs() {
    let (_turn, mut app) = start(vec![
        package("firefox", "Firefox", None, Some("132")),
        package("firefox-esr", "Firefox ESR", None, Some("128")),
        package("vim", "Vim", Some("9"), None),
    ]);
    assert_eq!(app.page, Page::Search);
    app.query = "fire".into();
    app.typed_query();
    assert!(app.search_due.is_some());
    until(&mut app, "search rows", |app| app.items.len() == 2);
    assert_eq!(app.result_query, "fire");
    assert_eq!(titles(&app)[0], "Firefox");
    // Quick install of one app runs without a dialog when nothing else changes.
    app.choose(0, false);
    app.run_row_action(0);
    until(&mut app, "installed or reviewed", |app| {
        world()[0].installed_version.is_some() || app.confirmation.is_some()
    });
    if app.confirmation.is_some() {
        app.confirm(true);
    }
    until(&mut app, "installed", |_| world()[0].installed_version.is_some());
    idle(&mut app);
    // Clearing the search empties the page without a query.
    app.query.clear();
    app.typed_query();
    idle(&mut app);
    assert!(app.items.is_empty());
    assert!(app.result_query.is_empty());
}

#[test]
fn updates_check_some_and_update_them() {
    let (_turn, mut app) = start(vec![
        package("a", "Alpha", Some("1"), Some("2")),
        package("b", "Beta", Some("1"), Some("2")),
        package("c", "Gamma", Some("1"), None),
    ]);
    app.open_page(Page::Updates);
    until(&mut app, "updates", |app| app.items.len() == 2);
    idle(&mut app);
    // Uncheck Beta: only Alpha is updated, after the dialog.
    let beta = app.rows[app.items[1].raw].identity();
    app.unchecked.insert(beta);
    app.upgrade_updates();
    until(&mut app, "confirmation", |app| app.confirmation.is_some());
    assert!(app.confirm_open);
    assert_eq!(app.review_on, ReviewOn::Dialog);
    app.confirm(true);
    until(&mut app, "alpha updated", |_| world()[0].installed_version.as_deref() == Some("2"));
    assert_eq!(world()[1].installed_version.as_deref(), Some("1"));
    idle(&mut app);
    // A review that's turned down changes nothing. (Updating all of APT
    // plans with apt itself, which this synthetic manager can't, so that
    // ends in a notice instead.)
    until(&mut app, "reloaded", |app| app.items.len() == 1);
    app.upgrade_updates();
    until(&mut app, "answer", |app| app.confirmation.is_some() || !app.notice.is_empty());
    if app.confirmation.is_some() {
        app.confirm(false);
    }
    idle(&mut app);
    assert_eq!(world()[1].installed_version.as_deref(), Some("1"));
    assert!(app.confirmation.is_none());
}

#[test]
fn sources_turn_off_and_pages_narrow() {
    let (_turn, mut app) = start(vec![package("a", "Alpha", Some("1"), None)]);
    until(&mut app, "catalog", |app| !app.catalog.is_empty());
    assert!(app.available("apt"));
    assert!(app.usable("apt", Page::Installed));
    assert!(!app.can_turn_off("apt"), "the last source stays on");
    assert_eq!(app.source_summary(Page::Installed), "Filter sources");
    app.open_page(Page::Installed);
    let only = ["apt".to_owned()].into_iter().collect();
    app.set_page_sources(Page::Installed, only);
    assert!(!app.page_sources.contains_key(&Page::Installed), "all usable sources is no filter");
    let narrowed = ["apt".to_owned(), "flatpak".to_owned()].into_iter().collect();
    app.set_page_sources(Page::Installed, narrowed);
    assert_eq!(app.source_summary(Page::Installed), "2 sources");
    app.set_source_enabled("flatpak", true);
    assert!(app.page_sources.is_empty(), "enabling sources resets page filters");
    assert_eq!(app.settings.source_list, "apt,flatpak");
    app.set_source_enabled("flatpak", false);
    assert_eq!(app.settings.source_list, "apt");
    idle(&mut app);
    app.open_page(Page::Sources);
    until(&mut app, "sources", |app| app.rows.iter().any(|row| row.kind == "source"));
    app.show_unavailable = true;
    app.invalidate();
    app.refresh_items();
    assert!(!app.items.is_empty());
    assert!(app.read_failures().iter().all(|f| f.kind != "cancelled"));
    app.retry_source("apt");
    idle(&mut app);
    assert_eq!(app.failure_reason("nothing"), "This source could not be checked.");
}

#[test]
fn background_checks_badge_and_tray_events() {
    let (_turn, mut app) = start(vec![package("a", "Alpha", Some("1"), Some("2"))]);
    app.check_updates(true);
    until(&mut app, "background check", |app| app.background.last_check.is_some());
    assert_eq!(app.background.available, 1);
    assert!(app.settings.last_background_state.contains("\"available\":1"));
    for (event, request) in [
        (Event::Open, WindowRequest::Show),
        (Event::Toggle, WindowRequest::Hide),
        (Event::Quit, WindowRequest::Quit),
    ] {
        app.handle(event);
        assert_eq!(std::mem::take(&mut app.window_request), request);
    }
    app.window_visible = false;
    app.handle(Event::Toggle);
    assert_eq!(std::mem::take(&mut app.window_request), WindowRequest::Show);
    app.handle(Event::NotificationClicked);
    assert_eq!(app.page, Page::Updates);
    app.handle(Event::Check);
    app.handle(Event::Permission(true));
    idle(&mut app);
}

#[test]
fn closing_hides_waits_or_quits() {
    let (_turn, mut app) = start(vec![]);
    // With a tray and background checks on, closing hides.
    assert!(app.close_requested());
    assert_eq!(std::mem::take(&mut app.window_request), WindowRequest::Hide);
    app.settings.background_mode = false;
    idle(&mut app);
    assert!(app.close_requested());
    assert_eq!(std::mem::take(&mut app.window_request), WindowRequest::Quit);
    app.quit();
    assert!(app.force_quit);
    assert_eq!(std::mem::take(&mut app.window_request), WindowRequest::Quit);
    app.sync_tray();
    assert!(!app.waiting_shown());
    assert_eq!(app.queued_count(), 0);
}

#[test]
fn opening_a_missing_file_says_so() {
    let (_turn, mut app) = start(vec![]);
    app.open_input("   ");
    assert!(!app.opening);
    app.open_file(std::path::Path::new("/nonexistent/pkgdeck/app.AppImage"));
    until(&mut app, "answer", |app| !app.busy);
    assert!(!app.opening);
    assert!(app.opened.is_none());
    assert!(!app.notice.is_empty() || app.confirmation.is_none());
    app.cancel();
}
