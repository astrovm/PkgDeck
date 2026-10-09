//! Draws the window from the [`App`]'s state: the sidebar, the page with
//! its list and details, and what floats above them (the Activity drawer,
//! dialogs, toasts). Each part is its own module.

mod activity;
mod dialogs;
pub mod icons;
mod list;
mod page;
mod settings_page;
pub mod theme;
pub mod widgets;
pub mod window;

use crate::{
    app::{App, WindowRequest},
    model::{self, Page, Tone},
    settings::Appearance,
    theme::Palette,
};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Id, Key, Modifiers, Rect, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};
use std::collections::HashSet;
use widgets::*;

/// What the window remembers that the app core doesn't need.
#[derive(Default)]
pub struct State {
    pub picker_open: bool,
    pub picker_draft: HashSet<String>,
    pub picker_search: String,
    pub picker_unavailable: bool,
    pub open_dialog: bool,
    pub open_link: String,
    pub repos_open: bool,
    pub add_repo: Option<dialogs::AddRepo>,
    pub checks_open: bool,
    pub screenshot: Option<(String, String)>,
    pub confirm_details: bool,
    pub notice_output: bool,
    pub dependencies_open: bool,
    pub launch_draft: Option<page::LaunchDraft>,
    pub launch_save_error: String,
    pub source_draft: Option<(String, String)>,
    pub source_save_error: String,
    pub focus_search: bool,
    pub focus_filter: bool,
    pub focus_list: bool,
    pub list_focused: bool,
    pub keyboard_nav: bool,
    pub reveal_selection: bool,
    pub failed_images: HashSet<String>,
    pub hide_progress_for: Option<u64>,
    pub started: bool,
}

/// Widths the layout switches at.
pub const RAIL_BELOW: f32 = 820.0;
pub const MEDIUM_BELOW: f32 = 748.0;
pub const COMPACT_BELOW: f32 = 560.0;
pub const SPLIT_FROM: f32 = 900.0;

pub fn search_id() -> Id {
    Id::new("pkgdeck-search")
}
pub fn filter_id() -> Id {
    Id::new("pkgdeck-filter")
}

/// One frame of the window.
pub fn show(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    set_reduce_motion(&ctx, app.settings.reduce_motion);
    ctx.set_theme(match app.settings.appearance {
        Appearance::System => egui::ThemePreference::System,
        Appearance::Light => egui::ThemePreference::Light,
        Appearance::Dark => egui::ThemePreference::Dark,
    });
    if !app.ui.started {
        app.ui.started = true;
        app.ui.focus_search = app.page == Page::Search;
    }
    app.refresh_items();
    keyboard(app, &ctx);
    dropped_files(app, &ctx);
    if ctx.input(|i| i.viewport().close_requested()) && !app.close_requested() {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }

    let palette = Palette::current(&ctx);
    let full = ui.max_rect();
    ui.painter()
        .rect_filled(full, CornerRadius::ZERO, palette.canvas);
    let rail = full.width() < RAIL_BELOW;
    let sidebar_width = glide(
        &ctx,
        Id::new("sidebar-width"),
        if rail { 68.0 } else { 224.0 },
        LAYOUT,
    );
    let sidebar = Rect::from_min_size(full.min, vec2(sidebar_width, full.height()));
    let content = Rect::from_min_max(pos2(sidebar.right(), full.top()), full.max);
    sidebar_ui(app, ui, sidebar, rail);
    content_ui(app, ui, content);
    activity::drawer(app, ui, full);
    dialogs::show(app, &ctx);
    toasts(app, ui, content);
}

