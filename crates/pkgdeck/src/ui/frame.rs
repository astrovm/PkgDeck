//! The window's own frame in the older styles: the title bar, its buttons
//! and the border, drawn the way each desktop drew its windows. Modern
//! keeps the system's frame.

use super::skin::{
    bands, bevel, darker, faded, gradient, gray, ink_on, lerp, lighter, retro_edge, rgb,
    top_corners,
};
use super::theme::{self, Palette};
use crate::settings::{Family, Style};
use eframe::egui::{
    self, pos2, vec2, viewport::ResizeDirection, Color32, CornerRadius, CursorIcon, Id, Painter,
    Rect, Sense, Stroke, StrokeKind, Ui, ViewportCommand,
};

/// A button in the title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Caption {
    Minimize,
    Maximize,
    Close,
}

impl Caption {
    pub fn label(self, maximized: bool) -> &'static str {
        match self {
            Self::Minimize => "Minimize window",
            Self::Maximize if maximized => "Restore window",
            Self::Maximize => "Maximize window",
            Self::Close => "Close window",
        }
    }
    fn command(self, maximized: bool) -> ViewportCommand {
        match self {
            Self::Minimize => ViewportCommand::Minimized(true),
            Self::Maximize => ViewportCommand::Maximized(!maximized),
            Self::Close => ViewportCommand::Close,
        }
    }
}

/// How a style's frame is built: the border on the sides and bottom, the
/// border above the title bar, the title bar's height, and the gap under
/// it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub side: f32,
    pub top: f32,
    pub title: f32,
    pub below: f32,
}

/// The frame `style` draws, or None when it keeps the system's.
pub fn metrics(style: Style) -> Option<Metrics> {
    let (side, top, title, below) = match style.family() {
        Family::Modern => return None,
        // Windows 98: a raised edge all round, the title bar inside it.
        Family::Retro => (4.0, 4.0, 22.0, 2.0),
        // XP: a blue border, the title bar up to the top.
        Family::Luna => (4.0, 0.0, 30.0, 0.0),
        // Vista and 7: 8 pixels of glass and two lines round the content,
        // the title on the glass.
        Family::Glass => (10.0, 0.0, 30.0, 2.0),
        // Aqua: a hairline and the striped title bar.
        Family::Gel => (1.0, 1.0, 23.0, 0.0),
        // Mac OS 9: a grey drag frame and the striped title bar.
        Family::Platinum => (5.0, 1.0, 21.0, 0.0),
    };
    Some(Metrics {
        side,
        top,
        title,
        below,
    })
}

/// Whether the window is maximized and whether it's the one in front.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    pub maximized: bool,
    pub active: bool,
}

impl WindowState {
    fn of(ctx: &egui::Context) -> Self {
        ctx.input(|i| {
            let viewport = i.viewport();
            Self {
                maximized: viewport.maximized.unwrap_or(false)
                    || viewport.fullscreen.unwrap_or(false),
                active: viewport.focused.unwrap_or(true),
            }
        })
    }
}

/// The title bar inside `full`, and what's left for the app under it.
pub fn layout(metrics: Metrics, full: Rect, maximized: bool) -> (Rect, Rect) {
    let (side, top) = if maximized {
        (0.0, 0.0)
    } else {
        (metrics.side, metrics.top)
    };
    let bar = Rect::from_min_max(
        pos2(full.left() + side, full.top() + top),
        pos2(full.right() - side, full.top() + top + metrics.title),
    );
    let inner = Rect::from_min_max(
        pos2(full.left() + side, bar.bottom() + metrics.below),
        pos2(full.right() - side, full.bottom() - side),
    );
    (bar, inner)
}

/// Draws the frame around `full` and handles its buttons, dragging and
/// double clicks. Returns where the app goes.
pub fn show(ui: &mut Ui, full: Rect, title: &str, palette: &Palette) -> Rect {
    let ctx = ui.ctx().clone();
    let style = theme::skin().style;
    let metrics = metrics(style);
    decorate(&ctx, metrics.is_none());
    let Some(metrics) = metrics else {
        return full;
    };
    let state = WindowState::of(&ctx);
    let (bar, inner) = layout(metrics, full, state.maximized);
    let painter = ui.painter().clone();
    paint_frame(&painter, full, bar, inner, palette, state);
    // Dragging the title bar moves the window; a double click maximizes.
    let id = Id::new("window-frame");
    let drag = ui.interact(bar, id.with("title"), Sense::click_and_drag());
    if drag.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!state.maximized));
    }
    let buttons = captions(style, bar);
    let hovered_group = buttons
        .iter()
        .any(|(_, rect)| ui.rect_contains_pointer(rect.expand(2.0)));
    for (caption, rect) in buttons {
        let response = ui.interact(rect, id.with(caption), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                caption.label(state.maximized),
            )
        });
        let look = Press {
            hovered: response.hovered(),
            pressed: response.is_pointer_button_down_on(),
            group: hovered_group,
        };
        paint_caption(&painter, rect, caption, look, palette, state);
        if response.clicked() {
            ctx.send_viewport_cmd(caption.command(state.maximized));
        }
    }
    paint_title(ui, bar, title, palette, state);
    inner
}

