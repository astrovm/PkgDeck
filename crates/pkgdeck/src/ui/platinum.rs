//! Mac OS 9's Platinum frame, measured from screenshots: a black edge
//! and a white one, grey frame, the content sunk in black, six pairs of
//! white and grey lines across the title bar, and 11-pixel boxes shaded
//! from the top left. In the dark, the same greys turned down, as the
//! Hi-Tech appearance did.

use super::frame::{Caption, Press, WindowState};
use eframe::egui::{pos2, vec2, Color32, CornerRadius, Painter, Rect};

/// Mac OS 9's grey `level`, or its dark counterpart.
pub fn tone(level: u8, dark: bool) -> Color32 {
    if dark {
        Color32::from_gray((f32::from(level) * 0.32 + 8.0) as u8)
    } else {
        Color32::from_gray(level)
    }
}

fn px(painter: &Painter, at: Rect, color: Color32) {
    painter.rect_filled(at, CornerRadius::ZERO, color);
}

fn row(painter: &Painter, x: f32, y: f32, w: f32, color: Color32) {
    px(
        painter,
        Rect::from_min_size(pos2(x, y), vec2(w, 1.0)),
        color,
    );
}

fn col(painter: &Painter, x: f32, y: f32, h: f32, color: Color32) {
    px(
        painter,
        Rect::from_min_size(pos2(x, y), vec2(1.0, h)),
        color,
    );
}

/// The frame round `full` and the content `inner`.
pub fn paint_frame(painter: &Painter, full: Rect, inner: Rect, state: WindowState, dark: bool) {
    let t = |level| tone(level, dark);
    let open = !state.maximized;
    // The title bar is a shade darker than the frame when it's in front.
    let face = if state.active { t(0xcc) } else { t(0xdd) };
    px(painter, full, face);
    if open {
        // Black outside, white just in from the top and left.
        let edge = if state.active { t(0x00) } else { t(0x55) };
        painter.rect_stroke(
            full,
            CornerRadius::ZERO,
            eframe::egui::Stroke::new(1.0, edge),
            eframe::egui::StrokeKind::Inside,
        );
        row(
            painter,
            full.left() + 1.0,
            full.top() + 1.0,
            full.width() - 2.0,
            t(0xff),
        );
        col(
            painter,
            full.left() + 1.0,
            full.top() + 1.0,
            full.height() - 2.0,
            t(0xff),
        );
        // And grey just in from the bottom and right.
        row(
            painter,
            full.left() + 2.0,
            full.bottom() - 2.0,
            full.width() - 3.0,
            t(0x99),
        );
        col(
            painter,
            full.right() - 2.0,
            full.top() + 2.0,
            full.height() - 3.0,
            t(0x99),
        );
    }
    // The content sits sunk in: grey above and left, white below and
    // right, then black round it. Filling the screen, it meets its edge.
    if !open {
        return;
    }
    let well = inner.expand(2.0);
    row(
        painter,
        well.left(),
        well.top(),
        well.width() - 1.0,
        t(0x99),
    );
    col(
        painter,
        well.left(),
        well.top(),
        well.height() - 1.0,
        t(0x99),
    );
    row(
        painter,
        well.left() + 1.0,
        well.bottom() - 1.0,
        well.width() - 1.0,
        t(0xff),
    );
    col(
        painter,
        well.right() - 1.0,
        well.top() + 1.0,
        well.height() - 1.0,
        t(0xff),
    );
    let rim = inner.expand(1.0);
    painter.rect_stroke(
        rim,
        CornerRadius::ZERO,
        eframe::egui::Stroke::new(1.0, t(0x00)),
        eframe::egui::StrokeKind::Inside,
    );
}

/// The six pairs of lines across the title bar between `from` and `to`,
/// one white over one grey; the white starts a pixel sooner.
pub fn stripes(painter: &Painter, bar: Rect, from: f32, to: f32, dark: bool) {
    if to - from < 2.0 {
        return;
    }
    for pair in 0..6 {
        let y = bar.top() + 3.0 + pair as f32 * 2.0;
        row(painter, from, y, to - from, tone(0xff, dark));
        row(
            painter,
            from + 1.0,
            y + 1.0,
            to - from - 1.0,
            tone(0x77, dark),
        );
    }
}

/// Where the boxes sit: close at the left, zoom and collapse at the
/// right, 11 pixels square, 4 down from the top of the bar and reaching
/// a pixel into the frame.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    let size = vec2(11.0, 11.0);
    let top = bar.top() + 4.0;
    // Filling the screen, the bar has no frame beside it to reach into.
    let inset = if bar.left() <= 0.5 { 5.0 } else { -1.0 };
    let close = Rect::from_min_size(pos2(bar.left() + inset, top), size);
    let minimize = Rect::from_min_size(pos2(bar.right() - inset - size.x, top), size);
    let maximize = minimize.translate(vec2(-size.x - 5.0, 0.0));
    vec![
        (Caption::Close, close),
        (Caption::Maximize, maximize),
        (Caption::Minimize, minimize),
    ]
}

