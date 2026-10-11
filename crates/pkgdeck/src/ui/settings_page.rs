//! Settings: how PkgDeck looks, when it checks for updates, which package
//! managers it uses, where Clean looks for old projects, what it is, and
//! its keyboard shortcuts.

use super::{skin, widgets::*};
use crate::{
    app::App,
    model::{self, Tone},
    settings::{
        Accent, Appearance, DarkTheme, Density, LightTheme, Style, TextSize, Theme, Typeface,
    },
    theme::{self, Palette},
};
use eframe::egui::{self, vec2, Id, Sense, Stroke, Ui, Vec2};
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
            sources(app, ui);
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
    let margin = egui::Margin::symmetric(18, 16);
    // The older styles frame settings in their group boxes.
    let Some(group) = skin::group(&palette) else {
        show_card(ui, card(&palette).inner_margin(margin), |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.set_width(ui.available_width());
            caption(ui, title);
            ui.add_space(4.0);
            content(ui);
        });
        return;
    };
    let font = if group.bold {
        theme::bold(group.size)
    } else {
        theme::font(group.size)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(title.to_owned(), font, group.ink);
    let title_height = galley.size().y;
    if group.on_line {
        ui.add_space(title_height / 2.0);
    }
    let shown = egui::Frame::new()
        .fill(group.fill)
        .stroke(group.stroke)
        .corner_radius(group.radius)
        .shadow(group.shadow)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.set_width(ui.available_width());
            // Room for the title, drawn once the box's size is known.
            ui.add_space(if group.on_line {
                4.0
            } else {
                title_height + 8.0
            });
            content(ui);
        });
    let rect = shown.response.rect;
    skin::paint_group(ui.painter(), rect, &palette);
    let at = if group.on_line {
        egui::pos2(rect.left() + 12.0, rect.top() - title_height / 2.0)
    } else {
        egui::pos2(rect.left() + 18.0, rect.top() + 14.0)
    };
    if group.on_line {
        // The title breaks the line it sits on.
        ui.painter().rect_filled(
            egui::Rect::from_min_size(at, galley.size()).expand2(vec2(5.0, 0.0)),
            egui::CornerRadius::ZERO,
            palette.canvas,
        );
    }
    ui.painter().galley(at, galley, group.ink);
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
        styles(app, ui);
        // Only the colours of the appearance on screen; the other mode keeps
        // its own choice. The older styles have their own.
        if app.settings.style == Style::Modern {
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
        }
        setting_row(ui, "Accent", "", vec2(swatches_width(), 34.0), |ui| {
            if let Some(accent) = swatches(ui, app.settings.accent) {
                app.settings.accent = accent;
            }
            custom_swatch(ui, &mut app.settings);
        });
        // Retro is square whatever the roundness.
        if app.settings.style != Style::Retro {
            // Quarter steps from square to twice the style's own.
            let mut step = (app.settings.roundness * 4.0).round().clamp(0.0, 8.0) as usize;
            let hint = match step {
                0 => "Square".to_owned(),
                4 => "As the style draws it".to_owned(),
                _ => format!("{}%", step * 25),
            };
            setting_row(ui, "Roundness", &hint, vec2(240.0, 28.0), |ui| {
                if steps(ui, Id::new("roundness"), 9, &mut step, true, 240.0) {
                    app.settings.roundness = step as f32 / 4.0;
                }
            });
        }
        setting_row(ui, "Density", "", vec2(252.0, 34.0), |ui| {
            let all = [Density::Compact, Density::Normal, Density::Roomy];
            if let Some(choice) = segmented(
                ui,
                Id::new("density"),
                &["Compact", "Normal", "Roomy"],
                position(&all, app.settings.density),
            ) {
                app.settings.density = all[choice];
            }
        });
        setting_row(ui, "Font", "", vec2(252.0, 34.0), |ui| {
            let all = [Typeface::Style, Typeface::Inter, Typeface::System];
            if let Some(choice) = segmented(
                ui,
                Id::new("typeface"),
                &["Style", "Inter", "System"],
                position(&all, app.settings.typeface),
            ) {
                app.settings.typeface = all[choice];
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
        // The desktop can turn movement off for every app; then it stays off.
        let system = app.platform.reduce_motion();
        let hint = if system {
            "Off in system settings"
        } else {
            "Movement when things open, close and change"
        };
        setting_row(ui, "Animations", hint, SWITCH, |ui| {
            let mut on = !app.reduce_motion();
            if switch(ui, &mut on, !system, "Animations").changed() {
                app.settings.reduce_motion = !on;
            }
        });
        theme_files(app, ui);
    });
}

