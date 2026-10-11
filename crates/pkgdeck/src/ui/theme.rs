//! PkgDeck's look: the colours of the chosen style, theme and accent, the
//! style's shapes, the fonts, and its line icons.

use super::icons;
use crate::settings::{Accent, DarkTheme, Density, LightTheme, Style, TextSize, Typeface};
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
    Color32::from_rgb(
        lerp(base.r(), top.r()),
        lerp(base.g(), top.g()),
        lerp(base.b(), top.b()),
    )
}
/// The person's choice of style, colours and size, for either appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scheme {
    pub dark: DarkTheme,
    pub light: LightTheme,
    pub accent: Accent,
    /// The colour [`Accent::Custom`] stands for.
    pub custom_accent: Option<Color32>,
    pub text_size: TextSize,
    pub style: Style,
    /// Times the style's own corner roundness, from 0 to 2.
    pub roundness: f32,
    pub density: Density,
    pub typeface: Typeface,
}
impl Default for Scheme {
    fn default() -> Self {
        Self {
            dark: DarkTheme::default(),
            light: LightTheme::default(),
            accent: Accent::default(),
            custom_accent: None,
            text_size: TextSize::default(),
            style: Style::default(),
            roundness: 1.0,
            density: Density::default(),
            typeface: Typeface::default(),
        }
    }
}

impl Scheme {
    /// What `settings` chose.
    pub fn of(settings: &crate::settings::Settings) -> Self {
        Self {
            dark: settings.dark_theme,
            light: settings.light_theme,
            accent: settings.accent,
            custom_accent: parse_hex(&settings.custom_accent),
            text_size: settings.text_size,
            style: settings.style,
            roundness: settings.roundness,
            density: settings.density,
            typeface: settings.typeface,
        }
    }
}

/// `#rrggbb` as a colour; anything else is none.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(rgb)
}
/// `color` as `#rrggbb`.
pub fn hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// The shapes of the style on screen, for painters that have no context
/// at hand. Each thread draws its own window, so tests don't mix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skin {
    pub style: Style,
    /// Times the style's own corner roundness.
    pub roundness: f32,
    pub density: Density,
}
impl Default for Skin {
    fn default() -> Self {
        Self {
            style: Style::Modern,
            roundness: 1.0,
            density: Density::Normal,
        }
    }
}
impl Skin {
    /// How round the style draws corners, before the person's choice.
    fn style_roundness(self) -> f32 {
        match self.style {
            Style::Modern | Style::Gel => 1.0,
            Style::Luna => 0.45,
            Style::Glass => 0.4,
            Style::Platinum => 0.3,
            Style::Retro => 0.0,
        }
    }
    /// A corner of `radius` at Modern's roundness, in this style.
    pub fn round(self, radius: u8) -> CornerRadius {
        let scale = self.style_roundness() * self.roundness.clamp(0.0, 2.0);
        CornerRadius::same((radius as f32 * scale).round().min(255.0) as u8)
    }
}
thread_local! {
    static SKIN: std::cell::Cell<Skin> = std::cell::Cell::new(Skin::default());
}
/// The style this thread's window shows.
pub fn skin() -> Skin {
    SKIN.with(std::cell::Cell::get)
}
/// Runs `paint` as if `skin` were on screen, for previews of a style.
pub fn with_skin<R>(skin: Skin, paint: impl FnOnce() -> R) -> R {
    let before = SKIN.with(|cell| cell.replace(skin));
    let result = paint();
    SKIN.with(|cell| cell.set(before));
    result
}
/// A corner of `radius` at Modern's roundness, in the style on screen.
pub fn round(radius: u8) -> CornerRadius {
    skin().round(radius)
}

