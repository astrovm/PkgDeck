//! GNOME 2's Clearlooks window frame, after its Metacity theme: a glossy
//! title bar in the selection colour, a white title outlined in its deep
//! shade, and glossy square buttons. Behind another window it all goes
//! grey.

use super::frame::{plain_glyph, window_corners, Caption, Press, WindowState};
use super::skin::{bands, gtk_shade, lighter, top_corners};
use super::theme::Palette;
use eframe::egui::{pos2, vec2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

/// The buttons' side.
const SIZE: f32 = 18.0;

/// Minimize, maximize and close at the right of `bar`, two pixels apart.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    [Caption::Minimize, Caption::Maximize, Caption::Close]
        .into_iter()
        .enumerate()
        .map(|(index, caption)| {
            let right = bar.right() - 1.0 - (2 - index) as f32 * (SIZE + 2.0);
            let rect = Rect::from_min_size(
                pos2(right - SIZE, (bar.center().y - SIZE / 2.0).round()),
                vec2(SIZE, SIZE),
            );
            (caption, rect)
        })
        .collect()
}

/// The selection colour the title bar is made of. Clearlooks' is a pale
/// blue, so the accent is lightened toward it.
pub(super) fn base(palette: &Palette) -> Color32 {
    lighter(palette.bar, 0.3)
}

/// The gloss Metacity's Clearlooks draws: a lighter upper half over a
/// slightly darker lower one.
fn gloss(color: Color32, up: (f32, f32), down: (f32, f32)) -> [(f32, Color32); 4] {
    [
        (0.0, gtk_shade(color, up.0)),
        (0.5, gtk_shade(color, up.1)),
        (0.5, gtk_shade(color, down.0)),
        (1.0, gtk_shade(color, down.1)),
    ]
}

/// The border and the glossy title bar, which spans the side borders.
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
    let (stops, line, edge) = if state.active {
        let color = base(palette);
        (
            gloss(color, (1.08, 1.02), (1.0, 0.94)),
            gtk_shade(color, 0.7),
            gtk_shade(color, 0.55),
        )
    } else {
        let color = palette.canvas;
        (
            gloss(color, (0.99, 0.95), (0.93, 0.89)),
            gtk_shade(color, 0.65),
            gtk_shade(color, 0.55),
        )
    };
    bands(painter, band, top_corners(corners), &stops);
    // A highlight along the top, kept off the rounded corners.
    painter.hline(
        band.x_range().shrink(f32::from(corners.nw)),
        band.top() + 1.5,
        Stroke::new(1.0, Color32::from_white_alpha(70)),
    );
    painter.hline(band.x_range(), band.bottom() - 0.5, Stroke::new(1.0, line));
    if !state.maximized {
        painter.rect_stroke(
            full,
            corners,
            Stroke::new(1.0, gtk_shade(palette.canvas, 0.45)),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            band,
            top_corners(corners),
            Stroke::new(1.0, edge),
            StrokeKind::Inside,
        );
    }
}

/// One button: glossy like the bar, lighter under the pointer and darker
/// held, its symbol bold in the bar's deep shade.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let color = if state.active {
        base(palette)
    } else {
        palette.canvas
    };
    let face = if press.pressed {
        gtk_shade(color, 0.85)
    } else if press.hovered {
        gtk_shade(color, 1.1)
    } else {
        color
    };
    let radius = CornerRadius::same(3);
    bands(
        painter,
        rect,
        radius,
        &gloss(face, (1.1, 1.02), (1.0, 0.92)),
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, gtk_shade(color, 0.6)),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        radius,
        Stroke::new(1.0, Color32::from_white_alpha(60)),
        StrokeKind::Inside,
    );
    let ink = if state.active {
        gtk_shade(color, 0.5)
    } else {
        super::skin::lerp(palette.ink, palette.canvas, 0.45)
    };
    plain_glyph(
        painter,
        pos2(rect.center().x.round(), rect.center().y.round()),
        caption,
        state.maximized,
        3.5,
        Stroke::new(2.5, ink),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Accent;

    #[test]
    fn the_buttons_sit_square_at_the_right_two_pixels_apart() {
        let bar = Rect::from_min_size(pos2(4.0, 0.0), vec2(600.0, 24.0));
        let buttons = captions(bar);
        assert_eq!(buttons[2].0, Caption::Close);
        for pair in buttons.windows(2) {
            assert_eq!(pair[1].1.left() - pair[0].1.right(), 2.0);
        }
        for (_, rect) in buttons {
            assert_eq!(rect.size(), vec2(SIZE, SIZE));
            assert!(bar.contains_rect(rect));
        }
    }

    #[test]
    fn the_bar_is_a_paler_accent_and_its_gloss_lightens_the_top() {
        let scheme = super::super::theme::Scheme {
            accent: Accent::Green,
            ..super::super::theme::Scheme::default()
        };
        let palette = Palette::of(scheme, false);
        let color = base(&palette);
        assert_eq!(color, lighter(palette.deep, 0.3));
        let stops = gloss(color, (1.08, 1.02), (1.0, 0.94));
        assert!(
            super::super::theme::luminance(stops[0].1) > super::super::theme::luminance(stops[3].1)
        );
        assert_eq!(stops[2].1, color);
    }
}
