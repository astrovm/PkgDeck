//! Settings: how PkgDeck looks, when it checks for updates, which package
//! managers it uses, where Clean looks for old projects, what it is, and
//! its keyboard shortcuts.

use super::{skin, widgets::*};
use crate::{
    app::App,
    model::{self, Tone},
    settings::{
        Accent, Appearance, DarkTheme, Density, Family, LightTheme, Style, TextSize, Typeface,
    },
    theme::{self, Palette},
};
use eframe::egui::{self, vec2, Id, Sense, Stroke, Ui, Vec2};
use pkgdeck_core::{
    backends::{ProjectSettings, WorktreeProtection},
    host::Host,
};
use std::path::{Path, PathBuf};

/// The parts of Settings, one shown at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Appearance,
    Updates,
    Sources,
    Clean,
    About,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Appearance,
        Tab::Updates,
        Tab::Sources,
        Tab::Clean,
        Tab::About,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Tab::Appearance => "Appearance",
            Tab::Updates => "Updates",
            Tab::Sources => "Sources",
            Tab::Clean => "Clean",
            Tab::About => "About",
        }
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let tab = app.ui.settings_tab;
    let width = ui.available_width().min(760.0);
    let names = Tab::ALL.map(Tab::name);
    let widths = label_widths(ui, &names, width);
    ui.allocate_ui(vec2(width, 34.0), |ui| {
        if let Some(choice) = segmented_sized(
            ui,
            Id::new("settings-tab"),
            &names,
            position(&Tab::ALL, tab),
            &widths,
        ) {
            app.ui.settings_tab = Tab::ALL[choice];
        }
    });
    ui.add_space(12.0);
    super::scrollbar::vertical()
        // Each tab keeps its own place.
        .id_salt(("settings", tab.name()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 6.0;
            let gap = 8.0;
            match tab {
                Tab::Appearance => {
                    style_section(app, ui);
                    ui.add_space(gap);
                    colours(app, ui);
                    ui.add_space(gap);
                    layout(app, ui);
                }
                Tab::Updates => {
                    checks(app, ui);
                    ui.add_space(gap);
                    automatic_updates(app, ui);
                    ui.add_space(gap);
                    notifications(app, ui);
                }
                Tab::Sources => sources(app, ui),
                Tab::Clean => clean(app, ui),
                Tab::About => {
                    about(app, ui);
                    ui.add_space(gap);
                    shortcuts(ui);
                }
            }
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
            if !title.is_empty() {
                caption(ui, title);
                ui.add_space(4.0);
            }
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
        .layout_no_wrap(title.to_owned(), font, palette.ink);
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
            ui.add_space(if group.on_line || title.is_empty() {
                4.0
            } else {
                title_height + 8.0
            });
            content(ui);
        });
    let rect = shown.response.rect;
    skin::paint_group(ui.painter(), rect, &palette);
    if title.is_empty() {
        return;
    }
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
    ui.painter().galley(at, galley, palette.ink);
}

