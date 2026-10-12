//! The window around the [`App`] for one run of the toolkit's loop, and
//! what it needs set up first.

use crate::{
    app::{App, WindowRequest},
    media, theme, ui,
};
use eframe::egui;
use std::{path::Path, sync::Mutex, time::Instant};
/// The window's context while one is open, so other threads can wake it.
static CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Wakes the window if there is one; otherwise the headless loop is
/// already waiting for the same event.
pub fn wake() {
    if let Ok(guard) = CONTEXT.lock() {
        if let Some(ctx) = guard.as_ref() {
            ctx.request_repaint();
        }
    }
}

/// Fonts, colours and image loaders for a new window.
pub fn setup(ctx: &egui::Context, root: &Path, media: Option<std::path::PathBuf>) {
    ctx.set_fonts(theme::fonts(root));
    theme::set_font_root(ctx, root);
    theme::apply(ctx);
    egui_extras::install_image_loaders(ctx);
    ctx.add_bytes_loader(std::sync::Arc::new(media::WebLoader::new(media)));
}

/// The window around the [`App`] for one run of the toolkit's loop.
pub struct Window<'a> {
    app: &'a mut App,
    smoke_test: Option<u32>,
    /// Whether a frame has been drawn, so the window is on screen.
    shown: bool,
    /// Whether the desktop was last asked to blur behind the window.
    blurred: Option<bool>,
}

impl<'a> Window<'a> {
    pub fn new(ctx: &egui::Context, app: &'a mut App) -> Self {
        if let Ok(mut guard) = CONTEXT.lock() {
            *guard = Some(ctx.clone());
        }
        app.window_visible = true;
        app.c().set_window_open(true);
        let smoke_test = app.launch.smoke_test.then_some(0);
        crate::platform::probe_blur(ctx);
        crate::backdrop::allow_wallpaper(wallpaper_cache());
        Self {
            app,
            smoke_test,
            shown: false,
            // A new window starts without blur, so only glass asks.
            blurred: Some(false),
        }
    }

    /// One frame: run what's due, draw, and answer what the frame asked.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        while let Some(input) = self.app.next_input() {
            if !input.is_empty() {
                self.app.open_input(&input);
            }
            self.app.window_request = WindowRequest::Show;
        }
        self.app.dragging = ctx.dragged_id().is_some();
        let wait = self.app.tick(Instant::now());
        ui::show(self.app, ui);
        if self.shown {
            self.app.sync_tray();
        } else {
            self.app.ask_about_setup();
        }
        self.shown = true;
        match ui::window_request(self.app) {
            WindowRequest::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.app.platform.activate();
            }
            WindowRequest::Hide | WindowRequest::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            WindowRequest::None => {}
        }
        if let Some(frames) = &mut self.smoke_test {
            *frames += 1;
            if *frames == 3 {
                let menu = self.app.platform.tray_titles();
                if !menu.is_empty() {
                    println!("PKGDECK_TRAY_MENU {}", menu.join("|"));
                }
                println!("PKGDECK_GUI_READY");
                self.app.force_quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
        ctx.request_repaint_after(wait);
    }
}

/// Where the wallpaper's blurred copy is kept: in PkgDeck's cache, unless
/// running as root or told not to cache.
fn wallpaper_cache() -> Option<std::path::PathBuf> {
    let var = |key| {
        std::env::var(key)
            .ok()
            .filter(|v: &String| v.starts_with('/'))
    };
    if rustix::process::geteuid().is_root() || std::env::var_os("PKGDECK_NO_CACHE").is_some() {
        return None;
    }
    let dir = var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| var("HOME").map(|home| std::path::PathBuf::from(home).join(".cache")))?;
    Some(dir.join("pkgdeck"))
}

/// Asks the desktop to blur behind the window, or to stop: through winit
/// on Wayland and macOS, and through KWin's own property on X11.
fn ask_blur(frame: &eframe::Frame, blur: bool) {
    if let Some(window) = frame.winit_window() {
        window.set_blur(blur);
    }
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let window = frame
            .window_handle()
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
                RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
                _ => None,
            });
        if let Some(window) = window {
            crate::platform::blur_x11(window, blur);
        }
    }
}

impl Drop for Window<'_> {
    fn drop(&mut self) {
        self.app.window_visible = false;
        self.app.c().set_window_open(false);
        if let Ok(mut guard) = CONTEXT.lock() {
            *guard = None;
        }
    }
}

impl eframe::App for Window<'_> {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let blur = crate::backdrop::can_blur()
            && self.app.settings.style.family() == crate::settings::Family::Glass;
        if self.blurred != Some(blur) {
            self.blurred = Some(blur);
            ask_blur(frame, blur);
        }
        theme::set_see_through(blur);
        self.frame(ui);
    }
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}
