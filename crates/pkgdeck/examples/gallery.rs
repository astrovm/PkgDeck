//! Renders each page with sample data to `target/gallery/*.png`, without a
//! display, so the look can be checked by eye:
//! `cargo run -p pkgdeck --example gallery`.

use eframe::egui;
use egui_kittest::Harness;
use pkgdeck::{
    app::{Launch, Toast, ToastAction},
    model::{self, Page, Tone},
    platform::Platform,
    settings::Store,
    App,
};
use serde_json::json;
use std::path::PathBuf;

fn sample_app() -> App {
    let (store, settings) = Store::open(None, None);
    let mut app = App::with_controller(
        pkgdeck_app::controller::ffi::create_synthetic_controller(|_, _, _, _| {
            Ok(pkgdeck_core::engine::Engine::default())
        }),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    let row = |name: &str,
               display: &str,
               source: &str,
               installed: Option<&str>,
               candidate: Option<&str>,
               update: &str,
               summary: &str| {
        serde_json::from_value::<model::Row>(json!({
            "kind": "package", "name": name, "display_name": display, "source": source,
            "architecture": "x86_64", "scope": "system", "scope_label": "System",
            "installed": installed, "candidate": candidate, "update": update, "summary": summary,
            "same_app_from": [], "same_app_group": null,
        }))
        .unwrap()
    };
    app.rows = vec![
        row(
            "org.mozilla.firefox",
            "Firefox",
            "flatpak",
            Some("131.0.2"),
            Some("132.0"),
            "available",
            "Fast, private and safe web browser",
        ),
        row(
            "gimp",
            "GIMP",
            "apt",
            Some("2.10.36"),
            None,
            "current",
            "GNU Image Manipulation Program",
        ),
        row(
            "ripgrep",
            "",
            "cargo",
            Some("14.1.0"),
            Some("14.1.1"),
            "available",
            "Recursively search directories for a regex pattern",
        ),
        row(
            "node",
            "Node.js",
            "homebrew",
            Some("22.9.0"),
            None,
            "current",
            "Platform built on V8 to build network applications",
        ),
        row(
            "htop",
            "",
            "apt",
            Some("3.3.0"),
            None,
            "current",
            "Interactive processes viewer",
        ),
        row(
            "com.visualstudio.code",
            "Visual Studio Code",
            "flatpak",
            Some("1.94.2"),
            None,
            "current",
            "Code editing. Redefined.",
        ),
        row(
            "typescript",
            "",
            "npm",
            Some("5.6.3"),
            Some("5.6.4"),
            "available",
            "TypeScript is a language for application scale JavaScript development",
        ),
        row(
            "uv",
            "",
            "uv",
            Some("0.4.20"),
            None,
            "current",
            "An extremely fast Python package and project manager",
        ),
        row(
            "org.kde.krita",
            "Krita",
            "flatpak",
            None,
            Some("5.2.6"),
            "unknown",
            "Digital painting, creative freedom",
        ),
    ];
    app.catalog = serde_json::from_value(json!([
        {"source": "apt", "name": "APT", "available": true, "capabilities": ["search", "installed", "upgrade", "clean"]},
        {"source": "flatpak", "name": "Flatpak", "available": true, "capabilities": ["search", "installed", "upgrade"]},
        {"source": "cargo", "name": "Cargo", "available": true, "capabilities": ["installed", "upgrade"]},
        {"source": "snap", "name": "Snap", "available": false, "summary": "snapd isn't running"},
    ]))
    .unwrap();
    app.background =
        serde_json::from_value(json!({"last_check": model::now() - 600, "available": 3})).unwrap();
    app.invalidate();
    app
}

fn offline(app: &mut App, page: Page) {
    app.page = page;
    app.invalidate();
}

fn render(name: &str, size: [f32; 2], dark: bool, prepare: impl FnOnce(&mut App)) {
    let mut app = sample_app();
    prepare(&mut app);
    app.settings.appearance = if dark {
        pkgdeck::settings::Appearance::Dark
    } else {
        pkgdeck::settings::Appearance::Light
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(size[0], size[1]))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, (ready, app): &mut (bool, App)| {
                if !*ready {
                    pkgdeck::setup(ui.ctx(), std::path::Path::new("/"), None);
                    *ready = true;
                    return;
                }
                pkgdeck::ui::show(app, ui)
            },
            (false, app),
        );
    harness.run_steps(40);
    let image = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gallery");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

fn main() {
    render("search-empty", [1180.0, 780.0], true, |app| {
        app.rows.clear()
    });
    render("installed", [1180.0, 780.0], true, |app| {
        offline(app, Page::Installed)
    });
    render("installed-light", [1180.0, 780.0], false, |app| {
        offline(app, Page::Installed)
    });
    render("details", [1280.0, 800.0], true, |app| {
        offline(app, Page::Installed);
        app.refresh_items();
        app.selected = Some(app.rows[app.items[0].raw].identity());
        app.page_open = true;
    });
    render("updates", [1180.0, 780.0], false, |app| {
        offline(app, Page::Updates);
        let rows: Vec<model::Row> = app
            .rows
            .iter()
            .filter(|r| r.has_update())
            .cloned()
            .collect();
        app.rows = rows;
        app.invalidate();
    });
    render("clean", [1500.0, 860.0], true, |app| {
        let task = |name: &str, display: &str, source: &str, kind: &str, summary: &str| {
            serde_json::from_value::<model::Row>(json!({
                "kind": "cleanup", "name": name, "display_name": display, "source": source,
                "cleanup_kind": kind, "summary": summary, "preview": "",
            }))
            .unwrap()
        };
        app.rows = vec![
            task(
                "apt-cache",
                "APT download cache",
                "apt",
                "package_cache",
                "104 cached downloads",
            ),
            task(
                "brew-cleanup",
                "Old Homebrew downloads and versions",
                "homebrew",
                "package_cache",
                "Old files that brew cleanup would remove",
            ),
            task(
                "apt-autoremove",
                "Unused dependencies",
                "apt",
                "orphan_dependencies",
                "3 packages nothing needs",
            ),
            task(
                "npm-cache",
                "npm package cache",
                "npm",
                "package_cache",
                "Clear cached downloads",
            ),
        ];
        offline(app, Page::Clean);
        app.refresh_items();
        app.selected = Some(app.rows[app.items[1].raw].identity());
    });
    render("settings", [1180.0, 900.0], true, |app| {
        offline(app, Page::Settings)
    });
    for width in [900.0, 700.0, 460.0] {
        render(&format!("settings-{width}"), [width, 900.0], true, |app| {
            offline(app, Page::Settings)
        });
    }
    render("narrow", [520.0, 760.0], true, |app| {
        offline(app, Page::Installed)
    });
    render("activity", [1180.0, 780.0], true, |app| {
        offline(app, Page::Installed);
        app.drawer_open = true;
        app.activity = serde_json::from_value(json!([
            {"id": 1, "operations": [{"install": {"backend": "flatpak", "name": "org.kde.krita"}}], "labels": ["Install Krita (Flatpak, System)"], "started_at": model::now() - 4000, "finished_at": model::now() - 3900, "state": "finished", "outcomes": []},
            {"id": 2, "operations": [{"upgrade_all": {"backend": "apt"}}], "labels": ["Update all (APT)"], "started_at": model::now() - 60, "state": "running", "outcomes": []}
        ])).unwrap();
    });
    render("confirm", [1180.0, 780.0], false, |app| {
        offline(app, Page::Installed);
        app.confirmation = Some(serde_json::from_value(json!({"action": "Remove GIMP", "summary": "Remove GIMP\nFrom APT, System", "details": "The following packages will be REMOVED:\n  gimp gimp-data"})).unwrap());
        app.confirm_open = true;
        app.toast = Some(Toast {
            text: "Installed Krita".into(),
            tone: Tone::Success,
            action: None,
            shown: std::time::Instant::now(),
            lasts: std::time::Duration::from_secs(60),
            from_notice: false,
        });
    });
    render("toasts", [1180.0, 780.0], false, |app| {
        offline(app, Page::Installed);
        let toast = |text: &str, action| Toast {
            text: text.into(),
            tone: Tone::Success,
            action,
            shown: std::time::Instant::now(),
            lasts: std::time::Duration::from_secs(60),
            from_notice: false,
        };
        app.toast = Some(toast("Update all Flatpak packages finished", None));
        app.restart_toast = Some(toast(
            "PkgDeck was updated. Restart it to use the new version.",
            Some(ToastAction::Restart),
        ));
    });
}
