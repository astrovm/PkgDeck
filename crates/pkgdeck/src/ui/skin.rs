//! How each style draws the pieces everything is built from: buttons,
//! fields, check boxes, panels, the canvas, the sidebar, the selection and
//! title strips. Modern draws them flat; the others add bevels, gloss,
//! gradients and pinstripes. Only egui's own shapes, so a frame costs the
//! same in any style.

use super::theme::{self, Palette};
use crate::settings::Style;
use eframe::egui::{
    epaint::{tessellator::path, Mesh, Shadow},
    Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2,
};

/// What a button is for, which decides how much it stands out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Weight {
    /// Filled with a colour: the page's main action.
    Strong,
    /// Outlined or softly tinted.
    Normal,
    /// No frame until hovered.
    Flat,
}

/// How far a button is hovered and pressed, from 0 to 1, and how opaque it
/// draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub hover: f32,
    pub press: f32,
    pub opacity: f32,
}

fn style() -> Style {
    theme::skin().style
}

/// `a` blended toward `b`, alpha included.
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}
fn lighter(color: Color32, amount: f32) -> Color32 {
    lerp(color, Color32::WHITE, amount)
}
fn darker(color: Color32, amount: f32) -> Color32 {
    lerp(color, Color32::BLACK, amount)
}
fn faded(color: Color32, opacity: f32) -> Color32 {
    color.gamma_multiply(opacity.clamp(0.0, 1.0))
}

/// A gradient from `start` to `end` across `rect`, rounded by `radius`:
/// top to bottom, or left to right when `across`.
pub fn gradient(
    painter: &Painter,
    rect: Rect,
    radius: CornerRadius,
    start: Color32,
    end: Color32,
    across: bool,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let mut outline = Vec::new();
    path::rounded_rectangle(&mut outline, rect, radius.into());
    let at = |point: Pos2| {
        let t = if across {
            (point.x - rect.left()) / rect.width()
        } else {
            (point.y - rect.top()) / rect.height()
        };
        lerp(start, end, t)
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), at(rect.center()));
    for point in &outline {
        mesh.colored_vertex(*point, at(*point));
    }
    let count = outline.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(mesh);
}

/// A light edge along the top and left of `rect` and a dark one along the
/// bottom and right: raised; swapped, sunken.
pub fn bevel(painter: &Painter, rect: Rect, light: Color32, dark: Color32) {
    let rect = rect.shrink(0.5);
    let stroke = |color| Stroke::new(1.0, color);
    painter.line_segment([rect.left_bottom(), rect.left_top()], stroke(light));
    painter.line_segment([rect.left_top(), rect.right_top()], stroke(light));
    painter.line_segment([rect.right_top(), rect.right_bottom()], stroke(dark));
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke(dark));
}

/// The two-step edge of the 1990s desktops around `rect`.
fn retro_edge(painter: &Painter, rect: Rect, face: Color32, sunken: bool) {
    let (outer_light, outer_dark) = (lighter(face, 0.9), darker(face, 0.85));
    let (inner_light, inner_dark) = (lighter(face, 0.45), darker(face, 0.33));
    if sunken {
        bevel(painter, rect, inner_dark, outer_light);
        bevel(painter, rect.shrink(1.0), outer_dark, inner_light);
    } else {
        bevel(painter, rect, outer_light, outer_dark);
        bevel(painter, rect.shrink(1.0), inner_light, inner_dark);
    }
}

/// A shine over the top half of `rect`, as on glossy buttons.
fn gloss(painter: &Painter, rect: Rect, radius: CornerRadius, strength: f32) {
    let top = Rect::from_min_max(
        rect.min + Vec2::splat(1.0),
        Pos2::new(rect.right() - 1.0, rect.center().y),
    );
    let radius = CornerRadius {
        sw: 0,
        se: 0,
        ..radius
    };
    gradient(
        painter,
        top,
        radius,
        Color32::from_white_alpha((200.0 * strength) as u8),
        Color32::from_white_alpha((40.0 * strength) as u8),
        false,
    );
}

