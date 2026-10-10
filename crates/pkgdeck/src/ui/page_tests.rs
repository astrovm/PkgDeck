//! Exercise the page headers with the real widgets and synthetic controller.
use super::*;
use crate::{app::Launch, model::Source, platform::Platform, settings::Store};
use egui_kittest::{
    kittest::{NodeT, Queryable},
    Harness,
};
use pkgdeck_app::controller::ffi::create_synthetic_controller;
use serde_json::{json, Value};

fn row(kind: &str) -> Row {
    serde_json::from_value(json!({
        "kind": kind, "name": "fixture", "display_name": "Fixture package",
        "source": "apt", "scope": "system", "installed": "1.0",
        "available": true, "summary": "Fixture summary", "preview": "Fixture preview",
        "cleanup_kind": "package_cache", "capabilities": ["search", "install"]
    }))
    .unwrap()
}

fn app(row: &Row, page: Page) -> App {
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
    app.catalog = ["apt", "flatpak"]
        .map(|source| Source {
            source: source.into(),
            available: true,
            capabilities: vec!["search".into(), "install".into(), "remove".into()],
            ..Source::default()
        })
        .to_vec();
    app.rows = vec![row.clone()];
    app.page = page;
    app.show_unavailable = true;
    app.invalidate();
    app.refresh_items();
    if !app.items.is_empty() {
        app.choose(0, true);
    }
    app
}

fn draw(
    mut app: App,
    row: Row,
    width: f32,
    narrow: bool,
    opened: bool,
) -> Harness<'static, (bool, App)> {
    let details = Details {
        package: Some(row.clone()),
        description: "Fixture description".into(),
        action: if row.is_installed() {
            "Remove"
        } else {
            "Install"
        }
        .into(),
        ..Details::default()
    };
    if opened {
        app.opened = Some(details.clone());
    }
    let mut harness = Harness::builder()
        .with_size(vec2(width, 500.0))
        .build_ui_state(
            move |ui, (ready, app): &mut (bool, App)| {
                if !*ready {
                    // This root has no host font directories: snapshots use the
                    // bundled faces and egui fallbacks on every OS and architecture.
                    let root =
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
                    crate::setup(ui.ctx(), &root, None);
                    set_reduce_motion(ui.ctx(), true);
                    *ready = true;
                    return;
                }
                let rect = ui.max_rect();
                if row.is_package() {
                    app_page(app, ui, rect, &row, Some(details.clone()), narrow, opened);
                } else {
                    panel(app, ui, rect, &row, narrow);
                }
            },
            (false, app),
        );
    harness.run_steps(4);
    harness
}

fn enabled(harness: &Harness<'_, (bool, App)>, label: &str) -> bool {
    !harness.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn cleanup_headers_have_one_action_and_close_back_to_the_list() {
    for (width, narrow) in [(440.0, true), (700.0, false)] {
        let row = row("cleanup");
        let mut harness = draw(app(&row, Page::Clean), row, width, narrow, false);
        assert!(enabled(&harness, "Clean"));
        assert!(harness.query_by_label("Enabled").is_none());
        let clean = harness.get_by_label("Clean").rect();
        let title = harness.get_by_label("Fixture package").rect();
        assert!(clean.left() >= title.right());
        assert!(clean.top() < title.bottom());
        assert!(clean.right() <= width);
        harness
            .get_by_label(if narrow { "Back" } else { "Close details" })
            .click();
        harness.run_steps(2);
        assert!(harness.state().1.selected.is_none());
        if !narrow {
            assert!(harness.state().1.ui.focus_list);
        }
    }
}

#[test]
fn cleanup_waits_for_busy_or_retained_state_and_only_cleans_a_selection() {
    let row = row("cleanup");
    let mut harness = draw(app(&row, Page::Clean), row, 700.0, false, false);
    harness.get_by_label("Clean").click();
    harness.run_steps(2);
    assert!(!harness.state().1.active_rows.is_empty());
    for (busy, retained, expected) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        harness.state_mut().1.busy = busy;
        let rows = harness.state().1.rows.clone();
        harness.state_mut().1.retained = retained.then_some(rows);
        harness.run_steps(2);
        assert_eq!(enabled(&harness, "Clean"), expected);
    }
    harness.state_mut().1.selected = None;
    harness.run_steps(2);
    assert!(harness.query_by_label("Clean").is_none());
}

#[test]
fn source_header_switch_respects_availability_writing_and_the_last_source() {
    for available in [None, Some(false), Some(true)] {
        for writing in [false, true] {
            for (sources, expected) in [
                (vec!["apt"], false),
                (vec!["apt", "flatpak"], true),
                (vec!["flatpak"], true),
            ] {
                let mut row = row("source");
                row.available = available;
                let mut app = app(&row, Page::Sources);
                app.enabled = sources.into_iter().map(String::from).collect();
                app.writing = writing;
                let harness = draw(app, row, 700.0, false, false);
                assert_eq!(
                    enabled(&harness, "Enabled"),
                    expected && available == Some(true) && !writing
                );
                assert!(harness.query_by_label("Clean").is_none());
            }
        }
    }
}

