//! How each style draws the pieces everything is built from, after the
//! desktops they're named for: Retro after Windows 98, Luna after Windows
//! XP, Glass after Windows Vista and 7, Gel after Mac OS X's Aqua and
//! Platinum after Mac OS 8 and 9. Buttons, fields, check boxes, panels,
//! the canvas, the sidebar, selections, title bars, list headings and
//! group boxes. Modern draws them flat. Only egui's own shapes, so a frame
//! costs the same in any style.

use super::theme::{self, Palette};
use crate::settings::Style;
use eframe::egui::{
    epaint::{tessellator::path, Mesh, RectShape, Shadow},
    pos2, vec2, Color32, CornerRadius, Galley, Painter, Pos2, Rangef, Rect, Stroke, StrokeKind,
};
use std::sync::Arc;

/// What a button is for, which decides how much it stands out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Weight {
    /// Filled with a colour: the page's main action. The older styles
    /// mark it as their default button.
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
fn gray(level: u8) -> Color32 {
    Color32::from_gray(level)
}
fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
fn top_corners(radius: CornerRadius) -> CornerRadius {
    CornerRadius {
        sw: 0,
        se: 0,
        ..radius
    }
}

/// White or black, whichever reads better on `color`.
pub fn ink_on(color: Color32) -> Color32 {
    if theme::contrast(Color32::WHITE, color) >= theme::contrast(Color32::BLACK, color) {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
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

/// The colour at `t` of a gradient through `stops`, given as places from 0
/// to 1 in order.
fn color_at(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let Some(&(first_at, first)) = stops.first() else {
        return Color32::TRANSPARENT;
    };
    if t <= first_at {
        return first;
    }
    for pair in stops.windows(2) {
        let ((from, a), (to, b)) = (pair[0], pair[1]);
        if t <= to {
            return lerp(a, b, (t - from) / (to - from).max(1e-6));
        }
    }
    stops[stops.len() - 1].1
}

/// A top-to-bottom gradient through several `stops` across `rect`,
/// rounded by `radius`: the many-banded fills of glossy buttons and title
/// bars. Two stops at nearly the same place make a hard edge.
pub fn bands(painter: &Painter, rect: Rect, radius: CornerRadius, stops: &[(f32, Color32)]) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || stops.is_empty() {
        return;
    }
    let limit = rect.width().min(rect.height()) / 2.0;
    let clamp = |radius: u8| (radius as f32).min(limit);
    let (nw, ne, sw, se) = (
        clamp(radius.nw),
        clamp(radius.ne),
        clamp(radius.sw),
        clamp(radius.se),
    );
    // How far in from its side a corner of `radius` keeps the outline,
    // `from_edge` away from the top or bottom.
    let inset = |radius: f32, from_edge: f32| {
        if radius <= 0.0 || from_edge >= radius {
            0.0
        } else {
            let d = radius - from_edge.max(0.0);
            radius - (radius * radius - d * d).max(0.0).sqrt()
        }
    };
    let left = |y: f32| rect.left() + inset(nw, y - rect.top()).max(inset(sw, rect.bottom() - y));
    let right = |y: f32| rect.right() - inset(ne, y - rect.top()).max(inset(se, rect.bottom() - y));
    let color = |y: f32| color_at(stops, (y - rect.top()) / rect.height());
    let mut ys: Vec<f32> = stops
        .iter()
        .map(|(at, _)| rect.top() + at.clamp(0.0, 1.0) * rect.height())
        .collect();
    for step in 0..=8 {
        let f = step as f32 / 8.0;
        ys.extend([rect.top() + nw * f, rect.top() + ne * f]);
        ys.extend([rect.bottom() - sw * f, rect.bottom() - se * f]);
    }
    ys.extend([rect.top(), rect.bottom()]);
    ys.retain(|y| (rect.top()..=rect.bottom()).contains(y));
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    let mut mesh = Mesh::default();
    for pair in ys.windows(2) {
        let (top, bottom) = (pair[0], pair[1]);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(left(top), top), color(top));
        mesh.colored_vertex(pos2(right(top), top), color(top));
        mesh.colored_vertex(pos2(left(bottom), bottom), color(bottom));
        mesh.colored_vertex(pos2(right(bottom), bottom), color(bottom));
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
    }
    painter.add(mesh);
}

