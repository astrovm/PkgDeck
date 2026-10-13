//! Scroll areas whose bars are drawn the way the desktop a style is a
//! version of draws them: Windows 98's dithered track between raised
//! buttons, XP's blue thumb with its grip, 7's glass, Aqua's gel capsule
//! with both arrows at one end. Modern keeps egui's own bar, floating over
//! the content.
//!
//! The bar is ours, not egui's: the area hides its own, the content is
//! narrowed by the bar's width, and the bar is drawn and driven here. egui
//! still does the wheel, the keys and dragging the content.

use super::{
    shade::{self, Axis},
    skin::{darker, faded, gray, lerp, lighter, retro_edge, rgb},
    theme::{self, Palette},
};
use crate::settings::Family;
use eframe::egui::{
    self, pos2, scroll_area::ScrollAreaOutput, vec2, Color32, CornerRadius, Id, Painter, Rect,
    Sense, Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
};
use std::hash::Hash;

/// How far an arrow scrolls on a click, and how fast it scrolls while held.
const STEP: f32 = 40.0;
const SPEED: f32 = 640.0;
/// How long a button is held before it starts repeating.
const REPEAT_AFTER: f64 = 0.4;

/// A scroll area along one axis, drawn in the style on screen.
pub struct Scroll {
    vertical: bool,
    /// What tells this area from its neighbours.
    salt: Id,
    auto_shrink: [bool; 2],
    max_height: Option<f32>,
    offset: Option<f32>,
}

pub fn vertical() -> Scroll {
    Scroll::new(true)
}

pub fn horizontal() -> Scroll {
    Scroll::new(false)
}

/// What the bar looked like last frame, which this frame's clicks and
/// reserved room go by.
#[derive(Clone, Copy)]
struct Last {
    inner: Rect,
    content: f32,
    offset: f32,
}

impl Last {
    fn needed(&self, vertical: bool) -> bool {
        self.content > span(vertical, self.inner).len().ceil()
    }
}

impl Scroll {
    fn new(vertical: bool) -> Self {
        Self {
            vertical,
            salt: Id::new("scroll_area"),
            auto_shrink: [true; 2],
            max_height: None,
            offset: None,
        }
    }

    pub fn id_salt(mut self, salt: impl Hash + std::fmt::Debug) -> Self {
        self.salt = Id::new(salt);
        self
    }

    pub fn auto_shrink(mut self, shrink: [bool; 2]) -> Self {
        self.auto_shrink = shrink;
        self
    }

    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = Some(height);
        self
    }

    /// Scrolls to `offset` this frame.
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R> {
        self.show_viewport(ui, |ui, _| add(ui))
    }

    pub fn show_viewport<R>(
        self,
        ui: &mut Ui,
        add: impl FnOnce(&mut Ui, Rect) -> R,
    ) -> ScrollAreaOutput<R> {
        let skin = theme::skin();
        let ctx = ui.ctx().clone();
        let palette = Palette::current(&ctx);
        let Some((thick, _)) = theme::scroll_bar(skin.style, &palette) else {
            let offset = self.offset;
            return self.area(offset).show_viewport(ui, add);
        };
        let family = skin.family();
        let (vertical, max_height) = (self.vertical, self.max_height);
        let key = ui.make_persistent_id(self.salt).with("skinned-bar");
        let last: Option<Last> = ctx.data(|d| d.get_temp(key));
        let reserved = last.is_some_and(|l| l.needed(vertical));

        // Last frame's bar answers this frame's clicks, so the content moves
        // the frame they happen.
        let mut offset = self.offset;
        let mut touch = Touch::default();
        if let Some(last) = last.filter(|_| reserved) {
            let parts = Parts::of(family, vertical, last, thick, ctx.pixels_per_point());
            (touch, offset) = drive(ui, key, vertical, &parts, last, offset);
        }

        let available = ui.available_rect_before_wrap();
        let mut area = self
            .area(offset)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        if reserved {
            area = if vertical {
                area.max_width(available.width() - thick)
            } else {
                area.max_height(
                    max_height
                        .unwrap_or(f32::INFINITY)
                        .min(available.height() - thick),
                )
            };
        }
        let out = area.show_viewport(ui, add);

        let now = Last {
            inner: out.inner_rect,
            content: if vertical {
                out.content_size.y
            } else {
                out.content_size.x
            },
            offset: if vertical {
                out.state.offset.y
            } else {
                out.state.offset.x
            },
        };
        ctx.data_mut(|d| d.insert_temp(key, now));
        let needed = now.needed(vertical);
        if needed != reserved {
            // The room for the bar changes next frame.
            ctx.request_repaint();
        }
        if needed && reserved {
            let parts = Parts::of(family, vertical, now, thick, ctx.pixels_per_point());
            ui.expand_to_include_rect(parts.strip);
            paint(
                &ui.painter().clone(),
                &ctx,
                &palette,
                family,
                vertical,
                &parts,
                touch,
            );
        }
        out
    }

    fn area(self, offset: Option<f32>) -> egui::ScrollArea {
        let mut area = if self.vertical {
            egui::ScrollArea::vertical()
        } else {
            egui::ScrollArea::horizontal()
        }
        .id_salt(self.salt)
        .auto_shrink(self.auto_shrink);
        if let Some(height) = self.max_height {
            area = area.max_height(height);
        }
        if let Some(offset) = offset {
            area = if self.vertical {
                area.vertical_scroll_offset(offset)
            } else {
                area.horizontal_scroll_offset(offset)
            };
        }
        area
    }
}