/// Lets the frame's edges resize the window. Called after the app has
/// drawn. It reads the pointer itself rather than adding widgets, so the
/// title bar and its buttons next to the edge don't take the press.
pub fn edges(ui: &mut Ui, full: Rect) {
    let ctx = ui.ctx().clone();
    let Some(metrics) = metrics(theme::skin().style) else {
        return;
    };
    if WindowState::of(&ctx).maximized {
        return;
    }
    let Some(direction) = ctx
        .pointer_hover_pos()
        .and_then(|pointer| edge_at(metrics, full, pointer))
    else {
        return;
    };
    ctx.set_cursor_icon(cursor(direction));
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Which way the window resizes from `pointer`: anywhere near the edge,
/// and diagonally near a corner.
pub fn edge_at(metrics: Metrics, full: Rect, pointer: egui::Pos2) -> Option<ResizeDirection> {
    if !full.contains(pointer) {
        return None;
    }
    let reach = metrics.side.max(5.0);
    let corner = reach * 3.0;
    let (x, y) = (pointer.x, pointer.y);
    let left = x < full.left() + reach;
    let right = x > full.right() - reach;
    let top = y < full.top() + reach;
    let bottom = y > full.bottom() - reach;
    if !(left || right || top || bottom) {
        return None;
    }
    let near_left = x < full.left() + corner;
    let near_right = x > full.right() - corner;
    let near_top = y < full.top() + corner;
    let near_bottom = y > full.bottom() - corner;
    Some(match () {
        _ if near_top && near_left => ResizeDirection::NorthWest,
        _ if near_top && near_right => ResizeDirection::NorthEast,
        _ if near_bottom && near_left => ResizeDirection::SouthWest,
        _ if near_bottom && near_right => ResizeDirection::SouthEast,
        _ if top => ResizeDirection::North,
        _ if bottom => ResizeDirection::South,
        _ if left => ResizeDirection::West,
        _ => ResizeDirection::East,
    })
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// Turns the system's frame on or off when the style asks for the other.
fn decorate(ctx: &egui::Context, on: bool) {
    let id = Id::new("window-decorated");
    if ctx.data(|data| data.get_temp::<bool>(id)) != Some(on) {
        ctx.data_mut(|data| data.insert_temp(id, on));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(on));
    }
}

/// Where each button of `style`'s title bar `bar` sits, left to right.
pub fn captions(style: Style, bar: Rect) -> Vec<(Caption, Rect)> {
    let square = |center: egui::Pos2, size: egui::Vec2| Rect::from_center_size(center, size);
    let y = bar.center().y;
    match style.family() {
        Family::Modern => vec![],
        Family::Retro => {
            // Minimize and maximize side by side, close apart.
            let size = vec2(18.0, 16.0);
            let close = square(pos2(bar.right() - 3.0 - size.x / 2.0, y), size);
            let maximize = close.translate(vec2(-size.x - 2.0, 0.0));
            let minimize = maximize.translate(vec2(-size.x, 0.0));
            vec![
                (Caption::Minimize, minimize),
                (Caption::Maximize, maximize),
                (Caption::Close, close),
            ]
        }
        Family::Luna => {
            let size = vec2(22.0, 22.0);
            let close = square(pos2(bar.right() - 5.0 - size.x / 2.0, y + 1.0), size);
            let maximize = close.translate(vec2(-size.x - 2.0, 0.0));
            let minimize = maximize.translate(vec2(-size.x - 2.0, 0.0));
            vec![
                (Caption::Minimize, minimize),
                (Caption::Maximize, maximize),
                (Caption::Close, close),
            ]
        }
        Family::Glass => super::aero::captions(style == Style::Vista, bar),
        Family::Gel => {
            // Close, minimize and zoom at the left: Jaguar's a pixel
            // narrower and further apart.
            let (size, left, pitch) = if style == Style::Gel {
                (vec2(13.0, 14.0), 9.0, 22.0)
            } else {
                (vec2(14.0, 14.0), 8.0, 21.0)
            };
            let at = |i: f32| square(pos2(bar.left() + left + size.x / 2.0 + i * pitch, y), size);
            vec![
                (Caption::Close, at(0.0)),
                (Caption::Minimize, at(1.0)),
                (Caption::Maximize, at(2.0)),
            ]
        }
        Family::Platinum => {
            // The close box at the left; zoom and collapse at the right.
            let size = vec2(13.0, 13.0);
            let close = square(pos2(bar.left() + 8.0 + size.x / 2.0, y), size);
            let minimize = square(pos2(bar.right() - 8.0 - size.x / 2.0, y), size);
            let maximize = minimize.translate(vec2(-size.x - 6.0, 0.0));
            vec![
                (Caption::Close, close),
                (Caption::Maximize, maximize),
                (Caption::Minimize, minimize),
            ]
        }
    }
}

/// How a caption button is being touched: itself, or the group it's in,
/// which shows Aqua's symbols.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Press {
    pub hovered: bool,
    pub pressed: bool,
    pub group: bool,
}