fn scheme_id() -> egui::Id {
    egui::Id::new("pkgdeck-scheme")
}
fn scheme(ctx: &egui::Context) -> Scheme {
    ctx.data(|data| data.get_temp(scheme_id()).unwrap_or_default())
}
fn font_root_id() -> egui::Id {
    egui::Id::new("pkgdeck-font-root")
}
/// Shows `scheme` from now on, restyling only when it changed.
pub fn set_scheme(ctx: &egui::Context, scheme: Scheme) {
    SKIN.with(|skin| {
        skin.set(Skin {
            style: scheme.style,
            roundness: scheme.roundness,
            density: scheme.density,
        })
    });
    let before: Option<Scheme> = ctx.data(|data| data.get_temp(scheme_id()));
    if before == Some(scheme) {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(scheme_id(), scheme));
    if before.is_none_or(|before| before.text_size != scheme.text_size) {
        ctx.set_zoom_factor(scheme.text_size.zoom());
    }
    let face_of = |scheme: Scheme| face(scheme.style, scheme.typeface);
    // The window starts with Inter; a style's own face loads once chosen.
    if before.map_or(face_of(scheme), face_of) != face_of(scheme)
        || before.is_none() && face_of(scheme) != Face::Inter
    {
        let root: Option<std::path::PathBuf> = ctx.data(|data| data.get_temp(font_root_id()));
        if let Some(root) = root {
            ctx.set_fonts(fonts_with(&root, face_of(scheme)));
        }
    }
    apply(ctx);
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The accent's colour on a light and on a dark background.
fn accent_colors(accent: Accent) -> (u32, u32) {
    match accent {
        Accent::Blue => (0x2f68d8, 0x7aa7ff),
        Accent::Purple => (0x7247d1, 0xb49cff),
        Accent::Pink => (0xbf2f78, 0xf48fc4),
        Accent::Red => (0xc22f3a, 0xff8f8f),
        Accent::Orange => (0xb85300, 0xffab66),
        Accent::Green => (0x1d7a46, 0x72d39b),
        Accent::Teal => (0x0b7580, 0x5ccfd6),
        Accent::Graphite => (0x3d3d3d, 0xd4d4d4),
        Accent::Navy => (0x000080, 0x7f9cff),
        Accent::Lavender => (0x5b5bb5, 0xb7b7ff),
        // Drawn from `custom_accent`; blue stands in until one is picked.
        Accent::Custom => (0x2f68d8, 0x7aa7ff),
    }
}

/// How bright `color` looks, from 0 to 1.
fn luminance(color: Color32) -> f32 {
    let channel = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}
/// How far apart two colours' brightness is, from 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
/// A picked accent, lightened or darkened until links and buttons in it
/// read on the canvas.
fn readable_accent(color: Color32, canvas: Color32, dark: bool) -> Color32 {
    let toward = if dark { Color32::WHITE } else { Color32::BLACK };
    (0..=10)
        .map(|step| over(color, toward, step as f32 * 0.08))
        .find(|candidate| contrast(*candidate, canvas) >= 3.0)
        .unwrap_or(toward)
}

impl Palette {
    pub fn of(scheme: Scheme, dark: bool) -> Self {
        // Canvas, surface, ink and muted text: Modern's come from the chosen
        // theme, the other styles bring their own.
        let (canvas, surface, ink, muted) = match (scheme.style, dark) {
            (Style::Modern, true) => match scheme.dark {
                DarkTheme::Charcoal => (0x121212, 0x1d1d1d, 0xececec, 0xa3a3a3),
                DarkTheme::Black => (0x000000, 0x111111, 0xe6e6e6, 0x9a9a9a),
                DarkTheme::Slate => (0x0e1015, 0x161a21, 0xe9edf3, 0x98a3b3),
            },
            (Style::Modern, false) => match scheme.light {
                LightTheme::Classic => (0xe8edf5, 0xf7f9fd, 0x152135, 0x4e5c72),
                LightTheme::White => (0xf3f3f3, 0xffffff, 0x1b1b1b, 0x5f5f5f),
                LightTheme::Paper => (0xf2ede3, 0xfbf8f2, 0x2a251e, 0x6b6357),
            },
            // High contrast, as the old desktops shipped for the dark.
            (Style::Retro, true) => (0x000000, 0x0c0c0c, 0xffffff, 0xc0c0c0),
            (Style::Retro, false) => (0xc0c0c0, 0xffffff, 0x000000, 0x404040),
            (Style::Luna, true) => (0x1d2230, 0x282e3e, 0xf0f2f6, 0xa9b1c2),
            (Style::Luna, false) => (0xece9d8, 0xffffff, 0x000000, 0x575757),
            (Style::Glass, true) => (0x0b1017, 0x151d28, 0xe8eef5, 0x95a3b5),
            (Style::Glass, false) => (0xd8e3f0, 0xfbfcfe, 0x1e1e1e, 0x56606c),
            (Style::Gel, true) => (0x262626, 0x313131, 0xeeeeee, 0xa3a3a3),
            (Style::Gel, false) => (0xe6e6e6, 0xffffff, 0x000000, 0x555555),
            (Style::Platinum, true) => (0x2e2e2e, 0x3b3b3b, 0xeeeeee, 0xababab),
            (Style::Platinum, false) => (0xdddddd, 0xeeeeee, 0x000000, 0x4a4a4a),
        };
        let (on_light, on_dark) = accent_colors(scheme.accent);
        let (canvas, ink) = (rgb(canvas), rgb(ink));
        let accent = match scheme.custom_accent {
            Some(color) if scheme.accent == Accent::Custom => readable_accent(color, canvas, dark),
            _ => rgb(if dark { on_dark } else { on_light }),
        };
        Self {
            canvas,
            surface: rgb(surface),
            ink,
            muted: rgb(muted),
            accent,
            accent_ink: if scheme.accent == Accent::Custom {
                // Whichever reads better on a colour picked by hand.
                let dark_ink = if dark { canvas } else { rgb(0x000000) };
                if contrast(Color32::WHITE, accent) >= contrast(dark_ink, accent) {
                    Color32::WHITE
                } else {
                    dark_ink
                }
            } else if dark {
                canvas
            } else {
                rgb(0xffffff)
            },
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
        Self::of(scheme(ctx), ctx.theme() == egui::Theme::Dark)
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

/// The font file at `path`, mapped from disk rather than read into memory:
/// the system shares those pages with every program using the font and
/// can drop them when short of memory. Mapped once per run.
#[allow(unsafe_code)]
fn mapped(path: &Path) -> Option<&'static [u8]> {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };
    static MAPPED: OnceLock<Mutex<HashMap<std::path::PathBuf, &'static [u8]>>> = OnceLock::new();
    let mut mapped = MAPPED.get_or_init(Default::default).lock().ok()?;
    if let Some(bytes) = mapped.get(path) {
        return Some(bytes);
    }
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: system font files are only replaced, not changed in place,
    // by package managers; a replaced file keeps the mapped one alive.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes: &'static [u8] = &Box::leak(Box::new(map))[..];
    mapped.insert(path.to_owned(), bytes);
    Some(bytes)
}

/// Regular and bold faces a style was drawn with, in the order they're
/// tried: the font, or a close one, where systems keep them.
const TAHOMA: &[(&str, &str)] = &[
    (
        "usr/share/wine/fonts/tahoma.ttf",
        "usr/share/wine/fonts/tahomabd.ttf",
    ),
    (
        "usr/share/fonts/truetype/wine/tahoma.ttf",
        "usr/share/fonts/truetype/wine/tahomabd.ttf",
    ),
    (
        "System/Library/Fonts/Supplemental/Tahoma.ttf",
        "System/Library/Fonts/Supplemental/Tahoma Bold.ttf",
    ),
    (
        "usr/share/fonts/truetype/msttcorefonts/Verdana.ttf",
        "usr/share/fonts/truetype/msttcorefonts/Verdana_Bold.ttf",
    ),
    (
        "usr/share/fonts/msttcore/verdana.ttf",
        "usr/share/fonts/msttcore/verdanab.ttf",
    ),
    (
        "System/Library/Fonts/Supplemental/Verdana.ttf",
        "System/Library/Fonts/Supplemental/Verdana Bold.ttf",
    ),
];
const LUCIDA: &[(&str, &str)] = &[
    (
        "System/Library/Fonts/LucidaGrande.ttc",
        "System/Library/Fonts/LucidaGrande.ttc",
    ),
    (
        "usr/share/fonts/truetype/lucida/LucidaSansRegular.ttf",
        "usr/share/fonts/truetype/lucida/LucidaSansDemiBold.ttf",
    ),
];

/// Which faces draw text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Inter,
    System,
    /// One of a style's own, from the list given, else Inter.
    Style(&'static [(&'static str, &'static str)]),
}

/// The face `typeface` stands for in `style`.
pub fn face(style: Style, typeface: Typeface) -> Face {
    match typeface {
        Typeface::Inter => Face::Inter,
        Typeface::System => Face::System,
        Typeface::Style => match style {
            Style::Retro | Style::Luna => Face::Style(TAHOMA),
            Style::Gel | Style::Platinum => Face::Style(LUCIDA),
            Style::Modern | Style::Glass => Face::Inter,
        },
    }
}

/// Inter for text, the system's monospace face, and the system's faces
/// behind them for scripts Inter lacks, found under `root`.
pub fn fonts(root: &Path) -> FontDefinitions {
    fonts_with(root, Face::Inter)
}

/// Remembers where fonts are looked for, so a style can load its own.
pub fn set_font_root(ctx: &egui::Context, root: &Path) {
    ctx.data_mut(|data| data.insert_temp(font_root_id(), root.to_path_buf()));
}

/// [`fonts`], with `face` first.
pub fn fonts_with(root: &Path, face: Face) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let read = |path: &str| mapped(&root.join(path));
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    let mut regular = vec!["inter".to_owned()];
    let mut bold = vec!["inter-semibold".to_owned()];
    // A style's own face goes first, when the system has it. Collections
    // keep the bold face second.
    if let Face::Style(candidates) = face {
        if let Some((style, style_bold, same)) = candidates
            .iter()
            .find_map(|(regular, bold)| Some((read(regular)?, read(bold)?, regular == bold)))
        {
            fonts
                .font_data
                .insert("style".into(), Arc::new(FontData::from_static(style)));
            let mut heavy = FontData::from_static(style_bold);
            if same {
                heavy.index = 1;
            }
            fonts.font_data.insert("style-bold".into(), Arc::new(heavy));
            regular.insert(0, "style".into());
            bold.insert(0, "style-bold".into());
        }
    }
    // The system's faces cover what Inter doesn't: other scripts, symbols.
    if let Some((system, system_bold)) = SANS
        .iter()
        .find_map(|(regular, bold)| Some((read(regular)?, read(bold)?)))
    {
        fonts
            .font_data
            .insert("system".into(), Arc::new(FontData::from_static(system)));
        fonts.font_data.insert(
            "system-bold".into(),
            Arc::new(FontData::from_static(system_bold)),
        );
        if face == Face::System {
            regular.insert(0, "system".into());
            bold.insert(0, "system-bold".into());
        } else {
            regular.push("system".into());
            bold.push("system-bold".into());
        }
    }
    if let Some(bytes) = OTHER_SCRIPTS.iter().find_map(|path| read(path)) {
        fonts.font_data.insert(
            "other-scripts".into(),
            Arc::new(FontData::from_static(bytes)),
        );
        regular.push("other-scripts".into());
        bold.push("other-scripts".into());
    }
    regular.extend(defaults.iter().cloned());
    bold.extend(defaults);
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts.families.insert(bold_family(), bold);
    if let Some(bytes) = MONO.iter().find_map(|path| read(path)) {
        fonts
            .font_data
            .insert("system-mono".into(), Arc::new(FontData::from_static(bytes)));
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "system-mono".into());
    }
    fonts
}

/// PkgDeck's colours, shapes and type on both of egui's themes, in the
/// scheme last set.
pub fn apply(ctx: &egui::Context) {
    let scheme = scheme(ctx);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let palette = Palette::of(scheme, theme == egui::Theme::Dark);
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
                widget.corner_radius = round(CONTROL_RADIUS);
                widget.expansion = 0.0;
            }
        });
    }
}

