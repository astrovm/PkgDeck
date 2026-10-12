//! Windows Vista's and 7's glass frame, measured from screenshots of the
//! real ones at 96 DPI: a dark outer edge and a white inner one, rounded
//! at the top; 8 pixels of glass; a light and a dark line round the
//! window's content; the title on the glass in a white glow; and the
//! caption buttons hung from the top edge.
//!
//! The glass mixes what's behind it the way Windows 7 did: mostly the
//! blurred desktop, its brightness lighting the glass colour, and a little
//! plain colour. Where the desktop blurs behind the window that is the
//! real desktop; elsewhere a painted one stands in for it.

use super::frame::{Caption, Press, WindowState};
use super::shade::{self, Axis};
use super::skin::{lerp, rgb};
use super::theme::{self, Palette};
use eframe::egui::{
    epaint::RectShape, pos2, vec2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind,
};

/// How Windows 7 balances a glass colour of intensity `alpha`, in percent:
/// plain colour, the colour lit by the brightness behind (afterglow), and
/// the blurred desktop itself. From themecpl.dll's own conversion, as
/// OpenGlass documents it.
pub fn balances(alpha: u8) -> (u32, u32, u32) {
    let balance = ((240 * alpha as i32 - 1530) / 459).clamp(0, 100) as u32;
    if balance < 50 {
        let blur = 100 - balance;
        (5, 100 - 5 - blur.min(95), blur.min(95))
    } else if balance >= 95 {
        let color = balance - 25;
        (color, 0, 100 - color)
    } else {
        let afterglow = 95 - balance;
        let blur = 50 - ((balance - 50) >> 1);
        (100 - afterglow - blur, afterglow, blur)
    }
}

/// How bright a desktop the glass is lit for, from 0 to 1.
const LIGHT: f32 = 0.6;

/// The glass `glass` (alpha its intensity) laid over the desktop as a
/// plain blend: a premultiplied colour whose alpha hides that much of
/// what's behind. Windows 7 lit the colour by the brightness behind it; a
/// blend can't, so it lights it as a fairly bright desktop would. Vista
/// blended plainly. Windows behind others are clearer.
pub fn mix(glass: Color32, vista: bool, active: bool) -> Color32 {
    let [r, g, b, a] = glass.to_srgba_unmultiplied();
    let alpha = if active { a } else { (a as f32 * 0.6) as u8 };
    let rgb = [r, g, b].map(|c| c as f32 / 255.0);
    let (cover, color) = if vista {
        let a = alpha as f32 / 255.0;
        (a, rgb.map(|c| c * a))
    } else {
        let (color, afterglow, blur) = balances(alpha);
        let [color, afterglow, blur] = [color, afterglow, blur].map(|p| p as f32 / 100.0);
        (1.0 - blur, rgb.map(|c| (color + afterglow * LIGHT) * c))
    };
    let to = |v: f32| (v.clamp(0.0, cover) * 255.0).round() as u8;
    Color32::from_rgba_premultiplied(to(color[0]), to(color[1]), to(color[2]), to(cover))
}

/// Where the caption buttons hang from the top of `bar`: one group, the
/// close button widest. Each rect is the button's face, without the
/// group's lines.
pub fn captions(vista: bool, bar: Rect) -> Vec<(Caption, Rect)> {
    // The group's right edge, 9 pixels in from the window's; maximized,
    // where the bar is the whole width, 2.
    let right = if bar.left() <= 0.5 {
        bar.right() - 2.0
    } else {
        bar.right() + 1.0
    };
    let top = bar.top() + 1.0;
    let face = |left: f32, width: f32, height: f32| {
        Rect::from_min_size(pos2(left, top), vec2(width, height))
    };
    // Both join them, a line apart; Vista's are a little smaller.
    let ([minimize, maximize, close], height) = if vista {
        ([26.0, 24.0, 41.0], 17.0)
    } else {
        ([27.0, 26.0, 46.0], 18.0)
    };
    let close = face(right - 1.0 - close, close, height);
    let maximize = face(close.left() - 1.0 - maximize, maximize, height);
    let minimize = face(maximize.left() - 1.0 - minimize, minimize, height);
    vec![
        (Caption::Minimize, minimize),
        (Caption::Maximize, maximize),
        (Caption::Close, close),
    ]
}

