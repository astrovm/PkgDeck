//! An app's page: beside the list for the selected row, or over the whole
//! page for a file or link that was opened. Sources, cleanup tasks and
//! failures get a smaller panel with their text.

use super::{list::package_icon, widgets::*};
use crate::{
    app::App,
    model::{self, Details, Page, Row, Tone, Variable},
    theme::{self, Palette},
};
use eframe::egui::{
    self, pos2, vec2, Align2, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};
use std::time::Instant;

/// The launch settings being edited, for one row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchDraft {
    pub identity: String,
    pub arguments: String,
    pub environment: Vec<Variable>,
    pub from: (String, Vec<Variable>),
}

/// The selected row's page or panel, in `rect`.
pub fn side(app: &mut App, ui: &mut Ui, rect: Rect, narrow: bool) {
    let Some(row) = app.selected_row().cloned() else {
        return;
    };
    if row.is_package() {
        let details = app.details_for_selection().cloned();
        app_page(app, ui, rect, &row, details, narrow, false);
    } else {
        panel(app, ui, rect, &row, narrow);
    }
}

pub fn opened_page(app: &mut App, ui: &mut Ui, rect: Rect) {
    let Some(opened) = app.opened.clone() else {
        return;
    };
    let row = opened.package.clone().unwrap_or_default();
    let entered = ease(ui.ctx(), Id::new("opened-in"), true, REVEAL);
    let rect = rect.translate(vec2((1.0 - entered) * 32.0, 0.0));
    ui.scope_builder(UiBuilder::new().max_rect(rect).id_salt("opened"), |ui| {
        ui.multiply_opacity(entered);
        app_page(
            app,
            ui,
            rect,
            &row,
            Some(opened),
            rect.width() < 600.0,
            true,
        );
    });
}

fn panel(app: &mut App, ui: &mut Ui, rect: Rect, row: &Row, narrow: bool) {
    let palette = Palette::current(ui.ctx());
    paint_card(ui, rect, &palette);
    let inner = rect.shrink(18.0);
    ui.scope_builder(UiBuilder::new().max_rect(inner).id_salt("panel"), |ui| {
        ui.horizontal(|ui| {
            if narrow
                && Button::new(Look::Flat, "Back")
                    .icon("back")
                    .icon_only(true)
                    .show(ui)
                    .clicked()
            {
                app.deselect();
            }
            let (icon, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
            ui.painter()
                .rect_filled(icon, CornerRadius::same(10), alpha(palette.accent, 0.1));
            let name = match row.kind.as_str() {
                "failure" => "warning",
                _ => row.source.as_str(),
            };
            theme::paint_icon(ui.painter(), icon.shrink(11.0), name, palette.accent);
            ui.vertical(|ui| {
                // Long titles wrap rather than run under the close button.
                ui.set_max_width((ui.available_width() - 40.0).max(80.0));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(row.title())
                            .font(theme::bold(18.0))
                            .color(palette.ink),
                    )
                    .wrap(),
                );
                let source = model::source_name(&row.source);
                if source != row.title() {
                    ui.label(egui::RichText::new(source).color(palette.muted));
                }
            });
            // Narrow windows already have Back for this.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if !narrow
                    && icon_button(ui, "cancel", "Close details", palette.muted, true).clicked()
                {
                    app.deselect();
                    app.ui.focus_list = true;
                }
            });
        });
        if row.kind == "cleanup" {
            if let Some(index) = app.selected_index() {
                ui.add_space(12.0);
                if Button::new(Look::Soft(Tone::Danger), "Clean")
                    .icon("remove")
                    .enabled(app.can_act() && app.retained.is_none())
                    .show(ui)
                    .clicked()
                {
                    app.run_row_action(index);
                }
            }
        }
        if row.kind == "source" && app.page == Page::Sources {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let enabled = app.enabled_sources().contains(&row.source);
                let mut on = enabled;
                let can = row.available.unwrap_or(false)
                    && !app.writing
                    && !(enabled && !app.can_turn_off(&row.source));
                if switch(ui, &mut on, can, "Enabled").changed() {
                    let source = row.source.clone();
                    app.set_source_enabled(&source, on);
                }
                ui.label(
                    egui::RichText::new(if enabled { "On" } else { "Off" }).color(palette.muted),
                );
            });
        }
        ui.add_space(12.0);
        let text = app.details.panel_text(&app.report);
        let text = if text.is_empty() {
            match row.kind.as_str() {
                "cleanup" => format!("{}\n\n{}", row.summary, row.preview),
                _ => row.summary.clone(),
            }
        } else {
            text
        };
        egui::ScrollArea::vertical()
            .id_salt("panel-text")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(text).color(palette.ink))
                        .selectable(true)
                        .wrap(),
                );
                if row.kind == "source" && !row.capabilities.is_empty() {
                    ui.add_space(12.0);
                    caption(ui, "Can");
                    chips(ui, &row.capabilities, &palette);
                }
            });
    });
}

