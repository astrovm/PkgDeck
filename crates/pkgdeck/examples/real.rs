//! Renders the window against this computer's real package managers, read
//! only and without a display, to `target/real/*.png`:
//! `cargo run -p pkgdeck --example real`. Nothing is installed, removed or
//! updated, and background checks stay off.

use eframe::egui;
use egui_kittest::Harness;
use pkgdeck::{
    app::Launch,
    model::Page,
    platform::Platform,
    settings::{Appearance, Settings, Store},
    App,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

type State = (bool, App);

fn step(harness: &mut Harness<'_, State>, frames: usize) {
    for _ in 0..frames {
        harness.state_mut().1.tick(Instant::now());
        harness.step();
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn settle(harness: &mut Harness<'_, State>, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    step(harness, 10);
    while harness.state().1.busy && Instant::now() < deadline {
        step(harness, 5);
    }
    step(harness, 30);
    let app = &harness.state().1;
    eprintln!(
        "{what}: {} rows, {} shown, busy {}",
        app.rows.len(),
        app.items.len(),
        app.busy
    );
}

fn save(harness: &mut Harness<'_, State>, name: &str) {
    let image = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/real");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

fn main() {
    let (store, _) = Store::open(None, None);
    let settings = Settings {
        background_mode: false,
        appearance: Appearance::Dark,
        ..Settings::default()
    };
    let app = App::with_controller(
        pkgdeck_app::controller::ffi::create_controller(),
        settings,
        store,
        Platform::start(|| {}),
        Launch::default(),
    );
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 820.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, (ready, app): &mut State| {
                if !*ready {
                    pkgdeck::setup(ui.ctx(), std::path::Path::new("/"), None);
                    *ready = true;
                    return;
                }
                pkgdeck::ui::show(app, ui)
            },
            (false, app),
        );
    for page in [Page::Installed, Page::Updates, Page::Clean, Page::Sources] {
        harness.state_mut().1.open_page(page);
        settle(&mut harness, page.name());
        save(&mut harness, page.name());
    }
    harness.state_mut().1.open_page(Page::Installed);
    settle(&mut harness, "Installed again");
    harness.state_mut().1.choose(0, true);
    settle(&mut harness, "details");
    step(&mut harness, 60);
    save(&mut harness, "details");
    harness.state_mut().1.open_page(Page::Search);
    harness.state_mut().1.query = "git".into();
    harness.state_mut().1.submit_search();
    settle(&mut harness, "Search");
    save(&mut harness, "Search");
    harness.state_mut().1.open_page(Page::Settings);
    step(&mut harness, 30);
    save(&mut harness, "Settings");
}
