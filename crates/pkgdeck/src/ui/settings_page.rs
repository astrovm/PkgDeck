//! Settings: how PkgDeck looks, when it checks for updates, what it is,
//! and its keyboard shortcuts.

use super::widgets::*;
use crate::{
    app::App,
    model::{self, Tone},
    settings::Appearance,
    theme::{self, Palette},
};
use eframe::egui::{self, vec2, CornerRadius, Id, Sense, Stroke, Ui, Vec2};

pub fn show(app: &mut App, ui: &mut Ui) {
    egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).show(ui, |ui| {
        let width = ui.available_width().min(760.0);
        ui.set_max_width(width);
        ui.spacing_mut().item_spacing.y = 6.0;
        appearance(app, ui);
        ui.add_space(8.0);
        update_checks(app, ui);
        ui.add_space(8.0);
        about(app, ui);
        ui.add_space(8.0);
        shortcuts(ui);
        ui.add_space(20.0);
    });
}

fn section(ui: &mut Ui, title: &str, content: impl FnOnce(&mut Ui)) {
    let palette = Palette::current(ui.ctx());
    card(&palette).inner_margin(egui::Margin::symmetric(18, 16)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.set_width(ui.available_width());
        caption(ui, title);
        ui.add_space(4.0);
        content(ui);
    });
}

/// A label with an optional line under it, and a control on the right.
fn setting_row(ui: &mut Ui, label: &str, hint: &str, control: impl FnOnce(&mut Ui)) {
    let palette = Palette::current(ui.ctx());
    let text = |ui: &mut Ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(label).font(theme::font(14.5)).color(palette.ink));
            if !hint.is_empty() {
                ui.label(egui::RichText::new(hint).font(theme::font(12.5)).color(palette.muted));
            }
        });
    };
    if ui.available_width() < 480.0 {
        text(ui);
        control(ui);
    } else {
        ui.horizontal(|ui| {
            text(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
        });
    }
    ui.add_space(10.0);
}

fn appearance(app: &mut App, ui: &mut Ui) {
    section(ui, "Appearance", |ui| {
        setting_row(ui, "Theme", "", |ui| {
            let current = match app.settings.appearance {
                Appearance::System => 0,
                Appearance::Light => 1,
                Appearance::Dark => 2,
            };
            if let Some(choice) = segmented(ui, Id::new("theme"), &["System", "Light", "Dark"], current) {
                app.settings.appearance = [Appearance::System, Appearance::Light, Appearance::Dark][choice];
            }
        });
        setting_row(ui, "Animations", "Movement when things open, close and change", |ui| {
            let mut on = !app.settings.reduce_motion;
            if switch(ui, &mut on, true, "Animations").changed() {
                app.settings.reduce_motion = !on;
            }
        });
    });
}

/// Choices side by side with a sliding marker. Returns a new choice.
fn segmented(ui: &mut Ui, id: Id, labels: &[&str], current: usize) -> Option<usize> {
    let palette = Palette::current(ui.ctx());
    let part = 84.0;
    let (rect, _) = ui.allocate_exact_size(vec2(part * labels.len() as f32, 34.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), palette.hover_solid());
    let x = glide(ui.ctx(), id.with("knob"), rect.left() + current as f32 * part, LAYOUT);
    ui.painter().rect(
        egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(part, rect.height())).shrink(3.0),
        CornerRadius::same(8),
        palette.surface,
        Stroke::new(1.0, palette.line),
        egui::StrokeKind::Inside,
    );
    let mut chosen = None;
    for (index, label) in labels.iter().enumerate() {
        let cell = egui::Rect::from_min_size(egui::pos2(rect.left() + index as f32 * part, rect.top()), vec2(part, rect.height()));
        let response = ui.interact(cell, id.with(index), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, index == current, *label));
        let color = if index == current { palette.ink } else { palette.muted };
        ui.painter().text(cell.center(), egui::Align2::CENTER_CENTER, *label, if index == current { theme::bold(13.5) } else { theme::font(13.5) }, color);
        if response.clicked() && index != current {
            chosen = Some(index);
        }
    }
    chosen
}

