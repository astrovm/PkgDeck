use super::*;
use egui_kittest::{kittest::Queryable, Harness};
use pkgdeck_core::{
    engine::Backend,
    package::{Availability, Capability},
};
use std::{path::PathBuf, time::Duration};
use theme::Palette;

fn package(backend: &str, name: &str, display_name: &str, version: &str, summary: &str) -> Package {
    Package {
        id: PackageId {
            backend: backend.into(),
            name: name.into(),
            architecture: "x86_64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: display_name.into(),
        summary: summary.into(),
        installed_version: Some(version.into()),
        candidate_version: Some(version.into()),
        update: UpdateAvailability::Current,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    }
}
fn installed() -> Vec<Package> {
    let mut krita = package(
        "flatpak",
        "org.kde.krita",
        "Krita",
        "5.2.6",
        "Digital painting, creative freedom",
    );
    krita.id.scope = Scope::User { uid: 1000 };
    krita.id.remote = Some("flathub".into());
    krita.candidate_version = Some("5.3.0".into());
    krita.update = UpdateAvailability::Available;
    let mut prettier = package(
        "npm",
        "prettier",
        "prettier",
        "3.2.5",
        "Prettier is an opinionated code formatter",
    );
    prettier.id.scope = Scope::User { uid: 1000 };
    prettier.candidate_version = None;
    prettier.update = UpdateAvailability::Available;
    vec![
        package(
            "apt",
            "firefox",
            "Firefox",
            "131.0+build1-0ubuntu1",
            "Safe and easy web browser from Mozilla",
        ),
        krita,
        // No display name: the package name stands in.
        package(
            "apt",
            "zsh",
            "",
            "5.9-6ubuntu3",
            "Shell with lots of features",
        ),
        prettier,
        package(
            "cargo",
            "ripgrep",
            "ripgrep",
            "14.1.1",
            "Recursively searches directories for a regex pattern",
        ),
        package(
            "docker",
            "nginx:latest",
            "nginx",
            "1.27.2",
            "Official build of Nginx",
        ),
    ]
}

/// A package manager that answers from `installed()`; "snap" can't be read.
struct Fixture(&'static str);
impl Backend for Fixture {
    fn id(&self) -> &str {
        self.0
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Installed, Capability::Details]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if self.0 == "snap" {
            return Err(EngineError::Unavailable {
                backend: "snap".into(),
                reason: "snapd isn't running".into(),
            });
        }
        Ok(installed()
            .into_iter()
            .filter(|package| package.id.backend == self.0)
            .collect())
    }
    fn details(&mut self, id: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        let package = installed()
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        if id.name == "zsh" {
            return Err(EngineError::NotFound);
        }
        let krita = id.name == "org.kde.krita";
        Ok(PackageDetails {
            description: if krita {
                "Krita is the full-featured digital art studio. It is perfect for sketching and painting, and presents an end-to-end solution for creating digital painting files from scratch by masters.".into()
            } else {
                String::new()
            },
            homepage: krita.then(|| "https://krita.org/".into()),
            dependencies: if krita {
                vec![
                    "org.kde.Platform".into(),
                    "org.freedesktop.Platform.GL.default".into(),
                ]
            } else {
                vec![]
            },
            package,
        })
    }
}
fn engine_of(ids: &[&'static str], sources: &[String]) -> Result<Engine, EngineError> {
    let mut engine = Engine::default();
    for id in ids {
        if sources.is_empty() || sources.iter().any(|source| source == id) {
            engine.register(Fixture(id))?;
        }
    }
    Ok(engine)
}
fn fixture_engine(
    sources: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    engine_of(
        &["apt", "flatpak", "npm", "cargo", "docker", "snap"],
        sources,
    )
}
fn apt_only(
    sources: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    engine_of(&["apt"], sources)
}
fn cargo_only(
    sources: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    engine_of(&["cargo"], sources)
}
fn nothing(
    _: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    Ok(Engine::default())
}
fn broken(
    _: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    Err(EngineError::NotFound)
}

/// The whole window, as people see it.
fn window(make_engine: MakeEngine, size: [f32; 2]) -> Harness<'static, Window> {
    let window = Window::new(&egui::Context::default(), make_engine, false);
    let harness = Harness::builder()
        .with_size(size)
        .build_ui_state(|ui, window: &mut Window| assert!(!window.frame(ui)), window);
    harness
}
/// Draw frames until `done`, as the window would while workers run.
fn until(harness: &mut Harness<'static, Window>, done: impl Fn(&Installed) -> bool) {
    let started = Instant::now();
    while !done(&harness.state().page) {
        assert!(started.elapsed() < Duration::from_secs(10), "timed out");
        harness.step();
        thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
}
fn loaded(make_engine: MakeEngine) -> Harness<'static, Window> {
    let mut harness = window(make_engine, [1200.0, 800.0]);
    until(&mut harness, |page| !page.loading());
    harness
}
fn names(harness: &mut Harness<'static, Window>) -> Vec<String> {
    harness
        .state_mut()
        .page
        .visible()
        .iter()
        .map(|package| shown_name(package).to_owned())
        .collect()
}
fn row(index: usize) -> String {
    row_label(&installed()[index])
}

#[test]
fn lists_what_is_installed_sorted_and_filtered() {
    let mut harness = loaded(fixture_engine);
    assert_eq!(
        names(&mut harness),
        ["Firefox", "Krita", "nginx", "prettier", "ripgrep", "zsh"]
    );
    // Each row reads as one item: name, source, version, update, summary.
    harness.get_by_label(&row(1));
    assert_eq!(
        row(1),
        "Krita, Flatpak, flathub, User, 5.2.6, Update 5.3.0 available, Digital painting, creative freedom"
    );
    assert!(row(3).contains("Update available"));
    harness.get_by_label("6 packages");
    // A source that couldn't be read says so, and the rest still show.
    assert_eq!(harness.state().page.failures().len(), 1);
    harness.get_by_label("Snap couldn't be read: snapd isn't running");
    let other = BackendFailure {
        backend: "apt".into(),
        error: EngineError::NotFound,
    };
    assert_eq!(
        failure_line(&other),
        "APT couldn't be read: no package matches the selection"
    );
    // The sidebar is the Qt app's; Installed is the section shown.
    for section in [
        "Search",
        "Installed",
        "Updates",
        "Clean",
        "Sources",
        "Settings",
    ] {
        harness.get_by_role_and_label(egui::accesskit::Role::Button, section);
    }
    harness.get_by_label("Search").hover();
    harness.run_steps(2);

    // The same column flips the order; another column sorts by it.
    harness.get_by_label("Sort by name").click();
    harness.run_steps(2);
    assert_eq!(harness.state().page.sorting(), (SortBy::Name, true));
    assert_eq!(
        names(&mut harness),
        ["zsh", "ripgrep", "prettier", "nginx", "Krita", "Firefox"]
    );
    harness.get_by_label("Sort by version").click();
    harness.run_steps(2);
    assert_eq!(harness.state().page.sorting(), (SortBy::Version, false));
    assert_eq!(
        names(&mut harness),
        ["nginx", "Firefox", "ripgrep", "prettier", "Krita", "zsh"]
    );
    harness.get_by_label("Sort by summary").click();
    harness.run_steps(2);
    assert_eq!(
        names(&mut harness),
        ["Krita", "nginx", "prettier", "ripgrep", "Firefox", "zsh"]
    );

    // The filter matches names and summaries, in any case.
    let filter = harness.get_by_label("Filter installed packages");
    filter.focus();
    filter.type_text("PAINT");
    harness.run_steps(2);
    assert_eq!(names(&mut harness), ["Krita"]);
    harness.get_by_label("1 of 6 packages");
    // Clear empties it; so does Esc.
    harness.get_by_label("Clear filter").click();
    harness.run_steps(2);
    assert_eq!(harness.state().page.filter(), "");
    harness.state_mut().page.set_filter("  nothing like this ");
    harness.run_steps(2);
    harness.get_by_label("Nothing installed matches “nothing like this”.");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert_eq!(harness.state().page.filter(), "");

    // Reload, or Ctrl+R, reads everything again.
    harness.get_by_label("Reload").click();
    harness.run_steps(1);
    until(&mut harness, |page| !page.loading());
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::R);
    harness.run_steps(1);
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().page.packages().len(), 6);
}