/// The title bar's colour where the style takes it from the accent.
fn title_colors(palette: &Palette, active: bool) -> (Color32, Color32) {
    let deep = palette.deep;
    match theme::skin().family() {
        // Windows 95's title bar is one solid colour.
        Family::Retro if active && theme::skin().style == Style::Win95 => (deep, deep),
        Family::Retro if theme::skin().style == Style::Win95 => {
            let dim = lerp(palette.canvas, palette.ink, 0.35);
            (dim, dim)
        }
        Family::Retro if active => {
            // Windows 98's navy fades to its light blue; another colour
            // fades to a lighter self.
            let end = if deep == rgb(0x000080) {
                rgb(0x1084d0)
            } else {
                lerp(deep, Color32::WHITE, 0.45)
            };
            (deep, end)
        }
        Family::Retro => {
            let dim = lerp(palette.canvas, palette.ink, 0.35);
            (dim, lerp(dim, palette.canvas, 0.6))
        }
        _ if active => (deep, deep),
        _ => {
            let washed = lerp(deep, Color32::WHITE, if palette.dark { 0.25 } else { 0.45 });
            (washed, washed)
        }
    }
}

/// The border and title bar's background.
pub fn paint_frame(
    painter: &Painter,
    full: Rect,
    bar: Rect,
    inner: Rect,
    palette: &Palette,
    state: WindowState,
) {
    let dark = palette.dark;
    let open = !state.maximized;
    match theme::skin().family() {
        Family::Modern => {}
        Family::Retro => {
            painter.rect_filled(full, CornerRadius::ZERO, palette.canvas);
            if open {
                retro_edge(painter, full, palette.canvas, false, 1.0);
            }
            let (from, to) = title_colors(palette, state.active);
            gradient(painter, bar, CornerRadius::ZERO, from, to, true);
        }
        Family::Luna => {
            let (blue, _) = title_colors(palette, state.active);
            // The sides fade from the title bar's blue to a deeper one.
            painter.rect_filled(full, CornerRadius::ZERO, darker(blue, 0.12));
            if open {
                painter.rect_stroke(
                    full,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, darker(blue, 0.45)),
                    StrokeKind::Inside,
                );
            }
            bands(
                painter,
                bar.expand2(vec2(if open { 4.0 } else { 0.0 }, 0.0)),
                top_corners(CornerRadius::same(if open { 8 } else { 0 })),
                &[
                    (0.0, lerp(blue, Color32::WHITE, 0.55)),
                    (0.08, lighter(blue, 0.25)),
                    (0.22, blue),
                    (0.55, lighter(blue, 0.1)),
                    (0.86, blue),
                    (1.0, darker(blue, 0.22)),
                ],
            );
        }
        Family::Glass => super::aero::paint_frame(
            painter,
            full,
            inner,
            palette,
            state,
            theme::skin().style == Style::Vista,
        ),
        Family::Gel if theme::skin().style == Style::Panther => {
            // Brushed metal from the title bar down, in one piece.
            painter.rect_filled(full, CornerRadius::ZERO, palette.canvas);
            gradient(
                painter,
                bar,
                CornerRadius::ZERO,
                Color32::from_white_alpha(if dark { 10 } else { 70 }),
                Color32::TRANSPARENT,
                false,
            );
            super::skin::brushed_metal(painter, full, dark);
            if open {
                painter.rect_stroke(
                    full,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, if dark { Color32::BLACK } else { gray(0x6c) }),
                    StrokeKind::Inside,
                );
            }
        }
        Family::Gel if theme::skin().style == Style::Tiger => {
            // A plain grey title bar, lit along its top edge.
            let (top, bottom, line) = match (dark, state.active) {
                (true, _) => (gray(0x4a), gray(0x2e), Color32::BLACK),
                (false, true) => (gray(0xe8), gray(0xc0), gray(0x86)),
                (false, false) => (gray(0xf2), gray(0xe0), gray(0xa8)),
            };
            painter.rect_filled(full, CornerRadius::ZERO, palette.canvas);
            gradient(painter, bar, CornerRadius::ZERO, top, bottom, false);
            painter.hline(
                bar.x_range(),
                bar.top() + 0.5,
                Stroke::new(1.0, Color32::from_white_alpha(if dark { 30 } else { 200 })),
            );
            painter.hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, line));
            if open {
                painter.rect_stroke(
                    full,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, if dark { Color32::BLACK } else { gray(0x7c) }),
                    StrokeKind::Inside,
                );
            }
        }
        Family::Gel => {
            let (top, bottom) = if dark {
                (gray(0x4a), gray(0x30))
            } else if state.active {
                (gray(0xf2), gray(0xcc))
            } else {
                (gray(0xf6), gray(0xe2))
            };
            painter.rect_filled(full, CornerRadius::ZERO, palette.canvas);
            gradient(painter, bar, CornerRadius::ZERO, top, bottom, false);
            // Aqua's pinstripes run through the title bar.
            let stripe = if dark {
                Color32::from_white_alpha(8)
            } else {
                Color32::from_white_alpha(110)
            };
            let mut y = bar.top() + 1.0;
            while y < bar.bottom() - 1.0 {
                painter.hline(bar.x_range(), y + 0.5, Stroke::new(1.0, stripe));
                y += 2.0;
            }
            painter.hline(
                bar.x_range(),
                bar.bottom() - 0.5,
                Stroke::new(1.0, if dark { Color32::BLACK } else { gray(0x8a) }),
            );
            if open {
                painter.rect_stroke(
                    full,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, if dark { Color32::BLACK } else { gray(0x7c) }),
                    StrokeKind::Inside,
                );
            }
        }
        Family::Platinum => {
            let face = palette.canvas;
            painter.rect_filled(full, CornerRadius::ZERO, face);
            if open {
                painter.rect_stroke(
                    full,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, if dark { gray(0x0a) } else { Color32::BLACK }),
                    StrokeKind::Inside,
                );
                bevel(
                    painter,
                    full.shrink(1.0),
                    lighter(face, 0.8),
                    darker(face, 0.3),
                );
            }
            // The content sits sunk into the frame.
            bevel(
                painter,
                inner.expand(1.0),
                darker(face, 0.3),
                lighter(face, 0.8),
            );
            if state.active {
                // Mac OS 9's lines, cleared where the boxes and title sit.
                let mut y = bar.top() + 4.0;
                while y < bar.bottom() - 4.0 {
                    let line = |at: f32| {
                        Rect::from_min_max(
                            pos2(bar.left() + 4.0, at),
                            pos2(bar.right() - 4.0, at + 1.0),
                        )
                    };
                    painter.rect_filled(line(y), CornerRadius::ZERO, darker(face, 0.28));
                    painter.rect_filled(line(y + 1.0), CornerRadius::ZERO, lighter(face, 0.7));
                    y += 3.0;
                }
            }
        }
    }
}