/// The corners a control of `rect` gets: Gel's are pills.
fn control_radius(rect: Rect, radius: CornerRadius) -> CornerRadius {
    if style() == Style::Gel {
        let skin = theme::skin();
        let pill = rect.height() / 2.0 * skin.roundness.clamp(0.0, 1.0);
        CornerRadius::same(pill.round().min(255.0) as u8)
    } else {
        radius
    }
}

/// A button's frame. `fill` and `stroke` are Modern's; other styles take
/// the colour and draw it their way.
pub fn paint_button(
    painter: &Painter,
    rect: Rect,
    weight: Weight,
    fill: Color32,
    stroke: Stroke,
    state: State,
    palette: &Palette,
) {
    let radius = control_radius(rect, theme::round(9));
    let State {
        hover,
        press,
        opacity,
    } = state;
    let modern = |painter: &Painter| {
        painter.rect(
            rect,
            radius,
            faded(fill, opacity),
            Stroke::new(stroke.width, faded(stroke.color, opacity)),
            StrokeKind::Inside,
        );
    };
    // Flat buttons stay flat until hovered in every style.
    if weight == Weight::Flat && hover + press <= 0.01 {
        return;
    }
    let face = match weight {
        Weight::Strong => fill,
        _ => palette.surface,
    };
    match style() {
        Style::Modern => modern(painter),
        Style::Retro | Style::Platinum => {
            let face = match weight {
                Weight::Strong => fill,
                // The grey of the old desktops' buttons.
                _ => lerp(
                    palette.canvas,
                    palette.surface,
                    if palette.dark { 0.6 } else { 0.0 },
                ),
            };
            let face = darker(face, press * 0.08);
            let shown = if weight == Weight::Flat {
                hover.max(press)
            } else {
                1.0
            };
            painter.rect_filled(rect, radius, faded(face, opacity * shown));
            if style() == Style::Retro {
                if weight == Weight::Strong {
                    painter.rect_stroke(
                        rect,
                        radius,
                        Stroke::new(1.0, faded(Color32::BLACK, opacity)),
                        StrokeKind::Inside,
                    );
                    retro_edge(painter, rect.shrink(1.0), face, press > 0.5);
                } else {
                    retro_edge(painter, rect, face, press > 0.5);
                }
            } else {
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1.0, faded(lerp(palette.ink, face, 0.35), opacity * shown)),
                    StrokeKind::Inside,
                );
                let (light, dark) = (lighter(face, 0.7), darker(face, 0.25));
                let inner = rect.shrink(1.0);
                if press > 0.5 {
                    bevel(painter, inner, dark, light);
                } else {
                    bevel(painter, inner, faded(light, shown), faded(dark, shown));
                }
            }
        }
        Style::Luna => {
            let (top, bottom, border) = match weight {
                Weight::Strong => (lighter(fill, 0.38), fill, darker(fill, 0.35)),
                _ => (
                    lighter(face, 0.6),
                    lerp(face, palette.canvas, 0.9),
                    lerp(palette.accent, palette.ink, 0.45),
                ),
            };
            let (top, bottom) = if press > 0.5 {
                (darker(bottom, 0.06), top)
            } else {
                (top, bottom)
            };
            let shown = if weight == Weight::Flat {
                hover.max(press)
            } else {
                1.0
            };
            gradient(
                painter,
                rect,
                radius,
                faded(top, opacity * shown),
                faded(bottom, opacity * shown),
                false,
            );
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(border, opacity * shown)),
                StrokeKind::Inside,
            );
            if hover > 0.0 && weight != Weight::Strong {
                // The warm glow of a hovered button.
                painter.rect_stroke(
                    rect.shrink(1.0),
                    radius,
                    Stroke::new(
                        1.5,
                        faded(Color32::from_rgb(0xf8, 0xb3, 0x30), hover * opacity),
                    ),
                    StrokeKind::Inside,
                );
            }
        }
        Style::Glass => {
            let base = match weight {
                Weight::Strong => fill,
                _ if palette.dark => lighter(palette.surface, 0.1),
                _ => Color32::from_rgb(0xe5, 0xe5, 0xe5),
            };
            let hovered = |color: Color32| lerp(color, lerp(color, palette.accent, 0.35), hover);
            let base = hovered(darker(base, press * 0.12));
            let shown = if weight == Weight::Flat {
                hover.max(press)
            } else {
                1.0
            };
            let alpha = opacity * shown;
            gradient(
                painter,
                rect,
                radius,
                faded(darker(base, 0.04), alpha),
                faded(darker(base, 0.14), alpha),
                false,
            );
            let top = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.center().y));
            gradient(
                painter,
                top,
                CornerRadius {
                    sw: 0,
                    se: 0,
                    ..radius
                },
                faded(lighter(base, 0.55), alpha),
                faded(lighter(base, 0.3), alpha),
                false,
            );
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(darker(base, 0.45), alpha)),
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                rect.shrink(1.0),
                radius,
                Stroke::new(1.0, faded(Color32::from_white_alpha(110), alpha)),
                StrokeKind::Inside,
            );
        }
        Style::Gel => {
            let base = match weight {
                Weight::Strong => fill,
                _ if palette.dark => lighter(palette.surface, 0.12),
                _ => Color32::from_rgb(0xe8, 0xe8, 0xe8),
            };
            let base = darker(lerp(base, palette.accent, hover * 0.15), press * 0.15);
            let shown = if weight == Weight::Flat {
                hover.max(press)
            } else {
                1.0
            };
            let alpha = opacity * shown;
            // Gel is darker at the top and glows at the bottom.
            gradient(
                painter,
                rect,
                radius,
                faded(darker(base, 0.12), alpha),
                faded(lighter(base, 0.45), alpha),
                false,
            );
            gloss(painter, rect, radius, alpha);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(darker(base, 0.4), alpha)),
                StrokeKind::Inside,
            );
        }
    }
}

