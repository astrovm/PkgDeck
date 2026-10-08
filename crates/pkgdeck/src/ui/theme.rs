//! PkgDeck's look: the colours of the Qt app's Theme.qml, the system's
//! fonts, and its line icons.

use super::icons;
use eframe::egui::{
    self, epaint::PathStroke, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Painter, Pos2, Rect, Shape, Stroke, TextStyle, Vec2,
};
use std::{path::Path, sync::Arc};

/// The colours of one appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub canvas: Color32,
    pub surface: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_ink: Color32,
    pub line: Color32,
    pub strong_line: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub heart: Color32,
    pub dark: bool,
}
fn tint(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (alpha * 255.0).round() as u8,
    )
}
fn over(base: Color32, top: Color32, amount: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(lerp(base.r(), top.r()), lerp(base.g(), top.g()), lerp(base.b(), top.b()))
}
impl Palette {
    pub fn of(dark: bool) -> Self {
        let rgb = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        let (ink, accent) = if dark {
            (rgb(0xe9edf3), rgb(0x7aa7ff))
        } else {
            (rgb(0x18212d), rgb(0x2f68d8))
        };
        Self {
            canvas: rgb(if dark { 0x0e1015 } else { 0xf4f6f9 }),
            surface: rgb(if dark { 0x161a21 } else { 0xffffff }),
            ink,
            muted: rgb(if dark { 0x98a3b3 } else { 0x5a6676 }),
            accent,
            accent_ink: rgb(if dark { 0x0e1015 } else { 0xffffff }),
            line: tint(ink, if dark { 0.12 } else { 0.13 }),
            strong_line: tint(ink, if dark { 0.3 } else { 0.32 }),
            hover: tint(ink, if dark { 0.06 } else { 0.05 }),
            selection: tint(accent, if dark { 0.2 } else { 0.14 }),
            danger: rgb(if dark { 0xf28b91 } else { 0xc1343f }),
            success: rgb(if dark { 0x6fd39a } else { 0x1b7a45 }),
            warning: rgb(if dark { 0xf2c56b } else { 0x9a6400 }),
            heart: rgb(0xe34b5f),
            dark,
        }
    }
    /// The hover tint laid over the surface, as one opaque colour.
    pub fn hover_solid(&self) -> Color32 {
        over(self.surface, self.ink, if self.dark { 0.07 } else { 0.05 })
    }
    /// The strong line laid over the surface, as one opaque colour.
    pub fn strong_line_solid(&self) -> Color32 {
        over(self.surface, self.ink, if self.dark { 0.28 } else { 0.24 })
    }
    /// The palette of what `ctx` shows now.
    pub fn current(ctx: &egui::Context) -> Self {
        Self::of(ctx.theme() == egui::Theme::Dark)
    }
}

pub const SMALL_RADIUS: u8 = 5;
pub const CONTROL_RADIUS: u8 = 9;
pub const CARD_RADIUS: u8 = 12;

/// The bold face, when the system has one that matches the regular face.
pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}
pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn bold(size: f32) -> FontId {
    FontId::new(size, bold_family())
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Font files to try, under the system's root: regular and bold of one
/// family, then a monospace face, then faces for scripts the first lack.
const SANS: &[(&str, &str)] = &[
    (
        "usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/noto/NotoSans-Regular.ttf",
        "usr/share/fonts/noto/NotoSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/google-noto/NotoSans-Regular.ttf",
        "usr/share/fonts/google-noto/NotoSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/TTF/DejaVuSans.ttf",
        "usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
    ),
    (
        "System/Library/Fonts/Supplemental/Arial.ttf",
        "System/Library/Fonts/Supplemental/Arial Bold.ttf",
    ),
];
const MONO: &[&str] = &[
    "usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    "usr/share/fonts/google-noto/NotoSansMono-Regular.ttf",
    "usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
    "System/Library/Fonts/Menlo.ttc",
];
const OTHER_SCRIPTS: &[&str] = &[
    "usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "System/Library/Fonts/Hiragino Sans GB.ttc",
];

/// Inter for text, the system's monospace face, and the system's faces
/// behind them for scripts Inter lacks, found under `root`.
pub fn fonts(root: &Path) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let read = |path: &str| std::fs::read(root.join(path)).ok();
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"))),
    );
    let mut regular = vec!["inter".to_owned()];
    let mut bold = vec!["inter-semibold".to_owned()];
    // The system's faces cover what Inter doesn't: other scripts, symbols.
    if let Some((system, system_bold)) = SANS
        .iter()
        .find_map(|(regular, bold)| Some((read(regular)?, read(bold)?)))
    {
        fonts.font_data.insert("system".into(), Arc::new(FontData::from_owned(system)));
        fonts.font_data.insert("system-bold".into(), Arc::new(FontData::from_owned(system_bold)));
        regular.push("system".into());
        bold.push("system-bold".into());
    }
    if let Some(bytes) = OTHER_SCRIPTS.iter().find_map(|path| read(path)) {
        fonts.font_data.insert("other-scripts".into(), Arc::new(FontData::from_owned(bytes)));
        regular.push("other-scripts".into());
        bold.push("other-scripts".into());
    }
    regular.extend(defaults.iter().cloned());
    bold.extend(defaults);
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts.families.insert(bold_family(), bold);
    if let Some(bytes) = MONO.iter().find_map(|path| read(path)) {
        fonts.font_data.insert("system-mono".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "system-mono".into());
    }
    fonts
}