/// Every stop of `stops` faded to `opacity`.
fn fade_all<const N: usize>(stops: [(f32, Color32); N], opacity: f32) -> [(f32, Color32); N] {
    stops.map(|(at, color)| (at, faded(color, opacity)))
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

/// Windows 98's two-step edge around `rect`: white and light grey above,
/// black and dark grey below; swapped when `sunken`.
fn retro_edge(painter: &Painter, rect: Rect, face: Color32, sunken: bool, opacity: f32) {
    let shade = |color| faded(color, opacity);
    let (outer_light, outer_dark) = (shade(lighter(face, 0.9)), shade(darker(face, 0.85)));
    let (inner_light, inner_dark) = (shade(lighter(face, 0.45)), shade(darker(face, 0.33)));
    if sunken {
        bevel(painter, rect, inner_dark, outer_light);
        bevel(painter, rect.shrink(1.0), outer_dark, inner_light);
    } else {
        bevel(painter, rect, outer_light, outer_dark);
        bevel(painter, rect.shrink(1.0), inner_light, inner_dark);
    }
}

/// A line cut into the surface: a dark one with a light one beside it, as
/// group boxes draw.
fn etched(painter: &Painter, rect: Rect, face: Color32) {
    let stroke = |color| Stroke::new(1.0, color);
    let line = Rect::from_min_max(rect.min, rect.max - vec2(1.0, 1.0));
    painter.rect_stroke(
        line.translate(vec2(1.0, 1.0)),
        CornerRadius::ZERO,
        stroke(lighter(face, 0.85)),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        line,
        CornerRadius::ZERO,
        stroke(darker(face, 0.33)),
        StrokeKind::Inside,
    );
}

/// The shine over the top half of a gel button: brightest at the top,
/// kept off the rounded ends.
fn gloss(painter: &Painter, rect: Rect, strength: f32) {
    let side = (rect.height() * 0.25).min(rect.width() * 0.2);
    let top = Rect::from_min_max(
        pos2(rect.left() + side, rect.top() + 1.0),
        pos2(rect.right() - side, rect.center().y),
    );
    gradient(
        painter,
        top,
        pill(top),
        Color32::from_white_alpha((210.0 * strength) as u8),
        Color32::from_white_alpha((50.0 * strength) as u8),
        false,
    );
}

/// The corners of an Aqua control `rect` big: a pill, as round as the
/// person keeps corners.
fn pill(rect: Rect) -> CornerRadius {
    let roundness = theme::skin().roundness.clamp(0.0, 1.0);
    CornerRadius::same((rect.height() / 2.0 * roundness).round().min(255.0) as u8)
}

/// The blue-grey line around Windows XP's fields and lists.
fn luna_line(palette: &Palette) -> Color32 {
    if palette.dark {
        lerp(palette.bright, gray(0x60), 0.5)
    } else {
        lerp(palette.deep, gray(0xa0), 0.55)
    }
}

/// A button's frame. `fill` and `stroke` are Modern's; the other styles
/// draw their own. Returns the colour its label takes in styles that pick
/// it, as their buttons aren't filled with the accent.
pub fn paint_button(
    painter: &Painter,
    rect: Rect,
    weight: Weight,
    fill: Color32,
    stroke: Stroke,
    state: State,
    palette: &Palette,
) -> Option<Color32> {
    let State {
        hover,
        press,
        opacity,
    } = state;
    // Flat buttons stay flat until hovered in every style.
    if weight == Weight::Flat && hover + press <= 0.01 {
        return None;
    }
    let shown = if weight == Weight::Flat {
        hover.max(press)
    } else {
        1.0
    };
    let alpha = opacity * shown;
    let pressed = press > 0.5;
    let dark = palette.dark;
    let strong = weight == Weight::Strong;
    match style() {
        Style::Modern => {
            painter.rect(
                rect,
                theme::round(9),
                faded(fill, opacity),
                Stroke::new(stroke.width, faded(stroke.color, opacity)),
                StrokeKind::Inside,
            );
            None
        }
        Style::Retro => {
            let face = palette.canvas;
            painter.rect_filled(rect, CornerRadius::ZERO, faded(face, alpha));
            let mut frame = rect;
            if strong {
                // The default button's extra black line.
                painter.rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, faded(Color32::BLACK, alpha)),
                    StrokeKind::Inside,
                );
                frame = rect.shrink(1.0);
            }
            if weight == Weight::Flat {
                // A toolbar button: one thin edge, sunken while pressed.
                let (light, shade) = (
                    faded(lighter(face, 0.9), alpha),
                    faded(darker(face, 0.4), alpha),
                );
                if pressed {
                    bevel(painter, frame, shade, light);
                } else {
                    bevel(painter, frame, light, shade);
                }
            } else {
                retro_edge(painter, frame, face, pressed, alpha);
            }
            Some(palette.ink)
        }
        Style::Luna => {
            let radius = theme::round(9);
            let stops = match (pressed, dark) {
                (false, false) => [
                    (0.0, Color32::WHITE),
                    (0.6, rgb(0xf4f3ee)),
                    (0.85, rgb(0xebe8de)),
                    (1.0, rgb(0xd6d0c5)),
                ],
                (true, false) => [
                    (0.0, rgb(0xe3e2da)),
                    (0.6, rgb(0xe6e5de)),
                    (0.85, rgb(0xeeede7)),
                    (1.0, rgb(0xf3f3ef)),
                ],
                (false, true) => {
                    let base = lighter(palette.surface, 0.1);
                    [
                        (0.0, lighter(base, 0.16)),
                        (0.6, base),
                        (0.85, darker(base, 0.08)),
                        (1.0, darker(base, 0.25)),
                    ]
                }
                (true, true) => {
                    let base = darker(palette.surface, 0.1);
                    [
                        (0.0, base),
                        (0.6, base),
                        (0.85, lighter(base, 0.04)),
                        (1.0, lighter(base, 0.08)),
                    ]
                }
            };
            bands(painter, rect, radius, &fade_all(stops, alpha));
            let border = if dark {
                lighter(palette.deep, 0.25)
            } else {
                darker(palette.deep, 0.45)
            };
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(border, alpha)),
                StrokeKind::Inside,
            );
            // Hovered buttons glow orange; the default one glows blue.
            let glow = rgb(0xf8b330);
            if hover > 0.0 && !pressed {
                painter.rect_stroke(
                    rect.shrink(1.0),
                    radius,
                    Stroke::new(2.0, faded(glow, hover * alpha)),
                    StrokeKind::Inside,
                );
            }
            if strong && hover < 1.0 && !pressed {
                painter.rect_stroke(
                    rect.shrink(1.0),
                    radius,
                    Stroke::new(
                        2.0,
                        faded(
                            lerp(palette.deep, Color32::WHITE, 0.45),
                            (1.0 - hover) * alpha,
                        ),
                    ),
                    StrokeKind::Inside,
                );
            }
            Some(if dark { palette.ink } else { Color32::BLACK })
        }
        Style::Glass => {
            let radius = theme::round(9);
            let blue = palette.deep;
            let (calm, lit, held, calm_line, lit_line) = if dark {
                let base = lighter(palette.surface, 0.1);
                let calm = [
                    lighter(base, 0.2),
                    lighter(base, 0.14),
                    base,
                    darker(base, 0.12),
                ];
                let lit = calm.map(|color| lerp(color, palette.bright, 0.3));
                let held = calm.map(|color| lerp(darker(color, 0.15), palette.bright, 0.35));
                (calm, lit, held, darker(palette.canvas, 0.5), palette.bright)
            } else {
                let tint = |amount| lerp(Color32::WHITE, blue, amount);
                (
                    [gray(0xf2), gray(0xeb), gray(0xdd), gray(0xcf)],
                    [tint(0.07), tint(0.13), tint(0.28), tint(0.36)],
                    [tint(0.12), tint(0.22), tint(0.42), tint(0.55)],
                    gray(0x70),
                    darker(lerp(blue, gray(0x80), 0.3), 0.1),
                )
            };
            // The default button sits part way to hovered.
            let lift = if strong { hover.max(0.55) } else { hover };
            let colors: Vec<Color32> = (0..4)
                .map(|i| {
                    if pressed {
                        held[i]
                    } else {
                        lerp(calm[i], lit[i], lift)
                    }
                })
                .collect();
            let stops = [
                (0.0, colors[0]),
                (0.5, colors[1]),
                (0.505, colors[2]),
                (1.0, colors[3]),
            ];
            bands(painter, rect, radius, &fade_all(stops, alpha));
            let line = if strong || pressed {
                lit_line
            } else {
                lerp(calm_line, lit_line, hover)
            };
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(line, alpha)),
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                rect.shrink(1.0),
                radius,
                Stroke::new(
                    1.0,
                    faded(
                        Color32::from_white_alpha(if dark { 30 } else { 150 }),
                        alpha,
                    ),
                ),
                StrokeKind::Inside,
            );
            Some(if dark { palette.ink } else { Color32::BLACK })
        }
        Style::Gel => {
            let radius = pill(rect);
            let blue = strong;
            let base = if blue {
                palette.deep
            } else if dark {
                lighter(palette.surface, 0.16)
            } else {
                gray(0xe6)
            };
            let base = darker(lighter(base, hover * 0.06), press * 0.15);
            let stops = if blue {
                [
                    (0.0, lighter(base, 0.45)),
                    (0.5, lighter(base, 0.2)),
                    (0.505, base),
                    (1.0, lighter(base, 0.6)),
                ]
            } else {
                let sheen = gel_sheen(dark);
                [
                    (0.0, lighter(base, 0.75 * sheen)),
                    (0.5, lighter(base, 0.45 * sheen)),
                    (0.505, base),
                    (1.0, lighter(base, 0.8 * sheen)),
                ]
            };
            // Aqua's controls cast a soft shadow down.
            painter.rect_filled(
                rect.translate(vec2(0.0, 1.0)),
                radius,
                Color32::from_black_alpha((40.0 * alpha) as u8),
            );
            bands(painter, rect, radius, &fade_all(stops, alpha));
            gloss(
                painter,
                rect,
                alpha * if blue { 1.0 } else { gel_sheen(dark) },
            );
            let line = if blue {
                darker(base, 0.45)
            } else if dark {
                darker(palette.canvas, 0.5)
            } else {
                gray(0x8c)
            };
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(line, alpha)),
                StrokeKind::Inside,
            );
            Some(if blue {
                ink_on(lighter(base, 0.2))
            } else if dark {
                palette.ink
            } else {
                Color32::BLACK
            })
        }
        Style::Platinum => {
            let radius = theme::round(9);
            let outline = if dark {
                lighter(palette.canvas, 0.55)
            } else {
                Color32::BLACK
            };
            let mut body = rect;
            if strong {
                // Mac OS's default button wears a thick ring.
                painter.rect_stroke(
                    rect,
                    theme::round(14),
                    Stroke::new(3.0, faded(outline, alpha)),
                    StrokeKind::Inside,
                );
                body = rect.shrink(4.0);
            }
            // Pressed buttons go dark with light text.
            let face = match (pressed, dark) {
                (false, false) => gray(0xdd),
                (true, false) => gray(0x66),
                (false, true) => lighter(palette.canvas, 0.12),
                (true, true) => lighter(palette.canvas, 0.4),
            };
            painter.rect_filled(body, radius, faded(face, alpha));
            let (light, shade) = (
                faded(lighter(face, 0.75), alpha),
                faded(darker(face, 0.28), alpha),
            );
            if pressed {
                bevel(painter, body.shrink(1.0), shade, light);
            } else {
                bevel(painter, body.shrink(1.0), light, shade);
            }
            painter.rect_stroke(
                body,
                radius,
                Stroke::new(1.0, faded(outline, alpha)),
                StrokeKind::Inside,
            );
            Some(if pressed { ink_on(face) } else { palette.ink })
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
    match style() {
        Style::Modern => {
            let ring = lerp(palette.line, palette.accent, focus);
            painter.rect(
                rect,
                radius,
                palette.surface,
                Stroke::new(1.0 + focus, ring),
                StrokeKind::Inside,
            );
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
            retro_edge(painter, rect, palette.canvas, true, 1.0);
        }
        Style::Luna => {
            painter.rect(
                rect,
                CornerRadius::ZERO,
                palette.surface,
                Stroke::new(1.0, lerp(luna_line(palette), palette.accent, focus)),
                StrokeKind::Inside,
            );
        }
        Style::Glass => {
            let radius = theme::round(5);
            let line = if palette.dark {
                lighter(palette.canvas, 0.3)
            } else {
                gray(0xab)
            };
            let line = lerp(line, palette.accent, focus);
            painter.rect(
                rect,
                radius,
                palette.surface,
                Stroke::new(1.0, line),
                StrokeKind::Inside,
            );
            // Vista's fields are darker along the top.
            painter.hline(
                (rect.left() + 2.0)..=(rect.right() - 2.0),
                rect.top() + 0.5,
                Stroke::new(1.0, darker(line, 0.15)),
            );
        }
        Style::Gel => {
            let radius = pill(rect);
            painter.rect_filled(rect, radius, palette.surface);
            // Fields sit a little into the window.
            let shade = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 5.0));
            gradient(
                painter,
                shade.shrink2(vec2(radius.nw as f32 * 0.5, 0.0)),
                top_corners(CornerRadius::same(radius.nw.min(5))),
                Color32::from_black_alpha(if palette.dark { 60 } else { 30 }),
                Color32::TRANSPARENT,
                false,
            );
            let line = if palette.dark {
                lighter(palette.canvas, 0.3)
            } else {
                gray(0x8e)
            };
            painter.rect_stroke(rect, radius, Stroke::new(1.0, line), StrokeKind::Inside);
            if focus > 0.0 {
                // Aqua's soft blue focus ring.
                let ring = rect.expand(3.0);
                painter.rect_stroke(
                    ring,
                    pill(ring),
                    Stroke::new(
                        3.5,
                        faded(lerp(palette.deep, Color32::WHITE, 0.4), 0.8 * focus),
                    ),
                    StrokeKind::Inside,
                );
            }
        }
        Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
            bevel(
                painter,
                rect,
                darker(palette.canvas, 0.35),
                lighter(palette.canvas, 0.8),
            );
            let outline = if palette.dark {
                lighter(palette.canvas, 0.4)
            } else {
                gray(0x44)
            };
            painter.rect_stroke(
                rect.shrink(1.0),
                CornerRadius::ZERO,
                Stroke::new(1.0, outline),
                StrokeKind::Inside,
            );
            if focus > 0.0 {
                painter.rect_stroke(
                    rect.expand(3.0),
                    CornerRadius::ZERO,
                    Stroke::new(2.0, faded(lerp(palette.deep, Color32::WHITE, 0.35), focus)),
                    StrokeKind::Inside,
                );
            }
        }
    }
}

