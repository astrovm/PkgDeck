//! What shows behind the window. Glass styles let the desktop show
//! through, blurred, where the desktop can blur behind a window: KWin, on
//! Wayland and X11, and macOS. Elsewhere they paint their own glass.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;

const UNKNOWN: u8 = 0;
const BLUR: u8 = 1;
const NO_BLUR: u8 = 2;

static CAN_BLUR: AtomicU8 = AtomicU8::new(UNKNOWN);
static PROBE: Once = Once::new();

/// Whether the desktop blurs behind windows that ask. False until known.
pub fn can_blur() -> bool {
    CAN_BLUR.load(Ordering::Relaxed) == BLUR
}

/// Sets what [`can_blur`] says, for tests and previews.
pub fn set_can_blur(blur: bool) {
    CAN_BLUR.store(if blur { BLUR } else { NO_BLUR }, Ordering::Relaxed);
}

/// Finds out whether the desktop can blur, once, off the caller's thread,
/// and repaints `ctx` when it knows.
pub fn probe(ctx: &eframe::egui::Context) {
    PROBE.call_once(|| {
        if CAN_BLUR.load(Ordering::Relaxed) != UNKNOWN {
            return;
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // PKGDECK_NO_BLUR turns it off, for desktops whose blur misbehaves.
            let blur = std::env::var_os("PKGDECK_NO_BLUR").is_none()
                && desktop_blurs(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref());
            set_can_blur(blur);
            if blur {
                ctx.request_repaint();
            }
        });
    });
}

#[cfg(target_os = "macos")]
fn desktop_blurs(_desktop: Option<&str>) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn desktop_blurs(desktop: Option<&str>) -> bool {
    blurs_on(desktop, kwin_blur_loaded)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn desktop_blurs(_desktop: Option<&str>) -> bool {
    false
}

/// Whether `desktop`, as `XDG_CURRENT_DESKTOP` names it, blurs behind
/// windows: KDE's, when its blur effect is on.
pub fn blurs_on(desktop: Option<&str>, effect_on: impl FnOnce() -> Option<bool>) -> bool {
    let kde = desktop
        .unwrap_or_default()
        .split(':')
        .any(|name| name.eq_ignore_ascii_case("kde"));
    kde && effect_on().unwrap_or(false)
}

/// Asks KWin whether its blur effect is on.
#[cfg(target_os = "linux")]
fn kwin_blur_loaded() -> Option<bool> {
    let connection = zbus::blocking::Connection::session().ok()?;
    let reply = connection
        .call_method(
            Some("org.kde.KWin"),
            "/Effects",
            Some("org.kde.kwin.Effects"),
            "isEffectLoaded",
            &("blur",),
        )
        .ok()?;
    reply.body().deserialize::<bool>().ok()
}

/// Asks the desktop to blur behind the X11 window `window`, or to stop.
/// KWin reads this property; an empty region means the whole window.
/// Wayland and macOS windows ask through winit instead.
#[cfg(target_os = "linux")]
pub fn blur_x11(window: u32, blur: bool) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    std::thread::spawn(move || {
        let Ok((connection, _)) = x11rb::connect(None) else {
            return;
        };
        let Ok(atom) = connection
            .intern_atom(false, b"_KDE_NET_WM_BLUR_BEHIND_REGION")
            .map_err(drop)
            .and_then(|cookie| cookie.reply().map_err(drop))
        else {
            return;
        };
        let _ = if blur {
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    atom.atom,
                    AtomEnum::CARDINAL,
                    &[],
                )
                .map(drop)
        } else {
            connection.delete_property(window, atom.atom).map(drop)
        };
        let _ = connection.flush();
    });
}

/// A tiny, blurred copy of the desktop's wallpaper, for glass to show
/// where the desktop can't blur behind the window.
pub struct Wallpaper {
    pub size: [usize; 2],
    /// Row by row, opaque.
    pub pixels: Vec<[u8; 4]>,
}

/// How wide the copy is: blurred this small, a few kilobytes.
const WALLPAPER_WIDTH: u32 = 96;

