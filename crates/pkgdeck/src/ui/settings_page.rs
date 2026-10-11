//! Settings: how PkgDeck looks, when it checks for updates, where Clean
//! looks for old projects, what it is, and its keyboard shortcuts.

use super::widgets::*;
use crate::{
    app::App,
    model::{self, Tone},
    settings::{Accent, Appearance, DarkTheme, LightTheme, TextSize},
    theme::{self, Palette},
};
use eframe::egui::{self, vec2, CornerRadius, Id, Sense, Stroke, Ui, Vec2};
use pkgdeck_core::{backends::ProjectSettings, host::Host};
use std::path::{Path, PathBuf};

pub fn show(app: &mut App, ui: &mut Ui) {
    egui::ScrollArea::vertical()
        .id_salt("settings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(760.0);
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 6.0;
            appearance(app, ui);
            ui.add_space(8.0);
            update_checks(app, ui);
            ui.add_space(8.0);
            clean(ui);
            ui.add_space(8.0);
            about(app, ui);
            ui.add_space(8.0);
            shortcuts(ui);
            ui.add_space(20.0);
        });
}

const SWITCH: Vec2 = vec2(40.0, 24.0);

fn section(ui: &mut Ui, title: &str, content: impl FnOnce(&mut Ui)) {
    let palette = Palette::current(ui.ctx());
    card(&palette)
        .inner_margin(egui::Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.set_width(ui.available_width());
            caption(ui, title);
            ui.add_space(4.0);
            content(ui);
        });
}

/// A label with an optional line under it, and a control `size` big: on
/// the right, both centred, while the text keeps room to wrap; under the
/// text when it wouldn't.
fn setting_row(ui: &mut Ui, label: &str, hint: &str, size: Vec2, control: impl FnOnce(&mut Ui)) {
    let palette = Palette::current(ui.ctx());
    let full = ui.available_width();
    let gap = 20.0;
    let beside = full - size.x - gap >= 200.0;
    let text_width = if beside { full - size.x - gap } else { full };
    // Laid out once, so what's measured is exactly what's drawn.
    let layout = |text: &str, size: f32, color| {
        ui.painter()
            .layout(text.to_owned(), theme::font(size), color, text_width)
    };
    let title = layout(label, 14.5, palette.ink);
    let note = (!hint.is_empty()).then(|| layout(hint, 12.5, palette.muted));
    let spacing = 2.0;
    let text_height = title.size().y + note.as_ref().map_or(0.0, |n| spacing + n.size().y);
    let height = if beside {
        text_height.max(size.y)
    } else {
        text_height + 8.0 + size.y
    };
    let (rect, _) = ui.allocate_exact_size(vec2(full, height), Sense::hover());
    let text_top = if beside {
        rect.center().y - text_height / 2.0
    } else {
        rect.top()
    };
    let text_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), text_top),
        vec2(text_width, text_height),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
        ui.spacing_mut().item_spacing.y = spacing;
        ui.add(egui::Label::new(title));
        if let Some(note) = note {
            ui.add(egui::Label::new(note));
        }
    });
    let control_rect = if beside {
        egui::Rect::from_min_size(
            egui::pos2(rect.right() - size.x, rect.center().y - size.y / 2.0),
            size,
        )
    } else {
        egui::Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - size.y), size)
    };
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(control_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        control,
    );
    // Clearly more than the gap between lines, so wrapped rows stay apart.
    ui.add_space(14.0);
}

fn appearance(app: &mut App, ui: &mut Ui) {
    section(ui, "Appearance", |ui| {
        setting_row(ui, "Theme", "", vec2(252.0, 34.0), |ui| {
            let all = [Appearance::System, Appearance::Light, Appearance::Dark];
            if let Some(choice) = segmented(
                ui,
                Id::new("theme"),
                &["System", "Light", "Dark"],
                position(&all, app.settings.appearance),
            ) {
                app.settings.appearance = all[choice];
            }
        });
        // Only the colours of the appearance on screen; the other mode keeps
        // its own choice.
        setting_row(ui, "Colours", "", vec2(252.0, 34.0), |ui| {
            if ui.ctx().theme() == egui::Theme::Dark {
                let all = [DarkTheme::Charcoal, DarkTheme::Black, DarkTheme::Slate];
                if let Some(choice) = segmented(
                    ui,
                    Id::new("dark-theme"),
                    &["Charcoal", "Black", "Slate"],
                    position(&all, app.settings.dark_theme),
                ) {
                    app.settings.dark_theme = all[choice];
                }
            } else {
                let all = [LightTheme::Classic, LightTheme::White, LightTheme::Paper];
                if let Some(choice) = segmented(
                    ui,
                    Id::new("light-theme"),
                    &["Classic", "White", "Paper"],
                    position(&all, app.settings.light_theme),
                ) {
                    app.settings.light_theme = all[choice];
                }
            }
        });
        setting_row(ui, "Accent", "", vec2(swatches_width(), 34.0), |ui| {
            if let Some(accent) = swatches(ui, app.settings.accent) {
                app.settings.accent = accent;
            }
        });
        setting_row(ui, "Text size", "", vec2(336.0, 34.0), |ui| {
            let all = [
                TextSize::Small,
                TextSize::Normal,
                TextSize::Large,
                TextSize::Larger,
            ];
            if let Some(choice) = segmented(
                ui,
                Id::new("text-size"),
                &["Small", "Normal", "Large", "Larger"],
                position(&all, app.settings.text_size),
            ) {
                app.settings.text_size = all[choice];
            }
        });
        setting_row(
            ui,
            "Animations",
            "Movement when things open, close and change",
            SWITCH,
            |ui| {
                let mut on = !app.settings.reduce_motion;
                if switch(ui, &mut on, true, "Animations").changed() {
                    app.settings.reduce_motion = !on;
                }
            },
        );
    });
}

