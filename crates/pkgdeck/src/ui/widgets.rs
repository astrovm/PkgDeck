//! PkgDeck's controls: buttons, switches, check boxes, fields, spinners,
//! progress bars and cards, painted by hand so they share one look and
//! animate the same way. Every animation goes through [`secs`], which is
//! zero when the person turned animations off.

use crate::{
    model::Tone,
    theme::{self, Palette, CARD_RADIUS, CONTROL_RADIUS, SMALL_RADIUS},
};
use eframe::egui::{
    self, emath::easing, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id,
    Margin, Pos2, Rect, Response, Sense, Shadow, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
    WidgetType,
};

pub const CONTROL_HEIGHT: f32 = 36.0;
pub const FEEDBACK: f32 = 0.06;
pub const REVEAL: f32 = 0.12;
pub const LAYOUT: f32 = 0.17;

fn motion_id() -> Id {
    Id::new("pkgdeck-reduce-motion")
}
pub fn set_reduce_motion(ctx: &egui::Context, reduce: bool) {
    ctx.data_mut(|data| data.insert_temp(motion_id(), reduce));
}
pub fn reduce_motion(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp(motion_id()).unwrap_or(false))
}
/// An animation's length, or none when animations are off.
pub fn secs(ctx: &egui::Context, seconds: f32) -> f32 {
    if reduce_motion(ctx) {
        0.0
    } else {
        seconds
    }
}
/// 0 → 1 as `value` turns on, eased.
pub fn ease(ctx: &egui::Context, id: Id, value: bool, seconds: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, value, secs(ctx, seconds), easing::cubic_out)
}
/// Where a [`glide`] is and how fast it moves.
#[derive(Clone, Copy)]
struct Spring {
    at: f32,
    speed: f32,
    time: f64,
}

/// Moves toward `target` like a critically damped spring: it settles in
/// about `seconds` without overshooting, and a target that changes on the
/// way bends the motion instead of restarting it.
pub fn glide(ctx: &egui::Context, id: Id, target: f32, seconds: f32) -> f32 {
    let seconds = secs(ctx, seconds);
    let now = ctx.input(|input| input.time);
    let id = id.with("glide");
    let Some(mut spring) = ctx.data(|data| data.get_temp::<Spring>(id)) else {
        ctx.data_mut(|data| {
            data.insert_temp(
                id,
                Spring {
                    at: target,
                    speed: 0.0,
                    time: now,
                },
            )
        });
        return target;
    };
    let dt = ((now - spring.time) as f32).clamp(0.0, 0.1);
    spring.time = now;
    if seconds <= 0.0 {
        spring.at = target;
        spring.speed = 0.0;
    } else {
        let (at, speed) = spring_step(spring.at - target, spring.speed, 6.0 / seconds, dt);
        // Close enough to stop asking for frames: under a fraction of a
        // pixel, or of a percent for fractions.
        let close = 2e-4 * target.abs().max(1.0);
        if at.abs() < close && speed.abs() < close * 10.0 {
            spring.at = target;
            spring.speed = 0.0;
        } else {
            spring.at = target + at;
            spring.speed = speed;
            ctx.request_repaint();
        }
    }
    ctx.data_mut(|data| data.insert_temp(id, spring));
    spring.at
}

/// One step of a critically damped spring: `offset` from rest and `speed`
/// after `dt` seconds with stiffness `omega`. Exact, so uneven frames don't
/// change the path.
pub fn spring_step(offset: f32, speed: f32, omega: f32, dt: f32) -> (f32, f32) {
    let decay = (-omega * dt).exp();
    let push = speed + omega * offset;
    (
        (offset + push * dt) * decay,
        (speed - omega * push * dt) * decay,
    )
}
/// How far along an animation that started `elapsed` seconds ago is.
pub fn progress_since(ctx: &egui::Context, elapsed: f32, seconds: f32) -> f32 {
    let seconds = secs(ctx, seconds);
    if seconds <= 0.0 {
        return 1.0;
    }
    let t = (elapsed / seconds).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    easing::cubic_out(t)
}
/// Seconds since the window started, for looping animations.
pub fn clock(ctx: &egui::Context) -> f32 {
    ctx.input(|input| input.time) as f32
}