fn keyboard(app: &mut App, ctx: &egui::Context) {
    let dialog_open = app.confirm_open
        || app.ui.repos_open
        || app.ui.open_dialog
        || app.ui.checks_open
        || app.ui.screenshot.is_some()
        || app.ui.add_repo.is_some()
        || app.ui.picker_open;
    let command = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key));
    for (index, key) in [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
    ]
    .into_iter()
    .enumerate()
    {
        if !dialog_open && command(key) {
            app.open_page(Page::ALL[index]);
            focus_for_page(app);
        }
    }
    if dialog_open {
        return;
    }
    if command(Key::Comma) {
        app.open_page(Page::Settings);
    }
    if command(Key::J) {
        app.drawer_open = !app.drawer_open;
        if app.drawer_open {
            app.c().refresh_activity();
            app.react();
        }
    }
    if command(Key::Q) {
        app.quit();
    }
    if command(Key::F) {
        match app.page {
            Page::Installed => app.ui.focus_filter = true,
            Page::Updates | Page::Clean => app.ui.picker_open = true,
            _ => {
                if app.page != Page::Search {
                    app.open_page(Page::Search);
                }
                app.ui.focus_search = true;
            }
        }
    }
    if command(Key::L) && !app.items.is_empty() {
        app.ui.focus_list = true;
    }
    if command(Key::R) && app.page.lists() {
        app.reload(true, true);
    }
    let can_change = !app.busy || app.writing;
    if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::U))
        && app.page == Page::Updates
        && can_change
    {
        app.upgrade_updates();
    }
    if command(Key::I) && can_change {
        app.propose("install");
    }
    if command(Key::D)
        && can_change
        && app
            .selected_row()
            .is_some_and(|row| model::can_remove(row, &app.catalog))
    {
        app.propose("remove");
    }
    if command(Key::U) && can_change {
        app.propose("upgrade");
    }
    let refresh = if cfg!(target_os = "macos") {
        ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::R))
    } else {
        command(Key::M)
    };
    if refresh && can_change {
        app.propose("refresh");
    }
    let back = ctx.input_mut(|i| {
        i.consume_key(Modifiers::ALT, Key::ArrowLeft)
            || (cfg!(target_os = "macos") && i.consume_key(Modifiers::COMMAND, Key::OpenBracket))
            || i.pointer.button_pressed(egui::PointerButton::Extra1)
    });
    if back && (app.opened.is_some() || app.page_open) && !app.review_on_page {
        app.close_page();
        app.ui.focus_list = true;
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        if app.drawer_open {
            app.drawer_open = false;
        } else if app.review_on_page {
            app.confirm(false);
        } else if app.opened.is_some() || app.page_open {
            app.close_page();
            app.ui.focus_list = true;
        } else if app.selected.is_some() {
            app.deselect();
        } else if app.page == Page::Search && !app.query.is_empty() {
            app.query.clear();
            app.typed_query();
        }
    }
    if cfg!(target_os = "macos") && command(Key::W) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn focus_for_page(app: &mut App) {
    match app.page {
        Page::Search => app.ui.focus_search = true,
        Page::Settings => {}
        _ => app.ui.focus_list = true,
    }
}

fn dropped_files(app: &mut App, ctx: &egui::Context) {
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    if dropped.len() == 1 {
        let path = dropped[0].path().to_owned();
        if !path.as_os_str().is_empty() {
            app.open_file(&path);
        }
    }
}

// ---------------------------------------------------------------------------
// Sidebar

