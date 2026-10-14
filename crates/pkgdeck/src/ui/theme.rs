//! PkgDeck's look: the colours of the chosen style, theme and accent, the
//! style's shapes, the fonts, and its line icons.

use super::icons;
use super::skin::lerp;
use crate::settings::{Accent, DarkTheme, Density, Family, LightTheme, Style, TextSize, Typeface};
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
    /// The accent deep enough for white text on it, as the older styles
    /// fill title bars and selections whatever the appearance.
    pub deep: Color32,
    /// The older styles' title bar fill: [`Palette::deep`] when the window
    /// bar is tinted, else a neutral grey white text still reads on.
    pub bar: Color32,
    /// The accent bright enough to read on black.
    pub bright: Color32,
    pub line: Color32,
    pub strong_line: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub heart: Color32,
    /// Windows 7's glass colour, its alpha the colour intensity, from the
    /// swatches it shipped.
    pub glass: Color32,
    /// The accent's hue, which [`Palette::tone`] mixes into greys.
    pub tint: Color32,
    /// How much of [`Palette::tint`] the window's greys take.
    pub tint_amount: f32,
    pub dark: bool,
}
/// The title bar grey when the window bar is not tinted: dark enough for
/// white text on a light window, a step lighter than a dark one.
fn neutral_bar(canvas: Color32, dark: bool) -> Color32 {
    // The canvas's average brightness, with no hue of its own.
    let average = (u16::from(canvas.r()) + u16::from(canvas.g()) + u16::from(canvas.b())) / 3;
    let grey = Color32::from_gray(average as u8);
    if dark {
        lerp(grey, Color32::WHITE, 0.25)
    } else {
        lerp(grey, Color32::BLACK, 0.6)
    }
}