#[test]
fn the_keyboard_moves_through_rows_and_opens_them() {
    let mut harness = loaded(fixture_engine);
    let id = |harness: &Harness<'static, Window>, name: &str| {
        harness
            .state()
            .page
            .packages()
            .iter()
            .find(|package| shown_name(package) == name)
            .map(|package| package.id.clone())
    };
    let cursor = |harness: &Harness<'static, Window>| {
        harness.state().page.cursor().map(|id| id.name.clone())
    };
    // Down starts at the top, Up at the bottom.
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("firefox"));
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("org.kde.krita"));
    harness.key_press(egui::Key::End);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("zsh"));
    harness.key_press(egui::Key::PageUp);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("firefox"));
    harness.key_press(egui::Key::PageDown);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("zsh"));
    harness.state_mut().page.cursor = None;
    harness.key_press(egui::Key::ArrowUp);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("zsh"));
    harness.state_mut().page.cursor = None;
    harness.key_press(egui::Key::Home);
    harness.run_steps(1);
    assert_eq!(cursor(&harness).as_deref(), Some("firefox"));

    // Space opens the highlighted row; with details open, arrows move them.
    harness.key_press(egui::Key::Space);
    harness.run_steps(1);
    assert_eq!(
        harness.state().page.selected(),
        id(&harness, "Firefox").as_ref()
    );
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(1);
    assert_eq!(
        harness.state().page.selected(),
        id(&harness, "Krita").as_ref()
    );
    // Esc closes them.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(1);
    assert_eq!(harness.state().page.selected(), None);

    // Ctrl+F goes to the filter, where Space types instead of opening.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    assert!(harness.ctx.memory(|memory| memory.focused().is_some()));
    harness.key_press(egui::Key::Space);
    harness.run_steps(1);
    assert_eq!(harness.state().page.selected(), None);
    // Enter opens the highlighted row, or the first one.
    harness.key_press(egui::Key::Enter);
    harness.run_steps(1);
    assert_eq!(
        harness.state().page.selected(),
        id(&harness, "Krita").as_ref()
    );
    harness.key_press(egui::Key::Escape);
    harness.state_mut().page.cursor = None;
    harness.key_press(egui::Key::Enter);
    harness.run_steps(1);
    assert_eq!(
        harness.state().page.selected(),
        id(&harness, "Firefox").as_ref()
    );

    // A short window scrolls to keep the highlighted row in view, both ways.
    let mut harness = window(fixture_engine, [1200.0, 420.0]);
    until(&mut harness, |page| !page.loading());
    harness.key_press(egui::Key::End);
    harness.run_steps(3);
    harness.key_press(egui::Key::Home);
    harness.run_steps(3);
    assert_eq!(cursor(&harness).as_deref(), Some("firefox"));
    // Keys on an empty list do nothing.
    harness.state_mut().page.set_filter("nothing like this");
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(1);
    harness.state_mut().page.cursor = None;
    harness.state_mut().page.open_cursor();
    assert_eq!(harness.state().page.selected(), None);
}