/// A check box, filled by `checked` (0 to 1). Returns the colour its tick
/// takes, or none when the style drew its own mark, as Platinum's ×.
pub fn paint_check(
    painter: &Painter,
    rect: Rect,
    palette: &Palette,
    checked: f32,
    hovered: bool,
    opacity: f32,
) -> Option<Color32> {
    let checked = checked.clamp(0.0, 1.0);
    let dark = palette.dark;
    match style() {
        Style::Modern => {
            let border = if hovered {
                palette.accent
            } else {
                palette.strong_line
            };
            painter.rect(
                rect,
                theme::round(5),
                faded(palette.accent, checked * opacity),
                Stroke::new(1.5, faded(lerp(border, palette.accent, checked), opacity)),
                StrokeKind::Inside,
            );
            Some(faded(palette.accent_ink, opacity))
        }
        Style::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, faded(palette.surface, opacity));
            retro_edge(painter, rect, palette.canvas, true, opacity);
            Some(faded(palette.ink, opacity))
        }
        Style::Luna => {
            let (top, bottom) = if dark {
                (darker(palette.surface, 0.15), palette.surface)
            } else {
                (rgb(0xdcdcd7), Color32::WHITE)
            };
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                faded(top, opacity),
                faded(bottom, opacity),
                false,
            );
            let border = if dark {
                lighter(palette.deep, 0.3)
            } else {
                darker(palette.deep, 0.5)
            };
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, faded(border, opacity)),
                StrokeKind::Inside,
            );
            if hovered {
                painter.rect_stroke(
                    rect.shrink(1.0),
                    CornerRadius::ZERO,
                    Stroke::new(2.0, faded(rgb(0xf8b330), opacity)),
                    StrokeKind::Inside,
                );
            }
            // XP's green tick.
            Some(faded(
                if dark { rgb(0x5cd05c) } else { rgb(0x21a121) },
                opacity,
            ))
        }
        Style::Glass => {
            let radius = theme::round(4);
            let line = if dark {
                lighter(palette.canvas, 0.4)
            } else {
                gray(0x8f)
            };
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(
                    1.0,
                    faded(lerp(line, palette.accent, hovered as u8 as f32), opacity),
                ),
                StrokeKind::Inside,
            );
            let inner = rect.shrink(2.0);
            let (top, bottom) = match (dark, hovered) {
                (false, false) => (gray(0xf4), gray(0xdf)),
                (false, true) => (
                    lerp(Color32::WHITE, palette.deep, 0.1),
                    lerp(Color32::WHITE, palette.deep, 0.28),
                ),
                (true, _) => (lighter(palette.surface, 0.2), palette.surface),
            };
            gradient(
                painter,
                inner,
                CornerRadius::ZERO,
                faded(top, opacity),
                faded(bottom, opacity),
                false,
            );
            Some(faded(
                if dark {
                    palette.bright
                } else {
                    darker(lerp(palette.deep, gray(0x50), 0.4), 0.25)
                },
                opacity,
            ))
        }
        Style::Gel => {
            let radius = CornerRadius::same(
                (rect.height() * 0.22 * theme::skin().roundness.clamp(0.0, 2.0)).round() as u8,
            );
            let clear = if dark {
                lighter(palette.surface, 0.16)
            } else {
                gray(0xe6)
            };
            let blue = palette.deep;
            let base = lerp(clear, blue, checked);
            let sheen = gel_sheen(dark) + (1.0 - gel_sheen(dark)) * checked;
            let stops = [
                (0.0, lighter(base, 0.6 * sheen)),
                (0.5, lighter(base, 0.35 * sheen)),
                (0.505, base),
                (1.0, lighter(base, 0.7 * sheen)),
            ];
            bands(painter, rect, radius, &fade_all(stops, opacity));
            gloss(painter, rect, opacity * sheen);
            let line = lerp(
                if dark {
                    darker(palette.canvas, 0.5)
                } else {
                    gray(0x8c)
                },
                darker(blue, 0.45),
                checked,
            );
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(line, opacity)),
                StrokeKind::Inside,
            );
            Some(faded(ink_on(lighter(blue, 0.2)), opacity))
        }
        Style::Platinum => {
            let (top, bottom) = if dark {
                (lighter(palette.canvas, 0.2), lighter(palette.canvas, 0.08))
            } else {
                (gray(0xf4), gray(0xc8))
            };
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                faded(top, opacity),
                faded(bottom, opacity),
                false,
            );
            bevel(
                painter,
                rect.shrink(1.0),
                faded(lighter(top, 0.6), opacity),
                faded(darker(bottom, 0.2), opacity),
            );
            let outline = if dark {
                lighter(palette.canvas, 0.55)
            } else {
                Color32::BLACK
            };
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(if hovered { 2.0 } else { 1.0 }, faded(outline, opacity)),
                StrokeKind::Inside,
            );
            if checked > 0.01 {
                // Mac OS marks a check box with a cross.
                let reach = rect.width().min(rect.height()) / 2.0 - 3.5;
                let arm = vec2(reach, reach) * checked;
                let across = vec2(reach, -reach) * checked;
                let ink = Stroke::new(1.8, faded(palette.ink, opacity));
                let center = rect.center();
                painter.line_segment([center - arm, center + arm], ink);
                painter.line_segment([center - across, center + across], ink);
            }
            None
        }
    }
}

