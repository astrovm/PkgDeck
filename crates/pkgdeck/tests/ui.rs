//! The window, drawn headless and used the way a person would: through
//! its accessibility tree, by name. The core behind it is a real
//! controller with no package managers; rows are set by hand.

use eframe::egui::{self, accesskit::Role};
use egui_kittest::{kittest::Queryable, Harness};
use pkgdeck::{
    app::{App, Launch},
    model::Page,
    platform::Platform,
    settings::{Appearance, Store},
};
use pkgdeck_app::controller::ffi::create_synthetic_controller;
use serde_json::json;

fn app() -> App {
    let (store, settings) = Store::open(None, None);
    let mut platform = Platform::start(|| {});
    platform.record_notifications();
    let mut app = App::with_controller(
        create_synthetic_controller(|_, _, _, _| Ok(pkgdeck_core::engine::Engine::default())),
        settings,
        store,
        platform,
        Launch::default(),
    );
    app.settings.reduce_motion = true;
    app.settings.appearance = Appearance::Dark;
    let row = |name: &str, display: &str, installed: Option<&str>, candidate: Option<&str>| {
        json!({
            "kind": "package", "name": name, "display_name": display, "source": "apt",
            "architecture": "x86_64", "scope": "system", "scope_label": "System",
            "installed": installed, "candidate": candidate,
            "update": if installed.is_some() && candidate.is_some() { "available" } else { "current" },
            "summary": format!("{display} summary"),
        })
    };
    let rows = json!([
        row("gimp", "GIMP", Some("2.10"), None),
        row("htop", "htop", Some("3.3"), Some("3.4")),
        row("krita", "Krita", None, Some("5.2")),
    ]);
    app.c().set_rows(rows.to_string().into());
    app.react();
    app
}

fn harness(app: App) -> Harness<'static, (bool, App)> {
    Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(
            |ui, (ready, app): &mut (bool, App)| {
                if !*ready {
                    pkgdeck::setup(ui.ctx(), std::path::Path::new("/"), None);
                    *ready = true;
                    return;
                }
                pkgdeck::ui::show(app, ui);
            },
            (false, app),
        )
}

fn show(page: Page) -> Harness<'static, (bool, App)> {
    let mut app = app();
    app.page = page;
    app.invalidate();
    let mut harness = harness(app);
    harness.run_steps(4);
    harness
}

#[test]
fn the_sidebar_switches_pages() {
    let mut harness = show(Page::Installed);
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run_steps(3);
    assert_eq!(harness.state().1.page, Page::Settings);
    harness
        .get_by_role_and_label(Role::Button, "Updates")
        .click();
    harness.run_steps(3);
    assert_eq!(harness.state().1.page, Page::Updates);
}

#[test]
fn a_row_opens_its_page_and_escape_closes_it() {
    let mut harness = show(Page::Installed);
    harness.get_by_label_contains("GIMP, APT, System").click();
    harness.run_steps(4);
    assert!(harness.state().1.page_open);
    assert_eq!(harness.state().1.selected_row().unwrap().name, "gimp");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(!harness.state().1.page_open);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.state().1.selected.is_none());
}

#[test]
fn updates_rows_can_be_unchecked() {
    let mut harness = show(Page::Updates);
    assert!(harness.query_by_label("Update all").is_some());
    harness.get_by_label("Select none").click();
    harness.run_steps(3);
    assert!(!harness.state().1.unchecked.is_empty());
    assert!(harness.query_by_label("Update all").is_none());
    harness.get_by_label("Select all").click();
    harness.run_steps(3);
    assert!(harness.state().1.unchecked.is_empty());
}

#[test]
fn the_installed_filter_narrows_the_list() {
    let mut harness = show(Page::Installed);
    harness.state_mut().1.filter = "gim".into();
    harness.state_mut().1.invalidate();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("GIMP, APT").is_some());
    assert!(harness.query_by_label_contains("htop, APT").is_none());
}

#[test]
fn a_confirmation_asks_and_cancel_turns_it_down() {
    let mut app = app();
    app.page = Page::Installed;
    app.invalidate();
    app.c().set_confirmation_data(json!({"action": "Remove GIMP", "summary": "Remove GIMP\nFrom APT", "details": "gimp gimp-data"}).to_string().into());
    app.c().set_confirmation("Remove GIMP?".into());
    app.react();
    assert!(app.confirm_open);
    let mut harness = harness(app);
    harness.run_steps(4);
    harness.get_by_label("Show details").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("Hide details").is_some());
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert!(!harness.state().1.confirm_open);
    assert!(harness.state().1.confirmation.is_none());
}

