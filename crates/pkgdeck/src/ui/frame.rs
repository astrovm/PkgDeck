//! The window's own frame in the older styles: the title bar, its buttons
//! and the border, drawn the way each desktop drew its windows. Modern
//! keeps the system's frame, and so does a style on the desktop it copies.

use super::skin::{faded, gradient, gray, ink_on, lerp, retro_edge, rgb};
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

/// The desktop the window is on, as far as its frame goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Desktop {
    #[default]
    Other,
    Kde,
    Mac,
}

impl Desktop {
    /// The one PkgDeck is running on.
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::of(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref())
        }
    }
    /// The Linux desktop `XDG_CURRENT_DESKTOP` names.
    pub fn of(names: Option<&str>) -> Self {
        if crate::backdrop::is_kde(names) {
            Self::Kde
        } else {
            Self::Other
        }
    }
}

static DESKTOP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Tells the frame which desktop the window is on. Until then, as in
/// tests and previews, it's none in particular.
pub fn set_desktop(desktop: Desktop) {
    DESKTOP.store(desktop as u8, std::sync::atomic::Ordering::Relaxed);
}

pub fn desktop() -> Desktop {
    Desktop::from_code(DESKTOP.load(std::sync::atomic::Ordering::Relaxed))
}

impl Desktop {
    /// The desktop `set_desktop` stored as `code`.
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Kde,
            2 => Self::Mac,
            _ => Self::Other,
        }
    }
}

/// Whether the system draws the window's frame for `style` on `desktop`:
/// Modern always keeps it, and a style keeps the real thing on the
/// desktop it copies, window menu and settings included. GNOME draws no
/// frames for apps, so GNOME 51 draws its own there too.
pub fn system_frame(style: Style, desktop: Desktop) -> bool {
    match style.family() {
        Family::Modern => true,
        Family::Breeze => desktop == Desktop::Kde,
        Family::Liquid => desktop == Desktop::Mac,
        _ => false,
    }
}

/// The title bar the app lays out for `style` on `desktop`: the style's
/// own, none under the system's, but on the Mac macOS 27 keeps its bar
/// under the system's lights, so the sidebar still runs up behind them.
pub fn drawn(style: Style, desktop: Desktop) -> Option<Metrics> {
    if system_frame(style, desktop) && style.family() != Family::Liquid {
        None
    } else {
        metrics(style)
    }
}

/// The frame `style` draws, or None when it keeps the system's.
pub fn metrics(style: Style) -> Option<Metrics> {
    let (side, top, title, below) = match style.family() {
        Family::Modern => return None,
        // Windows 98: a raised edge all round, the title bar inside it.
        Family::Retro => (4.0, 4.0, 18.0, 2.0),
        // XP: a blue border, the title bar up to the top.
        Family::Luna => (4.0, 0.0, 30.0, 0.0),
        // Windows 7: 8 pixels of glass and two lines round the content,
        // the title on the glass.
        Family::Glass => (10.0, 0.0, 30.0, 2.0),
        // Aqua: a hairline and the grey title bar.
        Family::Gel => (1.0, 1.0, 22.0, 0.0),
        // Windows 11: a one pixel edge and a 32 pixel title bar of Mica.
        Family::Fluent => (1.0, 1.0, 32.0, 0.0),
        // macOS 27: no edge but a hairline, the lights in a short bar the
        // sidebar runs up behind.
        Family::Liquid => (0.0, 0.0, 52.0, 0.0),
        // KWin's Plastik and Oxygen: a thin border of the window's grey
        // under a title bar that spans it.
        Family::Plastik => (4.0, 0.0, 22.0, 0.0),
        Family::Oxygen => (4.0, 0.0, 26.0, 0.0),
        // Breeze: a hairline outline and a 30 pixel title bar.
        Family::Breeze => (1.0, 0.0, 30.0, 0.0),
        // Metacity's Clearlooks: a four pixel border.
        Family::Clearlooks => (4.0, 0.0, 24.0, 0.0),
        // GNOME: a hairline and a 47 pixel header bar.
        Family::Adwaita => (1.0, 0.0, 47.0, 0.0),
    };
    Some(Metrics {
        side,
        top,
        title,
        below,
    })
}

/// The radius the window's own corners take, as on its desktop: Tiger
/// rounds the title bar only, Windows 11 all four, macOS 27 more. Square
/// maximized, and in the styles that draw their corners some other way.
pub fn window_corners(maximized: bool) -> CornerRadius {
    if maximized {
        return CornerRadius::ZERO;
    }
    // The desktop's own radius, in pixels, times the person's roundness.
    let skin = theme::skin();
    let scaled =
        |radius: f32| CornerRadius::same((radius * skin.roundness.clamp(0.0, 2.0)).round() as u8);
    match skin.family() {
        Family::Gel => super::skin::top_corners(scaled(5.0)),
        Family::Plastik => super::skin::top_corners(scaled(3.0)),
        Family::Fluent => scaled(8.0),
        Family::Liquid => scaled(26.0),
        Family::Oxygen => scaled(5.0),
        Family::Breeze => scaled(4.0),
        // Metacity rounds the title bar's corners a pixel more than the
        // bottom's.
        Family::Clearlooks => CornerRadius {
            nw: scaled(4.0).nw,
            ne: scaled(4.0).ne,
            sw: scaled(3.0).sw,
            se: scaled(3.0).se,
        },
        Family::Adwaita => scaled(15.0),
        _ => CornerRadius::ZERO,
    }
}

