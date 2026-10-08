//! Renders the screenshots in `docs/screenshots` from sample data, without
//! a display: `cargo run -p pkgdeck --example screenshots`.

use eframe::egui;
use egui_kittest::Harness;
use pkgdeck::{
    app::Launch,
    model::{self, Page},
    platform::Platform,
    settings::{Appearance, Store},
    App,
};
use serde_json::{json, Value};
use std::path::PathBuf;

fn row(name: &str, display: &str, source: &str, extra: Value) -> model::Row {
    let mut value = json!({
        "kind": "package", "name": name, "display_name": display, "source": source,
        "architecture": "x86_64", "scope": "system", "scope_label": "System",
        "installed": null, "candidate": null, "update": "unknown", "summary": "",
    });
    for (key, field) in extra.as_object().unwrap() {
        value[key] = field.clone();
    }
    serde_json::from_value(value).unwrap()
}

fn app() -> App {
    let (store, mut settings) = Store::open(None, None);
    settings.appearance = Appearance::Dark;
    let mut app = App::with_controller(
        pkgdeck_app::controller::ffi::create_synthetic_controller(|_, _, _, _| {
            Ok(pkgdeck_core::engine::Engine::default())
        }),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    app.catalog = serde_json::from_value(json!([
        {"source": "apt", "available": true, "capabilities": ["search", "installed", "upgrade", "clean", "remove"]},
        {"source": "flatpak", "available": true, "capabilities": ["search", "installed", "upgrade", "remove"]},
        {"source": "npm", "available": true, "capabilities": ["search", "installed", "upgrade", "remove"]},
        {"source": "cargo", "available": true, "capabilities": ["search", "installed", "upgrade", "remove"]},
    ]))
    .unwrap();
    app.background =
        serde_json::from_value(json!({"last_check": model::now() - 600, "available": 4})).unwrap();
    app
}

fn search_rows() -> Vec<model::Row> {
    vec![
        row(
            "ripgrep",
            "",
            "apt",
            json!({"candidate": "14.1.1-1", "summary": "Recursively searches directories for a regex pattern"}),
        ),
        row(
            "ripgrep",
            "",
            "cargo",
            json!({"installed": "14.1.0", "candidate": "14.1.1", "update": "available", "summary": "ripgrep is a line-oriented search tool", "scope": {"user": {"uid": 1000}}, "scope_label": "User"}),
        ),
        row(
            "ripgrep-all",
            "",
            "cargo",
            json!({"candidate": "0.10.9", "summary": "rga: ripgrep, but also search in PDFs, E-Books, Office documents, zip, tar.gz, etc.", "scope": {"user": {"uid": 1000}}, "scope_label": "User"}),
        ),
        row(
            "io.github.ripgrep.Gui",
            "Ripgrep GUI",
            "flatpak",
            json!({"candidate": "1.4.0", "remote": "flathub", "summary": "Search your files fast"}),
        ),
        row(
            "fd-find",
            "",
            "apt",
            json!({"candidate": "10.2.0-1", "summary": "Simple, fast and user-friendly alternative to find"}),
        ),
        row(
            "ugrep",
            "",
            "apt",
            json!({"candidate": "7.2.2", "summary": "Faster grep with an interactive query UI"}),
        ),
    ]
}

fn firefox_rows() -> Vec<model::Row> {
    vec![
        row(
            "firefox",
            "Firefox",
            "apt",
            json!({"installed": "131.0.3", "update": "current", "summary": "Safe and easy web browser from Mozilla", "same_app_group": "firefox", "same_app_from": ["flatpak"]}),
        ),
        row(
            "org.mozilla.firefox",
            "Firefox",
            "flatpak",
            json!({"installed": "131.0.3", "update": "current", "remote": "flathub", "reference": "app/org.mozilla.firefox/x86_64/stable", "summary": "Fast, Private & Safe Web Browser", "same_app_group": "firefox", "same_app_from": ["apt"]}),
        ),
        row(
            "gimp",
            "GIMP",
            "apt",
            json!({"installed": "2.10.38", "update": "current", "summary": "GNU Image Manipulation Program"}),
        ),
        row(
            "htop",
            "",
            "apt",
            json!({"installed": "3.3.0", "update": "current", "summary": "Interactive processes viewer"}),
        ),
        row(
            "org.kde.krita",
            "Krita",
            "flatpak",
            json!({"installed": "5.2.6", "update": "current", "remote": "flathub", "summary": "Digital painting, creative freedom"}),
        ),
        row(
            "typescript",
            "",
            "npm",
            json!({"installed": "5.6.3", "update": "current", "summary": "TypeScript is a language for application scale JavaScript"}),
        ),
    ]
}

fn update_rows() -> Vec<model::Row> {
    vec![
        row(
            "openssl",
            "",
            "apt",
            json!({"installed": "3.3.1-2", "candidate": "3.3.2-1", "update": "available", "summary": "Secure Sockets Layer toolkit"}),
        ),
        row(
            "typescript",
            "",
            "npm",
            json!({"installed": "5.6.3", "candidate": "5.6.4", "update": "available", "summary": "TypeScript is a language for application scale JavaScript", "scope_label": "/usr/lib/node_modules"}),
        ),
        row(
            "prettier",
            "",
            "npm",
            json!({"installed": "3.3.2", "candidate": "3.3.3", "update": "available", "summary": "Prettier is an opinionated code formatter", "scope_label": "/usr/lib/node_modules"}),
        ),
        row(
            "eslint",
            "",
            "npm",
            json!({"installed": "9.11.0", "candidate": "9.12.0", "update": "available", "summary": "An AST-based pattern checker for JavaScript", "scope_label": "/usr/lib/node_modules"}),
        ),
    ]
}

fn render(name: &str, size: [f32; 2], prepare: impl FnOnce(&mut App)) {
    let mut app = app();
    prepare(&mut app);
    app.invalidate();
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
    harness.run_steps(60);
    let image = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots");
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

fn main() {
    render("search", [1180.0, 720.0], |app| {
        app.query = "ripgrep".into();
        app.result_query = "ripgrep".into();
        app.rows = search_rows();
    });
    render("details", [1280.0, 780.0], |app| {
        app.page = Page::Installed;
        app.rows = firefox_rows();
        app.refresh_items();
        let raw = 1;
        app.selected = Some(app.rows[raw].identity());
        app.page_open = true;
        app.details = serde_json::from_value(json!({
            "package": {"kind": "package", "name": "org.mozilla.firefox", "display_name": "Firefox", "source": "flatpak", "architecture": "x86_64", "scope": "system", "remote": "flathub", "reference": "app/org.mozilla.firefox/x86_64/stable", "installed": "131.0.3", "summary": "Fast, Private & Safe Web Browser"},
            "description": "Firefox Browser, also known as Mozilla Firefox or simply Firefox, is a free and open-source web browser developed by the Mozilla Foundation. It blocks trackers, keeps your passwords and bookmarks in sync, and updates itself in the background.",
            "homepage": "https://www.mozilla.org/firefox/",
            "publisher": "Mozilla",
            "license": "MPL-2.0",
            "dependencies": ["org.freedesktop.Platform//24.08", "org.freedesktop.Platform.ffmpeg-full"],
        }))
        .unwrap();
    });
    render("updates", [1180.0, 640.0], |app| {
        app.page = Page::Updates;
        app.rows = update_rows();
    });
    render("narrow", [520.0, 820.0], |app| {
        app.page = Page::Installed;
        app.rows = firefox_rows();
    });
    render("repositories", [1180.0, 760.0], |app| {
        app.page = Page::Sources;
        app.rows = vec![];
        app.ui.repos_open = true;
        app.repositories = serde_json::from_value(json!({
            "repositories": [
                {"backend": "flatpak", "name": "flathub", "title": "Flathub", "url": "https://dl.flathub.org/repo/", "scope": "system", "enabled": true, "priority": 1},
                {"backend": "flatpak", "name": "flathub-beta", "title": "Flathub beta", "url": "https://dl.flathub.org/beta-repo/", "scope": "user", "enabled": false, "priority": 0},
                {"backend": "apt", "name": "noble", "title": "Ubuntu noble main restricted", "url": "http://archive.ubuntu.com/ubuntu", "scope": "system", "enabled": true},
                {"backend": "apt", "name": "noble-security", "title": "Ubuntu noble-security main", "url": "http://security.ubuntu.com/ubuntu", "scope": "system", "enabled": true},
                {"backend": "fwupd", "name": "lvfs", "title": "Linux Vendor Firmware Service", "url": "https://cdn.fwupd.org/downloads/firmware.xml.zst", "scope": "system", "enabled": true},
            ],
            "errors": [],
            "features": {"flatpak_user": true, "flatpak_system": true, "apt_editor": true},
        }))
        .unwrap();
    });
}