static WALLPAPER: std::sync::Mutex<Option<std::sync::Arc<Wallpaper>>> = std::sync::Mutex::new(None);
static WALLPAPER_LOAD: Once = Once::new();
/// Where the copy is kept between runs, once the window allows reading
/// the wallpaper at all; previews and tests never do.
static WALLPAPER_CACHE: std::sync::Mutex<Option<Option<std::path::PathBuf>>> =
    std::sync::Mutex::new(None);

/// Lets [`wallpaper`] read the desktop's wallpaper, keeping its copy in
/// `cache`.
pub fn allow_wallpaper(cache: Option<std::path::PathBuf>) {
    if let Ok(mut slot) = WALLPAPER_CACHE.lock() {
        *slot = Some(cache);
    }
}

/// The wallpaper's copy once loaded, if allowed. The first call starts
/// loading it off the caller's thread, from the cache when an earlier run
/// made it, and repaints `ctx` when it's ready.
pub fn wallpaper(ctx: &eframe::egui::Context) -> Option<std::sync::Arc<Wallpaper>> {
    let cache = WALLPAPER_CACHE.lock().ok()?.clone()?;
    WALLPAPER_LOAD.call_once(|| {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
            let Some(path) = home.as_deref().and_then(wallpaper_path) else {
                return;
            };
            if let Some(copy) = load_wallpaper(&path, cache.as_deref()) {
                if let Ok(mut slot) = WALLPAPER.lock() {
                    *slot = Some(std::sync::Arc::new(copy));
                }
                ctx.request_repaint();
            }
        });
    });
    WALLPAPER.lock().ok().and_then(|slot| slot.clone())
}

/// The wallpaper's file: Plasma's for the first screen, else GNOME's.
fn wallpaper_path(home: &std::path::Path) -> Option<std::path::PathBuf> {
    let plasma =
        std::fs::read_to_string(home.join(".config/plasma-org.kde.plasma.desktop-appletsrc"))
            .ok()
            .and_then(|text| plasma_wallpaper(&text));
    let path = plasma.or_else(|| {
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.background", "picture-uri"])
            .output()
            .ok()?;
        Some(
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .trim_matches('\'')
                .to_owned(),
        )
    })?;
    let path = std::path::PathBuf::from(path.strip_prefix("file://").unwrap_or(&path));
    if path.is_dir() {
        // A wallpaper package: its largest picture.
        let images = std::fs::read_dir(path.join("contents/images")).ok()?;
        return images
            .flatten()
            .map(|entry| entry.path())
            .max_by_key(|path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0));
    }
    path.is_file().then_some(path)
}

/// The picture Plasma's desktop shows on its first screen, from its
/// `plasma-org.kde.plasma.desktop-appletsrc`.
pub fn plasma_wallpaper(text: &str) -> Option<String> {
    let mut screens: std::collections::HashMap<String, String> = Default::default();
    let mut images: Vec<(String, String)> = Vec::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_owned();
            continue;
        }
        let containment = section
            .strip_prefix("[Containments][")
            .and_then(|rest| rest.split(']').next())
            .unwrap_or_default()
            .to_owned();
        if let Some(screen) = line.strip_prefix("lastScreen=") {
            if section == format!("[Containments][{containment}]") {
                screens.insert(containment.clone(), screen.to_owned());
            }
        }
        if let Some(image) = line.strip_prefix("Image=") {
            if section.ends_with("[Wallpaper][org.kde.image][General]") && !image.is_empty() {
                images.push((containment, image.to_owned()));
            }
        }
    }
    images
        .iter()
        .find(|(id, _)| screens.get(id).map(String::as_str) == Some("0"))
        .or(images.first())
        .map(|(_, image)| image.clone())
}

