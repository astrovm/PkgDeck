//! Plasma 6's Breeze window frame: a flat title bar in the header colour,
//! the title centred, and bare symbols at the right that sit on a disc
//! under the pointer, red for close.

use super::frame::{chevron_glyph, window_corners, Caption, Press, WindowState};
use super::skin::{darker, faded, lerp, rgb, top_corners};
use super::theme::Palette;
use eframe::egui::{pos2, vec2, Color32, Painter, Rect, Stroke, StrokeKind};

/// A button's disc, and the distance between two buttons' centres.
const DIAMETER: f32 = 18.0;
const PITCH: f32 = 24.0;

/// Minimize, maximize and close at the right of `bar`.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    [Caption::Minimize, Caption::Maximize, Caption::Close]
        .into_iter()
        .enumerate()
        .map(|(index, caption)| {
            let x = bar.right() - 6.0 - DIAMETER / 2.0 - (2 - index) as f32 * PITCH;
            let rect = Rect::from_center_size(pos2(x, bar.center().y), vec2(22.0, 22.0));
            (caption, rect)
        })
        .collect()
}

/// The title bar's colour: Breeze's header, paler while the window is
/// behind another.
fn header(palette: &Palette, active: bool) -> Color32 {
    let hex = match (palette.dark, active) {
        (false, true) => 0xdee0e2,
        (false, false) => 0xeff0f1,
        (true, true) => 0x292c30,
        (true, false) => 0x202326,
    };
    palette.tone(rgb(hex))
}

/// The window's grey, the header across the top and a hairline round it.
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
    let fill = header(palette, state.active);
    painter.rect_filled(band, top_corners(corners), fill);
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        Stroke::new(1.0, lerp(fill, palette.ink, 0.12)),
    );
    if !state.maximized {
        painter.rect_stroke(
            full,
            corners,
            Stroke::new(
                1.0,
                lerp(
                    palette.canvas,
                    palette.ink,
                    if palette.dark { 0.2 } else { 0.3 },
                ),
            ),
            StrokeKind::Inside,
        );
    }
}

/// Close's red.
const RED: u32 = 0xda4453;

/// One button: a bare symbol on a pale disc under the pointer; close turns
/// red with a white cross.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let center = rect.center();
    let ink = if state.active {
        palette.ink
    } else {
        palette.muted
    };
    let touched = press.hovered || press.pressed;
    let ink = if touched && caption == Caption::Close {
        let red = rgb(RED);
        let red = if press.pressed {
            darker(red, 0.15)
        } else {
            red
        };
        painter.circle_filled(center, DIAMETER / 2.0, red);
        Color32::WHITE
    } else {
        if touched {
            let wash = if press.pressed { 0.3 } else { 0.15 };
            painter.circle_filled(center, DIAMETER / 2.0, faded(palette.ink, wash));
        }
        ink
    };
    chevron_glyph(
        painter,
        pos2(center.x.round(), center.y.round()),
        caption,
        state.maximized,
        4.0,
        Stroke::new(1.3, ink),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_sit_at_the_right_and_the_header_dims_behind() {
        let bar = Rect::from_min_size(pos2(1.0, 0.0), vec2(600.0, 30.0));
        let buttons = captions(bar);
        assert_eq!(buttons[2].0, Caption::Close);
        assert!(buttons[2].1.right() <= bar.right() - 4.0);
        for (_, rect) in &buttons {
            assert!(bar.contains_rect(*rect));
        }
        for dark in [false, true] {
            let palette = Palette::of(super::super::theme::Scheme::default(), dark);
            assert_ne!(header(&palette, true), header(&palette, false));
        }
    }
}