/// A desktop to see through the glass where the real one can't show: a
/// pale sky, so the glass takes 7's familiar light blue, or a night one.
fn painted_desktop(painter: &Painter, full: Rect, radius: CornerRadius, dark: bool) {
    let (top, bottom) = if dark {
        (rgb(0x1c2c40), rgb(0x0a1018))
    } else {
        (rgb(0xe2edfb), rgb(0xc4d8f2))
    };
    shade::gradient(
        painter,
        full,
        radius,
        Axis::Down,
        &[(0.0, top), (1.0, bottom)],
    );
    shade::gradient(
        painter,
        full,
        radius,
        Axis::Across,
        &[
            (0.0, Color32::from_white_alpha(if dark { 10 } else { 40 })),
            (0.5, Color32::TRANSPARENT),
            (1.0, Color32::from_black_alpha(20)),
        ],
    );
}

/// The desktop's wallpaper, blurred, as a texture once it's loaded.
fn wallpaper_texture(ctx: &eframe::egui::Context) -> Option<eframe::egui::TextureId> {
    let id = eframe::egui::Id::new("pkgdeck-wallpaper");
    if let Some(texture) = ctx.data(|data| data.get_temp::<eframe::egui::TextureHandle>(id)) {
        return Some(texture.id());
    }
    let wallpaper = crate::backdrop::wallpaper(ctx)?;
    let image = eframe::egui::ColorImage::new(
        wallpaper.size,
        wallpaper
            .pixels
            .iter()
            .map(|[r, g, b, _]| Color32::from_rgb(*r, *g, *b))
            .collect(),
    );
    let texture = ctx.load_texture("wallpaper", image, eframe::egui::TextureOptions::LINEAR);
    let texture_id = texture.id();
    ctx.data_mut(|data| data.insert_temp(id, texture));
    Some(texture_id)
}

/// The part of the wallpaper behind the window `full`: where it sits on
/// its screen when the desktop says, else the middle of the screen.
fn wallpaper_uv(ctx: &eframe::egui::Context, full: Rect) -> Rect {
    let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    let monitor = monitor.filter(|m| m.x > 0.0 && m.y > 0.0);
    match (outer, monitor) {
        (Some(outer), Some(monitor)) if outer.min != pos2(0.0, 0.0) => {
            // On a second screen, where it sits on that screen, taking
            // the screens as the same size.
            let at = vec2(
                outer.min.x.rem_euclid(monitor.x),
                outer.min.y.rem_euclid(monitor.y),
            );
            let min = at / monitor;
            let max = (at + full.size()) / monitor;
            Rect::from_min_max(min.to_pos2(), max.to_pos2())
        }
        (_, Some(monitor)) => {
            let size = (full.size() / monitor).min(vec2(1.0, 1.0));
            Rect::from_center_size(pos2(0.5, 0.5), size)
        }
        _ => Rect::from_center_size(pos2(0.5, 0.5), vec2(0.6, 0.6)),
    }
}

/// The frame around `inner`: glass in `full`, rounded at the top unless
/// maximized.
pub fn paint_frame(
    painter: &Painter,
    full: Rect,
    inner: Rect,
    palette: &Palette,
    state: WindowState,
    vista: bool,
) {
    let open = !state.maximized;
    let radius = if open {
        CornerRadius {
            nw: 6,
            ne: 6,
            sw: 0,
            se: 0,
        }
    } else {
        CornerRadius::ZERO
    };
    let glass = mix(palette.glass, vista, state.active);
    if !theme::see_through() {
        match wallpaper_texture(painter.ctx()) {
            Some(texture) => {
                let uv = wallpaper_uv(painter.ctx(), full);
                painter
                    .add(RectShape::filled(full, radius, Color32::WHITE).with_texture(texture, uv));
            }
            None => painted_desktop(painter, full, radius, palette.dark),
        }
    }
    painter.rect_filled(full, radius, glass);
    if open {
        painter.rect_stroke(
            full,
            radius,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(8, 14, 24, 230)),
            StrokeKind::Inside,
        );
        let inside = CornerRadius {
            nw: radius.nw.saturating_sub(1),
            ne: radius.ne.saturating_sub(1),
            ..radius
        };
        painter.rect_stroke(
            full.shrink(1.0),
            inside,
            Stroke::new(
                1.0,
                Color32::from_white_alpha(if state.active { 200 } else { 150 }),
            ),
            StrokeKind::Inside,
        );
    }
    // A light line, then a dark one, round the content; maximized, the
    // screen's edge.
    if !open {
        return;
    }
    painter.rect_stroke(
        inner.expand(2.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, Color32::from_white_alpha(140)),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        inner.expand(1.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, Color32::from_rgba_premultiplied(8, 16, 32, 180)),
        StrokeKind::Inside,
    );
}