/// Draw a source's mark into `rect`: its icon, or its monogram when it has
/// no drawing of its own.
pub fn paint_source_icon(painter: &Painter, rect: Rect, id: &str, color: Color32) {
    if icons::drawn(id) {
        paint_icon(painter, rect, id, color);
        return;
    }
    let letters = icons::monogram(id);
    // Too small for two letters to read: the first alone, unboxed.
    if rect.width() < 16.0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letters.get(..1).unwrap_or(""),
            bold(rect.height() * 0.95),
            color,
        );
        return;
    }
    let scale = rect.width() / 24.0;
    painter.rect_stroke(
        rect.shrink(2.5 * scale),
        CornerRadius::same((5.0 * scale) as u8),
        Stroke::new((1.7 * scale).max(1.0), color),
        egui::StrokeKind::Middle,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letters,
        bold(rect.height() * 0.4),
        color,
    );
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
        "info" | "help" | "clock" => arc((12.0, 12.0), 10.0, 0.0, 360.0),
        "spinner" => arc((12.0, 12.0), 9.0, -90.0, 180.0),
        "system-image" => {
            arc((12.0, 12.0), 9.5, 0.0, 360.0);
            arc((12.0, 12.0), 2.5, 0.0, 360.0);
        }
        "rustup" => {
            arc((12.0, 12.0), 7.5, 0.0, 360.0);
            arc((12.0, 12.0), 2.5, 0.0, 360.0);
        }
        "refresh" => {
            arc((12.0, 12.0), 9.0, -158.4, -21.6);
            arc((12.0, 12.0), 9.0, 21.6, 158.4);
        }
        _ => {}
    }
}