fn sidebar_ui(app: &mut App, ui: &mut Ui, rect: Rect, rail: bool) {
    let ctx = ui.ctx().clone();
    let palette = Palette::current(&ctx);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, palette.surface);
    painter.line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0, palette.line),
    );
    let margin = if rail { 12.0 } else { 16.0 };
    let inner = rect.shrink2(vec2(margin, 0.0));
    // The logo and name, level with the page's heading.
    let logo = Rect::from_min_size(
        pos2(
            inner.left()
                + (if rail {
                    (inner.width() - 30.0) / 2.0
                } else {
                    4.0
                }),
            rect.top() + 24.0,
        ),
        Vec2::splat(30.0),
    );
    egui::Image::new(egui::include_image!("../../assets/logo.svg"))
        .fit_to_exact_size(logo.size())
        .paint_at(ui, logo);
    let name_shown = ease(&ctx, Id::new("sidebar-names"), !rail, LAYOUT);
    if name_shown > 0.02 {
        painter.text(
            pos2(logo.right() + 10.0, logo.center().y),
            Align2::LEFT_CENTER,
            "PkgDeck",
            theme::bold(19.0),
            alpha(palette.ink, name_shown),
        );
    }
    // The sections.
    let entry_height = 40.0;
    let top = rect.top() + 84.0;
    let current = Page::ALL.iter().position(|p| *p == app.page).unwrap_or(0);
    let pill_y = glide(
        &ctx,
        Id::new("nav-pill"),
        top + current as f32 * (entry_height + 4.0),
        LAYOUT,
    );
    let pill = Rect::from_min_size(
        pos2(inner.left(), pill_y),
        vec2(inner.width(), entry_height),
    );
    painter.rect_filled(pill, CornerRadius::same(10), palette.selection);
    if !rail {
        painter.rect_filled(
            Rect::from_min_size(
                pos2(pill.left(), pill.top() + 10.0),
                vec2(3.0, entry_height - 20.0),
            ),
            CornerRadius::same(2),
            palette.accent,
        );
    }
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let entry = Rect::from_min_size(
            pos2(inner.left(), top + index as f32 * (entry_height + 4.0)),
            vec2(inner.width(), entry_height),
        );
        let response = ui
            .interact(entry, Id::new(("nav", page.name())), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                true,
                page == app.page,
                page.name(),
            )
        });
        let hover = ease(
            &ctx,
            response.id.with("hover"),
            response.hovered() && page != app.page,
            FEEDBACK,
        );
        painter.rect_filled(
            entry,
            CornerRadius::same(10),
            alpha(palette.ink, hover * 0.05),
        );
        let is_current = page == app.page;
        let color = if is_current {
            palette.accent
        } else {
            mix(palette.muted, palette.ink, hover)
        };
        let icon_x = if rail {
            entry.center().x - 10.0
        } else {
            entry.left() + 14.0
        };
        theme::paint_icon(
            &painter,
            Rect::from_min_size(pos2(icon_x, entry.center().y - 10.0), Vec2::splat(20.0)),
            page.icon(),
            color,
        );
        if name_shown > 0.02 {
            painter.text(
                pos2(entry.left() + 46.0, entry.center().y),
                Align2::LEFT_CENTER,
                page.name(),
                if is_current {
                    theme::bold(14.5)
                } else {
                    theme::font(14.5)
                },
                alpha(
                    if is_current {
                        palette.ink
                    } else {
                        mix(palette.muted, palette.ink, 0.55 + hover * 0.45)
                    },
                    name_shown,
                ),
            );
        }
        // Updates waiting, from the last background check.
        if page == Page::Updates && app.background.available > 0 {
            let count = app.background.available.to_string();
            let pop = ui.ctx().animate_bool_with_time_and_easing(
                response.id.with("badge"),
                true,
                secs(&ctx, REVEAL),
                egui::emath::easing::back_out,
            );
            let center = if rail {
                pos2(entry.center().x + 12.0, entry.top() + 9.0)
            } else {
                pos2(entry.right() - 20.0, entry.center().y)
            };
            paint_badge(ui, center, &count, palette.accent, palette.accent_ink, pop);
        }
        if rail {
            response.clone().on_hover_text(page.name());
        }
        if response.clicked() {
            app.open_page(page);
            focus_for_page(app);
        }
    }
}

// ---------------------------------------------------------------------------
// The page

fn content_ui(app: &mut App, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let margin = if rect.width() + 224.0 < RAIL_BELOW {
        14.0
    } else {
        28.0
    };
    let inner = rect.shrink2(vec2(margin, 0.0));
    let inner = Rect::from_min_max(
        pos2(inner.left(), rect.top() + 20.0),
        pos2(inner.right(), rect.bottom() - margin.min(20.0)),
    );
    // Each page fades and rises into place.
    let entered = progress_since(&ctx, app.page_changed.elapsed().as_secs_f32(), 0.22);
    let lift = (1.0 - entered) * 10.0;
    let inner = inner.translate(vec2(0.0, lift));
    ui.scope_builder(UiBuilder::new().max_rect(inner).id_salt("content"), |ui| {
        ui.multiply_opacity(0.35 + 0.65 * entered);
        ui.spacing_mut().item_spacing = vec2(10.0, 12.0);
        header(app, ui);
        if app.opened.is_some() {
            let rest = ui.available_rect_before_wrap();
            page::opened_page(app, ui, rest);
            return;
        }
        match app.page {
            Page::Search => search_field(app, ui),
            Page::Installed => installed_filters(app, ui),
            Page::Settings => {
                settings_page::show(app, ui);
                return;
            }
            _ => {}
        }
        if app.opening {
            opening_row(app, ui);
        }
        progress_line(app, ui);
        notice_banner(app, ui);
        failure_banner(app, ui);
        let actions = page_actions_height(app);
        let mut rest = ui.available_rect_before_wrap();
        rest.max.x = inner.right();
        let main = Rect::from_min_max(rest.min, pos2(rest.right(), rest.bottom() - actions));
        main_area(app, ui, main);
        if actions > 0.0 {
            let row = Rect::from_min_max(pos2(rest.left(), main.bottom() + 12.0), rest.max);
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(row)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    page_actions(app, ui);
                },
            );
        }
    });
}

