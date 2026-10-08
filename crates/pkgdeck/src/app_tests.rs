//! The app's reactions, with the controller's properties set by hand: what
//! the window does when rows, notices, confirmations and background state
//! change, without running a package manager.

use super::*;
use pkgdeck_app::controller::ffi::create_synthetic_controller;
use pkgdeck_core::engine::Engine;
use serde_json::json;

fn empty(
    _: &[String],
    _: bool,
    _: pkgdeck_core::host::Authorization,
    _: &pkgdeck_core::process::Cancellation,
) -> Result<Engine, pkgdeck_core::engine::EngineError> {
    Ok(Engine::default())
}

fn app_with(launch: Launch) -> App {
    let (store, settings) = Store::open(None, None);
    let mut platform = Platform::start(|| {});
    platform.record_notifications();
    App::with_controller(
        create_synthetic_controller(empty),
        settings,
        store,
        platform,
        launch,
    )
}

fn app() -> App {
    app_with(Launch::default())
}

fn row(name: &str, extra: Value) -> Value {
    let mut row = json!({
        "kind": "package", "name": name, "display_name": name.to_uppercase(), "source": "apt",
        "architecture": "x86_64", "scope": "system", "installed": "1", "candidate": null,
        "update": "current", "summary": "",
    });
    for (key, value) in extra.as_object().unwrap() {
        row[key] = value.clone();
    }
    row
}

/// Sets the controller's rows as a load would, and reacts.
fn rows(app: &mut App, rows: Value) {
    app.c().set_rows(rows.to_string().into());
    app.react();
    app.refresh_items();
}

#[test]
fn launches_choose_sources_and_open_their_input() {
    let app = app_with(Launch {
        from: vec!["flatpak".into()],
        input: "/nonexistent/pkgdeck/thing.flatpakref".into(),
        ..Launch::default()
    });
    assert_eq!(app.enabled, HashSet::from(["flatpak".to_owned()]));
}

#[test]
fn turning_every_source_on_follows_new_sources_too() {
    let mut app = app_with(Launch {
        from: vec!["flatpak".into()],
        ..Launch::default()
    });
    for id in all_sources(&app.catalog) {
        app.set_source_enabled(&id, true);
    }
    assert!(app.enabled.is_empty());
    app.set_source_enabled("flatpak", false);
    assert!(!app.enabled_sources().contains("flatpak"));
}

#[test]
fn sources_list_the_enabled_ones_first() {
    let mut app = app_with(Launch {
        from: vec!["homebrew".into()],
        ..Launch::default()
    });
    app.page = Page::Sources;
    let source = |id: &str| json!({"kind": "source", "name": id, "source": id, "available": true});
    rows(&mut app, json!([source("apt"), source("homebrew")]));
    let order: Vec<&str> = app
        .items
        .iter()
        .map(|item| app.rows[item.raw].source.as_str())
        .collect();
    assert_eq!(order, ["homebrew", "apt"]);
    // Other pages keep their own narrower choice.
    app.page_sources
        .insert(Page::Installed, HashSet::from(["apt".to_owned()]));
    app.page = Page::Installed;
    rows(
        &mut app,
        json!([row("a", json!({})), row("b", json!({"source": "homebrew"}))]),
    );
    assert!(app.items.is_empty());
}

#[test]
fn rows_stay_on_screen_while_the_same_list_reloads() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
    assert_eq!(app.items.len(), 2);
    // A reload starts: the old rows stay until new ones cover them.
    app.retained = Some(app.rows.clone());
    app.c().set_busy(true);
    rows(&mut app, json!([]));
    assert_eq!(
        app.shown_rows().len(),
        2,
        "empty rows while busy keep the old ones"
    );
    rows(&mut app, json!([row("a", json!({}))]));
    assert!(app.retained.is_some(), "fewer rows don't replace them yet");
    // Retained rows can't be acted on or chosen.
    app.choose(0, true);
    assert!(app.selected.is_none());
    app.propose("remove");
    app.hovered_row(Some(0));
    app.tick(Instant::now() + Duration::from_secs(1));
    app.c().set_busy(false);
    app.react();
    assert!(app.retained.is_none());
    assert_eq!(app.items.len(), 1);
    // A search narrows retained rows by what's typed.
    app.page = Page::Search;
    app.query = "zz".into();
    app.retained = Some(app.rows.clone());
    app.invalidate();
    app.refresh_items();
    assert!(app.items.is_empty());
}

