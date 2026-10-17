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
fn pages_keep_their_own_choice_of_sources() {
    let mut app = app_with(Launch {
        from: vec!["homebrew".into()],
        ..Launch::default()
    });
    // Pages keep their own narrower choice.
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
fn refresh_the_page_refresh_checks_updates_once_and_retains_rows_until_it_finishes() {
    let mut app = app();
    app.page = Page::Updates;
    app.c().set_source_catalog(
        json!([{"source":"npm", "available":true, "capabilities":["installed"]}])
            .to_string()
            .into(),
    );
    app.react();
    rows(
        &mut app,
        json!([row(
            "tool",
            json!({"source":"npm", "candidate":"2", "update":"available"})
        )]),
    );
    app.refresh_page();
    assert!(app.checking_updates);
    assert!(app.busy);
    assert!(!app.can_act());
    assert_eq!(app.retained.as_ref().unwrap().len(), 1);
    assert!(app.confirmation.is_none());
    app.refresh_page();
    assert!(app.checking_updates);
    // Completion owns its report; it must not start the generic post-write
    // reload that would erase a failed index refresh from that report.
    app.c().set_writing(false);
    app.c().set_busy(false);
    app.c().set_report_state(r#"{"phase":"partial","failures":[{"source":"npm","kind":"failed","detail":"Network unavailable"}]}"#.into());
    app.react();
    assert!(!app.checking_updates);
    assert!(!app.post_write_reload);
    assert_eq!(app.read_failures().len(), 1);
    assert!(app.retained.is_none());
}

#[test]
fn refresh_page_refresh_respects_busy_confirmations_and_pages_without_lists() {
    let mut app = app();
    app.page = Page::Settings;
    app.refresh_page();
    assert!(!app.busy);
    app.page = Page::Updates;
    app.page = Page::Updates;
    app.enabled = HashSet::from(["invalid-old-setting".to_owned()]);
    app.refresh_page();
    assert!(!app.checking_updates);
    assert!(!app.busy);
    app.enabled.clear();
    app.page = Page::Installed;
    app.busy = true;
    app.refresh_page();
    assert!(!*app.ctl.busy());
    app.busy = false;
    app.c().set_confirmation("Review".into());
    app.react();
    app.refresh_page();
    assert!(!app.busy);
    app.confirmation = None;
    app.c().set_confirmation("".into());
    app.react();
    app.refresh_page();
    assert!(app.busy);
    assert!(!app.checking_updates);
}

#[test]
fn a_failed_row_says_why_until_the_controller_forgets_it() {
    let mut app = app();
    app.page = Page::Clean;
    rows(
        &mut app,
        json!([
            {"kind": "cleanup", "name": "a worktrees", "source": "projects", "cleanup_key": "worktrees:/a"},
            {"kind": "cleanup", "name": "b worktrees", "source": "projects", "cleanup_key": "worktrees:/b"},
        ]),
    );
    app.c().set_failed_rows(
        json!([{"target": {"source": "projects", "cleanup_key": "worktrees:/a"}, "reason": "b is busy"}])
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(app.row_failure(&app.rows[0]), Some("b is busy"));
    assert_eq!(app.row_failure(&app.rows[1]), None);
    app.c().set_failed_rows("[]".into());
    app.react();
    assert_eq!(app.row_failure(&app.rows[0]), None);
}

#[test]
fn another_section_never_shows_the_rows_of_the_one_left() {
    for (from, to) in [
        (Page::Installed, Page::Updates),
        (Page::Updates, Page::Installed),
        (Page::Installed, Page::Clean),
        (Page::Installed, Page::Search),
    ] {
        let mut app = app();
        app.page = from;
        rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
        assert_eq!(app.shown_rows().len(), 2);
        // Something still loads, so the new section waits for its rows.
        app.c().set_busy(true);
        app.react();
        if to == Page::Search {
            app.query = "a".into();
        }
        app.open_page(to);
        app.react();
        assert!(
            app.retained.is_none(),
            "{from:?} -> {to:?} kept the rows it left"
        );
        // Its own first rows are shown, even fewer than the old page had.
        rows(&mut app, json!([row("c", json!({"update": "available"}))]));
        let shown: Vec<_> = app
            .shown_rows()
            .iter()
            .map(|row| row.name.clone())
            .collect();
        assert_eq!(shown, ["c"], "{from:?} -> {to:?}");
    }
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

/// Runs a write and ends it with `batch` as its result. The job is still
/// busy when the write ends, as it is until the reload it starts finishes.
fn end_batch(app: &mut App, batch: Value) {
    app.c().set_busy(true);
    app.c().set_writing(true);
    app.react();
    app.c().set_batch_finished(batch.to_string().into());
    app.c().set_writing(false);
    app.react();
}

#[test]
fn finished_rows_stay_done_until_the_reload_confirms() {
    let mut app = app();
    app.page = Page::Updates;
    rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
    let (a, b) = (app.rows[0].identity(), app.rows[1].identity());
    end_batch(
        &mut app,
        json!({"sources": ["apt"], "finished": [row("a", json!({}))]}),
    );
    assert!(app.done_rows.contains(&a), "the finished row is Done");
    assert!(!app.done_rows.contains(&b), "an unfinished row is not");
    assert!(!app.post_write_reload, "a refresh replaces the full reload");
    // The reload confirms the rows, so the marks go.
    app.c().set_busy(false);
    app.react();
    assert!(app.done_rows.is_empty());
}

#[test]
fn a_failed_row_is_never_marked_done() {
    let mut app = app();
    app.page = Page::Updates;
    rows(&mut app, json!([row("a", json!({})), row("b", json!({}))]));
    let b = app.rows[1].identity();
    // `b` failed, so the batch lists only `a`.
    end_batch(
        &mut app,
        json!({"sources": ["apt"], "finished": [row("a", json!({}))]}),
    );
    assert!(!app.done_rows.contains(&b));
}

#[test]
fn a_single_target_batch_marks_nothing_and_reloads_the_page() {
    let mut app = app();
    app.page = Page::Updates;
    rows(&mut app, json!([row("a", json!({}))]));
    end_batch(&mut app, json!({"sources": [], "finished": []}));
    assert!(app.done_rows.is_empty());
    assert!(app.post_write_reload, "no refresh was planned, so reload");
}

#[test]
fn an_empty_first_run_batch_is_safe() {
    let mut app = app();
    app.page = Page::Updates;
    rows(&mut app, json!([row("a", json!({}))]));
    end_batch(&mut app, json!({}));
    assert!(app.done_rows.is_empty());
}

#[test]
fn other_pages_reload_instead_of_refreshing_sources() {
    let mut app = app();
    app.page = Page::Installed;
    rows(&mut app, json!([row("a", json!({}))]));
    let a = app.rows[0].identity();
    end_batch(
        &mut app,
        json!({"sources": ["apt"], "finished": [row("a", json!({}))]}),
    );
    assert!(app.done_rows.contains(&a));
    assert!(app.post_write_reload);
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
    // A failure is a calm toast whose Details opens Activity, which keeps it.
    app.c().set_failure(
        json!({"kind": "error", "title": "Failed", "retry": true})
            .to_string()
            .into(),
    );
    app.c().set_notice(
        json!({"kind": "error", "title": "Failed"})
            .to_string()
            .into(),
    );
    app.react();
    let toast = app.toast.clone().unwrap();
    assert_eq!(
        (toast.tone, toast.action, toast.lasts),
        (
            Tone::Warning,
            Some(ToastAction::Details),
            Duration::from_secs(8)
        )
    );
    assert!(app.failure_unseen);
    app.toast_action(ToastAction::Details);
    assert!(app.drawer_open && !app.failure_unseen);
    assert!(app.toast.is_none() && app.notice.is_empty());
    assert_eq!(app.failure.title, "Failed");
    // Seen with Activity open, it raises no dot.
    app.c().set_failure(
        json!({"kind": "error", "title": "Again"})
            .to_string()
            .into(),
    );
    app.react();
    assert!(!app.failure_unseen);
    app.toggle_activity();
    app.c().set_failure("{}".into());
    app.react();
    assert!(app.failure.is_empty() && !app.failure_unseen);
    // Only what needs the person now is a banner, and only on its page.
    app.c().set_notice(
        json!({"kind": "error", "title": "Denied", "urgent": true})
            .to_string()
            .into(),
    );
    app.react();
    assert!(app.toast.is_none());
    assert_eq!(app.notice_page, Page::Installed);
    // Clearing a notice clears its toast.
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
fn setup_is_offered_once() {
    let mut app = app();
    app.ask_about_setup();
    assert!(app.ui.setup_open);
    assert!(app.settings.setup_asked);
    // Once asked, never again, even after it closes.
    app.ui.setup_open = false;
    app.ask_about_setup();
    assert!(!app.ui.setup_open);
}

#[test]
fn setup_is_not_offered_when_it_makes_no_sense() {
    let toast = Toast {
        text: "PkgDeck was updated.".into(),
        tone: Tone::Success,
        action: Some(ToastAction::Restart),
        shown: Instant::now(),
        lasts: Duration::from_secs(60),
        from_notice: false,
    };
    let all_on = |app: &mut App| {
        app.settings.auto_update = true;
        app.settings.auto_clean = true;
        app.settings.system_approval = "polkit:automatic-updates".into();
        app.settings.autostart = true;
    };
    for case in 0..4 {
        let mut app = app();
        match case {
            0 => app.settings.setup_asked = true,
            // Everything in it needs background checks.
            1 => app.settings.background_mode = false,
            // Another sticky toast is showing; it waits for the next start.
            2 => app.restart_toast = Some(toast.clone()),
            // Nothing left to turn on.
            _ => all_on(&mut app),
        }
        app.ask_about_setup();
        assert!(!app.ui.setup_open, "{case}");
        assert_eq!(app.settings.setup_asked, case == 0, "{case}");
    }
    // Any one still off is worth asking about.
    for off in 0..4 {
        let mut app = app();
        all_on(&mut app);
        match off {
            0 => app.settings.auto_update = false,
            1 => app.settings.auto_clean = false,
            2 => app.settings.system_approval.clear(),
            _ => app.settings.autostart = false,
        }
        app.ask_about_setup();
        assert_eq!(
            app.ui.setup_open,
            off < 3 || app.platform.tray_available(),
            "{off}"
        );
    }
}

#[test]
fn movement_stops_when_asked_here_or_by_the_desktop() {
    let mut app = app();
    assert!(!app.reduce_motion());
    app.settings.reduce_motion = true;
    assert!(app.reduce_motion());
    app.settings.reduce_motion = false;
    app.platform.set_system_motion(true);
    assert!(app.reduce_motion());
    // The setting itself is left as it was.
    assert!(!app.settings.reduce_motion);
}

#[test]
fn automatic_cleaning_is_remembered_and_told_once() {
    let mut app = app();
    app.settings.auto_clean = true;
    app.apply_settings();
    assert!(app.c().auto_clean());
    let cleaned = |at: u64, cleaned: Value| {
        json!({"available": 0, "cleaned_at": at, "cleaned": cleaned}).to_string()
    };
    app.c().set_background_state(
        cleaned(
            500,
            json!({"tasks": 2, "failed": 0, "bytes": 4_200_000_000u64}),
        )
        .into(),
    );
    app.react();
    assert_eq!(app.settings.auto_cleaned_at, 500);
    assert_eq!(
        app.platform.recorded(),
        [("PkgDeck cleanup".to_owned(), "Freed 4.2 GB".to_owned())]
    );
    // The same run seen again, a later check without it, and a run that
    // found nothing say nothing more.
    app.c()
        .set_background_state(cleaned(500, json!({"tasks": 2, "failed": 0, "bytes": 1})).into());
    app.react();
    app.c()
        .set_background_state(json!({"available": 0}).to_string().into());
    app.react();
    assert_eq!(app.settings.auto_cleaned_at, 500);
    app.c()
        .set_background_state(cleaned(900, json!({"tasks": 0})).into());
    app.react();
    assert_eq!(app.settings.auto_cleaned_at, 900);
    assert_eq!(app.platform.recorded().len(), 1);
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
fn going_to_another_page_leaves_an_opened_file() {
    for page in [Page::Updates, Page::Search] {
        let mut app = app();
        app.c().set_opened(json!({"package": row("tool", json!({"installed": null, "candidate": "1"})), "action": "Install", "description": "d"}).to_string().into());
        app.react();
        assert!(app.opened.is_some());
        app.open_page(page);
        app.react();
        assert!(app.opened.is_none(), "{page:?}");
        assert_ne!(app.page_key().as_deref(), Some("opened"), "{page:?}");
    }
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
fn the_selection_is_found_again_after_the_list_changes() {
    let mut app = app();
    app.page = Page::Installed;
    rows(
        &mut app,
        json!([
            row("a", json!({})),
            row("b", json!({})),
            row("c", json!({}))
        ]),
    );
    app.choose(1, false);
    assert_eq!(app.selected_index(), Some(1));
    assert_eq!(app.selected_row().map(|row| row.name.as_str()), Some("b"));
    // The list moves under it.
    rows(&mut app, json!([row("b", json!({})), row("c", json!({}))]));
    assert_eq!(app.selected_index(), Some(0));
    assert_eq!(app.selected_index(), Some(0));
    // Rows cleared mid-frame find nothing, without a crash.
    app.rows.clear();
    assert_eq!(app.selected_index(), None);
    assert!(app.selected_item().is_none());
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
    app.page = Page::Installed;
    rows(
        &mut app,
        // A Mac app this computer can neither adopt nor remove.
        json!([row("m", json!({"source": "macos-apps", "installed": "1"}))]),
    );
    assert_eq!(app.items.len(), 1);
    app.run_row_action(0);
    assert!(app.confirmation.is_none() && app.active_rows.is_empty());
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
    app.choose(0, false);
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
    assert_eq!(failures[0].source, "apt");
    assert_eq!(failures[0].detail.as_deref(), Some("apt is broken"));
    assert_eq!(app.failure_reason("apt"), "apt is broken");
    app.c().set_report_state(json!({"phase": "partial", "failures": [{"source": "apt", "kind": "failed", "detail": "d"}, {"source": "apt", "kind": "cancelled"}]}).to_string().into());
    app.react();
    assert_eq!(app.read_failures().len(), 1);
    app.rows.clear();
    assert_eq!(app.failure_reason("apt"), "d");
}

#[test]
fn a_source_that_failed_more_than_once_is_one_entry_with_each_reason() {
    let mut app = app();
    app.page = Page::Updates;
    let failure =
        |detail: Option<&str>| json!({"source": "apt", "kind": "failed", "detail": detail});
    app.c().set_report_state(
        json!({"phase": "partial", "failures": [
            failure(Some("one remote is off")),
            failure(None),
            failure(Some("another can't be reached")),
            failure(Some("one remote is off")),
            {"source": "snap", "kind": "failed", "detail": "snap is broken"},
        ]})
        .to_string()
        .into(),
    );
    app.react();
    let failures = app.read_failures();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert_eq!(failures[0].source, "apt");
    assert_eq!(
        failures[0].detail.as_deref(),
        Some("one remote is off\nanother can't be reached")
    );
    assert_eq!(failures[1].detail.as_deref(), Some("snap is broken"));
    // A first failure with no reason takes the next one's.
    app.c().set_report_state(
        json!({"phase": "partial", "failures": [failure(None), failure(Some("late reason"))]})
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(
        app.read_failures()[0].detail.as_deref(),
        Some("late reason")
    );
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
    app.listener = crate::opening::claim(&path, "", false, || {}).unwrap();
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
fn a_copy_started_in_the_background_stays_hidden() {
    // No window was ever up: a restart after an update keeps it hidden,
    // and the tray's toggle shows it.
    let mut app = app();
    assert!(!app.window_visible);
    app.handle(Event::Toggle);
    assert_eq!(app.window_request, WindowRequest::Show);
}

#[test]
fn restarting_without_an_install_stays_open() {
    let mut app = app();
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

#[test]
fn a_finished_updates_list_sets_the_badge() {
    let mut app = app();
    app.c().set_background_state(
        json!({"last_check": 100, "available": 1, "failures": [], "notify": false})
            .to_string()
            .into(),
    );
    app.react();
    let updates = json!([
        row("a", json!({"candidate": "2", "update": "available"})),
        row("b", json!({"candidate": "3", "update": "available"})),
        row("c", json!({})),
    ]);
    // Another page's rows leave it alone.
    app.page = Page::Installed;
    app.c().set_rows(updates.to_string().into());
    app.react();
    assert_eq!(app.background.available, 1);
    // A snapshot still reloading doesn't count yet.
    app.page = Page::Updates;
    app.c().set_refreshing(true);
    app.c().set_rows(json!([updates[0]]).to_string().into());
    app.react();
    assert_eq!(app.background.available, 1);
    app.c().set_refreshing(false);
    app.react();
    assert_eq!(app.background.available, 1);
    // The finished list does, quietly.
    app.c().set_rows(updates.to_string().into());
    app.react();
    assert_eq!(app.background.available, 2);
    assert!(!app.background.notify);
    let saved: Value = serde_json::from_str(&app.settings.last_background_state).unwrap();
    assert_eq!(saved["available"], 2);
    assert!(app.platform.recorded().is_empty());
    // A list narrowed to some sources is only part of the count.
    app.page_sources
        .insert(Page::Updates, ["apt".to_owned()].into_iter().collect());
    app.c().set_rows(json!([updates[0]]).to_string().into());
    app.react();
    assert_eq!(app.background.available, 2);
}

#[test]
fn big_row_lists_come_back_the_same_from_memory() {
    let mut app = app();
    let list = |prefix: &str| {
        Value::Array(
            (0..200)
                .map(|i| row(&format!("{prefix}{i}"), json!({})))
                .collect(),
        )
        .to_string()
    };
    let show = |app: &mut App, text: &str| {
        app.c().set_rows(text.into());
        app.react();
        app.rows[0].name.clone()
    };
    // More lists than are kept, so the first is dropped and parsed again.
    for prefix in ["a", "b", "c", "d", "e"] {
        assert_eq!(show(&mut app, &list(prefix)), format!("{prefix}0"));
    }
    assert_eq!(show(&mut app, &list("a")), "a0");
    // Back to one still kept.
    assert_eq!(show(&mut app, &list("d")), "d0");
    assert_eq!(app.rows.len(), 200);
}

#[test]
fn the_window_sleeps_until_something_is_due() {
    let mut app = app();
    let now = Instant::now();
    app.c().set_needs_poll(false);
    app.last_poll = now;
    app.next_check = now + Duration::from_secs(60);
    assert_eq!(app.tick(now), SLEEP);
    // Its own timers: a toast that expires, a restart, the next check.
    app.toast = Some(Toast {
        text: "x".into(),
        tone: Tone::Success,
        action: None,
        shown: now,
        lasts: Duration::from_secs(4),
        from_notice: false,
    });
    assert_eq!(app.tick(now), Duration::from_secs(4));
    app.restart_at = Some(now + Duration::from_secs(3));
    assert_eq!(app.tick(now), Duration::from_secs(3));
    app.next_check = now + Duration::from_secs(2);
    assert_eq!(app.tick(now), Duration::from_secs(2));
    // While reading, a poll too soon after the last waits for its turn;
    // right after one, the next wake is a worker's reply.
    app.toast = None;
    app.restart_at = None;
    app.next_check = now + Duration::from_secs(60);
    app.c().set_needs_poll(true);
    app.last_poll = now;
    let soon = now + Duration::from_millis(50);
    assert_eq!(app.tick(soon), POLL_IDLE - Duration::from_millis(50));
    assert_eq!(app.last_poll, now);
    let later = now + POLL_IDLE;
    app.c().set_needs_poll(true);
    let wait = app.tick(later);
    assert_eq!(app.last_poll, later);
    // Still reading, and nothing is due: it looks again in a second in case
    // a worker stops without a reply.
    assert!(*app.ctl.needs_poll());
    assert_eq!(wait, Duration::from_secs(1));
}

#[test]
fn settings_wait_for_a_drag_to_end_before_they_are_written() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-drag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (store, settings) = Store::open(Some(dir.clone()), None);
    let mut app = App::with_controller(
        create_synthetic_controller(empty),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    app.tick(Instant::now());
    app.dragging = true;
    app.settings.name_column = 0.4;
    app.tick(Instant::now());
    assert_eq!(app.store.saved().name_column, 0.0);
    app.dragging = false;
    app.tick(Instant::now());
    assert_eq!(app.store.saved().name_column, 0.4);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sources_can_be_checked_again() {
    let mut app = app();
    // The first check, from the start, ends first.
    let deadline = Instant::now() + Duration::from_secs(20);
    while *app.ctl.needs_poll() && Instant::now() < deadline {
        app.tick(Instant::now() + Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!*app.ctl.needs_poll());
    app.c().recheck_sources();
    assert!(*app.ctl.needs_poll());
}

#[test]
fn the_tray_is_idle_only_with_nothing_running_or_waiting() {
    let mut app = app();
    app.c().set_window_open(false);
    // The first check of the sources, from the start, runs even then.
    assert!(!app.idle());
    let deadline = Instant::now() + Duration::from_secs(20);
    while !app.idle() && Instant::now() < deadline {
        app.tick(Instant::now() + Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.idle());
    app.busy = true;
    assert!(!app.idle());
    app.busy = false;
    app.search_due = Some(Instant::now());
    assert!(!app.idle());
    app.search_due = None;
    app.restart_at = Some(Instant::now());
    assert!(!app.idle());
    app.restart_at = None;
    app.c().set_needs_poll(true);
    assert!(!app.idle());
    app.c().set_needs_poll(false);
    app.c().set_confirmation("{\"title\":\"Remove?\"}".into());
    assert!(!app.idle());
    app.c().set_confirmation(QString::default());
    assert!(app.idle());
}

#[test]
fn cleanup_choices_survive_navigation_and_do_not_change_update_sources() {
    let mut app = app();
    app.catalog = serde_json::from_value(json!([
        {"source":"apt", "available":true, "capabilities":["installed","upgrade","clean"]},
        {"source":"dev-caches", "available":true, "capabilities":["clean"]}
    ]))
    .unwrap();
    app.enabled = ["apt".into()].into();
    assert!(app.effective_sources(Page::Clean).contains("dev-caches"));
    assert!(!app.effective_sources(Page::Updates).contains("dev-caches"));
    let row: Row = serde_json::from_value(json!({"kind":"cleanup", "source":"dev-caches", "name":"Compiler cache", "cleanup_key":"sccache", "cleanup_kind":"package_cache"})).unwrap();
    assert!(!app.row_checked(&row));
    app.set_row_checked(&row, true);
    assert!(app.row_checked(&row));
    app.set_row_checked(&row, false);
    app.unchecked.clear();
    assert!(!app.row_checked(&row));
    let saved = serde_json::to_string(&app.settings).unwrap();
    app.settings = serde_json::from_str(&saved).unwrap();
    assert!(!app.row_checked(&row));
    // Turning a source off for updates leaves Clean reading it.
    app.set_source_enabled("apt", false);
    assert!(app.effective_sources(Page::Clean).contains("apt"));
    assert_eq!(app.sources_csv(Page::Clean), "");
    assert!(!app.row_checked(&row));
}

#[test]
fn package_rows_check_on_their_own() {
    let mut app = app();
    let package: Row = serde_json::from_value(row("htop", json!({}))).unwrap();
    assert!(app.row_checked(&package));
    app.set_row_checked(&package, false);
    assert!(!app.row_checked(&package));
    assert!(app.settings.cleanup.excluded.len() == 1, "only sccache");
    app.set_row_checked(&package, true);
    assert!(app.row_checked(&package));
}

#[test]
fn a_clean_filter_with_no_usable_source_reads_nothing() {
    let mut app = app();
    app.catalog = serde_json::from_value(json!([
        {"source":"apt", "available":true, "capabilities":["installed","upgrade","clean"]},
        {"source":"dev-caches", "available":true, "capabilities":["clean"]}
    ]))
    .unwrap();
    // Sources turned off for updates still count on Clean.
    app.enabled = ["apt".into()].into();
    assert_eq!(app.sources_csv(Page::Clean), "");
    assert_eq!(app.sources_csv(Page::Updates), "apt");
    app.set_page_sources(Page::Clean, ["dev-caches".into()].into());
    assert_eq!(app.sources_csv(Page::Clean), "dev-caches");
    // Choosing every source is the same as no filter.
    app.set_page_sources(Page::Clean, ["apt".into(), "dev-caches".into()].into());
    assert_eq!(app.sources_csv(Page::Clean), "");
    // A filter that matches nothing here reads nothing, not everything.
    app.page_sources.insert(Page::Clean, ["gone".into()].into());
    assert_eq!(
        app.sources_csv(Page::Clean),
        pkgdeck_core::backends::NO_BACKENDS
    );
}

#[test]
fn clean_selected_reviews_only_the_checked_batch_tasks() {
    let mut app = app();
    app.page = Page::Clean;
    let rows = json!([
        {"kind":"cleanup", "name":"one", "source":"dev-caches", "cleanup_key":"one", "cleanup_kind":"package_cache"},
        {"kind":"cleanup", "name":"two", "source":"dev-caches", "cleanup_key":"two", "cleanup_kind":"package_cache"},
        {"kind":"cleanup", "name":"vol", "source":"docker", "cleanup_key":"vol", "cleanup_kind":"data", "cleanup_opt_in":true}
    ]);
    app.c().set_rows(rows.to_string().into());
    app.react();
    app.refresh_items();
    let two: Row = serde_json::from_value(rows[1].clone()).unwrap();
    app.set_row_checked(&two, false);
    // While an older list is still on screen, nothing is reviewed.
    app.retained = Some(app.rows.clone());
    app.clean_selected();
    assert!(app.active_rows.is_empty());
    app.retained = None;
    app.clean_selected();
    assert_eq!(app.active_rows.len(), 1);
    assert!(app.active_rows.contains(
        &serde_json::from_value::<Row>(rows[0].clone())
            .unwrap()
            .identity()
    ));
    // A volume goes once it's checked.
    let vol: Row = serde_json::from_value(rows[2].clone()).unwrap();
    assert!(!app.row_checked(&vol));
    app.set_row_checked(&vol, true);
    app.clean_selected();
    assert_eq!(app.active_rows.len(), 2);
    assert!(app.active_rows.contains(&vol.identity()));
    // Nothing checked: nothing to review.
    let mut app = self::app();
    app.page = Page::Clean;
    app.c().set_rows(json!([rows[2]]).to_string().into());
    app.react();
    app.refresh_items();
    app.clean_selected();
    assert!(app.active_rows.is_empty());
}

#[test]
fn refresh_failure_survives_the_real_worker_completion() {
    use pkgdeck_core::{
        engine::{Backend, EngineError},
        package::*,
    };
    struct FailingRefresh;
    impl Backend for FailingRefresh {
        fn id(&self) -> &str {
            "flatpak"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Installed, Capability::Refresh]
        }
        fn detect(
            &mut self,
            _: &pkgdeck_core::process::Cancellation,
        ) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn installed(
            &mut self,
            _: &pkgdeck_core::process::Cancellation,
        ) -> Result<Vec<Package>, EngineError> {
            Ok(vec![serde_json::from_value(json!({
                "id":{"backend":"flatpak","name":"fixture","architecture":"all","scope":"system"},
                "display_name":"Fixture","summary":"Disposable fixture","installed_version":"1","candidate_version":"2","update":"available"
            })).unwrap()])
        }
        fn execute(
            &mut self,
            _: &Operation,
            _: &pkgdeck_core::process::Cancellation,
            _: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            Err(EngineError::InvalidResponse {
                backend: "flatpak".into(),
                reason: "Fixture package server is unavailable".into(),
            })
        }
    }
    fn failing_engine(
        _: &[String],
        _: bool,
        _: pkgdeck_core::host::Authorization,
        _: &pkgdeck_core::process::Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut engine = Engine::default();
        engine.register(FailingRefresh)?;
        Ok(engine)
    }
    let (store, mut settings) = Store::open(None, None);
    settings.source_list = "flatpak".into();
    settings.background_mode = false;
    let mut app = App::with_controller(
        create_synthetic_controller(failing_engine),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    app.open_page(Page::Updates);
    let finish = |app: &mut App| {
        let until = Instant::now() + Duration::from_secs(5);
        while !app.idle() && Instant::now() < until {
            app.tick(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.idle());
    };
    finish(&mut app);
    assert_eq!(app.rows.iter().filter(|row| row.is_package()).count(), 1);
    app.refresh_page();
    assert!(app.checking_updates);
    finish(&mut app);
    assert!(!app.checking_updates);
    assert_eq!(
        app.read_failures().len(),
        1,
        "completion must preserve the failed index refresh"
    );
    assert_eq!(app.read_failures()[0].source, "flatpak");
    assert!(app.read_failures()[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("Fixture package server"));
    assert_eq!(app.rows.iter().filter(|row| row.is_package()).count(), 1);
    assert!(!app.post_write_reload);
    // A read retaining old rows cannot propose a change from that snapshot.
    app.retained = Some(app.rows.clone());
    app.propose("upgrade-all");
    assert!(!app.busy);
    assert!(app.confirmation.is_none());
    app.retained = None;
}

#[test]
fn refresh_checks_block_changes_and_other_work_keeps_its_existing_rules() {
    let mut app = app();
    let states = [
        (false, false, false, true), // idle
        (true, false, false, false), // loading the list
        (true, true, false, true),   // another write may be queued
        (true, false, true, true),   // background details don't block changes
        (true, true, true, true),
    ];
    for (busy, writing, reading, allowed) in states {
        app.busy = busy;
        app.writing = writing;
        app.reading = reading;
        app.checking_updates = false;
        assert_eq!(app.can_act(), allowed);
        app.checking_updates = true;
        assert!(!app.can_act());
        app.upgrade_updates();
        app.propose("upgrade-all");
        assert!(app.confirmation.is_none());
    }
}

#[test]
fn checks_tell_about_what_automatic_updates_leave_or_say_nothing() {
    let mut app = app();
    // An automatic run installs 3 and leaves 2 for you.
    app.c().set_background_state(
        json!({"available": 5, "notify": true, "manual": 2})
            .to_string()
            .into(),
    );
    app.react();
    assert_eq!(
        app.platform.recorded(),
        [(
            "PkgDeck updates".to_owned(),
            "2 updates available".to_owned()
        )]
    );
    // Do nothing: the badge still counts, and nothing pops up.
    app.settings
        .set_found_updates(crate::settings::FoundUpdates::Nothing);
    app.c()
        .set_background_state(json!({"available": 3, "notify": true}).to_string().into());
    app.react();
    assert_eq!(app.background.available, 3);
    assert_eq!(app.platform.recorded().len(), 1);
}

// The menu bar icon needs a running NSApplication, which tests don't have.
#[cfg(target_os = "linux")]
#[test]
fn the_tray_icon_shows_always_with_updates_or_never() {
    use crate::settings::TrayIcon;
    let mut app = app();
    let shown = |app: &mut App, tray: TrayIcon, available: u64| {
        app.settings.tray_icon = tray;
        app.background.available = available;
        app.sync_tray();
        !app.platform.tray_titles().is_empty()
    };
    assert!(shown(&mut app, TrayIcon::Always, 0));
    assert!(!shown(&mut app, TrayIcon::Updates, 0));
    assert!(shown(&mut app, TrayIcon::Updates, 2));
    assert!(!shown(&mut app, TrayIcon::Never, 2));
    // Without background checks there is no icon at all.
    app.settings.background_mode = false;
    assert!(!shown(&mut app, TrayIcon::Always, 2));
}

#[test]
fn the_last_flatpak_link_scope_is_saved_and_starts_the_next_link() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-link-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (store, settings) = Store::open(Some(dir.clone()), None);
    let mut app = App::with_controller(
        create_synthetic_controller(empty),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    assert!(!app.ctl.flatpak_links_for_system());
    app.choose_flatpak_link_scope(true);
    assert!(app.ctl.flatpak_links_for_system());
    assert!(app.store.saved().flatpak_links_for_system);
    // The next start reads it back and tells the controller.
    let (store, settings) = Store::open(Some(dir.clone()), None);
    assert!(settings.flatpak_links_for_system);
    let mut again = App::with_controller(
        create_synthetic_controller(empty),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    assert!(again.ctl.flatpak_links_for_system());
    again.choose_flatpak_link_scope(false);
    assert!(!again.ctl.flatpak_links_for_system());
    assert!(!again.store.saved().flatpak_links_for_system);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_tell_the_controller_which_sources_to_leave_for_you() {
    let mut app = app();
    app.settings
        .set_found_updates(crate::settings::FoundUpdates::Install);
    app.settings.set_installs_automatically("apt", false);
    // Waiting for the charger and a free connection starts on.
    app.apply_settings();
    assert!(app.c().auto_update());
    assert_eq!(app.ctl.manual_sources(), ["apt"]);
    assert_eq!(
        app.ctl.waits(),
        pkgdeck_core::hold::Waits {
            battery: true,
            metered: true
        }
    );
    app.settings.wait_for_charger = false;
    app.apply_settings();
    assert!(!app.ctl.waits().battery && app.ctl.waits().metered);
}
