//! Windows XP's Luna frame, measured from screenshots of Luna Blue: the
//! title bar row by row, the four-line frame and the caption buttons'
//! colours, lit from the top left. Other accents take the same light and
//! shade in their own colour; the dark appearance takes Royale Noir's.

use super::frame::{Caption, Press, WindowState};
use super::skin::{lerp, rgb};
use eframe::egui::{
    pos2, vec2, Color32, ColorImage, CornerRadius, Id, Painter, Rect, Stroke, StrokeKind,
    TextureHandle, TextureOptions,
};

/// Luna Blue's title bar, one colour per row from the window's top.
const TITLE: [u32; 30] = [
    0x0058ee, 0x3f97ff, 0x2b90ff, 0x0372ff, 0x0365f1, 0x005ce9, 0x0058e6, 0x0054e3, 0x0054e3,
    0x0054e3, 0x0055e5, 0x0055e5, 0x0055e5, 0x0055e5, 0x0055e5, 0x0055e5, 0x0055e5, 0x0058ee,
    0x005bf2, 0x005af5, 0x0060f9, 0x0164f9, 0x026afe, 0x026afe, 0x026afe, 0x026afe, 0x0065fd,
    0x0060fc, 0x004de3, 0x0043cf,
];

/// The frame's lines from the outside in.
const FRAME: [u32; 4] = [0x0019cf, 0x0831d9, 0x166aee, 0x0855dd];

/// Royale Noir's near-black title bar: a light line, a blue one, then
/// black-blue deepening down.
const NOIR: [(f32, u32); 5] = [
    (0.0, 0x1d2025),
    (0.04, 0x1c4ba5),
    (0.08, 0x121a29),
    (0.92, 0x080c13),
    (1.0, 0x000000),
];

/// The blue the measured colours are shades of.
const BLUE: u32 = 0x0054e3;

/// Minimize's and maximize's face, sampled at `ROWS` and `COLS` of the
/// 19-pixel face inside the white border.
const FACE: [[u32; 7]; 8] = [
    [
        0x739af8, 0x608cf7, 0x487af5, 0x3e73f5, 0x336cf4, 0x1f5def, 0x3467e0,
    ],
    [
        0x6590f7, 0x9ab7fa, 0x729af8, 0x628ef7, 0x5283f6, 0x376ef2, 0x1546b9,
    ],
    [
        0x4d7ef6, 0x7099f8, 0x5b86f7, 0x4072f5, 0x2d69f4, 0x2e69f2, 0x1d4cbc,
    ],
    [
        0x376ef4, 0x5785f7, 0x5281f6, 0x3b77f5, 0x226df6, 0x216af3, 0x1649be,
    ],
    [
        0x1e5df3, 0x4276f5, 0x427cf6, 0x347cf6, 0x1e77f8, 0x0d66f4, 0x0641c0,
    ],
    [
        0x0e50ed, 0x2863f0, 0x346cf1, 0x2a70f2, 0x136af4, 0x025af0, 0x003ab9,
    ],
    [
        0x285edf, 0x1b55df, 0x265de0, 0x1e5ee2, 0x0e5be5, 0x014ee2, 0x1f4eb8,
    ],
    [
        0x2856c3, 0x0d3fb6, 0x1445b9, 0x1045be, 0x0641c0, 0x003bbe, 0x1f4eb9,
    ],
];

