use super::*;
use egui_kittest::{kittest::Queryable, Harness};
use pkgdeck_core::{
    engine::Backend,
    package::{Availability, Capability},
};
use std::path::PathBuf;

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
        "5.2",
        "Digital painting",
    );
    krita.id.scope = Scope::User { uid: 1000 };
    krita.id.remote = Some("flathub".into());
    krita.candidate_version = Some("5.3".into());
    krita.update = UpdateAvailability::Available;
    vec![
        package("apt", "firefox", "Firefox", "131.0", "Web browser"),
        krita,
        // No display name: the package name stands in.
        package("apt", "zsh", "", "5.9", "Shell with lots of features"),
    ]
}

/// A package manager that answers from `installed()`; "snap" fails to read.
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
        Ok(PackageDetails {
            description: if id.name == "firefox" {
                String::new()
            } else {
                "Krita is a painting program.".into()
            },
            homepage: (id.name != "firefox").then(|| "https://krita.org".into()),
            dependencies: if id.name == "firefox" {
                vec![]
            } else {
                vec!["org.kde.Platform".into()]
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
    engine_of(&["apt", "flatpak", "snap"], sources)
}
fn apt_only(
    sources: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    engine_of(&["apt"], sources)
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

fn harness(make_engine: MakeEngine) -> Harness<'static, Installed> {
    let harness = Harness::builder()
        .with_size([1100.0, 760.0])
        .build_ui_state(
            |ui, page: &mut Installed| page.ui(ui),
            Installed::new(make_engine),
        );
    egui_extras::install_image_loaders(&harness.ctx);
    harness
}
/// Draw frames until `done`, as the window would while workers run.
fn until(harness: &mut Harness<'static, Installed>, done: impl Fn(&Installed) -> bool) {
    let started = Instant::now();
    while !done(harness.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "timed out");
        harness.step();
        thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
}
fn names(page: &mut Installed) -> Vec<String> {
    page.visible()
        .iter()
        .map(|package| shown_name(package).to_owned())
        .collect()
}

#[test]
fn lists_what_is_installed_sorted_and_filtered() {
    let mut harness = harness(fixture_engine);
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().packages().len(), 3);
    assert_eq!(names(harness.state_mut()), ["Firefox", "Krita", "zsh"]);
    // Each row says where it's from; an update shows under the version.
    harness.get_by_label("Krita");
    harness.get_by_label("Flatpak, flathub, User");
    harness.get_by_label("Update 5.3 available");
    harness.get_by_label("3 packages");
    // A source that couldn't be read says so, and the rest still show.
    assert_eq!(harness.state().failures().len(), 1);
    harness.get_by_label_contains("Snap couldn't be read.");

    // The same column flips the order; another column sorts by it.
    harness.get_by_label("Name ⏶").click();
    harness.run_steps(2);
    assert_eq!(harness.state().sorting(), (SortBy::Name, true));
    assert_eq!(names(harness.state_mut()), ["zsh", "Krita", "Firefox"]);
    harness.get_by_label("Version").click();
    harness.run_steps(2);
    assert_eq!(harness.state().sorting(), (SortBy::Version, false));
    assert_eq!(names(harness.state_mut()), ["Firefox", "Krita", "zsh"]);
    harness.get_by_label("Summary").click();
    harness.run_steps(2);
    assert_eq!(names(harness.state_mut()), ["Krita", "zsh", "Firefox"]);
    harness.get_by_label("Summary ⏶").click();
    harness.run_steps(2);
    harness.get_by_label("Summary ⏷");

    // Filtering matches names and summaries, in any case.
    let filter = harness.get_by_role(egui::accesskit::Role::TextInput);
    filter.focus();
    filter.type_text("PAINT");
    harness.run_steps(2);
    assert_eq!(names(harness.state_mut()), ["Krita"]);
    harness.get_by_label("1 of 3 packages");
    harness.state_mut().set_filter("  nothing like this ");
    harness.run_steps(2);
    harness.get_by_label("Nothing installed matches “nothing like this”.");
    harness.state_mut().set_filter("zsh");
    assert_eq!(names(harness.state_mut()), ["zsh"]);

    // Reload reads everything again.
    harness.state_mut().set_filter("");
    harness.get_by_label("Reload").click();
    harness.run_steps(1);
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().packages().len(), 3);
}

#[test]
fn opening_a_row_shows_its_details_below_the_list() {
    let mut harness = harness(fixture_engine);
    until(&mut harness, |page| !page.loading());
    harness.get_by_label("Krita").click();
    harness.run_steps(1);
    let krita = installed()[1].id.clone();
    assert_eq!(harness.state().selected(), Some(&krita));
    until(&mut harness, |page| page.details().is_some());
    harness.get_by_label("Version 5.2");
    assert_eq!(
        harness.query_all_by_label("Update 5.3 available").count(),
        2
    );
    harness.get_by_label("Krita is a painting program.");
    harness.get_by_label("https://krita.org");
    harness.get_by_label("Dependencies (1)").click();
    harness.run_steps(3);
    harness.get_by_label("org.kde.Platform");
    // Opening the open one again keeps what it has.
    harness.state_mut().open(&krita);
    assert!(harness.state().details().is_some());

    // No description: the summary stands in.
    harness.get_by_label("Firefox").click();
    until(&mut harness, |page| matches!(page.details(), Some(Ok(_))));
    assert_eq!(harness.query_all_by_label("Web browser").count(), 2);
    assert!(harness.query_by_label("Dependencies (0)").is_none());

    // Close, or Esc, goes back to the list alone.
    harness.get_by_label("Close").click();
    harness.run_steps(2);
    assert_eq!(harness.state().selected(), None);
    assert!(harness.query_by_label("Close").is_none());
    harness.get_by_label("Firefox").click();
    harness.run_steps(1);
    assert!(harness.state().selected().is_some());
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert_eq!(harness.state().selected(), None);
}