/// Tells the app's own painting where the window's corners are.
fn publish_corners(ctx: &egui::Context, full: Rect, maximized: bool) {
    let corners = window_corners(maximized);
    ctx.data_mut(|data| data.insert_temp(Id::unique("window-corners"), (full, corners)));
}

/// The corners a rectangle of the app's own painting must take where it
/// reaches a corner of the window, so it doesn't poke out of the rounded
/// frame.
pub fn fit(ctx: &egui::Context, rect: Rect) -> CornerRadius {
    let Some((full, corners)) =
        ctx.data(|data| data.get_temp::<(Rect, CornerRadius)>(Id::unique("window-corners")))
    else {
        return CornerRadius::ZERO;
    };
    // A corner of `rect` at (x, y) from the window's, inside the same corner.
    let corner = |radius: u8, dx: f32, dy: f32| {
        if dx.abs() <= 1.5 && dy.abs() <= 1.5 {
            radius.saturating_sub(dx.abs().max(dy.abs()).round() as u8)
        } else {
            0
        }
    };
    CornerRadius {
        nw: corner(
            corners.nw,
            rect.left() - full.left(),
            rect.top() - full.top(),
        ),
        ne: corner(
            corners.ne,
            full.right() - rect.right(),
            rect.top() - full.top(),
        ),
        sw: corner(
            corners.sw,
            rect.left() - full.left(),
            full.bottom() - rect.bottom(),
        ),
        se: corner(
            corners.se,
            full.right() - rect.right(),
            full.bottom() - rect.bottom(),
        ),
    }
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
    show_on(ui, full, title, palette, desktop())
}