fn header(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let width = ui.available_width();
    let compact = width < 600.0;
    ui.horizontal(|ui| {
        ui.set_min_height(38.0);
        ui.label(
            egui::RichText::new(app.page.name())
                .font(theme::bold(26.0))
                .color(palette.ink),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.page.filters_sources() && app.opened.is_none() {
                let filtered = app.page_sources.contains_key(&app.page);
                let label = app.source_summary(app.page);
                let response = Button::new(
                    if filtered {
                        Look::Soft(Tone::Accent)
                    } else {
                        Look::Secondary
                    },
                    &label,
                )
                .icon("filter")
                .icon_only(compact)
                .show(ui);
                if response.clicked() {
                    app.ui.picker_open = !app.ui.picker_open;
                    if app.ui.picker_open {
                        dialogs::open_picker(app);
                    }
                }
                dialogs::source_picker(app, ui, &response);
            }
            let working = app.writing;
            let count = app.queued_count() + usize::from(working);
            let response = Button::new(
                if working {
                    Look::Soft(Tone::Accent)
                } else {
                    Look::Secondary
                },
                if working { "Working" } else { "Activity" },
            )
            .icon("activity")
            .icon_only(compact)
            .tooltip("Activity (Ctrl+J)")
            .show(ui);
            let pop = ui.ctx().animate_bool_with_time_and_easing(
                Id::new("activity-badge"),
                count > 0,
                secs(ui.ctx(), REVEAL),
                egui::emath::easing::back_out,
            );
            if count > 0 || pop > 0.0 {
                paint_badge(
                    ui,
                    response.rect.right_top() + vec2(-4.0, 4.0),
                    &count.max(1).to_string(),
                    palette.accent,
                    palette.accent_ink,
                    pop,
                );
            }
            if response.clicked() {
                app.drawer_open = !app.drawer_open;
                if app.drawer_open {
                    app.c().refresh_activity();
                    app.react();
                }
            }
            let can_open_files = !model::file_patterns(&app.catalog).is_empty();
            if matches!(app.page, Page::Search | Page::Sources)
                && can_open_files
                && app.opened.is_none()
            {
                let response = Button::new(Look::Secondary, "Install from file…")
                    .icon("package")
                    .icon_only(compact)
                    .tooltip("Install from a file or link")
                    .enabled(!app.writing)
                    .show(ui);
                if response.clicked() {
                    app.c().check_sources();
                    app.react();
                    app.ui.open_link.clear();
                    app.ui.open_dialog = true;
                }
            }
        });
    });
}

fn search_field(app: &mut App, ui: &mut Ui) {
    let field = Field::new(&mut app.query, "Search apps and packages", search_id())
        .icon("search")
        .large()
        .show(ui);
    if app.ui.focus_search {
        app.ui.focus_search = false;
        app.ui.focus_list = false;
        field.response.request_focus();
    }
    if field.response.changed() || field.cleared {
        app.typed_query();
    }
    if field.submitted {
        app.submit_search();
        field.response.request_focus();
    }
    if field.response.has_focus()
        && ui.input(|i| i.key_pressed(Key::ArrowDown))
        && !app.items.is_empty()
    {
        app.ui.focus_list = true;
        app.ui.keyboard_nav = true;
        app.choose(0, false);
    }
}