pub fn alpha(color: Color32, amount: f32) -> Color32 {
    color.gamma_multiply(amount)
}
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}
pub fn tone(palette: &Palette, tone: Tone) -> Color32 {
    match tone {
        Tone::Accent => palette.accent,
        Tone::Success => palette.success,
        Tone::Danger => palette.danger,
        Tone::Warning => palette.warning,
        Tone::Muted => palette.muted,
    }
}

/// How a button looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// The page's main action: filled with the accent.
    Primary,
    /// A filled button in a tone, for main actions that remove.
    Solid(Tone),
    /// An outlined button on the surface.
    Secondary,
    /// A soft tinted button in a tone.
    Soft(Tone),
    /// Text and icon only, tinted on hover.
    Flat,
}

pub struct Button<'a> {
    look: Look,
    icon: Option<&'a str>,
    label: &'a str,
    tooltip: Option<String>,
    enabled: bool,
    icon_only: bool,
    small: bool,
    min_width: f32,
    id_salt: Option<Id>,
}

impl<'a> Button<'a> {
    pub fn new(look: Look, label: &'a str) -> Self {
        Self {
            look,
            icon: None,
            label,
            tooltip: None,
            enabled: true,
            icon_only: false,
            small: false,
            min_width: 0.0,
            id_salt: None,
        }
    }
    pub fn icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Only the icon, with the label as its tooltip.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
    pub fn id_salt(mut self, salt: impl std::hash::Hash + std::fmt::Debug) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let palette = Palette::current(ui.ctx());
        let height = if self.small { 30.0 } else { CONTROL_HEIGHT };
        let font = if self.small {
            theme::font(13.0)
        } else {
            theme::font(14.0)
        };
        let bold_font = if self.small {
            theme::bold(13.0)
        } else {
            theme::bold(14.0)
        };
        let strong = matches!(self.look, Look::Primary | Look::Solid(_) | Look::Soft(_));
        let text_font = if strong { bold_font } else { font };
        let icon_size = if self.small { 16.0 } else { 18.0 };
        let show_label = !self.icon_only || self.icon.is_none();
        let galley = show_label.then(|| {
            ui.painter()
                .layout_no_wrap(self.label.to_owned(), text_font.clone(), palette.ink)
        });
        let padding = if show_label {
            14.0
        } else {
            (height - icon_size) / 2.0
        };
        let mut width = padding * 2.0;
        if self.icon.is_some() {
            width += icon_size;
        }
        if let Some(galley) = &galley {
            width += galley.size().x;
            if self.icon.is_some() {
                width += 8.0;
            }
        }
        let width = width.max(self.min_width).max(height);
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
        let id = self.id_salt.unwrap_or(response.id);
        let response = response.on_hover_cursor(if self.enabled {
            CursorIcon::PointingHand
        } else {
            CursorIcon::Default
        });
        let label = self.label.to_owned();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, self.enabled, &label));
        if ui.is_rect_visible(rect) {
            let ctx = ui.ctx();
            let hover = ease(
                ctx,
                id.with("hover"),
                response.hovered() && self.enabled,
                FEEDBACK,
            );
            let press = ease(
                ctx,
                id.with("press"),
                response.is_pointer_button_down_on() && self.enabled,
                FEEDBACK,
            );
            let rect = rect.shrink(press * 0.6);
            let (fill, stroke, ink) = match self.look {
                Look::Primary => (
                    mix(palette.accent, Color32::WHITE, hover * 0.1),
                    Stroke::NONE,
                    palette.accent_ink,
                ),
                Look::Solid(t) => {
                    let color = tone(&palette, t);
                    (
                        mix(color, Color32::WHITE, hover * 0.1),
                        Stroke::NONE,
                        palette.surface,
                    )
                }
                Look::Secondary => (
                    mix(palette.surface, palette.hover_solid(), hover),
                    Stroke::new(1.0, mix(palette.line, palette.strong_line, hover)),
                    palette.ink,
                ),
                Look::Soft(t) => {
                    let color = tone(&palette, t);
                    (
                        alpha(color, 0.13 + hover * 0.08 + press * 0.05),
                        Stroke::new(1.0, alpha(color, 0.35)),
                        color,
                    )
                }
                Look::Flat => (
                    alpha(palette.ink, hover * 0.07 + press * 0.05),
                    Stroke::NONE,
                    palette.ink,
                ),
            };
            // A filled button that's off goes grey; a faded colour reads as
            // a lighter variant rather than unavailable.
            let filled = matches!(self.look, Look::Primary | Look::Solid(_));
            let (fill, ink) = if filled && !self.enabled {
                (palette.hover_solid(), palette.muted)
            } else {
                (fill, ink)
            };
            let opacity = if self.enabled || filled { 1.0 } else { 0.42 };
            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(CONTROL_RADIUS),
                alpha(fill, opacity),
                Stroke::new(stroke.width, alpha(stroke.color, opacity)),
                StrokeKind::Inside,
            );
            let ink = alpha(ink, opacity);
            let content = icon_size
                + galley.as_ref().map_or(0.0, |g| {
                    g.size().x + if self.icon.is_some() { 8.0 } else { 0.0 }
                });
            let mut x = rect.center().x - content / 2.0;
            if let Some(icon) = self.icon {
                let icon_rect = Rect::from_min_size(
                    Pos2::new(x, rect.center().y - icon_size / 2.0),
                    Vec2::splat(icon_size),
                );
                theme::paint_icon(painter, icon_rect, icon, ink);
                x += icon_size + 8.0;
            } else if let Some(galley) = &galley {
                x = rect.center().x - galley.size().x / 2.0;
            }
            if let Some(galley) = galley {
                let pos = Pos2::new(x, rect.center().y - galley.size().y / 2.0);
                painter.galley_with_override_text_color(pos, galley, ink);
            }
        }
        let tooltip = self
            .tooltip
            .or_else(|| (self.icon_only && !self.label.is_empty()).then(|| self.label.to_owned()));
        match tooltip {
            Some(text) if self.enabled => response.on_hover_text(text),
            Some(text) => response.on_disabled_hover_text(text),
            None => response,
        }
    }
}

