//! macOS 27's traffic lights, as macOS 26 draws them: flat coloured discs
//! with a deeper rim, grey while the window is behind another. Their
//! symbols show when the pointer is over the group.

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

/// How wide the deeper rim around a light's colour is.
const RIM: f32 = 0.5;

/// The rim around a light's colour.
fn rim_hue(caption: Caption) -> Color32 {
    match caption {
        Caption::Close => rgb(0xe63835),
        Caption::Minimize => rgb(0xeebb0b),
        Caption::Maximize => rgb(0x34b237),
    }
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
    // A flat disc: the colour inside a rim one shade deeper, and a faint
    // ring just outside it that keeps the light apart from the bar.
    let rim = if !lit {
        darker(base, 0.08)
    } else if press.pressed {
        darker(rim_hue(caption), 0.2)
    } else {
        rim_hue(caption)
    };
    painter.circle_filled(center, radius, rim);
    painter.circle_filled(center, radius - RIM, base);
    painter.circle_stroke(
        center,
        radius + 0.25,
        Stroke::new(
            0.5,
            if dark {
                Color32::from_white_alpha(36)
            } else {
                Color32::from_black_alpha(26)
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

    /// The circles one light paints, in order.
    fn circles(caption: Caption, press: Press, dark: bool, active: bool) -> Vec<(f32, Color32)> {
        let rect = Rect::from_center_size(pos2(26.0, 26.0), vec2(18.0, 18.0));
        let state = WindowState {
            maximized: false,
            active,
        };
        super::super::theme::painted(|ui| {
            paint_light(ui.painter(), rect, caption, press, dark, state);
        })
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::Circle(circle) => Some((circle.radius, circle.fill)),
            _ => None,
        })
        .collect()
    }

    #[test]
    fn a_light_is_a_flat_disc_in_a_deeper_rim() {
        let rest = Press::default();
        for caption in [Caption::Close, Caption::Minimize, Caption::Maximize] {
            let discs = circles(caption, rest, false, true);
            // The rim, then the colour inside it, then a bare ring around
            // both: no lens, no sheen.
            assert_eq!(
                discs[..2],
                [(7.0, rim_hue(caption)), (6.5, hue(caption))],
                "{caption:?}"
            );
            assert_eq!(discs.len(), 3);
            assert_eq!(discs[2].0, 7.25);
        }
        // Behind another window the lights go grey, rim and all.
        for (dark, grey) in [(false, 0xcf), (true, 0x59)] {
            let discs = circles(Caption::Close, rest, dark, false);
            assert_eq!(discs[1].1, gray(grey));
            assert_eq!(discs[0].1, darker(gray(grey), 0.08));
        }
        // Pressed, both shades deepen.
        let pressed = Press {
            pressed: true,
            group: true,
            ..rest
        };
        let discs = circles(Caption::Close, pressed, false, true);
        assert_eq!(discs[0].1, darker(rim_hue(Caption::Close), 0.2));
        assert_eq!(discs[1].1, darker(hue(Caption::Close), 0.2));
    }
}