#[test]
fn settings_switches_change_settings() {
    let mut harness = show(Page::Settings);
    let before = harness.state().1.settings.auto_update;
    harness
        .get_by_role_and_label(Role::CheckBox, "Install updates automatically")
        .click();
    harness.run_steps(3);
    assert_ne!(harness.state().1.settings.auto_update, before);
    harness.get_by_label("Light").click();
    harness.run_steps(3);
    assert_eq!(harness.state().1.settings.appearance, Appearance::Light);
    harness
        .get_by_role_and_label(Role::CheckBox, "Animations")
        .click();
    harness.run_steps(3);
    assert!(!harness.state().1.settings.reduce_motion);
}

#[test]
fn the_activity_drawer_opens_and_closes() {
    let mut harness = show(Page::Installed);
    harness.get_by_label("Activity").click();
    harness.run_steps(3);
    assert!(harness.state().1.drawer_open);
    assert!(
        harness.query_by_label("Nothing has run yet").is_some()
            || harness.state().1.activity.is_empty()
    );
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(!harness.state().1.drawer_open);
}

#[test]
fn narrow_windows_still_show_rows_and_their_page() {
    let mut app = app();
    app.page = Page::Installed;
    app.invalidate();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(480.0, 760.0))
        .build_ui_state(
            |ui, (ready, app): &mut (bool, App)| {
                if !*ready {
                    pkgdeck::setup(ui.ctx(), std::path::Path::new("/"), None);
                    *ready = true;
                    return;
                }
                pkgdeck::ui::show(app, ui);
            },
            (false, app),
        );
    harness.run_steps(4);
    harness.get_by_label_contains("Krita, APT").click();
    harness.run_steps(4);
    assert!(harness.state().1.page_open);
    harness.get_by_label("Back").click();
    harness.run_steps(4);
    assert!(!harness.state().1.page_open);
}

#[test]
fn rows_replaced_mid_frame_do_not_crash_the_list() {
    let mut harness = show(Page::Installed);
    assert!(!harness.state().1.items.is_empty());
    // A key or callback can swap the rows after this frame built its items.
    harness.state_mut().1.rows.clear();
    harness.run_steps(2);
    harness.state_mut().1.invalidate();
    harness.run_steps(2);
    assert!(harness.state().1.items.is_empty());
}

#[test]
fn arrow_keys_on_an_empty_list_do_nothing() {
    let mut harness = show(Page::Installed);
    harness.state_mut().1.ui.focus_list = true;
    harness.run_steps(2);
    harness.state_mut().1.filter = "nothing matches this".into();
    harness.state_mut().1.invalidate();
    harness.run_steps(2);
    for key in [egui::Key::ArrowDown, egui::Key::End, egui::Key::PageDown] {
        harness.key_press(key);
        harness.run_steps(2);
    }
    assert!(harness.state().1.items.is_empty());
}

#[test]
fn a_list_that_loads_late_does_not_steal_typing_from_search() {
    let mut harness = show(Page::Updates);
    let rows = harness.state().1.rows.clone();
    // Updates is still loading, so its list isn't drawn to take focus yet.
    harness.state_mut().1.rows.clear();
    harness.state_mut().1.invalidate();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num3);
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num1);
    harness.run_steps(2);
    harness.event(egui::Event::Text("fi".into()));
    harness.run_steps(2);
    // Search results arrive and the list draws.
    harness.state_mut().1.rows = rows;
    harness.state_mut().1.invalidate();
    harness.run_steps(3);
    harness.event(egui::Event::Text("re".into()));
    harness.run_steps(2);
    assert_eq!(harness.state().1.query, "fire");
    assert!(harness
        .ctx
        .memory(|m| m.has_focus(pkgdeck::ui::search_id())));
}

#[test]
fn rows_work_while_they_animate_in() {
    let mut app = app();
    app.settings.reduce_motion = false;
    app.page = Page::Installed;
    app.invalidate();
    let mut harness = harness(app);
    // Mid-entrance: rows are still rising into place.
    harness.run_steps(2);
    harness.get_by_label_contains("GIMP, APT").click();
    harness.run_steps(2);
    assert_eq!(harness.state().1.selected_row().unwrap().name, "gimp");
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(30);
    assert_eq!(harness.state().1.selected_row().unwrap().name, "htop");
}