/// A text field's frame, and its ring while `focus` (0 to 1) is on it.
pub fn paint_field(
    painter: &Painter,
    rect: Rect,
    radius: CornerRadius,
    palette: &Palette,
    focus: f32,
) {
    let ring = lerp(palette.line, palette.accent, focus);
    match style() {
        Style::Modern | Style::Gel => {
            painter.rect(
                rect,
                radius,
                palette.surface,
                Stroke::new(1.0 + focus, ring),
                StrokeKind::Inside,
            );
            if style() == Style::Gel {
                // Fields sit a little into the window.
                let shade = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 4.0));
                gradient(
                    painter,
                    shade.shrink2(Vec2::new(1.0, 0.0)),
                    CornerRadius {
                        sw: 0,
                        se: 0,
                        ..radius
                    },
                    Color32::from_black_alpha(28),
                    Color32::TRANSPARENT,
                    false,
                );
            }
            if focus > 0.0 {
                painter.rect_stroke(
                    rect.expand(3.0),
                    theme::round(12),
                    Stroke::new(3.0, palette.accent.gamma_multiply(0.18 * focus)),
                    StrokeKind::Inside,
                );
            }
        }
        Style::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
            let face = lerp(
                palette.canvas,
                palette.surface,
                if palette.dark { 0.6 } else { 0.0 },
            );
            retro_edge(painter, rect, face, true);
        }
        Style::Platinum => {
            painter.rect_filled(rect, radius, palette.surface);
            bevel(
                painter,
                rect,
                darker(palette.canvas, 0.35),
                lighter(palette.canvas, 0.8),
            );
            if focus > 0.0 {
                painter.rect_stroke(
                    rect.expand(2.0),
                    radius,
                    Stroke::new(2.0, palette.accent.gamma_multiply(focus)),
                    StrokeKind::Inside,
                );
            }
        }
        Style::Luna | Style::Glass => {
            let border = if style() == Style::Luna {
                lerp(palette.accent, palette.surface, 0.45)
            } else {
                lerp(palette.ink, palette.surface, 0.62)
            };
            painter.rect(
                rect,
                radius,
                palette.surface,
                Stroke::new(1.0, lerp(border, palette.accent, focus)),
                StrokeKind::Inside,
            );
        }
    }
}

