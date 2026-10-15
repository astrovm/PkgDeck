//! GNOME's window, as libadwaita draws it: a header bar the sidebar's own
//! colour runs up into, a bold centred title, and only a close button, a
//! round grey one, as GNOME's default layout has.

use super::frame::{fit, plain_glyph, window_corners, Caption, Press, WindowState};
use super::skin::{adwaita_sidebar, faded, top_corners};
use super::theme::Palette;
use eframe::egui::{pos2, vec2, Color32, Id, Painter, Rect, Stroke, StrokeKind};

/// The close button's diameter.
const DIAMETER: f32 = 24.0;

/// Close alone, at the right of `bar`.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    let center = pos2(bar.right() - 7.0 - DIAMETER / 2.0 - 4.0, bar.center().y);
    vec![(
        Caption::Close,
        Rect::from_center_size(center, vec2(DIAMETER, DIAMETER)),
    )]
}

/// The window, its header bar, the sidebar's colour running up beside the
/// title, and a hairline round it all.
pub fn paint_frame(
    painter: &Painter,
    full: Rect,
    bar: Rect,
    palette: &Palette,
    state: WindowState,
) {
    let dark = palette.dark;
    let corners = window_corners(state.maximized);
    painter.rect_filled(full, corners, palette.canvas);
    let header = Rect::from_min_max(full.min, pos2(full.right(), bar.bottom()));
    // Behind another window the header takes the window's colour.
    let fill = match (dark, state.active) {
        (_, false) => palette.canvas,
        (true, true) => palette.tone(super::skin::rgb(0x2e2e32)),
        (false, true) => palette.surface,
    };
    painter.rect_filled(header, top_corners(corners), fill);
    let sidebar = painter.ctx().data(|data| {
        data.get_temp::<f32>(Id::unique("sidebar-extent"))
            .unwrap_or(0.0)
    });
    if sidebar > 0.0 {
        let behind = Rect::from_min_max(
            full.min,
            pos2((bar.left() + sidebar).min(full.right()), bar.bottom()),
        );
        painter.rect_filled(behind, fit(painter.ctx(), behind), adwaita_sidebar(palette));
        painter.vline(
            behind.right() - 0.5,
            behind.y_range(),
            Stroke::new(1.0, Color32::from_black_alpha(if dark { 92 } else { 18 })),
        );
    }
    painter.hline(
        header.x_range(),
        header.bottom() - 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(if dark { 92 } else { 31 })),
    );
    if !state.maximized {
        let outline = if dark {
            Color32::from_white_alpha(18)
        } else {
            Color32::from_black_alpha(40)
        };
        painter.rect_stroke(full, corners, Stroke::new(1.0, outline), StrokeKind::Inside);
    }
}

/// The close button: a grey disc, deeper hovered and held, with a thin
/// cross.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let center = rect.center();
    let wash = 0.1
        + if press.pressed {
            0.2
        } else if press.hovered {
            0.05
        } else {
            0.0
        };
    painter.circle_filled(center, DIAMETER / 2.0, faded(palette.ink, wash));
    let ink = if state.active {
        palette.ink
    } else {
        faded(palette.ink, 0.5)
    };
    plain_glyph(
        painter,
        pos2(center.x.round(), center.y.round()),
        caption,
        state.maximized,
        3.75,
        Stroke::new(1.6, ink),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_offers_close_alone_at_the_right() {
        let bar = Rect::from_min_size(pos2(1.0, 0.0), vec2(600.0, 47.0));
        let buttons = captions(bar);
        assert_eq!(buttons.len(), 1);
        let (caption, rect) = buttons[0];
        assert_eq!(caption, Caption::Close);
        assert_eq!(rect.size(), vec2(DIAMETER, DIAMETER));
        assert!(bar.contains_rect(rect));
        assert!(rect.right() > bar.right() - 16.0);
    }
}