fn position<T: PartialEq>(all: &[T], current: T) -> usize {
    all.iter().position(|item| *item == current).unwrap_or(0)
}

const SWATCH: f32 = 26.0;
const SWATCH_GAP: f32 = 8.0;

fn swatches_width() -> f32 {
    let count = Accent::ALL.len() as f32;
    count * SWATCH + (count - 1.0) * SWATCH_GAP
}

/// A dot of each accent, in this appearance's shade, the current one
/// ringed. Returns a new choice.
fn swatches(ui: &mut Ui, current: Accent) -> Option<Accent> {
    let palette = Palette::current(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(swatches_width(), 34.0), Sense::hover());
    let mut chosen = None;
    for (index, accent) in Accent::ALL.into_iter().enumerate() {
        let center = egui::pos2(
            rect.left() + SWATCH / 2.0 + index as f32 * (SWATCH + SWATCH_GAP),
            rect.center().y,
        );
        let cell = egui::Rect::from_center_size(center, Vec2::splat(SWATCH));
        let response = ui
            .interact(cell, Id::new("accent").with(index), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(accent.name());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                true,
                accent == current,
                accent.name(),
            )
        });
        let color = Palette::of(
            theme::Scheme {
                accent,
                ..Default::default()
            },
            palette.dark,
        )
        .accent;
        let shown = ease(
            ui.ctx(),
            response.id.with("on"),
            accent == current,
            FEEDBACK,
        );
        let hovered = if response.hovered() { 1.0 } else { 0.0 };
        ui.painter()
            .circle_filled(center, SWATCH / 2.0 - 4.0 + 2.0 * hovered, color);
        if shown > 0.0 {
            ui.painter().circle_stroke(
                center,
                SWATCH / 2.0 - 1.0,
                Stroke::new(2.0, alpha(color, shown)),
            );
        }
        if response.clicked() && accent != current {
            chosen = Some(accent);
        }
    }
    chosen
}

