//! Windows 11's window frame: the title bar on Mica, and its three
//! caption buttons flush with the corner, 46 by 32, with one-pixel symbols.

use super::frame::{Caption, Press, WindowState};
use super::skin::{faded, lerp, rgb};
use super::theme::Palette;
use eframe::egui::{pos2, vec2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

/// The caption buttons' width.
const WIDTH: f32 = 46.0;

/// Where each caption button sits in `bar`: minimize, maximize and close
/// side by side, the last in the corner.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    [Caption::Minimize, Caption::Maximize, Caption::Close]
        .into_iter()
        .enumerate()
        .map(|(index, caption)| {
            let left = bar.right() - WIDTH * (3 - index) as f32;
            let rect = Rect::from_min_size(pos2(left, bar.top()), vec2(WIDTH, bar.height()));
            (caption, rect)
        })
        .collect()
}

/// The window's Mica and its edge.
pub fn paint_frame(painter: &Painter, full: Rect, palette: &Palette, state: WindowState) {
    let corners = super::frame::window_corners(state.maximized);
    painter.rect_filled(full, corners, palette.canvas);
    if !state.maximized {
        let edge = if state.active {
            palette.strong_line
        } else {
            palette.line
        };
        painter.rect_stroke(full, corners, Stroke::new(1.0, edge), StrokeKind::Inside);
    }
}

/// One caption button: clear until touched, then a pale square, and red
/// for close.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let red = caption == Caption::Close && (press.hovered || press.pressed);
    if red {
        let fill = rgb(0xc42b1c);
        // The close button sits in the window's corner and turns with it.
        let corner = super::frame::window_corners(state.maximized).ne;
        painter.rect_filled(
            rect,
            CornerRadius {
                ne: corner.saturating_sub(1),
                ..CornerRadius::ZERO
            },
            if press.pressed {
                faded(fill, 0.9)
            } else {
                fill
            },
        );
    } else if press.pressed {
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            faded(palette.ink, if palette.dark { 0.04 } else { 0.03 }),
        );
    } else if press.hovered {
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            faded(palette.ink, if palette.dark { 0.09 } else { 0.06 }),
        );
    }
    let ink = if red {
        Color32::WHITE
    } else if state.active || press.hovered {
        palette.ink
    } else {
        lerp(palette.ink, palette.canvas, 0.5)
    };
    // Snap to a pixel's middle so one-pixel lines stay crisp.
    let center = pos2(rect.center().x.floor() + 0.5, rect.center().y.floor() + 0.5);
    // A box's inside stroke is crisp when its corner is on a whole pixel.
    let whole = center - vec2(0.5, 0.5);
    let stroke = Stroke::new(1.0, ink);
    let half = 5.0;
    match caption {
        Caption::Minimize => {
            painter.hline(center.x - half..=center.x + half, center.y, stroke);
        }
        Caption::Maximize if state.maximized => {
            // Two windows, the front one a corner lower.
            let front =
                Rect::from_min_size(pos2(whole.x - half, whole.y - half + 2.0), vec2(8.0, 8.0));
            painter.rect_stroke(front, CornerRadius::same(1), stroke, StrokeKind::Inside);
            // The back window's visible edges, on the same half pixels.
            let back_box = front.translate(vec2(2.0, -2.0));
            let back = [
                pos2(back_box.left() + 0.5, front.top()),
                pos2(back_box.left() + 0.5, back_box.top() + 0.5),
                pos2(back_box.right() - 0.5, back_box.top() + 0.5),
                pos2(back_box.right() - 0.5, back_box.bottom() - 0.5),
                pos2(front.right(), back_box.bottom() - 0.5),
            ];
            painter.add(eframe::egui::Shape::line(back.to_vec(), stroke));
        }
        Caption::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(whole, vec2(10.0, 10.0)),
                CornerRadius::same(2),
                stroke,
                StrokeKind::Inside,
            );
        }
        Caption::Close => {
            painter.line_segment(
                [center + vec2(-half, -half), center + vec2(half, half)],
                stroke,
            );
            painter.line_segment(
                [center + vec2(-half, half), center + vec2(half, -half)],
                stroke,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_buttons_fill_the_corner_edge_to_edge() {
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 32.0));
        let buttons = captions(bar);
        let order: Vec<_> = buttons.iter().map(|(caption, _)| *caption).collect();
        assert_eq!(
            order,
            [Caption::Minimize, Caption::Maximize, Caption::Close]
        );
        // Close ends at the window's edge, and none leaves a gap.
        assert_eq!(buttons[2].1.right(), 800.0);
        assert_eq!(buttons[0].1.right(), buttons[1].1.left());
        assert_eq!(buttons[1].1.right(), buttons[2].1.left());
        for (_, rect) in buttons {
            assert_eq!(rect.size(), vec2(46.0, 32.0));
        }
    }
}
