//! How each style draws the pieces everything is built from, after the
//! desktop it's named for: Windows 98, XP and 7, and Mac OS X's Aqua.
//! Buttons, fields, check boxes, panels, the canvas, the sidebar,
//! selections, title bars, list headings and group boxes. Modern draws
//! them flat. Only egui's own shapes, so a frame costs the same in any
//! style.

use super::shade::{self, Axis};
use super::theme::{self, Palette};
use crate::settings::Family;
use eframe::egui::{
    self, epaint, epaint::Shadow, pos2, vec2, Color32, CornerRadius, FontId, Painter, Pos2, Rect,
    Stroke, StrokeKind,
};

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

/// The desktop the style on screen is a version of.
fn style() -> Family {
    theme::skin().family()
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
pub(super) fn lighter(color: Color32, amount: f32) -> Color32 {
    lerp(color, Color32::WHITE, amount)
}
pub(super) fn darker(color: Color32, amount: f32) -> Color32 {
    lerp(color, Color32::BLACK, amount)
}
pub(super) fn faded(color: Color32, opacity: f32) -> Color32 {
    color.gamma_multiply(opacity.clamp(0.0, 1.0))
}
pub(super) fn gray(level: u8) -> Color32 {
    Color32::from_gray(level)
}
pub(super) fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
pub(super) fn top_corners(radius: CornerRadius) -> CornerRadius {
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
    let axis = if across { Axis::Across } else { Axis::Down };
    shade::gradient(painter, rect, radius, axis, &[(0.0, start), (1.0, end)]);
}

/// A top-to-bottom gradient through several `stops` across `rect`,
/// rounded by `radius`: the many-banded fills of glossy buttons and title
/// bars. Two stops at nearly the same place make a hard edge.
pub fn bands(painter: &Painter, rect: Rect, radius: CornerRadius, stops: &[(f32, Color32)]) {
    shade::gradient(painter, rect, radius, Axis::Down, stops);
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
pub(super) fn retro_edge(painter: &Painter, rect: Rect, face: Color32, sunken: bool, opacity: f32) {
    let shade = |color| faded(color, opacity);
    // A dark face lights its edges less, or they glare.
    let (light, dark) = if theme::luminance(face) < 0.2 {
        ((0.38, 0.18), (0.9, 0.5))
    } else {
        ((0.9, 0.45), (0.85, 0.33))
    };
    let (outer_light, outer_dark) = (shade(lighter(face, light.0)), shade(darker(face, dark.0)));
    let (inner_light, inner_dark) = (shade(lighter(face, light.1)), shade(darker(face, dark.1)));
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
    // A dark face lights its lines less, as its bevels do.
    let (light, dark) = if theme::luminance(face) < 0.2 {
        (0.38, 0.5)
    } else {
        (0.85, 0.33)
    };
    let line = Rect::from_min_max(rect.min, rect.max - vec2(1.0, 1.0));
    painter.rect_stroke(
        line.translate(vec2(1.0, 1.0)),
        CornerRadius::ZERO,
        stroke(lighter(face, light)),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        line,
        CornerRadius::ZERO,
        stroke(darker(face, dark)),
        StrokeKind::Inside,
    );
}

/// The shine over the top half of a gel button: brightest at the top,
/// kept off the rounded ends.
pub(super) fn gloss(painter: &Painter, rect: Rect, strength: f32) {
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
        Family::Modern => {
            painter.rect(
                rect,
                theme::round(9),
                faded(fill, opacity),
                Stroke::new(stroke.width, faded(stroke.color, opacity)),
                StrokeKind::Inside,
            );
            None
        }
        Family::Fluent => {
            let radius = theme::round(9);
            painter.rect(
                rect,
                radius,
                faded(fill, opacity),
                Stroke::new(stroke.width, faded(stroke.color, opacity)),
                StrokeKind::Inside,
            );
            // Controls sit a little proud: the lower edge is the darker.
            if !strong && !dark && weight != Weight::Flat {
                let inset = f32::from(radius.sw).max(2.0);
                painter.hline(
                    rect.left() + inset..=rect.right() - inset,
                    rect.bottom() - 0.5,
                    Stroke::new(1.0, faded(palette.strong_line, opacity)),
                );
            }
            None
        }
        // Liquid Glass: a capsule with a bright edge round its inside.
        Family::Liquid => {
            let radius = pill(rect);
            painter.rect(
                rect,
                radius,
                faded(fill, opacity),
                Stroke::new(stroke.width, faded(stroke.color, opacity)),
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                rect.shrink(1.0),
                radius,
                Stroke::new(
                    1.0,
                    faded(
                        Color32::from_white_alpha(if dark { 26 } else { 120 }),
                        alpha,
                    ),
                ),
                StrokeKind::Inside,
            );
            None
        }
        Family::Retro => {
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
        Family::Luna => {
            let radius = theme::round(9);
            let tone = |hex| palette.tone(rgb(hex));
            let stops = match (pressed, dark) {
                (false, false) => [
                    (0.0, palette.tone(Color32::WHITE)),
                    (0.6, tone(0xf4f3ee)),
                    (0.85, tone(0xebe8de)),
                    (1.0, tone(0xd6d0c5)),
                ],
                (true, false) => [
                    (0.0, tone(0xe3e2da)),
                    (0.6, tone(0xe6e5de)),
                    (0.85, tone(0xeeede7)),
                    (1.0, tone(0xf3f3ef)),
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
        Family::Glass => {
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
                    [gray(0xf2), gray(0xeb), gray(0xdd), gray(0xcf)].map(|c| palette.tone(c)),
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
        Family::Gel => {
            let radius = pill(rect);
            let blue = strong;
            let base = if blue {
                palette.deep
            } else if dark {
                lighter(palette.surface, 0.16)
            } else {
                palette.tone(gray(0xe6))
            };
            let base = darker(lighter(base, hover * 0.06), press * 0.15);
            // Tiger's measured rows are its own grey; a tinted window
            // shades its own.
            let measured = if dark {
                None
            } else if !blue && palette.tint_amount == 0.0 {
                Some(&TIGER_BUTTON[..])
            } else if palette.deep == rgb(0x2f68d8) {
                Some(&TIGER_DEFAULT[..])
            } else {
                None
            };
            // Aqua's controls cast a soft shadow down.
            painter.rect_filled(
                rect.translate(vec2(0.0, 1.0)),
                radius,
                Color32::from_black_alpha((40.0 * alpha) as u8),
            );
            if let Some(rows) = measured {
                // Tiger's own gel, row by row: no shine laid on top, the
                // pale upper half is the light. Pressed, it darkens.
                let stops: Vec<(f32, Color32)> = rows
                    .iter()
                    .map(|&(t, color)| {
                        let color = darker(lighter(rgb(color), hover * 0.06), press * 0.15);
                        (t, faded(color, alpha))
                    })
                    .collect();
                bands(painter, rect, radius, &stops);
            } else {
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
                bands(painter, rect, radius, &fade_all(stops, alpha));
                gloss(
                    painter,
                    rect,
                    alpha * if blue { 1.0 } else { gel_sheen(dark) },
                );
            }
            let line = if measured.is_some() {
                if blue {
                    rgb(0x45457e)
                } else {
                    gray(0x60)
                }
            } else if blue {
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
        // Windows 11's box keeps its edge and grows a bar of accent along
        // the bottom while it's typed in.
        Family::Fluent => {
            painter.rect(
                rect,
                radius,
                palette.surface,
                Stroke::new(1.0, palette.line),
                StrokeKind::Inside,
            );
            if focus > 0.0 {
                let inset = f32::from(radius.sw).max(1.0);
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(rect.left() + inset, rect.bottom() - 2.0),
                        pos2(rect.right() - inset, rect.bottom()),
                    ),
                    theme::round(2),
                    faded(palette.accent, focus),
                );
            }
        }
        Family::Modern | Family::Liquid => {
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
        Family::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
            retro_edge(painter, rect, palette.canvas, true, 1.0);
        }
        Family::Luna => {
            painter.rect(
                rect,
                CornerRadius::ZERO,
                palette.surface,
                Stroke::new(1.0, lerp(luna_line(palette), palette.accent, focus)),
                StrokeKind::Inside,
            );
        }
        Family::Glass => {
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
            // Windows 7's fields are darker along the top.
            painter.hline(
                (rect.left() + 2.0)..=(rect.right() - 2.0),
                rect.top() + 0.5,
                Stroke::new(1.0, darker(line, 0.15)),
            );
        }
        Family::Gel => {
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
    }
}

/// A check box, filled by `checked` (0 to 1). Returns the colour its tick
/// takes.
pub fn paint_check(
    painter: &Painter,
    rect: Rect,
    palette: &Palette,
    checked: f32,
    hovered: bool,
    opacity: f32,
) -> Color32 {
    let checked = checked.clamp(0.0, 1.0);
    let dark = palette.dark;
    match style() {
        Family::Modern | Family::Fluent | Family::Liquid => {
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
            faded(palette.accent_ink, opacity)
        }
        Family::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, faded(palette.surface, opacity));
            retro_edge(painter, rect, palette.canvas, true, opacity);
            faded(palette.ink, opacity)
        }
        Family::Luna => {
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
            faded(if dark { rgb(0x5cd05c) } else { rgb(0x21a121) }, opacity)
        }
        Family::Glass => {
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
            faded(
                if dark {
                    palette.bright
                } else {
                    darker(lerp(palette.deep, gray(0x50), 0.4), 0.25)
                },
                opacity,
            )
        }
        Family::Gel => {
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
            faded(ink_on(lighter(blue, 0.2)), opacity)
        }
    }
}

/// Whether switches draw as check boxes: none of the older desktops had
/// switches.
pub fn switch_as_check() -> bool {
    !style().flat()
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

/// A slider in `rect`: its track along `track`, a mark at each of `stops`
/// and the thumb at `x`, `hover` lifting it.
pub fn paint_steps(
    painter: &Painter,
    track: Rect,
    stops: &[f32],
    x: f32,
    hover: f32,
    palette: &Palette,
    opacity: f32,
) {
    let shade = |color| faded(color, opacity);
    let y = track.center().y;
    let face = palette.canvas;
    // Windows and the old Mac mark the steps with ticks under the track.
    let ticks = |color: Color32| {
        for &at in stops {
            painter.vline(
                at.round() + 0.5,
                (y + 11.0)..=(y + 14.0),
                Stroke::new(1.0, shade(color)),
            );
        }
    };
    // A thumb that points down at the ticks.
    let pointed = |width: f32, height: f32| {
        let (left, right) = ((x - width / 2.0).round(), (x + width / 2.0).round());
        let (top, bottom) = (y - height / 2.0 - 1.0, y + height / 2.0 - 1.0);
        let tip = width / 2.0;
        [
            pos2(left, top),
            pos2(right, top),
            pos2(right, bottom - tip),
            pos2((left + right) / 2.0, bottom),
            pos2(left, bottom - tip),
        ]
    };
    let outline = |points: &[Pos2], light: Color32, dark: Color32| {
        let stroke = |color| Stroke::new(1.0, shade(color));
        painter.line_segment([points[4], points[0]], stroke(light));
        painter.line_segment([points[0], points[1]], stroke(light));
        painter.line_segment([points[1], points[2]], stroke(dark));
        painter.line_segment([points[2], points[3]], stroke(dark));
        painter.line_segment([points[3], points[4]], stroke(light));
    };
    let groove = Rect::from_center_size(track.center(), vec2(track.width() + 8.0, 4.0));
    match style() {
        Family::Modern | Family::Fluent | Family::Liquid => {
            painter.rect_filled(track, theme::round(2), shade(palette.rail()));
            painter.rect_filled(
                Rect::from_min_max(track.min, pos2(x, track.max.y)),
                theme::round(2),
                shade(palette.accent),
            );
            for &at in stops {
                let color = if at <= x {
                    palette.accent
                } else {
                    palette.rail()
                };
                painter.circle_filled(pos2(at, y), 3.0, shade(color));
            }
            painter.circle_filled(pos2(x, y), 9.0 + hover * 1.5, shade(palette.accent));
            painter.circle_filled(pos2(x, y), 4.0, shade(palette.surface));
        }
        Family::Retro => {
            // A sunken slot, and a grey thumb raised out of it.
            painter.rect_filled(groove, CornerRadius::ZERO, shade(face));
            retro_edge(painter, groove, face, true, opacity);
            ticks(palette.ink);
            let thumb = pointed(11.0, 20.0);
            painter.add(egui::Shape::convex_polygon(
                thumb.to_vec(),
                shade(face),
                Stroke::NONE,
            ));
            outline(&thumb, lighter(face, 0.9), darker(face, 0.85));
            let inner = [
                thumb[0] + vec2(1.0, 1.0),
                thumb[1] + vec2(-1.0, 1.0),
                thumb[2] + vec2(-1.0, 0.0),
                thumb[3] + vec2(0.0, -1.0),
                thumb[4] + vec2(1.0, 0.0),
            ];
            outline(&inner, lighter(face, 0.45), darker(face, 0.33));
        }
        Family::Luna | Family::Glass => {
            // A thin sunken line; XP's thumb is white with a green foot
            // that turns orange under the pointer, 7's a grey gradient
            // that turns blue.
            let line = Rect::from_center_size(track.center(), vec2(track.width() + 8.0, 3.0));
            painter.rect_filled(line, CornerRadius::ZERO, shade(lighter(face, 0.6)));
            bevel(
                painter,
                line,
                shade(darker(face, 0.3)),
                shade(lighter(face, 0.9)),
            );
            ticks(darker(face, 0.45));
            let luna = style() == Family::Luna;
            let rect = Rect::from_center_size(pos2(x.round(), y - 1.0), vec2(11.0, 20.0));
            let (top, bottom, border) = if luna {
                (rgb(0xffffff), rgb(0xe6e6e0), rgb(0x76889f))
            } else if hover > 0.5 {
                (rgb(0xe3f4fc), rgb(0xa9daf5), rgb(0x3c7fb1))
            } else {
                (rgb(0xf2f2f2), rgb(0xcfcfcf), rgb(0x707070))
            };
            let radius = CornerRadius::same(2);
            gradient(painter, rect, radius, shade(top), shade(bottom), false);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, shade(border)),
                StrokeKind::Inside,
            );
            if luna {
                let foot = if hover > 0.5 {
                    rgb(0xf8b636)
                } else {
                    rgb(0x21a121)
                };
                let foot_rect = Rect::from_min_max(
                    pos2(rect.left() + 1.0, rect.bottom() - 4.0),
                    pos2(rect.right() - 1.0, rect.bottom() - 1.0),
                );
                gradient(
                    painter,
                    foot_rect,
                    CornerRadius::ZERO,
                    shade(lighter(foot, 0.3)),
                    shade(foot),
                    false,
                );
            }
        }
        Family::Gel => {
            // A rounded groove and a blue gel bead on it.
            let groove = Rect::from_center_size(track.center(), vec2(track.width() + 8.0, 6.0));
            let round = CornerRadius::same(3);
            gradient(
                painter,
                groove,
                round,
                shade(darker(face, 0.25)),
                shade(lighter(face, 0.4)),
                false,
            );
            painter.rect_stroke(
                groove,
                round,
                Stroke::new(1.0, shade(darker(face, 0.4))),
                StrokeKind::Inside,
            );
            let bead = Rect::from_center_size(pos2(x, y), vec2(18.0, 18.0) + vec2(hover, hover));
            let blue = palette.accent;
            let round = CornerRadius::same(10);
            painter.add(
                epaint::RectShape::filled(
                    bead.translate(vec2(0.0, 1.0)),
                    round,
                    shade(Color32::from_black_alpha(60)),
                )
                .with_blur_width(3.0),
            );
            gradient(
                painter,
                bead,
                round,
                shade(darker(blue, 0.2)),
                shade(lighter(blue, 0.55)),
                false,
            );
            gloss(painter, bead, 0.9 * opacity * gel_sheen(palette.dark));
            painter.rect_stroke(
                bead,
                round,
                Stroke::new(1.0, shade(darker(blue, 0.45))),
                StrokeKind::Inside,
            );
        }
    }
}

/// The shadow a panel casts, if the style has one.
pub fn shadow(palette: &Palette) -> Shadow {
    let (offset, blur, alpha) = match style() {
        Family::Modern => (1, 6, if palette.dark { 40 } else { 10 }),
        // Windows 11's cards lie flat, held by their edge.
        Family::Fluent => (0, 0, 0),
        // macOS 27 lifts its groups a little more.
        Family::Liquid => (2, 14, if palette.dark { 60 } else { 18 }),
        // Explorer's panes lie flat on its white.
        Family::Glass if palette.dark => (2, 10, 80),
        Family::Gel => (3, 12, if palette.dark { 90 } else { 40 }),
        Family::Glass | Family::Retro | Family::Luna => (0, 0, 0),
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
        Family::Modern | Family::Fluent | Family::Liquid => palette.line,
        Family::Retro => darker(palette.canvas, 0.6),
        Family::Luna => luna_line(palette),
        Family::Glass if palette.dark => lighter(palette.canvas, 0.22),
        // A list view's frame.
        Family::Glass => rgb(0xc0c1cd),
        Family::Gel => lerp(palette.ink, palette.surface, 0.62),
    };
    Stroke::new(1.0, color)
}

/// How round a panel is: lists sit square in the older styles.
fn panel_radius(sunken: bool) -> CornerRadius {
    match style() {
        Family::Retro => CornerRadius::ZERO,
        Family::Luna if sunken => CornerRadius::ZERO,
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
        Family::Retro => retro_edge(painter, rect, palette.canvas, sunken, 1.0),
        Family::Modern
        | Family::Fluent
        | Family::Liquid
        | Family::Luna
        | Family::Glass
        | Family::Gel => {}
    }
}

/// Behind everything: the canvas colour, or Media Center's glow in grey.
pub fn paint_canvas(painter: &Painter, rect: Rect, palette: &Palette) {
    painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
    match style() {
        // Explorer's white, or in the dark a soft glow over near black.
        Family::Glass if palette.dark => {
            gradient(
                painter,
                rect,
                CornerRadius::ZERO,
                palette.tone(rgb(0x26292d)),
                palette.canvas,
                false,
            );
        }
        _ => {}
    }
}

/// macOS 27's sidebar: the window's grey a shade deeper, where its glass
/// would show the desktop.
pub fn sidebar_color(palette: &Palette) -> Color32 {
    lerp(
        palette.canvas,
        palette.ink,
        if palette.dark { 0.06 } else { 0.04 },
    )
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
        Family::Modern => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
            end(palette.line);
        }
        // Windows 11's navigation sits on the window's Mica.
        Family::Fluent => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
        }
        Family::Liquid => {
            painter.rect_filled(rect, CornerRadius::ZERO, sidebar_color(palette));
            end(palette.line);
        }
        Family::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
            // The pages in a list box, as Explorer's folders.
            let list = entries.expand2(vec2(6.0, 8.0));
            painter.rect_filled(list, CornerRadius::ZERO, palette.surface);
            retro_edge(painter, list, palette.canvas, true, 1.0);
            // A dark face lights its edge less, as its bevels do.
            let (light, shade) = if palette.dark { (0.3, 0.7) } else { (0.9, 0.4) };
            bevel(
                painter,
                Rect::from_min_max(pos2(rect.right() - 2.0, rect.top()), rect.max),
                lighter(palette.canvas, light),
                darker(palette.canvas, shade),
            );
        }
        Family::Luna => {
            // The blue task pane, with the pages in a task box.
            let (top, bottom) = if dark {
                // Near black like the page; the blue stays on the frame.
                (palette.surface, palette.canvas)
            } else {
                // Measured on XP's Explorer, in the accent's shade.
                let base = super::luna::base(palette.deep);
                (
                    super::luna::recolor(0x7ba2e7, base),
                    super::luna::recolor(0x6375d6, base),
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
                let base = super::luna::base(palette.deep);
                (
                    Color32::WHITE,
                    super::luna::recolor(0xc6d3f7, base),
                    super::luna::recolor(0xd6dff7, base),
                )
            };
            // The header fades across, white to pale blue.
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
                super::luna::recolor(0x215dc6, super::luna::base(palette.deep))
            };
            return SidebarInk {
                name: Some(link),
                entries: Some(link),
            };
        }
        Family::Glass if dark => {
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(60));
            end(Color32::from_white_alpha(24));
        }
        // Explorer's navigation pane and its pale blue splitter.
        Family::Glass => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.tone(rgb(0xfcfcfc)));
            end(palette.tone(rgb(0xd6e5f5)));
        }
        Family::Gel => {
            // The blue-grey source list; in the dark, a shade under the
            // window's grey.
            let list = if dark {
                darker(palette.canvas, 0.18)
            } else {
                lerp(gray(0xdc), palette.deep, 0.07)
            };
            painter.rect_filled(rect, CornerRadius::ZERO, list);
            end(if dark { Color32::BLACK } else { gray(0x9a) });
        }
    }
    SidebarInk::default()
}

