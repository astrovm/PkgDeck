//! The window, drawn headless and used the way a person would: through
//! its accessibility tree, by name. The core behind it is a real
//! controller with no package managers; rows are set by hand.

use eframe::egui::{self, accesskit::Role};
use egui_kittest::{
    kittest::{NodeT, Queryable},
    Harness,
};
use pkgdeck::{
    app::{App, Launch, Toast, ToastAction},
    model::Page,
    platform::Platform,
    settings::{Accent, Appearance, DarkTheme, LightTheme, Store, TextSize},
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
    harness_at(app, egui::vec2(1200.0, 800.0))
}

fn harness_at(app: App, size: egui::Vec2) -> Harness<'static, (bool, App)> {
    Harness::builder().with_size(size).build_ui_state(
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
fn the_restart_toast_offers_restart_and_closes() {
    let mut harness = show(Page::Installed);
    harness.state_mut().1.restart_toast = Some(Toast {
        text: "PkgDeck was updated. Restart it to use the new version.".into(),
        tone: pkgdeck::model::Tone::Success,
        action: Some(ToastAction::Restart),
        shown: std::time::Instant::now(),
        lasts: std::time::Duration::from_secs(60),
        from_notice: false,
    });
    harness.run_steps(3);
    assert!(harness.query_by_label("Restart").is_some());
    harness.get_by_label("Close").click();
    harness.run_steps(3);
    assert!(harness.state().1.restart_toast.is_none());
}

#[test]
fn an_update_can_start_from_the_installed_page() {
    let mut harness = show(Page::Installed);
    harness.get_by_label_contains("htop, APT, System").click();
    harness.run_steps(4);
    assert!(harness
        .query_by_role_and_label(Role::Button, "Remove")
        .is_some());
    harness
        .get_by_role_and_label(Role::Button, "Update")
        .click();
    harness.run_steps(3);
    let app = &harness.state().1;
    let htop = app.selected_row().unwrap().identity();
    assert!(app.active_rows.contains(&htop));

    // Only a running change hides it, not one waiting for review.
    assert!(harness
        .query_by_role_and_label(Role::Button, "Update")
        .is_some());
    // While it runs, the page offers Cancel, not a second Update.
    harness.state_mut().1.writing = true;
    harness.state_mut().1.active_rows.insert(htop);
    harness.run_steps(3);
    assert!(harness
        .query_by_role_and_label(Role::Button, "Cancel")
        .is_some());
    assert!(harness
        .query_by_role_and_label(Role::Button, "Update")
        .is_none());

    // Without an update waiting, there is nothing to update.
    let mut harness = show(Page::Installed);
    harness.get_by_label_contains("GIMP, APT, System").click();
    harness.run_steps(4);
    assert!(harness
        .query_by_role_and_label(Role::Button, "Remove")
        .is_some());
    assert!(harness
        .query_by_role_and_label(Role::Button, "Update")
        .is_none());
}

#[test]
fn dragging_a_column_divider_resizes_and_double_click_resets() {
    let mut harness = show(Page::Installed);
    let divider = harness.get_by_label("Resize Name column").rect().center();
    assert!(harness.query_by_label("Resize Version column").is_some());
    harness.hover_at(divider);
    harness.run_steps(1);
    harness.drag_at(divider);
    harness.run_steps(1);
    for step in 1..=4 {
        harness.hover_at(divider + egui::vec2(-20.0 * step as f32, 0.0));
        harness.run_steps(1);
    }
    harness.drop_at(divider + egui::vec2(-80.0, 0.0));
    harness.run_steps(2);
    let narrower = harness.state().1.settings.name_column;
    assert!(narrower > 0.0, "{narrower}");
    let moved = harness.get_by_label("Resize Name column").rect().center();
    assert!(
        (moved.x - (divider.x - 80.0)).abs() < 2.0,
        "{moved:?} {divider:?}"
    );
    assert_eq!(harness.state().1.settings.version_column, 0.0);

    // Two quick presses on the divider.
    for _ in 0..2 {
        harness.hover_at(moved);
        harness.drag_at(moved);
        harness.step();
        harness.event(egui::Event::PointerButton {
            pos: moved,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_steps(1);
    assert_eq!(harness.state().1.settings.name_column, 0.0);
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
fn updates_that_failed_or_need_the_password_are_marked() {
    let mut app = app();
    app.page = Page::Updates;
    app.c().set_held_updates(
        json!([
            {"reason": "failed", "source": "apt", "name": "htop",
                "identity": ["apt", "htop", "x86_64", null, "system", null]},
            {"reason": "password", "source": "homebrew-cask", "name": "htop"},
        ])
        .to_string()
        .into(),
    );
    app.react();
    app.invalidate();
    let mut harness = harness(app);
    harness.run_steps(4);
    assert!(harness
        .query_by_label_contains("Didn't update last time. Update all skips it")
        .is_some());
    // The cask mark belongs to the cask of that name, not this APT row.
    assert!(harness
        .query_by_label_contains("Needs your password")
        .is_none());
    // A row that isn't held says nothing extra.
    // Only the Updates page marks them.
    harness.state_mut().1.page = Page::Installed;
    harness.state_mut().1.invalidate();
    harness.run_steps(4);
    assert!(harness.query_by_label_contains("htop, APT").is_some());
    assert!(harness.query_by_label_contains("last time").is_none());
    harness.state_mut().1.page = Page::Updates;
    harness.state_mut().1.invalidate();
    harness.run_steps(4);
    assert!(harness.query_by_label_contains("last time").is_some());
    harness.state_mut().1.c().set_held_updates("[]".into());
    harness.state_mut().1.react();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("last time").is_none());
}

#[test]
fn the_installed_filter_narrows_the_list() {
    let mut harness = show(Page::Installed);
    // The filter box is the page's only text field.
    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(1);
    harness.get_by_role(Role::TextInput).type_text("gim");
    harness.run_steps(3);
    assert_eq!(harness.state().1.filter, "gim");
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
    // Only the colours of the mode on screen are offered.
    assert!(harness.query_by_label("Black").is_none());
    for choice in ["Paper", "Dark", "Black", "Teal", "Large"] {
        harness.get_by_label(choice).click();
        harness.run_steps(3);
    }
    let settings = &harness.state().1.settings;
    assert_eq!(settings.dark_theme, DarkTheme::Black);
    assert_eq!(settings.light_theme, LightTheme::Paper);
    assert_eq!(settings.accent, Accent::Teal);
    assert_eq!(settings.text_size, TextSize::Large);
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
fn a_new_page_opens_no_row() {
    let mut app = app();
    let task = json!({"kind": "cleanup", "name": "cache", "display_name": "APT download cache",
                      "source": "apt", "cleanup_kind": "package_cache", "summary": "12 cached downloads"});
    app.c().set_rows(json!([task]).to_string().into());
    app.react();
    app.page = Page::Clean;
    app.invalidate();
    let mut harness = harness(app);
    harness.run_steps(4);
    // Opening a page focuses its list, as the sidebar does.
    harness.state_mut().1.ui.focus_list = true;
    harness.run_steps(3);
    assert!(!harness.state().1.items.is_empty());
    assert!(harness.state().1.selected.is_none());
    assert!(harness.query_by_label("Close details").is_none());
}

#[test]
fn mid_width_windows_keep_the_list_beside_the_page() {
    let mut app = app();
    app.page = Page::Installed;
    app.invalidate();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(760.0, 760.0))
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
    harness.get_by_label_contains("GIMP, APT, System").click();
    harness.run_steps(6);
    assert!(harness.state().1.page_open);
    assert!(harness.query_by_label_contains("Krita, APT").is_some());
    assert!(harness.query_by_label("Back").is_none());
}

fn list_and_page_cards(harness: &Harness<'_, (bool, App)>) -> Vec<egui::Rect> {
    harness
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(shape)
                if shape.stroke.width == 1.0
                    && shape.corner_radius == egui::CornerRadius::same(12)
                    && shape.rect.height() > 350.0
                    && shape.rect.width() > 250.0 =>
            {
                Some(shape.rect)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn both_columns_keep_room_when_the_details_width_is_changed() {
    for width in [760.0, 1200.0, 1800.0] {
        for share in [0.3, 0.5, 0.65] {
            let mut app = app();
            app.page = Page::Installed;
            app.settings.details_width = share;
            app.invalidate();
            let mut harness = harness_at(app, egui::vec2(width, 800.0));
            harness.run_steps(4);
            harness.get_by_label_contains("GIMP, APT, System").click();
            harness.run_steps(4);
            let cards = list_and_page_cards(&harness);
            assert_eq!(cards.len(), 2, "{width}, {share}: {cards:?}");
            let (list, page) = (cards[0], cards[1]);
            assert!(
                list.width() >= 265.9 && page.width() >= 279.9,
                "{width}, {share}: {cards:?}"
            );
            assert!((page.left() - list.right() - 14.0).abs() < 0.1);
            assert_eq!(page.top(), list.top());
            assert_eq!(page.bottom(), list.bottom());
            assert!(page.right() <= width);
            if width == 760.0 && share == 0.3 {
                assert!((page.width() - 280.0).abs() < 0.1);
            }
            if width == 760.0 && share == 0.65 {
                assert!((list.width() - 266.0).abs() < 0.1);
            }
            if share == 0.5 {
                assert!((page.width() - list.width() - 14.0).abs() < 0.1);
            }
        }
    }
}

#[test]
fn the_columns_stay_inside_the_window_as_the_page_opens_and_closes() {
    let mut app = app();
    app.page = Page::Installed;
    app.settings.reduce_motion = false;
    app.invalidate();
    let mut harness = harness_at(app, egui::vec2(1200.0, 800.0));
    harness.run_steps(30);
    let list = list_and_page_cards(&harness)[0];
    harness.get_by_label_contains("GIMP, APT, System").click();
    for step in 0..30 {
        harness.run_steps(1);
        for card in list_and_page_cards(&harness) {
            assert!(
                card.is_finite() && card.width() >= 0.0,
                "step {step}: {card:?}"
            );
            assert!(
                card.left() >= list.left() && card.right() <= list.right() + 24.1,
                "step {step}: {card:?}"
            );
        }
    }
    harness.get_by_label("Close").click();
    for step in 0..30 {
        harness.run_steps(1);
        for card in list_and_page_cards(&harness) {
            assert!(
                card.is_finite() && card.width() >= 0.0,
                "step {step}: {card:?}"
            );
            assert!(
                card.left() >= list.left() && card.right() <= list.right() + 24.1,
                "step {step}: {card:?}"
            );
        }
    }
    let cards = list_and_page_cards(&harness);
    assert_eq!(cards.len(), 1);
    assert!((cards[0].left() - list.left()).abs() < 0.1);
    assert!((cards[0].right() - list.right()).abs() < 0.1);
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

#[test]
fn a_running_change_shows_its_words_only_while_it_is_the_live_one() {
    let mut app = app();
    app.activity = serde_json::from_value(json!([{
        "id": 7, "operations": [{"upgrade_all": {"backend": "apt"}}],
        "labels": ["Update all (APT)"], "started_at": 1, "state": "running", "outcomes": []
    }]))
    .unwrap();
    app.drawer_open = true;
    app.busy = true;
    app.writing = true;
    app.progress.activity_id = Some(7);
    app.progress.label = "Unpacking htop".into();
    let mut harness = harness(app);
    harness.run_steps(4);
    // The drawer is on the right; the page may have its own Cancel.
    let cancel_in_drawer = |harness: &Harness<'static, (bool, App)>| {
        harness
            .query_all_by_label("Cancel")
            .any(|node| node.rect().left() > 700.0)
    };
    assert!(harness.query_by_label("Unpacking htop").is_some());
    assert!(cancel_in_drawer(&harness));
    // Reading with a count: the count, but nothing to cancel.
    {
        let app = &mut harness.state_mut().1;
        // Not busy, so the page has no Cancel of its own for a load.
        app.busy = false;
        app.writing = false;
        app.reading = true;
        app.progress.label.clear();
        app.progress.done = 1;
        app.progress.total = 3;
    }
    harness.run_steps(2);
    assert!(harness.query_by_label("1 of 3").is_some());
    assert!(!cancel_in_drawer(&harness));
    // Words without a count, still with nothing to cancel.
    {
        let app = &mut harness.state_mut().1;
        app.progress.total = 0;
        app.progress.label = "Reading package lists".into();
    }
    harness.run_steps(2);
    assert!(harness.query_by_label("Reading package lists").is_some());
    assert!(!cancel_in_drawer(&harness));
    // Another change is the live one.
    {
        let app = &mut harness.state_mut().1;
        app.writing = true;
        app.progress.label = "Unpacking htop".into();
        app.progress.activity_id = Some(8);
    }
    harness.run_steps(2);
    assert!(harness.query_by_label("Unpacking htop").is_none());
    assert!(harness.query_by_label("1 of 3").is_none());
    assert!(!cancel_in_drawer(&harness));
}

#[test]
fn clean_all_works_while_lists_load_but_waits_for_a_change() {
    let mut app = app();
    let task = |name: &str| {
        json!({"kind": "cleanup", "name": name, "display_name": name, "source": "apt",
               "cleanup_kind": "package_cache", "summary": "12 cached downloads"})
    };
    app.c().set_rows(
        json!([task("APT download cache"), task("Unused dependencies")])
            .to_string()
            .into(),
    );
    app.react();
    app.page = Page::Clean;
    app.invalidate();
    app.busy = true;
    app.reading = true;
    let mut harness = harness(app);
    harness.run_steps(4);
    let enabled = |harness: &Harness<'static, (bool, App)>| {
        !harness
            .get_by_role_and_label(Role::Button, "Clean all")
            .accesskit_node()
            .is_disabled()
    };
    assert!(enabled(&harness));
    harness.state_mut().1.reading = false;
    harness.run_steps(2);
    assert!(!enabled(&harness));
    {
        let app = &mut harness.state_mut().1;
        app.busy = false;
        app.retained = Some(Vec::new());
    }
    harness.run_steps(2);
    assert!(!enabled(&harness));
}

#[test]
fn glides_settle_without_overshoot_whatever_the_frame_rate() {
    use pkgdeck::ui::widgets::spring_step;
    let omega = 6.0 / 0.17;
    // From 100 px away at rest, in 60 Hz frames.
    let (mut offset, mut speed) = (100.0_f32, 0.0_f32);
    for _ in 0..60 {
        (offset, speed) = spring_step(offset, speed, omega, 1.0 / 60.0);
        assert!(offset >= 0.0, "overshot to {offset}");
    }
    assert!(offset < 0.01, "still {offset} away after a second");
    // Two half steps land where one whole step does.
    let whole = spring_step(40.0, -300.0, omega, 0.02);
    let half = spring_step(40.0, -300.0, omega, 0.01);
    let halves = spring_step(half.0, half.1, omega, 0.01);
    assert!((whole.0 - halves.0).abs() < 1e-3 && (whole.1 - halves.1).abs() < 1e-2);
    // Most of the way there in the time it's given.
    let mut offset = 100.0_f32;
    let mut speed = 0.0_f32;
    for _ in 0..10 {
        (offset, speed) = spring_step(offset, speed, omega, 0.017);
    }
    assert!(offset < 5.0, "{offset}");
}
