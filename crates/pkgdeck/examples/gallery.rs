//! Renders each page with sample data to `target/gallery/*.png`, without a
//! display, so the look can be checked by eye:
//! `cargo run -p pkgdeck --example gallery`. `--only <text>` renders just
//! the shots whose name has `text`.

use eframe::egui;
use egui_kittest::{kittest::Queryable, Harness};
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
        {"source": "apt", "name": "APT", "available": true, "capabilities": ["search", "installed", "upgrade", "clean", "refresh"]},
        {"source": "flatpak", "name": "Flatpak", "available": true, "capabilities": ["search", "installed", "upgrade"]},
        {"source": "cargo", "name": "Cargo", "available": true, "capabilities": ["installed", "upgrade"]},
        {"source": "snap", "name": "Snap", "available": false, "summary": "snapd isn't running"},
        {"source": "homebrew", "name": "Homebrew", "available": false, "check_failed": true, "summary": "Homebrew took too long to answer."},
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
    render_then(name, size, dark, prepare, None);
}

/// [`render`], clicking what's labelled `click` first.
fn render_then(
    name: &str,
    size: [f32; 2],
    dark: bool,
    prepare: impl FnOnce(&mut App),
    click: Option<&str>,
) {
    let args: Vec<String> = std::env::args().collect();
    if let Some(only) = args
        .iter()
        .position(|arg| arg == "--only")
        .and_then(|i| args.get(i + 1))
    {
        if !name.contains(only.as_str()) {
            return;
        }
    }
    let mut app = sample_app();
    prepare(&mut app);
    // Past the page's fade-in, which runs on the wall clock.
    app.page_changed = std::time::Instant::now() - std::time::Duration::from_secs(1);
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
    if let Some(label) = click {
        harness.get_by_label(label).click();
        harness.run_steps(10);
    }
    let image = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gallery");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

/// Each style with the other options pushed to their ends, to see they
/// mix: a picked colour, corners, room, face and size. Only these with
/// `--mixes`.
fn mixes() {
    use pkgdeck::settings::{Accent, Density, Style, TextSize, Typeface};
    let mixes = [
        (
            Style::Modern,
            false,
            "#00ff66",
            0.0,
            Density::Compact,
            Typeface::System,
            TextSize::Small,
        ),
        (
            Style::Retro,
            true,
            "#ffe14d",
            2.0,
            Density::Roomy,
            Typeface::Inter,
            TextSize::Larger,
        ),
        (
            Style::Luna,
            true,
            "#ff4fa3",
            0.0,
            Density::Compact,
            Typeface::Style,
            TextSize::Large,
        ),
        (
            Style::Glass,
            false,
            "#0b3d0b",
            2.0,
            Density::Roomy,
            Typeface::System,
            TextSize::Small,
        ),
        (
            Style::Gel,
            false,
            "#fff3b0",
            0.0,
            Density::Roomy,
            Typeface::Inter,
            TextSize::Normal,
        ),
        (
            Style::Platinum,
            true,
            "#202020",
            2.0,
            Density::Compact,
            Typeface::Style,
            TextSize::Larger,
        ),
        (
            Style::Win95,
            false,
            "#008080",
            1.0,
            Density::Normal,
            Typeface::Style,
            TextSize::Normal,
        ),
        (
            Style::Vista,
            true,
            "#c03030",
            1.0,
            Density::Compact,
            Typeface::Style,
            TextSize::Normal,
        ),
        (
            Style::Platinum,
            false,
            "#3a7d44",
            0.0,
            Density::Normal,
            Typeface::System,
            TextSize::Large,
        ),
        (
            Style::Panther,
            true,
            "#7a5cff",
            1.0,
            Density::Roomy,
            Typeface::Style,
            TextSize::Normal,
        ),
        (
            Style::Tiger,
            false,
            "#ff8a00",
            2.0,
            Density::Compact,
            Typeface::Inter,
            TextSize::Small,
        ),
    ];
    for (style, dark, color, roundness, density, typeface, size) in mixes {
        let set = |app: &mut App| {
            app.settings.style = style;
            app.settings.accent = Accent::Custom;
            app.settings.custom_accent = color.to_owned();
            app.settings.roundness = roundness;
            app.settings.density = density;
            app.settings.typeface = typeface;
            app.settings.text_size = size;
        };
        let name = style.name().to_lowercase().replace(' ', "-");
        render(
            &format!("mix-{name}-installed"),
            [1180.0, 780.0],
            dark,
            |app| {
                set(app);
                offline(app, Page::Installed);
                app.refresh_items();
                app.selected = Some(app.rows[app.items[1].raw].identity());
            },
        );
        render(
            &format!("mix-{name}-settings"),
            [1180.0, 1100.0],
            dark,
            |app| {
                set(app);
                offline(app, Page::Settings);
            },
        );
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--mixes") {
        return mixes();
    }
    // Each style, light and dark, on a list and on Settings. Only these
    // with `--styles`.
    for style in pkgdeck::settings::Style::ALL {
        let name = style.name().to_lowercase().replace(' ', "-");
        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            render(
                &format!("style-{name}-installed-{mode}"),
                [1180.0, 780.0],
                dark,
                |app| {
                    app.settings.style = style;
                    app.settings.accent = style.accent();
                    offline(app, Page::Installed);
                },
            );
            // A row chosen, as each style marks it.
            render(
                &format!("style-{name}-selected-{mode}"),
                [1180.0, 780.0],
                dark,
                |app| {
                    app.settings.style = style;
                    app.settings.accent = style.accent();
                    offline(app, Page::Installed);
                    app.refresh_items();
                    app.selected = Some(app.rows[app.items[1].raw].identity());
                },
            );
            // The menu bar's View menu, open, where the style has one.
            if matches!(
                style.family(),
                pkgdeck::settings::Family::Retro | pkgdeck::settings::Family::Luna
            ) {
                render_then(
                    &format!("style-{name}-menu-{mode}"),
                    [1180.0, 780.0],
                    dark,
                    |app| {
                        app.settings.style = style;
                        app.settings.accent = style.accent();
                        offline(app, Page::Installed);
                    },
                    Some("View"),
                );
            }
            render(
                &format!("style-{name}-settings-{mode}"),
                [1180.0, 1100.0],
                dark,
                |app| {
                    app.settings.style = style;
                    app.settings.accent = style.accent();
                    offline(app, Page::Settings);
                },
            );
        }
    }
    if std::env::args().any(|arg| arg == "--styles") {
        return;
    }
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
        let task =
            |name: &str, display: &str, source: &str, kind: &str, summary: &str, bytes: u64| {
                serde_json::from_value::<model::Row>(json!({
                    "kind": "cleanup", "name": name, "display_name": display, "source": source,
                    "cleanup_kind": kind, "summary": summary, "preview": "", "bytes": bytes,
                }))
                .unwrap()
            };
        app.rows = vec![
            task(
                "build:~/code/website",
                "website build files",
                "projects",
                "build_files",
                "Unchanged for 3 months",
                2_400_000_000,
            ),
            task(
                "cargo",
                "Cargo downloads",
                "dev-caches",
                "package_cache",
                "Downloaded crates and Git checkouts",
                1_800_000_000,
            ),
            task(
                "unused-images",
                "Unused images",
                "docker",
                "unused",
                "4 images no container uses",
                950_000_000,
            ),
            task(
                "brew-cleanup",
                "Old Homebrew downloads and versions",
                "homebrew",
                "package_cache",
                "Old files that brew cleanup would remove",
                310_000_000,
            ),
            task(
                "unused-volumes",
                "Unused volumes",
                "docker",
                "data",
                "2 volumes no container uses",
                120_000_000,
            ),
            task(
                "apt-autoremove",
                "Unused dependencies",
                "apt",
                "orphan_dependencies",
                "3 packages nothing needs",
                48_000_000,
            ),
        ];
        app.rows[1].preview =
            "Empties these folders:\n~/.cargo/registry/cache\n~/.cargo/registry/src\n~/.cargo/git/checkouts".into();
        offline(app, Page::Clean);
        app.refresh_items();
        app.selected = Some(app.rows[app.items[1].raw].identity());
    });
    render("settings", [1180.0, 2200.0], true, |app| {
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