/// The marker under the chosen sidebar entry. Returns the colour text and
/// icons on it take when the style fills it solid.
pub fn paint_selection(painter: &Painter, rect: Rect, palette: &Palette) -> Option<Color32> {
    match style() {
        Family::Modern => {
            painter.rect_filled(rect, theme::round(10), palette.selection);
            None
        }
        // Windows 11's entry is a pale square beside a short accent bar.
        Family::Fluent => {
            painter.rect_filled(
                rect,
                theme::round(10),
                faded(palette.ink, if palette.dark { 0.09 } else { 0.06 }),
            );
            None
        }
        // macOS: the accent fills the row and the text turns white.
        Family::Liquid => {
            let fill = palette.deep;
            painter.rect_filled(rect, theme::round(8), fill);
            painter.rect_stroke(
                rect.shrink(0.5),
                theme::round(8),
                Stroke::new(
                    1.0,
                    Color32::from_white_alpha(if palette.dark { 36 } else { 70 }),
                ),
                StrokeKind::Inside,
            );
            Some(ink_on(fill))
        }
        Family::Retro | Family::Luna => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.deep);
            Some(ink_on(palette.deep))
        }
        Family::Glass => {
            glass_selection(painter, rect, palette, 1.0);
            None
        }
        Family::Gel => {
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

/// The background of list row `index`, where the style stripes them, as
/// Aqua's lists did in pale blue.
pub fn row_stripe(palette: &Palette, index: usize) -> Option<Color32> {
    (style() == Family::Gel && index % 2 == 1).then(|| {
        if palette.dark {
            lighter(palette.surface, 0.05)
        } else if palette.tint_amount > 0.0 {
            // Another accent stripes in its own pale shade.
            lerp(Color32::WHITE, palette.tint, 0.08)
        } else {
            rgb(0xedf3fe)
        }
    })
}

/// A list row under the pointer, `hover` from 0 to 1: Modern's soft tint,
/// Windows 7's pale blue box; the older lists didn't mark it.
pub fn paint_row_hover(painter: &Painter, rect: Rect, palette: &Palette, hover: f32) {
    if hover <= 0.0 {
        return;
    }
    match style() {
        Family::Modern | Family::Fluent | Family::Liquid => {
            painter.rect_filled(rect, theme::round(10), faded(palette.ink, hover * 0.045));
        }
        Family::Glass => {
            let radius = theme::round(10);
            let (top, bottom, line) = if palette.dark {
                (
                    lerp(palette.surface, palette.bright, 0.14),
                    lerp(palette.surface, palette.bright, 0.08),
                    lerp(palette.surface, palette.bright, 0.35),
                )
            } else {
                (rgb(0xfafbfd), rgb(0xebf3fd), rgb(0xb8d6fb))
            };
            gradient(
                painter,
                rect,
                radius,
                faded(top, hover),
                faded(bottom, hover),
                false,
            );
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, faded(line, hover)),
                StrokeKind::Inside,
            );
        }
        Family::Retro | Family::Luna | Family::Gel => {}
    }
}

