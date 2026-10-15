//! KDE 4's Oxygen window frame: the title bar is the window's own grey,
//! lit from above, with round slabs for buttons that glow under the
//! pointer, red for close.

use super::frame::{chevron_glyph, window_corners, Caption, Press, WindowState};
use super::skin::{darker, faded, gradient, lighter, oxygen_face, oxygen_glow, oxygen_lit, rgb};
use super::theme::Palette;
use eframe::egui::{pos2, vec2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

/// A button's slab, and the distance between two buttons' centres.
const DIAMETER: f32 = 18.0;
const PITCH: f32 = 21.0;

/// Minimize, maximize and close at the right of `bar`.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    [Caption::Minimize, Caption::Maximize, Caption::Close]
        .into_iter()
        .enumerate()
        .map(|(index, caption)| {
            let x = bar.right() - 4.0 - DIAMETER / 2.0 - (2 - index) as f32 * PITCH;
            let rect = Rect::from_center_size(pos2(x, bar.center().y), vec2(20.0, 20.0));
            (caption, rect)
        })
        .collect()
}

/// The window's lit grey, from the title bar down into the content.
pub fn paint_frame(
    painter: &Painter,
    full: Rect,
    bar: Rect,
    inner: Rect,
    palette: &Palette,
    state: WindowState,
) {
    let corners = window_corners(state.maximized);
    painter.rect_filled(full, corners, palette.canvas);
    let lit = oxygen_lit(palette);
    let band = Rect::from_min_max(full.min, pos2(full.right(), bar.bottom()));
    gradient(
        painter,
        band,
        super::skin::top_corners(corners),
        lighter(lit, if palette.dark { 0.04 } else { 0.3 }),
        lit,
        false,
    );
    // The borders beside the content fade as the content does.
    super::skin::paint_canvas(
        painter,
        Rect::from_x_y_ranges(full.x_range(), inner.y_range()),
        palette,
    );
    if !state.maximized {
        painter.rect_stroke(
            full,
            corners,
            Stroke::new(
                1.0,
                Color32::from_black_alpha(if palette.dark { 160 } else { 60 }),
            ),
            StrokeKind::Inside,
        );
    }
}

/// Close's glow.
const RED: u32 = 0xe2_4c_4c;

/// One button: a lit slab on a soft shadow. Under the pointer it glows,
/// in the accent's light or, for close, red; so does its symbol.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let dark = palette.dark;
    let center = rect.center();
    let slab = Rect::from_center_size(center, vec2(DIAMETER, DIAMETER));
    let round = CornerRadius::same((DIAMETER / 2.0) as u8);
    let base = oxygen_face(palette);
    let base = if press.pressed {
        darker(base, 0.1)
    } else {
        base
    };
    painter.circle_filled(
        center + vec2(0.0, 1.0),
        DIAMETER / 2.0,
        Color32::from_black_alpha(if dark { 70 } else { 30 }),
    );
    gradient(
        painter,
        slab,
        round,
        lighter(base, if dark { 0.15 } else { 0.65 }),
        darker(base, 0.06),
        false,
    );
    let glow = if caption == Caption::Close {
        rgb(RED)
    } else {
        oxygen_glow(palette)
    };
    let lit = press.hovered || press.pressed;
    if lit {
        painter.rect_stroke(slab, round, Stroke::new(1.5, glow), StrokeKind::Inside);
    }
    let ink = if lit {
        darker(glow, 0.2)
    } else if state.active {
        palette.ink
    } else {
        faded(palette.muted, 0.8)
    };
    chevron_glyph(
        painter,
        center,
        caption,
        state.maximized,
        3.5,
        Stroke::new(1.4, ink),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slabs_sit_at_the_right_and_close_glows_red() {
        let bar = Rect::from_min_size(pos2(4.0, 0.0), vec2(600.0, 26.0));
        let buttons = captions(bar);
        assert_eq!(buttons[2].0, Caption::Close);
        assert_eq!(buttons[2].1.center().x, bar.right() - 4.0 - DIAMETER / 2.0);
        assert_eq!(buttons[1].1.center().x, buttons[2].1.center().x - PITCH);
        for (_, rect) in &buttons {
            assert!(bar.contains_rect(*rect));
        }
        let palette = Palette::of(super::super::theme::Scheme::default(), false);
        let reds = super::super::theme::painted(|ui| {
            let press = Press {
                hovered: true,
                ..Press::default()
            };
            let state = WindowState {
                maximized: false,
                active: true,
            };
            paint_caption(
                ui.painter(),
                buttons[2].1,
                Caption::Close,
                press,
                &palette,
                state,
            );
        });
        let red = reds.iter().any(|shape| {
            matches!(shape, eframe::egui::Shape::Rect(slab) if slab.stroke.color == rgb(RED))
        });
        assert!(red, "{reds:?}");
    }
}