/// A check box's box, filled by `checked` (0 to 1). Returns the colour
/// the tick draws in.
pub fn paint_check(
    painter: &Painter,
    rect: Rect,
    palette: &Palette,
    checked: f32,
    hovered: bool,
    opacity: f32,
) -> Color32 {
    match style() {
        Style::Retro | Style::Platinum => {
            paint_field(painter, rect, theme::round(5), palette, 0.0);
            if hovered && style() == Style::Platinum {
                painter.rect_stroke(
                    rect,
                    theme::round(5),
                    Stroke::new(1.0, faded(palette.accent, opacity)),
                    StrokeKind::Inside,
                );
            }
            faded(palette.ink, opacity)
        }
        _ => {
            let border = if hovered {
                palette.accent
            } else {
                palette.strong_line
            };
            let radius = theme::round(5);
            if matches!(style(), Style::Luna | Style::Glass | Style::Gel) && checked < 0.99 {
                gradient(
                    painter,
                    rect,
                    radius,
                    faded(lerp(palette.canvas, palette.surface, 0.3), opacity),
                    faded(palette.surface, opacity),
                    false,
                );
            }
            painter.rect(
                rect,
                radius,
                faded(palette.accent, checked.clamp(0.0, 1.0) * opacity),
                Stroke::new(
                    1.5,
                    faded(
                        lerp(border, palette.accent, checked.clamp(0.0, 1.0)),
                        opacity,
                    ),
                ),
                StrokeKind::Inside,
            );
            if style() == Style::Gel && checked > 0.0 {
                gloss(painter, rect, radius, checked.clamp(0.0, 1.0) * opacity);
            }
            faded(palette.accent_ink, opacity)
        }
    }
}

/// A switch's knob, drawn square in the styles that had no round ones.
pub fn paint_knob(painter: &Painter, center: Pos2, radius: f32, palette: &Palette, opacity: f32) {
    match style() {
        Style::Retro | Style::Platinum => {
            let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
            let face = lerp(
                palette.canvas,
                Color32::WHITE,
                if palette.dark { 0.2 } else { 0.0 },
            );
            painter.rect_filled(rect, theme::round(4), faded(face, opacity));
            retro_edge(painter, rect, face, false);
        }
        _ => {
            painter.circle_filled(
                center + Vec2::new(0.0, 0.6),
                radius,
                Color32::from_black_alpha((46.0 * opacity) as u8),
            );
            painter.circle_filled(center, radius, faded(Color32::WHITE, opacity));
            if matches!(style(), Style::Gel | Style::Glass | Style::Luna) {
                let top = Rect::from_center_size(
                    center - Vec2::new(0.0, radius * 0.45),
                    Vec2::new(radius * 1.3, radius * 0.8),
                );
                gradient(
                    painter,
                    top,
                    CornerRadius::same((radius * 0.4) as u8),
                    Color32::from_white_alpha((230.0 * opacity) as u8),
                    Color32::from_black_alpha((10.0 * opacity) as u8),
                    false,
                );
                painter.circle_stroke(
                    center,
                    radius,
                    Stroke::new(0.8, Color32::from_black_alpha((60.0 * opacity) as u8)),
                );
            }
        }
    }
}

/// The shadow a panel casts, if the style has one.
pub fn shadow(palette: &Palette) -> Shadow {
    let (offset, blur, alpha) = match style() {
        Style::Modern => (1, 6, if palette.dark { 40 } else { 10 }),
        Style::Glass => (2, 12, if palette.dark { 70 } else { 30 }),
        Style::Gel => (3, 14, if palette.dark { 90 } else { 45 }),
        Style::Luna => (1, 3, if palette.dark { 40 } else { 14 }),
        Style::Retro | Style::Platinum => (0, 0, 0),
    };
    Shadow {
        offset: [0, offset],
        blur,
        spread: 0,
        color: Color32::from_black_alpha(alpha),
    }
}

/// The line around a panel.
pub fn panel_stroke(palette: &Palette) -> Stroke {
    match style() {
        Style::Luna => Stroke::new(1.0, lerp(palette.accent, palette.surface, 0.55)),
        Style::Platinum => Stroke::new(1.0, lerp(palette.ink, palette.canvas, 0.45)),
        Style::Retro => Stroke::new(1.0, darker(palette.canvas, 0.6)),
        _ => Stroke::new(1.0, palette.line),
    }
}