/// Whether the style marks the chosen sidebar entry with Modern's bar.
pub fn selection_bar() -> bool {
    matches!(style(), Family::Modern | Family::Fluent)
}

/// Whether app icons sit bare, as on the old desktops, rather than on tiles.
pub fn bare_icons() -> bool {
    !style().flat()
}

/// The colour a selected list row's text takes, when the style fills the
/// selection solid.
pub fn row_selection_ink(palette: &Palette) -> Option<Color32> {
    match style() {
        Family::Retro | Family::Luna | Family::Gel => Some(ink_on(palette.deep)),
        Family::Modern | Family::Fluent | Family::Liquid | Family::Glass => None,
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
        // Red, green and amber would vanish into the fill too.
        danger: ink,
        success: ink,
        warning: ink,
        ..*palette
    }
}

/// The selected list row's background, `shown` from 0 to 1.
pub fn paint_row_selection(painter: &Painter, rect: Rect, palette: &Palette, shown: f32) {
    match style() {
        Family::Modern | Family::Fluent | Family::Liquid => {
            painter.rect_filled(rect, theme::round(10), faded(palette.selection, shown));
        }
        Family::Retro | Family::Luna | Family::Gel => {
            painter.rect_filled(rect, CornerRadius::ZERO, faded(palette.deep, shown));
        }
        Family::Glass => glass_selection(painter, rect, palette, shown),
    }
}

