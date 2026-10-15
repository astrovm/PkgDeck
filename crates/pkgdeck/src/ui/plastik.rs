//! KDE 3's Plastik window frame: a title bar in the accent, lit from
//! above and spanning the window, square buttons at its right, and a thin
//! grey border with a dark edge.

use super::frame::{plain_glyph, title_colors, window_corners, Caption, Press, WindowState};
use super::skin::{bands, darker, faded, gradient, ink_on, lighter, top_corners};
use super::theme::Palette;
use eframe::egui::{pos2, vec2, CornerRadius, Painter, Rect, Stroke, StrokeKind};

/// The buttons' side.
const SIZE: f32 = 18.0;

/// Minimize, maximize and close at the right of `bar`, a pixel apart.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    [Caption::Minimize, Caption::Maximize, Caption::Close]
        .into_iter()
        .enumerate()
        .map(|(index, caption)| {
            let right = bar.right() - 2.0 - (2 - index) as f32 * (SIZE + 1.0);
            let rect = Rect::from_min_size(
                pos2(right - SIZE, (bar.center().y - SIZE / 2.0).round()),
                vec2(SIZE, SIZE),
            );
            (caption, rect)
        })
        .collect()
}

/// The border and the title bar, which spans the side borders.
pub fn paint_frame(
    painter: &Painter,
    full: Rect,
    bar: Rect,
    palette: &Palette,
    state: WindowState,
) {
    let corners = window_corners(state.maximized);
    painter.rect_filled(full, corners, palette.canvas);
    let band = Rect::from_min_max(full.min, pos2(full.right(), bar.bottom()));
    let (from, _) = title_colors(palette, state.active);
    bands(
        painter,
        band,
        top_corners(corners),
        &[
            (0.0, lighter(from, 0.3)),
            (0.5, from),
            (1.0, darker(from, 0.08)),
        ],
    );
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        Stroke::new(1.0, darker(from, 0.3)),
    );
    if !state.maximized {
        painter.rect_stroke(
            full,
            corners,
            Stroke::new(
                1.0,
                darker(palette.canvas, if palette.dark { 0.6 } else { 0.5 }),
            ),
            StrokeKind::Inside,
        );
    }
}

/// One button: a frame of light on the bar, brighter under the pointer and
/// darker held, its symbol in the title's colour over a soft shadow.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let (from, _) = title_colors(palette, state.active);
    let face = if press.pressed {
        darker(from, 0.15)
    } else if press.hovered {
        lighter(from, 0.25)
    } else {
        lighter(from, 0.1)
    };
    let radius = CornerRadius::same(2);
    gradient(painter, rect, radius, lighter(face, 0.2), face, false);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, darker(from, 0.3)),
        StrokeKind::Inside,
    );
    let ink = ink_on(from);
    let ink = if state.active { ink } else { faded(ink, 0.7) };
    let center = pos2(rect.center().x.round(), rect.center().y.round());
    let half = 4.0;
    plain_glyph(
        painter,
        center + vec2(1.0, 1.0),
        caption,
        state.maximized,
        half,
        Stroke::new(2.0, faded(darker(from, 0.5), 0.5)),
    );
    plain_glyph(
        painter,
        center,
        caption,
        state.maximized,
        half,
        Stroke::new(2.0, ink),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_sit_square_at_the_right_a_pixel_apart() {
        let bar = Rect::from_min_size(pos2(4.0, 0.0), vec2(600.0, 22.0));
        let buttons = captions(bar);
        assert_eq!(buttons[2].0, Caption::Close);
        assert_eq!(buttons[2].1.right(), bar.right() - 2.0);
        for pair in buttons.windows(2) {
            assert_eq!(pair[1].1.left() - pair[0].1.right(), 1.0);
        }
        for (_, rect) in buttons {
            assert_eq!(rect.size(), vec2(SIZE, SIZE));
            assert!(bar.contains_rect(rect));
        }
    }
}