const PREVIEW: Vec2 = vec2(104.0, 74.0);
const PREVIEW_GAP: f32 = 10.0;

/// A small window drawn in each style, the current one ringed. Picking
/// one also picks the accent it was made with.
fn styles(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let width = ui.available_width();
    let columns =
        (((width + PREVIEW_GAP) / (PREVIEW.x + PREVIEW_GAP)).floor() as usize).clamp(1, 6);
    let rows = Style::ALL.len().div_ceil(columns);
    let cell = vec2(PREVIEW.x, PREVIEW.y + 24.0);
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, rows as f32 * (cell.y + PREVIEW_GAP)),
        Sense::hover(),
    );
    for (index, style) in Style::ALL.into_iter().enumerate() {
        let at = rect.min
            + vec2(
                (index % columns) as f32 * (cell.x + PREVIEW_GAP),
                (index / columns) as f32 * (cell.y + PREVIEW_GAP),
            );
        let card = egui::Rect::from_min_size(at, PREVIEW);
        let response = ui
            .interact(card, Id::new("style").with(index), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let current = app.settings.style == style;
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, current, style.name())
        });
        let mut scheme = theme::Scheme::of(&app.settings);
        scheme.style = style;
        if !current {
            scheme.accent = style.accent();
        }
        let look = Palette::of(scheme, palette.dark);
        let skin = theme::Skin {
            style,
            roundness: scheme.roundness,
            density: scheme.density,
        };
        let painter = ui.painter_at(card);
        theme::with_skin(skin, || preview(&painter, card, &look));
        let hover = ease(
            ui.ctx(),
            response.id.with("hover"),
            response.hovered(),
            FEEDBACK,
        );
        let ring = ease(ui.ctx(), response.id.with("on"), current, FEEDBACK);
        let ring_color = mix(alpha(palette.strong_line, hover), palette.accent, ring);
        ui.painter().rect_stroke(
            card.expand(3.0),
            theme::round(12),
            Stroke::new(2.0, ring_color),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            egui::pos2(card.center().x, card.bottom() + 14.0),
            egui::Align2::CENTER_CENTER,
            style.name(),
            if current {
                theme::bold(13.0)
            } else {
                theme::font(13.0)
            },
            if current { palette.ink } else { palette.muted },
        );
        if response.clicked() && !current {
            app.settings.style = style;
            app.settings.accent = style.accent();
        }
    }
}

/// A window in miniature: a sidebar with a chosen entry, a panel with a
/// title strip and a button.
fn preview(painter: &egui::Painter, rect: egui::Rect, palette: &Palette) {
    skin::paint_canvas(painter, rect, palette);
    let sidebar = egui::Rect::from_min_size(rect.min, vec2(30.0, rect.height()));
    let pill = egui::Rect::from_min_size(sidebar.min + vec2(5.0, 16.0), vec2(20.0, 9.0));
    let logo = egui::Rect::from_min_size(sidebar.min + vec2(5.0, 6.0), vec2(20.0, 4.0));
    let entries =
        egui::Rect::from_min_max(pill.min, egui::pos2(pill.right(), rect.bottom() - 10.0));
    skin::paint_sidebar(painter, sidebar, logo, entries, palette);
    skin::paint_selection(painter, pill, palette);
    for row in 1..4 {
        let line = egui::Rect::from_min_size(
            sidebar.min + vec2(7.0, 16.0 + row as f32 * 13.0 + 3.0),
            vec2(14.0, 3.0),
        );
        painter.rect_filled(line, egui::CornerRadius::same(1), alpha(palette.muted, 0.5));
    }
    let panel = egui::Rect::from_min_max(
        egui::pos2(sidebar.right() + 8.0, rect.top() + 9.0),
        rect.max - vec2(8.0, 9.0),
    );
    skin::paint_panel(painter, panel, palette, false, skin::panel_stroke(palette));
    let strip = egui::Rect::from_min_size(panel.min, vec2(panel.width(), 13.0)).shrink(1.0);
    if skin::paint_title_strip(painter, strip, palette).is_none() {
        let caption = egui::Rect::from_min_size(panel.min + vec2(7.0, 6.0), vec2(24.0, 3.0));
        painter.rect_filled(
            caption,
            egui::CornerRadius::same(1),
            alpha(palette.muted, 0.6),
        );
    }
    for row in 0..2 {
        let line = egui::Rect::from_min_size(
            panel.min + vec2(7.0, 20.0 + row as f32 * 8.0),
            vec2(panel.width() * (0.7 - row as f32 * 0.2), 3.0),
        );
        painter.rect_filled(line, egui::CornerRadius::same(1), alpha(palette.ink, 0.35));
    }
    let button = egui::Rect::from_min_size(
        egui::pos2(panel.right() - 36.0, panel.bottom() - 18.0),
        vec2(29.0, 12.0),
    );
    skin::paint_button(
        painter,
        button,
        skin::Weight::Strong,
        palette.accent,
        Stroke::NONE,
        skin::State {
            hover: 0.0,
            press: 0.0,
            opacity: 1.0,
        },
        palette,
    );
}

