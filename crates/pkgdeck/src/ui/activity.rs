//! The Activity drawer: what ran, what runs, and what waits, newest first.

use super::widgets::*;
use crate::{
    app::App,
    model::{self, Activity},
    theme::{self, Palette},
};
use eframe::egui::{
    self, pos2, vec2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, Ui, UiBuilder,
    Vec2,
};

pub fn drawer(app: &mut App, ui: &mut Ui, window: Rect) {
    let ctx = ui.ctx().clone();
    let shown = ease(&ctx, Id::new("drawer"), app.drawer_open, LAYOUT);
    if shown <= 0.0 {
        return;
    }
    let palette = Palette::current(&ctx);
    // The page dims behind it; a click there closes it.
    let scrim = egui::Area::new(Id::new("drawer-scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(window.min)
        .show(&ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(window.size(), Sense::click());
            ui.painter().rect_filled(
                rect,
                CornerRadius::ZERO,
                Color32::from_black_alpha(
                    ((if palette.dark { 115.0 } else { 56.0 }) * shown) as u8,
                ),
            );
            response
        });
    if scrim.inner.clicked() {
        app.drawer_open = false;
    }
    let width = 480.0f32.min(window.width() - if window.width() < 520.0 { 0.0 } else { 48.0 });
    let rect = Rect::from_min_size(
        pos2(window.right() - width * shown, window.top()),
        vec2(width, window.height()),
    );
    egui::Area::new(Id::new("drawer-panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(&ctx, |ui| {
            let (panel, _) = ui.allocate_exact_size(rect.size(), Sense::click());
            ui.painter().add(
                egui::epaint::Shadow {
                    offset: [-6, 0],
                    blur: 30,
                    spread: 0,
                    color: Color32::from_black_alpha(if palette.dark { 120 } else { 40 }),
                }
                .as_shape(panel, CornerRadius::ZERO),
            );
            ui.painter()
                .rect_filled(panel, CornerRadius::ZERO, palette.surface);
            ui.painter().line_segment(
                [panel.left_top(), panel.left_bottom()],
                Stroke::new(1.0, palette.line),
            );
            ui.scope_builder(UiBuilder::new().max_rect(panel.shrink(20.0)), |ui| {
                content(app, ui);
            });
        });
}

fn content(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Activity")
                .font(theme::bold(20.0))
                .color(palette.ink),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, "cancel", "Close (Esc)", palette.muted, true).clicked() {
                app.drawer_open = false;
            }
            if app.queued_count() > 0
                && Button::new(Look::Secondary, "Cancel queued")
                    .small()
                    .show(ui)
                    .clicked()
            {
                app.c().cancel_queued();
                app.react();
            }
        });
    });
    ui.add_space(10.0);
    if app.activity.is_empty() {
        let center = ui.available_rect_before_wrap().center() - vec2(0.0, 60.0);
        theme::paint_icon(
            ui.painter(),
            Rect::from_center_size(center - vec2(0.0, 44.0), Vec2::splat(34.0)),
            "activity",
            palette.muted,
        );
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            "Nothing has run yet",
            theme::bold(16.0),
            palette.ink,
        );
        ui.painter().text(
            center + vec2(0.0, 26.0),
            egui::Align2::CENTER_CENTER,
            "Installs, removals and updates you run will appear here.",
            theme::font(13.5),
            palette.muted,
        );
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let entries: Vec<Activity> = app.activity.iter().rev().cloned().collect();
            for entry in entries {
                entry_card(app, ui, &entry);
            }
        });
}