/// The glass colour Windows 7 shipped closest to `accent`, its alpha the
/// colour intensity; dark windows take the deep swatches.
fn glass_color(accent: Accent, custom: Option<Color32>, dark: bool) -> Color32 {
    let argb: u32 = match accent {
        // Twilight.
        Accent::Blue | Accent::Navy if dark => 0xA80046AD,
        // Sky, 7's default.
        Accent::Blue => 0x6B74B8FC,
        Accent::Navy => 0xA80046AD,
        // Sea.
        Accent::Teal => 0x8032CDCD,
        // Leaf.
        Accent::Green => 0x6614A600,
        // Ruby.
        Accent::Red => 0xA8CE0F0F,
        // Pumpkin.
        Accent::Orange => 0x80FF9C00,
        // Fuchsia.
        Accent::Pink => 0x66FF0099,
        // Violet.
        Accent::Purple => 0x856E3BA1,
        Accent::Lavender => 0x528D5A94,
        // Slate.
        Accent::Graphite => 0x80555555,
        Accent::Custom => 0x80000000,
    };
    let [a, r, g, b] = argb.to_be_bytes();
    match custom {
        Some(color) => Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
        None => Color32::from_rgba_unmultiplied(r, g, b, a),
    }
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
    /// Whether the accent tints the greys of the window; see
    /// [`Settings::tint_backgrounds`](crate::settings::Settings::tint_backgrounds).
    pub tint_backgrounds: bool,
    /// Whether the accent colours the older styles' title bars; see
    /// [`Settings::window_bar_tint`](crate::settings::Settings::window_bar_tint).
    pub tint_window_bar: bool,
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
            tint_backgrounds: false,
            tint_window_bar: true,
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
            tint_backgrounds: settings.tint_backgrounds,
            tint_window_bar: settings.window_bar_tint(settings.style),
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
    /// The desktop the style on screen is a version of.
    pub fn family(self) -> Family {
        self.style.family()
    }
    /// How round the style draws corners, before the person's choice.
    fn style_roundness(self) -> f32 {
        match self.family() {
            Family::Modern => 1.0,
            // Windows 11's corners are small and even: 4 on controls, 8 on
            // cards.
            Family::Fluent => 0.55,
            // Liquid Glass rounds everything, to a capsule on controls.
            Family::Liquid => 1.1,
            Family::Luna => 0.45,
            // Aqua's panels are nearly square; its buttons are pills.
            Family::Gel => 0.35,
            Family::Glass => 0.4,
            Family::Retro => 0.0,
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
    static SEE_THROUGH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Whether this thread's window lets the desktop show through its glass,
/// blurred: the desktop blurs behind it and the style has glass.
pub fn see_through() -> bool {
    SEE_THROUGH.with(std::cell::Cell::get)
}
/// Sets [`see_through`] for this thread's window.
pub fn set_see_through(see_through: bool) {
    SEE_THROUGH.with(|cell| cell.set(see_through));
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
pub(super) fn luminance(color: Color32) -> f32 {
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
    /// Canvas, surface, ink and muted text as the style's desktop draws
    /// them in every version.
    fn family_colors(scheme: Scheme, dark: bool) -> (u32, u32, u32, u32) {
        match (scheme.style.family(), dark) {
            (Family::Modern, true) => match scheme.dark {
                DarkTheme::Charcoal => (0x121212, 0x1d1d1d, 0xececec, 0xa3a3a3),
                DarkTheme::Black => (0x000000, 0x111111, 0xe6e6e6, 0x9a9a9a),
                DarkTheme::Slate => (0x0e1015, 0x161a21, 0xe9edf3, 0x98a3b3),
            },
            (Family::Modern, false) => match scheme.light {
                LightTheme::Classic => (0xe8edf5, 0xf7f9fd, 0x152135, 0x4e5c72),
                LightTheme::White => (0xf3f3f3, 0xffffff, 0x1b1b1b, 0x5f5f5f),
                LightTheme::Paper => (0xf2ede3, 0xfbf8f2, 0x2a251e, 0x6b6357),
            },
            // Windows 98's grey turned down: the same bevels on a dark face,
            // fields darker still.
            (Family::Retro, true) => (0x3a3a3a, 0x1f1f1f, 0xeeeeee, 0xb0b0b0),
            (Family::Retro, false) => (0xc0c0c0, 0xffffff, 0x000000, 0x404040),
            (Family::Luna, true) => (0x1d2230, 0x282e3e, 0xf0f2f6, 0xa9b1c2),
            (Family::Luna, false) => (0xece9d8, 0xffffff, 0x000000, 0x575757),
            // Explorer's white; in the dark, Media Center's blue.
            (Family::Glass, true) => (0x04142f, 0x0b2148, 0xeef3fa, 0x9fb4d3),
            (Family::Glass, false) => (0xffffff, 0xffffff, 0x000000, 0x5a6270),
            (Family::Gel, true) => (0x1f1f1f, 0x2b2b2b, 0xe6e6e6, 0x9c9c9c),
            (Family::Gel, false) => (0xececec, 0xffffff, 0x000000, 0x555555),
            // Windows 11: Mica's grey with cards a step lighter.
            (Family::Fluent, true) => (0x202020, 0x2b2b2b, 0xffffff, 0xc5c5c5),
            (Family::Fluent, false) => (0xf3f3f3, 0xfbfbfb, 0x1b1b1b, 0x5f5f5f),
            // macOS: the window's grey behind white content.
            (Family::Liquid, true) => (0x1e1e20, 0x2a2a2d, 0xf5f5f7, 0x9d9da3),
            (Family::Liquid, false) => (0xf0f0f2, 0xffffff, 0x1d1d1f, 0x6e6e73),
        }
    }

    pub fn of(scheme: Scheme, dark: bool) -> Self {
        // Canvas, surface, ink and muted text: Modern's come from the chosen
        // theme, the other styles bring their own.
        let (canvas, surface, ink, muted) = Self::family_colors(scheme, dark);
        let (on_light, on_dark) = accent_colors(scheme.accent);
        let (canvas, ink) = (rgb(canvas), rgb(ink));
        let custom = scheme
            .custom_accent
            .filter(|_| scheme.accent == Accent::Custom);
        let (deep, bright) = match custom {
            Some(color) => (
                readable_accent(color, Color32::WHITE, false),
                readable_accent(color, Color32::BLACK, true),
            ),
            None => (rgb(on_light), rgb(on_dark)),
        };
        // The accent tints the window's greys: backgrounds, panels and
        // buttons. Every accent gets the same amount, so the tint switch
        // does the same thing whatever colour is picked.
        let hue = custom.unwrap_or(rgb(on_light));
        let tint_amount = match (scheme.style.family(), dark) {
            _ if !scheme.tint_backgrounds => 0.0,
            (Family::Modern | Family::Fluent | Family::Liquid, _) if dark => 0.05,
            (Family::Modern | Family::Fluent | Family::Liquid, _) => 0.04,
            (Family::Glass, false) => 0.08,
            (_, true) => 0.06,
            (_, false) => 0.08,
        };
        let tone = |color: Color32, share: f32| over(color, hue, tint_amount * share);
        // OLED black stays black.
        let canvas = if canvas == Color32::BLACK {
            canvas
        } else {
            tone(canvas, 1.0)
        };
        let surface = tone(rgb(surface), 0.5);
        // Media Center's navy and XP's slate are blue walls, so dark goes
        // near black in every accent, its own blue included. The accent is
        // left to the frame and selection.
        let (canvas, surface) = if dark && scheme.style.family() == Family::Glass {
            (tone(rgb(0x0e0f11), 1.0), tone(rgb(0x17191c), 1.0))
        } else if dark && scheme.style.family() == Family::Luna {
            (tone(rgb(0x1f2023), 1.0), tone(rgb(0x2a2b2f), 1.0))
        } else {
            (canvas, surface)
        };
        // Picked colours are made to read on the tinted canvas.
        let accent = match custom {
            Some(color) => readable_accent(color, canvas, dark),
            _ => rgb(if dark { on_dark } else { on_light }),
        };
        // A grey accent fills title bars and selections as dark as a dark
        // window itself; lift it until the fill stands out, white text
        // still reading on it.
        let deep = if dark {
            (0..=6)
                .map(|step| over(deep, Color32::WHITE, step as f32 * 0.05))
                .find(|fill| {
                    contrast(*fill, canvas) >= 1.25 || contrast(Color32::WHITE, *fill) < 4.5
                })
                .unwrap_or(deep)
        } else {
            deep
        };
        Self {
            canvas,
            surface,
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
            deep,
            bar: if scheme.tint_window_bar {
                deep
            } else {
                neutral_bar(canvas, dark)
            },
            bright,
            line: tint(ink, if dark { 0.12 } else { 0.13 }),
            strong_line: tint(ink, if dark { 0.3 } else { 0.32 }),
            hover: tint(ink, if dark { 0.06 } else { 0.05 }),
            selection: tint(accent, if dark { 0.2 } else { 0.14 }),
            danger: rgb(if dark { 0xf28b91 } else { 0xc1343f }),
            success: rgb(if dark { 0x6fd39a } else { 0x1b7a45 }),
            warning: rgb(if dark { 0xf2c56b } else { 0x9a6400 }),
            heart: rgb(0xe34b5f),
            glass: if scheme.tint_window_bar {
                glass_color(scheme.accent, custom, dark)
            } else {
                // Keep the glass's intensity, lose its hue.
                let grey = neutral_bar(canvas, dark);
                let alpha = glass_color(scheme.accent, custom, dark).a();
                Color32::from_rgba_unmultiplied(grey.r(), grey.g(), grey.b(), alpha)
            },
            tint: hue,
            tint_amount,
            dark,
        }
    }
    /// A grey the style draws, tinted by the accent as the window is.
    pub fn tone(&self, color: Color32) -> Color32 {
        over(color, self.tint, self.tint_amount)
    }
    /// The hover tint laid over the surface, as one opaque colour.
    pub fn hover_solid(&self) -> Color32 {
        over(self.surface, self.ink, if self.dark { 0.07 } else { 0.05 })
    }
    /// The strong line laid over the surface, as one opaque colour.
    pub fn strong_line_solid(&self) -> Color32 {
        over(self.surface, self.ink, if self.dark { 0.28 } else { 0.24 })
    }
    /// A rail that reads on the card and on the window behind it, since
    /// group boxes leave controls on the window's own colour.
    pub fn rail(&self) -> Color32 {
        let (card, window) = (luminance(self.surface), luminance(self.canvas));
        let base = if (window < card) != self.dark {
            self.canvas
        } else {
            self.surface
        };
        over(base, self.ink, if self.dark { 0.28 } else { 0.24 })
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
/// The face for window titles.
pub fn title_family() -> FontFamily {
    FontFamily::Name("title".into())
}
pub fn title(size: f32) -> FontId {
    FontId::new(size, title_family())
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
    // Drawn after Verdana, as Tahoma was, and narrow like it.
    (
        "usr/share/fonts/truetype/dejavu/DejaVuSansCondensed.ttf",
        "usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-Bold.ttf",
    ),
    (
        "usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed.ttf",
        "usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed-Bold.ttf",
    ),
    (
        "usr/share/fonts/TTF/DejaVuSansCondensed.ttf",
        "usr/share/fonts/TTF/DejaVuSansCondensed-Bold.ttf",
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
/// Windows 98 drew MS Sans Serif, a pixel font; W95FA redraws it.
const MS_SANS: &[(&str, &str)] = &[("bundled:W95FA.otf", "bundled:W95FA-Bold.ttf")];
/// XP's title bars: Trebuchet MS in bold.
const TREBUCHET_BOLD: &[&str] = &[
    "usr/share/fonts/truetype/msttcorefonts/trebucbd.ttf",
    "usr/share/fonts/truetype/msttcorefonts/Trebuchet_MS_Bold.ttf",
    "usr/share/fonts/msttcore/trebucbd.ttf",
    "System/Library/Fonts/Supplemental/Trebuchet MS Bold.ttf",
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
    // As wide and open, and on nearly every Linux desktop.
    (
        "usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
    ),
    (
        "usr/share/fonts/TTF/DejaVuSans.ttf",
        "usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    ),
];
/// Windows 7 drew Segoe UI; Microsoft's Selawik is made to stand in.
const SEGOE: &[(&str, &str)] = &[(
    "bundled:Selawik-Regular.ttf",
    "bundled:Selawik-Semibold.ttf",
)];
/// A font PkgDeck carries, by the name its list gives it.
fn bundled(path: &str) -> Option<&'static [u8]> {
    Some(match path.strip_prefix("bundled:")? {
        "W95FA.otf" => include_bytes!("../../assets/fonts/W95FA.otf"),
        "W95FA-Bold.ttf" => include_bytes!("../../assets/fonts/W95FA-Bold.ttf"),
        "Selawik-Regular.ttf" => include_bytes!("../../assets/fonts/Selawik-Regular.ttf"),
        "Selawik-Semibold.ttf" => include_bytes!("../../assets/fonts/Selawik-Semibold.ttf"),
        "Inter-Regular.ttf" => include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
        _ => return None,
    })
}

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
        Typeface::Style => match style.family() {
            Family::Retro => Face::Style(MS_SANS),
            Family::Luna => Face::Style(TAHOMA),
            Family::Gel => Face::Style(LUCIDA),
            Family::Glass => Face::Style(SEGOE),
            // Selawik stands in for Segoe UI Variable, Inter for SF Pro.
            Family::Fluent => Face::Style(SEGOE),
            Family::Modern | Family::Liquid => Face::Inter,
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
    let read = |path: &str| bundled(path).or_else(|| mapped(&root.join(path)));
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
    // Title bars take bold, or XP's own face for them.
    let mut title = bold.clone();
    if face == Face::Style(TAHOMA) {
        if let Some(bytes) = TREBUCHET_BOLD.iter().find_map(|path| read(path)) {
            fonts
                .font_data
                .insert("title".into(), Arc::new(FontData::from_static(bytes)));
            title.insert(0, "title".into());
        }
    }
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts.families.insert(bold_family(), bold);
    fonts.families.insert(title_family(), title);
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
            // The older styles' scroll bars are always there, wide and solid.
            if let Some((width, handle)) = scroll_bar(scheme.style, &palette) {
                style.spacing.scroll = egui::style::ScrollStyle {
                    bar_width: width,
                    bar_inner_margin: 0.0,
                    handle_min_length: 20.0,
                    dormant_background_opacity: 1.0,
                    active_background_opacity: 1.0,
                    interact_background_opacity: 1.0,
                    dormant_handle_opacity: 1.0,
                    active_handle_opacity: 1.0,
                    interact_handle_opacity: 1.0,
                    ..egui::style::ScrollStyle::solid()
                };
                let visuals = &mut style.visuals;
                visuals.extreme_bg_color = over(palette.surface, palette.canvas, 0.6);
                visuals.widgets.inactive.bg_fill = handle;
                visuals.widgets.hovered.bg_fill = over(handle, palette.ink, 0.08);
                visuals.widgets.active.bg_fill = over(handle, palette.ink, 0.16);
            }
        });
    }
}

/// How wide a style's scroll bars are and the colour of their handle;
/// none for Modern's, which float over the content.
pub fn scroll_bar(style: Style, palette: &Palette) -> Option<(f32, Color32)> {
    let handle = match style.family() {
        Family::Modern | Family::Fluent | Family::Liquid => return None,
        Family::Retro => palette.canvas,
        Family::Luna => over(
            palette.surface,
            palette.deep,
            if palette.dark { 0.45 } else { 0.28 },
        ),
        Family::Glass => over(palette.canvas, palette.ink, 0.12),
        Family::Gel => over(palette.surface, palette.deep, 0.6),
    };
    let width = match style.family() {
        Family::Gel => 15.0,
        Family::Luna | Family::Glass => 17.0,
        _ => 16.0,
    };
    Some((width, handle))
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

/// The logo's three faces in its 128-unit square: top, left and right.
const LOGO: [[(f32, f32); 4]; 3] = [
    [(64.0, 8.0), (112.0, 36.0), (64.0, 64.0), (16.0, 36.0)],
    [(16.0, 36.0), (64.0, 64.0), (64.0, 120.0), (16.0, 92.0)],
    [(64.0, 64.0), (112.0, 36.0), (112.0, 92.0), (64.0, 120.0)],
];

/// The colour the logo is drawn in for an accent.
pub fn logo_tint(accent: Accent, custom: Option<Color32>) -> Color32 {
    custom
        .filter(|_| accent == Accent::Custom)
        .unwrap_or(rgb(accent_colors(accent).0))
}

/// The faces' colours: the logo's own blues for blue, shades of `hue`
/// otherwise.
fn logo_colors(hue: Color32) -> [Color32; 3] {
    if hue == rgb(accent_colors(Accent::Blue).0) {
        [rgb(0x60a5fa), rgb(0x2563eb), rgb(0x3b82f6)]
    } else {
        [
            over(hue, Color32::WHITE, 0.4),
            hue,
            over(hue, Color32::WHITE, 0.18),
        ]
    }
}

/// PkgDeck's cube in `rect`, its three faces in shades of the accent.
pub fn paint_logo(painter: &Painter, rect: Rect, palette: &Palette) {
    // A dark accent is lifted so the cube shows on a dark window.
    let hue = if palette.dark && palette.tint != rgb(accent_colors(Accent::Blue).0) {
        readable_accent(palette.tint, palette.canvas, true)
    } else {
        palette.tint
    };
    let at = |(x, y): (f32, f32)| rect.min + Vec2::new(x, y) * (rect.width() / 128.0);
    for (face, color) in LOGO.iter().zip(logo_colors(hue)) {
        painter.add(Shape::convex_polygon(
            face.iter().copied().map(at).collect(),
            color,
            Stroke::NONE,
        ));
    }
}

/// The cube as `size` by `size` RGBA pixels, for the Dock, tray and window
/// icons. Each pixel averages 4 by 4 samples, so the edges are smooth.
pub fn logo_rgba(hue: Color32, size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let colors = logo_colors(hue);
    let scale = 128.0 / size as f32;
    let inside = |face: &[(f32, f32); 4], x: f32, y: f32| {
        let sides = (0..4).map(|i| {
            let ((ax, ay), (bx, by)) = (face[i], face[(i + 1) % 4]);
            (bx - ax) * (y - ay) - (by - ay) * (x - ax)
        });
        let sides: Vec<f32> = sides.collect();
        sides.iter().all(|&side| side >= 0.0) || sides.iter().all(|&side| side <= 0.0)
    };
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut sum, mut hits) = ([0u32; 3], 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * scale;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * scale;
                    if let Some(face) = LOGO.iter().position(|face| inside(face, px, py)) {
                        let [r, g, b, _] = colors[face].to_array();
                        sum = [sum[0] + r as u32, sum[1] + g as u32, sum[2] + b as u32];
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                pixels.extend([0; 4]);
            } else {
                let alpha = hits * 255 / (SAMPLES * SAMPLES);
                pixels.extend(sum.map(|channel| (channel / hits) as u8));
                pixels.push(alpha as u8);
            }
        }
    }
    pixels
}

/// Draw the icon called `name` into `rect`, as the Qt app's DeckIcon does.
pub fn paint_icon(painter: &Painter, rect: Rect, name: &str, color: Color32) {
    if !skin().family().flat() && name != "heart" {
        icon_fills(painter, rect, name, color);
    }
    icon_lines(painter, rect, name, color);
}

/// The colour an icon takes where the older styles draw icons in colour,
/// as theirs were: boxes in cardboard, checks and arrows in green, bins
/// and gears in grey, and each source in its own colour.
pub fn icon_hue(name: &str) -> Option<Color32> {
    let hex: u32 = match name {
        "package" | "apt" | "discover" | "toolbox" | "distrobox" => 0xc89a55,
        "search" | "filter" | "info" | "help" | "clock" => 0x4f86c6,
        "installed" | "updates" | "up" | "refresh" | "macos-updates" | "fwupd" => 0x2e9b3a,
        "install" | "down" | "flatpak" | "appimage" | "macos-apps" | "mas" => 0x3f7fd0,
        "remove" | "settings" | "sources" | "dev-caches" | "duplicates" | "system-image" => {
            0x8a95a3
        }
        "activity" | "npm" | "gem" => 0xc8392f,
        "projects" | "bell" | "lock" | "warning" | "homebrew" | "homebrew-cask" => 0xe0a526,
        "snap" => 0x6fb38f,
        "docker" => 0x2496ed,
        "podman" | "mise" => 0x8a3fa0,
        "cargo" | "rustup" | "composer" => 0xb5541c,
        "pnpm" | "bun" => 0xe8822a,
        "pip" | "pipx" | "uv" => 0x3a6fb0,
        _ => return None,
    };
    Some(Color32::from_rgb(
        (hex >> 16) as u8,
        (hex >> 8) as u8,
        hex as u8,
    ))
}

/// Icon `name` in colour, as the older styles drew toolbars, trees and
/// lists: its closed shapes filled in its hue, shaded where the style
/// shaded, inside a darker outline; `ink`'s alpha fades it. Modern, and
/// icons without a hue, keep `ink`.
pub fn paint_colour_icon(painter: &Painter, rect: Rect, name: &str, ink: Color32) {
    let family = skin().family();
    let Some(hue) = icon_hue(name).filter(|_| !family.flat()) else {
        return paint_icon(painter, rect, name, ink);
    };
    let opacity = f32::from(ink.a()) / 255.0;
    let (top, bottom, outline) = icon_tones(family, hue);
    let fade = |color: Color32| color.gamma_multiply(opacity);
    let scale = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x * scale, y * scale);
    for polygon in icons::closed_shapes(name).iter() {
        let points: Vec<Pos2> = polygon.iter().map(|&(x, y)| at(x, y)).collect();
        shaded_polygon(painter, &points, fade(top), fade(bottom));
    }
    for &(x, y, radius) in icons::rings(name) {
        painter.circle_filled(at(x, y), radius * scale, fade(over(top, bottom, 0.5)));
    }
    // A filled shape takes a thin outline, so its colour reads; lines
    // alone, a check or an arrow, are drawn bold in the colour itself.
    if icons::closed_shapes(name).is_empty() && icons::rings(name).is_empty() {
        icon_lines_wide(
            painter,
            rect,
            name,
            fade(over(hue, Color32::BLACK, 0.12)),
            2.4,
        );
    } else {
        icon_lines_wide(painter, rect, name, fade(outline), 1.3);
    }
}

/// A coloured icon's fill, top and bottom, and its outline: flat with a
/// black edge on Windows 98, shaded from light on XP, 7 and
/// Aqua.
fn icon_tones(family: Family, hue: Color32) -> (Color32, Color32, Color32) {
    match family {
        Family::Retro => {
            let fill = over(hue, Color32::WHITE, 0.15);
            (fill, fill, over(hue, Color32::BLACK, 0.75))
        }
        _ => (
            over(hue, Color32::WHITE, 0.55),
            hue,
            over(hue, Color32::BLACK, 0.45),
        ),
    }
}

/// A convex polygon shaded from `top` at its highest point to `bottom` at
/// its lowest.
fn shaded_polygon(painter: &Painter, points: &[Pos2], top: Color32, bottom: Color32) {
    if points.len() < 3 {
        return;
    }
    let (high, low) = points.iter().fold((f32::MAX, f32::MIN), |(high, low), p| {
        (high.min(p.y), low.max(p.y))
    });
    let mut mesh = egui::Mesh::default();
    for point in points {
        let t = (point.y - high) / (low - high).max(0.001);
        mesh.colored_vertex(*point, over(top, bottom, t));
    }
    for i in 1..points.len() as u32 - 1 {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(Shape::mesh(mesh));
}

/// A source's mark in colour: its drawn icon, or its letters in white on
/// a box of its colour.
pub fn paint_colour_source_icon(painter: &Painter, rect: Rect, id: &str, ink: Color32) {
    let family = skin().family();
    if family.flat() || icons::drawn(id) {
        return if icons::drawn(id) {
            paint_colour_icon(painter, rect, id, ink)
        } else {
            paint_source_icon(painter, rect, id, ink)
        };
    }
    let hue = icon_hue(id).unwrap_or(ink);
    let (top, bottom, outline) = icon_tones(family, hue);
    let scale = rect.width() / 24.0;
    let tile = rect.shrink(2.5 * scale);
    super::skin::gradient(
        painter,
        tile,
        CornerRadius::same((4.0 * scale) as u8),
        top,
        bottom,
        false,
    );
    painter.rect_stroke(
        tile,
        CornerRadius::same((4.0 * scale) as u8),
        Stroke::new((1.2 * scale).max(1.0), outline),
        egui::StrokeKind::Inside,
    );
    let letters = icons::monogram(id);
    let letters = if rect.width() < 16.0 {
        letters.get(..1).unwrap_or("")
    } else {
        letters
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letters,
        bold(rect.height() * 0.42),
        ink_on_hue(bottom),
    );
}

/// White on a dark hue, black on a light one.
fn ink_on_hue(hue: Color32) -> Color32 {
    if luminance(hue) < 0.4 {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

/// Icon `name`'s strokes and arcs in `color`.
fn icon_lines(painter: &Painter, rect: Rect, name: &str, color: Color32) {
    icon_lines_wide(painter, rect, name, color, 1.7);
}

/// [`icon_lines`], `width` wide in the icon's 24-unit grid.
fn icon_lines_wide(painter: &Painter, rect: Rect, name: &str, color: Color32, width: f32) {
    let scale = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x * scale, y * scale);
    let stroke = Stroke::new((width * scale).max(1.0), color);
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

/// The tint inside an icon's closed shapes, so the older styles' icons
/// read in two tones as theirs did: a light fill in a dark outline.
/// A faded icon keeps its fill as faint as its outline.
fn icon_fill(color: Color32) -> Color32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let solid = Color32::from_rgb(r, g, b);
    let fill = if luminance(solid) < 0.5 {
        over(solid, Color32::WHITE, 0.62)
    } else {
        solid.gamma_multiply(0.3)
    };
    fill.gamma_multiply(f32::from(a) / 255.0)
}

/// Fills the closed outlines and rings of icon `name` in [`icon_fill`].
fn icon_fills(painter: &Painter, rect: Rect, name: &str, color: Color32) {
    let scale = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x * scale, y * scale);
    let fill = icon_fill(color);
    for polygon in icons::closed_shapes(name).iter() {
        let points: Vec<Pos2> = polygon.iter().map(|&(x, y)| at(x, y)).collect();
        painter.add(Shape::convex_polygon(points, fill, Stroke::NONE));
    }
    for &(x, y, radius) in icons::rings(name) {
        painter.circle_filled(at(x, y), radius * scale, fill);
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

    fn schemes() -> Vec<(Scheme, bool)> {
        let mut all = vec![];
        for style in Style::ALL {
            for accent in Accent::ALL {
                for dark in [false, true] {
                    all.push((
                        Scheme {
                            style,
                            accent,
                            ..Scheme::default()
                        },
                        dark,
                    ));
                }
            }
        }
        all
    }

    #[test]
    fn old_desktops_keep_their_colours_in_the_dark() {
        let of = |style: Style, accent: Accent, dark| {
            Palette::of(
                Scheme {
                    style,
                    accent,
                    ..Scheme::default()
                },
                dark,
            )
        };
        // Title bars and selections follow the accent in every style.
        for style in [Style::Retro, Style::Luna, Style::Glass, Style::Aqua] {
            for accent in [Accent::Blue, Accent::Teal] {
                assert_eq!(
                    of(style, accent, true).deep,
                    of(style, accent, false).deep,
                    "{style:?} {accent:?}"
                );
            }
        }
        assert_eq!(of(Style::Retro, Accent::Navy, true).deep, rgb(0x000080));
        assert_eq!(
            of(Style::Retro, Accent::Navy, true).deep,
            of(Style::Retro, Accent::Navy, false).deep,
        );
        // Graphite, a grey as dark as the window, lifts off it in the dark.
        for style in Style::ALL {
            let graphite = of(style, Accent::Graphite, true);
            assert!(
                contrast(graphite.deep, graphite.canvas) >= 1.25,
                "{style:?}"
            );
            assert!(contrast(Color32::WHITE, graphite.deep) >= 4.5, "{style:?}");
        }
        // Windows 98 turns its grey down rather than going black, so its
        // bevels still show: lighter than black, darker than its fields'
        // ink.
        let grey = of(Style::Retro, Accent::Navy, true);
        assert_ne!(grey.canvas, Color32::BLACK);
        assert!(luminance(grey.surface) < luminance(grey.canvas));
    }

    #[test]
    fn every_style_reads_in_light_and_dark_with_every_accent() {
        for (scheme, dark) in schemes() {
            let palette = Palette::of(scheme, dark);
            let at = format!("{:?} {:?} dark={dark}", scheme.style, scheme.accent);
            assert_eq!(palette.dark, dark);
            assert!(contrast(palette.ink, palette.canvas) >= 7.0, "{at}");
            assert!(contrast(palette.ink, palette.surface) >= 7.0, "{at}");
            assert!(contrast(palette.muted, palette.surface) >= 4.5, "{at}");
            assert!(contrast(palette.accent, palette.surface) >= 2.0, "{at}");
            // Dark canvases are darker than their ink, light ones lighter.
            assert_eq!(
                luminance(palette.canvas) < luminance(palette.ink),
                dark,
                "{at}"
            );
        }
        // Each desktop looks its own. Versions of one desktop may share
        // colours and differ in how they draw.
        for dark in [false, true] {
            let colours = |style| {
                let palette = Palette::of(
                    Scheme {
                        style,
                        ..Scheme::default()
                    },
                    dark,
                );
                (palette.canvas, palette.surface)
            };
            for a in Style::ALL {
                for b in Style::ALL {
                    if a != b && colours(a) == colours(b) {
                        assert_eq!(a.family(), b.family(), "{a:?} {b:?} dark={dark}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_untinted_window_bar_is_grey_and_readable_and_the_rest_keeps_the_accent() {
        for style in Style::ALL {
            for dark in [false, true] {
                let scheme = |tint_window_bar| Scheme {
                    style,
                    accent: Accent::Red,
                    tint_window_bar,
                    ..Scheme::default()
                };
                let tinted = Palette::of(scheme(true), dark);
                let grey = Palette::of(scheme(false), dark);
                let context = format!("{style:?} dark={dark}");
                assert_eq!(tinted.bar, tinted.deep, "{context}");
                assert_eq!(grey.bar.r(), grey.bar.g(), "{context}");
                assert_eq!(grey.bar.g(), grey.bar.b(), "{context}");
                assert!(contrast(Color32::WHITE, grey.bar) >= 4.5, "{context}");
                assert_eq!(grey.glass.a(), tinted.glass.a(), "{context}");
                assert_eq!(grey.glass.r(), grey.glass.g(), "{context}");
                assert_eq!(grey.glass.g(), grey.glass.b(), "{context}");
                // Only the bars and the glass lose the accent.
                assert_eq!(grey.accent, tinted.accent, "{context}");
                assert_eq!(grey.selection, tinted.selection, "{context}");
                assert_eq!(grey.canvas, tinted.canvas, "{context}");
            }
        }
    }

    #[test]
    fn modern_keeps_its_colours_whatever_the_new_choices() {
        let tinted = Scheme {
            tint_backgrounds: true,
            ..Scheme::default()
        };
        let modern = Palette::of(tinted, true);
        // Modern's greys, tinted with its blue.
        let blue = rgb(accent_colors(Accent::Blue).0);
        assert_eq!(modern.canvas, over(rgb(0x121212), blue, 0.05));
        assert_eq!(modern.accent, rgb(0x7aa7ff));
        let light = Palette::of(tinted, false);
        assert_eq!(light.canvas, over(rgb(0xe8edf5), blue, 0.04));
        assert_eq!(light.accent_ink, Color32::WHITE);
        let tuned = Scheme {
            roundness: 2.0,
            density: Density::Roomy,
            typeface: Typeface::System,
            custom_accent: Some(Color32::RED),
            ..tinted
        };
        // A picked colour counts only once Custom is the accent.
        assert_eq!(Palette::of(tuned, true), modern);
        // Older styles bring their own colours whatever Modern's theme.
        let retro = |dark: DarkTheme| {
            Palette::of(
                Scheme {
                    style: Style::Retro,
                    dark,
                    ..Scheme::default()
                },
                true,
            )
        };
        assert_eq!(retro(DarkTheme::Slate), retro(DarkTheme::Black));
    }

    #[test]
    fn the_accent_tints_the_window_in_every_style_and_accent() {
        for style in Style::ALL {
            for dark in [false, true] {
                let at = format!("{style:?} dark={dark}");
                let wearing = |accent| {
                    Palette::of(
                        Scheme {
                            style,
                            accent,
                            tint_backgrounds: true,
                            ..Scheme::default()
                        },
                        dark,
                    )
                };
                let blue = wearing(Accent::Blue);
                let green = wearing(Accent::Green);
                // Blue gets the same tint as any other accent.
                assert!(blue.tint_amount > 0.0, "{at}");
                assert_eq!(blue.tint_amount, green.tint_amount, "{at}");
                assert!(green.tint_amount > 0.0, "{at}");
                assert_ne!(green.canvas, blue.canvas, "{at}");
                assert_ne!(green.surface, blue.surface, "{at}");
                assert_eq!(
                    green.tone(Color32::WHITE),
                    over(Color32::WHITE, green.tint, green.tint_amount)
                );
            }
        }
        // Windows 11 in its own blue is Mica's plain #f3f3f3.
        let mica = Palette::of(
            Scheme {
                style: Style::Fluent,
                ..Scheme::default()
            },
            false,
        );
        assert_eq!(mica.canvas, rgb(0xf3f3f3));
        // OLED black stays black whatever the accent.
        let black = Palette::of(
            Scheme {
                dark: DarkTheme::Black,
                accent: Accent::Purple,
                ..Scheme::default()
            },
            true,
        );
        assert_eq!(black.canvas, Color32::BLACK);
        // Windows 7's dark blue gives way to near black.
        let glass = |accent| {
            Palette::of(
                Scheme {
                    style: Style::Glass,
                    accent,
                    ..Scheme::default()
                },
                true,
            )
            .canvas
        };
        let green = glass(Accent::Green);
        assert!(luminance(green) < 0.01, "{green:?}");
        // Its own blue too: near black, not Media Center's navy.
        assert_eq!(glass(Accent::Blue), rgb(0x0e0f11));
    }

    #[test]
    fn windows_7_light_tints_its_white_canvas_visibly() {
        let canvas = |tint_backgrounds| {
            Palette::of(
                Scheme {
                    style: Style::Glass,
                    accent: Accent::Blue,
                    tint_backgrounds,
                    ..Scheme::default()
                },
                false,
            )
            .canvas
        };
        let plain = canvas(false);
        assert_eq!(plain, rgb(0xffffff));
        // Tinted, the white reads as blue, not as white.
        let tinted = canvas(true);
        assert!(255 - tinted.r() >= 10, "{tinted:?}");
        assert!(tinted.b() > tinted.r(), "{tinted:?}");
    }

    #[test]
    fn turning_the_tint_off_keeps_the_greys_neutral_and_the_accent_on_controls() {
        for style in Style::ALL {
            for dark in [false, true] {
                let at = format!("{style:?} dark={dark}");
                let plain = |accent| {
                    Palette::of(
                        Scheme {
                            style,
                            accent,
                            tint_backgrounds: false,
                            ..Scheme::default()
                        },
                        dark,
                    )
                };
                let blue = plain(Accent::Blue);
                let green = plain(Accent::Green);
                assert_eq!(green.tint_amount, 0.0, "{at}");
                // The greys ignore the accent.
                assert_eq!(green.canvas, blue.canvas, "{at}");
                assert_eq!(green.surface, blue.surface, "{at}");
                assert_eq!(green.tone(Color32::WHITE), Color32::WHITE, "{at}");
                // The accent still colours selection, links and controls.
                assert_ne!(green.accent, blue.accent, "{at}");
                assert_ne!(green.selection, blue.selection, "{at}");
                // And the tint is what changes the greys.
                let tinted = Palette::of(
                    Scheme {
                        style,
                        accent: Accent::Green,
                        ..Scheme::default()
                    },
                    dark,
                );
                if tinted.tint_amount > 0.0 {
                    assert_ne!(tinted.canvas, green.canvas, "{at}");
                }
            }
        }
        // Modern in light is Classic's blue-grey, untouched by any accent.
        for accent in [Accent::Blue, Accent::Red, Accent::Custom] {
            let light = Palette::of(
                Scheme {
                    accent,
                    custom_accent: Some(Color32::RED),
                    tint_backgrounds: false,
                    ..Scheme::default()
                },
                false,
            );
            assert_eq!(light.canvas, rgb(0xe8edf5), "{accent:?}");
            assert_eq!(light.surface, rgb(0xf7f9fd), "{accent:?}");
        }
    }

    #[test]
    fn the_drawn_logo_matches_the_shipped_one_and_follows_the_accent() {
        // In blue, the drawn cube is the logo PkgDeck ships.
        let shipped = image::load_from_memory(include_bytes!("../../assets/logo-64.png"))
            .unwrap()
            .to_rgba8()
            .into_raw();
        let blue = logo_tint(Accent::Blue, None);
        let drawn = logo_rgba(blue, 64);
        assert_eq!(drawn.len(), shipped.len());
        let off = drawn
            .iter()
            .zip(&shipped)
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum::<u64>()
            / drawn.len() as u64;
        assert!(off <= 2, "{off}");
        // Corners stay clear; each face takes a shade of the accent.
        let green = logo_tint(Accent::Green, None);
        let pixels = logo_rgba(green, 128);
        let at = |x: usize, y: usize| {
            let i = (y * 128 + x) * 4;
            Color32::from_rgba_unmultiplied(pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3])
        };
        assert_eq!(at(0, 0).a(), 0);
        assert_eq!(at(127, 127).a(), 0);
        assert_eq!(at(64, 36), logo_colors(green)[0]);
        assert_eq!(at(40, 78), green);
        assert_eq!(at(88, 78), logo_colors(green)[2]);
        // A picked colour counts only once Custom is the accent.
        let red = Some(Color32::RED);
        assert_eq!(logo_tint(Accent::Custom, red), Color32::RED);
        assert_eq!(logo_tint(Accent::Green, red), green);
        assert_eq!(
            Palette::of(
                Scheme {
                    accent: Accent::Green,
                    ..Scheme::default()
                },
                false
            )
            .tint,
            green
        );
    }

    #[test]
    fn a_picked_accent_is_made_to_read_with_ink_that_reads_on_it() {
        let picks = [
            0xffff00, 0xffffff, 0x000000, 0x777777, 0x00ff00, 0x0000ff, 0xff8800, 0x3a6ea5,
        ];
        for style in Style::ALL {
            for dark in [false, true] {
                for pick in picks {
                    let scheme = Scheme {
                        style,
                        accent: Accent::Custom,
                        custom_accent: Some(rgb(pick)),
                        ..Scheme::default()
                    };
                    let palette = Palette::of(scheme, dark);
                    let at = format!("{style:?} dark={dark} {pick:06x}");
                    assert!(contrast(palette.accent, palette.canvas) >= 3.0, "{at}");
                    assert!(contrast(palette.accent_ink, palette.accent) >= 3.0, "{at}");
                }
            }
        }
        // A colour that already reads is kept as it is.
        let blue = rgb(0x2050c0);
        assert_eq!(readable_accent(blue, Color32::WHITE, false), blue);
        // Yellow on white darkens; black on black lightens.
        let yellow = readable_accent(rgb(0xffff00), Color32::WHITE, false);
        assert!(luminance(yellow) < luminance(rgb(0xffff00)));
        let black = readable_accent(Color32::BLACK, Color32::BLACK, true);
        assert!(luminance(black) > 0.0);
        // Custom without a colour is the blue it starts from.
        let unset = Palette::of(
            Scheme {
                accent: Accent::Custom,
                ..Scheme::default()
            },
            false,
        );
        assert_eq!(unset.accent, rgb(accent_colors(Accent::Custom).0));
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
        assert_eq!(contrast(blue, blue), 1.0);
    }

    #[test]
    fn colours_go_to_hex_and_back() {
        for color in [rgb(0x000000), rgb(0xffffff), rgb(0x3a6ea5), rgb(0x0a0b0c)] {
            assert_eq!(parse_hex(&hex(color)), Some(color));
        }
        assert_eq!(hex(rgb(0x0a0b0c)), "#0a0b0c");
        assert_eq!(parse_hex(" #FF8800 "), Some(rgb(0xff8800)));
        for bad in [
            "", "#", "ff8800", "#ff880", "#ff88000", "#gg8800", "#ff 800", "#ÿÿÿ",
        ] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn each_style_rounds_corners_its_own_way_and_the_person_scales_it() {
        let round = |style, roundness: f32, radius| {
            Skin {
                style,
                roundness,
                density: Density::Normal,
            }
            .round(radius)
        };
        assert_eq!(round(Style::Modern, 1.0, 12), CornerRadius::same(12));
        assert_eq!(round(Style::Aqua, 1.0, 12), CornerRadius::same(4));
        assert_eq!(round(Style::Retro, 2.0, 12), CornerRadius::ZERO);
        assert_eq!(round(Style::Luna, 1.0, 12), CornerRadius::same(5));
        assert_eq!(round(Style::Glass, 1.0, 10), CornerRadius::same(4));
        assert_eq!(round(Style::Modern, 0.0, 12), CornerRadius::ZERO);
        assert_eq!(round(Style::Modern, 2.0, 12), CornerRadius::same(24));
        assert_eq!(round(Style::Modern, 9.0, 12), CornerRadius::same(24));
        assert_eq!(round(Style::Modern, -1.0, 12), CornerRadius::ZERO);
        assert_eq!(round(Style::Modern, 2.0, 200), CornerRadius::same(255));
        assert_eq!(Skin::default().style, Style::Modern);
        // A preview borrows a style and gives it back.
        let luna = Skin {
            style: Style::Luna,
            ..Skin::default()
        };
        let before = skin();
        assert_eq!(with_skin(luna, skin), luna);
        assert_eq!(with_skin(luna, || super::round(12)), CornerRadius::same(5));
        assert_eq!(skin(), before);
    }

    #[test]
    fn styles_choose_their_faces_unless_the_person_does() {
        assert_eq!(face(Style::Modern, Typeface::Style), Face::Inter);
        assert_eq!(face(Style::Glass, Typeface::Style), Face::Style(SEGOE));
        assert_eq!(face(Style::Retro, Typeface::Style), Face::Style(MS_SANS));
        assert_eq!(face(Style::Luna, Typeface::Style), Face::Style(TAHOMA));
        assert_eq!(face(Style::Aqua, Typeface::Style), Face::Style(LUCIDA));
        for style in Style::ALL {
            assert_eq!(face(style, Typeface::Inter), Face::Inter);
            assert_eq!(face(style, Typeface::System), Face::System);
        }
    }

    #[test]
    fn carried_faces_load_with_nothing_installed() {
        let root = std::env::temp_dir().join(format!("pkgdeck-carried-{}", std::process::id()));
        for list in [MS_SANS, SEGOE] {
            let fonts = fonts_with(&root, Face::Style(list));
            assert_eq!(fonts.families[&FontFamily::Proportional][0], "style");
            assert_eq!(fonts.families[&bold_family()][0], "style-bold");
        }
        // Each draws its own letters: the same words come out another width
        // than in Inter.
        let width = |fonts: FontDefinitions, family: FontFamily| {
            let ctx = egui::Context::default();
            ctx.set_fonts(fonts);
            let mut width = 0.0;
            for _ in 0..2 {
                let mut output = ctx.run_ui(Default::default(), |ui| {
                    width = ui
                        .painter()
                        .layout_no_wrap(
                            "Installed packages".into(),
                            FontId::new(14.0, family.clone()),
                            Color32::BLACK,
                        )
                        .size()
                        .x;
                });
                output.textures_delta.clear();
            }
            width
        };
        for family in [FontFamily::Proportional, bold_family()] {
            let inter = width(fonts_with(&root, Face::Inter), family.clone());
            for list in [MS_SANS, SEGOE] {
                let own = width(fonts_with(&root, Face::Style(list)), family.clone());
                assert!((own - inter).abs() > 1.0, "{family:?} {own} {inter}");
            }
        }
        // Every carried name in every list is one PkgDeck has.
        for list in [MS_SANS, SEGOE, TAHOMA, LUCIDA] {
            for path in list.iter().flat_map(|(a, b)| [*a, *b]) {
                assert!(
                    !path.starts_with("bundled:") || bundled(path).is_some(),
                    "{path}"
                );
            }
        }
        assert_eq!(bundled("bundled:Nope.ttf"), None);
        assert_eq!(bundled("W95FA.otf"), None);
    }

    #[test]
    fn a_style_face_goes_first_when_installed_and_inter_otherwise() {
        let root = std::env::temp_dir().join(format!("pkgdeck-faces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let regular = |fonts: &FontDefinitions| fonts.families[&FontFamily::Proportional].clone();
        // Nothing installed: Inter, whatever was asked.
        for face in [Face::Inter, Face::System, Face::Style(TAHOMA)] {
            assert_eq!(regular(&fonts_with(&root, face))[0], "inter");
        }
        let (tahoma, tahoma_bold) = TAHOMA[0];
        for path in [tahoma, tahoma_bold, SANS[0].0, SANS[0].1] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"face").unwrap();
        }
        let fonts = fonts_with(&root, Face::Style(TAHOMA));
        assert_eq!(regular(&fonts)[..2], ["style", "inter"]);
        assert_eq!(fonts.families[&bold_family()][0], "style-bold");
        assert_eq!(fonts.font_data["style-bold"].index, 0);
        // Titles stay bold until XP's own face is around, and only XP takes it.
        let title = |fonts: &FontDefinitions| fonts.families[&title_family()][0].clone();
        assert_eq!(title(&fonts), "style-bold");
        let path = root.join(TREBUCHET_BOLD[0]);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"title").unwrap();
        assert_eq!(title(&fonts_with(&root, Face::Style(TAHOMA))), "title");
        assert_eq!(title(&fonts_with(&root, Face::Inter)), "inter-semibold");
        assert_eq!(regular(&fonts_with(&root, Face::System))[0], "system");
        let inter = fonts_with(&root, Face::Inter);
        assert_eq!(regular(&inter)[0], "inter");
        assert!(regular(&inter).contains(&"system".to_owned()));
        // A collection keeps its bold face second.
        let (lucida, _) = LUCIDA[0];
        let path = root.join(lucida);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"collection").unwrap();
        let fonts = fonts_with(&root, Face::Style(LUCIDA));
        assert_eq!(fonts.font_data["style-bold"].index, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_style_loads_its_face_and_reshapes_at_once() {
        let ctx = egui::Context::default();
        let root = std::env::temp_dir().join(format!("pkgdeck-scheme-{}", std::process::id()));
        set_font_root(&ctx, &root);
        let retro = Scheme {
            style: Style::Retro,
            ..Scheme::default()
        };
        set_scheme(&ctx, retro);
        assert_eq!(skin().style, Style::Retro);
        assert_eq!(scheme(&ctx), retro);
        assert_eq!(super::round(12), CornerRadius::ZERO);
        set_scheme(&ctx, Scheme::default());
        assert_eq!(skin().style, Style::Modern);
        assert_eq!(super::round(12), CornerRadius::same(12));
    }

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
    fn old_styles_fill_their_icons_in_a_second_tone() {
        let fills = |style: Style, color: Color32| {
            let skin = Skin {
                style,
                ..Skin::default()
            };
            with_skin(skin, || {
                paint(|painter| paint_icon(painter, square(0.0, 24.0), "package", color))
            })
            .iter()
            .filter(|shape| matches!(shape, Shape::Path(path) if path.fill != Color32::TRANSPARENT))
            .count()
        };
        assert_eq!(fills(Style::Modern, Color32::BLACK), 0);
        assert_eq!(fills(Style::Retro, Color32::BLACK), 1);
        // Light on dark, then dark on light.
        assert!(luminance(icon_fill(Color32::BLACK)) > 0.25);
        assert!(icon_fill(Color32::WHITE).a() < 255);
        // A faded icon's fill fades with it.
        let faded = icon_fill(Color32::from_black_alpha(128));
        assert!((120..=136).contains(&faded.a()), "{faded:?}");
        assert_eq!(icon_fill(Color32::TRANSPARENT).a(), 0);
        let rings = with_skin(
            Skin {
                style: Style::Aqua,
                ..Skin::default()
            },
            || paint(|painter| paint_icon(painter, square(0.0, 24.0), "search", Color32::BLACK)),
        );
        assert!(rings.iter().any(
            |shape| matches!(shape, Shape::Circle(circle) if circle.fill != Color32::TRANSPARENT)
        ));
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