/// The white glow the title sits in, so it reads on any glass.
pub fn paint_title_glow(painter: &Painter, text: Rect, dark: bool) {
    let glow = if dark {
        Color32::from_black_alpha(120)
    } else {
        Color32::from_white_alpha(170)
    };
    // A soft cloud, not a pill: narrower than the text, blurred far out.
    painter.add(
        RectShape::filled(text.expand2(vec2(-2.0, -3.0)), CornerRadius::same(8), glow)
            .with_blur_width(18.0),
    );
}

/// The look of one button's face: its colours top to bottom, and the glow
/// rising from its bottom edge.
struct Face {
    stops: Vec<(f32, Color32)>,
    glow: Option<Color32>,
    halo: Option<Color32>,
    edge: Color32,
}

fn face_of(caption: Caption, press: Press, active: bool, vista: bool) -> Face {
    let white = Color32::from_white_alpha;
    let red = caption == Caption::Close && (active || press.hovered);
    match (red, press.pressed, press.hovered) {
        (true, true, _) => Face {
            stops: vec![
                (0.0, rgb(0xd6928a)),
                (0.5, rgb(0xb0503f)),
                (0.5, rgb(0x8f1d08)),
                (1.0, rgb(0xb5482c)),
            ],
            glow: Some(Color32::from_rgba_unmultiplied(0xf4, 0xb0, 0x60, 150)),
            halo: None,
            edge: rgb(0x3a0b05),
        },
        (true, false, true) => Face {
            stops: vec![
                (0.0, rgb(0xfb9d8b)),
                (0.25, rgb(0xee6d56)),
                (0.5, rgb(0xee6d56)),
                (0.5, rgb(0xd42809)),
                (1.0, rgb(0xd8452a)),
            ],
            glow: Some(Color32::from_rgba_unmultiplied(0xf4, 0xe6, 0x76, 200)),
            halo: Some(Color32::from_rgba_unmultiplied(0xe6, 0x8e, 0x75, 170)),
            edge: rgb(0x47100a),
        },
        (true, false, false) if vista => Face {
            stops: vec![
                (0.0, rgb(0xf4d3cc)),
                (0.1, rgb(0xe9a99c)),
                (0.45, rgb(0xde9284)),
                (0.47, rgb(0xb7432c)),
                (0.75, rgb(0xb8432c)),
                (0.95, rgb(0xd58576)),
                (1.0, rgb(0xecc6c0)),
            ],
            glow: None,
            halo: None,
            edge: rgb(0x431422),
        },
        (true, false, false) => Face {
            stops: vec![
                (0.0, rgb(0xe8aaa7)),
                (0.45, rgb(0xd67b74)),
                (0.5, rgb(0xc54335)),
                (0.75, rgb(0xac3e34)),
                (0.95, rgb(0xcf7d6a)),
                (1.0, rgb(0xdfb1aa)),
            ],
            glow: None,
            halo: None,
            edge: rgb(0x391209),
        },
        (false, true, _) => Face {
            stops: vec![
                (0.0, rgb(0x86a7bc)),
                (0.5, rgb(0x86a7bc)),
                (0.5, rgb(0x092747)),
                (1.0, rgb(0x0c3a5c)),
            ],
            glow: Some(Color32::from_rgba_unmultiplied(0x0b, 0xfd, 0xfa, 170)),
            halo: None,
            edge: rgb(0x0b1a2a),
        },
        (false, false, true) => Face {
            stops: vec![
                (0.0, rgb(0xd7ebf7)),
                (0.5, rgb(0xb6d9ee)),
                (0.5, rgb(0x1a6ca1)),
                (1.0, rgb(0x2a85bd)),
            ],
            glow: Some(Color32::from_rgba_unmultiplied(0x2a, 0xce, 0xda, 210)),
            halo: Some(Color32::from_rgba_unmultiplied(0x5d, 0xc4, 0xf0, 150)),
            edge: rgb(0x14304a),
        },
        (false, false, false) => Face {
            // Clear glass, lit above its middle and shaded below.
            stops: vec![
                (0.0, white(140)),
                (0.2, white(130)),
                (0.5, white(55)),
                (0.5, Color32::from_rgba_premultiplied(0, 4, 20, 56)),
                (0.75, Color32::from_rgba_premultiplied(0, 4, 20, 50)),
                (1.0, Color32::from_rgba_premultiplied(5, 12, 25, 40)),
            ],
            glow: None,
            halo: None,
            edge: Color32::from_rgba_premultiplied(10, 18, 30, if active { 190 } else { 130 }),
        },
    }
}