#[test]
fn changes_flash_the_rows_they_touched() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
    app.c().set_writing(true);
    app.react();
    assert_eq!(app.snapshot.len(), 2);
    app.c().set_busy(true);
    app.c().set_writing(false);
    app.react();
    assert!(app.post_write_reload);
    rows(
        &mut app,
        json!([row("a", json!({"installed": "2"})), row("b", json!({}))]),
    );
    assert_eq!(app.flashes.len(), 1);
    app.c().set_busy(false);
    app.react();
    assert!(
        !app.post_write_reload,
        "the page reloads once the write ends"
    );
    app.tick(Instant::now() + FLASH * 2);
    assert!(app.flashes.is_empty());
    // Without a snapshot there's nothing to compare.
    app.snapshot.clear();
    app.flash_changed_rows();
}

#[test]
fn a_close_while_busy_waits_for_the_job() {
    let mut app = app();
    app.settings.background_mode = false;
    app.c().set_busy(true);
    app.react();
    assert!(!app.close_requested());
    assert!(app.close_pending);
    app.c().set_busy(true);
    app.react();
    app.c().set_busy(false);
    app.react();
    assert_eq!(app.window_request, WindowRequest::Quit);
    let mut app = self::app();
    app.c().set_busy(true);
    app.react();
    app.quit();
    assert!(app.close_pending && app.force_quit);
    // Writing to a closing window doesn't reload it.
    let mut app = self::app();
    app.page = Page::Installed;
    app.close_pending = true;
    app.c().set_writing(true);
    app.react();
    app.c().set_writing(false);
    app.react();
    assert!(!app.post_write_reload);
}

#[test]
fn reads_show_who_they_wait_for_after_a_moment() {
    let mut app = app();
    app.c().set_busy(true);
    app.react();
    assert!(!app.waiting_shown());
    app.read_started = Some(Instant::now() - Duration::from_secs(1));
    assert!(app.waiting_shown());
    app.c().set_writing(true);
    app.react();
    assert!(!app.waiting_shown());
}

#[test]
fn notices_become_toasts_banners_and_undo() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("a", json!({}))]));
    let target = json!({"source": "apt", "name": "a", "architecture": "x86_64", "scope": "system"});
    app.c().set_notice(json!({"kind": "success", "title": "Removed A", "undo": true, "undo_action": "install", "target": target}).to_string().into());
    app.react();
    let toast = app.toast.clone().unwrap();
    assert_eq!(
        (toast.tone, toast.action),
        (Tone::Success, Some(ToastAction::Undo))
    );
    app.toast_action(ToastAction::Undo);
    assert!(app.toast.is_none());
    assert!(app.active_rows.contains(&app.rows[0].identity()));
    // Info is a toast too; it expires and dismisses its notice.
    app.c().set_notice(
        json!({"kind": "info", "title": "Nothing to do"})
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(app.toast.as_ref().unwrap().tone, Tone::Accent);
    app.tick(Instant::now() + Duration::from_secs(6));
    assert!(app.toast.is_none());
    assert!(app.notice.is_empty());
    // An error is a banner, not a toast; clearing it clears a notice toast.
    app.c().set_notice(
        json!({"kind": "error", "title": "Failed"})
            .to_string()
            .into(),
    );
    app.react();
    assert!(app.toast.is_none());
    app.toast = Some(Toast {
        text: "x".into(),
        tone: Tone::Success,
        action: None,
        shown: Instant::now(),
        lasts: Duration::from_secs(5),
        from_notice: true,
    });
    app.c().set_notice("{}".into());
    app.react();
    assert!(app.toast.is_none());
    // Undo of a row that's gone, or of a notice without a target, does nothing.
    app.c().set_notice(json!({"kind": "success", "title": "x", "undo": true, "undo_action": "remove", "target": {"source": "apt", "name": "gone"}}).to_string().into());
    app.react();
    app.toast_action(ToastAction::Undo);
    app.c()
        .set_notice(json!({"kind": "success", "title": "x"}).to_string().into());
    app.react();
    app.toast_action(ToastAction::Undo);
}