/// A panel: the list, a card. `sunken` ones hold things, like a list box.
pub fn paint_panel(painter: &Painter, rect: Rect, palette: &Palette, sunken: bool, stroke: Stroke) {
    let radius = theme::round(12);
    let shadow = shadow(palette);
    if shadow.color.a() > 0 {
        painter.add(shadow.as_shape(rect, radius));
    }
    painter.rect(rect, radius, palette.surface, stroke, StrokeKind::Inside);
    edge(painter, rect, palette, sunken);
}

/// What a style adds around a panel already drawn: bevels and highlights.
pub fn edge(painter: &Painter, rect: Rect, palette: &Palette, sunken: bool) {
    match style() {
        Style::Retro => {
            let face = lerp(
                palette.canvas,
                palette.surface,
                if palette.dark { 0.6 } else { 0.0 },
            );
            retro_edge(painter, rect, face, sunken);
        }
        Style::Platinum => {
            let (light, dark) = (lighter(palette.surface, 0.8), darker(palette.canvas, 0.3));
            if sunken {
                bevel(painter, rect.shrink(1.0), dark, light);
            } else {
                bevel(painter, rect.shrink(1.0), light, dark);
            }
        }
        Style::Glass => {
            // Glass catches the light along its top edge.
            let radius = theme::round(12);
            let top = Rect::from_min_max(
                rect.min + Vec2::splat(1.0),
                Pos2::new(rect.right() - 1.0, rect.top() + 14.0),
            );
            gradient(
                painter,
                top,
                CornerRadius {
                    sw: 0,
                    se: 0,
                    ..radius
                },
                Color32::from_white_alpha(if palette.dark { 18 } else { 140 }),
                Color32::TRANSPARENT,
                false,
            );
        }
        Style::Modern | Style::Luna | Style::Gel => {}
    }
}

/// Behind everything: the canvas colour, with Gel's pinstripes or Glass's
/// light from above.
pub fn paint_canvas(painter: &Painter, rect: Rect, palette: &Palette) {
    painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
    match style() {
        Style::Gel => {
            let stripe = if palette.dark {
                Color32::from_white_alpha(7)
            } else {
                Color32::from_white_alpha(120)
            };
            let mut mesh = Mesh::default();
            let mut y = rect.top();
            while y < rect.bottom() {
                mesh.add_colored_rect(
                    Rect::from_min_max(Pos2::new(rect.left(), y), Pos2::new(rect.right(), y + 2.0)),
                    stripe,
                );
                y += 4.0;
            }
            painter.add(mesh);
        }
        Style::Glass => gradient(
            painter,
            rect,
            CornerRadius::ZERO,
            lerp(
                palette.canvas,
                palette.accent,
                if palette.dark { 0.1 } else { 0.16 },
            ),
            palette.canvas,
            false,
        ),
        _ => {}
    }
}

/// The sidebar's background, with the line that ends it on the right.
pub fn paint_sidebar(painter: &Painter, rect: Rect, palette: &Palette) {
    let line = Stroke::new(1.0, palette.line);
    match style() {
        Style::Modern => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
        }
        Style::Luna | Style::Glass => {
            let (top, bottom) = if style() == Style::Luna {
                (0.08, 0.22)
            } else {
                (0.02, 0.1)
            };
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                lerp(palette.surface, palette.accent, top),
                lerp(palette.surface, palette.accent, bottom),
                false,
            );
        }
        Style::Gel => gradient(
            painter,
            rect,
            CornerRadius::ZERO,
            lerp(palette.surface, palette.canvas, 0.3),
            palette.canvas,
            false,
        ),
        Style::Retro | Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
            let edge = Rect::from_min_max(
                Pos2::new(rect.right() - 2.0, rect.top()),
                rect.right_bottom(),
            );
            painter.rect_filled(edge, CornerRadius::ZERO, darker(palette.canvas, 0.3));
            return;
        }
    }
    painter.line_segment([rect.right_top(), rect.right_bottom()], line);
}