/// A round icon button without a frame until hovered.
pub fn icon_button(
    ui: &mut Ui,
    icon: &str,
    tooltip: &str,
    color: Color32,
    enabled: bool,
) -> Response {
    let size = 32.0;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let tip = tooltip.to_owned();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &tip));
    if ui.is_rect_visible(rect) {
        let hover = ease(
            ui.ctx(),
            response.id.with("hover"),
            response.hovered() && enabled,
            FEEDBACK,
        );
        let opacity = if enabled { 1.0 } else { 0.42 };
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(CONTROL_RADIUS),
            alpha(color, hover * 0.12),
        );
        theme::paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(18.0)),
            icon,
            alpha(color, opacity),
        );
    }
    let response = if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    };
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// An on/off switch. Returns the response; `on` flips on click.
pub fn switch(ui: &mut Ui, on: &mut bool, enabled: bool, label: &str) -> Response {
    let size = vec2(40.0, 24.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(size, sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let value = *on;
    let text = label.to_owned();
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, value, &text));
    if ui.is_rect_visible(rect) {
        let palette = Palette::current(ui.ctx());
        let t = ease(ui.ctx(), response.id, *on, REVEAL);
        let hover = ease(
            ui.ctx(),
            response.id.with("hover"),
            response.hovered() && enabled,
            FEEDBACK,
        );
        let opacity = if enabled { 1.0 } else { 0.42 };
        let track = mix(palette.strong_line_solid(), palette.accent, t);
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(12),
            alpha(mix(track, palette.ink, hover * 0.06), opacity),
        );
        let radius = 9.0 + hover * 0.6;
        let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
        ui.painter().add(egui::Shape::circle_filled(
            Pos2::new(x, rect.center().y + 0.6),
            radius,
            alpha(Color32::BLACK, 0.18 * opacity),
        ));
        ui.painter().circle_filled(
            Pos2::new(x, rect.center().y),
            radius,
            alpha(Color32::WHITE, opacity),
        );
    }
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A check box that pops when ticked.
pub fn tickbox(ui: &mut Ui, checked: bool, enabled: bool, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(22.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let text = label.to_owned();
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, checked, &text));
    paint_tick(ui, rect, response.id, checked, enabled, response.hovered());
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