#[test]
fn self_updates_restart_now_or_ask() {
    let mut app = app();
    app.c().set_self_update("manual".into());
    app.react();
    assert_eq!(
        app.restart_toast.as_ref().unwrap().action,
        Some(ToastAction::Restart)
    );
    app.toast_action(ToastAction::Restart);
    assert!(app.restart_toast.is_none());
    // An unknown kind does nothing.
    app.c().set_self_update("later".into());
    app.react();
    // Automatic: after five seconds, later while a change runs.
    app.c().set_self_update("automatic".into());
    app.react();
    let at = app.restart_at.unwrap();
    app.c().set_writing(true);
    app.react();
    app.tick(at + Duration::from_millis(10));
    assert!(app.restart_at.unwrap() > at);
    app.c().set_writing(false);
    app.react();
    app.tick(app.restart_at.unwrap() + Duration::from_millis(10));
    assert!(app.restart_at.is_none());
    // The restart toast lasts a day.
    app.restart_toast = Some(Toast {
        text: "x".into(),
        tone: Tone::Success,
        action: None,
        shown: Instant::now(),
        lasts: Duration::from_secs(1),
        from_notice: false,
    });
    app.tick(Instant::now() + Duration::from_secs(2));
    assert!(app.restart_toast.is_none());
}

#[test]
fn background_state_saves_badges_and_notifies() {
    let mut app = app();
    app.c().set_background_state("".into());
    app.react();
    assert_eq!(app.background, BackgroundState::default());
    app.c().set_background_state(json!({"last_check": 100, "available": 2, "failures": [{"source": "snap", "kind": "unavailable"}], "notify": true}).to_string().into());
    app.react();
    assert_eq!(app.background.available, 2);
    let saved: Value = serde_json::from_str(&app.settings.last_background_state).unwrap();
    assert_eq!(saved["failures"][0]["source"], "snap");
    assert_eq!(saved["notify"], false);
    app.c()
        .set_background_state(json!({"available": 1, "notify": true}).to_string().into());
    app.react();
    assert_eq!(app.background.available, 1);
    app.c().set_auto_update_result(
        json!({"updated": 1, "failed": 0, "total": 1})
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(
        app.platform.recorded(),
        [
            (
                "PkgDeck updates".to_owned(),
                "2 updates available".to_owned()
            ),
            (
                "PkgDeck updates".to_owned(),
                "1 update available".to_owned()
            ),
            (
                "PkgDeck updates".to_owned(),
                "Installed 1 update".to_owned()
            ),
        ]
    );
    // Nothing installed: no notification.
    app.c()
        .set_auto_update_result(json!({"total": 0}).to_string().into());
    app.react();
    assert_eq!(app.platform.recorded().len(), 3);
    app.c().set_notification_history("{\"notified\":{}}".into());
    app.c()
        .set_system_approval("sudoers:automatic-updates".into());
    app.react();
    assert_eq!(app.settings.notification_history, "{\"notified\":{}}");
    assert_eq!(app.settings.system_approval, "sudoers:automatic-updates");
}

#[test]
fn confirmations_show_on_the_page_in_a_dialog_or_run_alone() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("a", json!({}))]));
    app.choose(0, true);
    let identity = app.selected.clone().unwrap();
    app.review_on = ReviewOn::Page(identity.clone());
    app.c()
        .set_confirmation_data(json!({"action": "Remove A"}).to_string().into());
    app.c().set_confirmation("Remove A?".into());
    app.react();
    assert!(app.review_on_page && !app.confirm_open);
    // New data for the same review keeps where it shows.
    app.c().set_confirmation_data(
        json!({"action": "Remove A", "notes": ["n"]})
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(app.confirmation.as_ref().unwrap().notes, ["n"]);
    // Choosing another row turns the review down.
    rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
    app.choose(1, true);
    assert!(!app.review_on_page);
    // A Flatpak reference asks in the dialog.
    app.review_on = ReviewOn::Page(app.selected.clone().unwrap());
    app.c().set_confirmation("".into());
    app.react();
    app.c().set_confirmation_data(
        json!({"action": "Install", "flatpak_ref_scope": "user"})
            .to_string()
            .into(),
    );
    app.c().set_confirmation("Install?".into());
    app.react();
    assert!(app.confirm_open);
    app.confirm(false);
    // A quick change that only touches the asked app runs on its own.
    app.quick_change = true;
    app.c().set_confirmation("".into());
    app.react();
    app.c().set_confirmation_data(
        json!({"action": "Install", "review": false})
            .to_string()
            .into(),
    );
    app.c().set_confirmation("Install B?".into());
    app.react();
    assert_eq!(app.auto_confirm.as_deref(), Some("Install B?"));
    app.tick(Instant::now());
    assert!(app.auto_confirm.is_none());
    // A review that changed in the meantime isn't confirmed blindly.
    app.auto_confirm = Some("old".into());
    app.c().set_confirmation("new".into());
    app.react();
    app.tick(Instant::now());
    // Leaving a page with its review open turns it down.
    app.review_on_page = true;
    app.deselect();
    assert!(!app.review_on_page);
    app.review_on_page = true;
    app.close_page();
    assert!(!app.review_on_page);
}