fn installed_filters(app: &mut App, ui: &mut Ui) {
    let compact = ui.available_width() < 600.0;
    ui.horizontal(|ui| {
        let adoptable = app
            .rows
            .iter()
            .filter(|row| row.source == "appimage" && model::can_adopt(row, &app.catalog))
            .count();
        let reserve = if compact { 40.0 } else { 200.0 }
            + if adoptable >= 2 && !compact {
                200.0
            } else {
                0.0
            };
        let width = (ui.available_width() - reserve).max(160.0);
        let field = Field::new(&mut app.filter, "Filter installed packages", filter_id())
            .icon("filter")
            .width(width)
            .show(ui);
        if app.ui.focus_filter {
            app.ui.focus_filter = false;
            app.ui.focus_list = false;
            field.response.request_focus();
        }
        if field.response.changed() || field.cleared {
            app.invalidate();
        }
        if field.response.has_focus()
            && ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::Enter))
            && !app.items.is_empty()
        {
            app.ui.focus_list = true;
            app.ui.keyboard_nav = true;
            app.choose(0, false);
        }
        let has_packages = app.rows.iter().any(model::Row::is_package);
        if has_packages || app.duplicates_only {
            let checked = app.duplicates_only;
            let tick = tickbox(ui, checked, true, "Duplicate installs")
                .on_hover_text("Only apps installed more than once");
            let label = if compact {
                tick.clone()
            } else {
                ui.add(egui::Label::new("Duplicate installs").sense(Sense::click()))
            };
            if tick.clicked() || label.clicked() {
                app.duplicates_only = !checked;
                app.invalidate();
            }
        }
        if adoptable >= 2 && !compact {
            let label = format!("Manage {adoptable} AppImages");
            if Button::new(Look::Soft(Tone::Accent), &label)
                .icon("install")
                .tooltip(format!("Manage {adoptable} AppImages with PkgDeck"))
                .enabled(app.can_act() && app.retained.is_none())
                .show(ui)
                .clicked()
            {
                app.propose("adopt-all");
            }
        }
    });
}

fn opening_row(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    ui.horizontal(|ui| {
        spinner(ui, 18.0, palette.accent);
        ui.label(egui::RichText::new("Opening…").color(palette.muted));
        if Button::new(Look::Flat, "Cancel").small().show(ui).clicked() {
            app.cancel();
        }
    });
}

/// The running change: what it is, how far along, and Cancel.
pub fn progress_line(app: &mut App, ui: &mut Ui) {
    if !app.writing || app.progress.label.is_empty() {
        return;
    }
    if app.drawer_open {
        return;
    }
    progress_card(app, ui, "progress-line");
}

pub fn progress_card(app: &mut App, ui: &mut Ui, salt: &str) {
    let palette = Palette::current(ui.ctx());
    let progress = app.progress.clone();
    card(&palette)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                spinner(ui, 16.0, palette.accent);
                let label_width = (ui.available_width() * 0.42).max(120.0);
                let galley = one_line(
                    ui,
                    &progress.label,
                    theme::bold(14.0),
                    palette.ink,
                    label_width,
                );
                let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x, 20.0), Sense::hover());
                ui.painter().galley(
                    pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
                    galley,
                    palette.ink,
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Button::new(Look::Flat, "Cancel")
                        .small()
                        .icon("cancel")
                        .show(ui)
                        .clicked()
                    {
                        app.cancel();
                    }
                    let count = progress.count();
                    if !count.is_empty() {
                        ui.label(
                            egui::RichText::new(count)
                                .font(theme::mono(12.5))
                                .color(palette.muted),
                        );
                    }
                    let (bar, _) = ui.allocate_exact_size(
                        vec2(ui.available_width().max(40.0), 6.0),
                        Sense::hover(),
                    );
                    paint_bar(
                        ui,
                        bar,
                        progress.bar(),
                        palette.accent,
                        Id::new(("bar", salt)),
                    );
                });
            });
        });
}