pub fn paint_tick(ui: &Ui, rect: Rect, id: Id, checked: bool, enabled: bool, hovered: bool) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let palette = Palette::current(ui.ctx());
    let t = ui.ctx().animate_bool_with_time_and_easing(
        id.with("tick"),
        checked,
        secs(ui.ctx(), REVEAL),
        easing::back_out,
    );
    let opacity = if enabled { 1.0 } else { 0.42 };
    let box_rect = Rect::from_center_size(rect.center(), Vec2::splat(18.0));
    let border = if hovered && enabled {
        palette.accent
    } else {
        palette.strong_line
    };
    ui.painter().rect(
        box_rect,
        CornerRadius::same(SMALL_RADIUS),
        alpha(palette.accent, t.clamp(0.0, 1.0) * opacity),
        Stroke::new(
            1.5,
            alpha(mix(border, palette.accent, t.clamp(0.0, 1.0)), opacity),
        ),
        StrokeKind::Inside,
    );
    if t > 0.01 {
        let scale = t.max(0.0);
        let check = Rect::from_center_size(box_rect.center(), Vec2::splat(14.0 * scale));
        theme::paint_icon(
            ui.painter(),
            check,
            "installed",
            alpha(palette.accent_ink, opacity),
        );
    }
}

/// A text field with an icon, a clear button and an animated focus ring.
pub struct Field<'a> {
    text: &'a mut String,
    hint: &'a str,
    icon: Option<&'a str>,
    id: Id,
    width: Option<f32>,
    monospace: bool,
    large: bool,
}
pub struct FieldResponse {
    pub response: Response,
    pub cleared: bool,
    pub submitted: bool,
}
impl<'a> Field<'a> {
    pub fn new(text: &'a mut String, hint: &'a str, id: Id) -> Self {
        Self {
            text,
            hint,
            icon: None,
            id,
            width: None,
            monospace: false,
            large: false,
        }
    }
    pub fn icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }
    pub fn show(self, ui: &mut Ui) -> FieldResponse {
        let palette = Palette::current(ui.ctx());
        let height = if self.large {
            46.0
        } else {
            CONTROL_HEIGHT + 2.0
        };
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let focused = ui.ctx().memory(|m| m.has_focus(self.id));
        let focus = ease(ui.ctx(), self.id.with("focus"), focused, REVEAL);
        let painter = ui.painter().clone();
        painter.rect(
            rect,
            CornerRadius::same(CONTROL_RADIUS + if self.large { 3 } else { 0 }),
            palette.surface,
            Stroke::new(1.0 + focus, mix(palette.line, palette.accent, focus)),
            StrokeKind::Inside,
        );
        if focus > 0.0 {
            painter.rect_stroke(
                rect.expand(3.0),
                CornerRadius::same(CONTROL_RADIUS + 3),
                Stroke::new(3.0, alpha(palette.accent, 0.18 * focus)),
                StrokeKind::Inside,
            );
        }
        let mut left = rect.left() + 12.0;
        if let Some(icon) = self.icon {
            let icon_rect =
                Rect::from_center_size(Pos2::new(left + 9.0, rect.center().y), Vec2::splat(18.0));
            theme::paint_icon(
                &painter,
                icon_rect,
                icon,
                mix(palette.muted, palette.accent, focus),
            );
            left += 28.0;
        }
        let clear_shown = ease(
            ui.ctx(),
            self.id.with("clear"),
            !self.text.is_empty(),
            REVEAL,
        );
        let right = rect.right() - 8.0 - 28.0 * clear_shown.ceil();
        let font: FontId = if self.monospace {
            theme::mono(13.5)
        } else if self.large {
            theme::font(16.0)
        } else {
            theme::font(14.5)
        };
        let text_rect =
            Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(right, rect.bottom()));
        let edit = egui::TextEdit::singleline(self.text)
            .id(self.id)
            .hint_text(egui::RichText::new(self.hint).color(palette.muted))
            .font(font)
            .frame(Frame::NONE)
            .margin(Margin::symmetric(0, 0))
            .vertical_align(egui::Align::Center)
            .desired_width(text_rect.width());
        let output = ui.put(text_rect, edit);
        let submitted = output.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut cleared = false;
        if clear_shown > 0.0 {
            let clear_rect = Rect::from_center_size(
                Pos2::new(rect.right() - 22.0, rect.center().y),
                Vec2::splat(26.0),
            );
            let clear = ui
                .interact(clear_rect, self.id.with("clear-button"), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Clear");
            let hover = ease(ui.ctx(), clear.id, clear.hovered(), FEEDBACK);
            painter.circle_filled(
                clear_rect.center(),
                11.0,
                alpha(palette.ink, (0.07 + hover * 0.08) * clear_shown),
            );
            theme::paint_icon(
                &painter,
                Rect::from_center_size(clear_rect.center(), Vec2::splat(13.0)),
                "cancel",
                alpha(palette.muted, clear_shown),
            );
            if clear.clicked() {
                self.text.clear();
                cleared = true;
                output.request_focus();
            }
        }
        FieldResponse {
            response: output,
            cleared,
            submitted,
        }
    }
}