/// A label with an optional line under it, and a control `size` big: on
/// the right, both centred, while the text keeps room to wrap; under the
/// text when it wouldn't.
fn setting_row(ui: &mut Ui, label: &str, hint: &str, size: Vec2, control: impl FnOnce(&mut Ui)) {
    let palette = Palette::current(ui.ctx());
    let full = ui.available_width();
    // A control never runs past the card; it narrows instead.
    let size = vec2(size.x.min(full), size.y);
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

fn style_section(app: &mut App, ui: &mut Ui) {
    section(ui, "Style", |ui| styles(app, ui));
}

fn colours(app: &mut App, ui: &mut Ui) {
    section(ui, "Colours", |ui| {
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
        // As many dots in a line as fit, then the next line.
        let count = Accent::ALL.len() + 1;
        let columns = (((ui.available_width() + SWATCH_GAP) / (SWATCH + SWATCH_GAP)).floor()
            as usize)
            .clamp(1, count);
        let size = vec2(
            swatches_width(columns),
            count.div_ceil(columns) as f32 * SWATCH_ROW,
        );
        setting_row(ui, "Accent", "", size, |ui| {
            if let Some(accent) = swatches(ui, &mut app.settings, columns) {
                app.settings.accent = accent;
            }
        });
        setting_row(ui, "Tint backgrounds", "", SWITCH, |ui| {
            let mut on = app.settings.tint_backgrounds;
            if switch(ui, &mut on, true, "Tint backgrounds").changed() {
                app.settings.tint_backgrounds = on;
            }
        });
        if app.settings.style.tints_window_bar() {
            setting_row(ui, "Tint window bar", "", SWITCH, |ui| {
                let style = app.settings.style;
                let mut on = app.settings.window_bar_tint(style);
                if switch(ui, &mut on, true, "Tint window bar").changed() {
                    app.settings.set_window_bar_tint(style, on);
                }
            });
        }
    });
}

fn layout(app: &mut App, ui: &mut Ui) {
    section(ui, "Layout", |ui| {
        // Windows 98 is square whatever the roundness.
        if app.settings.style.family() != Family::Retro {
            // Quarter steps from square to twice the style's own.
            let mut step = (app.settings.roundness * 4.0).round().clamp(0.0, 8.0) as usize;
            let hint = match step {
                0 => "Square".to_owned(),
                4 => "Default".to_owned(),
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
        let hint = if system { "Off in system settings" } else { "" };
        setting_row(ui, "Animations", hint, SWITCH, |ui| {
            let mut on = !app.reduce_motion();
            if switch(ui, &mut on, !system, "Animations").changed() {
                app.settings.reduce_motion = !on;
            }
        });
    });
}

const PREVIEW: Vec2 = vec2(104.0, 74.0);
const PREVIEW_GAP: f32 = 10.0;

/// `text` laid out in one line, and in a smaller size when it would run
/// wider than `max`.
fn fitted(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max: f32,
) -> std::sync::Arc<egui::Galley> {
    let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
    if galley.size().x <= max {
        return galley;
    }
    let size = (font.size * max / galley.size().x).floor().max(8.0);
    painter.layout_no_wrap(text.to_owned(), egui::FontId::new(size, font.family), color)
}

/// A small window drawn in each style, the current one ringed.
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
        // Each card shows the window bar tint that style keeps.
        scheme.tint_window_bar = app.settings.window_bar_tint(style);
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
        let ink = if current { palette.ink } else { palette.muted };
        let font = if current {
            theme::bold(13.0)
        } else {
            theme::font(13.0)
        };
        // A long name shrinks rather than run into its neighbour.
        let name = fitted(
            ui.painter(),
            style.name(),
            font,
            ink,
            PREVIEW.x + PREVIEW_GAP,
        );
        let at = egui::pos2(card.center().x, card.bottom() + 14.0) - name.size() / 2.0;
        ui.painter().galley(at, name, ink);
        if response.clicked() && !current {
            app.settings.style = style;
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
fn custom_swatch(ui: &mut Ui, settings: &mut crate::settings::Settings, cell: egui::Rect) {
    let palette = Palette::current(ui.ctx());
    let response = ui
        .interact(cell, Id::new("custom-accent"), Sense::click())
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

fn position<T: PartialEq>(all: &[T], current: T) -> usize {
    all.iter().position(|item| *item == current).unwrap_or(0)
}

const SWATCH: f32 = 26.0;
const SWATCH_GAP: f32 = 8.0;

const SWATCH_ROW: f32 = 34.0;

/// How wide `columns` dots are.
fn swatches_width(columns: usize) -> f32 {
    let columns = columns as f32;
    columns * SWATCH + (columns - 1.0) * SWATCH_GAP
}

/// A dot of each accent, in this appearance's shade, the current one
/// ringed, `columns` to a line, and last the custom colour. Returns a new
/// preset choice.
fn swatches(
    ui: &mut Ui,
    settings: &mut crate::settings::Settings,
    columns: usize,
) -> Option<Accent> {
    let palette = Palette::current(ui.ctx());
    let current = settings.accent;
    let count = Accent::ALL.len() + 1;
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            swatches_width(columns),
            count.div_ceil(columns) as f32 * SWATCH_ROW,
        ),
        Sense::hover(),
    );
    let center_of = |index: usize| {
        egui::pos2(
            rect.left() + SWATCH / 2.0 + (index % columns) as f32 * (SWATCH + SWATCH_GAP),
            rect.top() + SWATCH_ROW / 2.0 + (index / columns) as f32 * SWATCH_ROW,
        )
    };
    custom_swatch(
        ui,
        settings,
        egui::Rect::from_center_size(center_of(Accent::ALL.len()), Vec2::splat(SWATCH)),
    );
    let mut chosen = None;
    for (index, accent) in Accent::ALL.into_iter().enumerate() {
        let center = center_of(index);
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
    let part = (ui.available_width() / labels.len() as f32).min(84.0);
    segmented_sized(ui, id, labels, current, &vec![part; labels.len()])
}

/// Widths for choices sized to their labels, together no wider than `max`.
fn label_widths(ui: &Ui, labels: &[&str], max: f32) -> Vec<f32> {
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| {
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                theme::bold(13.5),
                egui::Color32::WHITE,
            );
            (galley.size().x + 32.0).max(84.0)
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let scale = (max / total).min(1.0);
    widths.into_iter().map(|width| width * scale).collect()
}

/// `segmented`, each choice as wide as `widths` says.
fn segmented_sized(
    ui: &mut Ui,
    id: Id,
    labels: &[&str],
    current: usize,
    widths: &[f32],
) -> Option<usize> {
    let palette = Palette::current(ui.ctx());
    let starts: Vec<f32> = widths
        .iter()
        .scan(0.0, |at, width| {
            let start = *at;
            *at += width;
            Some(start)
        })
        .collect();
    let (rect, _) = ui.allocate_exact_size(vec2(widths.iter().sum(), 34.0), Sense::hover());
    let modern = theme::skin().family().flat();
    if modern {
        ui.painter()
            .rect_filled(rect, theme::round(10), palette.hover_solid());
    } else {
        skin::paint_field(ui.painter(), rect, theme::round(10), &palette, 0.0);
    }
    let x = glide(
        ui.ctx(),
        id.with("knob"),
        rect.left() + starts[current],
        LAYOUT,
    );
    let knob_width = glide(ui.ctx(), id.with("knob-width"), widths[current], LAYOUT);
    let knob =
        egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(knob_width, rect.height()))
            .shrink(3.0);
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
        let family = theme::skin().family();
        if let Some(ink) = skin::paint_button(
            ui.painter(),
            knob,
            if family == Family::Gel {
                skin::Weight::Strong
            } else {
                skin::Weight::Normal
            },
            palette.surface,
            Stroke::new(1.0, palette.line),
            skin::State {
                hover: 0.0,
                press: if family == Family::Retro { 1.0 } else { 0.0 },
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
            egui::pos2(rect.left() + starts[index], rect.top()),
            vec2(widths[index], rect.height()),
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
        // A long label shrinks rather than run into its neighbour.
        let text = fitted(
            ui.painter(),
            label,
            if index == current {
                theme::bold(13.5)
            } else {
                theme::font(13.5)
            },
            color,
            widths[index] - 10.0,
        );
        ui.painter()
            .galley(cell.center() - text.size() / 2.0, text, color);
        if response.clicked() && index != current {
            chosen = Some(index);
        }
    }
    chosen
}

/// Why a switch that needs background checks is off.
const NEEDS_BACKGROUND: &str = "Needs Background checks";

/// The switches Settings shares with the first-run dialog. Each needs
/// background checks.
pub(super) fn auto_update_row(app: &mut App, ui: &mut Ui) {
    let background = app.settings.background_mode;
    let hint = if background {
        "After each check"
    } else {
        NEEDS_BACKGROUND
    };
    setting_row(ui, "Update automatically", hint, SWITCH, |ui| {
        let mut on = app.settings.auto_update;
        if switch(ui, &mut on, background, "Update automatically").changed() {
            app.settings.auto_update = on;
            app.c().set_auto_update(on);
        }
    });
}

pub(super) fn auto_clean_row(app: &mut App, ui: &mut Ui) {
    let background = app.settings.background_mode;
    let hint = if background {
        "Once a day. Cleans what's checked on Clean"
    } else {
        NEEDS_BACKGROUND
    };
    setting_row(ui, "Clean automatically", hint, SWITCH, |ui| {
        let mut on = app.settings.auto_clean;
        if switch(ui, &mut on, background, "Clean automatically").changed() {
            app.settings.auto_clean = on;
            let at = app.settings.auto_cleaned_at;
            app.c().set_auto_clean(on, at);
        }
    });
}

/// Turning it on or off asks for the password, then the result comes back
/// through the controller.
pub(super) fn password_row(app: &mut App, ui: &mut Ui, enabled: bool) {
    let palette = Palette::current(ui.ctx());
    setting_row(
        ui,
        "Skip the password",
        "For automatic updates of system packages",
        SWITCH,
        |ui| {
            let mut on = !app.settings.system_approval.is_empty();
            if switch(ui, &mut on, enabled, "Skip the password").changed() {
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
    let can = app.settings.background_mode && app.platform.tray_available();
    let hint = if app.settings.background_mode {
        ""
    } else {
        NEEDS_BACKGROUND
    };
    setting_row(ui, "Start at login", hint, SWITCH, |ui| {
        let mut on = app.settings.autostart;
        if switch(ui, &mut on, can, "Start at login").changed() && app.c().set_autostart(on) {
            app.settings.autostart = on;
        }
    });
}

/// Turns background checks on or off. Off also stops starting at login;
/// false when that couldn't be undone.
fn set_background(app: &mut App, on: bool) -> bool {
    if !on && app.settings.autostart && !app.c().set_autostart(false) {
        return false;
    }
    if !on {
        app.settings.autostart = false;
    }
    app.settings.background_mode = on;
    app.sync_tray();
    true
}

/// A line with an icon, for what is happening now.
fn status_line(ui: &mut Ui, icon: &str, text: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        theme::paint_icon(ui.painter(), rect, icon, color);
        ui.label(egui::RichText::new(text).color(color));
    });
}

fn checks(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "Checks", |ui| {
        let background = app.settings.background_mode;
        setting_row(
            ui,
            "Background checks",
            "Keeps checking after you close the window",
            SWITCH,
            |ui| {
                let mut on = background;
                if switch(ui, &mut on, true, "Background checks").changed() {
                    set_background(app, on);
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
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            login_row(app, ui);
        }
        if let Some(at) = app.background.last_check {
            let count = app.background.available;
            let text = format!(
                "Checked {}, {count} update{}",
                model::short_datetime(at),
                if count == 1 { "" } else { "s" }
            );
            status_line(ui, "refresh", &text, palette.muted);
        }
    });
}

fn automatic_updates(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "Automatic updates", |ui| {
        auto_update_row(app, ui);
        setting_row(
            ui,
            "Allow removals",
            "Like an old kernel replaced by a new one",
            SWITCH,
            |ui| {
                let mut on = app.settings.allow_removals;
                if switch(ui, &mut on, true, "Allow removals").changed() {
                    app.settings.allow_removals = on;
                    app.c().set_allow_removals(on);
                }
            },
        );
        password_row(app, ui, true);
        if let Some(waiting) = app
            .background
            .waiting_text()
            .filter(|_| app.settings.background_mode && app.settings.auto_update)
        {
            status_line(ui, "clock", waiting, palette.muted);
        }
    });
}

fn notifications(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "Notifications", |ui| {
        let warning = if !app.platform.tray_available() {
            Some(if cfg!(target_os = "macos") {
                "No menu bar icon, so no notifications"
            } else {
                "No system tray, so no notifications"
            })
        } else if !app.platform.notifications_available() {
            Some(if cfg!(target_os = "macos") {
                "Off in System Settings"
            } else {
                "This system tray can't show notifications"
            })
        } else if app.platform.permission_needed() {
            Some("Allow PkgDeck to show its own icon")
        } else {
            None
        };
        // Said, not hidden behind a hover: it is why nothing pops up.
        if let Some(warning) = warning {
            status_line(ui, "warning", warning, palette.warning);
        }
        ui.horizontal(|ui| {
            if Button::new(Look::Soft(Tone::Accent), "Test notification")
                .icon("bell")
                .small()
                .enabled(app.settings.background_mode && app.platform.notifications_available())
                .show(ui)
                .clicked()
            {
                if app.platform.permission_needed() {
                    app.platform.request_permission();
                }
                app.platform
                    .notify("PkgDeck test", "Notifications are working.");
            }
            if app.platform.permission_needed()
                && Button::new(Look::Secondary, "Notification settings")
                    .small()
                    .show(ui)
                    .clicked()
            {
                app.platform.open_notification_settings();
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

/// Package managers that manage repositories in PkgDeck.
const REPOSITORY_SOURCES: [&str; 5] = ["flatpak", "fwupd", "apt", "dnf", "zypper"];

/// Every package manager found on this computer, with a switch, and the
/// repositories of those that have them.
fn sources(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let shown = shown_sources(&app.catalog);
    let enabled = app.enabled_sources();
    section(ui, "Package managers", |ui| {
        if app.catalog.is_empty() {
            ui.label(egui::RichText::new("Looking for package managers…").color(palette.muted));
        }
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
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&entry.summary)
                            .font(theme::font(12.5))
                            .color(palette.danger),
                    );
                    if Button::new(Look::Flat, "Retry")
                        .icon("refresh")
                        .small()
                        .show(ui)
                        .clicked()
                    {
                        app.c().recheck_sources();
                        app.react();
                    }
                });
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
        .filter(|entry| {
            entry.capabilities.is_empty()
                || entry
                    .capabilities
                    .iter()
                    .any(|capability| capability != "clean")
        })
        .cloned()
        .collect();
    shown.sort_by_key(|entry| model::source_name(&entry.source).to_lowercase());
    shown
}

/// Where Clean looks for old projects, kept in PkgDeck's own config file
/// for the engine and `pkd` to read too.
fn clean(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let id = Id::new("project-settings");
    let mut projects = ui
        .data_mut(|data| data.get_temp::<ProjectSettings>(id))
        .unwrap_or_else(|| ProjectSettings::load(&Host::current()));
    let before = projects.clone();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    section(ui, "Automatic cleaning", |ui| {
        auto_clean_row(app, ui);
        let background = app.settings.background_mode;
        setting_row(
            ui,
            "Clean finished worktrees",
            if background {
                "Once a day. Keeps branches"
            } else {
                NEEDS_BACKGROUND
            },
            SWITCH,
            |ui| {
                let mut on = app.settings.auto_clean_worktrees;
                if switch(ui, &mut on, background, "Clean finished worktrees").changed() {
                    app.settings.auto_clean_worktrees = on;
                    app.c().set_auto_clean_worktrees(on);
                }
            },
        );
        ui.horizontal(|ui| {
            if !background
                && Button::new(Look::Soft(Tone::Accent), "Turn on Background checks")
                    .small()
                    .show(ui)
                    .clicked()
            {
                set_background(app, true);
            }
            if Button::new(Look::Secondary, "Choose what to clean")
                .icon("remove")
                .small()
                .show(ui)
                .clicked()
            {
                app.open_page(model::Page::Clean);
            }
        });
    });
    ui.add_space(8.0);
    section(ui, "Old projects", |ui| {
        let step = project_step(projects.days);
        setting_row(
            ui,
            "Old after",
            &format!("{} without changes", days_text(projects.days)),
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
        worktree_protection(ui, &mut projects);
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Folders")
                .font(theme::font(14.5))
                .color(palette.ink),
        );
        let mut removed = None;
        for (index, folder) in projects.folders.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(shown_folder(folder, home.as_deref()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "remove", "Remove folder", palette.muted, true).clicked() {
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
            .map(|error| format!("Couldn't save: {error}"));
        ui.data_mut(|data| match error {
            Some(error) => {
                data.insert_temp(Id::new("project-settings-error"), error);
            }
            None => data.remove::<String>(Id::new("project-settings-error")),
        });
    }
    ui.data_mut(|data| data.insert_temp(id, projects));
}

/// What a finished worktree with uncommitted files needs before it goes:
/// one choice each, its cost said under it, the safest first.
fn worktree_protection(ui: &mut Ui, projects: &mut ProjectSettings) {
    let palette = Palette::current(ui.ctx());
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Worktrees with uncommitted files")
            .font(theme::font(14.5))
            .color(palette.ink),
    );
    for choice in WorktreeProtection::ALL {
        let risky = choice != WorktreeProtection::AnyLocalFiles;
        if radio(
            ui,
            choice == projects.worktree_protection,
            choice.name(),
            choice.description(),
            if risky {
                palette.warning
            } else {
                palette.muted
            },
        ) {
            projects.worktree_protection = choice;
        }
    }
}

/// A round choice with a line under it. True when clicked.
fn radio(ui: &mut Ui, on: bool, label: &str, hint: &str, hint_color: egui::Color32) -> bool {
    let palette = Palette::current(ui.ctx());
    let width = ui.available_width();
    let dot = 18.0;
    let text_left = dot + 10.0;
    let title = ui.painter().layout(
        label.to_owned(),
        if on {
            theme::bold(14.0)
        } else {
            theme::font(14.0)
        },
        palette.ink,
        width - text_left,
    );
    let note = ui.painter().layout(
        hint.to_owned(),
        theme::font(12.5),
        hint_color,
        width - text_left,
    );
    let height = (title.size().y + 2.0 + note.size().y).max(dot) + 8.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, label));
    // The line under it is read with it.
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_description(hint.to_owned()));
    let center = egui::pos2(
        rect.left() + dot / 2.0,
        rect.top() + 4.0 + title.size().y / 2.0,
    );
    let shown = ease(ui.ctx(), response.id.with("on"), on, FEEDBACK);
    let hover = ease(
        ui.ctx(),
        response.id.with("hover"),
        response.hovered(),
        FEEDBACK,
    );
    ui.painter().circle_stroke(
        center,
        dot / 2.0 - 1.0,
        Stroke::new(
            1.5,
            mix(
                mix(palette.strong_line, palette.ink, hover * 0.4),
                palette.accent,
                shown,
            ),
        ),
    );
    if shown > 0.0 {
        ui.painter()
            .circle_filled(center, (dot / 2.0 - 5.0) * shown, palette.accent);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            theme::round(6),
            Stroke::new(2.0, palette.accent),
            egui::StrokeKind::Outside,
        );
    }
    let text = egui::pos2(rect.left() + text_left, rect.top() + 4.0);
    let title_height = title.size().y;
    ui.painter().galley(text, title, palette.ink);
    ui.painter()
        .galley(text + vec2(0.0, title_height + 2.0), note, hint_color);
    response.clicked() && !on
}

fn about(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    // The tab already says what this is.
    section(ui, "", |ui| {
        ui.horizontal(|ui| {
            let (logo, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
            theme::paint_logo(ui.painter(), logo, &palette);
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
    ("Refresh or check for updates", "Ctrl+R"),
    ("Apply a confirmation", "Ctrl+Enter"),
    ("Clear search or close details", "Esc"),
    ("Quit", "Ctrl+Q"),
];

fn shortcuts(ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    section(ui, "Keyboard shortcuts", |ui| {
        let all = SHORTCUTS;
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
    fn worktree_protection_offers_every_choice_and_says_what_each_loses() {
        use egui_kittest::{
            kittest::{NodeT, Queryable},
            Harness,
        };
        let mut harness = Harness::builder()
            .with_size(vec2(760.0, 320.0))
            .build_ui_state(
                |ui, (ready, projects)| {
                    if !*ready {
                        crate::setup(ui.ctx(), Path::new("/"), None);
                        *ready = true;
                        return;
                    }
                    worktree_protection(ui, projects);
                },
                (false, ProjectSettings::default()),
            );
        harness.run_steps(4);
        // Every choice and its cost show at once, the safest chosen.
        for choice in WorktreeProtection::ALL {
            let node =
                harness.get_by_role_and_label(egui::accesskit::Role::RadioButton, choice.name());
            assert_eq!(
                node.accesskit_node().description(),
                Some(choice.description().to_owned())
            );
        }
        assert_eq!(
            harness.state().1.worktree_protection,
            WorktreeProtection::AnyLocalFiles
        );
        for choice in [
            WorktreeProtection::UncommittedChanges,
            WorktreeProtection::EditedTrackedFiles,
            WorktreeProtection::AnyLocalFiles,
        ] {
            harness
                .get_by_role_and_label(egui::accesskit::Role::RadioButton, choice.name())
                .click();
            harness.run_steps(3);
            assert_eq!(harness.state().1.worktree_protection, choice);
        }
    }

    #[test]
    fn sources_leave_out_managers_that_only_clean() {
        let catalog: Vec<model::Source> = serde_json::from_value(serde_json::json!([
            {"source":"apt", "available":true, "capabilities":["clean","upgrade"]},
            {"source":"projects", "available":true, "capabilities":["clean"]},
            {"source":"dev-caches", "available":true, "capabilities":["clean"]}
        ]))
        .unwrap();
        assert_eq!(
            shown_sources(&catalog)
                .iter()
                .map(|source| source.source.as_str())
                .collect::<Vec<_>>(),
            ["apt"]
        );
    }

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
    fn a_long_style_name_shrinks_to_its_card_and_a_short_one_keeps_its_size() {
        theme::painted(|ui| {
            let max = PREVIEW.x + PREVIEW_GAP;
            let ink = egui::Color32::WHITE;
            for style in Style::ALL {
                let name = fitted(ui.painter(), style.name(), theme::bold(13.0), ink, max);
                assert!(name.size().x <= max, "{style:?}");
            }
            let short = fitted(ui.painter(), "Modern", theme::bold(13.0), ink, max);
            let plain = ui
                .painter()
                .layout_no_wrap("Modern".into(), theme::bold(13.0), ink);
            assert_eq!(short.size(), plain.size());
            // One that is far too wide for the card comes out smaller.
            let text = "Mac OS X Tiger and Panther";
            let wide = fitted(ui.painter(), text, theme::bold(13.0), ink, max);
            let full = ui
                .painter()
                .layout_no_wrap(text.into(), theme::bold(13.0), ink);
            assert!(full.size().x > max);
            assert!(wide.size().x <= max && wide.size().y < full.size().y);
        });
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