#[test]
fn source_switch_changes_the_chosen_sources_and_other_panels_have_no_switch() {
    let source_row = row("source");
    let mut harness = draw(
        app(&source_row, Page::Sources),
        source_row,
        700.0,
        false,
        false,
    );
    assert!(harness.query_by_label("On").is_some());
    harness.get_by_label("Enabled").click();
    harness.run_steps(2);
    assert!(!harness.state().1.enabled_sources().contains("apt"));
    assert!(harness.query_by_label("Off").is_some());
    harness.get_by_label("Enabled").click();
    harness.run_steps(2);
    assert!(harness.state().1.enabled_sources().contains("apt"));
    harness.state_mut().1.page = Page::Installed;
    harness.run_steps(2);
    assert!(harness.query_by_label("Enabled").is_none());
    let row = row("failure");
    let harness = draw(app(&row, Page::Sources), row, 700.0, false, false);
    assert!(harness.query_by_label("Enabled").is_none());
    assert!(harness.query_by_label("Clean").is_none());
}

#[test]
fn package_headers_show_actions_for_opened_files_and_both_flatpak_scopes() {
    for installed in [false, true] {
        let mut row = row("package");
        row.installed = installed.then(|| "1.0".into());
        row.candidate = Some("2.0".into());
        row.source = "appimage".into();
        row.adopt_with = Some("appimage".into());
        let harness = draw(app(&row, Page::Installed), row, 700.0, false, true);
        assert!(harness.query_by_label("Back").is_some());
        assert!(harness.query_by_label("Close").is_none());
        assert!(harness.query_by_label("Manage").is_none());
        assert_eq!(harness.query_by_label("Launch").is_some(), installed);
        assert_eq!(harness.query_by_label("Install").is_some(), !installed);
        assert!(harness.query_by_label("Remove").is_none());
    }
    for count in [0, 1, 2] {
        let mut row = row("package");
        row.source = "flatpak".into();
        let mut app = app(&row, Page::Installed);
        app.items[0].variants = (0..count).collect();
        let harness = draw(app, row, 700.0, false, false);
        // The scope selector paints System and User, while the ordinary
        // package header only names its current source and scope.
        let painted = header_geometry(&harness);
        let texts = painted.as_array().unwrap();
        let has_user = texts.iter().any(|entry| entry["text"] == "User");
        assert_eq!(has_user, count > 1);
        let has_merged_label = texts.iter().any(|entry| {
            entry["text"]
                .as_str()
                .is_some_and(|text| text.contains("System and user"))
        });
        assert_eq!(has_merged_label, count > 1);
    }
}

#[test]
fn package_actions_close_and_wait_for_review_or_a_retained_list() {
    let row = row("package");
    for narrow in [false, true] {
        let mut harness = draw(
            app(&row, Page::Installed),
            row.clone(),
            700.0,
            narrow,
            false,
        );
        assert_eq!(harness.query_by_label("Close").is_some(), !narrow);
        assert_eq!(harness.query_by_label("Back").is_some(), narrow);
        for (busy, retained, review, expected) in [
            (false, false, false, true),
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
        ] {
            let app = &mut harness.state_mut().1;
            app.busy = busy;
            app.retained = retained.then(|| app.rows.clone());
            app.review_on_page = review;
            harness.run_steps(2);
            assert_eq!(enabled(&harness, "Remove"), expected);
        }
        harness.state_mut().1.review_on_page = false;
        harness.state_mut().1.busy = false;
        harness.get_by_label("Remove").click();
        harness.run_steps(2);
        assert!(!harness.state().1.active_rows.is_empty());
        harness
            .get_by_label(if narrow { "Back" } else { "Close" })
            .click();
        harness.run_steps(2);
        assert!(!harness.state().1.page_open);
        assert!(harness.state().1.ui.focus_list);
    }
}

#[test]
fn opened_install_actions_work_and_review_keeps_the_back_button_disabled() {
    let mut row = row("package");
    row.installed = None;
    row.candidate = Some("2.0".into());
    let mut harness = draw(app(&row, Page::Search), row, 700.0, false, true);
    harness.get_by_label("Install").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().1.review_on,
        crate::app::ReviewOn::Page("opened".into())
    );
    harness.state_mut().1.review_on_page = true;
    harness.run_steps(2);
    assert!(!enabled(&harness, "Back"));
    assert!(!enabled(&harness, "Install"));
    harness.state_mut().1.review_on_page = false;
    harness.run_steps(2);
    harness.get_by_label("Back").click();
    harness.run_steps(2);
    assert!(harness.state().1.opened.is_none());
}