/// A stretch along an axis.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    start: f32,
    end: f32,
}

impl Span {
    fn len(self) -> f32 {
        self.end - self.start
    }
}

/// `rect`'s extent along the bar's direction of travel.
fn span(vertical: bool, rect: Rect) -> Span {
    if vertical {
        Span {
            start: rect.top(),
            end: rect.bottom(),
        }
    } else {
        Span {
            start: rect.left(),
            end: rect.right(),
        }
    }
}

/// The strip `along` of `strip`'s length, across all of its width.
fn slice(vertical: bool, strip: Rect, along: Span) -> Rect {
    if vertical {
        Rect::from_min_max(
            pos2(strip.left(), along.start),
            pos2(strip.right(), along.end),
        )
    } else {
        Rect::from_min_max(
            pos2(along.start, strip.top()),
            pos2(along.end, strip.bottom()),
        )
    }
}

/// Where each piece of a bar sits.
#[derive(Clone, Copy, Debug)]
struct Parts {
    strip: Rect,
    /// The arrow towards the start, and towards the end.
    back: Rect,
    forward: Rect,
    track: Rect,
    thumb: Rect,
    /// How far the content scrolls.
    max: f32,
    /// How much of the track the thumb may travel.
    travel: f32,
    scrolled: f32,
}

