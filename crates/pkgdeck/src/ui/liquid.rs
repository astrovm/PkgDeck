//! macOS 27's traffic lights, drawn as Liquid Glass: a coloured lens with
//! a bright rim and a highlight across its top, grey while the window is
//! behind another. Their symbols show when the pointer is over the group.

use super::frame::{Caption, Press, WindowState};
use super::skin::{darker, gray, rgb};
use eframe::egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

/// The light's diameter and the distance between two lights' centres, as
/// measured on a macOS 26 window at 2x.
const DIAMETER: f32 = 14.0;
const PITCH: f32 = 23.0;
/// How far the first light's centre is from the window's left edge. It is
/// as far from the top, so the bar is twice this tall.
const INSET: f32 = 26.0;

/// Close, minimize and zoom at the left of `bar`, level with its middle.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    let y = bar.center().y;
    [Caption::Close, Caption::Minimize, Caption::Maximize]
        .into_iter()
        .enumerate()
        .map(|(index, caption)| {
            let x = bar.left() + INSET + index as f32 * PITCH;
            (
                caption,
                Rect::from_center_size(pos2(x, y), vec2(18.0, 18.0)),
            )
        })
        .collect()
}

/// A light's colour.
fn hue(caption: Caption) -> Color32 {
    match caption {
        Caption::Close => rgb(0xec6765),
        Caption::Minimize => rgb(0xf2ca44),
        Caption::Maximize => rgb(0x65c466),
    }
}

/// One light. `press.group` is whether the pointer is on any of the three.
pub fn paint_light(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    dark: bool,
    state: WindowState,
) {
    let center = rect.center();
    let radius = DIAMETER / 2.0;
    let lit = state.active || press.group;
    let base = if lit {
        hue(caption)
    } else if dark {
        gray(0x59)
    } else {
        gray(0xcf)
    };
    let base = if press.pressed {
        darker(base, 0.2)
    } else {
        base
    };
    // The lens: a darker body with the colour sitting a little high,
    // so the lower rim reads as glass thickness.
    painter.circle_filled(center, radius, darker(base, 0.22));
    painter.circle_filled(center - vec2(0.0, 0.6), radius - 0.9, base);
    // The light collecting along the top.
    let sheen = Rect::from_center_size(center - vec2(0.0, 2.7), vec2(7.2, 3.4));
    painter.rect_filled(
        sheen,
        eframe::egui::CornerRadius::same(2),
        Color32::from_white_alpha(if lit { 105 } else { 70 }),
    );
    painter.circle_stroke(
        center,
        radius - 0.25,
        Stroke::new(
            0.75,
            if dark {
                Color32::from_white_alpha(46)
            } else {
                Color32::from_black_alpha(46)
            },
        ),
    );
    if press.group && state.active {
        symbol(painter, center, caption, state.maximized);
    }
}

/// The symbol on a light, in dark ink.
fn symbol(painter: &Painter, center: Pos2, caption: Caption, maximized: bool) {
    let ink = Color32::from_black_alpha(165);
    let stroke = Stroke::new(1.25, ink);
    match caption {
        Caption::Close => {
            let d = 2.3;
            painter.line_segment([center + vec2(-d, -d), center + vec2(d, d)], stroke);
            painter.line_segment([center + vec2(-d, d), center + vec2(d, -d)], stroke);
        }
        Caption::Minimize => {
            painter.line_segment([center + vec2(-2.7, 0.0), center + vec2(2.7, 0.0)], stroke);
        }
        Caption::Maximize => {
            // Two right triangles, their square corners out to the window's
            // corners; in toward the middle when it already fills the screen.
            let (corner, leg) = if maximized { (0.5, 2.8) } else { (3.0, 3.2) };
            for sign in [1.0_f32, -1.0] {
                let at = center + vec2(sign * corner, -sign * corner);
                painter.add(Shape::convex_polygon(
                    vec![
                        at,
                        at + vec2(-sign * leg, 0.0) * if maximized { -1.0 } else { 1.0 },
                        at + vec2(0.0, sign * leg) * if maximized { -1.0 } else { 1.0 },
                    ],
                    ink,
                    Stroke::NONE,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lights_sit_in_a_row_inside_a_narrow_sidebar() {
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 52.0));
        let lights = captions(bar);
        let order: Vec<_> = lights.iter().map(|(caption, _)| *caption).collect();
        assert_eq!(
            order,
            [Caption::Close, Caption::Minimize, Caption::Maximize]
        );
        // Level with the bar's middle, and the last clear of the 96 pixel
        // rail the sidebar shrinks to.
        for (_, rect) in &lights {
            assert_eq!(rect.center().y, bar.center().y);
        }
        assert!(lights[2].1.right() < 96.0);
        assert!(lights[0].1.right() < lights[1].1.left());
    }
}