fn notice_banner(app: &mut App, ui: &mut Ui) {
    let notice = app.notice.clone();
    if notice.is_empty() || notice.is_toast() {
        return;
    }
    let palette = Palette::current(ui.ctx());
    let color = if notice.kind == "error" {
        palette.danger
    } else {
        palette.warning
    };
    egui::Frame::new()
        .fill(alpha(color, if palette.dark { 0.12 } else { 0.08 }))
        .stroke(Stroke::new(1.0, alpha(color, 0.35)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                theme::paint_icon(ui.painter(), icon, "warning", color);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&notice.title)
                            .font(theme::bold(14.5))
                            .color(palette.ink),
                    );
                    if !notice.detail.is_empty() {
                        ui.label(egui::RichText::new(&notice.detail).color(palette.muted));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "cancel", "Dismiss", palette.muted, true).clicked() {
                        app.c().dismiss_notice();
                        app.react();
                        app.ui.notice_output = false;
                    }
                    if notice.retry
                        && Button::new(Look::Soft(Tone::Accent), "Retry")
                            .small()
                            .icon("refresh")
                            .enabled(!app.busy)
                            .show(ui)
                            .clicked()
                    {
                        app.c().retry_change();
                        app.react();
                    }
                    if !notice.output.is_empty() {
                        let label = if app.ui.notice_output {
                            "Hide details"
                        } else {
                            "Show details"
                        };
                        if Button::new(Look::Flat, label).small().show(ui).clicked() {
                            app.ui.notice_output = !app.ui.notice_output;
                        }
                    }
                });
            });
            if app.ui.notice_output && !notice.output.is_empty() {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(palette.canvas)
                    .stroke(Stroke::new(1.0, palette.line))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .id_salt("notice-output")
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&notice.output)
                                            .font(theme::mono(12.5))
                                            .color(palette.ink),
                                    )
                                    .selectable(true),
                                );
                            });
                    });
                if Button::new(Look::Flat, "Copy").small().show(ui).clicked() {
                    ui.ctx().copy_text(notice.output.clone());
                }
            }
        });
}

fn failure_banner(app: &mut App, ui: &mut Ui) {
    if !app.page.lists() || app.page == Page::Sources {
        return;
    }
    let failures = app.read_failures();
    if failures.is_empty() || app.items.is_empty() {
        return;
    }
    let palette = Palette::current(ui.ctx());
    let mut title = model::failure_title(&failures);
    if app.page == Page::Updates {
        title.push_str(". Update all will retry the check.");
    }
    egui::Frame::new()
        .fill(alpha(
            palette.warning,
            if palette.dark { 0.10 } else { 0.07 },
        ))
        .stroke(Stroke::new(1.0, alpha(palette.warning, 0.3)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                theme::paint_icon(ui.painter(), icon, "warning", palette.warning);
                ui.label(egui::RichText::new(&title).color(palette.ink));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Button::new(Look::Flat, "Details")
                        .small()
                        .show(ui)
                        .clicked()
                    {
                        app.ui.checks_open = true;
                    }
                    if let [one] = failures.as_slice() {
                        if one.kind != "partial" && app.can_turn_off(&one.source) {
                            let label = format!("Turn off {}", model::source_name(&one.source));
                            if Button::new(Look::Flat, &label).small().show(ui).clicked() {
                                let source = one.source.clone();
                                app.set_source_enabled(&source, false);
                            }
                        }
                    }
                    if Button::new(Look::Soft(Tone::Accent), "Retry")
                        .small()
                        .icon("refresh")
                        .enabled(!app.busy)
                        .show(ui)
                        .clicked()
                    {
                        app.reload(true, true);
                    }
                });
            });
        });
}