/// One caption button.
pub fn paint_caption(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    press: Press,
    palette: &Palette,
    state: WindowState,
) {
    let dark = palette.dark;
    match theme::skin().family() {
        Family::Modern => {}
        Family::Retro => {
            let face = palette.canvas;
            painter.rect_filled(rect, CornerRadius::ZERO, face);
            retro_edge(painter, rect, face, press.pressed, 1.0);
            let shift = if press.pressed {
                vec2(1.0, 1.0)
            } else {
                vec2(0.0, 0.0)
            };
            glyph(
                painter,
                rect.translate(shift),
                caption,
                state.maximized,
                palette.ink,
                1.6,
            );
        }
        Family::Luna => {
            let close = caption == Caption::Close;
            let base = if close {
                rgb(0xd8462a)
            } else {
                let (blue, _) = title_colors(palette, state.active);
                lighter(blue, 0.12)
            };
            let base = if press.pressed {
                darker(base, 0.2)
            } else if press.hovered {
                lighter(base, 0.18)
            } else {
                base
            };
            let radius = CornerRadius::same(3);
            gradient(painter, rect, radius, lighter(base, 0.35), base, false);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, Color32::from_white_alpha(225)),
                StrokeKind::Inside,
            );
            glyph(painter, rect, caption, state.maximized, Color32::WHITE, 2.0);
        }
        Family::Glass => super::aero::paint_caption(
            painter,
            rect,
            caption,
            press,
            state,
            theme::skin().style == Style::Vista,
        ),
        Family::Gel => {
            let look = if !state.active && !press.group {
                super::gel::Look::Asleep
            } else if press.pressed {
                super::gel::Look::Pressed
            } else {
                super::gel::Look::Lit
            };
            let jaguar = theme::skin().style == Style::Gel;
            super::gel::paint_light(painter, rect, caption, jaguar, look, press.group);
        }
        Family::Platinum => {
            if !state.active {
                return;
            }
            let face = palette.canvas;
            // Clear the lines around the box.
            painter.rect_filled(rect.expand(3.0), CornerRadius::ZERO, face);
            let outline = if dark { gray(0x10) } else { gray(0x40) };
            let (top, bottom) = if press.pressed {
                (darker(face, 0.35), darker(face, 0.15))
            } else {
                (lighter(face, 0.7), darker(face, 0.08))
            };
            gradient(painter, rect, CornerRadius::ZERO, top, bottom, false);
            bevel(
                painter,
                rect.shrink(1.0),
                lighter(face, 0.85),
                darker(face, 0.25),
            );
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, outline),
                StrokeKind::Inside,
            );
            let mark = Stroke::new(1.0, outline);
            match caption {
                Caption::Close => {}
                Caption::Maximize => {
                    // A small box in the corner of the big one.
                    let small = Rect::from_min_size(rect.min + vec2(1.5, 1.5), vec2(6.0, 6.0));
                    painter.rect_stroke(small, CornerRadius::ZERO, mark, StrokeKind::Inside);
                }
                Caption::Minimize => {
                    painter.hline(rect.x_range(), rect.center().y - 1.0, mark);
                    painter.hline(rect.x_range(), rect.center().y + 1.0, mark);
                }
            }
        }
    }
}