/// Close's red face, sampled the same way.
const CLOSE: [[u32; 7]; 8] = [
    [
        0xeb8b73, 0xe97b60, 0xe56648, 0xe45f3e, 0xe25633, 0xdc451f, 0xd05334,
    ],
    [
        0xe97f65, 0xf1ac9a, 0xeb8b72, 0xe97c62, 0xe66f52, 0xe15937, 0xaa3315,
    ],
    [
        0xe66a4d, 0xeb8a70, 0xe8765c, 0xe45f40, 0xe25430, 0xdf522e, 0xad3a1d,
    ],
    [
        0xe35937, 0xe87257, 0xe66e52, 0xe55f3a, 0xe55022, 0xe24e21, 0xaf3516,
    ],
    [
        0xdf451e, 0xe46242, 0xe66847, 0xe7643b, 0xea541f, 0xe6440d, 0xb12b06,
    ],
    [
        0xd8370e, 0xdd4c28, 0xe05734, 0xe2552a, 0xe54813, 0xe03802, 0xaa2300,
    ],
    [
        0xcd4928, 0xcc3e1b, 0xcf4726, 0xd2451e, 0xd53d0e, 0xd13001, 0xab3c1f,
    ],
    [
        0xb54428, 0xa62b0d, 0xa93214, 0xae3110, 0xb12a06, 0xae2400, 0xac3c1f,
    ],
];

const ROWS: [f32; 8] = [0.0, 2.0, 5.0, 9.0, 13.0, 16.0, 17.0, 18.0];
const COLS: [f32; 7] = [0.0, 2.0, 5.0, 9.0, 13.0, 16.0, 18.0];

fn luma(color: Color32) -> f32 {
    (0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32) / 255.0
}

/// `color`, a shade of Luna's blue, as the same shade of `base`.
pub fn recolor(color: u32, base: Option<Color32>) -> Color32 {
    let color = rgb(color);
    let Some(base) = base else {
        return color;
    };
    let shift = luma(color) - luma(rgb(BLUE));
    if shift >= 0.0 {
        lerp(
            base,
            Color32::WHITE,
            (shift / (1.0 - luma(rgb(BLUE)))).min(1.0),
        )
    } else {
        lerp(base, Color32::BLACK, (-shift / luma(rgb(BLUE))).min(1.0))
    }
}

/// The title bar's colours top to bottom, as gradient stops; each
/// measured row keeps its own band.
fn title_stops(base: Option<Color32>, active: bool, dark: bool) -> Vec<(f32, Color32)> {
    if dark {
        let stops = NOIR.iter().map(|&(t, c)| (t, rgb(c)));
        return if active {
            stops.collect()
        } else {
            stops
                .map(|(t, c)| (t, lerp(c, rgb(0x3a3f48), 0.5)))
                .collect()
        };
    }
    let rows = TITLE.len() as f32;
    TITLE
        .iter()
        .enumerate()
        .flat_map(|(row, &color)| {
            let color = recolor(color, base);
            // Under the title, another accent darkens until white reads
            // on it; Luna's blue already does.
            let mut color = color;
            if base.is_some() && (4..28).contains(&row) {
                for _ in 0..20 {
                    if super::theme::contrast(Color32::WHITE, color) >= 4.5 {
                        break;
                    }
                    color = lerp(color, Color32::BLACK, 0.06);
                }
            }
            // Away from the front, the blue washes out to Luna's pale one.
            let color = if active {
                color
            } else {
                lerp(color, rgb(0xcbc2dc), 0.6)
            };
            [(row as f32 / rows, color), ((row + 1) as f32 / rows, color)]
        })
        .collect()
}

/// The accent to shade with, or `None` for Luna's own blue.
pub fn base(deep: Color32) -> Option<Color32> {
    (deep != rgb(0x2f68d8)).then_some(deep)
}