fn paint_card(ui: &Ui, rect: Rect, palette: &Palette) {
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 1],
            blur: 6,
            spread: 0,
            color: egui::Color32::from_black_alpha(if palette.dark { 40 } else { 10 }),
        }
        .as_shape(rect, CornerRadius::same(12)),
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        palette.surface,
        Stroke::new(1.0, palette.line),
        StrokeKind::Inside,
    );
}

fn chips(ui: &mut Ui, items: &[String], palette: &Palette) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for item in items {
            egui::Frame::new()
                .fill(palette.hover_solid())
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(9, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(item)
                            .font(theme::font(12.5))
                            .color(palette.ink),
                    );
                });
        }
    });
}

/// What the page's main button does.
enum Main {
    Opened(String),
    Row(model::Action),
}

#[allow(clippy::too_many_arguments)]
fn app_page(
    app: &mut App,
    ui: &mut Ui,
    rect: Rect,
    row: &Row,
    details: Option<Details>,
    narrow: bool,
    opened: bool,
) {
    let ctx = ui.ctx().clone();
    let palette = Palette::current(&ctx);
    paint_card(ui, rect, &palette);
    let identity = row.identity();
    let raw = if opened {
        None
    } else {
        app.selected_item().map(|item| item.raw)
    };
    let item = if opened {
        None
    } else {
        app.selected_item().cloned()
    };
    let appimage = !opened && row.source == "appimage" && row.is_installed();
    if let (Some(raw), true) = (raw, appimage) {
        app.app_info(raw, &identity);
    }
    let main = if opened {
        details
            .as_ref()
            .map(|d| d.action.clone())
            .filter(|a| !a.is_empty())
            .map(Main::Opened)
    } else {
        model::row_action(row, app.page, &app.catalog).map(Main::Row)
    };
    let launchable =
        appimage && app.launch_settings.error.is_none() && !app.app_file.path.is_empty()
            || opened
                && details.as_ref().is_some_and(|d| {
                    d.package
                        .as_ref()
                        .is_some_and(|p| p.source == "appimage" && p.is_installed())
                });
    let can_act = app.can_act() && app.retained.is_none() && !app.review_on_page;
    let inner = rect.shrink2(vec2(20.0, 18.0));
    ui.scope_builder(UiBuilder::new().max_rect(inner).id_salt(("app-page", &identity)), |ui| {
        // Header.
        ui.horizontal(|ui| {
            if (opened || narrow)
                && Button::new(Look::Flat, "Back").icon("back").icon_only(true).enabled(!app.review_on_page).tooltip("Back").show(ui).clicked()
            {
                app.close_page();
                app.ui.focus_list = true;
            }
            let size = if opened && !narrow { 72.0 } else { 52.0 };
            let (icon, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            package_icon(app, ui, icon, row);
            ui.vertical(|ui| {
                ui.set_max_width((ui.available_width() - 160.0).max(120.0));
                ui.add(egui::Label::new(egui::RichText::new(row.title()).font(theme::bold(if opened { 24.0 } else { 20.0 })).color(palette.ink)).truncate());
                let variants = item.as_ref().map(|i| i.variants.clone()).unwrap_or_default();
                let line = model::source_line(row, variants.len() > 1);
                ui.add(egui::Label::new(egui::RichText::new(line).color(palette.muted)).truncate());
                let version = row.installed.clone().filter(|v| !v.is_empty()).or(row.candidate.clone()).unwrap_or_default();
                if !version.is_empty() {
                    ui.label(egui::RichText::new(version).font(theme::mono(12.5)).color(palette.muted));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if !opened && !narrow && icon_button(ui, "cancel", "Close", palette.muted, true).clicked() {
                    app.close_page();
                    app.ui.focus_list = true;
                }
            });
        });
        ui.add_space(10.0);
        // Actions.
        ui.horizontal_wrapped(|ui| {
            let update_waits = row.has_update();
            if launchable {
                let look = if update_waits { Look::Secondary } else { Look::Primary };
                if Button::new(look, "Launch").icon("launch").show(ui).clicked() {
                    let error = if opened {
                        app.c().launch_opened().to_string()
                    } else {
                        raw.map(|raw| app.c().launch_app(raw as i32).to_string()).unwrap_or_default()
                    };
                    app.launch_error = error;
                }
            }
            match &main {
                Some(Main::Opened(action)) if !launchable => {
                    if Button::new(Look::Solid(Tone::Success), action).icon("install").enabled(can_act).show(ui).clicked() {
                        app.run_page_action();
                    }
                }
                Some(Main::Row(action)) => {
                    let look = match action {
                        model::Action::Remove => Look::Soft(Tone::Danger),
                        _ if launchable && !update_waits => Look::Soft(action.tone()),
                        model::Action::Install => Look::Solid(Tone::Success),
                        _ => Look::Primary,
                    };
                    let running = app.writing && app.active_rows.contains(&identity);
                    if running {
                        if Button::new(Look::Secondary, "Cancel").icon("cancel").show(ui).clicked() {
                            app.cancel();
                        }
                    } else if Button::new(look, action.label())
                        .icon(action.icon())
                        .icon_only(*action == model::Action::Remove && launchable)
                        .enabled(can_act)
                        .show(ui)
                        .clicked()
                    {
                        app.run_page_action();
                    }
                }
                _ => {}
            }
            if !opened && model::can_adopt(row, &app.catalog) && !matches!(main, Some(Main::Row(model::Action::Adopt))) {
                if let Some(index) = app.selected_index() {
                    if Button::new(Look::Soft(Tone::Accent), "Manage").icon("install").enabled(can_act).show(ui).clicked() {
                        app.run_adopt(index);
                    }
                }
            }
            // System or User, for a Flatpak installed both ways.
            if let Some(item) = &item {
                if item.variants.len() > 1 {
                    ui.add_space(6.0);
                    scope_toggle(app, ui, item.variants.clone(), &identity, can_act);
                }
            }
        });
        if !opened && model::can_adopt(row, &app.catalog) && row.source == "appimage" {
            ui.label(egui::RichText::new("Manage moves it into PkgDeck, which then keeps it updated with its menu entry and icon.").font(theme::font(13.0)).color(palette.muted));
        }
        if !app.launch_error.is_empty() {
            ui.label(egui::RichText::new(&app.launch_error).color(palette.danger));
        }
        if appimage && app.app_file.without_fuse {
            ui.label(egui::RichText::new("Starts unpacked: this computer doesn't have FUSE 2 (libfuse2), which this AppImage needs to start the usual way.").color(palette.warning));
        }
        ui.add_space(6.0);
        let progress_here = app.writing && (app.active_rows.contains(&identity) || app.progress.for_row(&identity, &row.source).is_some() || opened);
        if progress_here && !app.progress.label.is_empty() {
            super::progress_card(app, ui, "page-progress");
        }
        ui.painter().line_segment(
            [pos2(ui.min_rect().left(), ui.cursor().top()), pos2(ui.max_rect().right(), ui.cursor().top())],
            Stroke::new(1.0, palette.line),
        );
        ui.add_space(8.0);
        // Body.
        let loaded = details.as_ref().is_some_and(|d| !d.more || opened);
        let fade = progress_since(&ctx, app.details_loaded.elapsed().as_secs_f32(), 0.14);
        egui::ScrollArea::vertical().id_salt(("page-body", &identity)).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if app.review_on_page {
                review_card(app, ui);
                ui.add_space(10.0);
            }
            match details {
                Some(details) if loaded || !details.description.is_empty() => {
                    ui.multiply_opacity(0.55 + 0.45 * fade);
                    body(app, ui, row, &details, raw, appimage, opened, narrow);
                    if details.more && !opened {
                        skeleton(ui, &palette);
                    }
                }
                _ => skeleton(ui, &palette),
            }
        });
    });
}

