//! `pkgdeck`: the window. With `--background` it starts in the tray or
//! menu bar; a second launch hands its file or link to the running one.

use pkgdeck::{app, opening, platform, settings, App, Window};
use std::time::{Duration, Instant};

/// Finder and the Dock start apps with only the system PATH, so add
/// Homebrew's and MacPorts' folders for the managers they install.
#[cfg(target_os = "macos")]
fn prepare_macos_environment() {
    let current =
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    for dir in [
        "/opt/local/sbin",
        "/opt/local/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
        "/opt/homebrew/sbin",
        "/opt/homebrew/bin",
    ] {
        if !dirs.iter().any(|existing| existing.as_os_str() == dir) {
            dirs.insert(0, dir.into());
        }
    }
    if let Ok(path) = std::env::join_paths(dirs) {
        std::env::set_var("PATH", path);
    }
}

/// The Homebrew cask links the bundled binary into Homebrew's bin directory.
/// Notifications need the bundle's identifier, which a symlink hides, so
/// run the binary from inside the bundle instead.
#[cfg(target_os = "macos")]
fn exec_from_bundle() {
    use std::os::unix::process::CommandExt;
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Ok(real) = executable.canonicalize() else {
        return;
    };
    let in_bundle = real
        .parent()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"));
    if !in_bundle || real == executable {
        return;
    }
    let error = std::process::Command::new(&real)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("pkgdeck: cannot run {}: {error}", real.display());
    std::process::exit(1);
}

fn parse(args: &[String]) -> app::Launch {
    let mut launch = app::Launch {
        input: opening::input_from(&args[1.min(args.len())..]),
        ..app::Launch::default()
    };
    let known = pkgdeck_core::backends::BACKEND_IDS;
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--background" => launch.background = true,
            "--smoke-test" => launch.smoke_test = true,
            "--auth" => launch.sudo = iter.next().is_some_and(|value| value == "sudo"),
            "--auth=sudo" => launch.sudo = true,
            "--from" => {
                if let Some(id) = iter.next().filter(|id| known.contains(&id.as_str())) {
                    launch.from.push(id.clone());
                }
            }
            other => {
                if let Some(id) = other
                    .strip_prefix("--from=")
                    .filter(|id| known.contains(id))
                {
                    launch.from.push(id.to_owned());
                }
            }
        }
    }
    launch
}

/// At login or after a restart there's nothing to show: a copy already
/// running stays as it is.
fn quiet(launch: &app::Launch) -> bool {
    launch.background && launch.input.is_empty()
}

/// The window's first size: 4:3, as tall as before so it still fits a
/// 1366×768 screen.
const DEFAULT_SIZE: [f32; 2] = [1040.0, 780.0];

/// `decorated` is whether the system draws the window's frame: the older
/// styles draw their own.
fn options(decorated: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PkgDeck")
            .with_decorations(decorated)
            // Glass styles let the desktop show through, blurred.
            .with_transparent(true)
            .with_app_id("io.github.astrovm.PkgDeck")
            .with_inner_size(DEFAULT_SIZE)
            .with_min_inner_size([360.0, 400.0])
            .with_drag_and_drop(true)
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/logo-64.png"))
                    .unwrap_or_default(),
            ),
        run_and_return: true,
        ..Default::default()
    }
}

/// Set on the copy [`start_fresh`] runs, so it carries the last check over.
const FRESH_START: &str = "PKGDECK_FRESH_START";
/// How long the tray stays idle after the window closed before starting
/// fresh, so files being written in the background are finished.
const SETTLE: Duration = Duration::from_secs(2);

/// What a copy started fresh in the tray is given: the same options,
/// without what was opened, and in the background.
fn fresh_args(args: &[String]) -> Vec<String> {
    let input = opening::input_from(&args[1.min(args.len())..]);
    let mut kept: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|arg| *arg != "--background" && (input.is_empty() || **arg != input))
        .cloned()
        .collect();
    kept.push("--background".into());
    kept
}