#[test]
fn opening_a_row_shows_its_details_below_the_list() {
    let mut harness = window(fixture_engine, [1200.0, 1200.0]);
    until(&mut harness, |page| !page.loading());
    harness.get_by_label(&row(1)).click();
    harness.run_steps(1);
    let krita = installed()[1].id.clone();
    assert_eq!(harness.state().page.selected(), Some(&krita));
    until(&mut harness, |page| page.details().is_some());
    harness.get_by_label_contains("Krita is the full-featured digital art studio.");
    harness.get_by_label("Version");
    harness.get_by_label("5.2.6");
    harness.get_by_label("5.3.0");
    harness.get_by_label("krita.org");
    harness.get_by_label("Dependencies (2)").click();
    harness.run_steps(3);
    harness.get_by_label("org.kde.Platform");
    harness.get_by_label("Dependencies (2)").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("org.kde.Platform").is_none());
    // Opening the open one again keeps what it has.
    harness.state_mut().page.open(&krita);
    assert!(harness.state().page.details().is_some());

    // No description: the summary stands in. An update with no version
    // says it's available.
    harness.get_by_label(&row(3)).click();
    until(&mut harness, |page| matches!(page.details(), Some(Ok(_))));
    harness.get_by_label("Available");
    assert!(harness.query_by_label("Homepage").is_none());
    harness.get_by_label("Prettier is an opinionated code formatter");

    // Drag the handle to give the details more room; double-click resets it.
    let saved = |harness: &Harness<'static, Window>| -> Option<f32> {
        harness
            .ctx
            .data_mut(|data| data.get_persisted(egui::Id::new("installed_details_height")))
    };
    let handle = harness.get_by_label("Resize details").rect().center();
    harness.hover_at(handle);
    harness.run_steps(1);
    harness.drag_at(handle);
    harness.run_steps(1);
    harness.hover_at(handle - egui::vec2(0.0, 80.0));
    harness.run_steps(1);
    harness.drop_at(handle - egui::vec2(0.0, 80.0));
    harness.run_steps(2);
    assert!(saved(&harness).is_some());
    // A clock of its own, so the clicks before can't make it a triple-click.
    for at in [1000.0, 1000.1] {
        harness.input_mut().time = Some(at);
        harness.get_by_label("Resize details").click();
        harness.step();
    }
    harness.input_mut().time = None;
    harness.run_steps(2);
    assert_eq!(saved(&harness), None);

    // Close goes back to the list alone.
    harness.get_by_label("Close details").click();
    harness.run_steps(2);
    assert_eq!(harness.state().page.selected(), None);
    assert!(harness.query_by_label("Close details").is_none());
}

