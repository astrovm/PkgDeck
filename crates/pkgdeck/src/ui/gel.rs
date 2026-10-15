//! Aqua's traffic lights, shaded pixel by pixel from columns measured on
//! screenshots of Tiger: a dark rim, a white cap of light at
//! the top, a deep hue that glows brighter toward the bottom.

use super::frame::Caption;
use eframe::egui::{
    pos2, vec2, Color32, ColorImage, Context, Id, Painter, Rect, Stroke, TextureHandle,
    TextureOptions,
};

/// One light's colours: a column through its middle, top to bottom, and
/// the rim at its sides.
struct Light {
    column: [u32; 14],
    side: u32,
}

/// Tiger's lights.
const TIGER: [Light; 3] = [
    Light {
        column: [
            0x131313, 0x999090, 0xe5dede, 0xdfb4b5, 0xc5635e, 0xc34a43, 0xd56056, 0xea7870,
            0xf88d84, 0xfa9d93, 0xfaa89f, 0xf9b5aa, 0xdf9f97, 0x7f4e46,
        ],
        side: 0x591014,
    },
    Light {
        column: [
            0x251113, 0xa39090, 0xe7dede, 0xead2b8, 0xe1a74e, 0xe89e28, 0xf5b13e, 0xfac557,
            0xfdd96a, 0xffec79, 0xfffa84, 0xfdfd91, 0xe6e480, 0x858135,
        ],
        side: 0x7a3013,
    },
    Light {
        column: [
            0x131313, 0x909190, 0xdde1e0, 0xc3d8b8, 0x85b552, 0x75b12d, 0x88c341, 0xa0d85b,
            0xb5eb6f, 0xc4fc7e, 0xd1ff8a, 0xdbfd95, 0xc2e282, 0x668138,
        ],
        side: 0x1b3d12,
    },
];

/// A light in a window that isn't in front: clear grey.
const ASLEEP: Light = Light {
    column: [
        0x646464, 0xa0a0a0, 0xdadada, 0xd2d2d2, 0xbbbbbb, 0xc4c4c4, 0xd2d2d2, 0xdcdcdc, 0xe5e5e5,
        0xe7e7e7, 0xe8e8e8, 0xefefef, 0xdfdfdf, 0xa6a6a6,
    ],
    side: 0x878787,
};

/// How a light shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Lit,
    Pressed,
    Asleep,
}

fn channels(color: u32) -> [f32; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8].map(f32::from)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The light's colour at `(u, v)`, both -1 to 1 across it, or `None`
/// outside it. Fitted to the screenshots.
fn shade(light: &Light, u: f32, v: f32) -> Option<[f32; 3]> {
    let r = u.hypot(v);
    if r > 1.0 {
        return None;
    }
    let column = light.column.map(channels);
    let side = channels(light.side);
    // The hue, deepest just under the cap, brightest near the bottom.
    let row = (v + 1.0) / 2.0 * 14.0 - 0.5;
    let body = if row <= 5.0 {
        column[5]
    } else if row >= 12.0 {
        column[12]
    } else {
        let i = row as usize;
        mix(column[i], column[i + 1], row - i as f32)
    };
    // Darker toward the sides, less so where the bottom glows.
    let edge = smoothstep(0.2, 1.25, r) * (1.0 - 1.55 * v.max(0.0));
    let mut color = mix(body, side, edge.max(0.0));
    // The cap of light at the top.
    let (cy, cw, ch) = (-0.62, 0.6, 0.24);
    let d = (u / cw).hypot((v - cy) / ch);
    if d < 1.0 {
        let t = (v - (cy - ch)) / (2.0 * ch);
        let alpha = (0.95 + (0.65 - 0.95) * t) * (1.0 - smoothstep(0.75, 1.0, d));
        color = mix(color, mix([255.0; 3], body, 0.15), alpha);
    }
    // The rim: black at the top, the hue's dark at the sides, lighter
    // at the bottom.
    let width = 0.24;
    if r > 1.0 - width {
        let rim = if v < 0.0 {
            mix(side, column[0], -v)
        } else {
            mix(side, column[13], v)
        };
        color = mix(color, rim, smoothstep(1.0 - width, 1.0 - width * 0.4, r));
    }
    Some(color)
}

/// The light drawn `size` pixels across, smoothed at its edge.
fn image(light: &Light, size: [usize; 2], dim: f32) -> ColorImage {
    const SAMPLES: usize = 4;
    let [w, h] = size;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0; 3];
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / w as f32 * 2.0 - 1.0;
                    let v = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / h as f32 * 2.0 - 1.0;
                    if let Some(color) = shade(light, u, v) {
                        sum = [0, 1, 2].map(|i| sum[i] + color[i]);
                        hits += 1;
                    }
                }
            }
            let cover = hits as f32 / (SAMPLES * SAMPLES) as f32;
            // Premultiplied: the sum over all samples is already scaled.
            let to = |c: f32| (c * dim / (SAMPLES * SAMPLES) as f32).round() as u8;
            pixels.push(Color32::from_rgba_premultiplied(
                to(sum[0]),
                to(sum[1]),
                to(sum[2]),
                (cover * 255.0).round() as u8,
            ));
        }
    }
    ColorImage::new(size, pixels)
}

fn light(caption: Caption, look: Look) -> &'static Light {
    if look == Look::Asleep {
        return &ASLEEP;
    }
    &TIGER[match caption {
        Caption::Close => 0,
        Caption::Minimize => 1,
        Caption::Maximize => 2,
    }]
}