/// An open arc that turns.
pub fn spinner(ui: &mut Ui, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_spinner(ui, rect, color);
    response
}
pub fn paint_spinner(ui: &Ui, rect: Rect, color: Color32) {
    if reduce_motion(ui.ctx()) || !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx().request_repaint();
    let turn = clock(ui.ctx()) * std::f32::consts::TAU / 1.1;
    let radius = rect.width() * 0.38;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let angle = turn + i as f32 / 24.0 * std::f32::consts::PI * 1.5;
            rect.center() + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        Stroke::new((rect.width() / 12.0).max(1.6), color),
    ));
}

/// A thin bar: filled to `fraction`, or a sweeping segment when unknown.
pub fn paint_bar(ui: &Ui, rect: Rect, fraction: Option<f32>, color: Color32, id: Id) {
    let radius = CornerRadius::same((rect.height() / 2.0).round() as u8);
    ui.painter().rect_filled(rect, radius, alpha(color, 0.18));
    match fraction {
        Some(fraction) => {
            let shown = glide(ui.ctx(), id, fraction.clamp(0.0, 1.0), LAYOUT);
            if shown > 0.0 {
                let mut fill = rect;
                fill.set_width((rect.width() * shown).max(rect.height()));
                ui.painter().rect_filled(fill, radius, color);
            }
        }
        None => {
            let width = rect.width() * 0.3;
            let x = if reduce_motion(ui.ctx()) {
                rect.center().x - width / 2.0
            } else {
                ui.ctx().request_repaint();
                let t = (clock(ui.ctx()) / 1.25).fract();
                let eased = easing::cubic_in_out(t);
                rect.left() - width + (rect.width() + width) * eased
            };
            let segment = Rect::from_min_max(
                Pos2::new(x.max(rect.left()), rect.top()),
                Pos2::new((x + width).min(rect.right()), rect.bottom()),
            );
            if segment.width() > 0.0 {
                ui.painter().rect_filled(segment, radius, color);
            }
        }
    }
}