#[test]
fn details_that_fail_or_arrive_late_are_handled() {
    let mut harness = loaded(fixture_engine);
    // Details that can't be read say why, under what the row knows.
    harness.get_by_label(&row(2)).click();
    until(&mut harness, |page| page.details().is_some());
    harness.get_by_label_contains("Details couldn't be loaded. no package matches the selection");
    harness.get_by_label("5.9-6ubuntu3");

    // A lookup for a package that's no longer open fills nothing, and the
    // open one shows it's still loading.
    let firefox = installed()[0].id.clone();
    harness.state_mut().page.open(&firefox);
    harness.state_mut().page.selected = Some(installed()[1].id.clone());
    until(&mut harness, |page| page.details_worker.is_none());
    assert!(harness.state().page.details().is_none());
    harness.get_by_label("Loading details…");

    // A package that's gone after a reload closes its details.
    harness.state_mut().page.make_engine = apt_only;
    harness.state_mut().page.reload();
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().page.selected(), None);
    assert_eq!(names(&mut harness), ["Firefox", "zsh"]);
    // One that stays keeps them.
    harness.state_mut().page.open(&firefox);
    harness.state_mut().page.reload();
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().page.selected(), Some(&firefox));
    // A selection the list doesn't have draws no details.
    harness.state_mut().page.selected = Some(installed()[1].id.clone());
    harness.run_steps(2);
    assert!(harness.query_by_label("Close details").is_none());
}

#[test]
fn says_what_it_is_waiting_for_and_when_nothing_could_be_read() {
    let harness = loaded(broken);
    assert_eq!(
        harness.state().page.error(),
        Some("no package matches the selection")
    );
    harness.get_by_label_contains("Couldn't read installed packages.");

    // While reading, it names the sources it still waits for.
    let mut harness = loaded(nothing);
    harness.get_by_label("Nothing installed was found.");
    harness.get_by_label("0 packages");
    let page = &mut harness.state_mut().page;
    page.list = Some(spawn(None, |_, _: &mut dyn FnMut(ListReply)| {}));
    page.waiting = vec!["apt".into(), "flatpak".into()];
    harness.run_steps(2);
    harness.get_by_label("Reading installed packages…");
    harness.get_by_label("Waiting for APT, Flatpak");
    assert_eq!(harness.state().page.waiting(), ["APT", "Flatpak"]);
    // One package is one package.
    let harness = loaded(cargo_only);
    harness.get_by_label("1 package");
}