/// Windows' caption symbols in `ink`, `weight` thick.
fn glyph(
    painter: &Painter,
    rect: Rect,
    caption: Caption,
    maximized: bool,
    ink: Color32,
    weight: f32,
) {
    let c = rect.center();
    let stroke = Stroke::new(weight, ink);
    let unit = (rect.height() / 16.0).max(0.8);
    match caption {
        Caption::Minimize => {
            let y = c.y + 3.0 * unit;
            painter.line_segment(
                [pos2(c.x - 4.0 * unit, y), pos2(c.x + 2.0 * unit, y)],
                stroke,
            );
        }
        Caption::Maximize => {
            let size = 8.0 * unit;
            let box_at = |min: egui::Pos2| {
                let r = Rect::from_min_size(min, vec2(size, size * 0.85));
                painter.rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(weight * 0.6, ink),
                    StrokeKind::Inside,
                );
                painter.hline(r.x_range(), r.top() + weight * 0.6, stroke);
            };
            if maximized {
                let small = size * 0.75;
                let back = c + vec2(-small / 2.0 + 1.5 * unit, -small / 2.0 - 1.5 * unit);
                let front = c + vec2(-small / 2.0 - 1.0 * unit, -small / 2.0 + 1.0 * unit);
                for min in [back, front] {
                    let r = Rect::from_min_size(min, vec2(small, small * 0.85));
                    painter.rect_stroke(
                        r,
                        CornerRadius::ZERO,
                        Stroke::new(weight * 0.6, ink),
                        StrokeKind::Inside,
                    );
                    painter.hline(r.x_range(), r.top() + weight * 0.5, stroke);
                }
            } else {
                box_at(c - vec2(size / 2.0, size * 0.85 / 2.0));
            }
        }
        Caption::Close => {
            let arm = 3.5 * unit;
            painter.line_segment([c - vec2(arm, arm), c + vec2(arm, arm)], stroke);
            painter.line_segment([c + vec2(-arm, arm), c + vec2(arm, -arm)], stroke);
        }
    }
}