#[test]
fn opened_files_get_a_page_and_its_main_action() {
    let mut app = app();
    app.opening = true;
    app.c().set_opened(json!({"package": row("tool", json!({"installed": null, "candidate": "1"})), "action": "Install", "description": "d"}).to_string().into());
    app.react();
    assert!(!app.opening);
    assert_eq!(app.page_key().as_deref(), Some("opened"));
    assert_eq!(app.opened.as_ref().unwrap().action, "Install");
    app.run_page_action();
    assert_eq!(app.review_on, ReviewOn::Page("opened".into()));
    app.c().set_busy(true);
    app.react();
    app.quick_change = false;
    app.run_page_action();
    app.c().set_busy(false);
    app.react();
    app.review_on_page = true;
    app.close_page();
    assert!(app.opened.is_none());
    app.c().set_opened("".into());
    app.react();
    assert!(app.opened.is_none());
}

#[test]
fn flatpak_copies_switch_scope_and_reselect_when_one_goes() {
    let mut app = app();
    app.page = Page::Installed;
    let system = row(
        "org.gimp.GIMP",
        json!({"source": "flatpak", "remote": "flathub"}),
    );
    let user = row(
        "org.gimp.GIMP",
        json!({"source": "flatpak", "remote": "flathub", "scope": {"user": {"uid": 1}}, "installed": null}),
    );
    rows(&mut app, json!([system.clone(), user.clone()]));
    assert_eq!(app.items.len(), 1);
    app.choose(0, true);
    let first = app.selected.clone().unwrap();
    app.choose_scope(1);
    assert_ne!(app.selected.clone().unwrap(), first);
    assert_eq!(app.scope_choices.len(), 1);
    // Out of range, nothing selected, or busy: nothing happens.
    app.choose_scope(5);
    app.c().set_busy(true);
    app.react();
    app.choose_scope(0);
    app.c().set_busy(false);
    app.react();
    let chosen = app.selected.clone();
    app.selected = None;
    app.choose_scope(0);
    app.selected = chosen;
    // The chosen copy goes away: the other copy is selected, page still open.
    rows(&mut app, json!([system]));
    assert_eq!(app.selected.as_deref(), Some(first.as_str()));
    assert!(app.page_open);
}

#[test]
fn selections_survive_reloads_or_clear() {
    let mut app = app();
    app.page = Page::Search;
    rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
    app.choose(1, false);
    app.choose(1, false);
    app.choose(9, false);
    // Search keeps the selection while the raw rows still have it.
    app.result_query = "x".into();
    app.query = "x".into();
    app.invalidate();
    app.refresh_items();
    app.restore_selection();
    assert!(app.selected.is_some());
    // Elsewhere a missing row clears it.
    app.page = Page::Installed;
    rows(&mut app, json!([row("c", json!({}))]));
    assert!(app.selected.is_none());
    // A row still there asks for its details again when they're empty.
    app.choose(0, true);
    app.c().set_details("{}".into());
    app.react();
    app.restore_selection();
    assert!(app.selected.is_some());
    assert!(app.selected_row().is_some());
    assert!(app.details_for_selection().is_none());
}