/// The frame, the title bar and their rounded top corners.
pub fn paint_frame(
    painter: &Painter,
    full: Rect,
    bar: Rect,
    palette_deep: Color32,
    state: WindowState,
    dark: bool,
) {
    let base = base(palette_deep);
    let open = !state.maximized;
    let wash = |color: Color32| {
        if dark {
            lerp(color, rgb(0x0b0f17), 0.85)
        } else if state.active {
            color
        } else {
            lerp(color, rgb(0xcbc2dc), 0.6)
        }
    };
    let radius = if open {
        CornerRadius {
            nw: 8,
            ne: 8,
            ..CornerRadius::ZERO
        }
    } else {
        CornerRadius::ZERO
    };
    if open {
        // Four lines round the sides and bottom, darkest outside.
        for (i, &line) in FRAME.iter().enumerate() {
            let edge = Rect::from_min_max(
                full.min + vec2(i as f32, 0.0),
                full.max - vec2(i as f32, i as f32),
            );
            painter.rect_filled(edge, radius, wash(recolor(line, base)));
        }
    } else {
        painter.rect_filled(full, CornerRadius::ZERO, wash(recolor(FRAME[3], base)));
    }
    let strip = if open {
        Rect::from_x_y_ranges(full.x_range(), bar.y_range())
    } else {
        bar
    };
    super::shade::gradient(
        painter,
        strip,
        radius,
        super::shade::Axis::Down,
        &title_stops(base, state.active, dark),
    );
}

/// Where the three buttons sit: 21 pixels square, 2 apart, 2 in from
/// the frame and 6 down from the top.
pub fn captions(bar: Rect) -> Vec<(Caption, Rect)> {
    let size = vec2(21.0, 21.0);
    let close = Rect::from_min_size(pos2(bar.right() - 2.0 - size.x, bar.top() + 6.0), size);
    let maximize = close.translate(vec2(-size.x - 2.0, 0.0));
    let minimize = maximize.translate(vec2(-size.x - 2.0, 0.0));
    vec![
        (Caption::Minimize, minimize),
        (Caption::Maximize, maximize),
        (Caption::Close, close),
    ]
}

/// How a button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Light {
    Rest,
    Hover,
    Down,
    Away,
}

/// The face's colour `x`, `y` pixels into its 19-pixel square, between
/// the samples.
fn sample(face: &[[u32; 7]; 8], x: f32, y: f32) -> [f32; 3] {
    let find = |marks: &[f32], at: f32| {
        let i = marks
            .windows(2)
            .position(|pair| at <= pair[1])
            .unwrap_or(marks.len() - 2);
        let t = ((at - marks[i]) / (marks[i + 1] - marks[i])).clamp(0.0, 1.0);
        (i, t)
    };
    let (row, ty) = find(&ROWS, y);
    let (col, tx) = find(&COLS, x);
    let c = |r: usize, k: usize| {
        let v = face[r][k];
        [(v >> 16) as u8, (v >> 8) as u8, v as u8].map(f32::from)
    };
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    mix(
        mix(c(row, col), c(row, col + 1), tx),
        mix(c(row + 1, col), c(row + 1, col + 1), tx),
        ty,
    )
}

/// A button's face at `size` pixels for `scale` pixels a point.
fn image(
    close: bool,
    light: Light,
    base: Option<Color32>,
    dark: bool,
    size: [usize; 2],
    scale: f32,
) -> ColorImage {
    let face = if close { &CLOSE } else { &FACE };
    let [w, h] = size;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            // Points across the 19-point face.
            let fx = (x as f32 + 0.5) / scale - 0.5;
            let fy = (y as f32 + 0.5) / scale - 0.5;
            let [r, g, b] = sample(face, fx, fy);
            let mut color = Color32::from_rgb(r as u8, g as u8, b as u8);
            if !close {
                let packed = u32::from_be_bytes([0, color.r(), color.g(), color.b()]);
                color = recolor(packed, base);
                if dark {
                    color = lerp(color, rgb(0x0b1220), 0.55);
                }
            }
            color = match light {
                Light::Rest => color,
                Light::Hover => lerp(color, Color32::WHITE, 0.2),
                Light::Down => lerp(color, Color32::BLACK, 0.22),
                Light::Away => lerp(color, rgb(0xcbc2dc), 0.55),
            };
            pixels.push(color);
        }
    }
    ColorImage::new(size, pixels)
}