/// The list, with the selected row's page beside or over it.
fn main_area(app: &mut App, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let selected_panel = app.selected_row().map(|row| row.is_package());
    let showing_page = app.page_open && selected_panel == Some(true);
    let showing_panel = selected_panel == Some(false);
    let wants_side = showing_page || showing_panel;
    let split = rect.width() >= SPLIT_FROM;
    let open = ease(&ctx, Id::new("details-open"), wants_side, LAYOUT);
    if !split {
        // Narrow windows show one at a time: the page slides over the list.
        if wants_side || open > 0.0 {
            let offset = (1.0 - open) * 40.0;
            if open < 1.0 {
                list::list_card(app, ui, rect);
            }
            let page_rect = rect.translate(vec2(offset, 0.0));
            ui.scope_builder(
                UiBuilder::new().max_rect(page_rect).id_salt("narrow-page"),
                |ui| {
                    ui.multiply_opacity(open);
                    ui.painter().rect_filled(
                        page_rect,
                        CornerRadius::same(12),
                        alpha(Palette::current(&ctx).canvas, open),
                    );
                    if wants_side {
                        page::side(app, ui, page_rect, true);
                    }
                },
            );
        } else {
            list::list_card(app, ui, rect);
        }
        return;
    }
    let share = if app.settings.details_width > 0.0 {
        app.settings.details_width.clamp(0.3, 0.65)
    } else {
        0.44
    };
    let side_width = (rect.width() * share).clamp(340.0, rect.width() - 380.0) * open;
    let gap = 14.0 * open;
    let list_rect = Rect::from_min_max(
        rect.min,
        pos2(rect.right() - side_width - gap, rect.bottom()),
    );
    list::list_card(app, ui, list_rect);
    if open > 0.0 {
        let side_rect = Rect::from_min_max(pos2(list_rect.right() + gap, rect.top()), rect.max);
        // The splitter between them.
        let handle = Rect::from_center_size(
            pos2(list_rect.right() + gap / 2.0, rect.center().y),
            vec2(12.0, rect.height()),
        );
        let response = ui
            .interact(handle, Id::new("details-splitter"), Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        let active = response.hovered() || response.dragged();
        let shown = ease(&ctx, response.id.with("hover"), active, FEEDBACK);
        if shown > 0.0 {
            ui.painter().rect_filled(
                Rect::from_center_size(handle.center(), vec2(3.0, 44.0)),
                CornerRadius::same(2),
                alpha(Palette::current(&ctx).accent, shown),
            );
        }
        if response.dragged() {
            let width = rect.right() - ctx.pointer_interact_pos().map_or(side_rect.left(), |p| p.x);
            app.settings.details_width = (width / rect.width()).clamp(0.3, 0.65);
        }
        if response.double_clicked() {
            app.settings.details_width = 0.0;
        }
        let slide = (1.0 - open) * 24.0;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(side_rect.translate(vec2(slide, 0.0)))
                .id_salt("side-page"),
            |ui| {
                ui.multiply_opacity(open);
                if wants_side {
                    page::side(app, ui, side_rect.translate(vec2(slide, 0.0)), false);
                }
            },
        );
    }
}

fn page_actions_height(app: &App) -> f32 {
    let any = match app.page {
        Page::Updates => app
            .items
            .iter()
            .any(|i| app.shown_rows()[i.raw].is_package()),
        Page::Clean => app.items.len() > 1,
        Page::Sources => true,
        _ => false,
    };
    if any {
        CONTROL_HEIGHT + 12.0
    } else {
        0.0
    }
}

