//! Renders each page with sample data to `target/gallery/*.png`, without a
//! display, so the look can be checked by eye:
//! `cargo run -p pkgdeck --example gallery`. `--only <text>` renders just
//! the shots whose name has `text`; `--styles`, `--mixes` and
//! `--accents` render more.

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
    if name == "cleanup-settings" {
        harness
            .ctx
            .data_mut(|data| data.insert_temp(egui::Id::new("cleanup-choices-open"), true));
    }
    harness.run_steps(40);
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
            Style::Aqua,
            false,
            "#fff3b0",
            0.0,
            Density::Roomy,
            Typeface::Inter,
            TextSize::Normal,
        ),
        (
            Style::Aqua,
            true,
            "#ff8a00",
            2.0,
            Density::Compact,
            Typeface::Inter,
            TextSize::Small,
        ),
        (
            Style::Fluent,
            true,
            "#60cdff",
            1.0,
            Density::Normal,
            Typeface::Style,
            TextSize::Normal,
        ),
        (
            Style::Fluent,
            false,
            "#c42b1c",
            2.0,
            Density::Roomy,
            Typeface::Inter,
            TextSize::Large,
        ),
        (
            Style::Liquid,
            false,
            "#007aff",
            1.0,
            Density::Normal,
            Typeface::Style,
            TextSize::Normal,
        ),
        (
            Style::Liquid,
            true,
            "#ff375f",
            0.0,
            Density::Compact,
            Typeface::System,
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

/// Settings and a chosen row in every style, mode and accent, plus a
/// colour picked by hand and each of Modern's themes. Only these with
/// `--accents`.
fn accents() {
    use pkgdeck::settings::{Accent, DarkTheme, LightTheme, Style};
    let shot = |name: String, dark: bool, set: &dyn Fn(&mut App)| {
        render(&format!("{name}-settings"), [1180.0, 760.0], dark, |app| {
            set(app);
            offline(app, Page::Settings);
        });
        render(&format!("{name}-selected"), [1180.0, 520.0], dark, |app| {
            set(app);
            offline(app, Page::Installed);
            app.refresh_items();
            app.selected = Some(app.rows[app.items[1].raw].identity());
        });
    };
    for style in Style::ALL {
        let name = style.name().to_lowercase().replace(' ', "-");
        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            for accent in Accent::ALL.into_iter().chain([Accent::Custom]) {
                let label = accent.name().to_lowercase();
                shot(format!("accent-{name}-{mode}-{label}"), dark, &|app| {
                    app.settings.style = style;
                    app.settings.accent = accent;
                    app.settings.custom_accent = "#e8c547".into();
                });
            }
        }
    }
    for theme in [DarkTheme::Charcoal, DarkTheme::Black, DarkTheme::Slate] {
        shot(
            format!("theme-dark-{theme:?}").to_lowercase(),
            true,
            &|app| {
                app.settings.dark_theme = theme;
            },
        );
    }
    for theme in [LightTheme::Classic, LightTheme::White, LightTheme::Paper] {
        shot(
            format!("theme-light-{theme:?}").to_lowercase(),
            false,
            &|app| {
                app.settings.light_theme = theme;
            },
        );
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--mixes") {
        return mixes();
    }
    if std::env::args().any(|arg| arg == "--accents") {
        return accents();
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
            // Short enough that the page scrolls, to see the bar.
            render(
                &format!("style-{name}-scroll-{mode}"),
                [900.0, 420.0],
                dark,
                |app| {
                    app.settings.style = style;
                    app.settings.accent = style.accent();
                    offline(app, Page::Settings);
                },
            );
            // Nothing to update: the empty state and its button.
            render(
                &format!("style-{name}-updates-{mode}"),
                [900.0, 600.0],
                dark,
                |app| {
                    app.settings.style = style;
                    app.settings.accent = style.accent();
                    offline(app, Page::Updates);
                    app.rows.clear();
                    app.invalidate();
                },
            );
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
        let task = |name: &str,
                    display: &str,
                    source: &str,
                    kind: &str,
                    summary: &str,
                    bytes: u64| {
            serde_json::from_value::<model::Row>(json!({
                    "kind": "cleanup", "name": name, "display_name": display, "source": source,
                    "cleanup_key": name, "cleanup_kind": kind, "summary": summary, "preview": "", "bytes": bytes,
                }))
                .unwrap()
        };
        app.rows = vec![
            task(
                "sccache",
                "sccache compiler cache",
                "dev-caches",
                "package_cache",
                "Manages its own size limit",
                2_000_000_000,
            ),
            task(
                "worktrees:/fake/project",
                "project worktrees",
                "projects",
                "worktree",
                "2 finished worktrees",
                3_000_000_000,
            ),
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
        app.rows[3].preview =
            "Empties these folders:\n~/.cargo/registry/cache\n~/.cargo/registry/src\n~/.cargo/git/checkouts".into();
        offline(app, Page::Clean);
        app.refresh_items();
        app.selected = Some(app.rows[app.items[1].raw].identity());
    });
    for tab in pkgdeck::ui::SettingsTab::ALL {
        let name = tab.name().to_lowercase();
        render(&format!("settings-{name}"), [1180.0, 1100.0], true, |app| {
            offline(app, Page::Settings);
            app.ui.settings_tab = tab;
        });
    }
    for width in [900.0, 700.0, 460.0] {
        for tab in [
            pkgdeck::ui::SettingsTab::Appearance,
            pkgdeck::ui::SettingsTab::Clean,
        ] {
            let name = tab.name().to_lowercase();
            render(
                &format!("settings-{name}-{width}"),
                [width, 1700.0],
                true,
                |app| {
                    offline(app, Page::Settings);
                    app.ui.settings_tab = tab;
                },
            );
        }
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
    for (name, confirmation) in [
        (
            "confirm-update",
            json!({"action": "Update", "summary": "Update 7 selected packages\nIf one update fails, the others can still finish.", "items": [
                {"group": "npm", "icon_name": "npm", "title": "@anthropic-ai/claude-code", "from": "2.1.3", "to": "2.1.4"},
                {"group": "Flatpak", "icon_name": "flatpak", "title": "Krita", "from": "5.2.9", "to": "5.2.11"},
                {"group": "Flatpak", "icon_name": "flatpak", "title": "Telegram Desktop", "from": "5.6.3", "to": "5.6.4"},
                {"group": "APT", "icon_name": "apt", "title": "libssl3", "from": "3.0.13-1", "to": "3.0.15-1"},
                {"group": "APT", "icon_name": "apt", "title": "openssh-client", "from": "1:9.6p1-3", "to": "1:9.6p1-4"},
                {"group": "APT", "icon_name": "apt", "title": "curl", "from": "8.5.0-2", "to": "8.5.0-3"},
                {"group": "Firmware", "icon_name": "fwupd", "title": "UEFI dbx", "from": "371", "to": "417", "note": "Revokes old boot loaders"}
            ]}),
        ),
        (
            "confirm-update-apt",
            json!({"action": "Update", "summary": "Update 7 packages", "items": [
                {"group": "APT", "icon_name": "apt", "title": "ffmpeg", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"},
                {"group": "APT", "icon_name": "apt", "title": "libavcodec62", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"},
                {"group": "APT", "icon_name": "apt", "title": "libavcodec62 (i386)", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"},
                {"group": "APT", "icon_name": "apt", "title": "libavformat62", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"},
                {"group": "APT", "icon_name": "apt", "title": "libavfilter11", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"},
                {"group": "APT", "icon_name": "apt", "title": "libswscale9", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"},
                {"group": "APT", "icon_name": "apt", "title": "libswresample6", "from": "7:8.0.1-3ubuntu2+esm4", "to": "7:8.0.1-3ubuntu2+esm5"}
            ]}),
        ),
        (
            "confirm-update-short",
            json!({"action": "Update", "summary": "Update 2 selected packages", "items": [
                {"group": "npm", "icon_name": "npm", "title": "@anthropic-ai/claude-code", "from": "2.1.3", "to": "2.1.4"},
                {"group": "Flatpak", "icon_name": "flatpak", "title": "Telegram Desktop", "from": "5.6.3", "to": "5.6.4"}
            ]}),
        ),
        (
            "confirm-refresh",
            json!({"action": "Refresh", "summary": "Refresh 3 sources", "items": [
                {"icon_name": "apt", "title": "APT"}, {"icon_name": "flatpak", "title": "Flatpak"}, {"icon_name": "fwupd", "title": "Firmware"}
            ]}),
        ),
        (
            "confirm-clean",
            json!({"action": "Clean 2 tasks", "summary": "Clean 2 tasks\nFrees about 4.2 GB.\nFinished tasks cannot be undone.", "items": [
                {"group": "APT", "icon_name": "apt", "title": "Package cache", "size": "3.1 GB", "preview": "Would remove 412 files in /var/cache/apt/archives"},
                {"group": "Docker", "icon_name": "docker", "title": "Unused images", "size": "1.1 GB", "preview": "Deleted Images:\nuntagged: nginx:1.25"}
            ]}),
        ),
    ] {
        render(name, [1180.0, 780.0], true, |app| {
            offline(app, Page::Installed);
            app.confirmation = Some(serde_json::from_value(confirmation).unwrap());
            app.confirm_open = true;
            // Long lists start closed, so open them.
            app.ui.confirm_details_flipped =
                name.starts_with("confirm-update") && !name.ends_with("short");
        });
    }
    render("notice-error", [1000.0, 420.0], true, |app| {
        offline(app, Page::Updates);
        app.c().set_notice(json!({"kind": "error", "title": "2 of 6 changes failed", "detail": "Refresh Firmware package lists: Firmware couldn't run: Error executing command as another user: Not authorized.\nRefresh Snap package lists: Snap couldn't run: Error executing command as another user: Not authorized.", "output": "Refresh Firmware package lists\nError executing command as another user: Not authorized\n\nThis incident has been reported.", "retry": true}).to_string().into());
        app.react();
        app.ui.notice_output = true;
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