#[test]
fn row_actions_need_a_row_an_action_and_a_free_controller() {
    let mut app = app();
    app.page = Page::Sources;
    rows(
        &mut app,
        json!([{"kind": "source", "name": "apt", "source": "apt", "available": true}]),
    );
    app.run_row_action(0);
    app.run_row_action(7);
    app.run_adopt(7);
    app.run_adopt(0);
    app.page = Page::Installed;
    rows(
        &mut app,
        json!([
            row("a", json!({})),
            row("t", json!({"source": "appimage", "adopt_with": "appimage"}))
        ]),
    );
    app.c().set_busy(true);
    app.react();
    app.run_row_action(0);
    app.run_adopt(1);
    app.c().set_busy(false);
    app.react();
    app.run_adopt(1);
    assert!(!app.active_rows.is_empty());
    app.propose("adopt-all");
    app.propose("clean-all");
    app.choose(0, false);
    app.propose("refresh");
    // Updates with nothing checked do nothing.
    app.page = Page::Updates;
    app.invalidate();
    app.refresh_items();
    app.unchecked = app
        .items
        .iter()
        .map(|i| app.rows[i.raw].identity())
        .collect();
    app.upgrade_updates();
}

#[test]
fn pages_load_what_they_show() {
    let mut app = app();
    app.open_page(Page::Settings);
    app.reload(true, false);
    app.submit_search();
    app.open_page(Page::Search);
    app.open_page(Page::Search);
    app.query = "x".into();
    app.retry_source("apt");
    app.page_sources
        .insert(Page::Installed, HashSet::from(["apt".to_owned()]));
    assert_eq!(app.source_summary(Page::Installed), "APT");
    assert_eq!(app.sources_csv(Page::Installed), "apt");
    assert_eq!(app.sources_csv(Page::Sources), "");
    app.enabled = HashSet::from(["apt".to_owned(), "snap".to_owned()]);
    assert_eq!(app.sources_csv(Page::Updates), "apt,snap");
    app.change_repository(
        json!({"backend": "flatpak", "name": "x", "scope": "user", "action": "remove"}),
    );
    app.cancel();
}

#[test]
fn installed_appimages_read_their_page_once() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("t", json!({"source": "appimage"}))]));
    app.app_info(0, "t");
    app.app_info(0, "t");
    assert_eq!(app.app_info_for.as_deref(), Some("t"));
    app.reread_app_info();
    assert!(app.app_info_for.is_none());
}

#[test]
fn failures_come_from_the_report_or_failure_rows() {
    let mut app = app();
    app.page = Page::Installed;
    rows(
        &mut app,
        json!([
            {"kind": "failure", "source": "apt", "failure_kind": "unavailable", "summary": "apt is broken"},
            {"kind": "failure", "source": "snap", "failure_kind": "unsupported"},
        ]),
    );
    let failures = app.read_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(app.failure_reason("apt"), "apt is broken");
    app.c().set_report_state(json!({"phase": "partial", "failures": [{"source": "apt", "kind": "failed", "detail": "d"}, {"source": "apt", "kind": "cancelled"}]}).to_string().into());
    app.react();
    assert_eq!(app.read_failures().len(), 1);
    app.rows.clear();
    assert_eq!(app.failure_reason("apt"), "d");
}