/// Whether switches draw as check boxes: none of the older desktops had
/// switches.
pub fn switch_as_check() -> bool {
    style() != Style::Modern
}

/// A switch's knob: white, with a soft shadow under it.
pub fn paint_knob(painter: &Painter, center: Pos2, radius: f32, opacity: f32) {
    painter.circle_filled(
        center + vec2(0.0, 0.6),
        radius,
        Color32::from_black_alpha((46.0 * opacity) as u8),
    );
    painter.circle_filled(center, radius, faded(Color32::WHITE, opacity));
}

/// The shadow a panel casts, if the style has one.
pub fn shadow(palette: &Palette) -> Shadow {
    let (offset, blur, alpha) = match style() {
        Style::Modern => (1, 6, if palette.dark { 40 } else { 10 }),
        Style::Glass => (2, 10, if palette.dark { 80 } else { 26 }),
        Style::Gel => (3, 12, if palette.dark { 90 } else { 40 }),
        Style::Retro | Style::Luna | Style::Platinum => (0, 0, 0),
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
    let color = match style() {
        Style::Modern => palette.line,
        Style::Retro => darker(palette.canvas, 0.6),
        Style::Luna => luna_line(palette),
        Style::Glass if palette.dark => lighter(palette.canvas, 0.22),
        Style::Glass | Style::Gel => lerp(palette.ink, palette.surface, 0.62),
        Style::Platinum => lerp(palette.ink, palette.canvas, 0.3),
    };
    Stroke::new(1.0, color)
}

/// How round a panel is: lists sit square in the older styles.
fn panel_radius(sunken: bool) -> CornerRadius {
    match style() {
        Style::Retro => CornerRadius::ZERO,
        Style::Luna | Style::Platinum if sunken => CornerRadius::ZERO,
        _ => theme::round(12),
    }
}

/// A panel: the list, a card. `sunken` ones hold things, like a list box.
pub fn paint_panel(painter: &Painter, rect: Rect, palette: &Palette, sunken: bool, stroke: Stroke) {
    let radius = panel_radius(sunken);
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
        Style::Retro => retro_edge(painter, rect, palette.canvas, sunken, 1.0),
        Style::Platinum => {
            let (light, dark) = (lighter(palette.surface, 0.8), darker(palette.canvas, 0.3));
            if sunken {
                bevel(painter, rect.shrink(1.0), dark, light);
            } else {
                bevel(painter, rect.shrink(1.0), light, dark);
            }
        }
        Style::Modern | Style::Luna | Style::Glass | Style::Gel => {}
    }
}

/// Behind everything: the canvas colour, with Aqua's pinstripes or
/// Vista's glass.
pub fn paint_canvas(painter: &Painter, rect: Rect, palette: &Palette) {
    painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
    match style() {
        Style::Gel => {
            let stripe = if palette.dark {
                Color32::from_white_alpha(7)
            } else {
                Color32::from_white_alpha(150)
            };
            let mut mesh = Mesh::default();
            let mut y = rect.top();
            while y < rect.bottom() {
                mesh.add_colored_rect(
                    Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right(), y + 2.0)),
                    stripe,
                );
                y += 4.0;
            }
            painter.add(mesh);
        }
        Style::Glass => {
            // Tinted glass, deepest at the top, with light caught in streaks.
            let (top, middle, bottom) = if palette.dark {
                (
                    lerp(palette.canvas, palette.deep, 0.5),
                    lerp(palette.canvas, palette.deep, 0.25),
                    palette.canvas,
                )
            } else {
                (
                    lerp(palette.canvas, palette.deep, 0.42),
                    lerp(palette.canvas, palette.deep, 0.18),
                    lighter(palette.canvas, 0.35),
                )
            };
            let reach = (rect.height() / 600.0).clamp(0.05, 1.0);
            bands(
                painter,
                rect,
                CornerRadius::ZERO,
                &[(0.0, top), (0.4 / reach.max(0.4), middle), (1.0, bottom)],
            );
            let shine = if palette.dark { 9 } else { 30 };
            let slant = rect.height().min(900.0) * 0.4;
            let mut mesh = Mesh::default();
            let mut x = rect.left() - slant;
            let widths = [70.0, 24.0, 110.0, 36.0];
            let gaps = [40.0, 150.0, 60.0, 220.0];
            let mut index = 0;
            let lit = Color32::from_white_alpha(shine);
            let bottom = rect.top() + slant * 2.0;
            while x < rect.right() {
                let width = widths[index % widths.len()];
                let streak = [
                    (pos2(x + slant, rect.top()), lit),
                    (pos2(x + slant + width, rect.top()), lit),
                    (pos2(x - slant + width, bottom), Color32::TRANSPARENT),
                    (pos2(x - slant, bottom), Color32::TRANSPARENT),
                ];
                fan(&mut mesh, &clip_sides(&streak, rect.x_range()));
                x += width + gaps[index % gaps.len()];
                index += 1;
            }
            painter.add(mesh);
        }
        _ => {}
    }
}

/// The part of the convex `polygon` between the sides of `x`, its colours
/// blended where it's cut.
fn clip_sides(polygon: &[(Pos2, Color32)], x: Rangef) -> Vec<(Pos2, Color32)> {
    let mut points = polygon.to_vec();
    for (edge, keep_right) in [(x.min, true), (x.max, false)] {
        let inside = |p: Pos2| if keep_right { p.x >= edge } else { p.x <= edge };
        let mut kept = Vec::with_capacity(points.len() + 2);
        for (i, &(a, a_color)) in points.iter().enumerate() {
            let (b, b_color) = points[(i + 1) % points.len()];
            if inside(a) {
                kept.push((a, a_color));
            }
            if inside(a) != inside(b) {
                let t = (edge - a.x) / (b.x - a.x);
                kept.push((a.lerp(b, t), lerp(a_color, b_color, t)));
            }
        }
        points = kept;
    }
    points
}

/// Adds the convex `polygon` to `mesh` as a fan of triangles.
fn fan(mesh: &mut Mesh, polygon: &[(Pos2, Color32)]) {
    let base = mesh.vertices.len() as u32;
    for &(pos, color) in polygon {
        mesh.colored_vertex(pos, color);
    }
    for i in 2..polygon.len() as u32 {
        mesh.add_triangle(base, base + i - 1, base + i);
    }
}

/// The colours the sidebar's name and entries take when the style picks
/// them, as XP's blue links.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SidebarInk {
    pub name: Option<Color32>,
    pub entries: Option<Color32>,
}