fn entry_card(app: &mut App, ui: &mut Ui, entry: &Activity) {
    let palette = Palette::current(ui.ctx());
    let color = tone(&palette, entry.tone());
    let others = entry.others();
    let log = entry.log_text();
    let expandable = !others.is_empty() || !log.is_empty() || entry.finished_at.is_some();
    let expanded = app.expanded.contains(&entry.id);
    let response = egui::Frame::new()
        .fill(palette.canvas)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin {
            left: 16,
            right: 12,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                let title_width = ui.available_width() - 170.0;
                let title = one_line(
                    ui,
                    &entry.title(),
                    theme::bold(14.5),
                    palette.ink,
                    title_width.max(80.0),
                );
                let (rect, _) = ui.allocate_exact_size(vec2(title.size().x, 20.0), Sense::hover());
                ui.painter().galley(
                    pos2(rect.left(), rect.center().y - title.size().y / 2.0),
                    title,
                    palette.ink,
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if expandable {
                        let turn = ease(ui.ctx(), Id::new(("chevron", entry.id)), expanded, REVEAL);
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        let icon = if turn > 0.5 { "up" } else { "down" };
                        theme::paint_icon(ui.painter(), rect, icon, palette.muted);
                    }
                    let mut when = model::time_or_date(entry.started_at, model::now());
                    if entry.frontend == "auto" {
                        when = format!("Automatic, {when}");
                    }
                    ui.label(
                        egui::RichText::new(when)
                            .font(theme::font(12.0))
                            .color(palette.muted),
                    );
                    egui::Frame::new()
                        .fill(alpha(color, 0.14))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(entry.result())
                                    .font(theme::bold(11.5))
                                    .color(color),
                            );
                        });
                });
            });
            if !others.is_empty() && !expanded {
                ui.label(
                    egui::RichText::new(format!("and {} more", others.len()))
                        .font(theme::font(12.5))
                        .color(palette.muted),
                );
            }
            if entry.running() {
                let live = app.progress.activity_id == Some(entry.id);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if live && !app.progress.label.is_empty() {
                        ui.label(
                            egui::RichText::new(&app.progress.label)
                                .font(theme::font(12.5))
                                .color(palette.muted),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if live
                            && app.writing
                            && Button::new(Look::Flat, "Cancel").small().show(ui).clicked()
                        {
                            app.cancel();
                        }
                        if live {
                            let count = app.progress.count();
                            if !count.is_empty() {
                                ui.label(
                                    egui::RichText::new(count)
                                        .font(theme::mono(12.0))
                                        .color(palette.muted),
                                );
                            }
                        }
                    });
                });
                let (bar, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 5.0), Sense::hover());
                let fraction = if live { app.progress.bar() } else { None };
                paint_bar(
                    ui,
                    bar,
                    fraction,
                    palette.accent,
                    Id::new(("activity-bar", entry.id)),
                );
            }
            let open = ease(ui.ctx(), Id::new(("expanded", entry.id)), expanded, REVEAL);
            if open > 0.0 && expanded {
                ui.scope(|ui| {
                    ui.multiply_opacity(open);
                    for other in &others {
                        ui.label(
                            egui::RichText::new(other)
                                .font(theme::font(13.0))
                                .color(palette.ink),
                        );
                    }
                    if let Some(finished) = entry.finished_at {
                        ui.label(
                            egui::RichText::new(format!(
                                "Finished {}, took {}",
                                model::time_or_date(finished, model::now()),
                                model::duration(finished - entry.started_at)
                            ))
                            .font(theme::font(12.5))
                            .color(palette.muted),
                        );
                    }
                    if !log.is_empty() {
                        egui::Frame::new()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.line))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt(("log", entry.id))
                                    .max_height(220.0)
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&log)
                                                    .font(theme::mono(12.0))
                                                    .color(palette.ink),
                                            )
                                            .selectable(true),
                                        );
                                    });
                            });
                    }
                });
            }
            header.response
        });
    // The tone bar on the left edge.
    let rect = response.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min + vec2(0.0, 10.0), vec2(3.0, rect.height() - 20.0)),
        CornerRadius::same(2),
        color,
    );
    if expandable {
        let click = ui
            .interact(
                response.inner.rect,
                Id::new(("entry", entry.id)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        if click.clicked() {
            if expanded {
                app.expanded.remove(&entry.id);
            } else {
                app.expanded.insert(entry.id);
            }
        }
    }
}