/// The shapes `paint` lays down in one frame, flattened.
#[cfg(test)]
pub fn painted(paint: impl FnOnce(&mut egui::Ui)) -> Vec<Shape> {
    fn flatten(shape: Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.into_iter().for_each(|shape| flatten(shape, out)),
            shape => out.push(shape),
        }
    }
    let ctx = egui::Context::default();
    // The app's fonts, as the window has them, take a frame to bind.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    crate::setup(&ctx, &root, None);
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut paint = Some(paint);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(paint) = paint.take() {
            paint(ui);
        }
    });
    output.textures_delta.clear();
    let mut shapes = Vec::new();
    for clipped in output.shapes {
        flatten(clipped.shape, &mut shapes);
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(paint: impl FnOnce(&Painter)) -> Vec<Shape> {
        painted(|ui| paint(ui.painter()))
    }

    /// The text shapes: what they say and at what size.
    fn texts(shapes: &[Shape]) -> Vec<(String, f32)> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.job.sections[0].format.font_id.size,
                )),
                _ => None,
            })
            .collect()
    }

    fn square(at: f32, side: f32) -> Rect {
        Rect::from_min_size(Pos2::new(at, at), Vec2::splat(side))
    }

    #[test]
    fn circle_icons_draw_their_rings() {
        // Rings are 33 point lines; straight strokes are shorter.
        let rings = |name: &str| {
            paint(|painter| paint_icon(painter, square(0.0, 24.0), name, Color32::WHITE))
                .iter()
                .filter(|shape| matches!(shape, Shape::Path(path) if path.points.len() > 30))
                .count()
        };
        assert_eq!(rings("system-image"), 2);
        assert_eq!(rings("rustup"), 2);
        assert_eq!(rings("package"), 0);
    }

    #[test]
    fn drawn_sources_paint_their_icon_and_others_their_letters() {
        let apt =
            paint(|painter| paint_source_icon(painter, square(0.0, 24.0), "apt", Color32::WHITE));
        assert!(texts(&apt).is_empty());
        assert!(!apt.is_empty());

        // Big enough for both letters, boxed, all in proportion.
        let big =
            paint(|painter| paint_source_icon(painter, square(10.0, 48.0), "dnf", Color32::WHITE));
        assert_eq!(texts(&big), [("Dn".to_owned(), 48.0 * 0.4)]);
        let frame = big
            .iter()
            .find_map(|shape| match shape {
                Shape::Rect(rect) => Some(rect.clone()),
                _ => None,
            })
            .expect("a box");
        assert_eq!(frame.rect, square(10.0, 48.0).shrink(5.0));
        assert_eq!(frame.corner_radius, CornerRadius::same(10));
        assert_eq!(frame.stroke.width, 3.4);

        // From 16 points the box shows; below, just the first letter.
        let at = |side: f32| {
            paint(|painter| paint_source_icon(painter, square(0.0, side), "dnf", Color32::WHITE))
        };
        let sixteen = at(16.0);
        assert_eq!(texts(&sixteen)[0].0, "Dn");
        assert!(sixteen.iter().any(|shape| matches!(shape, Shape::Rect(_))));
        let small = at(15.0);
        assert_eq!(texts(&small), [("D".to_owned(), 15.0 * 0.95)]);
        assert!(!small.iter().any(|shape| matches!(shape, Shape::Rect(_))));
    }
}