/// The sidebar's background: `logo` is the row with the name, `entries`
/// holds the pages.
pub fn paint_sidebar(
    painter: &Painter,
    rect: Rect,
    logo: Rect,
    entries: Rect,
    palette: &Palette,
) -> SidebarInk {
    let dark = palette.dark;
    let end = |color| {
        painter.vline(
            rect.right() - 0.5,
            rect.top()..=rect.bottom(),
            Stroke::new(1.0, color),
        )
    };
    match style() {
        Style::Modern => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
            end(palette.line);
        }
        Style::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
            // The pages in a list box, as Explorer's folders.
            let list = entries.expand2(vec2(6.0, 8.0));
            painter.rect_filled(list, CornerRadius::ZERO, palette.surface);
            retro_edge(painter, list, palette.canvas, true, 1.0);
            bevel(
                painter,
                Rect::from_min_max(pos2(rect.right() - 2.0, rect.top()), rect.max),
                lighter(palette.canvas, 0.9),
                darker(palette.canvas, 0.4),
            );
        }
        Style::Luna => {
            // The blue task pane, with the pages in a task box.
            let (top, bottom) = if dark {
                (
                    lerp(palette.canvas, palette.deep, 0.35),
                    lerp(palette.canvas, palette.deep, 0.18),
                )
            } else {
                (
                    lerp(palette.deep, Color32::WHITE, 0.45),
                    lerp(palette.deep, Color32::WHITE, 0.22),
                )
            };
            gradient(painter, rect, CornerRadius::ZERO, top, bottom, false);
            let margin = (rect.width() * 0.06).clamp(4.0, 12.0);
            let header = Rect::from_min_max(
                pos2(rect.left() + margin, logo.top() - 10.0),
                pos2(rect.right() - margin, logo.bottom() + 10.0),
            );
            let body = Rect::from_min_max(
                header.left_bottom(),
                pos2(header.right(), entries.bottom() + 10.0),
            );
            let radius = top_corners(theme::round(12));
            let (header_top, header_bottom, inside) = if dark {
                (
                    lighter(palette.surface, 0.14),
                    palette.surface,
                    darker(palette.surface, 0.12),
                )
            } else {
                (
                    Color32::WHITE,
                    lerp(palette.deep, Color32::WHITE, 0.78),
                    lerp(palette.deep, Color32::WHITE, 0.84),
                )
            };
            gradient(painter, header, radius, header_top, header_bottom, true);
            painter.rect_filled(body, CornerRadius::ZERO, inside);
            painter.rect_stroke(
                body,
                CornerRadius::ZERO,
                Stroke::new(1.0, Color32::from_white_alpha(if dark { 20 } else { 255 })),
                StrokeKind::Inside,
            );
            let link = if dark {
                palette.bright
            } else {
                darker(palette.deep, 0.15)
            };
            return SidebarInk {
                name: Some(link),
                entries: Some(link),
            };
        }
        Style::Glass => {
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                if dark {
                    Color32::from_black_alpha(60)
                } else {
                    Color32::from_white_alpha(60)
                },
            );
            end(Color32::from_white_alpha(if dark { 24 } else { 120 }));
        }
        Style::Gel => {
            // The blue-grey source list.
            let list = if dark {
                lerp(palette.canvas, palette.deep, 0.1)
            } else {
                lerp(gray(0xdc), palette.deep, 0.07)
            };
            painter.rect_filled(rect, CornerRadius::ZERO, list);
            end(if dark { Color32::BLACK } else { gray(0x9a) });
        }
        Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
            bevel(
                painter,
                Rect::from_min_max(pos2(rect.right() - 2.0, rect.top()), rect.max),
                lighter(palette.canvas, 0.8),
                darker(palette.canvas, 0.35),
            );
        }
    }
    SidebarInk::default()
}

/// The marker under the chosen sidebar entry. Returns the colour text and
/// icons on it take when the style fills it solid.
pub fn paint_selection(painter: &Painter, rect: Rect, palette: &Palette) -> Option<Color32> {
    match style() {
        Style::Modern => {
            painter.rect_filled(rect, theme::round(10), palette.selection);
            None
        }
        Style::Retro | Style::Luna => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.deep);
            Some(ink_on(palette.deep))
        }
        Style::Glass => {
            glass_selection(painter, rect, palette, 1.0);
            None
        }
        Style::Gel => {
            // A source list's selection: a blue band with a light top edge.
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                lighter(palette.deep, 0.3),
                darker(palette.deep, 0.1),
                false,
            );
            painter.hline(
                rect.x_range(),
                rect.top() + 0.5,
                Stroke::new(1.0, lighter(palette.deep, 0.5)),
            );
            Some(Color32::WHITE)
        }
        Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, platinum_highlight(palette));
            None
        }
    }
}

/// Mac OS's lavender highlight, from the accent.
fn platinum_highlight(palette: &Palette) -> Color32 {
    if palette.dark {
        lerp(palette.surface, palette.bright, 0.35)
    } else {
        lerp(Color32::WHITE, palette.deep, 0.3)
    }
}

/// Windows 7's pale blue selection with its darker edge.
fn glass_selection(painter: &Painter, rect: Rect, palette: &Palette, shown: f32) {
    let radius = theme::round(10);
    let (top, bottom, line) = if palette.dark {
        (
            lerp(palette.surface, palette.bright, 0.28),
            lerp(palette.surface, palette.bright, 0.18),
            lerp(palette.surface, palette.bright, 0.55),
        )
    } else {
        (
            lerp(Color32::WHITE, palette.deep, 0.1),
            lerp(Color32::WHITE, palette.deep, 0.22),
            lerp(Color32::WHITE, palette.deep, 0.5),
        )
    };
    gradient(
        painter,
        rect,
        radius,
        faded(top, shown),
        faded(bottom, shown),
        false,
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, faded(line, shown)),
        StrokeKind::Inside,
    );
}

/// Whether the style marks the chosen sidebar entry with Modern's bar.
pub fn selection_bar() -> bool {
    style() == Style::Modern
}

/// The colour a selected list row's text takes, when the style fills the
/// selection solid.
pub fn row_selection_ink(palette: &Palette) -> Option<Color32> {
    match style() {
        Style::Retro | Style::Luna | Style::Gel => Some(ink_on(palette.deep)),
        Style::Modern | Style::Glass | Style::Platinum => None,
    }
}

/// `palette` for things drawn on a solid selection in `ink`.
pub fn on_fill(palette: &Palette, ink: Color32) -> Palette {
    Palette {
        ink,
        muted: lerp(ink, palette.deep, 0.25),
        accent: ink,
        accent_ink: palette.deep,
        line: faded(ink, 0.25),
        strong_line: faded(ink, 0.5),
        ..*palette
    }
}

/// The selected list row's background, `shown` from 0 to 1.
pub fn paint_row_selection(painter: &Painter, rect: Rect, palette: &Palette, shown: f32) {
    match style() {
        Style::Modern => {
            painter.rect_filled(rect, theme::round(10), faded(palette.selection, shown));
        }
        Style::Retro | Style::Luna | Style::Gel => {
            painter.rect_filled(rect, CornerRadius::ZERO, faded(palette.deep, shown));
        }
        Style::Glass => glass_selection(painter, rect, palette, shown),
        Style::Platinum => {
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                faded(platinum_highlight(palette), shown),
            );
        }
    }
}