/// The last swatch: a colour picked by hand.
fn custom_swatch(ui: &mut Ui, settings: &mut crate::settings::Settings) {
    let palette = Palette::current(ui.ctx());
    let (cell, response) = ui.allocate_exact_size(Vec2::splat(SWATCH), Sense::click());
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Pick a colour");
    let current = settings.accent == Accent::Custom;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, current, "Custom accent")
    });
    let center = cell.center();
    let hovered = if response.hovered() { 1.0 } else { 0.0 };
    let radius = SWATCH / 2.0 - 4.0 + 2.0 * hovered;
    let picked = theme::parse_hex(&settings.custom_accent);
    match picked.filter(|_| current) {
        Some(color) => {
            ui.painter().circle_filled(center, radius, color);
            ui.painter()
                .circle_stroke(center, SWATCH / 2.0 - 1.0, Stroke::new(2.0, color));
        }
        None => paint_wheel(ui.painter(), center, radius),
    }
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let mut color = picked.unwrap_or(palette.accent);
            if egui::color_picker::color_picker_color32(
                ui,
                &mut color,
                egui::color_picker::Alpha::Opaque,
            ) {
                settings.accent = Accent::Custom;
                settings.custom_accent = theme::hex(color);
            }
        });
}

/// A colour wheel, for the custom accent before one is picked.
fn paint_wheel(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    let steps = 24;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, egui::Color32::WHITE);
    for step in 0..=steps {
        let turn = step as f32 / steps as f32;
        let angle = turn * std::f32::consts::TAU;
        let color = egui::ecolor::Hsva::new(turn, 0.75, 0.95, 1.0).into();
        mesh.colored_vertex(center + radius * vec2(angle.cos(), angle.sin()), color);
    }
    for step in 0..steps {
        mesh.add_triangle(0, step + 1, step + 2);
    }
    painter.add(mesh);
}

/// Saving the look to a file and loading one.
fn theme_files(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let error_id = Id::new("theme-file-error");
    setting_row(
        ui,
        "Theme file",
        "Share a look, or use someone else's",
        vec2(220.0, 34.0),
        |ui| {
            ui.horizontal(|ui| {
                if Button::new(Look::Secondary, "Export")
                    .icon("external")
                    .small()
                    .show(ui)
                    .clicked()
                {
                    let dialog = rfd::FileDialog::new()
                        .set_title("Export theme")
                        .set_file_name(format!("{}.toml", app.settings.style.name()))
                        .add_filter("Theme", &["toml"]);
                    if let Some(path) = dialog.save_file() {
                        let error = Theme::of(&app.settings)
                            .write(&path)
                            .err()
                            .map(|error| format!("Couldn't save the theme: {error}"));
                        ui.data_mut(|data| data.insert_temp(error_id, error));
                    }
                }
                if Button::new(Look::Secondary, "Import")
                    .icon("install")
                    .small()
                    .show(ui)
                    .clicked()
                {
                    let dialog = rfd::FileDialog::new()
                        .set_title("Import theme")
                        .add_filter("Theme", &["toml"]);
                    if let Some(path) = dialog.pick_file() {
                        let error = match Theme::read(&path) {
                            Ok(theme) => {
                                theme.apply(&mut app.settings);
                                None
                            }
                            Err(error) => Some(error.to_string()),
                        };
                        ui.data_mut(|data| data.insert_temp(error_id, error));
                    }
                }
            });
        },
    );
    if let Some(Some(error)) = ui.data(|data| data.get_temp::<Option<String>>(error_id)) {
        ui.label(egui::RichText::new(error).color(palette.danger));
    }
}