/// PkgDeck's colours, shapes and type on both of egui's themes.
pub fn apply(ctx: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let palette = Palette::of(theme == egui::Theme::Dark);
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (TextStyle::Small, font(12.5)),
                (TextStyle::Body, font(14.5)),
                (TextStyle::Button, font(14.5)),
                (TextStyle::Heading, font(26.0)),
                (TextStyle::Monospace, mono(13.5)),
            ]
            .into();
            style.spacing.item_spacing = Vec2::new(10.0, 8.0);
            style.spacing.button_padding = Vec2::new(12.0, 7.0);
            style.spacing.interact_size.y = 34.0;
            style.interaction.selectable_labels = false;
            let visuals = &mut style.visuals;
            visuals.panel_fill = palette.canvas;
            visuals.window_fill = palette.surface;
            visuals.extreme_bg_color = palette.surface;
            visuals.faint_bg_color = palette.hover;
            visuals.hyperlink_color = palette.accent;
            visuals.error_fg_color = palette.danger;
            visuals.warn_fg_color = palette.warning;
            visuals.selection.bg_fill = tint(palette.accent, 0.35);
            visuals.selection.stroke = Stroke::new(1.0, palette.accent);
            visuals.window_stroke = Stroke::new(1.0, palette.line);
            visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.line);
            visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.ink);
            for (widget, fill) in [
                (&mut visuals.widgets.inactive, Color32::TRANSPARENT),
                (&mut visuals.widgets.hovered, palette.hover),
                (&mut visuals.widgets.active, tint(palette.ink, 0.12)),
                (&mut visuals.widgets.open, palette.hover),
            ] {
                widget.bg_fill = fill;
                widget.weak_bg_fill = fill;
                widget.bg_stroke = Stroke::new(1.0, palette.line);
                widget.fg_stroke = Stroke::new(1.0, palette.ink);
                widget.corner_radius = CornerRadius::same(CONTROL_RADIUS);
                widget.expansion = 0.0;
            }
        });
    }
}

/// Draw the icon called `name` into `rect`, as the Qt app's DeckIcon does.
pub fn paint_icon(painter: &Painter, rect: Rect, name: &str, color: Color32) {
    let scale = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x * scale, y * scale);
    let stroke = Stroke::new((1.7 * scale).max(1.0), color);
    if name == "heart" {
        // Filled: two lobes and the point below them.
        for x in [7.0, 17.0] {
            painter.circle_filled(at(x, 8.6), 5.0 * scale, color);
        }
        painter.add(Shape::convex_polygon(
            vec![at(2.3, 10.4), at(21.7, 10.4), at(12.0, 21.0)],
            color,
            Stroke::NONE,
        ));
        return;
    }
    for line in icons::strokes(name) {
        let points: Vec<Pos2> = line.chunks(2).map(|xy| at(xy[0], xy[1])).collect();
        // Round ends, as Qt's RoundCap draws them.
        for end in [points[0], points[points.len() - 1]] {
            painter.circle_filled(end, stroke.width / 2.0, color);
        }
        painter.add(Shape::line(points, PathStroke::from(stroke)));
    }
    let arc = |center: (f32, f32), radius: f32, from: f32, to: f32| {
        let steps = 32;
        let points: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let angle = (from + (to - from) * i as f32 / steps as f32).to_radians();
                at(
                    center.0 + radius * angle.cos(),
                    center.1 + radius * angle.sin(),
                )
            })
            .collect();
        painter.add(Shape::line(points, PathStroke::from(stroke)));
    };
    match name {
        "search" => arc((10.0, 10.0), 7.0, 0.0, 360.0),
        "info" | "help" => arc((12.0, 12.0), 10.0, 0.0, 360.0),
        "spinner" => arc((12.0, 12.0), 9.0, -90.0, 180.0),
        "refresh" => {
            arc((12.0, 12.0), 9.0, -158.4, -21.6);
            arc((12.0, 12.0), 9.0, 21.6, 158.4);
        }
        _ => {}
    }
}