/// A window's title bar across `rect`, as the page's heading sits in.
/// Returns the colour its text takes, or none for Modern, which has none.
pub fn paint_title_strip(painter: &Painter, rect: Rect, palette: &Palette) -> Option<Color32> {
    let dark = palette.dark;
    match style() {
        Style::Modern => None,
        Style::Retro => {
            // Windows 98's navy fading to a lighter blue.
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                palette.deep,
                lerp(palette.deep, palette.bright, 0.85),
                true,
            );
            Some(ink_on(palette.deep))
        }
        Style::Luna => {
            let radius = top_corners(theme::round(18));
            let blue = palette.deep;
            bands(
                painter,
                rect,
                radius,
                &[
                    (0.0, lerp(blue, Color32::WHITE, 0.5)),
                    (0.1, lighter(blue, 0.15)),
                    (0.45, blue),
                    (0.85, darker(blue, 0.08)),
                    (1.0, darker(blue, 0.3)),
                ],
            );
            Some(ink_on(blue))
        }
        Style::Glass => {
            // The page's title sits on the glass itself.
            Some(palette.ink)
        }
        Style::Gel => {
            let radius = top_corners(theme::round(14));
            let (top, bottom, line) = if dark {
                (
                    lighter(palette.surface, 0.16),
                    palette.surface,
                    Color32::BLACK,
                )
            } else {
                (gray(0xec), gray(0xbe), gray(0x7e))
            };
            gradient(painter, rect, radius, top, bottom, false);
            painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, line));
            Some(palette.ink)
        }
        Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
            bevel(
                painter,
                rect,
                lighter(palette.canvas, 0.8),
                darker(palette.canvas, 0.3),
            );
            let mut mesh = Mesh::default();
            let mut y = rect.top() + 4.0;
            while y < rect.bottom() - 5.0 {
                let line = |at: f32| {
                    Rect::from_min_max(
                        pos2(rect.left() + 4.0, at),
                        pos2(rect.right() - 4.0, at + 1.0),
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

/// A title bar's text at `pos`, with what the style puts behind it: XP's
/// shadow, Vista's glow, Aqua's emboss, Platinum's clearing in the lines.
pub fn paint_title_text(
    painter: &Painter,
    pos: Pos2,
    galley: Arc<Galley>,
    ink: Color32,
    palette: &Palette,
) {
    let area = Rect::from_min_size(pos, galley.size());
    match style() {
        Style::Luna => {
            painter.galley_with_override_text_color(
                pos + vec2(1.0, 1.0),
                galley.clone(),
                faded(darker(palette.deep, 0.6), 0.8),
            );
        }
        Style::Glass => {
            let glow = if palette.dark {
                Color32::from_black_alpha(140)
            } else {
                Color32::from_white_alpha(190)
            };
            painter.add(
                RectShape::filled(area.expand2(vec2(10.0, 2.0)), CornerRadius::same(12), glow)
                    .with_blur_width(16.0),
            );
        }
        Style::Gel if !palette.dark => {
            painter.galley_with_override_text_color(
                pos + vec2(0.0, 1.0),
                galley.clone(),
                Color32::from_white_alpha(150),
            );
        }
        Style::Platinum => {
            painter.rect_filled(
                area.expand2(vec2(6.0, 0.0)),
                CornerRadius::ZERO,
                palette.canvas,
            );
        }
        _ => {}
    }
    painter.galley_with_override_text_color(pos, galley, ink);
}

/// A list's column headings as the style draws them: in title case in
/// the older styles, capitals in Modern.
pub fn column_title(title: &str) -> String {
    if style() == Style::Modern {
        return title.to_owned();
    }
    let lower = title.to_lowercase();
    let mut letters = lower.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// The band behind a list's column headings. False in Modern, which only
/// draws a line under them.
pub fn paint_column_band(painter: &Painter, rect: Rect, palette: &Palette) -> bool {
    let dark = palette.dark;
    match style() {
        Style::Modern => return false,
        Style::Retro | Style::Platinum => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
        }
        Style::Luna => {
            let (top, bottom) = if dark {
                (lighter(palette.surface, 0.12), palette.surface)
            } else {
                (Color32::WHITE, rgb(0xebeadb))
            };
            gradient(painter, rect, CornerRadius::ZERO, top, bottom, false);
            // XP's headings stand on three lines, each a little darker.
            let lines = if dark {
                [
                    darker(palette.surface, 0.2),
                    darker(palette.surface, 0.3),
                    darker(palette.surface, 0.4),
                ]
            } else {
                [rgb(0xe2decd), rgb(0xd6d2c2), rgb(0xcbc7b8)]
            };
            for (step, color) in lines.into_iter().enumerate() {
                painter.hline(
                    rect.x_range(),
                    rect.bottom() - 2.5 + step as f32,
                    Stroke::new(1.0, color),
                );
            }
        }
        Style::Glass => {
            let (top, bottom) = if dark {
                (lighter(palette.surface, 0.08), palette.surface)
            } else {
                (Color32::WHITE, rgb(0xf4f6f9))
            };
            gradient(painter, rect, CornerRadius::ZERO, top, bottom, false);
            painter.hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                Stroke::new(1.0, lerp(palette.ink, palette.surface, 0.82)),
            );
        }
        Style::Gel => {
            bands(painter, rect, CornerRadius::ZERO, &gel_band(palette, None));
            painter.hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                Stroke::new(1.0, if dark { Color32::BLACK } else { gray(0x9a) }),
            );
        }
    }
    true
}

/// Aqua's heading band: grey gloss, or blue when it's the sorted column.
fn gel_band(palette: &Palette, blue: Option<Color32>) -> [(f32, Color32); 4] {
    let base = blue.unwrap_or(if palette.dark {
        lighter(palette.surface, 0.1)
    } else {
        gray(0xe4)
    });
    let sheen = if blue.is_some() {
        1.0
    } else {
        gel_sheen(palette.dark)
    };
    [
        (0.0, lighter(base, 0.8 * sheen)),
        (0.5, lighter(base, 0.5 * sheen)),
        (0.505, base),
        (1.0, lighter(base, 0.45 * sheen)),
    ]
}

/// How much shine Aqua's clear gel keeps: a dark window turns it down so
/// light text still reads on it.
fn gel_sheen(dark: bool) -> f32 {
    if dark {
        0.3
    } else {
        1.0
    }
}

/// One column heading's cell. Returns the colour its title takes when the
/// style picks it, as Aqua's blue sorted column.
pub fn paint_column_cell(
    painter: &Painter,
    cell: Rect,
    palette: &Palette,
    sorted: bool,
    hovered: bool,
) -> Option<Color32> {
    let dark = palette.dark;
    let divider = |color| {
        painter.vline(
            cell.right() - 0.5,
            (cell.top() + 5.0)..=(cell.bottom() - 6.0),
            Stroke::new(1.0, color),
        );
    };
    match style() {
        Style::Modern => None,
        Style::Retro => {
            retro_edge(painter, cell, palette.canvas, false, 1.0);
            None
        }
        Style::Platinum => {
            if sorted {
                painter.rect_filled(cell, CornerRadius::ZERO, darker(palette.canvas, 0.12));
            }
            bevel(
                painter,
                cell,
                lighter(palette.canvas, 0.8),
                darker(palette.canvas, 0.35),
            );
            None
        }
        Style::Luna => {
            divider(if dark {
                darker(palette.surface, 0.3)
            } else {
                rgb(0xc7c5b2)
            });
            if hovered {
                painter.rect_filled(
                    Rect::from_min_max(pos2(cell.left(), cell.bottom() - 3.0), cell.max),
                    CornerRadius::ZERO,
                    rgb(0xf9b03d),
                );
            }
            None
        }
        Style::Glass => {
            if hovered {
                let tint = |amount| lerp(palette.surface, palette.deep, amount);
                gradient(
                    painter,
                    cell,
                    CornerRadius::ZERO,
                    tint(0.06),
                    tint(0.16),
                    false,
                );
                painter.rect_stroke(
                    cell,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, tint(0.4)),
                    StrokeKind::Inside,
                );
            } else {
                divider(lerp(palette.ink, palette.surface, 0.84));
            }
            None
        }
        Style::Gel => {
            if sorted {
                let blue = lerp(palette.deep, Color32::WHITE, 0.2);
                bands(
                    painter,
                    cell,
                    CornerRadius::ZERO,
                    &gel_band(palette, Some(blue)),
                );
                divider(darker(blue, 0.3));
                Some(ink_on(blue))
            } else {
                divider(if dark { Color32::BLACK } else { gray(0xb4) });
                None
            }
        }
    }
}

/// How a group of settings is framed: Modern's card, or the older
/// styles' group boxes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Group {
    pub fill: Color32,
    pub stroke: Stroke,
    pub radius: CornerRadius,
    pub shadow: Shadow,
    /// The title's colour.
    pub ink: Color32,
    /// The title sits on the box's top line, rather than inside it.
    pub on_line: bool,
    pub bold: bool,
    pub size: f32,
}

/// The group box a style frames settings in; none for Modern's cards.
pub fn group(palette: &Palette) -> Option<Group> {
    let dark = palette.dark;
    let none = Group {
        fill: Color32::TRANSPARENT,
        stroke: Stroke::NONE,
        radius: CornerRadius::ZERO,
        shadow: Shadow::NONE,
        ink: palette.ink,
        on_line: true,
        bold: false,
        size: 14.0,
    };
    Some(match style() {
        Style::Modern => return None,
        // Etched after the content, in `paint_group`.
        Style::Retro => none,
        Style::Platinum => Group { bold: true, ..none },
        Style::Luna => Group {
            stroke: Stroke::new(1.0, darker(palette.canvas, 0.14)),
            radius: theme::round(8),
            ink: if dark {
                palette.bright
            } else {
                darker(palette.deep, 0.05)
            },
            ..none
        },
        Style::Glass => Group {
            fill: palette.surface,
            stroke: panel_stroke(palette),
            radius: theme::round(10),
            shadow: shadow(palette),
            // Vista's dark blue main instruction.
            ink: if dark {
                palette.bright
            } else {
                darker(palette.deep, 0.3)
            },
            on_line: false,
            size: 16.0,
            ..none
        },
        Style::Gel => Group {
            fill: if dark {
                Color32::from_white_alpha(8)
            } else {
                Color32::from_black_alpha(12)
            },
            stroke: Stroke::new(
                1.0,
                if dark {
                    Color32::from_white_alpha(18)
                } else {
                    Color32::from_black_alpha(34)
                },
            ),
            radius: theme::round(14),
            on_line: false,
            bold: true,
            ..none
        },
    })
}