/// A placeholder bar that breathes while something loads.
pub fn paint_skeleton(ui: &Ui, rect: Rect, color: Color32) {
    let pulse = if reduce_motion(ui.ctx()) {
        0.75
    } else {
        ui.ctx().request_repaint();
        0.55 + 0.45 * (0.5 + 0.5 * (clock(ui.ctx()) * std::f32::consts::TAU / 1.6).sin())
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(SMALL_RADIUS), alpha(color, pulse));
}

/// A rounded panel on the surface colour.
pub fn card(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(16))
        .shadow(Shadow {
            offset: [0, 1],
            blur: 6,
            spread: 0,
            color: Color32::from_black_alpha(if palette.dark { 40 } else { 10 }),
        })
}

/// A small pill with a count, e.g. updates waiting.
pub fn paint_badge(ui: &Ui, center: Pos2, text: &str, fill: Color32, ink: Color32, scale: f32) {
    if scale <= 0.0 {
        return;
    }
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), theme::bold(11.5 * scale.max(0.01)), ink);
    let size = vec2(
        (galley.size().x + 12.0 * scale).max(20.0 * scale),
        20.0 * scale,
    );
    let rect = Rect::from_center_size(center, size);
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, ink);
}

/// A label that cuts long text short with an ellipsis.
pub fn one_line(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// Text that wraps within `width`.
pub fn wrapped(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
    rows: usize,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

/// A caption in small capitals with spacing, for section titles.
pub fn caption(ui: &mut Ui, text: &str) {
    let palette = Palette::current(ui.ctx());
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(theme::bold(11.5))
            .color(palette.muted)
            .extra_letter_spacing(0.7),
    );
}

/// Paints a galley at `pos` anchored as `align` says.
pub fn paint_text(
    ui: &Ui,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    ui.painter().text(pos, align, text, font, color)
}

/// A row whose contents line up from the right, as tall as they are.
pub fn right<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add)
            .inner
    })
    .inner
}

/// A slider that snaps to `count` steps. Returns true when it moved.
pub fn steps(
    ui: &mut Ui,
    id: Id,
    count: usize,
    index: &mut usize,
    enabled: bool,
    width: f32,
) -> bool {
    let palette = Palette::current(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(
        vec2(width, 28.0),
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 20.0, 4.0));
    let at = |i: usize| track.left() + track.width() * i as f32 / (count - 1).max(1) as f32;
    let before = *index;
    if let Some(pos) = response.interact_pointer_pos() {
        let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
        *index = (t * (count - 1) as f32).round() as usize;
    }
    let opacity = if enabled { 1.0 } else { 0.42 };
    let x = glide(ui.ctx(), id, at(*index), REVEAL);
    let painter = ui.painter();
    painter.rect_filled(
        track,
        CornerRadius::same(2),
        alpha(palette.strong_line_solid(), opacity),
    );
    painter.rect_filled(
        Rect::from_min_max(track.min, Pos2::new(x, track.max.y)),
        CornerRadius::same(2),
        alpha(palette.accent, opacity),
    );
    for i in 0..count {
        let color = if at(i) <= x {
            palette.accent
        } else {
            palette.strong_line_solid()
        };
        painter.circle_filled(
            Pos2::new(at(i), track.center().y),
            3.0,
            alpha(color, opacity),
        );
    }
    let hover = ease(
        ui.ctx(),
        id.with("hover"),
        response.hovered() || response.dragged(),
        FEEDBACK,
    );
    painter.circle_filled(
        Pos2::new(x, track.center().y),
        9.0 + hover * 1.5,
        alpha(palette.accent, opacity),
    );
    painter.circle_filled(
        Pos2::new(x, track.center().y),
        4.0,
        alpha(palette.surface, opacity),
    );
    if enabled {
        response.on_hover_cursor(CursorIcon::Grab);
    }
    *index != before
}