fn show_on(ui: &mut Ui, full: Rect, title: &str, palette: &Palette, desktop: Desktop) -> Rect {
    let ctx = ui.ctx().clone();
    let style = theme::skin().style;
    let system = system_frame(style, desktop);
    decorate(&ctx, system);
    let Some(metrics) = drawn(style, desktop) else {
        publish_corners(&ctx, full, true);
        return full;
    };
    let state = WindowState::of(&ctx);
    publish_corners(&ctx, full, state.maximized);
    let (bar, inner) = layout(metrics, full, state.maximized);
    let painter = ui.painter().clone();
    paint_frame(&painter, full, bar, inner, palette, state);
    // Dragging the title bar moves the window; a double click maximizes.
    let id = Id::unique("window-frame");
    let drag = ui.interact(bar, id.with("title"), Sense::click_and_drag());
    if drag.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!state.maximized));
    }
    // Under the system's frame its own buttons stand in.
    let buttons = if system { vec![] } else { captions(style, bar) };
    let hovered_group = buttons
        .iter()
        .any(|(_, rect)| ui.rect_contains_pointer(rect.expand(2.0)));
    for (caption, rect) in buttons {
        let response = ui.interact(rect, id.with(caption), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, caption.label(state.maximized))
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
pub fn edges(ui: &mut Ui, full: Rect, resizing_sidebar: bool) {
    edges_on(ui, full, resizing_sidebar, desktop());
}

fn edges_on(ui: &mut Ui, full: Rect, resizing_sidebar: bool, desktop: Desktop) {
    let ctx = ui.ctx().clone();
    let style = theme::skin().style;
    let state = WindowState::of(&ctx);
    if system_frame(style, desktop) || state.maximized {
        return;
    }
    if style.family() == Family::Liquid {
        // The sidebar and page reach the edge, so the hairline goes on top.
        let palette = Palette::current(&ctx);
        let edge = if state.active {
            palette.strong_line
        } else {
            palette.line
        };
        ui.painter().rect_stroke(
            full,
            window_corners(false),
            Stroke::new(1.0, edge),
            StrokeKind::Inside,
        );
    }
    // At the divider's endpoints the sidebar owns the drag, even where
    // its handle overlaps the window's outer resize edge.
    if resizing_sidebar {
        return;
    }
    let Some(direction) = ctx
        .pointer_hover_pos()
        .and_then(|pointer| edge_at(style, full, pointer))
    else {
        return;
    };
    ctx.set_cursor_icon(cursor(direction));
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Which way the window resizes from `pointer`: anywhere near the edge,
/// and diagonally near a corner, but not on a title bar button that
/// reaches into the edge.
pub fn edge_at(style: Style, full: Rect, pointer: egui::Pos2) -> Option<ResizeDirection> {
    let metrics = metrics(style)?;
    if !full.contains(pointer) {
        return None;
    }
    let (bar, _) = layout(metrics, full, false);
    if captions(style, bar)
        .iter()
        .any(|(_, rect)| rect.contains(pointer))
    {
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
    let id = Id::unique("window-decorated");
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
        Family::Fluent => super::fluent::captions(bar),
        Family::Liquid => super::liquid::captions(bar),
        Family::Retro => {
            // 16 by 14, 2 in from the bar: minimize and maximize side by
            // side, close 2 apart.
            let size = vec2(16.0, 14.0);
            let close =
                Rect::from_min_size(pos2(bar.right() - 2.0 - size.x, bar.top() + 2.0), size);
            let maximize = close.translate(vec2(-size.x - 2.0, 0.0));
            let minimize = maximize.translate(vec2(-size.x, 0.0));
            vec![
                (Caption::Minimize, minimize),
                (Caption::Maximize, maximize),
                (Caption::Close, close),
            ]
        }
        Family::Luna => super::luna::captions(bar),
        Family::Glass => super::aero::captions(bar),
        Family::Gel => {
            // Close, minimize and zoom at the left.
            let (size, left, pitch) = (vec2(14.0, 14.0), 8.0, 21.0);
            let at = |i: f32| square(pos2(bar.left() + left + size.x / 2.0 + i * pitch, y), size);
            vec![
                (Caption::Close, at(0.0)),
                (Caption::Minimize, at(1.0)),
                (Caption::Maximize, at(2.0)),
            ]
        }
        Family::Plastik => super::plastik::captions(bar),
        Family::Oxygen => super::oxygen::captions(bar),
        Family::Breeze => super::breeze::captions(bar),
        Family::Clearlooks => super::clearlooks::captions(bar),
        Family::Adwaita => super::adwaita::captions(bar),
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

/// The title bar's colour where the style takes it from the accent, or a
/// neutral grey when the window bar is not tinted.
pub(super) fn title_colors(palette: &Palette, active: bool) -> (Color32, Color32) {
    let bar = palette.bar;
    match theme::skin().family() {
        Family::Retro if active => {
            // Windows 98's navy fades to its light blue; another colour
            // fades to a lighter self.
            let end = if bar == rgb(0x000080) {
                rgb(0x1084d0)
            } else {
                lerp(bar, Color32::WHITE, 0.45)
            };
            (bar, end)
        }
        Family::Retro => {
            let dim = lerp(palette.canvas, palette.ink, 0.35);
            (dim, lerp(dim, palette.canvas, 0.6))
        }
        _ if active => (bar, bar),
        _ => {
            let washed = lerp(bar, Color32::WHITE, if palette.dark { 0.25 } else { 0.45 });
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
        Family::Fluent => super::fluent::paint_frame(painter, full, palette, state),
        Family::Liquid => {
            painter.rect_filled(full, window_corners(state.maximized), palette.canvas);
            // The sidebar runs up behind the lights, as far as the page.
            let sidebar = painter.ctx().data(|data| {
                data.get_temp::<f32>(Id::unique("sidebar-extent"))
                    .unwrap_or(0.0)
            });
            let behind = Rect::from_min_max(
                full.min,
                pos2((full.left() + sidebar).min(full.right()), bar.bottom()),
            );
            painter.rect_filled(
                behind,
                fit(painter.ctx(), behind),
                super::skin::sidebar_color(palette),
            );
            // The sidebar's divider runs up behind the lights too, so the
            // border has no gap between the title bar and the sidebar.
            painter.vline(
                behind.right() - 0.5,
                full.top()..=bar.bottom(),
                Stroke::new(1.0, palette.line),
            );
        }
        Family::Retro => {
            painter.rect_filled(full, CornerRadius::ZERO, palette.canvas);
            if open {
                retro_edge(painter, full, palette.canvas, false, 1.0);
            }
            let (from, to) = title_colors(palette, state.active);
            gradient(painter, bar, CornerRadius::ZERO, from, to, true);
        }
        Family::Luna => super::luna::paint_frame(painter, full, bar, palette.bar, state, dark),
        Family::Glass => super::aero::paint_frame(painter, full, inner, palette, state),
        Family::Gel => {
            // A plain grey title bar, lit along its top edge.
            let (top, bottom, line) = match (dark, state.active) {
                (true, _) => (gray(0x4a), gray(0x2e), Color32::BLACK),
                (false, true) => (gray(0xe8), gray(0xca), gray(0x8c)),
                (false, false) => (gray(0xf2), gray(0xe0), gray(0xa8)),
            };
            let corners = window_corners(state.maximized);
            painter.rect_filled(full, corners, palette.canvas);
            gradient(painter, bar, corners, top, bottom, false);
            // The highlight stops where the corners turn.
            let lit = bar.x_range().shrink(f32::from(corners.nw));
            painter.hline(
                lit,
                bar.top() + 0.5,
                Stroke::new(1.0, Color32::from_white_alpha(if dark { 30 } else { 200 })),
            );
            painter.hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, line));
            if open {
                painter.rect_stroke(
                    full,
                    corners,
                    Stroke::new(1.0, if dark { Color32::BLACK } else { gray(0x7c) }),
                    StrokeKind::Inside,
                );
            }
        }
        Family::Plastik => super::plastik::paint_frame(painter, full, bar, palette, state),
        Family::Oxygen => super::oxygen::paint_frame(painter, full, bar, inner, palette, state),
        Family::Breeze => super::breeze::paint_frame(painter, full, bar, palette, state),
        Family::Clearlooks => super::clearlooks::paint_frame(painter, full, bar, palette, state),
        Family::Adwaita => super::adwaita::paint_frame(painter, full, bar, palette, state),
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
        Family::Fluent => {
            super::fluent::paint_caption(painter, rect, caption, press, palette, state)
        }
        Family::Liquid => super::liquid::paint_light(painter, rect, caption, press, dark, state),
        Family::Retro => {
            let face = palette.canvas;
            painter.rect_filled(rect, CornerRadius::ZERO, face);
            retro_edge(painter, rect, face, press.pressed, 1.0);
            let shift = if press.pressed {
                vec2(1.0, 1.0)
            } else {
                vec2(0.0, 0.0)
            };
            retro_glyph(
                painter,
                rect.min + shift,
                caption,
                state.maximized,
                palette.ink,
                face,
            );
        }
        Family::Luna => {
            super::luna::paint_caption(painter, rect, caption, press, state, palette.deep, dark)
        }
        Family::Glass => super::aero::paint_caption(painter, rect, caption, press, state),
        Family::Gel => {
            let look = if !state.active && !press.group {
                super::gel::Look::Asleep
            } else if press.pressed {
                super::gel::Look::Pressed
            } else {
                super::gel::Look::Lit
            };
            super::gel::paint_light(painter, rect, caption, look, press.group);
        }
        Family::Plastik => {
            super::plastik::paint_caption(painter, rect, caption, press, palette, state)
        }
        Family::Oxygen => {
            super::oxygen::paint_caption(painter, rect, caption, press, palette, state)
        }
        Family::Breeze => {
            super::breeze::paint_caption(painter, rect, caption, press, palette, state)
        }
        Family::Clearlooks => {
            super::clearlooks::paint_caption(painter, rect, caption, press, palette, state)
        }
        Family::Adwaita => {
            super::adwaita::paint_caption(painter, rect, caption, press, palette, state)
        }
    }
}

/// The caption symbols most desktops share, `half` from the middle to
/// their edge: a bar low down, a window, two windows, and a cross.
pub(super) fn plain_glyph(
    painter: &Painter,
    center: egui::Pos2,
    caption: Caption,
    maximized: bool,
    half: f32,
    stroke: Stroke,
) {
    match caption {
        Caption::Minimize => {
            painter.hline(
                center.x - half..=center.x + half,
                center.y + half - stroke.width / 2.0,
                stroke,
            );
        }
        Caption::Maximize if maximized => {
            // Two windows, the front one a corner lower.
            let side = half * 1.5;
            let front = Rect::from_min_size(
                pos2(center.x - half, center.y + half - side),
                vec2(side, side),
            );
            let back = front.translate(vec2(half * 2.0 - side, side - half * 2.0));
            painter.rect_stroke(back, CornerRadius::ZERO, stroke, StrokeKind::Inside);
            painter.rect_stroke(front, CornerRadius::ZERO, stroke, StrokeKind::Inside);
        }
        Caption::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(center, vec2(half * 2.0, half * 2.0)),
                CornerRadius::ZERO,
                stroke,
                StrokeKind::Inside,
            );
        }
        Caption::Close => {
            painter.line_segment(
                [center + vec2(-half, -half), center + vec2(half, half)],
                stroke,
            );
            painter.line_segment(
                [center + vec2(-half, half), center + vec2(half, -half)],
                stroke,
            );
        }
    }
}

/// KDE's caption symbols since Oxygen, `half` from the middle to their
/// edge: a chevron down, a chevron up, a diamond to restore, and a cross.
pub(super) fn chevron_glyph(
    painter: &Painter,
    center: egui::Pos2,
    caption: Caption,
    maximized: bool,
    half: f32,
    stroke: Stroke,
) {
    let at = |x: f32, y: f32| center + vec2(x * half, y * half);
    match caption {
        Caption::Minimize => {
            painter.add(egui::Shape::line(
                vec![at(-1.0, -0.5), at(0.0, 0.5), at(1.0, -0.5)],
                stroke,
            ));
        }
        Caption::Maximize if maximized => {
            painter.add(egui::Shape::closed_line(
                vec![at(0.0, -1.0), at(1.0, 0.0), at(0.0, 1.0), at(-1.0, 0.0)],
                stroke,
            ));
        }
        Caption::Maximize => {
            painter.add(egui::Shape::line(
                vec![at(-1.0, 0.5), at(0.0, -0.5), at(1.0, 0.5)],
                stroke,
            ));
        }
        Caption::Close => plain_glyph(painter, center, caption, maximized, half, stroke),
    }
}

/// Windows 98's caption symbols, pixel for pixel, in a 16 by
/// 14 button whose top left is `at`.
fn retro_glyph(
    painter: &Painter,
    at: egui::Pos2,
    caption: Caption,
    maximized: bool,
    ink: Color32,
    face: Color32,
) {
    let px = |x: f32, y: f32, w: f32, h: f32| {
        painter.rect_filled(
            Rect::from_min_size(at + vec2(x, y), vec2(w, h)),
            CornerRadius::ZERO,
            ink,
        );
    };
    // A window: its outline and a thick top.
    let window = |x: f32, y: f32, w: f32, h: f32, top: f32| {
        px(x, y, w, top);
        px(x, y + h - 1.0, w, 1.0);
        px(x, y, 1.0, h);
        px(x + w - 1.0, y, 1.0, h);
    };
    match caption {
        Caption::Minimize => px(4.0, 9.0, 6.0, 2.0),
        Caption::Maximize if maximized => {
            window(5.0, 2.0, 6.0, 6.0, 2.0);
            // The front window hides the back one's corner.
            painter.rect_filled(
                Rect::from_min_size(at + vec2(3.0, 5.0), vec2(6.0, 6.0)),
                CornerRadius::ZERO,
                face,
            );
            window(3.0, 5.0, 6.0, 6.0, 2.0);
        }
        Caption::Maximize => window(3.0, 2.0, 9.0, 9.0, 2.0),
        Caption::Close => {
            // Two pixels wide, crossing in the middle.
            let rows: [&[(f32, f32)]; 7] = [
                &[(0.0, 2.0), (6.0, 2.0)],
                &[(1.0, 2.0), (5.0, 2.0)],
                &[(2.0, 4.0)],
                &[(3.0, 2.0)],
                &[(2.0, 4.0)],
                &[(1.0, 2.0), (5.0, 2.0)],
                &[(0.0, 2.0), (6.0, 2.0)],
            ];
            for (y, spans) in rows.iter().enumerate() {
                for &(x, w) in *spans {
                    px(4.0 + x, 3.0 + y as f32, w, 1.0);
                }
            }
        }
    }
}

/// The window's icon and title in the title bar.
fn paint_title(ui: &Ui, bar: Rect, title: &str, palette: &Palette, state: WindowState) {
    let painter = ui.painter();
    let style = theme::skin().family();
    let (from, _) = title_colors(palette, state.active);
    // The title's face and colour, whether it's centred, and whether the
    // window's icon sits at the bar's left.
    let (font, ink, centered, icon) = match style {
        Family::Modern | Family::Liquid => return,
        Family::Fluent => (
            theme::font(12.0),
            if state.active {
                palette.ink
            } else {
                palette.muted
            },
            false,
            true,
        ),
        Family::Retro => (
            theme::bold(12.0),
            if state.active {
                ink_on(from)
            } else {
                lerp(palette.canvas, Color32::WHITE, 0.6)
            },
            false,
            true,
        ),
        Family::Luna => (theme::title(15.0), Color32::WHITE, false, true),
        Family::Glass => (
            theme::font(12.5),
            if palette.dark {
                Color32::WHITE
            } else {
                Color32::BLACK
            },
            false,
            true,
        ),
        Family::Gel => (
            theme::font(13.0),
            if state.active {
                palette.ink
            } else {
                palette.muted
            },
            true,
            false,
        ),
        Family::Plastik => (
            theme::bold(12.0),
            if state.active {
                ink_on(from)
            } else {
                lerp(ink_on(from), from, 0.35)
            },
            false,
            true,
        ),
        Family::Oxygen | Family::Breeze => (
            if style == Family::Oxygen {
                theme::bold(12.0)
            } else {
                theme::font(12.5)
            },
            if state.active {
                palette.ink
            } else {
                palette.muted
            },
            true,
            true,
        ),
        Family::Clearlooks => (
            theme::bold(12.5),
            if state.active {
                Color32::WHITE
            } else {
                lerp(palette.ink, palette.canvas, 0.45)
            },
            true,
            true,
        ),
        Family::Adwaita => (
            theme::bold(14.0),
            if state.active {
                palette.ink
            } else {
                lerp(palette.ink, palette.canvas, 0.45)
            },
            true,
            false,
        ),
    };
    let galley = painter.layout_no_wrap(title.to_owned(), font, ink);
    let size = 16.0;
    // Windows 7's icon sits 8 pixels in from the window's edge, or the
    // screen's when maximized.
    let glass = style == Family::Glass && bar.left() > 0.5;
    let left = bar.left()
        + match style {
            _ if glass => -2.0,
            Family::Fluent => 12.0,
            Family::Oxygen | Family::Breeze | Family::Clearlooks => 6.0,
            _ => 4.0,
        };
    let icon_at = icon
        .then(|| Rect::from_center_size(pos2(left + size / 2.0, bar.center().y), vec2(size, size)));
    let y = bar.center().y - galley.size().y / 2.0;
    let text_at = match icon_at {
        _ if centered => pos2(bar.center().x - galley.size().x / 2.0, y),
        Some(icon_rect) => pos2(icon_rect.right() + 5.0, y),
        None => pos2(left, y),
    };
    let area = Rect::from_min_size(text_at, galley.size());
    match style {
        Family::Luna => {
            painter.galley_with_override_text_color(
                text_at + vec2(1.0, 1.0),
                galley.clone(),
                if palette.dark {
                    Color32::BLACK
                } else {
                    faded(rgb(0x0f1089), 0.85)
                },
            );
        }
        Family::Glass => super::aero::paint_title_glow(painter, area, palette.dark),
        // Aqua and Oxygen press their titles into the bar: a light line
        // under the letters.
        Family::Gel | Family::Oxygen if !palette.dark => {
            painter.galley_with_override_text_color(
                text_at + vec2(0.0, 1.0),
                galley.clone(),
                Color32::from_white_alpha(160),
            );
        }
        Family::Plastik if state.active => {
            painter.galley_with_override_text_color(
                text_at + vec2(1.0, 1.0),
                galley.clone(),
                faded(super::skin::darker(from, 0.5), 0.6),
            );
        }
        // Clearlooks outlines its white title in the bar's deep shade.
        Family::Clearlooks if state.active => {
            let outline = super::skin::gtk_shade(super::clearlooks::base(palette), 0.7);
            for shift in [
                vec2(1.0, 0.0),
                vec2(-1.0, 0.0),
                vec2(0.0, 1.0),
                vec2(0.0, -1.0),
            ] {
                painter.galley_with_override_text_color(text_at + shift, galley.clone(), outline);
            }
        }
        _ => {}
    }
    if let Some(icon_rect) = icon_at {
        theme::paint_logo(painter, icon_rect, palette);
    }
    painter.galley_with_override_text_color(text_at, galley, ink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Accent;

    use eframe::egui::Shape;

    /// Every style but Modern draws its frame.
    fn framed() -> impl Iterator<Item = Style> {
        Style::ALL
            .into_iter()
            .filter(|style| *style != Style::Modern)
    }

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
            accent: Accent::Blue,
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
        for style in framed() {
            let metrics = metrics(style).unwrap();
            // Windows 98's 18 is the shortest.
            assert!(metrics.title >= 18.0, "{style:?}");
            // macOS 27 has no border: its sidebar runs to the edge.
            let border = if style.family() == Family::Liquid {
                0.0
            } else {
                1.0
            };
            assert!(metrics.side >= border, "{style:?}");
        }
    }

    #[test]
    fn the_title_bar_sits_on_top_and_the_app_under_it() {
        for style in framed() {
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
        for style in framed() {
            let (bar, _) = layout(metrics(style).unwrap(), window(), false);
            let buttons = captions(style, bar);
            let mut kinds: Vec<_> = buttons.iter().map(|(caption, _)| *caption).collect();
            kinds.sort_by_key(|caption| *caption as u8);
            // GNOME offers close alone.
            let expected: &[Caption] = if style.family() == Family::Adwaita {
                &[Caption::Close]
            } else {
                &[Caption::Minimize, Caption::Maximize, Caption::Close]
            };
            assert_eq!(kinds, expected, "{style:?}");
            for (i, (caption, rect)) in buttons.iter().enumerate() {
                assert!(bar.contains_rect(*rect), "{style:?} {caption:?}");
                assert!(rect.width() >= 11.0, "{style:?} {caption:?}");
                // Left to right, without overlapping.
                if let Some((_, next)) = buttons.get(i + 1) {
                    assert!(rect.right() <= next.left(), "{style:?} {caption:?}");
                }
            }
            let at = |kind| buttons.iter().find(|(c, _)| *c == kind).unwrap().1;
            let close = at(Caption::Close);
            match style.family() {
                // The Mac closes at the left.
                Family::Gel | Family::Liquid => {
                    assert!(close.left() < bar.left() + 20.0, "{style:?}")
                }
                // Windows, KDE and GNOME close at the right, Windows at the
                // very edge.
                family => {
                    let edge = match family {
                        Family::Retro | Family::Luna | Family::Glass | Family::Fluent => 8.0,
                        _ => 24.0,
                    };
                    assert!(close.right() > bar.right() - edge, "{style:?}");
                    if family != Family::Adwaita {
                        assert!(close.left() > at(Caption::Maximize).right() - 0.5);
                    }
                }
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
        let navy_scheme = theme::Scheme {
            style: Style::Retro,
            accent: Accent::Navy,
            ..theme::Scheme::default()
        };
        let navy = Palette::of(navy_scheme, false);
        assert_eq!(navy.deep, rgb(0x000080));
        let (from, to) = in_style(Style::Retro, || title_colors(&navy, true));
        assert_eq!((from, to), (rgb(0x000080), rgb(0x1084d0)));
        // Any other accent fades from itself to a lighter shade.
        let blue = palette(Style::Retro, false);
        let (from, to) = in_style(Style::Retro, || title_colors(&blue, true));
        assert_eq!(from, blue.deep);
        assert_ne!(to, from);
        for style in framed() {
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
        green.bar = green.deep;
        let (from, to) = in_style(Style::Retro, || title_colors(&green, true));
        assert_eq!(from, green.deep);
        assert_eq!(to, lerp(green.deep, Color32::WHITE, 0.45));
    }

    #[test]
    fn untinted_title_bars_are_grey_in_every_state() {
        for style in framed() {
            for dark in [false, true] {
                let scheme = theme::Scheme {
                    style,
                    accent: crate::settings::Accent::Red,
                    tint_window_bar: false,
                    ..theme::Scheme::default()
                };
                let palette = Palette::of(scheme, dark);
                in_style(style, || {
                    for active in [true, false] {
                        let (from, to) = title_colors(&palette, active);
                        for color in [from, to] {
                            let context = format!("{style:?} dark={dark} active={active}");
                            assert_eq!(color.r(), color.g(), "{context}");
                            assert_eq!(color.g(), color.b(), "{context}");
                        }
                    }
                });
            }
        }
    }

    #[test]
    fn every_frame_draws_inside_the_window_in_every_state() {
        for style in framed() {
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
                    assert!(shapes.len() >= 2, "{at}");
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
                    // Windows 98 is square through and through.
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
        let at = |x, y| edge_at(Style::Retro, window(), pos2(x, y));
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
        assert_eq!(
            edge_at(Style::Glass, window(), pos2(6.0, 200.0)),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            edge_at(Style::Glass, window(), pos2(18.0, 1.0)),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(edge_at(Style::Modern, window(), pos2(1.0, 1.0)), None);
    }

    #[test]
    fn title_bar_buttons_in_the_edge_still_click() {
        // Windows 7 hangs its from the top, inside the resize border.
        for style in Style::ALL {
            let Some(metrics) = metrics(style) else {
                continue;
            };
            let (bar, _) = layout(metrics, window(), false);
            for (caption, rect) in captions(style, bar) {
                for point in [rect.center(), rect.center_top() + vec2(0.0, 0.5)] {
                    assert_eq!(
                        edge_at(style, window(), point),
                        None,
                        "{style:?} {caption:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn windows_round_the_corners_their_desktop_does() {
        let corners = |style, maximized| in_style(style, || window_corners(maximized));
        // Tiger rounds the title bar and keeps the bottom square.
        let tiger = corners(Style::Aqua, false);
        assert_eq!((tiger.nw, tiger.ne), (5, 5));
        assert_eq!((tiger.sw, tiger.se), (0, 0));
        assert_eq!(corners(Style::Fluent, false), CornerRadius::same(8));
        assert_eq!(corners(Style::Liquid, false), CornerRadius::same(26));
        // Maximized, or with no rounded corners to speak of, they're square.
        for style in Style::ALL {
            assert_eq!(corners(style, true), CornerRadius::ZERO, "{style:?}");
        }
        for style in [Style::Modern, Style::Retro] {
            assert_eq!(corners(style, false), CornerRadius::ZERO, "{style:?}");
        }
        // The person's roundness reaches them: none is none.
        let flat = theme::Skin {
            style: Style::Liquid,
            roundness: 0.0,
            ..theme::Skin::default()
        };
        assert_eq!(
            theme::with_skin(flat, || window_corners(false)),
            CornerRadius::ZERO
        );
    }

    #[test]
    fn the_app_follows_the_window_into_its_corners_and_only_there() {
        for (style, radius) in [(Style::Fluent, 8), (Style::Liquid, 26)] {
            let palette = palette(style, false);
            in_style(style, || {
                theme::painted(|ui| {
                    // Nothing published yet: nothing is rounded.
                    let ctx = ui.ctx().clone();
                    assert_eq!(fit(&ctx, window()), CornerRadius::ZERO);
                    show(ui, window(), "PkgDeck", &palette);
                    let whole = fit(&ctx, window());
                    assert_eq!(whole, CornerRadius::same(radius), "{style:?}");
                    // One pixel in, as under Windows 11's edge, a pixel less.
                    let inset = fit(&ctx, window().shrink(1.0));
                    assert_eq!(inset, CornerRadius::same(radius - 1), "{style:?}");
                    // The bottom-left of the window only: its sidebar.
                    let sidebar =
                        Rect::from_min_max(pos2(0.0, 100.0), pos2(200.0, window().bottom()));
                    let sidebar = fit(&ctx, sidebar);
                    assert_eq!((sidebar.sw, sidebar.nw), (radius, 0), "{style:?}");
                    assert_eq!((sidebar.se, sidebar.ne), (0, 0), "{style:?}");
                    // Away from every corner there is nothing to follow.
                    let middle = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 300.0));
                    assert_eq!(fit(&ctx, middle), CornerRadius::ZERO, "{style:?}");
                })
            });
        }
    }

    #[test]
    fn the_mac_hairline_goes_on_top_of_the_edge_to_edge_sidebar() {
        let palette = palette(Style::Liquid, false);
        let shapes = in_style(Style::Liquid, || {
            theme::painted(|ui| {
                show(ui, window(), "PkgDeck", &palette);
                // The app fills the room under the bar.
                ui.painter()
                    .rect_filled(window(), CornerRadius::ZERO, Color32::RED);
                edges(ui, window(), false);
            })
        });
        match shapes.last() {
            Some(Shape::Rect(outline)) => {
                assert_eq!(outline.rect, window());
                assert!(outline.fill.a() == 0 && outline.stroke.width == 1.0);
            }
            other => panic!("the outline should come last, got {other:?}"),
        }
    }

    #[test]
    fn under_the_system_frame_the_system_draws_the_edge() {
        let palette = palette(Style::Liquid, false);
        let shapes = in_style(Style::Liquid, || {
            theme::painted(|ui| {
                show_on(ui, window(), "PkgDeck", &palette, Desktop::Mac);
                ui.painter()
                    .rect_filled(window(), CornerRadius::ZERO, Color32::RED);
                edges_on(ui, window(), false, Desktop::Mac);
            })
        });
        match shapes.last() {
            Some(Shape::Rect(app)) => assert_eq!(app.fill, Color32::RED),
            other => panic!("the app should come last, got {other:?}"),
        }
    }

    #[test]
    fn the_desktop_set_is_the_one_read_back() {
        for desktop in [Desktop::Other, Desktop::Kde, Desktop::Mac] {
            assert_eq!(Desktop::from_code(desktop as u8), desktop);
        }
        assert_eq!(Desktop::from_code(9), Desktop::Other);
    }

    #[test]
    fn the_mac_sidebar_divider_runs_up_through_the_title_bar() {
        let palette = palette(Style::Liquid, false);
        let metrics = metrics(Style::Liquid).expect("Liquid draws a frame");
        let (bar, _) = layout(metrics, window(), false);
        let shapes = in_style(Style::Liquid, || {
            theme::painted(|ui| {
                ui.ctx()
                    .data_mut(|data| data.insert_temp(Id::unique("sidebar-extent"), 200.0_f32));
                show(ui, window(), "PkgDeck", &palette);
            })
        });
        let divider = shapes.iter().any(|shape| match shape {
            Shape::LineSegment { points, .. } => {
                (points[0].x - 199.5).abs() < 0.01
                    && (points[1].x - 199.5).abs() < 0.01
                    && points[0].y == window().top()
                    && points[1].y == bar.bottom()
            }
            _ => false,
        });
        assert!(divider, "no divider through the title bar: {shapes:?}");
    }

    #[test]
    fn gnome_51s_header_is_flat_over_the_page() {
        for dark in [false, true] {
            let palette = palette(Style::Adwaita, dark);
            let metrics = metrics(Style::Adwaita).expect("GNOME 51 draws a frame");
            let (bar, _) = layout(metrics, window(), false);
            let shapes = in_style(Style::Adwaita, || {
                theme::painted(|ui| {
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(Id::unique("sidebar-extent"), 200.0_f32));
                    show(ui, window(), "PkgDeck", &palette);
                })
            });
            for shape in &shapes {
                match shape {
                    // No line under the header, over the sidebar or the page.
                    Shape::LineSegment { points, .. } => assert!(
                        points[0].y != points[1].y || (points[0].y - bar.bottom()).abs() > 1.0,
                        "a line under the header: {shape:?}"
                    ),
                    // Beside the sidebar the header is the page's colour.
                    Shape::Rect(rect) if rect.rect.right() > 202.0 && rect.fill.a() > 0 => {
                        assert_eq!(rect.fill, palette.canvas, "dark {dark}: {shape:?}");
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn the_desktop_comes_from_its_names() {
        assert_eq!(Desktop::of(Some("KDE")), Desktop::Kde);
        assert_eq!(Desktop::of(Some("ubuntu:KDE")), Desktop::Kde);
        for names in [
            None,
            Some(""),
            Some("GNOME"),
            Some("ubuntu:GNOME"),
            Some("KDEX"),
        ] {
            assert_eq!(Desktop::of(names), Desktop::Other, "{names:?}");
        }
    }

    #[test]
    fn a_style_keeps_the_system_frame_on_the_desktop_it_copies() {
        let desktops = [Desktop::Other, Desktop::Kde, Desktop::Mac];
        for style in Style::ALL {
            for desktop in desktops {
                let system = matches!(
                    (style, desktop),
                    (Style::Modern, _)
                        | (Style::Breeze, Desktop::Kde)
                        | (Style::Liquid, Desktop::Mac)
                );
                assert_eq!(
                    system_frame(style, desktop),
                    system,
                    "{style:?} {desktop:?}"
                );
            }
        }
        // GNOME has no frame of its own for apps, so GNOME 51 never asks.
        assert!(!system_frame(Style::Adwaita, Desktop::Other));
    }

    #[test]
    fn only_macos_27_lays_out_a_bar_under_the_system_frame() {
        assert_eq!(drawn(Style::Breeze, Desktop::Kde), None);
        assert_eq!(drawn(Style::Modern, Desktop::Mac), None);
        assert_eq!(drawn(Style::Liquid, Desktop::Mac), metrics(Style::Liquid));
        // The styles that copy another desktop still draw theirs.
        assert_eq!(drawn(Style::Breeze, Desktop::Mac), metrics(Style::Breeze));
        assert_eq!(drawn(Style::Oxygen, Desktop::Kde), metrics(Style::Oxygen));
        assert_eq!(drawn(Style::Aqua, Desktop::Mac), metrics(Style::Aqua));
    }

    #[test]
    fn under_the_system_frame_no_window_buttons_are_drawn() {
        let cases = [
            (Style::Breeze, Desktop::Kde, true),
            (Style::Liquid, Desktop::Mac, true),
            (Style::Breeze, Desktop::Other, false),
            (Style::Liquid, Desktop::Other, false),
        ];
        for (style, desktop, system) in cases {
            let palette = palette(style, false);
            let ctx = egui::Context::default();
            let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            crate::setup(&ctx, &fixtures, None);
            ctx.run_ui(egui::RawInput::default(), |_| {})
                .textures_delta
                .clear();
            let mut room = Rect::NOTHING;
            let mut output = in_style(style, || {
                ctx.run_ui(egui::RawInput::default(), |ui| {
                    room = show_on(ui, window(), "PkgDeck", &palette, desktop);
                })
            });
            output.textures_delta.clear();
            let decorations: Vec<bool> = output
                .viewport_output
                .values()
                .flat_map(|viewport| &viewport.commands)
                .filter_map(|command| match command {
                    ViewportCommand::Decorations(on) => Some(*on),
                    _ => None,
                })
                .collect();
            assert_eq!(decorations, [system], "{style:?} {desktop:?}");
            let close = Id::unique("window-frame").with(Caption::Close);
            assert_eq!(
                ctx.read_response(close).is_some(),
                !system,
                "{style:?} {desktop:?}"
            );
            match drawn(style, desktop) {
                None => assert_eq!(room, window(), "{style:?}"),
                Some(metrics) => {
                    assert_eq!(room, layout(metrics, window(), false).1, "{style:?}");
                }
            }
        }
        assert_eq!(drawn(Style::Breeze, Desktop::Kde), None);
    }

    #[test]
    fn showing_the_frame_hands_the_app_the_room_under_the_bar() {
        for style in Style::ALL {
            let palette = palette(style, false);
            let mut room = Rect::NOTHING;
            in_style(style, || {
                theme::painted(|ui| {
                    room = show(ui, window(), "PkgDeck", &palette);
                    edges(ui, window(), false);
                })
            });
            match metrics(style) {
                None => assert_eq!(room, window()),
                Some(metrics) => assert_eq!(room, layout(metrics, window(), false).1),
            }
        }
    }
}