impl Parts {
    fn of(family: Family, vertical: bool, at: Last, thick: f32, ppp: f32) -> Self {
        // On whole pixels, so edges stay crisp and the dither lines up.
        let snap = |v: f32| (v * ppp).round() / ppp;
        let inner = Rect::from_min_max(
            pos2(snap(at.inner.left()), snap(at.inner.top())),
            pos2(snap(at.inner.right()), snap(at.inner.bottom())),
        );
        let strip = if vertical {
            Rect::from_min_size(inner.right_top(), vec2(thick, inner.height()))
        } else {
            Rect::from_min_size(inner.left_bottom(), vec2(inner.width(), thick))
        };
        let whole = span(vertical, strip);
        let button = thick.min((whole.len() - 4.0).max(0.0) / 2.0);
        let (back, forward, track) = if family == Family::Gel {
            // Aqua keeps both arrows together at the end.
            (
                Span {
                    start: whole.end - 2.0 * button,
                    end: whole.end - button,
                },
                Span {
                    start: whole.end - button,
                    end: whole.end,
                },
                Span {
                    start: whole.start,
                    end: whole.end - 2.0 * button,
                },
            )
        } else {
            (
                Span {
                    start: whole.start,
                    end: whole.start + button,
                },
                Span {
                    start: whole.end - button,
                    end: whole.end,
                },
                Span {
                    start: whole.start + button,
                    end: whole.end - button,
                },
            )
        };
        let view = span(vertical, inner).len();
        let max = (at.content - view).max(0.0);
        let thumb_len = (track.len() * view / at.content.max(1.0))
            .max(thick)
            .min(track.len().max(0.0));
        let travel = (track.len() - thumb_len).max(0.0);
        let scrolled = if max > 0.0 {
            (at.offset / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let from = snap(track.start + travel * scrolled);
        Self {
            strip,
            back: slice(vertical, strip, back),
            forward: slice(vertical, strip, forward),
            track: slice(vertical, strip, track),
            thumb: slice(
                vertical,
                strip,
                Span {
                    start: from,
                    end: snap(from + thumb_len),
                },
            ),
            max,
            travel,
            scrolled: at.offset,
        }
    }
}

/// Which pieces the pointer is on or pressing.
#[derive(Clone, Copy, Default)]
struct Touch {
    back: Piece,
    forward: Piece,
    thumb: Piece,
}

#[derive(Clone, Copy, Default)]
struct Piece {
    hover: bool,
    press: bool,
}

impl Piece {
    fn of(response: &egui::Response) -> Self {
        Self {
            hover: response.hovered(),
            press: response.is_pointer_button_down_on(),
        }
    }
}

/// The distance a held button asks for this frame: a step on the press,
/// then a steady speed once held for a moment.
fn push(ctx: &egui::Context, id: Id, down: bool, step: f32, speed: f32) -> f32 {
    let began: Option<f64> = ctx.data(|d| d.get_temp(id));
    if !down {
        if began.is_some() {
            ctx.data_mut(|d| d.remove::<f64>(id));
        }
        return 0.0;
    }
    let (now, dt) = ctx.input(|i| (i.time, i.stable_dt.min(0.1)));
    ctx.request_repaint();
    match began {
        None => {
            ctx.data_mut(|d| d.insert_temp(id, now));
            step
        }
        Some(began) if now - began > REPEAT_AFTER => speed * dt,
        Some(_) => 0.0,
    }
}

/// Reads the bar's pieces for clicks and drags, and returns what they are
/// doing and the offset they ask for.
fn drive(
    ui: &Ui,
    key: Id,
    vertical: bool,
    parts: &Parts,
    last: Last,
    asked: Option<f32>,
) -> (Touch, Option<f32>) {
    let ctx = ui.ctx();
    let along = |pos: egui::Pos2| if vertical { pos.y } else { pos.x };
    let mut offset = asked.unwrap_or(last.offset);
    let mut changed = asked.is_some();

    let track = ui.interact(parts.track, key.with("track"), Sense::click());
    let back = ui.interact(parts.back, key.with("back"), Sense::click());
    let forward = ui.interact(parts.forward, key.with("forward"), Sense::click());
    let thumb = ui.interact(parts.thumb, key.with("thumb"), Sense::drag());

    let mut ask = |by: f32| {
        if by != 0.0 {
            offset = (offset + by).clamp(0.0, parts.max);
            changed = true;
        }
    };
    ask(-push(
        ctx,
        key.with("back-held"),
        back.is_pointer_button_down_on() && last.offset > 0.0,
        STEP,
        SPEED,
    ));
    ask(push(
        ctx,
        key.with("forward-held"),
        forward.is_pointer_button_down_on() && last.offset < parts.max,
        STEP,
        SPEED,
    ));

    // A click on the track pages towards it, until the thumb gets there.
    let view = span(vertical, last.inner).len();
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let towards = pointer.map(|pos| {
        let thumb = span(vertical, parts.thumb);
        if along(pos) < thumb.start {
            -1.0
        } else if along(pos) > thumb.end {
            1.0
        } else {
            0.0
        }
    });
    let paging = track.is_pointer_button_down_on() && !thumb.is_pointer_button_down_on();
    let direction = towards.unwrap_or(0.0);
    ask(direction
        * push(
            ctx,
            key.with("track-held"),
            paging && direction != 0.0,
            view * 0.9,
            view * 4.0,
        ));

    // Dragging the thumb keeps it under the pointer where it was grabbed.
    let grab_id = key.with("grab");
    if thumb.drag_started() {
        if let Some(pos) = thumb.interact_pointer_pos() {
            let grab = along(pos) - span(vertical, parts.thumb).start;
            ctx.data_mut(|d| d.insert_temp(grab_id, grab));
        }
    }
    if thumb.dragged() {
        let grab: f32 = ctx.data(|d| d.get_temp(grab_id)).unwrap_or(0.0);
        if let (Some(pos), true) = (thumb.interact_pointer_pos(), parts.travel > 0.0) {
            let track = span(vertical, parts.track);
            let at = ((along(pos) - grab - track.start) / parts.travel).clamp(0.0, 1.0);
            offset = at * parts.max;
            changed = true;
        }
    }

    (
        Touch {
            back: Piece::of(&back),
            forward: Piece::of(&forward),
            thumb: Piece {
                hover: thumb.hovered(),
                press: thumb.dragged() || thumb.is_pointer_button_down_on(),
            },
        },
        changed.then_some(offset),
    )
}

/// Which way an arrow points.
#[derive(Clone, Copy)]
enum Toward {
    Back,
    Forward,
}

/// A solid triangle in `rect`'s middle, `size` wide at its base, pointing
/// along the bar.
fn arrow(painter: &Painter, rect: Rect, vertical: bool, toward: Toward, size: f32, color: Color32) {
    let c = rect.center();
    let sign = match toward {
        Toward::Back => -1.0,
        Toward::Forward => 1.0,
    };
    // The base is twice as wide as the arrow is tall.
    let (reach, half) = (size * 0.25, size * 0.5);
    let points = if vertical {
        vec![
            pos2(c.x, c.y + sign * reach),
            pos2(c.x - half, c.y - sign * reach),
            pos2(c.x + half, c.y - sign * reach),
        ]
    } else {
        vec![
            pos2(c.x + sign * reach, c.y),
            pos2(c.x - sign * reach, c.y - half),
            pos2(c.x - sign * reach, c.y + half),
        ]
    };
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// How many points a tile of the dither covers each way.
const TILE: f32 = 16.0;

/// A square of `a` and `b` on a one-pixel checker, tiled into the dither of
/// Windows 98's scroll track.
fn dither(ctx: &egui::Context, a: Color32, b: Color32) -> TextureHandle {
    let id = Id::new(("scroll-dither", a, b));
    if let Some(texture) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return texture;
    }
    let side = TILE as usize;
    let pixels = (0..side * side)
        .map(|i| {
            if (i % side + i / side).is_multiple_of(2) {
                a
            } else {
                b
            }
        })
        .collect();
    let texture = ctx.load_texture(
        "scroll-dither",
        egui::ColorImage::new([side, side], pixels),
        TextureOptions::NEAREST,
    );
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    texture
}

/// `rect` filled with the dither.
fn paint_dither(painter: &Painter, rect: Rect, texture: &TextureHandle) {
    let mut y = rect.top();
    while y < rect.bottom() {
        let high = (rect.bottom() - y).min(TILE);
        let mut x = rect.left();
        while x < rect.right() {
            let wide = (rect.right() - x).min(TILE);
            painter.image(
                texture.id(),
                Rect::from_min_size(pos2(x, y), vec2(wide, high)),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(wide / TILE, high / TILE)),
                Color32::WHITE,
            );
            x += TILE;
        }
        y += TILE;
    }
}

/// Three short lines across the middle of a thumb, for a hand to hold: a
/// dark one with a light one beside it.
fn grip(painter: &Painter, thumb: Rect, vertical: bool, dark: Color32, light: Color32) {
    let along = span(vertical, thumb);
    if along.len() < 26.0 {
        return;
    }
    let mid = (along.start + along.end) / 2.0;
    let across = if vertical {
        (thumb.center().x - 4.0, thumb.center().x + 4.0)
    } else {
        (thumb.center().y - 4.0, thumb.center().y + 4.0)
    };
    for line in [-3.0f32, 0.0, 3.0] {
        let at = (mid + line).floor() + 0.5;
        for (shift, color) in [(0.0, dark), (1.0, light)] {
            let (from, to) = if vertical {
                (pos2(across.0, at + shift), pos2(across.1, at + shift))
            } else {
                (pos2(at + shift, across.0), pos2(at + shift, across.1))
            };
            painter.line_segment([from, to], Stroke::new(1.0, color));
        }
    }
}

/// The direction a gradient runs to go across the bar.
fn across(vertical: bool) -> Axis {
    if vertical {
        Axis::Across
    } else {
        Axis::Down
    }
}

fn paint(
    painter: &Painter,
    ctx: &egui::Context,
    palette: &Palette,
    family: Family,
    vertical: bool,
    parts: &Parts,
    touch: Touch,
) {
    let can_back = parts.scrolled > 0.5;
    let can_forward = parts.scrolled < parts.max - 0.5;
    match family {
        Family::Modern => {}
        Family::Retro => retro(
            painter,
            ctx,
            palette,
            vertical,
            parts,
            touch,
            can_back,
            can_forward,
        ),
        Family::Luna => luna(
            painter,
            palette,
            vertical,
            parts,
            touch,
            can_back,
            can_forward,
        ),
        Family::Glass => glass(
            painter,
            palette,
            vertical,
            parts,
            touch,
            can_back,
            can_forward,
        ),
        Family::Gel => gel(
            painter,
            palette,
            vertical,
            parts,
            touch,
            can_back,
            can_forward,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn retro(
    painter: &Painter,
    ctx: &egui::Context,
    palette: &Palette,
    vertical: bool,
    parts: &Parts,
    touch: Touch,
    can_back: bool,
    can_forward: bool,
) {
    let face = palette.canvas;
    // The track is a checker of the face and white.
    let light = lighter(face, if palette.dark { 0.12 } else { 0.9 });
    paint_dither(painter, parts.track, &dither(ctx, face, light));
    let button = |rect: Rect, piece: Piece, enabled: bool, toward: Toward| {
        painter.rect_filled(rect, CornerRadius::ZERO, face);
        let down = piece.press && enabled;
        retro_edge(painter, rect, face, down, 1.0);
        let ink = if enabled {
            palette.ink
        } else {
            darker(face, 0.4)
        };
        let nudge = if down { vec2(1.0, 1.0) } else { vec2(0.0, 0.0) };
        arrow(
            painter,
            rect.translate(nudge),
            vertical,
            toward,
            rect.width().min(rect.height()) * 0.42,
            ink,
        );
    };
    button(parts.back, touch.back, can_back, Toward::Back);
    button(parts.forward, touch.forward, can_forward, Toward::Forward);
    painter.rect_filled(parts.thumb, CornerRadius::ZERO, face);
    retro_edge(painter, parts.thumb, face, false, 1.0);
}

#[allow(clippy::too_many_arguments)]
fn luna(
    painter: &Painter,
    palette: &Palette,
    vertical: bool,
    parts: &Parts,
    touch: Touch,
    can_back: bool,
    can_forward: bool,
) {
    let dark = palette.dark;
    let blue = palette.deep;
    let radius = theme::round(3);
    let cross = across(vertical);
    // The track, pale and a little darker on its first edge.
    let (edge, bright) = if dark {
        (darker(palette.surface, 0.2), lighter(palette.surface, 0.06))
    } else {
        (palette.tone(rgb(0xe3e1d8)), palette.tone(rgb(0xfaf9f6)))
    };
    shade::gradient(
        painter,
        parts.strip,
        CornerRadius::ZERO,
        cross,
        &[(0.0, edge), (1.0, bright)],
    );
    // A blue body, light along its first edge and deeper along its last.
    let body = |rect: Rect, piece: Piece, enabled: bool| {
        let shift = if !enabled {
            0.0
        } else if piece.press {
            -0.12
        } else if piece.hover {
            0.14
        } else {
            0.0
        };
        let tones = if dark {
            [
                lerp(lighter(palette.surface, 0.1), blue, 0.35),
                lerp(palette.surface, blue, 0.55),
                lerp(darker(palette.surface, 0.1), blue, 0.7),
            ]
        } else {
            [
                lerp(Color32::WHITE, blue, 0.2),
                lerp(Color32::WHITE, blue, 0.4),
                lerp(Color32::WHITE, blue, 0.56),
            ]
        };
        let tones = tones.map(|c| {
            if shift >= 0.0 {
                lighter(c, shift)
            } else {
                darker(c, -shift)
            }
        });
        let fill = if enabled {
            tones
        } else {
            let flat = lerp(palette.canvas, tones[1], 0.25);
            [flat, flat, flat]
        };
        shade::gradient(
            painter,
            rect,
            radius,
            cross,
            &[(0.0, fill[0]), (0.4, fill[1]), (1.0, fill[2])],
        );
        let line = if dark {
            lighter(blue, 0.2)
        } else {
            lerp(blue, Color32::WHITE, 0.25)
        };
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, if enabled { line } else { faded(line, 0.5) }),
            StrokeKind::Inside,
        );
        if enabled {
            painter.rect_stroke(
                rect.shrink(1.0),
                radius,
                Stroke::new(1.0, Color32::from_white_alpha(if dark { 28 } else { 150 })),
                StrokeKind::Inside,
            );
        }
    };
    let ink = if dark {
        palette.ink
    } else {
        darker(lerp(blue, gray(0x80), 0.4), 0.35)
    };
    for (rect, piece, enabled, toward) in [
        (parts.back, touch.back, can_back, Toward::Back),
        (parts.forward, touch.forward, can_forward, Toward::Forward),
    ] {
        let rect = rect.shrink(1.0);
        body(rect, piece, enabled);
        let ink = if enabled { ink } else { faded(ink, 0.4) };
        arrow(
            painter,
            rect,
            vertical,
            toward,
            rect.width().min(rect.height()) * 0.4,
            ink,
        );
    }
    let thumb = parts.thumb.shrink(1.0);
    body(thumb, touch.thumb, true);
    grip(
        painter,
        thumb,
        vertical,
        faded(darker(blue, 0.2), 0.55),
        Color32::from_white_alpha(if dark { 40 } else { 200 }),
    );
}

#[allow(clippy::too_many_arguments)]
fn glass(
    painter: &Painter,
    palette: &Palette,
    vertical: bool,
    parts: &Parts,
    touch: Touch,
    can_back: bool,
    can_forward: bool,
) {
    let dark = palette.dark;
    let blue = palette.deep;
    let radius = theme::round(2);
    let cross = across(vertical);
    let (edge, mid, bright) = if dark {
        (
            darker(palette.surface, 0.25),
            darker(palette.surface, 0.1),
            palette.surface,
        )
    } else {
        (
            palette.tone(gray(0xe4)),
            palette.tone(gray(0xee)),
            palette.tone(gray(0xf7)),
        )
    };
    shade::gradient(
        painter,
        parts.strip,
        CornerRadius::ZERO,
        cross,
        &[(0.0, edge), (0.3, mid), (1.0, bright)],
    );
    // Glass split into a light half and a darker one, blue when hovered.
    let body = |rect: Rect, piece: Piece, enabled: bool| {
        let (calm, lit, held, calm_line, lit_line) = if dark {
            let base = lighter(palette.surface, 0.1);
            let calm = [
                lighter(base, 0.2),
                lighter(base, 0.14),
                base,
                darker(base, 0.12),
            ];
            let lit = calm.map(|c| lerp(c, palette.bright, 0.3));
            let held = calm.map(|c| lerp(darker(c, 0.15), palette.bright, 0.35));
            (calm, lit, held, darker(palette.canvas, 0.5), palette.bright)
        } else {
            let tint = |amount| lerp(Color32::WHITE, blue, amount);
            (
                [gray(0xf4), gray(0xe9), gray(0xdb), gray(0xd0)].map(|c| palette.tone(c)),
                [tint(0.07), tint(0.13), tint(0.28), tint(0.36)],
                [tint(0.12), tint(0.22), tint(0.42), tint(0.55)],
                gray(0x8e),
                darker(lerp(blue, gray(0x80), 0.3), 0.1),
            )
        };
        let (colors, line) = if !enabled {
            (calm.map(|c| lerp(c, palette.canvas, 0.5)), calm_line)
        } else if piece.press {
            (held, lit_line)
        } else if piece.hover {
            (lit, lit_line)
        } else {
            (calm, calm_line)
        };
        shade::gradient(
            painter,
            rect,
            radius,
            cross,
            &[
                (0.0, colors[0]),
                (0.5, colors[1]),
                (0.505, colors[2]),
                (1.0, colors[3]),
            ],
        );
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, if enabled { line } else { faded(line, 0.5) }),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            rect.shrink(1.0),
            radius,
            Stroke::new(1.0, Color32::from_white_alpha(if dark { 30 } else { 150 })),
            StrokeKind::Inside,
        );
    };
    for (rect, piece, enabled, toward) in [
        (parts.back, touch.back, can_back, Toward::Back),
        (parts.forward, touch.forward, can_forward, Toward::Forward),
    ] {
        body(rect, piece, enabled);
        let ink = if dark { palette.ink } else { gray(0x40) };
        arrow(
            painter,
            rect,
            vertical,
            toward,
            rect.width().min(rect.height()) * 0.38,
            if enabled { ink } else { faded(ink, 0.35) },
        );
    }
    body(parts.thumb, touch.thumb, true);
    grip(
        painter,
        parts.thumb,
        vertical,
        if dark {
            Color32::from_black_alpha(150)
        } else {
            faded(gray(0x70), 0.9)
        },
        Color32::from_white_alpha(if dark { 40 } else { 220 }),
    );
}