fn page_actions(app: &mut App, ui: &mut Ui) {
    let width = ui.available_width();
    match app.page {
        Page::Updates => {
            let rows = app.shown_rows();
            let total = app
                .items
                .iter()
                .filter(|i| rows[i.raw].is_package())
                .count();
            let unchecked = app
                .items
                .iter()
                .filter(|i| app.unchecked.contains(&rows[i.raw].identity()))
                .count();
            let checked = total - unchecked;
            let label = if unchecked == 0 {
                "Update all".to_owned()
            } else if width < 560.0 {
                "Update".to_owned()
            } else {
                format!("Update selected ({checked})")
            };
            if checked > 0
                && Button::new(Look::Primary, &label)
                    .icon("updates")
                    .tooltip("Ctrl+Shift+U")
                    .enabled(!(app.busy && !app.writing))
                    .show(ui)
                    .clicked()
            {
                app.upgrade_updates();
            }
            let narrow = width < 420.0;
            if unchecked > 0
                && Button::new(Look::Secondary, "Select all")
                    .icon("installed")
                    .icon_only(narrow)
                    .show(ui)
                    .clicked()
            {
                app.unchecked.clear();
            }
            if checked > 0
                && Button::new(Look::Secondary, "Select none")
                    .icon("cancel")
                    .icon_only(narrow)
                    .show(ui)
                    .clicked()
            {
                let rows = app.shown_rows();
                let all: HashSet<String> =
                    app.items.iter().map(|i| rows[i.raw].identity()).collect();
                app.unchecked = all;
            }
        }
        Page::Clean => {
            if Button::new(Look::Solid(Tone::Danger), "Clean all")
                .icon("remove")
                .enabled(app.can_act() && app.retained.is_none())
                .show(ui)
                .clicked()
            {
                app.propose("clean-all");
            }
        }
        Page::Sources => {
            let selected = app.selected_row().map(|row| row.source.clone());
            if let Some(source) = selected {
                if Button::new(Look::Primary, "Refresh sources")
                    .icon("refresh")
                    .enabled((!app.busy || app.writing) && app.available(&source))
                    .show(ui)
                    .clicked()
                {
                    app.propose("refresh");
                }
            }
            let repos = ["flatpak", "fwupd", "apt", "dnf", "zypper"]
                .iter()
                .any(|id| app.available(id));
            if repos
                && Button::new(Look::Secondary, "Repositories")
                    .icon("sources")
                    .enabled(!app.busy)
                    .show(ui)
                    .clicked()
            {
                app.ui.repos_open = true;
                app.c().load_repositories();
                app.react();
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Toasts

fn toasts(app: &mut App, ui: &mut Ui, content: Rect) {
    let ctx = ui.ctx().clone();
    let palette = Palette::current(&ctx);
    let mut bottom = content.bottom() - 24.0 - page_actions_height(app);
    for restart in [true, false] {
        let toast = if restart {
            app.restart_toast.clone()
        } else {
            app.toast.clone()
        };
        let id = Id::new(("toast", restart));
        let shown = ease(&ctx, id, toast.is_some(), REVEAL);
        let Some(toast) = toast else { continue };
        let width = 460.0f32.min(content.width() - 32.0);
        let text = wrapped(
            ui,
            &toast.text,
            theme::font(14.0),
            palette.ink,
            width - 120.0,
            3,
        );
        let height = (text.size().y + 28.0).max(52.0);
        let rect = Rect::from_min_size(
            pos2(
                content.center().x - width / 2.0,
                bottom - height + (1.0 - shown) * 14.0,
            ),
            vec2(width, height),
        );
        bottom = rect.top() - 10.0;
        let area = egui::Area::new(id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .show(&ctx, |ui| {
                ui.multiply_opacity(shown);
                let (frame_rect, response) = ui.allocate_exact_size(rect.size(), Sense::hover());
                ui.painter().add(
                    egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 18,
                        spread: 0,
                        color: Color32::from_black_alpha(if palette.dark { 90 } else { 30 }),
                    }
                    .as_shape(frame_rect, CornerRadius::same(12)),
                );
                ui.painter().rect(
                    frame_rect,
                    CornerRadius::same(12),
                    palette.surface,
                    Stroke::new(1.0, palette.strong_line),
                    StrokeKind::Inside,
                );
                let color = tone(&palette, toast.tone);
                let icon = match toast.tone {
                    Tone::Success => "installed",
                    Tone::Danger | Tone::Warning => "warning",
                    _ => "info",
                };
                theme::paint_icon(
                    ui.painter(),
                    Rect::from_center_size(
                        pos2(frame_rect.left() + 24.0, frame_rect.center().y),
                        Vec2::splat(20.0),
                    ),
                    icon,
                    color,
                );
                ui.painter().galley(
                    pos2(
                        frame_rect.left() + 46.0,
                        frame_rect.center().y - text.size().y / 2.0,
                    ),
                    text.clone(),
                    palette.ink,
                );
                let close = Rect::from_center_size(
                    pos2(frame_rect.right() - 24.0, frame_rect.center().y),
                    Vec2::splat(30.0),
                );
                let mut ui_close = ui.new_child(UiBuilder::new().max_rect(close));
                if icon_button(&mut ui_close, "cancel", "Close", palette.muted, true).clicked() {
                    if restart {
                        app.restart_toast = None;
                    } else {
                        app.hide_toast();
                    }
                }
                if let Some(action) = toast.action {
                    let label = match action {
                        crate::app::ToastAction::Undo => "Undo",
                        crate::app::ToastAction::Restart => "Restart",
                    };
                    let action_rect = Rect::from_min_size(
                        pos2(close.left() - 86.0, frame_rect.center().y - 15.0),
                        vec2(80.0, 30.0),
                    );
                    let mut ui_action = ui.new_child(
                        UiBuilder::new()
                            .max_rect(action_rect)
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    if Button::new(Look::Soft(Tone::Accent), label)
                        .small()
                        .show(&mut ui_action)
                        .clicked()
                    {
                        app.toast_action(action);
                    }
                }
                response.hovered()
            });
        // Hovering keeps it up.
        if area.inner {
            if restart {
                if let Some(t) = &mut app.restart_toast {
                    t.shown = std::time::Instant::now();
                }
            } else if let Some(t) = &mut app.toast {
                t.shown = std::time::Instant::now();
            }
        }
    }
}

/// The window's state for the shell around it after a frame.
pub fn window_request(app: &mut App) -> WindowRequest {
    std::mem::take(&mut app.window_request)
}