/// The wallpaper at `path` shrunk and blurred, from `cache` if it was
/// made before from the same file, else made and kept there.
fn load_wallpaper(path: &std::path::Path, cache: Option<&std::path::Path>) -> Option<Wallpaper> {
    use std::hash::{Hash, Hasher};
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (path, modified).hash(&mut hasher);
    let kept = cache.map(|dir| dir.join(format!("backdrop-{:016x}.png", hasher.finish())));
    let small = match kept.as_deref().and_then(|file| image::open(file).ok()) {
        Some(small) => small.to_rgba8(),
        None => {
            let full = image::open(path).ok()?;
            let height = (WALLPAPER_WIDTH as f32 * full.height() as f32
                / full.width().max(1) as f32)
                .round()
                .max(1.0) as u32;
            let small = image::imageops::blur(
                &full.thumbnail_exact(WALLPAPER_WIDTH, height).to_rgba8(),
                2.5,
            );
            if let (Some(file), Some(dir)) = (kept.as_deref(), cache) {
                let _ = std::fs::create_dir_all(dir);
                let _ = small.save(file);
            }
            small
        }
    };
    Some(Wallpaper {
        size: [small.width() as usize, small.height() as usize],
        pixels: small.pixels().map(|p| [p[0], p[1], p[2], 255]).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plasma_shows_the_first_screens_wallpaper() {
        let config = "[Containments][28]\nlastScreen=1\nwallpaperplugin=org.kde.image\n\n\
            [Containments][28][Wallpaper][org.kde.image][General]\nImage=file:///a/second.png\n\n\
            [Containments][29]\nlastScreen=0\n\n\
            [Containments][29][Wallpaper][org.kde.image][General]\nImage=file:///a/first.jpg\n";
        assert_eq!(
            plasma_wallpaper(config).as_deref(),
            Some("file:///a/first.jpg")
        );
        // No screen marked first: the first picture found.
        let one = "[Containments][3][Wallpaper][org.kde.image][General]\nImage=/w/only.png\n";
        assert_eq!(plasma_wallpaper(one).as_deref(), Some("/w/only.png"));
        // Images outside a wallpaper's settings, or empty, don't count.
        assert_eq!(plasma_wallpaper("[General]\nImage=/x.png\n"), None);
        let empty = "[Containments][3][Wallpaper][org.kde.image][General]\nImage=\n";
        assert_eq!(plasma_wallpaper(empty), None);
        assert_eq!(plasma_wallpaper(""), None);
    }

    #[test]
    fn wallpapers_shrink_blur_and_are_kept_for_next_time() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-backdrop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("wall.png");
        // Half red, half blue, 400 by 200.
        let image = image::RgbaImage::from_fn(400, 200, |x, _| {
            if x < 200 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        image.save(&picture).unwrap();
        let cache = dir.join("cache");
        let made = load_wallpaper(&picture, Some(&cache)).unwrap();
        assert_eq!(made.size, [96, 48]);
        assert_eq!(made.pixels.len(), 96 * 48);
        // Blurred where the halves meet, plain at the ends.
        let at = |x: usize| made.pixels[24 * 96 + x];
        assert!(at(2)[0] > 240 && at(2)[2] < 15, "{:?}", at(2));
        assert!(at(48)[0] > 30 && at(48)[2] > 30, "{:?}", at(48));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        // The kept copy is read back without the picture.
        std::fs::write(&picture, b"gone").unwrap();
        let kept = load_wallpaper(&picture, Some(&cache));
        // The picture changed, so the old copy no longer matches it.
        assert!(kept.is_none());
        // Nothing at all to read.
        assert!(load_wallpaper(&dir.join("missing.png"), None).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_kde_with_its_blur_on_blurs() {
        let on = || Some(true);
        assert!(blurs_on(Some("KDE"), on));
        assert!(blurs_on(Some("kde"), on));
        assert!(blurs_on(Some("ubuntu:KDE"), on));
        assert!(!blurs_on(Some("KDE"), || Some(false)));
        // No answer from KWin is no blur.
        assert!(!blurs_on(Some("KDE"), || None));
        for desktop in [
            Some("GNOME"),
            Some("ubuntu:GNOME"),
            Some("XFCE"),
            Some(""),
            None,
        ] {
            assert!(!blurs_on(desktop, on), "{desktop:?}");
        }
        // Other desktops aren't even asked.
        assert!(!blurs_on(Some("GNOME"), || panic!("asked")));
        assert!(!blurs_on(Some("KDEx"), on));
    }
}