/// The texture for one light at `size` pixels, made once.
fn texture(ctx: &Context, caption: Caption, look: Look, size: [usize; 2]) -> TextureHandle {
    let id = Id::unique(("pkgdeck-gel-light", caption, look, size));
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
        return texture;
    }
    let dim = if look == Look::Pressed { 0.72 } else { 1.0 };
    let image = image(light(caption, look), size, dim);
    let texture = ctx.load_texture("gel-light", image, TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// One traffic light in `rect`, with its symbol when `marked`.
pub fn paint_light(painter: &Painter, rect: Rect, caption: Caption, look: Look, marked: bool) {
    let scale = painter.ctx().pixels_per_point();
    let size = [
        (rect.width() * scale).round().max(1.0) as usize,
        (rect.height() * scale).round().max(1.0) as usize,
    ];
    let texture = texture(painter.ctx(), caption, look, size);
    painter.image(
        texture.id(),
        rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    if marked && look != Look::Asleep {
        symbol(painter, rect, caption);
    }
}

/// The symbol a light shows under the pointer, in its hue's darkest.
fn symbol(painter: &Painter, rect: Rect, caption: Caption) {
    let ink = match caption {
        Caption::Close => Color32::from_rgb(0x4d, 0x00, 0x00),
        Caption::Minimize => Color32::from_rgb(0x99, 0x57, 0x00),
        Caption::Maximize => Color32::from_rgb(0x00, 0x65, 0x00),
    };
    let stroke = Stroke::new(rect.width() / 11.0, ink);
    let c = rect.center() + vec2(0.0, rect.height() * 0.03);
    let arm = rect.width() * 0.22;
    match caption {
        Caption::Close => {
            let d = arm * 0.85;
            painter.line_segment([c - vec2(d, d), c + vec2(d, d)], stroke);
            painter.line_segment([c - vec2(d, -d), c + vec2(d, -d)], stroke);
        }
        Caption::Minimize => {
            painter.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], stroke);
        }
        Caption::Maximize => {
            painter.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], stroke);
            painter.line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(light: &Light, u: f32, v: f32) -> [u8; 3] {
        shade(light, u, v).unwrap().map(|c| c.round() as u8)
    }

    fn near(a: [u8; 3], b: u32, by: u8) -> bool {
        let b = channels(b).map(|c| c as u8);
        (0..3).all(|i| a[i].abs_diff(b[i]) <= by)
    }

    #[test]
    fn lights_follow_the_screenshots() {
        for lights in [&TIGER] {
            for light in lights {
                // Round, and nothing outside.
                assert!(shade(light, 0.75, 0.75).is_none());
                assert!(shade(light, 0.0, 0.99).is_some());
                // The middle row of the column, the glow near the bottom
                // and the cap of light near the top.
                let middle = (7.5 / 14.0) * 2.0 - 1.0;
                assert!(near(at(light, 0.0, middle), light.column[7], 6));
                let glow = (10.5 / 14.0) * 2.0 - 1.0;
                assert!(near(at(light, 0.0, glow), light.column[10], 12));
                let cap = at(light, 0.0, -0.7);
                let deep = at(light, 0.0, -0.2);
                assert!(
                    cap.map(u32::from).iter().sum::<u32>()
                        > deep.map(u32::from).iter().sum::<u32>()
                );
                // Darker at the rim than inside.
                let rim = at(light, 0.97, 0.0);
                let inside = at(light, 0.0, 0.0);
                assert!(
                    rim.map(u32::from).iter().sum::<u32>()
                        < inside.map(u32::from).iter().sum::<u32>()
                );
            }
        }
    }

    #[test]
    fn images_are_smooth_round_and_dim_when_pressed() {
        let lit = image(&TIGER[0], [28, 28], 1.0);
        let pressed = image(&TIGER[0], [28, 28], 0.72);
        assert_eq!(lit.size, [28, 28]);
        // The corners are clear, the middle solid, the edge part covered.
        assert_eq!(lit.pixels[0].a(), 0);
        assert_eq!(lit.pixels[14 * 28 + 14].a(), 255);
        assert!(lit.pixels.iter().any(|p| (1..255).contains(&p.a())));
        // Premultiplied colour never passes its own cover.
        assert!(lit
            .pixels
            .iter()
            .all(|p| p.r() <= p.a() && p.g() <= p.a() && p.b() <= p.a()));
        let middle = 14 * 28 + 14;
        assert!(pressed.pixels[middle].r() < lit.pixels[middle].r());
        assert_eq!(pressed.pixels[middle].a(), 255);
    }

    #[test]
    fn asleep_lights_are_grey_and_others_take_their_hue() {
        for caption in [Caption::Close, Caption::Minimize, Caption::Maximize] {
            let grey = at(light(caption, Look::Asleep), 0.0, 0.0);
            assert!(grey[0].abs_diff(grey[1]) <= 2 && grey[1].abs_diff(grey[2]) <= 2);
            let hue = at(light(caption, Look::Lit), 0.0, 0.0);
            let spread = hue.iter().max().unwrap() - hue.iter().min().unwrap();
            assert!(spread > 60, "{caption:?} {hue:?}");
        }
        // Red, then yellow, then green.
        let lit = |caption| at(light(caption, Look::Lit), 0.0, 0.0);
        assert!(lit(Caption::Close)[0] > lit(Caption::Close)[1]);
        assert!(lit(Caption::Minimize)[0] > 200 && lit(Caption::Minimize)[1] > 150);
        assert!(lit(Caption::Maximize)[1] > lit(Caption::Maximize)[0]);
    }
}