#[test]
fn details_that_fail_or_arrive_late_are_handled() {
    let mut harness = harness(fixture_engine);
    until(&mut harness, |page| !page.loading());
    // Details that can't be read say why, under what the row knows.
    harness.get_by_label("zsh").click();
    until(&mut harness, |page| page.details().is_some());
    harness.get_by_label_contains("Details couldn't be loaded. no package matches the selection");
    harness.get_by_label("Version 5.9");

    // A lookup for a package that's no longer open fills nothing.
    let firefox = installed()[0].id.clone();
    harness.state_mut().open(&firefox);
    harness.state_mut().selected = Some(installed()[1].id.clone());
    until(&mut harness, |page| page.details_worker.is_none());
    assert!(harness.state().details().is_none());
    // The row's summary shows while details load, in the list and below it.
    assert_eq!(harness.query_all_by_label("Digital painting").count(), 2);

    // A package that's gone after a reload closes its details.
    harness.state_mut().make_engine = apt_only;
    harness.state_mut().reload();
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().selected(), None);
    assert_eq!(names(harness.state_mut()), ["Firefox", "zsh"]);
    // One that stays keeps them.
    harness.state_mut().open(&firefox);
    harness.state_mut().reload();
    until(&mut harness, |page| !page.loading());
    assert_eq!(harness.state().selected(), Some(&firefox));
    // A selection the list doesn't have draws no details.
    harness.state_mut().selected = Some(installed()[1].id.clone());
    harness.run_steps(2);
    assert!(harness.query_by_label("Close").is_none());
}

#[test]
fn says_when_nothing_could_be_read_or_nothing_is_installed() {
    let mut harness = harness(broken);
    until(&mut harness, |page| !page.loading());
    assert_eq!(
        harness.state().error(),
        Some("no package matches the selection")
    );
    harness.get_by_label_contains("Couldn't read installed packages.");

    let mut harness = harness_with(nothing);
    harness.get_by_label("Reading installed packages…");
    until(&mut harness, |page| !page.loading());
    harness.get_by_label("Nothing installed was found.");
    harness.get_by_label("0 packages");
}
fn harness_with(make_engine: MakeEngine) -> Harness<'static, Installed> {
    // Holds the first frame until rows arrive, to show the reading state.
    let mut page = Installed::new(make_engine);
    page.list = Some(spawn(|_, _: &mut dyn FnMut(ListReply)| {}));
    let mut harness = Harness::builder()
        .with_size([800.0, 600.0])
        .build_ui_state(|ui, page: &mut Installed| page.ui(ui), page);
    harness.run_steps(1);
    harness.state_mut().reload();
    harness
}

#[test]
fn rows_show_icons_and_where_packages_come_from() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-egui-icon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let icon = dir.join("icon.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([47, 104, 216, 255]))
        .save(&icon)
        .unwrap();
    let mut harness = harness(fixture_engine);
    until(&mut harness, |page| !page.loading());
    let page = harness.state_mut();
    let mut with_icon = page.packages()[0].clone();
    with_icon.icon = Some(icon);
    page.packages[0] = with_icon.clone();
    page.open(&with_icon.id);
    until(&mut harness, |page| page.details().is_some());
    assert_eq!(harness.query_all_by_label("Firefox").count(), 2);
    std::fs::remove_dir_all(dir).unwrap();

    let mut id = installed()[0].id.clone();
    id.scope = Scope::Environment {
        path: PathBuf::from("/opt/tools"),
    };
    assert_eq!(source_line(&id), "APT, /opt/tools");
    let mut same = installed()[1].clone();
    same.candidate_version = same.installed_version.clone();
    assert_eq!(update_line(&same).as_deref(), Some("Update available"));
    same.candidate_version = None;
    assert_eq!(update_line(&same).as_deref(), Some("Update available"));
    assert_eq!(update_line(&installed()[0]), None);
}

#[test]
fn the_window_draws_the_page_and_a_smoke_test_closes_it() {
    for smoke_test in [false, true] {
        let ctx = egui::Context::default();
        let window = Window::new(&ctx, nothing, smoke_test);
        let mut harness = Harness::builder().build_ui_state(
            |ui, window: &mut Window| assert_eq!(window.frame(ui), window.smoke_test),
            window,
        );
        harness.run_steps(1);
        harness.get_by_label("Installed");
        // PkgDeck's accent is the link and selection colour, in both themes.
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let accent = ctx.style_of(theme).visuals.hyperlink_color;
            assert_eq!(ctx.style_of(theme).visuals.selection.stroke.color, accent);
        }
    }
}

#[test]
fn ties_sort_by_name_then_by_source() {
    let mut page = Installed::new(nothing);
    let flatpak = package(
        "flatpak",
        "org.mozilla.firefox",
        "Firefox",
        "131.0",
        "Web browser",
    );
    let apt = package("apt", "firefox", "Firefox", "131.0", "Web browser");
    let chromium = package("apt", "chromium", "Chromium", "131.0", "Web browser");
    page.show_report(PackageReport {
        packages: vec![flatpak.clone(), apt.clone(), chromium],
        ..Default::default()
    });
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
