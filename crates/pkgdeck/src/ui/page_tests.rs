//! Exercise the page headers with the real widgets and synthetic controller.
use super::*;
use crate::{
    app::Launch,
    model::{Page, Source},
    platform::Platform,
    settings::Store,
};
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
    platform.record_links();
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
    app.invalidate();
    app.refresh_items();
    if !app.items.is_empty() {
        app.choose(0, true);
    }
    app
}

fn draw(
    app: App,
    row: Row,
    width: f32,
    narrow: bool,
    opened: bool,
) -> Harness<'static, (bool, App)> {
    draw_with_fallback(app, row, width, narrow, opened, false)
}

fn fixture_details(row: &Row) -> Details {
    Details {
        package: Some(row.clone()),
        description: "Fixture description".into(),
        action: if row.is_installed() {
            "Remove"
        } else {
            "Install"
        }
        .into(),
        ..Details::default()
    }
}

fn draw_with_fallback(
    app: App,
    row: Row,
    width: f32,
    narrow: bool,
    opened: bool,
    allow_missing_glyphs: bool,
) -> Harness<'static, (bool, App)> {
    let details = fixture_details(&row);
    draw_details(
        app,
        row,
        details,
        width,
        narrow,
        opened,
        allow_missing_glyphs,
    )
}

fn draw_details(
    mut app: App,
    row: Row,
    details: Details,
    width: f32,
    narrow: bool,
    opened: bool,
    allow_missing_glyphs: bool,
) -> Harness<'static, (bool, App)> {
    if opened {
        app.opened = Some(details.clone());
    }
    let builder = Harness::builder().with_size(vec2(width, 500.0));
    let builder = if allow_missing_glyphs {
        // Header geometry includes arbitrary package names, including emoji
        // outside the bundled faces. Use the application's replacement glyph
        // instead of kittest's new missing-glyph panic or host-specific fonts.
        builder.allow_missing_glyphs()
    } else {
        builder
    };
    let mut harness = builder.build_ui_state(
        move |ui, (ready, app): &mut (bool, App)| {
            if !*ready {
                // This root has no host font directories: snapshots use the
                // bundled faces and egui fallbacks on every OS and architecture.
                let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
                crate::setup(ui.ctx(), &root, None);
                set_reduce_motion(ui.ctx(), true);
                *ready = true;
                return;
            }
            let rect = ui.max_rect();
            if row.is_package() {
                app_page(
                    app,
                    ui,
                    rect,
                    &row,
                    Some(details.clone()),
                    Layout { narrow, opened },
                );
            } else {
                panel(app, ui, rect, &row, narrow);
            }
        },
        (false, app),
    );
    harness.run_steps(4);
    harness
}

#[test]
fn an_opened_link_is_labeled_a_link_and_a_file_a_file() {
    for (location, label) in [
        ("https://example.invalid/app.flatpakref", "Link"),
        ("/home/user/Downloads/app.flatpakref", "File"),
    ] {
        let row = row("package");
        let details = Details {
            location: location.into(),
            ..fixture_details(&row)
        };
        let harness = draw_details(
            app(&row, Page::Search),
            row,
            details,
            700.0,
            false,
            true,
            false,
        );
        assert!(harness.query_by_label(label).is_some(), "{location}");
        assert!(harness.query_by_label(location).is_some(), "{location}");
    }
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
fn an_installed_flatpak_app_opened_from_a_link_can_launch() {
    for launchable in [false, true] {
        let mut row = row("package");
        row.source = "flatpak".into();
        row.installed = Some(String::new());
        let details = Details {
            package: Some(row.clone()),
            launchable,
            ..Details::default()
        };
        let harness = draw_details(
            app(&row, Page::Search),
            row,
            details,
            700.0,
            false,
            true,
            false,
        );
        // Installed with nothing left to do: Launch, or no button at all.
        assert_eq!(harness.query_by_label("Launch").is_some(), launchable);
        assert!(harness.query_by_label("Install").is_none());
    }
}

#[test]
fn an_appimage_file_opened_without_installing_can_still_launch() {
    for runnable in [false, true] {
        let mut row = row("package");
        row.source = "appimage".into();
        row.installed = None;
        let details = Details {
            runnable,
            ..fixture_details(&row)
        };
        let mut harness = draw_details(
            app(&row, Page::Search),
            row,
            details,
            700.0,
            false,
            true,
            false,
        );
        // Install stays the main button; Launch only starts the file.
        assert!(harness.query_by_label("Install").is_some());
        assert_eq!(harness.query_by_label("Launch").is_some(), runnable);
        if runnable {
            harness.get_by_label("Launch").click();
            harness.run_steps(2);
            // Nothing is open in this controller, and the page says so.
            assert_eq!(harness.state().1.launch_error, "Nothing to start.");
        }
    }
}

#[test]
fn the_homepage_and_an_appimage_folder_open_on_the_desktop() {
    let mut row = row("package");
    row.source = "appimage".into();
    let details = Details {
        homepage: "https://example.invalid/fixture".into(),
        ..fixture_details(&row)
    };
    let app = app(&row, Page::Installed);
    let mut harness = draw_details(app, row, details, 700.0, false, false, false);
    // The first frame reads the file's facts; this one is in a folder.
    harness.state_mut().1.app_file.folder = Some("/home/fixture/My Apps".into());
    harness.run_steps(2);
    harness.get_by_label("example.invalid/fixture ↗").click();
    harness.run_steps(2);
    harness.get_by_label("Show in folder").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().1.platform.opened(),
        [
            "https://example.invalid/fixture",
            "file:///home/fixture/My%20Apps"
        ]
    );
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
    for kind in ["cleanup", "package"] {
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
                _ => Page::Installed,
            };
            for opened in [false, true]
                .into_iter()
                .filter(|opened| !*opened || kind == "package")
            {
                if kind == "package" {
                    row.adopt_with = Some("apt".into());
                }
                let harness =
                    draw_with_fallback(app(&row, page), row.clone(), width, narrow, opened, true);
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
fn an_apps_own_data_shows_opens_and_goes_to_the_trash() {
    let mut row = row("package");
    row.source = "flatpak".into();
    let details = Details {
        data: Some(crate::model::AppData {
            folder: "/home/fixture/.var/app/org.example.My App".into(),
            shown: "~/.var/app/org.example.My App".into(),
            size: "1.2 GB".into(),
        }),
        ..fixture_details(&row)
    };
    let identity = row.identity();
    let mut harness = draw_details(
        app(&row, Page::Installed),
        row.clone(),
        details,
        700.0,
        false,
        false,
        false,
    );
    assert!(harness
        .query_by_label("~/.var/app/org.example.My App (1.2 GB)")
        .is_some());
    harness.get_by_label("Open data folder").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().1.platform.opened(),
        ["file:///home/fixture/.var/app/org.example.My%20App"]
    );
    // Moving it is a change like any other: reviewed first, for this row.
    harness.get_by_label("Move data to Trash").click();
    harness.run_steps(2);
    assert!(harness.state().1.active_rows.contains(&identity));
    // Without such a folder, none of it shows.
    let harness = draw_details(
        app(&row, Page::Installed),
        row.clone(),
        fixture_details(&row),
        700.0,
        false,
        false,
        false,
    );
    assert!(harness.query_by_label("Open data folder").is_none());
    assert!(harness.query_by_label("Move data to Trash").is_none());
}