/// What a group box draws over its frame once its size is known: the
/// etched lines of Windows 98 and Mac OS.
pub fn paint_group(painter: &Painter, rect: Rect, palette: &Palette) {
    if matches!(style(), Style::Retro | Style::Platinum) {
        etched(painter, rect, palette.canvas);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::settings::{Accent, Density};
    use eframe::egui::{FontId, Shape};

    fn scheme(style: Style) -> theme::Scheme {
        theme::Scheme {
            style,
            accent: style.accent(),
            ..theme::Scheme::default()
        }
    }

    /// What painting `paint` adds with `skin` on screen.
    fn drawn_with(
        skin: theme::Skin,
        palette: &Palette,
        paint: impl FnOnce(&Painter, &Palette),
    ) -> Vec<Shape> {
        // Whatever an empty frame draws comes first.
        let empty = theme::painted(|_| {}).len();
        let mut shapes =
            theme::with_skin(skin, || theme::painted(|ui| paint(ui.painter(), palette)));
        shapes.drain(..empty);
        shapes
    }

    /// What painting `paint` in `style` adds, with the palette it drew in.
    fn drawn(style: Style, dark: bool, paint: impl FnOnce(&Painter, &Palette)) -> Vec<Shape> {
        let skin = theme::Skin {
            style,
            ..theme::Skin::default()
        };
        drawn_with(skin, &Palette::of(scheme(style), dark), paint)
    }

    fn in_style<R>(style: Style, run: impl FnOnce() -> R) -> R {
        let skin = theme::Skin {
            style,
            ..theme::Skin::default()
        };
        theme::with_skin(skin, run)
    }

    fn rect() -> Rect {
        Rect::from_min_size(pos2(10.0, 10.0), vec2(120.0, 36.0))
    }

    fn rounded(shapes: &[Shape]) -> bool {
        shapes.iter().any(|shape| match shape {
            // Soft shadows and glows have no corners to see.
            Shape::Rect(rect) => rect.blur_width == 0.0 && rect.corner_radius != CornerRadius::ZERO,
            _ => false,
        })
    }

    fn meshes(shapes: &[Shape]) -> usize {
        shapes
            .iter()
            .filter(|shape| matches!(shape, Shape::Mesh(_)))
            .count()
    }

    /// Every point drawn is a real number inside `area`, so nothing spills
    /// or turns into NaN at the ends of the settings.
    fn inside(shapes: &[Shape], area: Rect) -> bool {
        let area = area.expand(6.0);
        shapes.iter().all(|shape| {
            let bounds = match shape {
                // Soft shadows and glows fade out past their shape on purpose.
                Shape::Rect(rect) if rect.blur_width > 0.0 => rect.rect,
                _ => shape.visual_bounding_rect(),
            };
            !bounds.is_positive() || (bounds.is_finite() && area.contains_rect(bounds))
        })
    }

    const STILL: State = State {
        hover: 0.0,
        press: 0.0,
        opacity: 1.0,
    };

    /// Paints every piece a style has, checking what each piece returns.
    fn every_piece(painter: &Painter, palette: &Palette, at: &str) {
        let style = style();
        paint_canvas(painter, rect(), palette);
        let ink = paint_sidebar(
            painter,
            rect(),
            rect().shrink(4.0),
            rect().shrink(8.0),
            palette,
        );
        for color in [ink.name, ink.entries].into_iter().flatten() {
            assert!(color.a() > 0, "{at}");
        }
        for sunken in [false, true] {
            paint_panel(painter, rect(), palette, sunken, panel_stroke(palette));
            edge(painter, rect(), palette, sunken);
        }
        for weight in [Weight::Strong, Weight::Normal, Weight::Flat] {
            for state in [
                STILL,
                State {
                    hover: 1.0,
                    press: 1.0,
                    opacity: 0.4,
                },
            ] {
                let ink = paint_button(
                    painter,
                    rect(),
                    weight,
                    palette.accent,
                    Stroke::new(1.0, palette.line),
                    state,
                    palette,
                );
                // A bare flat button keeps the caller's ink.
                let bare = weight == Weight::Flat && state == STILL;
                assert_eq!(
                    ink.is_some(),
                    style != Style::Modern && !bare,
                    "{at} {weight:?}"
                );
                assert!(ink.is_none_or(|ink| ink.a() > 0), "{at}");
            }
        }
        for focus in [0.0, 1.0] {
            paint_field(painter, rect(), theme::round(9), palette, focus);
        }
        for checked in [0.0, 0.5, 1.0] {
            let tick = paint_check(painter, rect(), palette, checked, true, 1.0);
            // Platinum draws its own cross; the rest leave the tick to the
            // caller.
            assert_eq!(tick.is_none(), style == Style::Platinum, "{at}");
        }
        paint_knob(painter, rect().center(), 9.0, 1.0);
        if let Some(ink) = paint_selection(painter, rect(), palette) {
            assert!(ink.a() > 0, "{at}");
        }
        for shown in [0.0, 1.0] {
            paint_row_selection(painter, rect(), palette, shown);
        }
        let strip = paint_title_strip(painter, rect(), palette);
        assert_eq!(strip.is_some(), style != Style::Modern, "{at}");
        if let Some(ink) = strip {
            let galley = painter.layout_no_wrap("Installed".into(), FontId::default(), ink);
            // Inset as the header sets it, so Glass's glow has room.
            paint_title_text(painter, rect().min + vec2(12.0, 6.0), galley, ink, palette);
        }
        let band = paint_column_band(painter, rect(), palette);
        assert_eq!(band, style != Style::Modern, "{at}");
        for (sorted, hovered) in [(false, false), (true, true)] {
            let ink = paint_column_cell(painter, rect(), palette, sorted, hovered);
            // Only Aqua's sorted column turns blue and picks its own ink.
            assert_eq!(ink.is_some(), style == Style::Gel && sorted, "{at}");
        }
        paint_group(painter, rect(), palette);
    }

    #[test]
    fn every_style_draws_every_piece_in_light_and_dark() {
        for style in Style::ALL {
            for dark in [false, true] {
                let at = format!("{style:?} dark={dark}");
                let shapes = drawn(style, dark, |painter, palette| {
                    every_piece(painter, palette, &at)
                });
                assert!(shapes.len() > 20, "{at}");
                assert!(inside(&shapes, rect()), "{at}");
                // Retro is square through and through.
                assert_eq!(rounded(&shapes), style != Style::Retro, "{at}");
            }
        }
    }

    #[test]
    fn every_mix_of_colour_corners_and_room_draws_and_reads() {
        // Picked colours at the ends: bright, near black, near white.
        let custom = [
            None,
            Some(Color32::from_rgb(0xff, 0xe1, 0x4d)),
            Some(Color32::from_rgb(0x20, 0x20, 0x20)),
            Some(Color32::from_rgb(0xf4, 0xf4, 0xf4)),
            Some(Color32::from_rgb(0x00, 0xff, 0x66)),
        ];
        for style in Style::ALL {
            for dark in [false, true] {
                for color in custom {
                    for roundness in [0.0, 1.0, 2.0] {
                        for density in [Density::Compact, Density::Normal, Density::Roomy] {
                            let scheme = theme::Scheme {
                                accent: if color.is_some() {
                                    Accent::Custom
                                } else {
                                    style.accent()
                                },
                                custom_accent: color,
                                roundness,
                                density,
                                ..scheme(style)
                            };
                            let palette = Palette::of(scheme, dark);
                            let skin = theme::Skin {
                                style,
                                roundness,
                                density,
                            };
                            let at = format!(
                                "{style:?} dark={dark} {color:?} round={roundness} {density:?}"
                            );
                            let shapes = drawn_with(skin, &palette, |painter, palette| {
                                every_piece(painter, palette, &at);
                                // Text on a solid selection reads.
                                if let Some(ink) = row_selection_ink(palette) {
                                    assert!(theme::contrast(ink, palette.deep) >= 4.0, "{at}");
                                    let on = on_fill(palette, ink);
                                    assert_eq!((on.ink, on.accent), (ink, ink), "{at}");
                                }
                                // The accent reads on the window and on cards.
                                for under in [palette.surface, palette.canvas] {
                                    assert!(theme::contrast(palette.accent, under) >= 2.5, "{at}");
                                }
                                assert!(
                                    theme::contrast(palette.rail(), palette.canvas) > 1.4,
                                    "{at}"
                                );
                            });
                            assert!(inside(&shapes, rect()), "{at}");
                            // Square corners: Retro always, the rest at 0.
                            if style == Style::Retro {
                                assert!(!rounded(&shapes), "{at}");
                            }
                            if roundness == 0.0 && matches!(style, Style::Glass | Style::Platinum) {
                                assert!(!rounded(&shapes), "{at}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn flat_buttons_stay_bare_until_hovered_in_every_style() {
        for style in Style::ALL {
            let flat = |hover| {
                drawn(style, false, |painter, palette| {
                    paint_button(
                        painter,
                        rect(),
                        Weight::Flat,
                        palette.hover,
                        Stroke::NONE,
                        State { hover, ..STILL },
                        palette,
                    );
                })
            };
            assert!(flat(0.0).is_empty(), "{style:?}");
            assert!(!flat(1.0).is_empty(), "{style:?}");
        }
    }

    #[test]
    fn modern_keeps_its_own_look() {
        let fill = Color32::from_rgb(1, 2, 3);
        let stroke = Stroke::new(1.0, Color32::from_rgb(4, 5, 6));
        for dark in [false, true] {
            let shapes = drawn(Style::Modern, dark, |painter, palette| {
                paint_button(
                    painter,
                    rect(),
                    Weight::Normal,
                    fill,
                    stroke,
                    STILL,
                    palette,
                );
            });
            let [Shape::Rect(button)] = &shapes[..] else {
                panic!("{shapes:?}")
            };
            assert_eq!(
                (
                    button.rect,
                    button.fill,
                    button.stroke,
                    button.corner_radius
                ),
                (rect(), fill, stroke, CornerRadius::same(9))
            );
            // A field gets its focus ring only while focused.
            let field = |focus| {
                drawn(Style::Modern, dark, |painter, palette| {
                    paint_field(painter, rect(), CornerRadius::same(9), palette, focus)
                })
                .len()
            };
            assert_eq!((field(0.0), field(1.0)), (1, 2));
            let canvas = drawn(Style::Modern, dark, |painter, palette| {
                paint_canvas(painter, rect(), palette)
            });
            assert_eq!(meshes(&canvas), 0);
        }
        let palette = Palette::of(scheme(Style::Modern), false);
        in_style(Style::Modern, || {
            assert!(selection_bar());
            assert!(!switch_as_check());
            assert!(group(&palette).is_none());
            assert!(row_selection_ink(&palette).is_none());
            assert_eq!(column_title("NAME"), "NAME");
        });
    }

    #[test]
    fn the_older_styles_bring_their_own_pieces() {
        for style in Style::ALL.into_iter().filter(|s| *s != Style::Modern) {
            let palette = Palette::of(scheme(style), false);
            in_style(style, || {
                assert!(!selection_bar(), "{style:?}");
                assert!(switch_as_check(), "{style:?}");
                assert_eq!(column_title("SUMMARY"), "Summary");
                assert_eq!(column_title(""), "");
                let group = group(&palette).expect("a group box");
                // Group boxes sit their title on the line in Windows 98 and
                // Mac OS 8, and inside the box in the rest.
                assert_eq!(
                    group.on_line,
                    matches!(style, Style::Retro | Style::Platinum | Style::Luna),
                    "{style:?}"
                );
                assert_eq!(
                    row_selection_ink(&palette).is_some(),
                    matches!(style, Style::Retro | Style::Luna | Style::Gel),
                    "{style:?}"
                );
            });
        }
        // Pinstripes and light from above are meshes over the colour.
        let canvas = |style| {
            drawn(style, false, |painter, palette| {
                paint_canvas(painter, rect(), palette)
            })
        };
        assert!(meshes(&canvas(Style::Gel)) >= 1);
        assert!(meshes(&canvas(Style::Glass)) >= 1);
        assert_eq!(meshes(&canvas(Style::Retro)), 0);
        // The old desktops cast no shadows; the glossy ones do.
        for (style, shadowed) in [
            (Style::Retro, false),
            (Style::Luna, false),
            (Style::Platinum, false),
            (Style::Modern, true),
            (Style::Glass, true),
            (Style::Gel, true),
        ] {
            let palette = Palette::of(scheme(style), false);
            let shadow = in_style(style, || shadow(&palette));
            assert_eq!(shadow.color.a() > 0, shadowed, "{style:?}");
        }
    }

    #[test]
    fn gel_turns_its_shine_down_in_the_dark() {
        // Light text has to read on a dark window's clear gel.
        for dark in [false, true] {
            let shapes = drawn(Style::Gel, dark, |painter, palette| {
                let ink = paint_button(
                    painter,
                    rect(),
                    Weight::Normal,
                    palette.surface,
                    Stroke::NONE,
                    STILL,
                    palette,
                )
                .unwrap();
                // The brightest band the clear gel draws.
                let base = if dark {
                    lighter(palette.surface, 0.16)
                } else {
                    gray(0xe6)
                };
                let top = lighter(base, 0.8 * gel_sheen(dark));
                assert!(theme::contrast(ink, top) >= 3.0, "dark={dark}");
            });
            assert!(!shapes.is_empty());
        }
    }

    #[test]
    fn bands_cover_their_rect_and_pick_colours_between_stops() {
        let stops = [(0.0, Color32::BLACK), (1.0, Color32::WHITE)];
        assert_eq!(color_at(&stops, -1.0), Color32::BLACK);
        assert_eq!(color_at(&stops, 2.0), Color32::WHITE);
        assert_eq!(
            color_at(&stops, 0.5),
            lerp(Color32::BLACK, Color32::WHITE, 0.5)
        );
        for radius in [CornerRadius::ZERO, CornerRadius::same(12)] {
            let shapes = drawn(Style::Luna, false, |painter, _| {
                bands(painter, rect(), radius, &stops)
            });
            let bounds = shapes
                .iter()
                .map(Shape::visual_bounding_rect)
                .fold(Rect::NOTHING, |a, b| a.union(b));
            assert!((bounds.top() - rect().top()).abs() < 1.0, "{bounds:?}");
            assert!(
                (bounds.bottom() - rect().bottom()).abs() < 1.0,
                "{bounds:?}"
            );
            assert!((bounds.width() - rect().width()).abs() < 1.0, "{bounds:?}");
        }
        // Empty areas draw nothing rather than a broken shape.
        let none = drawn(Style::Luna, false, |painter, _| {
            gradient(
                painter,
                Rect::NOTHING,
                CornerRadius::ZERO,
                Color32::RED,
                Color32::BLUE,
                false,
            );
            bands(painter, Rect::NOTHING, CornerRadius::ZERO, &stops);
        });
        assert!(none.is_empty());
    }

    #[test]
    fn ink_takes_whichever_of_white_and_black_reads_better() {
        assert_eq!(ink_on(Color32::from_rgb(0, 0, 0x80)), Color32::WHITE);
        assert_eq!(ink_on(Color32::from_rgb(0x7f, 0x9c, 0xff)), Color32::BLACK);
        assert_eq!(ink_on(Color32::WHITE), Color32::BLACK);
        assert_eq!(ink_on(Color32::BLACK), Color32::WHITE);
    }

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
        assert_eq!(lighter(a, 1.0), Color32::WHITE);
        assert_eq!(darker(a, 1.0), Color32::BLACK);
        assert_eq!(faded(a, 0.0).a(), 0);
    }
}