/// Runs PkgDeck again in this process, in the tray. A new program image
/// leaves behind all a closed window used: graphics libraries, fonts,
/// images and memory the allocator kept. Returns only if that failed.
fn start_fresh(args: &[String]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let program = std::env::current_exe()
        .ok()
        .filter(|path| path.is_file())
        .unwrap_or_else(|| "/proc/self/exe".into());
    let mut command = std::process::Command::new(program);
    if let Some(name) = args.first() {
        command.arg0(name);
    }
    command.args(fresh_args(args)).env(FRESH_START, "1").exec()
}

/// Debug builds have no packaged `pkgdeck-host-runner` beside them, so
/// they start themselves as the one elevated helper for a confirmed batch
/// (see `pkgdeck_core::batch::begin`). Release builds never do.
#[cfg(debug_assertions)]
fn run_as_batch_runner() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|flag| flag == "--batch-runner") {
        let args: Vec<_> = args.collect();
        let failed = pkgdeck_core::unattended::run_runner(&args)
            .inspect_err(|error| eprintln!("pkgdeck-host-runner: {error}"))
            .is_err();
        std::process::exit(i32::from(failed));
    }
}

fn main() {
    #[cfg(debug_assertions)]
    run_as_batch_runner();
    pkgdeck::memory::tune();
    let args: Vec<String> = std::env::args().collect();
    let fresh = std::env::var_os(FRESH_START).is_some();
    // Package managers PkgDeck runs don't need to see it.
    std::env::remove_var(FRESH_START);
    if args.iter().any(|arg| arg == "--version") {
        println!("pkgdeck {}", pkgdeck_core::VERSION);
        return;
    }
    #[cfg(target_os = "macos")]
    {
        exec_from_bundle();
        prepare_macos_environment();
    }
    let launch = parse(&args);
    let var = |key: &str| std::env::var(key).ok();
    let socket = opening::socket_path(var("XDG_RUNTIME_DIR"), rustix::process::geteuid().as_raw());
    let listener = if launch.smoke_test {
        None
    } else {
        match opening::claim(&socket, &launch.input, quiet(&launch), pkgdeck::wake) {
            Ok(None) => return,
            Ok(Some(listener)) => Some(listener),
            Err(_) => None,
        }
    };
    let home = var("HOME")
        .filter(|home| home.starts_with('/'))
        .map(std::path::PathBuf::from);
    let (store, settings) = settings::Store::open(settings::directory(var), home.as_deref());
    let media = (!rustix::process::geteuid().is_root() && var("PKGDECK_NO_CACHE").is_none())
        .then(|| {
            var("XDG_CACHE_HOME")
                .filter(|dir| dir.starts_with('/'))
                .map(std::path::PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".cache")))
        })
        .flatten()
        .map(|dir| dir.join("pkgdeck/media"));
    let mut platform = platform::Platform::start(pkgdeck::wake);
    platform.follow_system_motion();
    pkgdeck_app::wake::set_waker(pkgdeck::wake);
    let start_hidden = launch.background && settings.background_mode && platform.tray_available();
    let controller = pkgdeck_app::controller::ffi::create_controller();
    let mut app = App::with_controller(controller, settings, store, platform, launch);
    app.listener = listener;
    if fresh {
        if let Some(last) = app.background.last_check {
            app.c().resume_checks(last.max(0) as u64);
        }
    }
    // An open window adds the menu bar or tray icon once it shows: making
    // it first only delays the window.
    if start_hidden {
        app.c().set_window_open(false);
        app.sync_tray();
        pkgdeck::memory::release();
    }
    let mut show = !start_hidden;
    // Once a window has closed, start fresh when the tray is idle.
    let mut fresh_due = false;
    loop {
        if show {
            app.platform.watch_reopen(false);
            app.platform.set_dock_visible(true);
            let media = media.clone();
            let result = eframe::run_native(
                "PkgDeck",
                options(app.settings.style == pkgdeck::settings::Style::Modern),
                Box::new(|cc| {
                    pkgdeck::setup(&cc.egui_ctx, std::path::Path::new("/"), media);
                    Ok(Box::new(Window::new(&cc.egui_ctx, &mut app)))
                }),
            );
            if let Err(error) = result {
                eprintln!("pkgdeck: {error}");
                std::process::exit(1);
            }
        }
        if app.force_quit || !app.settings.background_mode || !app.platform.tray_available() {
            app.save_settings();
            return;
        }
        // In the tray: keep checking, wait for the window to be asked back.
        app.platform.watch_reopen(true);
        if show {
            fresh_due = !app.launch.smoke_test;
            pkgdeck::memory::release();
        }
        show = false;
        let mut idle_since: Option<Instant> = None;
        let mut was_idle = true;
        while !show {
            let now = Instant::now();
            let wait = app.tick(now).min(Duration::from_millis(500));
            while let Some(input) = app.next_input() {
                if !input.is_empty() {
                    app.open_input(&input);
                }
                show = true;
            }
            match std::mem::take(&mut app.window_request) {
                app::WindowRequest::Show => show = true,
                app::WindowRequest::Quit => {
                    app.save_settings();
                    return;
                }
                _ => {}
            }
            app.sync_tray();
            let idle = app.idle();
            if idle && !was_idle {
                // A check or change just finished: give back what it used.
                pkgdeck::memory::release();
            }
            was_idle = idle;
            idle_since = if idle { idle_since.or(Some(now)) } else { None };
            if !show && fresh_due && idle_since.is_some_and(|at| now - at >= SETTLE) {
                app.save_settings();
                let error = start_fresh(&args);
                eprintln!("pkgdeck: cannot start fresh in the tray: {error}");
                fresh_due = false;
            }
            if !show {
                let wait = if fresh_due && idle {
                    wait.min(SETTLE)
                } else {
                    wait
                };
                for event in app.platform.wait(wait) {
                    app.handle(event);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_background_launch_with_nothing_to_open_is_quiet() {
        let launch = |args: &[&str]| {
            let mut args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            args.insert(0, "pkgdeck".into());
            quiet(&parse(&args))
        };
        assert!(launch(&["--background"]));
        assert!(!launch(&[]));
        assert!(!launch(&["/tmp/a.flatpakref"]));
        assert!(!launch(&["--background", "/tmp/a.flatpakref"]));
    }

    #[test]
    fn a_fresh_start_keeps_the_options_but_not_what_was_opened() {
        let fresh = |args: &[&str]| {
            let mut args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            args.insert(0, "pkgdeck".into());
            fresh_args(&args)
        };
        assert_eq!(fresh(&[]), ["--background"]);
        // Never twice, and always last.
        assert_eq!(fresh(&["--background"]), ["--background"]);
        assert_eq!(
            fresh(&["--background", "--auth", "sudo", "--from=apt"]),
            ["--auth", "sudo", "--from=apt", "--background"]
        );
        // A file opened at launch was handled; the fresh copy doesn't
        // open it again.
        assert_eq!(
            fresh(&["/tmp/a.flatpakref", "--from", "flatpak"]),
            ["--from", "flatpak", "--background"]
        );
        assert_eq!(
            fresh(&["https://example.com/app.flatpakref"]),
            ["--background"]
        );
        assert!(quiet(&parse(
            &std::iter::once("pkgdeck".to_owned())
                .chain(fresh(&["/tmp/a.deb"]))
                .collect::<Vec<_>>()
        )));
        // The program's own name doesn't count as something to open.
        assert_eq!(fresh_args(&[]), ["--background"]);
    }

    #[test]
    fn the_window_opens_at_4_by_3() {
        let size = options(true).viewport.inner_size.unwrap();
        assert_eq!(size, DEFAULT_SIZE.into());
        assert_eq!(size.x * 3.0, size.y * 4.0);
    }
}