/// One caption button with its symbol. The buttons share a group edge:
/// rounded under the minimize and close buttons, a line between.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    state: WindowState,
    vista: bool,
) {
    let face = face_of(caption, press, state.active, vista);
    let round = match caption {
        Caption::Minimize => CornerRadius {
            sw: 5,
            ..CornerRadius::ZERO
        },
        Caption::Close => CornerRadius {
            se: 5,
            ..CornerRadius::ZERO
        },
        Caption::Maximize => CornerRadius::ZERO,
    };
    // The edge sits round the face, its top line on the window's own.
    let edge = rect.expand(1.0);
    let edge_round = CornerRadius {
        sw: round.sw.saturating_add(u8::from(round.sw > 0)),
        se: round.se.saturating_add(u8::from(round.se > 0)),
        nw: round.nw.saturating_add(u8::from(round.nw > 0)),
        ne: round.ne.saturating_add(u8::from(round.ne > 0)),
    };
    if let Some(halo) = face.halo {
        painter.add(RectShape::filled(rect.expand(2.0), round, halo).with_blur_width(9.0));
    }
    // A white line outside the group, then its dark edge. The group
    // hangs from the window's top line, so neither crosses it.
    let halo = edge.expand(1.0);
    // Only round the group's outside, not between its buttons.
    let (left, right) = match caption {
        Caption::Minimize => (halo.left(), rect.right()),
        Caption::Maximize => (rect.left(), rect.right()),
        Caption::Close => (rect.left(), halo.right()),
    };
    let outside = Rect::from_x_y_ranges(left..=right, halo.y_range());
    painter.with_clip_rect(outside).rect_stroke(
        halo,
        edge_round,
        Stroke::new(
            1.0,
            Color32::from_white_alpha(if state.active { 150 } else { 100 }),
        ),
        StrokeKind::Inside,
    );
    shade::gradient(painter, rect, round, Axis::Down, &face.stops);
    painter.rect_stroke(
        edge,
        edge_round,
        Stroke::new(1.0, face.edge),
        StrokeKind::Inside,
    );
    if let Some(glow) = face.glow {
        // Light pooling up from the bottom edge.
        let clip = painter.with_clip_rect(rect);
        let pool = Rect::from_center_size(
            pos2(rect.center().x, rect.bottom()),
            vec2(rect.width() * 1.1, rect.height() * 1.3),
        );
        let center = pool.center();
        let reach = pool.size() / 2.0;
        shade::lit(&clip, &shade::ellipse(pool), center, 6, &|p| {
            let d = ((p - center) / reach).length().min(1.0);
            let fade = (1.0 - d / 0.65).clamp(0.0, 1.0);
            glow.gamma_multiply(fade)
        });
    }
    if caption == Caption::Close && face.glow.is_none() {
        // Shade at both ends of the red.
        shade::gradient(
            painter,
            rect,
            round,
            Axis::Across,
            &[
                (0.0, Color32::from_black_alpha(46)),
                (0.22, Color32::TRANSPARENT),
                (0.78, Color32::TRANSPARENT),
                (1.0, Color32::from_black_alpha(46)),
            ],
        );
    }
    // A white line just inside the face.
    painter.rect_stroke(
        rect,
        round,
        Stroke::new(
            1.0,
            Color32::from_white_alpha(if state.active { 110 } else { 80 }),
        ),
        StrokeKind::Inside,
    );
    // The window's white top line runs on across the group.
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(
            1.0,
            Color32::from_white_alpha(if state.active { 235 } else { 170 }),
        ),
    );
    symbol(
        painter,
        rect,
        caption,
        state.maximized,
        state.active || press.hovered,
    );
}