fn update_checks(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "Update checks", |ui| {
        let background = app.settings.background_mode;
        setting_row(ui, "Background checks", if cfg!(target_os = "macos") { "Keeps checking from the menu bar after the window closes" } else { "Keeps checking from the system tray after the window closes" }, |ui| {
            let mut on = background;
            if switch(ui, &mut on, true, "Background checks").changed() {
                if !on && app.settings.autostart && !app.c().set_autostart(false) {
                    return;
                }
                if !on {
                    app.settings.autostart = false;
                }
                app.settings.background_mode = on;
                app.sync_tray();
            }
        });
        let interval = model::nearest_interval(app.settings.check_interval);
        setting_row(ui, "Check every", model::INTERVALS[interval].1, |ui| {
            let mut index = interval;
            if steps(ui, Id::new("interval"), model::INTERVALS.len(), &mut index, background, 240.0) {
                let minutes = model::INTERVALS[index].0;
                app.settings.check_interval = minutes;
                app.c().set_check_interval(minutes);
            }
        });
        setting_row(ui, "Install updates automatically", "", |ui| {
            let mut on = app.settings.auto_update;
            if switch(ui, &mut on, background, "Install updates automatically").changed() {
                app.settings.auto_update = on;
                app.c().set_auto_update(on);
            }
        });
        setting_row(ui, "Allow updates that remove packages", "Like an old kernel replaced by a new one", |ui| {
            let mut on = app.settings.allow_removals;
            if switch(ui, &mut on, true, "Allow updates that remove packages").changed() {
                app.settings.allow_removals = on;
                app.c().set_allow_removals(on);
            }
        });
        setting_row(ui, "Allow automatic updates without a password", "Changes you start still ask", |ui| {
            let mut on = !app.settings.system_approval.is_empty();
            if switch(ui, &mut on, true, "Allow automatic updates without a password").changed() {
                app.c().allow_system_updates(on);
                app.react();
            }
        });
        let error = app.ctl.approval_error().to_string();
        if !error.is_empty() {
            ui.label(egui::RichText::new(error).color(palette.danger));
        }
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            setting_row(ui, "Start in background at login", "", |ui| {
                let mut on = app.settings.autostart;
                let can = background && app.platform.tray_available();
                if switch(ui, &mut on, can, "Start in background at login").changed() && app.c().set_autostart(on) {
                    app.settings.autostart = on;
                }
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let text = match app.background.last_check {
                Some(at) => {
                    let count = app.background.available;
                    format!("Last check: {}, {count} update{} found", model::short_datetime(at), if count == 1 { "" } else { "s" })
                }
                None => "Last check: never".into(),
            };
            ui.label(egui::RichText::new(text).color(palette.muted));
            let warning = if !app.platform.tray_available() {
                Some(if cfg!(target_os = "macos") { "No menu bar icon, so no notifications" } else { "No system tray, so no notifications" })
            } else if !app.platform.notifications_available() {
                Some(if cfg!(target_os = "macos") { "Notifications are off in System Settings" } else { "This system tray can't show notifications" })
            } else if app.platform.permission_needed() {
                Some("Notifications show Script Editor's icon until you allow PkgDeck")
            } else {
                None
            };
            if let Some(warning) = warning {
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                theme::paint_icon(ui.painter(), rect, "warning", palette.warning);
                response.on_hover_text(warning);
            }
        });
        ui.horizontal(|ui| {
            if app.platform.permission_needed() && Button::new(Look::Secondary, "Notification settings").small().show(ui).clicked() {
                app.platform.open_notification_settings();
            }
            if Button::new(Look::Soft(Tone::Accent), "Test notification")
                .icon("bell")
                .small()
                .enabled(background && app.platform.notifications_available())
                .show(ui)
                .clicked()
            {
                if app.platform.permission_needed() {
                    app.platform.request_permission();
                }
                app.platform.notify("PkgDeck test", "Desktop notifications are working.");
            }
        });
    });
}

fn about(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "About", |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(egui::include_image!("../../assets/logo.svg")).fit_to_exact_size(Vec2::splat(40.0)));
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("PkgDeck {}", app.ctl.version())).font(theme::bold(16.0)).color(palette.ink));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(egui::RichText::new("Made with").color(palette.muted));
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    theme::paint_icon(ui.painter(), rect, "heart", palette.heart);
                    ui.label(egui::RichText::new("by astro").color(palette.muted));
                });
            });
            right(ui, |ui| {
                if Button::new(Look::Secondary, "GitHub").icon("external").show(ui).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab("https://github.com/astrovm/PkgDeck"));
                }
            });
        });
    });
}

/// "⇧⌘U" on a Mac, the text as written elsewhere.
pub fn keys(text: &str) -> String {
    if !cfg!(target_os = "macos") {
        return text.to_owned();
    }
    let mut out = String::new();
    let parts: Vec<&str> = text.split('+').collect();
    let (key, modifiers) = parts.split_last().map_or(("", &[][..]), |(k, m)| (*k, m));
    for (name, symbol) in [("Alt", "⌥"), ("Shift", "⇧"), ("Ctrl", "⌘")] {
        if modifiers.contains(&name) {
            out.push_str(symbol);
        }
    }
    out.push_str(key);
    out
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Search", "Ctrl+1"),
    ("Installed", "Ctrl+2"),
    ("Updates", "Ctrl+3"),
    ("Clean", "Ctrl+4"),
    ("Sources", "Ctrl+5"),
    ("Settings", "Ctrl+,"),
    ("Search or filter", "Ctrl+F"),
    ("Activity", "Ctrl+J"),
    ("Focus results", "Ctrl+L"),
    ("Select result", "↑ / ↓"),
    ("Open the selected row", "Space"),
    ("Install, remove or update the selected row", "Enter"),
    ("Update selected", "Ctrl+Shift+U"),
    ("Install", "Ctrl+I"),
    ("Remove", "Ctrl+D"),
    ("Update", "Ctrl+U"),
    ("Reload the page", "Ctrl+R"),
    ("Apply a confirmation", "Ctrl+Enter"),
    ("Clear search or close details", "Esc"),
    ("Quit", "Ctrl+Q"),
];

fn shortcuts(ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "Keyboard shortcuts", |ui| {
        let refresh = if cfg!(target_os = "macos") { "Ctrl+Shift+R" } else { "Ctrl+M" };
        let mut all: Vec<(&str, &str)> = SHORTCUTS.to_vec();
        all.insert(17, ("Refresh the source's package lists", refresh));
        let columns = if ui.available_width() >= 620.0 { 2 } else { 1 };
        let per = all.len().div_ceil(columns);
        ui.columns(columns, |cols| {
            for (col, chunk) in cols.iter_mut().zip(all.chunks(per)) {
                for (label, key) in chunk {
                    col.horizontal(|ui| {
                        ui.label(egui::RichText::new(*label).color(palette.ink));
                        right(ui, |ui| {
                            egui::Frame::new()
                                .fill(palette.canvas)
                                .stroke(Stroke::new(1.0, palette.line))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(7, 2))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(keys(key)).font(theme::mono(12.0)).color(palette.muted));
                                });
                        });
                    });
                }
            }
        });
    });
}