/// The stretch of the title bar the lines run along on each side of the
/// title, between the boxes.
pub fn stripe_spans(bar: Rect, text: Rect) -> [(f32, f32); 2] {
    let boxes = captions(bar);
    let left = boxes[0].1.right() + 5.0;
    let right = boxes[1].1.left() - 5.0;
    [
        (left, (text.left() - 6.0).max(left)),
        ((text.right() + 6.0).min(right), right),
    ]
}

/// One box: a dark frame in a groove, shaded inside from the top left,
/// with zoom's small window or collapse's bar.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    state: WindowState,
    dark: bool,
) {
    if !state.active {
        return;
    }
    let t = |level| tone(level, dark);
    let (x, y) = (rect.left(), rect.top());
    // The groove: grey above and left, white below and right.
    row(painter, x - 1.0, y - 1.0, 12.0, t(0x88));
    col(painter, x - 1.0, y - 1.0, 12.0, t(0x88));
    row(painter, x, y + 11.0, 12.0, t(0xff));
    col(painter, x + 11.0, y, 12.0, t(0xff));
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        eframe::egui::Stroke::new(1.0, t(0x22)),
        eframe::egui::StrokeKind::Inside,
    );
    // Inside, light from the top left: a white corner, grey right and
    // bottom, and a diagonal ramp, turned round when pressed.
    px(
        painter,
        Rect::from_min_size(pos2(x + 1.0, y + 1.0), vec2(9.0, 9.0)),
        t(0xcc),
    );
    let ramp: [u8; 13] = [
        0x99, 0x99, 0xaa, 0xaa, 0xbb, 0xbb, 0xcc, 0xcc, 0xdd, 0xdd, 0xee, 0xee, 0xff,
    ];
    for j in 0..7 {
        for i in 0..7 {
            let k = if press.pressed { 12 - (i + j) } else { i + j };
            let level = if press.pressed {
                ramp[k].saturating_sub(0x22)
            } else {
                ramp[k]
            };
            px(
                painter,
                Rect::from_min_size(pos2(x + 2.0 + i as f32, y + 2.0 + j as f32), vec2(1.0, 1.0)),
                t(level),
            );
        }
    }
    if !press.pressed {
        px(
            painter,
            Rect::from_min_size(pos2(x + 1.0, y + 1.0), vec2(1.0, 1.0)),
            t(0xff),
        );
        col(painter, x + 9.0, y + 2.0, 7.0, t(0x88));
        row(painter, x + 2.0, y + 9.0, 8.0, t(0x88));
    }
    let ink = t(0x22);
    match caption {
        Caption::Close => {}
        Caption::Maximize => {
            // A small window in the top left corner.
            let small = Rect::from_min_size(pos2(x + 1.0, y + 1.0), vec2(6.0, 6.0));
            painter.rect_stroke(
                small,
                CornerRadius::ZERO,
                eframe::egui::Stroke::new(1.0, ink),
                eframe::egui::StrokeKind::Inside,
            );
        }
        Caption::Minimize => {
            // A bar across the middle.
            row(painter, x + 1.0, y + 4.0, 9.0, ink);
            row(painter, x + 1.0, y + 6.0, 9.0, ink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_sit_where_mac_os_9_put_them() {
        let bar = Rect::from_min_max(pos2(6.0, 1.0), pos2(494.0, 22.0));
        let boxes = captions(bar);
        // Measured: the close box 5 pixels in and down from the window's
        // corner, 11 square.
        assert_eq!(
            boxes[0].1,
            Rect::from_min_size(pos2(5.0, 5.0), vec2(11.0, 11.0))
        );
        assert_eq!(boxes[2].1.right(), 495.0);
        assert!(boxes[1].1.right() < boxes[2].1.left());
        // The lines stop short of the boxes and the title.
        let text = Rect::from_min_max(pos2(200.0, 4.0), pos2(260.0, 18.0));
        let [left, right] = stripe_spans(bar, text);
        assert_eq!(left.0, 21.0);
        assert_eq!(left.1, 194.0);
        assert_eq!(right.0, 266.0);
        assert!(right.1 < boxes[1].1.left());
    }

    #[test]
    fn the_dark_keeps_the_order_of_the_greys() {
        for (a, b) in [(0x00, 0x22), (0x77, 0x99), (0xcc, 0xff)] {
            assert!(tone(a, true).r() < tone(b, true).r());
            assert_eq!(tone(a, false), Color32::from_gray(a));
        }
        assert!(tone(0xff, true).r() < 0x70);
    }
}