/// The button's white symbol with its dark outline, drawn on whole pixels
/// as Windows drew it.
fn symbol(painter: &Painter, rect: Rect, caption: Caption, maximized: bool, active: bool) {
    let outline = if active {
        rgb(0x535767)
    } else {
        lerp(rgb(0x535767), Color32::WHITE, 0.3)
    };
    let white = Color32::from_gray(if active { 250 } else { 235 });
    let c = rect.center();
    let at = |x: f32, y: f32, w: f32, h: f32| {
        Rect::from_min_size(
            pos2((c.x + x).round(), (rect.top() + y).round()),
            vec2(w, h),
        )
    };
    let ring = |outer: Rect, thick: f32| {
        painter.rect_filled(outer, CornerRadius::ZERO, outline);
        painter.rect_filled(outer.shrink(1.0), CornerRadius::ZERO, white);
        let hole = Rect::from_min_max(
            outer.min + vec2(1.0 + thick, 1.0 + thick + 1.0),
            outer.max - vec2(1.0 + thick, 1.0 + thick),
        );
        painter.rect_filled(hole.expand(1.0), CornerRadius::ZERO, outline);
        painter.rect_filled(
            hole,
            CornerRadius::ZERO,
            Color32::from_rgba_premultiplied(40, 80, 120, 90),
        );
    };
    match caption {
        Caption::Minimize => {
            let bar = at(-5.0, 9.0, 10.0, 5.0);
            painter.rect_filled(bar, CornerRadius::ZERO, outline);
            painter.rect_filled(bar.shrink(1.0), CornerRadius::ZERO, white);
        }
        Caption::Maximize if maximized => {
            ring(at(-3.0, 3.0, 9.0, 8.0), 1.0);
            ring(at(-6.0, 6.0, 9.0, 8.0), 1.0);
        }
        Caption::Maximize => ring(at(-6.0, 4.0, 12.0, 10.0), 2.0),
        Caption::Close => {
            let middle = pos2(c.x.round(), (rect.top() + 9.5).round());
            let (dx, dy) = (4.0, 3.5);
            for (from, to) in [
                (middle + vec2(-dx, -dy), middle + vec2(dx, dy)),
                (middle + vec2(-dx, dy), middle + vec2(dx, -dy)),
            ] {
                painter.line_segment([from, to], Stroke::new(5.0, outline));
            }
            for (from, to) in [
                (
                    middle + vec2(-dx + 0.4, -dy + 0.35),
                    middle + vec2(dx - 0.4, dy - 0.35),
                ),
                (
                    middle + vec2(-dx + 0.4, dy - 0.35),
                    middle + vec2(dx - 0.4, -dy + 0.35),
                ),
            ] {
                painter.line_segment([from, to], Stroke::new(3.0, white));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_7_balances_match_its_swatches() {
        // Sky, Windows 7's default: about half the desktop shows through.
        assert_eq!(balances(0x6B), (8, 43, 49));
        // Twilight, nearly opaque colour.
        assert_eq!(balances(0xA8), (56, 11, 33));
        // The faintest and the strongest intensities.
        let (color, afterglow, blur) = balances(26);
        assert_eq!(color + afterglow + blur, 100);
        assert!(blur >= 90, "{blur}");
        let (color, afterglow, blur) = balances(255);
        assert_eq!((color, afterglow, blur), (75, 0, 25));
        for alpha in 0..=255u8 {
            let (color, afterglow, blur) = balances(alpha);
            assert_eq!(color + afterglow + blur, 100, "{alpha}");
        }
    }

    #[test]
    fn glass_shows_the_desktop_and_clears_behind_other_windows() {
        let sky = Color32::from_rgba_unmultiplied(0x74, 0xb8, 0xfc, 0x6b);
        let front = mix(sky, false, true);
        let behind = mix(sky, false, false);
        // About half the desktop shows through, more behind other windows.
        assert!((90..=150).contains(&front.a()), "{front:?}");
        assert!(behind.a() < front.a());
        // Valid premultiplied colours, blue over red.
        for glass in [front, behind] {
            assert!(glass.r() <= glass.a() && glass.g() <= glass.a() && glass.b() <= glass.a());
            assert!(glass.b() > glass.r());
        }
        // Vista blends plainly: Graphite is black smoke.
        let graphite = mix(Color32::from_rgba_unmultiplied(0, 0, 0, 0xa3), true, true);
        assert_eq!(graphite, Color32::from_rgba_premultiplied(0, 0, 0, 0xa3));
    }

    #[test]
    fn both_join_their_buttons_with_the_close_widest() {
        let bar = Rect::from_min_max(pos2(10.0, 0.0), pos2(667.0, 30.0));
        let seven = captions(false, bar);
        let widths: Vec<f32> = seven.iter().map(|(_, r)| r.width()).collect();
        assert_eq!(widths, [27.0, 26.0, 46.0]);
        // One line between neighbours; the group ends 9 pixels from the
        // window's edge, hung under its top line.
        assert_eq!(seven[1].1.left() - seven[0].1.right(), 1.0);
        assert_eq!(seven[2].1.left() - seven[1].1.right(), 1.0);
        assert_eq!(seven[2].1.right(), 667.0);
        assert!(seven
            .iter()
            .all(|(_, r)| r.top() == 1.0 && r.height() == 18.0));
        let vista = captions(true, bar);
        assert!(vista[2].1.width() > vista[0].1.width());
        assert_eq!(vista[1].1.left() - vista[0].1.right(), 1.0);
        assert_eq!(vista[2].1.right(), 667.0);
        // Maximized, the group keeps clear of the screen's edge.
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 30.0));
        assert_eq!(captions(false, full)[2].1.right(), 797.0);
    }
}