/// One caption button: its face, white border and white symbol.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    state: WindowState,
    deep: Color32,
    dark: bool,
) {
    let light = if press.pressed {
        Light::Down
    } else if press.hovered {
        Light::Hover
    } else if !state.active {
        Light::Away
    } else {
        Light::Rest
    };
    let close = caption == Caption::Close;
    let base = base(deep);
    let face = rect.shrink(1.0);
    let scale = painter.ctx().pixels_per_point();
    let size = [
        (face.width() * scale).round().max(1.0) as usize,
        (face.height() * scale).round().max(1.0) as usize,
    ];
    // One slot per face and light, so a colour being dragged replaces its
    // texture rather than piling up new ones. Close is red whatever the
    // accent.
    let (base, dark) = if close { (None, false) } else { (base, dark) };
    let slot = Id::new(("pkgdeck-luna-caption", close, light));
    let key = Id::new((base, dark, size));
    let cached = painter
        .ctx()
        .data(|data| data.get_temp::<(Id, TextureHandle)>(slot))
        .filter(|(held, _)| *held == key);
    let texture = match cached {
        Some((_, texture)) => texture,
        None => {
            let texture = painter.ctx().load_texture(
                "luna-caption",
                image(close, light, base, dark, size, scale),
                TextureOptions::LINEAR,
            );
            painter
                .ctx()
                .data_mut(|data| data.insert_temp(slot, (key, texture.clone())));
            texture
        }
    };
    let round = CornerRadius::same(3);
    painter.add(
        eframe::egui::epaint::RectShape::filled(face, CornerRadius::same(2), Color32::WHITE)
            .with_texture(
                texture.id(),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            ),
    );
    let border = if state.active || press.hovered {
        Color32::WHITE
    } else {
        Color32::from_white_alpha(200)
    };
    painter.rect_stroke(rect, round, Stroke::new(1.0, border), StrokeKind::Inside);
    symbol(painter, rect, caption, state.maximized);
}