#[test]
fn manage_is_an_extra_action_for_adoptable_rows_and_no_action_is_invented() {
    let mut row = row("package");
    row.adopt_with = Some("apt".into());
    let mut harness = draw(
        app(&row, Page::Installed),
        row.clone(),
        1000.0,
        false,
        false,
    );
    assert!(harness.query_by_label("Remove").is_some());
    harness.get_by_label("Manage").click();
    harness.run_steps(2);
    assert!(harness.state().1.active_rows.contains(&row.identity()));
    row.source = "macos-apps".into();
    row.adopt_with = None;
    let harness = draw(app(&row, Page::Installed), row, 1000.0, false, false);
    for label in ["Manage", "Remove", "Install", "Launch"] {
        assert!(harness.query_by_label(label).is_none(), "{label}");
    }
}

fn header_geometry(harness: &Harness<'_, (bool, App)>) -> Value {
    fn visit(shape: &egui::Shape, out: &mut Vec<Value>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, out)),
            egui::Shape::Text(text) if text.pos.y < 165.0 => out.push(json!({
                "text": text.galley.text(), "x": text.pos.x, "y": text.pos.y,
                "width": text.galley.size().x, "height": text.galley.size().y,
            })),
            _ => {}
        }
    }
    let mut geometry = Vec::new();
    for shape in &harness.output().shapes {
        visit(&shape.shape, &mut geometry);
    }
    for label in [
        "Back",
        "Close",
        "Close details",
        "Clean",
        "Enabled",
        "Launch",
        "Remove",
        "Install",
        "Manage",
    ] {
        if let Some(node) = harness.query_by_label(label) {
            let rect = node.rect();
            geometry.push(json!({"control": label, "x": rect.min.x, "y": rect.min.y,
                "width": rect.width(), "height": rect.height()}));
        }
    }
    Value::Array(geometry)
}

#[test]
fn page_headers_keep_their_title_and_actions_in_the_reviewed_positions() {
    let mut actual = serde_json::Map::new();
    for kind in ["cleanup", "source", "package"] {
        for (width, narrow) in [
            (440.0, true),
            (480.0, false),
            (500.0, false),
            (504.0, false),
            (560.0, false),
            (600.0, false),
            (700.0, false),
            (1000.0, false),
        ] {
            let mut row = row(kind);
            row.display_name = "A long fixture title with accents áé and emoji 🦀 that must leave room for the header actions".into();
            let page = match kind {
                "cleanup" => Page::Clean,
                "source" => Page::Sources,
                _ => Page::Installed,
            };
            for opened in [false, true]
                .into_iter()
                .filter(|opened| !*opened || kind == "package")
            {
                if kind == "package" {
                    row.adopt_with = Some("apt".into());
                }
                if kind == "source" {
                    row.source =
                        "A long fixture source name that must leave room for its switch".into();
                }
                let harness = draw(app(&row, page), row.clone(), width, narrow, opened);
                actual.insert(
                    format!("{kind}-{width}-{narrow}-{opened}"),
                    header_geometry(&harness),
                );
            }
        }
    }
    let actual = Value::Object(actual);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/page-header-layout.json");
    if std::env::var_os("PKGDECK_UPDATE_LAYOUT").is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        return;
    }
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    fn compare(actual: &Value, expected: &Value, path: &str) {
        match (actual, expected) {
            (Value::Number(a), Value::Number(b)) => assert!(
                (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 0.5,
                "{path}: {a} != {b}"
            ),
            (Value::Array(a), Value::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{path}: missing or extra header content");
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    compare(a, b, &format!("{path}/{i}"));
                }
            }
            (Value::Object(a), Value::Object(b)) => {
                assert_eq!(
                    a.keys().collect::<Vec<_>>(),
                    b.keys().collect::<Vec<_>>(),
                    "{path}"
                );
                for (key, a) in a {
                    compare(a, &b[key], &format!("{path}/{key}"));
                }
            }
            _ => assert_eq!(actual, expected, "{path}"),
        }
    }
    compare(&actual, &expected, "headers");
}

#[test]
fn source_capability_chips_wrap_whole_instead_of_squeezing_their_text() {
    let capabilities = [
        "search",
        "details",
        "installed",
        "install",
        "remove",
        "refresh",
        "upgrade",
    ];
    let mut row = row("source");
    row.capabilities = capabilities.map(String::from).to_vec();
    for width in [300.0, 420.0, 700.0] {
        let harness = draw(app(&row, Page::Sources), row.clone(), width, false, false);
        let heights: Vec<_> = capabilities
            .iter()
            .map(|label| harness.get_by_label(label).rect())
            .inspect(|chip| assert!(chip.right() <= width, "{width}: {chip:?}"))
            .map(|chip| chip.height())
            .collect();
        assert!(
            heights
                .iter()
                .all(|height| (height - heights[0]).abs() < 0.5),
            "{width}: {heights:?}"
        );
        let rows: std::collections::BTreeSet<_> = capabilities
            .iter()
            .map(|label| harness.get_by_label(label).rect().top() as i32)
            .collect();
        assert!(width < 500.0 || rows.len() == 1, "{width}: {rows:?}");
    }
}