/// The marker under the chosen sidebar entry or row. Returns the colour
/// text and icons on it take when the style fills it solid.
pub fn paint_selection(painter: &Painter, rect: Rect, palette: &Palette) -> Option<Color32> {
    let radius = theme::round(10);
    match style() {
        Style::Modern => {
            painter.rect_filled(rect, radius, palette.selection);
            None
        }
        Style::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.accent);
            Some(palette.accent_ink)
        }
        Style::Luna | Style::Gel => {
            gradient(
                painter,
                rect,
                radius,
                lighter(palette.accent, 0.3),
                palette.accent,
                false,
            );
            if style() == Style::Gel {
                gloss(painter, rect, radius, 0.6);
            }
            Some(palette.accent_ink)
        }
        Style::Glass => {
            let base = lerp(palette.surface, palette.accent, 0.18);
            gradient(painter, rect, radius, lighter(base, 0.4), base, false);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, lerp(palette.accent, palette.surface, 0.45)),
                StrokeKind::Inside,
            );
            None
        }
        Style::Platinum => {
            painter.rect_filled(rect, radius, lerp(palette.surface, palette.accent, 0.35));
            None
        }
    }
}

/// Whether the style marks the chosen sidebar entry with Modern's bar.
pub fn selection_bar() -> bool {
    style() == Style::Modern
}

/// A title strip across the top of a panel, like an old window's title
/// bar. Returns the colour its text takes, or none for styles without
/// strips.
pub fn paint_title_strip(painter: &Painter, rect: Rect, palette: &Palette) -> Option<Color32> {
    let radius = theme::round(12);
    let top_only = CornerRadius {
        sw: 0,
        se: 0,
        ..radius
    };
    match style() {
        Style::Modern => None,
        Style::Retro => {
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                palette.accent,
                lighter(palette.accent, 0.55),
                true,
            );
            Some(Color32::WHITE)
        }
        Style::Luna => {
            gradient(
                painter,
                rect,
                top_only,
                lighter(palette.accent, 0.35),
                darker(palette.accent, 0.08),
                false,
            );
            gloss(painter, rect, top_only, 0.5);
            Some(Color32::WHITE)
        }
        Style::Glass => {
            let base = if palette.dark {
                lerp(palette.surface, palette.accent, 0.25)
            } else {
                lerp(palette.accent, Color32::WHITE, 0.55)
            };
            gradient(painter, rect, top_only, lighter(base, 0.25), base, false);
            gloss(painter, rect, top_only, 0.7);
            Some(palette.ink)
        }
        Style::Gel => {
            let base = if palette.dark {
                lighter(palette.surface, 0.1)
            } else {
                Color32::from_rgb(0xdc, 0xdc, 0xdc)
            };
            gradient(painter, rect, top_only, lighter(base, 0.5), base, false);
            Some(palette.ink)
        }
        Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
            let mut mesh = Mesh::default();
            let mut y = rect.top() + 5.0;
            while y < rect.bottom() - 5.0 {
                let line = |at: f32| {
                    Rect::from_min_max(
                        Pos2::new(rect.left() + 4.0, at),
                        Pos2::new(rect.right() - 4.0, at + 1.0),
                    )
                };
                mesh.add_colored_rect(line(y), darker(palette.canvas, 0.25));
                mesh.add_colored_rect(line(y + 1.0), lighter(palette.canvas, 0.7));
                y += 3.0;
            }
            painter.add(mesh);
            Some(palette.ink)
        }
    }
}

/// Whether a title strip's text needs the strip cleared behind it, as
/// Platinum's lines would run through it.
pub fn title_needs_backing() -> bool {
    style() == Style::Platinum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_reach_both_ends_and_stay_inside() {
        let (a, b) = (
            Color32::from_rgb(0, 100, 200),
            Color32::from_rgb(200, 100, 0),
        );
        assert_eq!(lerp(a, b, 0.0), a);
        assert_eq!(lerp(a, b, 1.0), b);
        assert_eq!(lerp(a, b, -1.0), a);
        assert_eq!(lerp(a, b, 2.0), b);
        assert_eq!(lerp(a, b, 0.5), Color32::from_rgb(100, 100, 100));
    }
}