#[test]
fn narrow_windows_show_icons_only_and_fewer_columns() {
    let mut harness = window(fixture_engine, [700.0, 640.0]);
    until(&mut harness, |page| !page.loading());
    assert!(harness.query_by_label("Sort by name").is_none());
    harness
        .get_by_role_and_label(egui::accesskit::Role::Button, "Installed")
        .hover();
    harness.run_steps(2);
    harness.get_by_label(&row(0)).click();
    until(&mut harness, |page| page.details().is_some());
}

#[test]
fn rows_show_package_icons_in_both_themes() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-egui-icon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let icon = dir.join("icon.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([47, 104, 216, 255]))
        .save(&icon)
        .unwrap();
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut harness = loaded(fixture_engine);
        harness.ctx.set_theme(theme);
        let page = &mut harness.state_mut().page;
        page.packages[0].icon = Some(icon.clone());
        let firefox = page.packages[0].id.clone();
        page.open(&firefox);
        until(&mut harness, |page| page.details().is_some());
        assert_eq!(
            Palette::current(&harness.ctx),
            Palette::of(theme == egui::Theme::Dark)
        );
    }
    std::fs::remove_dir_all(dir).unwrap();

    let mut id = installed()[0].id.clone();
    id.scope = Scope::Environment {
        path: PathBuf::from("/opt/tools"),
    };
    assert_eq!(source_line(&id), "APT, /opt/tools");
    let mut same = installed()[1].clone();
    same.candidate_version = same.installed_version.clone();
    assert_eq!(update_line(&same).as_deref(), Some("Update available"));
    assert_eq!(update_line(&installed()[0]), None);
}

#[test]
fn every_icon_draws() {
    let mut harness = Harness::new_ui(|ui| {
        for (i, name) in ICON_NAMES.iter().chain(&["unknown-source"]).enumerate() {
            let at = egui::pos2((i % 10) as f32 * 30.0, (i / 10) as f32 * 30.0);
            theme::paint_icon(
                ui.painter(),
                egui::Rect::from_min_size(at, egui::vec2(24.0, 24.0)),
                name,
                egui::Color32::WHITE,
            );
        }
    });
    harness.run();
    assert!(ICON_NAMES.contains(&"package"));
}

#[test]
fn the_systems_fonts_are_used_when_found() {
    let root = std::env::temp_dir().join(format!("pkgdeck-egui-fonts-{}", std::process::id()));
    let bytes = egui::FontDefinitions::default()
        .font_data
        .values()
        .next()
        .unwrap()
        .font
        .to_vec();
    for path in [
        "usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
        "usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        "usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), &bytes).unwrap();
    }
    let fonts = theme::fonts(&root);
    for name in ["system", "system-bold", "system-mono", "other-scripts"] {
        assert!(fonts.font_data.contains_key(name), "{name}");
    }
    assert_eq!(fonts.families[&egui::FontFamily::Proportional][0], "system");
    assert_eq!(fonts.families[&theme::bold_family()][0], "system-bold");
    assert_eq!(
        fonts.families[&egui::FontFamily::Monospace].last().unwrap(),
        "other-scripts"
    );
    // A window draws with them; a smoke test closes after one frame.
    let window = Window::with_fonts(&egui::Context::default(), nothing, true, &root);
    let mut harness = Harness::new_ui_state(
        |ui, (window, closed): &mut (Window, bool)| *closed |= window.frame(ui),
        (window, false),
    );
    harness.run_steps(2);
    assert!(harness.state().1);
    std::fs::remove_dir_all(&root).unwrap();
    // None found: egui's own, with the regular face standing in for bold.
    let fonts = theme::fonts(&root);
    assert!(!fonts.font_data.contains_key("system"));
    assert_eq!(
        fonts.families[&theme::bold_family()],
        fonts.families[&egui::FontFamily::Proportional]
    );
}