#[allow(clippy::too_many_arguments)]
fn gel(
    painter: &Painter,
    palette: &Palette,
    vertical: bool,
    parts: &Parts,
    touch: Touch,
    can_back: bool,
    can_forward: bool,
) {
    let dark = palette.dark;
    let cross = across(vertical);
    // A groove: darker along its first edge, paling to the other.
    let (edge, bright, line) = if dark {
        (
            darker(palette.surface, 0.35),
            darker(palette.surface, 0.05),
            darker(palette.canvas, 0.5),
        )
    } else {
        (gray(0xd2), gray(0xf9), gray(0x9c))
    };
    shade::gradient(
        painter,
        parts.track,
        CornerRadius::ZERO,
        cross,
        &[(0.0, edge), (0.45, lerp(edge, bright, 0.8)), (1.0, bright)],
    );
    // The arrows' footing is the groove too, under their capsule.
    shade::gradient(
        painter,
        parts.back.union(parts.forward),
        CornerRadius::ZERO,
        cross,
        &[(0.0, edge), (0.45, lerp(edge, bright, 0.8)), (1.0, bright)],
    );
    let first = Stroke::new(1.0, line);
    if vertical {
        painter.line_segment(
            [
                pos2(parts.strip.left() + 0.5, parts.strip.top()),
                pos2(parts.strip.left() + 0.5, parts.strip.bottom()),
            ],
            first,
        );
    } else {
        painter.line_segment(
            [
                pos2(parts.strip.left(), parts.strip.top() + 0.5),
                pos2(parts.strip.right(), parts.strip.top() + 0.5),
            ],
            first,
        );
    }

    let blue = palette.deep;
    // A gel capsule: bright near its first edge, then deepening.
    let capsule = |rect: Rect, piece: Piece, enabled: bool| {
        let radius = CornerRadius::same((rect.width().min(rect.height()) / 2.0).round() as u8);
        let base = darker(
            lighter(blue, if piece.hover { 0.06 } else { 0.0 }),
            if piece.press { 0.15 } else { 0.0 },
        );
        let base = if enabled {
            base
        } else {
            lerp(base, palette.canvas, 0.55)
        };
        shade::gradient(
            painter,
            rect,
            radius,
            cross,
            &[
                (0.0, lighter(base, 0.12)),
                (0.3, lighter(base, 0.6)),
                (0.52, lighter(base, 0.22)),
                (0.525, base),
                (1.0, lighter(base, 0.5)),
            ],
        );
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(
                1.0,
                faded(darker(base, 0.45), if enabled { 1.0 } else { 0.6 }),
            ),
            StrokeKind::Inside,
        );
        base
    };
    let thumb = if vertical {
        parts.thumb.shrink2(vec2(1.0, 0.0))
    } else {
        parts.thumb.shrink2(vec2(0.0, 1.0))
    };
    let base = capsule(thumb, touch.thumb, true);
    grip(
        painter,
        thumb,
        vertical,
        faded(darker(base, 0.5), 0.7),
        Color32::from_white_alpha(120),
    );

    // Both arrows in one capsule, a seam between them.
    let both = parts.back.union(parts.forward);
    let both = if vertical {
        both.shrink2(vec2(1.0, 0.0))
    } else {
        both.shrink2(vec2(0.0, 1.0))
    };
    let base = capsule(both, Piece::default(), true);
    let ink = darker(base, 0.65);
    for (rect, piece, enabled, toward) in [
        (parts.back, touch.back, can_back, Toward::Back),
        (parts.forward, touch.forward, can_forward, Toward::Forward),
    ] {
        if piece.press && enabled {
            painter.with_clip_rect(rect).rect_filled(
                both,
                CornerRadius::same(255),
                Color32::from_black_alpha(50),
            );
        }
        arrow(
            painter,
            rect,
            vertical,
            toward,
            rect.width().min(rect.height()) * 0.4,
            if enabled { ink } else { faded(ink, 0.35) },
        );
    }
    let seam = if vertical {
        [
            pos2(both.left() + 2.0, parts.forward.top()),
            pos2(both.right() - 2.0, parts.forward.top()),
        ]
    } else {
        [
            pos2(parts.forward.left(), both.top() + 2.0),
            pos2(parts.forward.left(), both.bottom() - 2.0),
        ]
    };
    painter.line_segment(seam, Stroke::new(1.0, faded(darker(base, 0.45), 0.7)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{settings::Style, ui::theme::Skin};
    use eframe::egui::{Event, PointerButton, Pos2, RawInput};

    fn skin(style: Style) -> Skin {
        Skin {
            style,
            ..Skin::default()
        }
    }

    /// A window with one scroll area over `content` points, driven by hand.
    struct Window {
        ctx: egui::Context,
        style: Style,
        vertical: bool,
        content: f32,
        time: f64,
        inner: Rect,
        offset: f32,
    }

    impl Window {
        fn new(style: Style, vertical: bool, content: f32) -> Self {
            let mut window = Self {
                ctx: egui::Context::default(),
                style,
                vertical,
                content,
                time: 0.0,
                inner: Rect::NOTHING,
                offset: 0.0,
            };
            // The bar's room is settled over a few frames.
            for _ in 0..4 {
                window.frame(vec![]);
            }
            window
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                time: Some(self.time),
                events,
                ..RawInput::default()
            };
            let (style, vertical, content) = (self.style, self.vertical, self.content);
            let (mut inner, mut offset) = (self.inner, self.offset);
            theme::with_skin(skin(style), || {
                let mut output = self.ctx.run_ui(input, |ui| {
                    let scroll = if vertical {
                        super::vertical()
                    } else {
                        horizontal()
                    };
                    let out = scroll
                        .id_salt("test")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.allocate_space(if vertical {
                                vec2(10.0, content)
                            } else {
                                vec2(content, 10.0)
                            });
                        });
                    inner = out.inner_rect;
                    offset = if vertical {
                        out.state.offset.y
                    } else {
                        out.state.offset.x
                    };
                });
                // Nothing here draws, so nobody takes the textures.
                output.textures_delta.clear();
            });
            (self.inner, self.offset) = (inner, offset);
        }

        fn last(&self) -> Last {
            Last {
                inner: self.inner,
                content: self.content,
                offset: self.offset,
            }
        }

        fn thick(&self) -> f32 {
            theme::scroll_bar(self.style, &Palette::current(&self.ctx))
                .map_or(0.0, |(width, _)| width)
        }

        fn parts(&self) -> Parts {
            let thick = self.thick();
            Parts::of(
                theme::with_skin(skin(self.style), theme::skin).family(),
                self.vertical,
                self.last(),
                thick,
                self.ctx.pixels_per_point(),
            )
        }

        fn view(&self) -> f32 {
            span(self.vertical, self.inner).len()
        }

        fn press(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerMoved(at)]);
            self.frame(vec![]);
            self.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }]);
            self.frame(vec![]);
        }

        fn drag_to(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerMoved(at)]);
            self.frame(vec![]);
        }

        fn release(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }]);
            self.frame(vec![]);
        }
    }

    #[test]
    fn a_short_page_has_no_bar_and_keeps_its_width() {
        let window = Window::new(Style::Retro, true, 50.0);
        assert!(!window.last().needed(true));
        assert!(window.inner.width() > 380.0 - window.thick());
    }

    #[test]
    fn a_long_page_gives_up_the_bars_width() {
        let long = Window::new(Style::Retro, true, 2000.0);
        let short = Window::new(Style::Retro, true, 50.0);
        assert!(long.last().needed(true));
        assert!((short.inner.width() - long.inner.width() - long.thick()).abs() < 1.0);
    }

    #[test]
    fn modern_keeps_eguis_floating_bar() {
        let long = Window::new(Style::Modern, true, 2000.0);
        let short = Window::new(Style::Modern, true, 50.0);
        assert_eq!(long.inner.width(), short.inner.width());
    }

    #[test]
    fn the_arrows_scroll_a_step_and_stop_at_the_ends() {
        let mut window = Window::new(Style::Retro, true, 2000.0);
        let parts = window.parts();
        // At the top the back arrow does nothing.
        window.press(parts.back.center());
        window.release(parts.back.center());
        assert_eq!(window.offset, 0.0);
        window.press(parts.forward.center());
        window.release(parts.forward.center());
        assert_eq!(window.offset, STEP);
        window.press(parts.back.center());
        window.release(parts.back.center());
        assert_eq!(window.offset, 0.0);
    }

    #[test]
    fn dragging_the_thumb_keeps_it_under_the_pointer() {
        for style in [Style::Retro, Style::Luna, Style::Glass, Style::Aqua] {
            let mut window = Window::new(style, true, 2000.0);
            let parts = window.parts();
            let grab = parts.thumb.center();
            window.press(grab);
            window.drag_to(grab + vec2(0.0, parts.travel / 2.0));
            let max = window.last().content - window.view();
            // The thumb sits on whole pixels, which is a pixel of travel.
            let pixel = max / parts.travel;
            assert!(
                (window.offset - max / 2.0).abs() <= pixel,
                "{style:?}: {} of {max}",
                window.offset
            );
            window.drag_to(grab + vec2(0.0, 5000.0));
            assert!((window.offset - max).abs() < 0.5, "{style:?}");
            window.drag_to(grab - vec2(0.0, 5000.0));
            assert_eq!(window.offset, 0.0, "{style:?}");
            window.release(grab);
        }
    }

    #[test]
    fn a_click_on_the_track_pages_towards_it() {
        let mut window = Window::new(Style::Luna, true, 2000.0);
        let parts = window.parts();
        let at = pos2(parts.track.center().x, parts.track.bottom() - 4.0);
        window.press(at);
        window.release(at);
        assert!((window.offset - window.view() * 0.9).abs() < 1.0);
    }

    #[test]
    fn a_horizontal_bar_drags_along_x() {
        let mut window = Window::new(Style::Glass, false, 3000.0);
        let parts = window.parts();
        let grab = parts.thumb.center();
        window.press(grab);
        window.drag_to(grab + vec2(parts.travel, 0.0));
        let max = window.last().content - window.view();
        assert!((window.offset - max).abs() < 1.0);
        window.release(grab);
    }

    #[test]
    fn the_pieces_always_fit_the_strip() {
        for family in [Family::Retro, Family::Luna, Family::Glass, Family::Gel] {
            for vertical in [true, false] {
                for len in [1.0f32, 10.0, 34.0, 60.0, 400.0, 2000.0] {
                    for content in [len + 1.0, len * 1.5, len * 40.0] {
                        for step in 0..=10 {
                            let offset = (content - len) * step as f32 / 10.0;
                            let inner = if vertical {
                                Rect::from_min_size(pos2(5.0, 7.0), vec2(300.0, len))
                            } else {
                                Rect::from_min_size(pos2(5.0, 7.0), vec2(len, 300.0))
                            };
                            let parts = Parts::of(
                                family,
                                vertical,
                                Last {
                                    inner,
                                    content,
                                    offset,
                                },
                                17.0,
                                2.0,
                            );
                            let whole = span(vertical, parts.strip);
                            let thumb = span(vertical, parts.thumb);
                            let track = span(vertical, parts.track);
                            let ok = |s: Span| {
                                s.start >= whole.start - 0.01 && s.end <= whole.end + 0.01
                            };
                            assert!(ok(thumb), "{family:?} {len} {content} {offset}");
                            assert!(ok(span(vertical, parts.back)));
                            assert!(ok(span(vertical, parts.forward)));
                            // The thumb stays on its track when there's room for it.
                            if track.len() >= 17.0 {
                                assert!(
                                    thumb.start >= track.start - 0.51
                                        && thumb.end <= track.end + 0.51,
                                    "{family:?} {len} {content} {offset}: {thumb:?} in {track:?}"
                                );
                            }
                            assert!(thumb.len() >= 0.0);
                        }
                    }
                }
            }
        }
    }
}
