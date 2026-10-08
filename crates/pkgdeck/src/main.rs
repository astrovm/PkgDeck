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

fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PkgDeck")
            .with_app_id("io.github.astrovm.PkgDeck")
            .with_inner_size([1180.0, 780.0])
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
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
        match opening::claim(&socket, &launch.input, pkgdeck::wake) {
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
    let platform = platform::Platform::start(pkgdeck::wake);
    let start_hidden = launch.background && settings.background_mode && platform.tray_available();
    let controller = pkgdeck_app::controller::ffi::create_controller();
    let mut app = App::with_controller(controller, settings, store, platform, launch);
    app.listener = listener;
    app.sync_tray();
    let mut show = !start_hidden;
    loop {
        if show {
            app.platform.watch_reopen(false);
            app.platform.set_dock_visible(true);
            let media = media.clone();
            let result = eframe::run_native(
                "PkgDeck",
                options(),
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
        show = false;
        while !show {
            let wait = app.tick(Instant::now()).min(Duration::from_millis(500));
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
            if !show {
                for event in app.platform.wait(wait) {
                    app.handle(event);
                }
            }
        }
    }
}