fn scope_toggle(app: &mut App, ui: &mut Ui, variants: Vec<usize>, identity: &str, enabled: bool) {
    let palette = Palette::current(ui.ctx());
    let current = variants
        .iter()
        .position(|&raw| app.rows.get(raw).is_some_and(|r| r.identity() == identity))
        .unwrap_or(0);
    let (rect, _) = ui.allocate_exact_size(vec2(150.0, 32.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), palette.hover_solid());
    let half = rect.width() / 2.0;
    let x = glide(
        ui.ctx(),
        Id::new(("scope-knob", identity)),
        rect.left() + current as f32 * half,
        LAYOUT,
    );
    ui.painter().rect(
        Rect::from_min_size(pos2(x, rect.top()), vec2(half, rect.height())).shrink(3.0),
        CornerRadius::same(7),
        palette.surface,
        Stroke::new(1.0, palette.line),
        StrokeKind::Inside,
    );
    for (index, label) in ["System", "User"].into_iter().enumerate() {
        let part = Rect::from_min_size(
            pos2(rect.left() + index as f32 * half, rect.top()),
            vec2(half, rect.height()),
        );
        let response = ui
            .interact(
                part,
                Id::new(("scope", label, identity)),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let color = if index == current {
            palette.ink
        } else {
            palette.muted
        };
        ui.painter().text(
            part.center(),
            Align2::CENTER_CENTER,
            label,
            if index == current {
                theme::bold(13.0)
            } else {
                theme::font(13.0)
            },
            color,
        );
        if response.clicked() && index != current {
            app.choose_scope(index);
        }
    }
}

fn skeleton(ui: &mut Ui, palette: &Palette) {
    ui.add_space(6.0);
    let width = ui.available_width();
    for fraction in [0.92, 1.0, 0.78, 0.46] {
        let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        paint_skeleton(
            ui,
            Rect::from_min_size(rect.min, vec2(width * fraction, 10.0)),
            palette.hover_solid(),
        );
    }
}

fn review_card(app: &mut App, ui: &mut Ui) {
    let palette = Palette::current(ui.ctx());
    let Some(data) = app.confirmation.clone() else {
        return;
    };
    let color = if data.removes() {
        palette.danger
    } else {
        palette.accent
    };
    let entered = ease(ui.ctx(), Id::new("review-in"), true, REVEAL);
    egui::Frame::new()
        .fill(alpha(color, 0.09 * entered))
        .stroke(Stroke::new(1.0, alpha(color, 0.35 * entered)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = format!(
                "{}?",
                if data.action.is_empty() {
                    "Apply"
                } else {
                    data.action.trim_end_matches('?')
                }
            );
            ui.label(
                egui::RichText::new(title)
                    .font(theme::bold(16.0))
                    .color(palette.ink),
            );
            for note in &data.notes {
                ui.label(egui::RichText::new(note).color(palette.muted));
            }
            if !data.changes.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(data.changes.join("\n"))
                            .font(theme::font(13.0))
                            .color(palette.muted),
                    )
                    .selectable(true),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let verb = data.verb();
                let look = if data.removes() {
                    Look::Solid(Tone::Danger)
                } else {
                    Look::Primary
                };
                if Button::new(look, &verb)
                    .icon(if data.removes() { "remove" } else { "install" })
                    .show(ui)
                    .clicked()
                {
                    app.confirm(true);
                }
                if Button::new(Look::Secondary, "Cancel").show(ui).clicked() {
                    app.confirm(false);
                }
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn body(
    app: &mut App,
    ui: &mut Ui,
    row: &Row,
    details: &Details,
    raw: Option<usize>,
    appimage: bool,
    opened: bool,
    narrow: bool,
) {
    let palette = Palette::current(ui.ctx());
    // Screenshots.
    if let Some(shots) = details.screenshots.as_ref().filter(|s| !s.is_empty()) {
        let size = if opened && !narrow {
            vec2(360.0, 225.0)
        } else if narrow {
            vec2(200.0, 120.0)
        } else {
            vec2(260.0, 150.0)
        };
        egui::ScrollArea::horizontal()
            .id_salt(("shots", row.identity()))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for shot in shots {
                        if app.ui.failed_images.contains(&shot.url) {
                            continue;
                        }
                        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                        let response = response.on_hover_cursor(CursorIcon::ZoomIn);
                        let hover = ease(ui.ctx(), response.id, response.hovered(), FEEDBACK);
                        let image = egui::Image::new(shot.url.clone())
                            .fit_to_exact_size(size)
                            .corner_radius(CornerRadius::same(10));
                        match image.load_for_size(ui.ctx(), size) {
                            Ok(egui::load::TexturePoll::Ready { texture }) => {
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(10),
                                    palette.hover_solid(),
                                );
                                let fit = egui::Image::new(egui::load::SizedTexture::new(
                                    texture.id,
                                    texture.size,
                                ))
                                .fit_to_exact_size(size)
                                .corner_radius(CornerRadius::same(10));
                                let shown = fit.calc_size(size, Some(texture.size));
                                fit.paint_at(ui, Rect::from_center_size(rect.center(), shown));
                                ui.painter().rect_stroke(
                                    rect,
                                    CornerRadius::same(10),
                                    Stroke::new(
                                        1.0 + hover,
                                        mix(palette.line, palette.accent, hover),
                                    ),
                                    StrokeKind::Inside,
                                );
                            }
                            Ok(_) => paint_skeleton(ui, rect, palette.hover_solid()),
                            Err(_) => {
                                app.ui.failed_images.insert(shot.url.clone());
                            }
                        }
                        if response.clicked() {
                            app.ui.screenshot = Some((shot.url.clone(), shot.caption.clone()));
                        }
                    }
                });
            });
        ui.add_space(10.0);
    }
    let description = if details.description.trim() == row.summary.trim() && !opened {
        ""
    } else {
        details.description.as_str()
    };
    if !row.summary.is_empty() && row.summary.trim() != details.description.trim() {
        ui.label(
            egui::RichText::new(&row.summary)
                .font(theme::bold(15.5))
                .color(palette.ink),
        );
        ui.add_space(4.0);
    }
    if !description.is_empty() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(description)
                    .font(theme::font(14.5))
                    .color(alpha(palette.ink, 0.88))
                    .line_height(Some(22.0)),
            )
            .selectable(true)
            .wrap(),
        );
        ui.add_space(10.0);
    }
    // Facts.
    let mut facts: Vec<(&str, String)> = vec![];
    if appimage {
        let state = update_state(app, row);
        if !state.is_empty() {
            facts.push(("Updates", state));
        }
    }
    if !details.publisher.is_empty() {
        facts.push(("Publisher", details.publisher.clone()));
    }
    if !details.license.is_empty() {
        facts.push(("License", details.license.clone()));
    }
    let file = if appimage && !app.app_file.path.is_empty() {
        app.app_file.path.clone()
    } else {
        details.location.clone()
    };
    if !file.is_empty() {
        facts.push(("File", file));
    }
    if appimage {
        if let Some(bytes) = app.app_file.bytes {
            facts.push(("Size", model::size(bytes)));
        }
        if let Some(modified) = app.app_file.modified {
            facts.push(("Updated", model::short_datetime(modified)));
        }
    }
    if !facts.is_empty() || !details.homepage.is_empty() {
        egui::Grid::new(("facts", row.identity()))
            .num_columns(2)
            .spacing(vec2(18.0, 8.0))
            .show(ui, |ui| {
                for (label, value) in &facts {
                    ui.label(
                        egui::RichText::new(*label)
                            .font(theme::font(13.0))
                            .color(palette.muted),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(value).color(palette.ink))
                            .selectable(true)
                            .wrap(),
                    );
                    ui.end_row();
                }
                if !details.homepage.is_empty() {
                    ui.label(
                        egui::RichText::new("Homepage")
                            .font(theme::font(13.0))
                            .color(palette.muted),
                    );
                    let shown = details
                        .homepage
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_end_matches('/');
                    let link = ui.add(egui::Link::new(
                        egui::RichText::new(format!("{shown} ↗")).color(palette.accent),
                    ));
                    if link.clicked()
                        && (details.homepage.starts_with("https://")
                            || details.homepage.starts_with("http://"))
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&details.homepage));
                    }
                    ui.end_row();
                }
            });
        ui.add_space(10.0);
    }
    if appimage {
        if let Some(folder) = app.app_file.folder.clone() {
            if Button::new(Look::Secondary, "Show in folder")
                .icon("external")
                .small()
                .show(ui)
                .clicked()
            {
                let url = format!(
                    "file://{}",
                    folder.split('/').map(percent).collect::<Vec<_>>().join("/")
                );
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            ui.add_space(8.0);
        }
    }
    // Dependencies.
    if !details.dependencies.is_empty() {
        let open = app.ui.dependencies_open;
        let label = format!("Dependencies ({})", details.dependencies.len());
        let response = ui
            .horizontal(|ui| {
                let (chevron, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                let icon = if open { "down" } else { "right" };
                theme::paint_icon(ui.painter(), chevron, icon, palette.muted);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&label)
                            .font(theme::bold(13.5))
                            .color(palette.ink),
                    )
                    .sense(Sense::click()),
                )
            })
            .inner
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            app.ui.dependencies_open = !open;
        }
        let shown = ease(ui.ctx(), Id::new(("deps", row.identity())), open, REVEAL);
        if shown > 0.0 {
            ui.scope(|ui| {
                ui.multiply_opacity(shown);
                chips(ui, &details.dependencies, &palette);
            });
        }
        ui.add_space(10.0);
    }
    if let (true, Some(raw)) = (appimage, raw) {
        launch_editor(app, ui, raw, &row.identity());
        source_editor(app, ui, raw, &row.identity());
    }
}