/// A window's title bar across `rect`, as the page's heading sits in.
/// Returns the colour its text takes, or none for Modern, which has none.
pub fn paint_title_strip(painter: &Painter, rect: Rect, palette: &Palette) -> Option<Color32> {
    let dark = palette.dark;
    match style() {
        Family::Modern | Family::Fluent | Family::Liquid => None,
        Family::Retro => {
            // Windows 98's navy fading to a lighter blue.
            let end = lerp(palette.deep, palette.bright, 0.85);
            gradient(painter, rect, CornerRadius::ZERO, palette.deep, end, true);
            Some(ink_on(palette.deep))
        }
        Family::Luna => {
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
        // Explorer's command bar.
        Family::Glass if !dark => {
            bands(
                painter,
                rect,
                CornerRadius::ZERO,
                &[
                    (0.0, rgb(0xfafcfd)),
                    (0.5, rgb(0xe6f0fa)),
                    (0.5, rgb(0xdce6f4)),
                    (1.0, rgb(0xdce7f6)),
                ],
            );
            painter.hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                Stroke::new(1.0, rgb(0xa0afc3)),
            );
            Some(palette.ink)
        }
        Family::Glass => Some(palette.ink),
        Family::Gel => {
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
    }
}

/// The page's heading: plain ink in every style and accent.
pub fn heading(palette: &Palette) -> (FontId, Color32) {
    match style() {
        Family::Modern | Family::Fluent | Family::Liquid => (theme::bold(26.0), palette.ink),
        Family::Glass => (theme::font(24.0), palette.ink),
        Family::Luna | Family::Retro | Family::Gel => (theme::bold(22.0), palette.ink),
    }
}

/// A list's column headings as the style draws them: in title case in
/// the older styles, capitals in the flat ones.
pub fn column_title(title: &str) -> String {
    if style().flat() {
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
        Family::Modern | Family::Fluent | Family::Liquid => return false,
        Family::Retro => {
            painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
        }
        Family::Luna => {
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
        Family::Glass => {
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
        Family::Gel => {
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

/// Tiger's white gel button, measured row by row: pale above a hard
/// split, brightening to white below.
const TIGER_BUTTON: [(f32, u32); 12] = [
    (0.0, 0xf9f9f9),
    (0.08, 0xf2f2f2),
    (0.16, 0xeeeeee),
    (0.21, 0xececec),
    (0.42, 0xededed),
    (0.42, 0xdadada),
    (0.47, 0xdfdfdf),
    (0.58, 0xe7e7e7),
    (0.68, 0xf0f0f0),
    (0.79, 0xf9f9f9),
    (0.84, 0xffffff),
    (1.0, 0xffffff),
];

/// Tiger's blue default button, measured the same way.
const TIGER_DEFAULT: [(f32, u32); 11] = [
    (0.0, 0xc7ceea),
    (0.16, 0xa1bee5),
    (0.26, 0x89b6e3),
    (0.42, 0x82b4e7),
    (0.42, 0x569de6),
    (0.53, 0x67aef4),
    (0.63, 0x74bafc),
    (0.74, 0x85caff),
    (0.84, 0x89d2ff),
    (0.9, 0x8ddaff),
    (1.0, 0x89dffc),
];

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
        Family::Modern | Family::Fluent | Family::Liquid => None,
        Family::Retro => {
            retro_edge(painter, cell, palette.canvas, false, 1.0);
            None
        }
        Family::Luna => {
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
        Family::Glass => {
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
        Family::Gel => {
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
        Family::Modern | Family::Fluent | Family::Liquid => return None,
        // Etched after the content, in `paint_group`.
        Family::Retro => none,
        Family::Luna => Group {
            stroke: Stroke::new(1.0, darker(palette.canvas, 0.14)),
            radius: theme::round(8),
            ..none
        },
        Family::Glass => Group {
            fill: palette.surface,
            stroke: panel_stroke(palette),
            radius: theme::round(10),
            shadow: shadow(palette),
            on_line: false,
            size: 16.0,
            ..none
        },
        Family::Gel => Group {
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
    if matches!(style(), Family::Retro) {
        etched(painter, rect, palette.canvas);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Style;

    use crate::settings::{Accent, Density};
    use crate::ui::shade::color_at;
    use eframe::egui::Shape;

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
        // Glows reach 6 points out, and soft edges half a pixel more.
        let area = area.expand(6.5);
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
                assert_eq!(ink.is_some(), !style.flat() && !bare, "{at} {weight:?}");
                assert!(ink.is_none_or(|ink| ink.a() > 0), "{at}");
            }
        }
        for focus in [0.0, 1.0] {
            paint_field(painter, rect(), theme::round(9), palette, focus);
        }
        for checked in [0.0, 0.5, 1.0] {
            let tick = paint_check(painter, rect(), palette, checked, true, 1.0);
            assert!(tick.a() > 0, "{at}");
        }
        paint_knob(painter, rect().center(), 9.0, 1.0);
        if let Some(ink) = paint_selection(painter, rect(), palette) {
            assert!(ink.a() > 0, "{at}");
        }
        for shown in [0.0, 1.0] {
            paint_row_selection(painter, rect(), palette, shown);
        }
        let strip = paint_title_strip(painter, rect(), palette);
        assert_eq!(strip.is_some(), !style.flat(), "{at}");
        assert!(heading(palette).1.a() > 0, "{at}");
        let band = paint_column_band(painter, rect(), palette);
        assert_eq!(band, !style.flat(), "{at}");
        for (sorted, hovered) in [(false, false), (true, true)] {
            let ink = paint_column_cell(painter, rect(), palette, sorted, hovered);
            // Only Aqua's sorted column turns blue and picks its own ink.
            assert_eq!(ink.is_some(), style == Family::Gel && sorted, "{at}");
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
                assert!(shapes.len() > 10, "{at}");
                assert!(inside(&shapes, rect()), "{at}");
                // Retro is square through and through.
                assert_eq!(rounded(&shapes), style.family() != Family::Retro, "{at}");
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
                                    assert_eq!((on.danger, on.warning), (ink, ink), "{at}");
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
                            if roundness == 0.0 && matches!(style, Style::Glass) {
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
            assert!(!bare_icons());
            assert!(!switch_as_check());
            assert!(group(&palette).is_none());
            assert!(row_selection_ink(&palette).is_none());
            assert_eq!(column_title("NAME"), "NAME");
        });
    }

    #[test]
    fn the_older_styles_bring_their_own_pieces() {
        for style in Style::ALL.into_iter().filter(|s| !s.family().flat()) {
            let palette = Palette::of(scheme(style), false);
            in_style(style, || {
                assert!(!selection_bar(), "{style:?}");
                assert!(bare_icons(), "{style:?}");
                assert!(switch_as_check(), "{style:?}");
                assert_eq!(column_title("SUMMARY"), "Summary");
                assert_eq!(column_title(""), "");
                let group = group(&palette).expect("a group box");
                // Group boxes sit their title on the line in Windows 98 and
                // XP, and inside the box in the rest.
                assert_eq!(
                    group.on_line,
                    matches!(style.family(), Family::Retro | Family::Luna),
                    "{style:?}"
                );
                assert_eq!(
                    row_selection_ink(&palette).is_some(),
                    matches!(style.family(), Family::Retro | Family::Luna | Family::Gel),
                    "{style:?}"
                );
            });
        }
        // Windows 11 and macOS 27 keep Modern's layout: tiles, switches, no
        // group boxes; only Windows 11 marks the selection with a bar.
        for style in [Style::Fluent, Style::Liquid] {
            let palette = Palette::of(scheme(style), false);
            in_style(style, || {
                assert_eq!(selection_bar(), style == Style::Fluent, "{style:?}");
                assert!(!bare_icons(), "{style:?}");
                assert!(!switch_as_check(), "{style:?}");
                assert_eq!(column_title("SUMMARY"), "SUMMARY", "{style:?}");
                assert!(group(&palette).is_none(), "{style:?}");
                assert!(row_selection_ink(&palette).is_none(), "{style:?}");
            });
        }
        // The canvas is plain colour; only glass in the dark fades.
        let canvas = |style| {
            drawn(style, false, |painter, palette| {
                paint_canvas(painter, rect(), palette)
            })
        };
        assert_eq!(meshes(&canvas(Style::Aqua)), 0);
        assert_eq!(meshes(&canvas(Style::Retro)), 0);
        // Explorer is plain white; its dark versions fade.
        assert_eq!(meshes(&canvas(Style::Glass)), 0);
        let dark = |style| {
            drawn(style, true, |painter, palette| {
                paint_canvas(painter, rect(), palette)
            })
        };
        assert!(meshes(&dark(Style::Glass)) >= 1);
        // The old desktops cast no shadows; the glossy ones do.
        for (style, shadowed) in [
            (Style::Retro, false),
            (Style::Luna, false),
            (Style::Modern, true),
            (Style::Glass, false),
            (Style::Aqua, true),
        ] {
            let palette = Palette::of(scheme(style), false);
            let shadow = in_style(style, || shadow(&palette));
            assert_eq!(shadow.color.a() > 0, shadowed, "{style:?}");
        }
    }

    #[test]
    fn tiger_buttons_wear_the_measured_gel() {
        let colours = |shapes: &[Shape]| {
            let mut colours = std::collections::BTreeSet::new();
            for shape in shapes {
                if let Shape::Mesh(mesh) = shape {
                    colours.extend(mesh.vertices.iter().map(|v| v.color.to_array()));
                }
            }
            colours
        };
        let button = |dark, weight, state| {
            colours(&drawn(Style::Aqua, dark, |painter, palette| {
                paint_button(
                    painter,
                    rect(),
                    weight,
                    palette.accent,
                    Stroke::new(1.0, palette.line),
                    state,
                    palette,
                );
            }))
        };
        // The white button's split row and the blue one's, as measured.
        let plain = button(false, Weight::Normal, STILL);
        assert!(plain.contains(&rgb(0xdadada).to_array()), "{plain:?}");
        let blue = button(false, Weight::Strong, STILL);
        assert!(blue.contains(&rgb(0x569de6).to_array()), "{blue:?}");
        // Hovered or pressed, it shifts off the measured rows.
        for state in [
            State {
                hover: 1.0,
                ..STILL
            },
            State {
                press: 1.0,
                ..STILL
            },
        ] {
            assert!(!button(false, Weight::Normal, state).contains(&rgb(0xdadada).to_array()));
        }
        // The dark version has no Tiger to measure; it keeps its own gel.
        assert!(!button(true, Weight::Normal, STILL).contains(&rgb(0xdadada).to_array()));
    }

    #[test]
    fn gel_turns_its_shine_down_in_the_dark() {
        // Light text has to read on a dark window's clear gel.
        for dark in [false, true] {
            let shapes = drawn(Style::Aqua, dark, |painter, palette| {
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
            // Rounded fills fade half a pixel past their edge.
            assert!((bounds.top() - rect().top()).abs() <= 0.5, "{bounds:?}");
            assert!(
                (bounds.bottom() - rect().bottom()).abs() <= 0.5,
                "{bounds:?}"
            );
            assert!((bounds.width() - rect().width()).abs() <= 1.0, "{bounds:?}");
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