fn position<T: PartialEq>(all: &[T], current: T) -> usize {
    all.iter().position(|item| *item == current).unwrap_or(0)
}

const SWATCH: f32 = 26.0;
const SWATCH_GAP: f32 = 8.0;

fn swatches_width() -> f32 {
    // The presets and the custom colour.
    let count = Accent::ALL.len() as f32 + 1.0;
    count * SWATCH + (count - 1.0) * SWATCH_GAP
}

/// A dot of each accent, in this appearance's shade, the current one
/// ringed. Returns a new choice.
fn swatches(ui: &mut Ui, current: Accent) -> Option<Accent> {
    let palette = Palette::current(ui.ctx());
    let width = swatches_width() - SWATCH - SWATCH_GAP;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
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
    let modern = theme::skin().style == Style::Modern;
    if modern {
        ui.painter()
            .rect_filled(rect, theme::round(10), palette.hover_solid());
    } else {
        skin::paint_field(ui.painter(), rect, theme::round(10), &palette, 0.0);
    }
    let x = glide(
        ui.ctx(),
        id.with("knob"),
        rect.left() + current as f32 * part,
        LAYOUT,
    );
    let knob =
        egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(part, rect.height())).shrink(3.0);
    let mut chosen_ink = palette.ink;
    if modern {
        ui.painter().rect(
            knob,
            theme::round(8),
            palette.surface,
            Stroke::new(1.0, palette.line),
            egui::StrokeKind::Inside,
        );
    } else {
        // Aqua's chosen segment is blue gel; Windows 98's is pressed in.
        let style = theme::skin().style;
        if let Some(ink) = skin::paint_button(
            ui.painter(),
            knob,
            if style == Style::Gel {
                skin::Weight::Strong
            } else {
                skin::Weight::Normal
            },
            palette.surface,
            Stroke::new(1.0, palette.line),
            skin::State {
                hover: 0.0,
                press: if style == Style::Retro { 1.0 } else { 0.0 },
                opacity: 1.0,
            },
            &palette,
        ) {
            chosen_ink = ink;
        }
    }
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
            chosen_ink
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

/// The switches Settings shares with the first-run dialog. Each needs
/// background checks.
pub(super) fn auto_update_row(app: &mut App, ui: &mut Ui) {
    let background = app.settings.background_mode;
    setting_row(
        ui,
        "Update apps and packages automatically",
        "After each background check",
        SWITCH,
        |ui| {
            let mut on = app.settings.auto_update;
            if switch(
                ui,
                &mut on,
                background,
                "Update apps and packages automatically",
            )
            .changed()
            {
                app.settings.auto_update = on;
                app.c().set_auto_update(on);
            }
        },
    );
}

pub(super) fn auto_clean_row(app: &mut App, ui: &mut Ui) {
    let background = app.settings.background_mode;
    setting_row(
        ui,
        "Clean caches automatically",
        "Once a day. Also unused images and build files of old projects",
        SWITCH,
        |ui| {
            let mut on = app.settings.auto_clean;
            if switch(ui, &mut on, background, "Clean caches automatically").changed() {
                app.settings.auto_clean = on;
                let at = app.settings.auto_cleaned_at;
                app.c().set_auto_clean(on, at);
            }
        },
    );
}

/// Turning it on or off asks for the password, then the result comes back
/// through the controller.
pub(super) fn password_row(app: &mut App, ui: &mut Ui, enabled: bool) {
    let palette = Palette::current(ui.ctx());
    setting_row(
        ui,
        "Update system packages without a password",
        if cfg!(target_os = "macos") {
            "Only automatic updates of MacPorts and macOS"
        } else {
            "Only automatic updates of APT, Flatpak and other system sources"
        },
        SWITCH,
        |ui| {
            let mut on = !app.settings.system_approval.is_empty();
            if switch(
                ui,
                &mut on,
                enabled,
                "Update system packages without a password",
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
}

pub(super) fn login_row(app: &mut App, ui: &mut Ui) {
    setting_row(ui, "Start in background at login", "", SWITCH, |ui| {
        let mut on = app.settings.autostart;
        let can = app.settings.background_mode && app.platform.tray_available();
        if switch(ui, &mut on, can, "Start in background at login").changed()
            && app.c().set_autostart(on)
        {
            app.settings.autostart = on;
        }
    });
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
        auto_update_row(app, ui);
        auto_clean_row(app, ui);
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
        password_row(app, ui, true);
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            login_row(app, ui);
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
        });
        {
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
            // Said, not hidden behind a hover: it is why nothing pops up.
            if let Some(warning) = warning {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    theme::paint_icon(ui.painter(), rect, "warning", palette.warning);
                    ui.label(egui::RichText::new(warning).color(palette.warning));
                });
            }
        }
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
/// Package managers that manage repositories in PkgDeck.
const REPOSITORY_SOURCES: [&str; 5] = ["flatpak", "fwupd", "apt", "dnf", "zypper"];