fn percent(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn update_state(app: &App, row: &Row) -> String {
    if row.has_update() {
        return match row.candidate.as_deref().filter(|c| !c.is_empty()) {
            Some(version) => format!("Update {version} available"),
            None => "Update available".into(),
        };
    }
    if let Some(github) = app
        .update_source
        .github
        .as_deref()
        .filter(|g| !g.is_empty())
    {
        return format!("From GitHub, {github}");
    }
    if app.update_source.builtin {
        return "Updates itself".into();
    }
    if app.update_source.editable {
        return "No update source yet. Add its GitHub project below.".into();
    }
    String::new()
}

fn launch_editor(app: &mut App, ui: &mut Ui, raw: usize, identity: &str) {
    let settings = app.launch_settings.clone();
    if !settings.editable {
        return;
    }
    let palette = Palette::current(ui.ctx());
    let from = (settings.arguments.clone(), settings.environment.clone());
    if app
        .ui
        .launch_draft
        .as_ref()
        .is_none_or(|d| d.identity != identity || d.from != from)
    {
        app.ui.launch_draft = Some(LaunchDraft {
            identity: identity.to_owned(),
            arguments: settings.arguments.clone(),
            environment: settings.environment.clone(),
            from,
        });
        app.ui.launch_save_error.clear();
    }
    let draft = app.ui.launch_draft.as_mut().expect("draft");
    caption(ui, "Launch settings");
    ui.label(
        egui::RichText::new("Command line arguments")
            .font(theme::font(13.0))
            .color(palette.muted),
    );
    Field::new(
        &mut draft.arguments,
        "--example",
        Id::new(("args", identity)),
    )
    .monospace()
    .show(ui);
    ui.label(
        egui::RichText::new("Environment variables")
            .font(theme::font(13.0))
            .color(palette.muted),
    );
    let mut remove = None;
    for (index, variable) in draft.environment.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 60.0) / 2.0;
            Field::new(
                &mut variable.name,
                "NAME",
                Id::new(("env-name", identity, index)),
            )
            .monospace()
            .width(width)
            .show(ui);
            ui.label("=");
            Field::new(
                &mut variable.value,
                "value",
                Id::new(("env-value", identity, index)),
            )
            .monospace()
            .width(width - 16.0)
            .show(ui);
            if icon_button(ui, "remove", "Remove variable", palette.muted, true).clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        draft.environment.remove(index);
    }
    let edited = draft.arguments != draft.from.0 || draft.environment != draft.from.1;
    let mut save = None;
    ui.horizontal(|ui| {
        if Button::new(Look::Secondary, "Add variable")
            .icon("add")
            .small()
            .show(ui)
            .clicked()
        {
            draft.environment.push(Variable::default());
        }
        if Button::new(Look::Primary, "Save")
            .small()
            .enabled(edited)
            .show(ui)
            .clicked()
        {
            let environment = serde_json::to_string(
                &draft
                    .environment
                    .iter()
                    .map(|v| serde_json::json!({"name": v.name, "value": v.value}))
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default();
            save = Some((draft.arguments.clone(), environment));
        }
    });
    if let Some((arguments, environment)) = save {
        let error = app
            .c()
            .save_app_launch_settings(raw as i32, arguments.into(), environment.into())
            .to_string();
        if error.is_empty() {
            app.reread_app_info();
        }
        app.ui.launch_save_error = error;
    }
    if !app.ui.launch_save_error.is_empty() {
        ui.label(egui::RichText::new(&app.ui.launch_save_error).color(palette.danger));
    }
    ui.add_space(10.0);
}