#[test]
fn ticks_run_what_is_due() {
    let mut app = app();
    // Tray events arrive through the tick. Tests start platforms in
    // parallel and the newest one hears the tray, so this one claims it.
    for _ in 0..200 {
        app.platform = Platform::start(|| {});
        crate::platform::send(Event::Open);
        app.tick(Instant::now());
        if app.window_request == WindowRequest::Show {
            break;
        }
    }
    assert_eq!(app.window_request, WindowRequest::Show);
    app.platform.record_notifications();
    // A background check is due.
    app.next_check = Instant::now() - Duration::from_secs(1);
    app.tick(Instant::now());
    assert!(app.next_check > Instant::now());
    // A restart that isn't due yet waits.
    let at = Instant::now() + Duration::from_secs(60);
    app.restart_at = Some(at);
    app.tick(Instant::now());
    assert_eq!(app.restart_at, Some(at));
    // A search due soon shortens the wait.
    app.search_due = Some(Instant::now() + Duration::from_millis(50));
    assert!(app.tick(Instant::now()) <= Duration::from_millis(50));
    // Second launches hand their input over.
    let path = std::env::temp_dir().join(format!("pkgdeck-app-{}", std::process::id()));
    app.listener = crate::opening::claim(&path, "", || {}).unwrap();
    assert!(crate::opening::forward(&path, "/nonexistent/pkgdeck/x.deb"));
    let mut input = None;
    for _ in 0..500 {
        input = app.next_input();
        if input.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(input.as_deref(), Some("/nonexistent/pkgdeck/x.deb"));
    app.listener = None;
    assert_eq!(app.next_input(), None);
}

#[test]
fn activity_and_repositories_are_read() {
    let mut app = app();
    app.c()
        .set_activity(json!([{"id": 3, "state": "queued"}]).to_string().into());
    app.c().set_repositories(
        json!({"repositories": [{"backend": "flatpak", "name": "flathub", "enabled": true}]})
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.repositories.repositories[0].name, "flathub");
}

#[test]
fn writes_off_a_list_page_dont_reload() {
    let mut app = app();
    app.page = Page::Settings;
    app.c().set_writing(true);
    app.react();
    app.c().set_writing(false);
    app.react();
    assert!(!app.post_write_reload);
}

#[test]
fn a_selection_without_its_row_or_group_clears() {
    let mut app = app();
    app.page = Page::Installed;
    let copy = row("org.x.App", json!({"source": "flatpak"}));
    rows(&mut app, json!([copy]));
    app.choose(0, true);
    rows(&mut app, json!([row("other", json!({}))]));
    assert!(app.selected.is_none());
    // On Search, a selection whose raw row is still there but filtered stays.
    app.page = Page::Search;
    rows(&mut app, json!([row("kept", json!({}))]));
    app.result_query = "kept".into();
    app.invalidate();
    app.refresh_items();
    app.choose(0, false);
    app.page_sources
        .insert(Page::Search, HashSet::from(["snap".to_owned()]));
    app.invalidate();
    app.refresh_items();
    app.restore_selection();
    assert!(app.selected.is_some());
}

#[test]
fn proposals_for_the_selection_review_where_it_was_asked() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("a", json!({}))]));
    app.choose(0, true);
    app.propose("remove");
    assert_eq!(app.review_on, ReviewOn::Page(app.selected.clone().unwrap()));
}

#[test]
fn opening_text_that_isnt_a_file_answers_at_once() {
    let mut app = app();
    app.open_input("   ");
    assert!(!app.opening);
}

#[test]
fn restarting_without_an_install_stays_open() {
    let mut app = app();
    app.window_visible = false;
    app.restart();
    assert!(!app.force_quit);
    app.restarted(true);
    assert!(app.force_quit);
    assert_eq!(app.window_request, WindowRequest::Quit);
}

#[test]
fn quiet_paths_do_nothing() {
    let mut app = app();
    app.page = Page::Installed;
    // The writing flag changed back before anyone looked.
    app.c().set_writing(true);
    app.c().set_writing(false);
    app.react();
    assert!(!app.post_write_reload);
    // A background check that found nothing new doesn't notify.
    app.c().set_background_state(
        json!({"last_check": 1, "available": 0, "notify": false})
            .to_string()
            .into(),
    );
    app.react();
    assert!(app.platform.recorded().is_empty());
    // No toast to hide; a toast of its own doesn't dismiss the notice.
    app.hide_toast();
    app.toast = Some(Toast {
        text: "x".into(),
        tone: Tone::Success,
        action: None,
        shown: Instant::now(),
        lasts: Duration::from_secs(5),
        from_notice: false,
    });
    app.hide_toast();
    // Rows a change didn't know about don't flash.
    rows(&mut app, json!([row("a", json!({}))]));
    app.c().set_writing(true);
    app.react();
    rows(
        &mut app,
        json!([row("a", json!({})), row("new", json!({}))]),
    );
    assert!(app.flashes.is_empty());
    // Opening while busy keeps showing "Opening…".
    app.c().set_busy(true);
    app.react();
    app.open_input("/nonexistent/pkgdeck/x.deb");
    assert!(app.opening);
}
