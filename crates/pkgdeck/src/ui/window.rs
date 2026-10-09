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
    theme::apply(ctx);
    egui_extras::install_image_loaders(ctx);
    ctx.add_bytes_loader(std::sync::Arc::new(media::WebLoader::new(media)));
}

/// The window around the [`App`] for one run of the toolkit's loop.
pub struct Window<'a> {
    app: &'a mut App,
    smoke_test: Option<u32>,
}

impl<'a> Window<'a> {
    pub fn new(ctx: &egui::Context, app: &'a mut App) -> Self {
        if let Ok(mut guard) = CONTEXT.lock() {
            *guard = Some(ctx.clone());
        }
        app.window_visible = true;
        let smoke_test = app.launch.smoke_test.then_some(0);
        Self { app, smoke_test }
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
        let wait = self.app.tick(Instant::now());
        ui::show(self.app, ui);
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

impl Drop for Window<'_> {
    fn drop(&mut self) {
        self.app.window_visible = false;
        if let Ok(mut guard) = CONTEXT.lock() {
            *guard = None;
        }
    }
}

impl eframe::App for Window<'_> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}