fn source_editor(app: &mut App, ui: &mut Ui, raw: usize, identity: &str) {
    let source = app.update_source.clone();
    if !source.editable {
        return;
    }
    let palette = Palette::current(ui.ctx());
    let saved = source.github.clone().unwrap_or_default();
    if app
        .ui
        .source_draft
        .as_ref()
        .is_none_or(|(id, _)| id != identity)
    {
        app.ui.source_draft = Some((identity.to_owned(), saved.clone()));
        app.ui.source_save_error.clear();
    }
    caption(ui, "Updates from GitHub");
    let mut submit = false;
    ui.horizontal(|ui| {
        let draft = &mut app.ui.source_draft.as_mut().expect("draft").1;
        let field = Field::new(draft, "owner/name", Id::new(("github", identity)))
            .width((ui.available_width() - 90.0).max(120.0))
            .show(ui);
        let edited = *draft != saved;
        submit = field.submitted && edited;
        if Button::new(Look::Primary, "Save")
            .small()
            .enabled(edited)
            .show(ui)
            .clicked()
        {
            submit = true;
        }
    });
    if submit {
        let github = app
            .ui
            .source_draft
            .as_ref()
            .map(|(_, d)| d.clone())
            .unwrap_or_default();
        let error = app
            .c()
            .save_app_update_source(raw as i32, github.into())
            .to_string();
        if error.is_empty() {
            app.reread_app_info();
            app.ui.source_draft = None;
        }
        app.ui.source_save_error = error;
    }
    if !app.ui.source_save_error.is_empty() {
        ui.label(egui::RichText::new(&app.ui.source_save_error).color(palette.danger));
    }
    let _ = Instant::now();
}