/// Choices side by side with a sliding marker. Returns a new choice.
fn segmented(ui: &mut Ui, id: Id, labels: &[&str], current: usize) -> Option<usize> {
    let palette = Palette::current(ui.ctx());
    let part = 84.0;
    let (rect, _) = ui.allocate_exact_size(vec2(part * labels.len() as f32, 34.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), palette.hover_solid());
    let x = glide(
        ui.ctx(),
        id.with("knob"),
        rect.left() + current as f32 * part,
        LAYOUT,
    );
    ui.painter().rect(
        egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(part, rect.height())).shrink(3.0),
        CornerRadius::same(8),
        palette.surface,
        Stroke::new(1.0, palette.line),
        egui::StrokeKind::Inside,
    );
    let mut chosen = None;
    for (index, label) in labels.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(rect.left() + index as f32 * part, rect.top()),
            vec2(part, rect.height()),
        );
        let response = ui
            .interact(cell, id.with(index), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                true,
                index == current,
                *label,
            )
        });
        let color = if index == current {
            palette.ink
        } else {
            palette.muted
        };
        ui.painter().text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            if index == current {
                theme::bold(13.5)
            } else {
                theme::font(13.5)
            },
            color,
        );
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
        setting_row(
            ui,
            "Background checks",
            if cfg!(target_os = "macos") {
                "Keeps checking from the menu bar after the window closes"
            } else {
                "Keeps checking from the system tray after the window closes"
            },
            SWITCH,
            |ui| {
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
            },
        );
        let interval = model::nearest_interval(app.settings.check_interval);
        setting_row(
            ui,
            "Check every",
            model::INTERVALS[interval].1,
            vec2(240.0, 28.0),
            |ui| {
                let mut index = interval;
                if steps(
                    ui,
                    Id::new("interval"),
                    model::INTERVALS.len(),
                    &mut index,
                    background,
                    240.0,
                ) {
                    let minutes = model::INTERVALS[index].0;
                    app.settings.check_interval = minutes;
                    app.c().set_check_interval(minutes);
                }
            },
        );
        setting_row(ui, "Install updates automatically", "", SWITCH, |ui| {
            let mut on = app.settings.auto_update;
            if switch(ui, &mut on, background, "Install updates automatically").changed() {
                app.settings.auto_update = on;
                app.c().set_auto_update(on);
            }
        });
        setting_row(
            ui,
            "Clean automatically",
            "Once a day: caches, unused images and build files of old projects",
            SWITCH,
            |ui| {
                let mut on = app.settings.auto_clean;
                if switch(ui, &mut on, background, "Clean automatically").changed() {
                    app.settings.auto_clean = on;
                    let at = app.settings.auto_cleaned_at;
                    app.c().set_auto_clean(on, at);
                }
            },
        );
        setting_row(
            ui,
            "Allow updates that remove packages",
            "Like an old kernel replaced by a new one",
            SWITCH,
            |ui| {
                let mut on = app.settings.allow_removals;
                if switch(ui, &mut on, true, "Allow updates that remove packages").changed() {
                    app.settings.allow_removals = on;
                    app.c().set_allow_removals(on);
                }
            },
        );
        setting_row(
            ui,
            "Allow automatic updates without a password",
            "Changes you start still ask",
            SWITCH,
            |ui| {
                let mut on = !app.settings.system_approval.is_empty();
                if switch(
                    ui,
                    &mut on,
                    true,
                    "Allow automatic updates without a password",
                )
                .changed()
                {
                    app.c().allow_system_updates(on);
                    app.react();
                }
            },
        );
        let error = app.ctl.approval_error().to_string();
        if !error.is_empty() {
            ui.label(egui::RichText::new(error).color(palette.danger));
        }
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            setting_row(ui, "Start in background at login", "", SWITCH, |ui| {
                let mut on = app.settings.autostart;
                let can = background && app.platform.tray_available();
                if switch(ui, &mut on, can, "Start in background at login").changed()
                    && app.c().set_autostart(on)
                {
                    app.settings.autostart = on;
                }
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let text = match app.background.last_check {
                Some(at) => {
                    let count = app.background.available;
                    format!(
                        "Last check: {}, {count} update{} found",
                        model::short_datetime(at),
                        if count == 1 { "" } else { "s" }
                    )
                }
                None => "Last check: never".into(),
            };
            ui.label(egui::RichText::new(text).color(palette.muted));
            let warning = if !app.platform.tray_available() {
                Some(if cfg!(target_os = "macos") {
                    "No menu bar icon, so no notifications"
                } else {
                    "No system tray, so no notifications"
                })
            } else if !app.platform.notifications_available() {
                Some(if cfg!(target_os = "macos") {
                    "Notifications are off in System Settings"
                } else {
                    "This system tray can't show notifications"
                })
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
        if let Some(waiting) = app
            .background
            .waiting_text()
            .filter(|_| background && app.settings.auto_update)
        {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                theme::paint_icon(ui.painter(), rect, "clock", palette.muted);
                ui.label(egui::RichText::new(waiting).color(palette.muted));
            });
        }
        ui.horizontal(|ui| {
            if app.platform.permission_needed()
                && Button::new(Look::Secondary, "Notification settings")
                    .small()
                    .show(ui)
                    .clicked()
            {
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
                app.platform
                    .notify("PkgDeck test", "Desktop notifications are working.");
            }
        });
    });
}

/// How long a project stays untouched before Clean offers its build files.
const PROJECT_DAYS: [u32; 9] = [1, 3, 7, 14, 30, 60, 90, 180, 365];

/// The step for `days`, or the next longer one.
fn project_step(days: u32) -> usize {
    PROJECT_DAYS
        .iter()
        .position(|step| *step >= days)
        .unwrap_or(PROJECT_DAYS.len() - 1)
}

fn days_text(days: u32) -> String {
    match days {
        1 => "1 day".into(),
        7 => "1 week".into(),
        14 => "2 weeks".into(),
        365 => "1 year".into(),
        days => format!("{days} days"),
    }
}