#[test]
fn ties_sort_by_name_then_by_source() {
    let mut page = Installed::new(nothing, None);
    let flatpak = package(
        "flatpak",
        "org.mozilla.firefox",
        "Firefox",
        "131.0",
        "Web browser",
    );
    let apt = package("apt", "firefox", "Firefox", "131.0", "Web browser");
    let chromium = package("apt", "chromium", "Chromium", "131.0", "Web browser");
    page.show_report(
        PackageReport {
            packages: vec![flatpak, apt, chromium],
            ..Default::default()
        },
        true,
    );
    page.sort_by(SortBy::Version);
    let order: Vec<_> = page
        .visible()
        .iter()
        .map(|package| package.id.backend.clone() + "/" + &package.id.name)
        .collect();
    assert_eq!(
        order,
        ["apt/chromium", "apt/firefox", "flatpak/org.mozilla.firefox"]
    );
}

#[test]
fn refresh_keeps_pending_sources_and_their_open_details() {
    let mut harness = loaded(fixture_engine);
    let krita = installed()[1].id.clone();
    harness.state_mut().page.open(&krita);
    until(&mut harness, |page| page.details().is_some());
    let apt_report = || PackageReport {
        packages: installed()
            .into_iter()
            .filter(|package| package.id.backend == "apt")
            .collect(),
        successful_sources: vec!["apt".into()],
        ..Default::default()
    };
    harness.state_mut().page.show_report(apt_report(), false);
    assert_eq!(harness.state().page.selected(), Some(&krita));
    assert_eq!(harness.state().page.packages().len(), 6);
    assert!(harness.state().page.details().is_some());
    harness.run_steps(2);
    harness.get_by_label_contains("Krita is the full-featured digital art studio.");
    harness.state_mut().page.show_report(apt_report(), true);
    assert_eq!(harness.state().page.selected(), None);
    assert_eq!(harness.state().page.packages().len(), 2);
}

#[test]
fn enter_after_filtering_opens_a_visible_row() {
    let mut harness = loaded(fixture_engine);
    harness.key_press(egui::Key::End);
    harness.run_steps(1);
    assert_eq!(harness.state().page.cursor().unwrap().name, "zsh");
    harness.get_by_label("Filter installed packages").focus();
    harness
        .get_by_label("Filter installed packages")
        .type_text("PAINT");
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(1);
    assert_eq!(harness.state().page.selected(), Some(&installed()[1].id));
    harness.key_press(egui::Key::Escape);
    harness.run_steps(1);
    harness.get_by_label("Filter installed packages").focus();
    harness
        .get_by_label("Filter installed packages")
        .type_text("nothing");
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(1);
    assert_eq!(harness.state().page.selected(), None);
}

#[test]
fn focused_buttons_handle_enter_without_opening_a_row() {
    let mut harness = loaded(fixture_engine);
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(1);
    harness.get_by_label("Reload").focus();
    harness.run_steps(1);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(1);
    assert_eq!(harness.state().page.selected(), None);
    until(&mut harness, |page| !page.loading());
}

#[test]
fn small_windows_can_scroll_to_the_details_controls() {
    let mut harness = window(apt_only, [480.0, 360.0]);
    until(&mut harness, |page| !page.loading());
    harness.get_by_label(&row(0)).click();
    harness.run_steps(2);
    harness.hover_at(egui::pos2(450.0, 325.0));
    harness.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -500.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(8);
    let close = harness.get_by_label("Close details");
    assert!(close.rect().center().y < 360.0);
    close.click();
    harness.run_steps(2);
    assert_eq!(harness.state().page.selected(), None);
}

#[test]
fn a_failed_refresh_keeps_the_last_list_usable() {
    let mut harness = loaded(fixture_engine);
    harness.state_mut().page.make_engine = broken;
    harness.state_mut().page.reload();
    until(&mut harness, |page| !page.loading());
    harness.get_by_label_contains("Couldn't read installed packages.");
    harness.get_by_label(&row(0)).click();
    harness.run_steps(1);
    assert_eq!(harness.state().page.selected(), Some(&installed()[0].id));
}