/// The white symbols, on whole pixels as XP drew them.
fn symbol(painter: &Painter, rect: Rect, caption: Caption, maximized: bool) {
    let white = Color32::WHITE;
    let at =
        |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(rect.min + vec2(x, y), vec2(w, h));
    let frame = |painter: &Painter, outer: Rect, top: f32| {
        painter.rect_filled(
            Rect::from_min_size(outer.min, vec2(outer.width(), top)),
            CornerRadius::ZERO,
            white,
        );
        painter.rect_stroke(
            outer,
            CornerRadius::ZERO,
            Stroke::new(1.0, white),
            StrokeKind::Inside,
        );
    };
    match caption {
        Caption::Minimize => {
            painter.rect_filled(at(5.0, 13.0, 7.0, 3.0), CornerRadius::ZERO, white);
        }
        Caption::Maximize if maximized => {
            // Two windows, the front one lower left.
            frame(painter, at(8.0, 4.0, 8.0, 8.0), 2.0);
            let front = at(5.0, 8.0, 8.0, 8.0);
            frame(painter, front, 2.0);
        }
        Caption::Maximize => frame(painter, at(5.0, 5.0, 11.0, 11.0), 3.0),
        Caption::Close => {
            let stroke = Stroke::new(2.4, white);
            let (a, b) = (rect.min + vec2(5.6, 5.6), rect.min + vec2(15.4, 15.4));
            painter.line_segment([a, b], stroke);
            painter.line_segment([pos2(a.x, b.y), pos2(b.x, a.y)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_a_colour_keeps_one_texture_per_button() {
        let ctx = eframe::egui::Context::default();
        let textures = || ctx.tex_manager().read().num_allocated();
        let mut counts = Vec::new();
        for hue in 0..40u8 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                let painter = ui.painter().clone();
                let bar = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(800.0, 30.0));
                for (caption, rect) in captions(bar) {
                    paint_caption(
                        &painter,
                        rect,
                        caption,
                        Press::default(),
                        WindowState {
                            maximized: false,
                            active: true,
                        },
                        Color32::from_rgb(hue.wrapping_mul(6), 0x60, 0x30),
                        false,
                    );
                }
            });
            output.textures_delta.clear();
            counts.push(textures());
        }
        // The first frame loads the faces; new colours replace them.
        assert!(
            counts.windows(2).all(|pair| pair[1] == pair[0]),
            "{counts:?}"
        );
    }

    #[test]
    fn blue_keeps_its_measured_colours_and_others_follow_its_shading() {
        assert_eq!(recolor(0x3f97ff, None), rgb(0x3f97ff));
        let green = rgb(0x1d7a46);
        // The base blue becomes the accent; lighter and darker rows stay
        // lighter and darker.
        assert_eq!(recolor(BLUE, Some(green)), green);
        assert!(luma(recolor(0x3f97ff, Some(green))) > luma(green));
        assert!(luma(recolor(0x0043cf, Some(green))) < luma(green));
        assert_eq!(base(rgb(0x2f68d8)), None);
        assert_eq!(base(green), Some(green));
    }

    #[test]
    fn white_titles_read_on_every_accent() {
        use crate::settings::{Accent, Style};
        use crate::ui::theme::{contrast, Palette, Scheme};
        for accent in Accent::ALL {
            let palette = Palette::of(
                Scheme {
                    style: Style::Luna,
                    accent,
                    ..Scheme::default()
                },
                false,
            );
            // The rows the title's letters sit on.
            for (_, color) in &title_stops(base(palette.deep), true, false)[14..44] {
                let ratio = contrast(Color32::WHITE, *color);
                assert!(ratio >= 4.5, "{accent:?} {color:?} {ratio}");
            }
        }
    }

    #[test]
    fn the_title_bar_runs_row_by_row() {
        let stops = title_stops(None, true, false);
        assert_eq!(stops.len(), 60);
        assert_eq!(stops[2], (1.0 / 30.0, rgb(0x3f97ff)));
        assert_eq!(stops.last().unwrap().1, rgb(0x0043cf));
        // Behind, it pales; in the dark, it's Royale Noir's black.
        let away = title_stops(None, false, false);
        assert!(luma(away[14].1) > luma(stops[14].1));
        let noir = title_stops(None, true, true);
        assert!(noir.iter().all(|(_, c)| luma(*c) < 0.35));
    }

    #[test]
    fn buttons_sit_where_xp_put_them() {
        let bar = Rect::from_min_max(pos2(4.0, 0.0), pos2(781.0, 30.0));
        let rects: Vec<Rect> = captions(bar).into_iter().map(|(_, r)| r).collect();
        // Measured: 758..778, 735..755, 712..732 across, 6..26 down.
        assert_eq!(
            rects[2],
            Rect::from_min_size(pos2(758.0, 6.0), vec2(21.0, 21.0))
        );
        assert_eq!(rects[1].left(), 735.0);
        assert_eq!(rects[0].left(), 712.0);
    }

    #[test]
    fn faces_are_lit_from_the_top_left() {
        for close in [false, true] {
            let face = if close { &CLOSE } else { &FACE };
            let sum = |c: [f32; 3]| c.iter().sum::<f32>();
            assert!(sum(sample(face, 3.0, 3.0)) > sum(sample(face, 15.0, 15.0)));
            // On a sample, the sample itself.
            let corner = sample(face, 18.0, 18.0);
            let want = face[7][6];
            assert_eq!(corner[0] as u32, want >> 16);
        }
        let rest = image(false, Light::Rest, None, false, [19, 19], 1.0);
        let hover = image(false, Light::Hover, None, false, [19, 19], 1.0);
        let down = image(false, Light::Down, None, false, [19, 19], 1.0);
        let middle = 9 * 19 + 9;
        let lum = |c: Color32| luma(c);
        assert!(lum(hover.pixels[middle]) > lum(rest.pixels[middle]));
        assert!(lum(down.pixels[middle]) < lum(rest.pixels[middle]));
    }
}