/// The window's icon and title in the title bar.
fn paint_title(ui: &Ui, bar: Rect, title: &str, palette: &Palette, state: WindowState) {
    let painter = ui.painter();
    let style = theme::skin().family();
    let (from, _) = title_colors(palette, state.active);
    let (font, ink, centered) = match style {
        Family::Modern => return,
        Family::Retro => (
            theme::bold(13.0),
            if state.active {
                ink_on(from)
            } else {
                lerp(palette.canvas, Color32::WHITE, 0.6)
            },
            false,
        ),
        Family::Luna => (theme::title(15.0), Color32::WHITE, false),
        Family::Glass => (
            theme::font(12.5),
            if palette.dark {
                Color32::WHITE
            } else {
                Color32::BLACK
            },
            false,
        ),
        Family::Gel => (
            theme::font(13.0),
            if state.active {
                palette.ink
            } else {
                palette.muted
            },
            true,
        ),
        Family::Platinum => (
            theme::bold(13.0),
            if state.active {
                palette.ink
            } else {
                palette.muted
            },
            true,
        ),
    };
    let galley = painter.layout_no_wrap(title.to_owned(), font, ink);
    let icon = 16.0;
    let (icon_at, text_at) = if centered {
        let text = pos2(
            bar.center().x - galley.size().x / 2.0,
            bar.center().y - galley.size().y / 2.0,
        );
        (None, text)
    } else {
        // Vista's and 7's icon sits 8 pixels in from the window's edge,
        // or the screen's when maximized.
        let glass = style == Family::Glass && bar.left() > 0.5;
        let left = bar.left() + if glass { -2.0 } else { 4.0 };
        let icon_rect =
            Rect::from_center_size(pos2(left + icon / 2.0, bar.center().y), vec2(icon, icon));
        (
            Some(icon_rect),
            pos2(
                icon_rect.right() + 5.0,
                bar.center().y - galley.size().y / 2.0,
            ),
        )
    };
    let area = Rect::from_min_size(text_at, galley.size());
    match style {
        Family::Luna => {
            painter.galley_with_override_text_color(
                text_at + vec2(1.0, 1.0),
                galley.clone(),
                faded(darker(from, 0.6), 0.85),
            );
        }
        Family::Glass => super::aero::paint_title_glow(painter, area, palette.dark),
        Family::Gel if !palette.dark => {
            painter.galley_with_override_text_color(
                text_at + vec2(0.0, 1.0),
                galley.clone(),
                Color32::from_white_alpha(160),
            );
        }
        Family::Platinum if state.active => {
            painter.rect_filled(
                area.expand2(vec2(6.0, 0.0)),
                CornerRadius::ZERO,
                palette.canvas,
            );
        }
        _ => {}
    }
    if let Some(icon_rect) = icon_at {
        egui::Image::new(egui::include_image!("../../assets/logo.svg"))
            .fit_to_exact_size(icon_rect.size())
            .paint_at(ui, icon_rect);
    }
    painter.galley_with_override_text_color(text_at, galley, ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    use eframe::egui::Shape;

    /// Every style but Modern draws its frame.
    const FRAMED: [Style; 9] = [
        Style::Win95,
        Style::Retro,
        Style::Luna,
        Style::Vista,
        Style::Glass,
        Style::Platinum,
        Style::Gel,
        Style::Panther,
        Style::Tiger,
    ];

    fn in_style<R>(style: Style, run: impl FnOnce() -> R) -> R {
        let skin = theme::Skin {
            style,
            ..theme::Skin::default()
        };
        theme::with_skin(skin, run)
    }

    fn palette(style: Style, dark: bool) -> Palette {
        let scheme = theme::Scheme {
            style,
            accent: style.accent(),
            ..theme::Scheme::default()
        };
        Palette::of(scheme, dark)
    }

    fn window() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 400.0))
    }

    fn states() -> [WindowState; 4] {
        [
            WindowState {
                maximized: false,
                active: true,
            },
            WindowState {
                maximized: false,
                active: false,
            },
            WindowState {
                maximized: true,
                active: true,
            },
            WindowState {
                maximized: true,
                active: false,
            },
        ]
    }

    /// Everything a shape could touch, blur included.
    fn reach(shape: &Shape) -> Rect {
        match shape {
            Shape::Rect(rect) => rect.rect.expand(rect.blur_width),
            shape => shape.visual_bounding_rect(),
        }
    }

    #[test]
    fn modern_keeps_the_system_frame_and_the_rest_draw_their_own() {
        assert_eq!(metrics(Style::Modern), None);
        assert!(captions(Style::Modern, window()).is_empty());
        for style in FRAMED {
            let metrics = metrics(style).unwrap();
            assert!(metrics.title >= 20.0, "{style:?}");
            assert!(metrics.side >= 1.0, "{style:?}");
        }
    }

    #[test]
    fn the_title_bar_sits_on_top_and_the_app_under_it() {
        for style in FRAMED {
            let metrics = metrics(style).unwrap();
            let (bar, inner) = layout(metrics, window(), false);
            assert_eq!(bar.height(), metrics.title, "{style:?}");
            assert_eq!(bar.top(), metrics.top, "{style:?}");
            assert_eq!(inner.top(), bar.bottom() + metrics.below, "{style:?}");
            assert_eq!(inner.left(), metrics.side, "{style:?}");
            assert_eq!(inner.right(), window().right() - metrics.side, "{style:?}");
            assert_eq!(
                inner.bottom(),
                window().bottom() - metrics.side,
                "{style:?}"
            );
            // Maximized, the borders go and the bar reaches the screen's top.
            let (bar, inner) = layout(metrics, window(), true);
            assert_eq!(bar.top(), 0.0, "{style:?}");
            assert_eq!((bar.left(), bar.right()), (0.0, 640.0), "{style:?}");
            assert_eq!((inner.left(), inner.right()), (0.0, 640.0), "{style:?}");
            assert_eq!(inner.bottom(), 400.0, "{style:?}");
        }
    }

    #[test]
    fn every_frame_has_its_three_buttons_where_its_desktop_put_them() {
        for style in FRAMED {
            let (bar, _) = layout(metrics(style).unwrap(), window(), false);
            let buttons = captions(style, bar);
            let mut kinds: Vec<_> = buttons.iter().map(|(caption, _)| *caption).collect();
            kinds.sort_by_key(|caption| *caption as u8);
            assert_eq!(
                kinds,
                [Caption::Minimize, Caption::Maximize, Caption::Close],
                "{style:?}"
            );
            for (i, (caption, rect)) in buttons.iter().enumerate() {
                assert!(bar.contains_rect(*rect), "{style:?} {caption:?}");
                assert!(rect.width() >= 12.0, "{style:?} {caption:?}");
                // Left to right, without overlapping.
                if let Some((_, next)) = buttons.get(i + 1) {
                    assert!(rect.right() <= next.left(), "{style:?} {caption:?}");
                }
            }
            let at = |kind| buttons.iter().find(|(c, _)| *c == kind).unwrap().1;
            let close = at(Caption::Close);
            match style.family() {
                // Windows closes at the far right.
                Family::Retro | Family::Luna | Family::Glass => {
                    assert!(close.right() > bar.right() - 8.0, "{style:?}");
                    assert!(close.left() > at(Caption::Maximize).right() - 0.5);
                }
                // The Mac closes at the left.
                _ => assert!(close.left() < bar.left() + 12.0, "{style:?}"),
            }
        }
    }

    #[test]
    fn buttons_say_and_do_what_they_are() {
        assert_eq!(Caption::Minimize.label(false), "Minimize window");
        assert_eq!(Caption::Maximize.label(false), "Maximize window");
        assert_eq!(Caption::Maximize.label(true), "Restore window");
        assert_eq!(Caption::Close.label(true), "Close window");
        assert_eq!(
            Caption::Minimize.command(false),
            ViewportCommand::Minimized(true)
        );
        assert_eq!(
            Caption::Maximize.command(false),
            ViewportCommand::Maximized(true)
        );
        assert_eq!(
            Caption::Maximize.command(true),
            ViewportCommand::Maximized(false)
        );
        assert_eq!(Caption::Close.command(false), ViewportCommand::Close);
    }

    #[test]
    fn title_bars_take_the_accent_and_dim_when_the_window_is_behind() {
        // Windows 98's own navy fades to its own light blue.
        let navy = palette(Style::Retro, false);
        assert_eq!(navy.deep, rgb(0x000080));
        let (from, to) = in_style(Style::Retro, || title_colors(&navy, true));
        assert_eq!((from, to), (rgb(0x000080), rgb(0x1084d0)));
        for style in FRAMED {
            for dark in [false, true] {
                let palette = palette(style, dark);
                in_style(style, || {
                    let active = title_colors(&palette, true);
                    let behind = title_colors(&palette, false);
                    assert_eq!(active.0, palette.deep, "{style:?}");
                    assert_ne!(active, behind, "{style:?} dark={dark}");
                });
            }
        }
        // Another accent fades to a lighter self.
        let mut green = palette(Style::Retro, false);
        green.deep = rgb(0x006000);
        let (from, to) = in_style(Style::Retro, || title_colors(&green, true));
        assert_eq!(from, green.deep);
        assert_eq!(to, lerp(green.deep, Color32::WHITE, 0.45));
    }

    #[test]
    fn windows_95_keeps_its_title_bar_one_solid_colour() {
        let navy = palette(Style::Win95, false);
        let (active, behind) = in_style(Style::Win95, || {
            (title_colors(&navy, true), title_colors(&navy, false))
        });
        assert_eq!(active, (rgb(0x000080), rgb(0x000080)));
        assert_eq!(behind.0, behind.1);
        assert_ne!(behind, active);
        // 98 fades the same navy.
        let (from, to) = in_style(Style::Retro, || title_colors(&navy, true));
        assert_ne!(from, to);
    }

    #[test]
    fn windows_7_hangs_bigger_buttons_than_vista() {
        let (bar, _) = layout(metrics(Style::Glass).unwrap(), window(), false);
        let size = |style| {
            captions(style, bar)
                .iter()
                .map(|(caption, rect)| (*caption, rect.size()))
                .collect::<Vec<_>>()
        };
        for ((caption, seven), (_, vista)) in size(Style::Glass).into_iter().zip(size(Style::Vista))
        {
            assert!(seven.x > vista.x, "{caption:?}");
            assert!(seven.y > vista.y, "{caption:?}");
        }
    }

    #[test]
    fn versions_of_one_desktop_draw_their_own_frame() {
        let frame = |style: Style, dark: bool| {
            let palette = palette(style, dark);
            let metrics = metrics(style).unwrap();
            let (bar, inner) = layout(metrics, window(), false);
            let state = WindowState {
                maximized: false,
                active: true,
            };
            in_style(style, || {
                theme::painted(|ui| {
                    paint_frame(ui.painter(), window(), bar, inner, &palette, state);
                })
            })
        };
        let text = |style, dark| format!("{:?}", frame(style, dark));
        for dark in [false, true] {
            assert_ne!(text(Style::Vista, dark), text(Style::Glass, dark));
            let macs = [Style::Gel, Style::Panther, Style::Tiger].map(|s| text(s, dark));
            assert_ne!(macs[0], macs[1]);
            assert_ne!(macs[0], macs[2]);
            assert_ne!(macs[1], macs[2]);
            // Panther's metal is a row of grain per pixel down the window.
            let rows = |style| {
                frame(style, dark)
                    .iter()
                    .map(|shape| match shape {
                        Shape::Mesh(mesh) => mesh.vertices.len(),
                        _ => 0,
                    })
                    .sum::<usize>()
            };
            assert!(rows(Style::Panther) >= 4 * 400, "dark={dark}");
            assert!(rows(Style::Tiger) < 400, "dark={dark}");
        }
    }

    #[test]
    fn every_frame_draws_inside_the_window_in_every_state() {
        for style in FRAMED {
            for dark in [false, true] {
                let palette = palette(style, dark);
                for state in states() {
                    let at = format!("{style:?} dark={dark} {state:?}");
                    let metrics = metrics(style).unwrap();
                    let (bar, inner) = layout(metrics, window(), state.maximized);
                    let shapes = in_style(style, || {
                        theme::painted(|ui| {
                            let painter = ui.painter().clone();
                            paint_frame(&painter, window(), bar, inner, &palette, state);
                            for (caption, rect) in captions(style, bar) {
                                for press in [
                                    Press::default(),
                                    Press {
                                        hovered: true,
                                        pressed: true,
                                        group: true,
                                    },
                                ] {
                                    paint_caption(&painter, rect, caption, press, &palette, state);
                                }
                            }
                            paint_title(ui, bar, "PkgDeck", &palette, state);
                        })
                    });
                    assert!(shapes.len() > 5, "{at}");
                    for shape in &shapes {
                        // Glows may spill out; they're clipped by the window.
                        if matches!(shape, Shape::Rect(rect) if rect.blur_width > 0.0) {
                            continue;
                        }
                        let bounds = reach(shape);
                        assert!(
                            window().expand(1.0).contains_rect(bounds) || !bounds.is_positive(),
                            "{at}: {shape:?}"
                        );
                    }
                    // Windows 95 and 98 are square through and through.
                    if style.family() == Family::Retro {
                        assert!(
                            shapes.iter().all(|shape| !matches!(shape,
                                Shape::Rect(rect) if rect.corner_radius != CornerRadius::ZERO)),
                            "{at}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_edges_resize_and_the_corners_resize_both_ways() {
        let retro = metrics(Style::Retro).unwrap();
        let at = |x, y| edge_at(retro, window(), pos2(x, y));
        assert_eq!(at(320.0, 1.0), Some(ResizeDirection::North));
        assert_eq!(at(320.0, 399.0), Some(ResizeDirection::South));
        assert_eq!(at(1.0, 200.0), Some(ResizeDirection::West));
        assert_eq!(at(639.0, 200.0), Some(ResizeDirection::East));
        assert_eq!(at(1.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(639.0, 1.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(1.0, 399.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(639.0, 399.0), Some(ResizeDirection::SouthEast));
        // A corner reaches a little along each edge.
        assert_eq!(at(12.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(639.0, 388.0), Some(ResizeDirection::SouthEast));
        assert_eq!(at(16.0, 1.0), Some(ResizeDirection::North));
        assert_eq!(at(639.0, 384.0), Some(ResizeDirection::East));
        // Just inside the edge, and outside the window, nothing.
        assert_eq!(at(5.0, 200.0), None);
        assert_eq!(at(320.0, 5.0), None);
        assert_eq!(at(320.0, 200.0), None);
        assert_eq!(at(-1.0, 200.0), None);
        assert_eq!(at(641.0, 1.0), None);
        // Glass's thick border reaches further in.
        let glass = metrics(Style::Glass).unwrap();
        assert_eq!(
            edge_at(glass, window(), pos2(6.0, 200.0)),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            edge_at(glass, window(), pos2(18.0, 1.0)),
            Some(ResizeDirection::NorthWest)
        );
    }

    #[test]
    fn showing_the_frame_hands_the_app_the_room_under_the_bar() {
        for style in Style::ALL {
            let palette = palette(style, false);
            let mut room = Rect::NOTHING;
            in_style(style, || {
                theme::painted(|ui| {
                    room = show(ui, window(), "PkgDeck", &palette);
                    edges(ui, window());
                })
            });
            match metrics(style) {
                None => assert_eq!(room, window()),
                Some(metrics) => assert_eq!(room, layout(metrics, window(), false).1),
            }
        }
    }
}