/// Every package manager found on this computer, with a switch, and the
/// repositories of those that have them.
fn sources(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let shown = shown_sources(&app.catalog);
    let enabled = app.enabled_sources();
    section(ui, "Sources", |ui| {
        let note = if app.catalog.is_empty() {
            "Looking for package managers…"
        } else {
            "PkgDeck uses the package managers that are on."
        };
        ui.label(egui::RichText::new(note).color(palette.muted));
        ui.add_space(4.0);
        for entry in &shown {
            let on_now = enabled.contains(&entry.source);
            ui.horizontal(|ui| {
                ui.set_min_height(SWITCH.y + 6.0);
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                theme::paint_source_icon(ui.painter(), icon, &entry.source, palette.accent);
                ui.label(
                    egui::RichText::new(model::source_name(&entry.source))
                        .font(theme::font(14.5))
                        .color(palette.ink),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut on = on_now;
                    let can = !app.writing && !(on_now && !app.can_turn_off(&entry.source));
                    if switch(ui, &mut on, can, model::source_name(&entry.source)).changed() {
                        app.set_source_enabled(&entry.source, on);
                    }
                });
            });
            if entry.check_failed {
                ui.label(
                    egui::RichText::new(&entry.summary)
                        .font(theme::font(12.5))
                        .color(palette.danger),
                );
                if Button::new(Look::Flat, "Try again")
                    .icon("refresh")
                    .small()
                    .show(ui)
                    .clicked()
                {
                    app.c().recheck_sources();
                    app.react();
                }
            }
        }
        if REPOSITORY_SOURCES.iter().any(|id| app.available(id)) {
            ui.add_space(6.0);
            if Button::new(Look::Secondary, "Repositories")
                .icon("sources")
                .small()
                .enabled(!app.busy)
                .show(ui)
                .clicked()
            {
                app.ui.repos_open = true;
                app.c().load_repositories();
                app.react();
            }
        }
    });
}

/// The package managers worth a switch: found here, or found but failing
/// their check, by name.
fn shown_sources(catalog: &[model::Source]) -> Vec<model::Source> {
    let mut shown: Vec<model::Source> = catalog
        .iter()
        .filter(|entry| entry.available || entry.check_failed)
        .cloned()
        .collect();
    shown.sort_by_key(|entry| model::source_name(&entry.source).to_lowercase());
    shown
}

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
    ("Settings", "Ctrl+,"),
    ("Search or filter", "Ctrl+F"),
    ("Activity", "Ctrl+J"),
    ("Focus results", "Ctrl+L"),
    ("Select result", "↑ / ↓"),
    ("Open the selected row", "Space"),
    ("Install, remove or update the selected row", "Enter"),
    ("Update selected", "Ctrl+Shift+U"),
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
        // Folded away: most people never need the list.
        let open_id = Id::new("shortcuts-open");
        let open = ui
            .data(|data| data.get_temp::<bool>(open_id))
            .unwrap_or(false);
        let label = if open {
            "Hide shortcuts"
        } else {
            "Show shortcuts"
        };
        if Button::new(Look::Secondary, label)
            .icon(if open { "up" } else { "down" })
            .small()
            .show(ui)
            .clicked()
        {
            ui.data_mut(|data| data.insert_temp(open_id, !open));
        }
        if !open {
            return;
        }
        ui.add_space(6.0);
        let mut all: Vec<(&str, &str)> = SHORTCUTS.to_vec();
        let after_reload = all
            .iter()
            .position(|(label, _)| *label == "Reload the page")
            .map_or(all.len(), |at| at + 1);
        all.insert(after_reload, ("Refresh sources", refresh));
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
                                .corner_radius(theme::round(6))
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