/// `path` with the home folder written as `~`.
fn shown_folder(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Where Clean looks for old projects, kept in PkgDeck's own config file
/// for the engine and `pkd` to read too.
fn clean(ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let id = Id::new("project-settings");
    let mut projects = ui
        .data_mut(|data| data.get_temp::<ProjectSettings>(id))
        .unwrap_or_else(|| ProjectSettings::load(&Host::current()));
    let before = projects.clone();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    section(ui, "Clean", |ui| {
        let step = project_step(projects.days);
        setting_row(
            ui,
            "Old projects",
            &format!(
                "Build files and worktrees unchanged for {}",
                days_text(projects.days)
            ),
            vec2(240.0, 28.0),
            |ui| {
                let mut index = step;
                if steps(
                    ui,
                    Id::new("project-days"),
                    PROJECT_DAYS.len(),
                    &mut index,
                    true,
                    240.0,
                ) {
                    projects.days = PROJECT_DAYS[index];
                }
            },
        );
        ui.label(egui::RichText::new("Look for projects in").color(palette.muted));
        let mut removed = None;
        for (index, folder) in projects.folders.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(shown_folder(folder, home.as_deref()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "remove", "Stop looking here", palette.muted, true).clicked()
                    {
                        removed = Some(index);
                    }
                });
            });
        }
        if let Some(index) = removed {
            projects.folders.remove(index);
        }
        if Button::new(Look::Secondary, "Add folder")
            .icon("add")
            .small()
            .show(ui)
            .clicked()
        {
            let mut dialog = rfd::FileDialog::new().set_title("Look for projects in");
            if let Some(home) = &home {
                dialog = dialog.set_directory(home);
            }
            if let Some(folder) = dialog.pick_folder() {
                if !projects.folders.contains(&folder) {
                    projects.folders.push(folder);
                }
            }
        }
        let error_id = Id::new("project-settings-error");
        if let Some(error) = ui.data_mut(|data| data.get_temp::<String>(error_id)) {
            ui.label(egui::RichText::new(error).color(palette.danger));
        }
    });
    if projects != before {
        let error = projects
            .save(&Host::current())
            .err()
            .map(|error| format!("Couldn't save the folders: {error}"));
        ui.data_mut(|data| match error {
            Some(error) => {
                data.insert_temp(Id::new("project-settings-error"), error);
            }
            None => data.remove::<String>(Id::new("project-settings-error")),
        });
    }
    ui.data_mut(|data| data.insert_temp(id, projects));
}

fn about(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "About", |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::Image::new(egui::include_image!("../../assets/logo.svg"))
                    .fit_to_exact_size(Vec2::splat(40.0)),
            );
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!("PkgDeck {}", app.ctl.version()))
                        .font(theme::bold(16.0))
                        .color(palette.ink),
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(egui::RichText::new("Made with").color(palette.muted));
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    theme::paint_icon(ui.painter(), rect, "heart", palette.heart);
                    ui.label(egui::RichText::new("by astro").color(palette.muted));
                });
            });
            right(ui, |ui| {
                if Button::new(Look::Secondary, "GitHub")
                    .icon("external")
                    .show(ui)
                    .clicked()
                {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab("https://github.com/astrovm/PkgDeck"));
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
        let refresh = if cfg!(target_os = "macos") {
            "Ctrl+Shift+R"
        } else {
            "Ctrl+M"
        };
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
                                    ui.label(
                                        egui::RichText::new(keys(key))
                                            .font(theme::mono(12.0))
                                            .color(palette.muted),
                                    );
                                });
                        });
                    });
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_waits_snap_to_steps_and_read_naturally() {
        assert_eq!(PROJECT_DAYS[project_step(7)], 7);
        // A hand-edited wait shows on the next longer step.
        assert_eq!(PROJECT_DAYS[project_step(10)], 14);
        assert_eq!(PROJECT_DAYS[project_step(0)], 1);
        assert_eq!(PROJECT_DAYS[project_step(9999)], 365);
        let said: Vec<_> = PROJECT_DAYS.iter().map(|days| days_text(*days)).collect();
        assert_eq!(
            said,
            [
                "1 day", "3 days", "1 week", "2 weeks", "30 days", "60 days", "90 days",
                "180 days", "1 year"
            ]
        );
        assert_eq!(days_text(10), "10 days");
    }

    #[test]
    fn folders_are_shown_from_the_home() {
        let home = Path::new("/home/me");
        assert_eq!(shown_folder(home, Some(home)), "~");
        assert_eq!(
            shown_folder(Path::new("/home/me/code"), Some(home)),
            "~/code"
        );
        assert_eq!(
            shown_folder(Path::new("/srv/code"), Some(home)),
            "/srv/code"
        );
        assert_eq!(
            shown_folder(Path::new("/home/meg"), Some(home)),
            "/home/meg"
        );
        assert_eq!(
            shown_folder(Path::new("/home/me/code"), None),
            "/home/me/code"
        );
    }
}
