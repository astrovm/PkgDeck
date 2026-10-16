//! Dialogs and the source picker: confirming a change, repositories,
//! adding a Flatpak repository, opening a file or link, source checks,
//! first-run setup and a screenshot up close.

use super::widgets::*;
use crate::{
    app::App,
    model::{self, Page, Tone},
    theme::{self, Palette},
};
use eframe::egui::{
    self, pos2, vec2, Color32, Id, Key, Modifiers, Rect, Response, Sense, Stroke, Ui, Vec2,
};
use serde_json::json;
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct AddRepo {
    pub name: String,
    pub url: String,
    pub user: bool,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    confirm(app, ctx);
    open_link(app, ctx);
    repositories(app, ctx);
    add_repository(app, ctx);
    source_checks(app, ctx);
    setup(app, ctx);
    screenshot(app, ctx);
}

/// A modal card that fades and grows in. Returns false when it should
/// close (Esc or a click outside).
fn modal(
    ctx: &egui::Context,
    id: &str,
    open: bool,
    width: f32,
    content: impl FnOnce(&mut Ui),
) -> bool {
    let palette = Palette::current(ctx);
    let shown = ease(ctx, Id::unique((id, "shown")), open, REVEAL);
    if shown <= 0.0 {
        return open;
    }
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - 32.0);
    let dim = Color32::from_black_alpha(((if palette.dark { 130.0 } else { 70.0 }) * shown) as u8);
    let card = card(&palette)
        .inner_margin(egui::Margin::same(22))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(if palette.dark { 120 } else { 45 }),
        });
    // egui's own backdrop is square, so it would fill the see-through
    // corners of a rounded window. The page dims here instead, inside
    // them, under the card.
    let response = egui::Modal::new(Id::unique(id))
        .backdrop_color(Color32::TRANSPARENT)
        .frame(egui::Frame::NONE)
        .area(
            egui::Modal::default_area(Id::unique(id))
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, (1.0 - shown) * 12.0)),
        )
        .show(ctx, |ui| {
            ctx.layer_painter(ui.layer_id()).rect_filled(
                screen,
                super::frame::fit(ctx, screen),
                dim,
            );
            card.show(ui, |ui| {
                ui.multiply_opacity(shown);
                ui.set_width(width - 44.0);
                ui.set_max_height(screen.height() - 80.0);
                content(ui);
            });
        });
    !(open && response.should_close())
}

fn title(ui: &mut Ui, text: &str) -> bool {
    let palette = Palette::current(ui.ctx());
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(text)
                .font(theme::bold(19.0))
                .color(palette.ink),
        );
        right(ui, |ui| {
            close = icon_button(ui, "cancel", "Close (Esc)", palette.muted, true).clicked();
        });
    });
    ui.add_space(8.0);
    close
}

// ---------------------------------------------------------------------------

fn confirm(app: &mut App, ctx: &egui::Context) {
    let open = app.confirm_open && app.confirmation.is_some();
    let data = app.confirmation.clone().unwrap_or_default();
    let text = app.confirmation_text();
    let mut answer: Option<bool> = None;
    let mut scope: Option<bool> = None;
    let mut flipped = app.ui.confirm_details_flipped;
    if open {
        let verb = data.verb();
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::Enter) {
                answer = Some(true);
            }
            if i.consume_key(Modifiers::ALT, Key::C) {
                answer = Some(false);
            }
            if let Some(key) = verb
                .chars()
                .map(|c| c.to_ascii_uppercase())
                .find(|c| *c != 'C')
                .and_then(|c| Key::from_name(&c.to_string()))
            {
                if i.consume_key(Modifiers::ALT, key) {
                    answer = Some(true);
                }
            }
        });
    }
    let still = modal(ctx, "confirm", open, 600.0, |ui| {
        let palette = Palette::current(ui.ctx());
        if title(ui, "Confirm changes") {
            answer = Some(false);
        }
        if !data.flatpak_ref_scope.is_empty() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Install for").color(palette.muted));
                let system = data.flatpak_ref_scope == "system";
                for (label, value) in [("User", false), ("System", true)] {
                    let look = if system == value {
                        Look::Soft(Tone::Accent)
                    } else {
                        Look::Secondary
                    };
                    if Button::new(look, label).small().show(ui).clicked() && system != value {
                        scope = Some(value);
                    }
                }
            });
            ui.add_space(6.0);
        }
        let summary = [data.summary.as_str(), data.body.as_str(), text.as_str()]
            .into_iter()
            .find(|t| !t.is_empty())
            .unwrap_or_default()
            .to_owned();
        let mut lines = summary.lines();
        egui::Frame::new()
            .fill(palette.selection)
            .corner_radius(theme::round(10))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if let Some(icon) = data.icon.as_deref().filter(|i| !i.is_empty()) {
                        let uri = if icon.starts_with("https://") || icon.starts_with("file://") {
                            icon.to_owned()
                        } else {
                            format!("file://{icon}")
                        };
                        ui.add(
                            egui::Image::new(uri)
                                .fit_to_exact_size(Vec2::splat(44.0))
                                .corner_radius(theme::round(8)),
                        );
                    }
                    ui.vertical(|ui| {
                        if let Some(first) = lines.next() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(first)
                                        .font(theme::bold(15.0))
                                        .color(palette.ink),
                                )
                                .wrap(),
                            );
                        }
                        let rest: Vec<&str> = lines.collect();
                        if !rest.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(rest.join("\n")).color(palette.muted),
                                )
                                .wrap(),
                            );
                        }
                    });
                });
            });
        if data.has_details() {
            ui.add_space(6.0);
            let details = data.short() != flipped;
            if Button::new(
                Look::Flat,
                if details {
                    "Hide details"
                } else {
                    "Show details"
                },
            )
            .small()
            .icon(if details { "up" } else { "down" })
            .show(ui)
            .clicked()
            {
                flipped = !flipped;
            }
            if details {
                egui::Frame::new()
                    .fill(palette.canvas)
                    .stroke(Stroke::new(1.0, palette.line))
                    .corner_radius(theme::round(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        super::scrollbar::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                // Leave room for the scroll bar beside long versions.
                                ui.set_width(ui.available_width() - 14.0);
                                detail_list(ui, &data, &palette);
                            });
                    });
            }
        }
        ui.add_space(14.0);
        right(ui, |ui| {
            let verb = data.verb();
            let look = if data.removes() {
                Look::Solid(Tone::Danger)
            } else {
                Look::Primary
            };
            if Button::new(look, &verb)
                .min_width(96.0)
                .tooltip("Ctrl+Enter")
                .show(ui)
                .clicked()
            {
                answer = Some(true);
            }
            if Button::new(Look::Secondary, "Cancel")
                .min_width(96.0)
                .show(ui)
                .clicked()
            {
                answer = Some(false);
            }
        });
    });
    app.ui.confirm_details_flipped = flipped && open;
    if let Some(system) = scope {
        app.c().set_open_flatpak_scope(system);
        app.react();
    }
    if open && !still && answer.is_none() {
        answer = Some(false);
    }
    if let Some(approved) = answer {
        app.ui.confirm_details_flipped = false;
        app.confirm(approved);
    }
}

/// What a confirmation changes: rows grouped by source, each with its
/// version change and size, then any text that goes with them.
fn detail_list(ui: &mut Ui, data: &model::Confirmation, palette: &Palette) {
    ui.spacing_mut().item_spacing.y = 3.0;
    let mut groups: Vec<(&str, Vec<&model::ConfirmItem>)> = Vec::new();
    for item in &data.items {
        match groups.iter_mut().find(|(name, _)| *name == item.group) {
            Some((_, rows)) => rows.push(item),
            None => groups.push((&item.group, vec![item])),
        }
    }
    for (index, (group, rows)) in groups.iter().enumerate() {
        if index > 0 {
            ui.add_space(8.0);
        }
        if !group.is_empty() {
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                theme::paint_icon(ui.painter(), icon, &rows[0].icon_name, palette.muted);
                ui.label(
                    egui::RichText::new(*group)
                        .font(theme::bold(13.0))
                        .color(palette.ink),
                );
                ui.label(
                    egui::RichText::new(format!("· {}", rows.len()))
                        .font(theme::font(12.5))
                        .color(palette.muted),
                );
            });
            ui.add_space(2.0);
        }
        for item in rows {
            detail_row(ui, item, !group.is_empty(), palette);
        }
    }
    let text = data.details.trim();
    if !text.is_empty() {
        if !data.items.is_empty() {
            ui.add_space(8.0);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .font(theme::font(13.0))
                    .color(palette.muted),
            )
            .selectable(true),
        );
    }
}

fn detail_row(ui: &mut Ui, item: &model::ConfirmItem, grouped: bool, palette: &Palette) {
    egui::Sides::new().shrink_left().truncate().show(
        ui,
        |ui| {
            if grouped {
                ui.add_space(22.0);
            }
            if let Some(icon) = item.icon.as_deref().filter(|icon| !icon.is_empty()) {
                let uri = if icon.starts_with("https://") || icon.starts_with("file://") {
                    icon.to_owned()
                } else {
                    format!("file://{icon}")
                };
                ui.add(
                    egui::Image::new(uri)
                        .fit_to_exact_size(Vec2::splat(18.0))
                        .corner_radius(theme::round(4)),
                );
            } else if !grouped && !item.icon_name.is_empty() {
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                theme::paint_icon(ui.painter(), icon, &item.icon_name, palette.muted);
            }
            ui.add(
                egui::Label::new(egui::RichText::new(&item.title).color(palette.ink)).truncate(),
            );
        },
        |ui| {
            if let Some(size) = &item.size {
                ui.label(
                    egui::RichText::new(size)
                        .font(theme::font(12.5))
                        .color(palette.muted),
                );
            }
            match (item.from.as_deref(), item.to.as_deref()) {
                (Some(from), Some(to)) if from != to => {
                    ui.label(
                        egui::RichText::new(to)
                            .font(theme::mono(12.5))
                            .color(palette.accent),
                    );
                    ui.label(
                        egui::RichText::new("→")
                            .font(theme::mono(12.5))
                            .color(palette.muted),
                    );
                    ui.label(
                        egui::RichText::new(from)
                            .font(theme::mono(12.5))
                            .color(palette.muted),
                    );
                }
                (_, Some(version)) | (Some(version), None) => {
                    ui.label(
                        egui::RichText::new(version)
                            .font(theme::mono(12.5))
                            .color(palette.muted),
                    );
                }
                (None, None) => {}
            }
        },
    );
    let indent = if grouped { 22 } else { 0 };
    let beside = |ui: &mut Ui, label: egui::Label| {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: indent,
                ..Default::default()
            })
            .show(ui, |ui| ui.add(label.wrap()));
    };
    if !item.note.is_empty() {
        let note = egui::RichText::new(&item.note)
            .font(theme::font(12.5))
            .color(palette.muted);
        beside(ui, egui::Label::new(note));
    }
    if !item.preview.trim().is_empty() {
        let preview = egui::RichText::new(item.preview.trim())
            .font(theme::mono(12.0))
            .color(palette.muted);
        beside(ui, egui::Label::new(preview).selectable(true));
        ui.add_space(4.0);
    }
}

// ---------------------------------------------------------------------------

fn link_valid(text: &str) -> bool {
    let rest = text.strip_prefix("flatpak+").unwrap_or(text);
    rest.strip_prefix("https://")
        .is_some_and(|tail| !tail.is_empty() && !tail.chars().any(char::is_whitespace))
}

fn open_link(app: &mut App, ctx: &egui::Context) {
    let open = app.ui.open_dialog;
    let mut close = false;
    let mut choose_file = false;
    let mut preview = false;
    let still = modal(ctx, "open-link", open, 480.0, |ui| {
        if title(ui, "Install from a file or link") {
            close = true;
        }
        let field = Field::new(
            &mut app.ui.open_link,
            "Paste an HTTPS link",
            Id::unique("open-link-field"),
        )
        .icon("external")
        .show(ui);
        if ui.ctx().animate_bool(Id::unique("open-link-focus"), true) < 0.5 {
            field.response.request_focus();
        }
        let valid = link_valid(app.ui.open_link.trim());
        preview = field.submitted && valid;
        ui.add_space(14.0);
        right(ui, |ui| {
            if Button::new(Look::Primary, "Preview link")
                .enabled(valid)
                .show(ui)
                .clicked()
            {
                preview = true;
            }
            if Button::new(Look::Secondary, "Choose file…")
                .icon("package")
                .show(ui)
                .clicked()
            {
                choose_file = true;
            }
        });
    });
    if open && (!still || close) {
        app.ui.open_dialog = false;
    }
    if preview {
        app.ui.open_dialog = false;
        let link = app.ui.open_link.trim().to_owned();
        app.open_input(&link);
    }
    if choose_file {
        app.ui.open_dialog = false;
        let patterns = model::file_patterns(&app.catalog);
        let mut dialog = rfd::FileDialog::new().set_title("Open installation file");
        if !patterns.is_empty() {
            dialog = dialog.add_filter("Packages and sources", &patterns);
        }
        if let Some(path) = dialog.pick_file() {
            app.open_file(&path);
        }
    }
}

// ---------------------------------------------------------------------------

fn repositories(app: &mut App, ctx: &egui::Context) {
    let open = app.ui.repos_open;
    let repos = app.repositories.clone();
    let mut close = false;
    let mut changes: Vec<serde_json::Value> = vec![];
    let mut add = false;
    let mut reload = false;
    let still = modal(ctx, "repositories", open, 850.0, |ui| {
        let palette = Palette::current(ui.ctx());
        if title(ui, "Repositories") {
            close = true;
        }
        ui.horizontal(|ui| {
            let features = &repos.features;
            if (features.flatpak_user || features.flatpak_system)
                && Button::new(Look::Soft(Tone::Accent), "Add Flatpak repository").icon("add").enabled(!app.busy).small().show(ui).clicked()
            {
                add = true;
            }
            if features.apt_editor && Button::new(Look::Secondary, "Edit APT sources").enabled(!app.busy).small().show(ui).clicked() {
                changes.push(json!({"backend": "apt", "name": "sources", "scope": "system", "action": "open_editor"}));
            }
            right(ui, |ui| {
                if icon_button(ui, "refresh", "Reload", palette.muted, !app.busy).clicked() {
                    reload = true;
                }
                if app.busy {
                    spinner(ui, 16.0, palette.accent);
                }
            });
        });
        ui.add_space(8.0);
        super::scrollbar::vertical().max_height(ui.ctx().content_rect().height() * 0.62).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let columns = if ui.available_width() >= 620.0 { 2 } else { 1 };
            let width = (ui.available_width() - 10.0 * (columns - 1) as f32) / columns as f32;
            for chunk in repos.repositories.chunks(columns) {
                ui.horizontal_top(|ui| {
                    for repo in chunk {
                        egui::Frame::new()
                            .fill(palette.canvas)
                            .corner_radius(theme::round(10))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                // The card stacks its lines: the row's
                                // left-to-right layout would set them side
                                // by side and widen the dialog.
                                ui.set_width(width - 24.0);
                                ui.vertical(|ui| {
                                    let flatpak_editable = repo.backend == "flatpak" && if repo.scope == "system" { repos.features.flatpak_system } else { repos.features.flatpak_user };
                                    let editable = repo.backend == "fwupd" || flatpak_editable;
                                    let target = json!({"backend": repo.backend, "name": repo.name, "scope": repo.scope});
                                    ui.horizontal(|ui| {
                                        let tick = tickbox(ui, repo.enabled, editable && !app.busy, "Enabled");
                                        if tick.clicked() {
                                            let mut change = target.clone();
                                            change["action"] = json!("set_enabled");
                                            change["enabled"] = json!(!repo.enabled);
                                            changes.push(change);
                                        }
                                        ui.vertical(|ui| {
                                            let name = if repo.title.is_empty() { &repo.name } else { &repo.title };
                                            ui.add(egui::Label::new(egui::RichText::new(name).font(theme::bold(14.0)).color(palette.ink)).truncate());
                                            let scope = if repo.scope == "system" { "System" } else { "User" };
                                            ui.label(egui::RichText::new(format!("{}, {scope}", model::source_name(&repo.backend))).font(theme::font(12.5)).color(palette.muted));
                                            if !repo.url.is_empty() && !name.contains(&repo.url) {
                                                ui.add(egui::Label::new(egui::RichText::new(&repo.url).font(theme::font(12.0)).color(palette.muted)).truncate());
                                            }
                                        });
                                    });
                                    if repo.priority.is_some() || flatpak_editable { ui.horizontal(|ui| {
                                        if let Some(priority) = repo.priority {
                                            ui.label(egui::RichText::new(format!("Priority {priority}")).font(theme::font(12.5)).color(palette.muted));
                                            if flatpak_editable {
                                                for (icon, tip, delta, ok) in [("up", "Raise priority", 1, priority < 9999), ("down", "Lower priority", -1, priority > 0)] {
                                                    if icon_button(ui, icon, tip, palette.muted, ok && !app.busy).clicked() {
                                                        let mut change = target.clone();
                                                        change["action"] = json!("set_priority");
                                                        change["priority"] = json!(priority + delta);
                                                        changes.push(change);
                                                    }
                                                }
                                            }
                                        }
                                        if flatpak_editable && icon_button(ui, "remove", "Remove repository", palette.danger, !app.busy).clicked() {
                                            let mut change = target.clone();
                                            change["action"] = json!("remove");
                                            changes.push(change);
                                        }
                                    }); }
                                });
                            });
                    }
                });
                ui.add_space(6.0);
            }
        });
        for error in &repos.errors {
            ui.label(egui::RichText::new(error).color(palette.muted));
        }
        let status = app.ctl.status().to_string();
        if app.busy && !status.is_empty() {
            ui.label(egui::RichText::new(status).color(palette.muted));
        }
    });
    if open && (!still || close) {
        app.ui.repos_open = false;
    }
    if reload {
        app.c().load_repositories();
        app.react();
    }
    if add {
        app.ui.add_repo = Some(AddRepo {
            user: !repos.features.flatpak_system,
            ..AddRepo::default()
        });
    }
    for change in changes {
        app.change_repository(change);
    }
}

fn name_valid(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}
fn repo_url_valid(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(|rest| {
        rest.len() > ".flatpakrepo".len()
            && rest.ends_with(".flatpakrepo")
            && !rest.chars().any(char::is_whitespace)
    })
}

fn add_repository(app: &mut App, ctx: &egui::Context) {
    let open = app.ui.add_repo.is_some();
    let features = app.repositories.features.clone();
    let mut draft = app.ui.add_repo.clone().unwrap_or_default();
    let mut close = false;
    let mut submit = false;
    let still = modal(ctx, "add-repository", open, 480.0, |ui| {
        let palette = Palette::current(ui.ctx());
        if title(ui, "Add Flatpak repository") {
            close = true;
        }
        ui.label(egui::RichText::new("Name").color(palette.muted));
        Field::new(&mut draft.name, "flathub", Id::unique("repo-name")).show(ui);
        let name_ok = name_valid(&draft.name);
        if !draft.name.is_empty() && !name_ok {
            ui.label(
                egui::RichText::new(
                    "Letters, numbers, dots, dashes or underscores. Can't start with a dash.",
                )
                .color(palette.danger),
            );
        }
        ui.label(egui::RichText::new("Address").color(palette.muted));
        Field::new(
            &mut draft.url,
            "https://…/repository.flatpakrepo",
            Id::unique("repo-url"),
        )
        .show(ui);
        let url_ok = repo_url_valid(&draft.url);
        if !draft.url.is_empty() && !url_ok {
            ui.label(egui::RichText::new("Enter an HTTPS .flatpakrepo URL.").color(palette.danger));
        }
        if features.flatpak_user && features.flatpak_system {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("For").color(palette.muted));
                for (label, user) in [("User", true), ("System", false)] {
                    let look = if draft.user == user {
                        Look::Soft(Tone::Accent)
                    } else {
                        Look::Secondary
                    };
                    if Button::new(look, label).small().show(ui).clicked() {
                        draft.user = user;
                    }
                }
            });
        }
        ui.add_space(12.0);
        right(ui, |ui| {
            if Button::new(Look::Primary, "Add")
                .enabled(name_ok && url_ok)
                .show(ui)
                .clicked()
            {
                submit = true;
            }
            if Button::new(Look::Secondary, "Cancel").show(ui).clicked() {
                close = true;
            }
        });
    });
    if open {
        app.ui.add_repo = Some(draft.clone());
    }
    if open && (!still || close) {
        app.ui.add_repo = None;
    }
    if submit {
        app.ui.add_repo = None;
        app.change_repository(json!({
            "backend": "flatpak",
            "name": draft.name,
            "scope": if draft.user { "user" } else { "system" },
            "action": "add",
            "url": draft.url,
        }));
    }
}

// ---------------------------------------------------------------------------

fn source_checks(app: &mut App, ctx: &egui::Context) {
    let open = app.ui.checks_open;
    let failures = app.read_failures();
    let mut close = false;
    let mut retry: Option<String> = None;
    let mut turn_off: Option<String> = None;
    let still = modal(ctx, "source-checks", open, 620.0, |ui| {
        let palette = Palette::current(ui.ctx());
        if title(ui, "Source checks") {
            close = true;
        }
        super::scrollbar::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                for failure in &failures {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(model::source_name(&failure.source))
                                .font(theme::bold(14.5))
                                .color(palette.ink),
                        );
                        right(ui, |ui| {
                            let dirty = app.page == Page::Search && app.search_due.is_some();
                            if Button::new(Look::Soft(Tone::Accent), "Retry")
                                .small()
                                .enabled(!app.busy && failure.kind != "unsupported" && !dirty)
                                .show(ui)
                                .clicked()
                            {
                                retry = Some(failure.source.clone());
                            }
                            if failure.kind != "partial"
                                && app.can_turn_off(&failure.source)
                                && Button::new(Look::Flat, "Turn off")
                                    .small()
                                    .show(ui)
                                    .clicked()
                            {
                                turn_off = Some(failure.source.clone());
                            }
                        });
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(app.failure_reason(&failure.source))
                                .color(palette.muted),
                        )
                        .wrap(),
                    );
                    ui.add_space(10.0);
                }
            });
        right(ui, |ui| {
            if Button::new(Look::Secondary, "Copy diagnostics")
                .show(ui)
                .clicked()
            {
                let mut text = format!("View: {}\nState: {}\n", app.page.name(), app.report.phase);
                for failure in &failures {
                    text.push_str(&format!(
                        "{} ({}): {}\n",
                        failure.source,
                        failure.kind,
                        app.failure_reason(&failure.source)
                    ));
                }
                ui.ctx().copy_text(text);
            }
        });
    });
    if open && (!still || close) {
        app.ui.checks_open = false;
    }
    if let Some(source) = retry {
        app.ui.checks_open = false;
        app.retry_source(&source);
    }
    if let Some(source) = turn_off {
        app.ui.checks_open = false;
        app.set_source_enabled(&source, false);
    }
}

/// Shown once, the first time the window opens: what PkgDeck can do on its
/// own. Each switch applies right away, as in Settings.
fn setup(app: &mut App, ctx: &egui::Context) {
    use super::settings_page::{auto_clean_row, auto_update_row, login_row, password_row};
    let open = app.ui.setup_open;
    let mut close = false;
    let still = modal(ctx, "setup", open, 560.0, |ui| {
        if title(ui, "Set up PkgDeck") {
            close = true;
        }
        ui.spacing_mut().item_spacing.y = 10.0;
        if cfg!(any(target_os = "linux", target_os = "macos")) && app.platform.tray_available() {
            login_row(app, ui);
        }
        auto_update_row(app, ui);
        // Only matters with automatic updates; still there to turn off.
        let approved = !app.settings.system_approval.is_empty();
        password_row(app, ui, app.settings.auto_update || approved);
        auto_clean_row(app, ui);
        ui.add_space(6.0);
        right(ui, |ui| {
            if Button::new(Look::Primary, "Done")
                .min_width(96.0)
                .show(ui)
                .clicked()
            {
                close = true;
            }
        });
    });
    if open && (!still || close) {
        app.ui.setup_open = false;
    }
}

fn screenshot(app: &mut App, ctx: &egui::Context) {
    let open = app.ui.screenshot.is_some();
    let (url, caption) = app.ui.screenshot.clone().unwrap_or_default();
    let mut close = false;
    let screen = ctx.content_rect();
    let still = modal(
        ctx,
        "screenshot",
        open,
        1040.0f32.min(screen.width() - 32.0),
        |ui| {
            let palette = Palette::current(ui.ctx());
            if title(
                ui,
                if caption.is_empty() {
                    "Screenshot"
                } else {
                    &caption
                },
            ) {
                close = true;
            }
            let max = vec2(ui.available_width(), (screen.height() - 160.0).min(680.0));
            let image = egui::Image::new(url.clone())
                .max_size(max)
                .corner_radius(theme::round(8));
            match image.load_for_size(ui.ctx(), max) {
                Ok(egui::load::TexturePoll::Ready { .. }) => {
                    ui.vertical_centered(|ui| {
                        ui.add(image);
                    });
                }
                Ok(_) => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(max.y / 2.0 - 20.0);
                        spinner(ui, 28.0, palette.accent);
                        ui.add_space(max.y / 2.0 - 20.0);
                    });
                }
                Err(_) => {
                    ui.label(egui::RichText::new("Screenshot unavailable").color(palette.muted));
                }
            }
        },
    );
    if open && (!still || close) {
        app.ui.screenshot = None;
    }
}

// ---------------------------------------------------------------------------
// The source picker

pub fn open_picker(app: &mut App) {
    app.c().check_sources();
    app.react();
    app.ui.picker_search.clear();
    app.ui.picker_duplicates = app.duplicates_only;
    let page = app.page;
    app.ui.picker_draft = app
        .effective_sources(page)
        .into_iter()
        .filter(|id| app.usable(id, page))
        .collect();
}

/// Puts the picker's choices to use as soon as one changes.
fn apply_picker(app: &mut App, page: Page) {
    if page == Page::Installed && app.duplicates_only != app.ui.picker_duplicates {
        app.duplicates_only = app.ui.picker_duplicates;
        app.invalidate();
    }
    // Duplicates are found among the loaded rows: only a change of
    // sources needs a reload.
    let current: HashSet<String> = app
        .effective_sources(page)
        .into_iter()
        .filter(|id| app.usable(id, page))
        .collect();
    if app.ui.picker_draft != current {
        app.set_page_sources(page, app.ui.picker_draft.clone());
    }
}

/// Shows every source again, and every package on Installed.
pub fn clear_filter(app: &mut App) {
    let page = app.page;
    app.ui.picker_duplicates = false;
    app.ui.picker_draft = (if page == Page::Clean {
        app.enabled_cleanup_sources()
    } else {
        app.enabled_sources()
    })
    .into_iter()
    .filter(|id| app.usable(id, page))
    .collect();
    if page == Page::Installed && app.duplicates_only {
        app.duplicates_only = false;
        app.invalidate();
    }
    // A choice can name every source, or sources since turned off.
    if app.page_sources.contains_key(&page) {
        app.set_page_sources(page, app.ui.picker_draft.clone());
    }
}

/// Where a picker line's name starts: after the tick and the mark.
const PICKER_TEXT_INSET: f32 = 62.0;

/// A picker line: tick, mark and name, all one click target. True when
/// clicked.
fn picker_row(
    ui: &mut Ui,
    palette: &Palette,
    id: &str,
    name: &str,
    checked: bool,
    enabled: bool,
    mark: impl FnOnce(&mut Ui, Rect, Color32),
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), 30.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, checked, name));
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, theme::round(6), palette.hover);
    }
    let tick = Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), Vec2::splat(22.0));
    paint_tick(
        ui,
        tick,
        Id::unique(("picker-tick", id)),
        checked,
        enabled,
        response.hovered(),
    );
    let ink = if enabled { palette.ink } else { palette.muted };
    let icon = Rect::from_center_size(pos2(rect.left() + 42.0, rect.center().y), Vec2::splat(18.0));
    mark(ui, icon, ink);
    ui.painter().text(
        pos2(rect.left() + PICKER_TEXT_INSET, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::TextStyle::Body.resolve(ui.style()),
        ink,
    );
    let clicked = enabled && response.clicked();
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    clicked
}

pub fn source_picker(app: &mut App, ui: &mut Ui, button: &Response) {
    let ctx = ui.ctx().clone();
    let shown = ease(&ctx, Id::unique("picker-shown"), app.ui.picker_open, REVEAL);
    if shown <= 0.0 {
        return;
    }
    let palette = Palette::current(&ctx);
    let page = app.page;
    if app.ui.picker_draft.is_empty() && !app.catalog.is_empty() && app.ui.picker_open {
        app.ui.picker_draft = app
            .effective_sources(page)
            .into_iter()
            .filter(|id| app.usable(id, page))
            .collect();
    }
    let width = 340.0f32.min(ctx.content_rect().width() - 24.0);
    let pos = egui::pos2(
        button.rect.right() - width,
        button.rect.bottom() + 6.0 - (1.0 - shown) * 6.0,
    );
    let mut changed = false;
    let area = egui::Area::new(Id::unique("source-picker"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(&ctx, |ui| {
            ui.multiply_opacity(shown);
            card(&palette)
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 28,
                    spread: 0,
                    color: Color32::from_black_alpha(if palette.dark { 110 } else { 40 }),
                })
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    if app.catalog.is_empty() {
                        ui.horizontal(|ui| {
                            spinner(ui, 16.0, palette.accent);
                            ui.label(egui::RichText::new("Checking sources…").color(palette.muted));
                        });
                        return;
                    }
                    let field = Field::new(
                        &mut app.ui.picker_search,
                        "Find a source",
                        Id::unique("picker-search"),
                    )
                    .icon("search")
                    .show(ui);
                    if ui.ctx().animate_bool(Id::unique("picker-focus"), true) < 0.5 {
                        field.response.request_focus();
                    }
                    ui.add_space(6.0);
                    // Installed only, and only while something is installed
                    // twice. Stays while on, so it can be turned off.
                    if page == Page::Installed
                        && (app.duplicates_only
                            || app.rows.iter().any(|row| !row.same_app_from.is_empty()))
                    {
                        let on = app.ui.picker_duplicates;
                        if picker_row(
                            ui,
                            &palette,
                            "duplicates",
                            "Only duplicates",
                            on,
                            true,
                            |ui, rect, color| {
                                theme::paint_icon(ui.painter(), rect, "duplicates", color);
                            },
                        ) {
                            app.ui.picker_duplicates = !on;
                            changed = true;
                        }
                        ui.add_space(2.0);
                        ui.separator();
                    }
                    let search = app.ui.picker_search.trim().to_lowercase();
                    let enabled = if page == Page::Clean {
                        app.enabled_cleanup_sources()
                    } else {
                        app.enabled_sources()
                    };
                    let ids = model::all_sources(&app.catalog);
                    super::scrollbar::vertical()
                        // Full width, so the bar sits on the card's edge.
                        .auto_shrink([false, true])
                        .max_height((ctx.content_rect().height() * 0.6).min(440.0))
                        .show(ui, |ui| {
                            let mut sections: Vec<(&str, Vec<String>)> =
                                model::CATEGORIES.iter().map(|c| (*c, vec![])).collect();
                            let mut unavailable = vec![];
                            for id in ids {
                                if !search.is_empty()
                                    && !format!("{} {id}", model::source_name(&id))
                                        .to_lowercase()
                                        .contains(&search)
                                {
                                    continue;
                                }
                                if !app.available(&id) {
                                    unavailable.push(id);
                                    continue;
                                }
                                let category = model::category(&id);
                                if let Some(section) =
                                    sections.iter_mut().find(|(c, _)| *c == category)
                                {
                                    section.1.push(id);
                                }
                            }
                            if app.ui.picker_unavailable {
                                sections.push(("Unavailable", unavailable.clone()));
                            }
                            for (name, ids) in sections {
                                if ids.is_empty() {
                                    continue;
                                }
                                ui.add_space(4.0);
                                caption(ui, name);
                                for id in ids {
                                    let usable = app.usable(&id, page) && enabled.contains(&id);
                                    let checked = app.ui.picker_draft.contains(&id);
                                    let last = checked && app.ui.picker_draft.len() == 1;
                                    let can = usable && !last;
                                    let name = model::source_name(&id);
                                    if picker_row(
                                        ui,
                                        &palette,
                                        &id,
                                        name,
                                        checked && usable,
                                        can,
                                        |ui, rect, color| {
                                            theme::paint_source_icon(
                                                ui.painter(),
                                                rect,
                                                &id,
                                                color,
                                            );
                                        },
                                    ) {
                                        if checked {
                                            app.ui.picker_draft.remove(&id);
                                        } else {
                                            app.ui.picker_draft.insert(id.clone());
                                        }
                                        changed = true;
                                    }
                                    if !usable {
                                        let reason = if !app.available(&id) {
                                            model::catalog_entry(&app.catalog, &id).summary
                                        } else if !enabled.contains(&id) {
                                            "Turned off in Settings".into()
                                        } else {
                                            "Not supported in this view".into()
                                        };
                                        // Lined up with the name, just under it.
                                        ui.add_space(-6.0);
                                        ui.horizontal(|ui| {
                                            ui.add_space(PICKER_TEXT_INSET);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(reason)
                                                        .font(theme::font(12.0))
                                                        .color(palette.muted),
                                                )
                                                .wrap(),
                                            );
                                        });
                                    }
                                }
                            }
                            if !unavailable.is_empty() {
                                ui.add_space(4.0);
                                let label = if app.ui.picker_unavailable {
                                    "Hide unavailable".to_owned()
                                } else {
                                    format!("Show unavailable ({})", unavailable.len())
                                };
                                if Button::new(Look::Flat, &label).small().show(ui).clicked() {
                                    app.ui.picker_unavailable = !app.ui.picker_unavailable;
                                }
                            }
                        });
                });
        });
    if changed {
        apply_picker(app, page);
    }
    let clicked_outside = ctx.input(|i| i.pointer.any_click())
        && !area
            .response
            .rect
            .contains(ctx.pointer_interact_pos().unwrap_or_default())
        && !button
            .rect
            .contains(ctx.pointer_interact_pos().unwrap_or_default());
    let escape =
        app.ui.picker_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    if app.ui.picker_open && (clicked_outside || escape) {
        app.ui.picker_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_names_are_checked() {
        assert!(link_valid(
            "https://dl.flathub.org/repo/appstream/org.gimp.GIMP.flatpakref"
        ));
        assert!(link_valid("flatpak+https://example.org/app"));
        assert!(!link_valid("http://example.org"));
        assert!(!link_valid("https://"));
        assert!(!link_valid("https://exa mple.org"));
        assert!(!link_valid(""));
        assert!(name_valid("flathub"));
        assert!(name_valid("_my.repo-1"));
        assert!(!name_valid("-bad"));
        assert!(!name_valid(""));
        assert!(!name_valid("ñandú"));
        assert!(repo_url_valid(
            "https://dl.flathub.org/repo/flathub.flatpakrepo"
        ));
        assert!(!repo_url_valid("https://.flatpakrepo"));
        assert!(!repo_url_valid("http://x/y.flatpakrepo"));
        assert!(!repo_url_valid("https://x/y.flatpakref"));
    }

    #[test]
    fn the_page_dims_inside_the_windows_rounded_corners() {
        use crate::settings::Style;
        use eframe::egui::epaint::Shape;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 400.0));
        let ctx = egui::Context::default();
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        crate::setup(&ctx, &fixtures, None);
        super::super::widgets::set_reduce_motion(&ctx, true);
        let input = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        for style in [Style::Adwaita, Style::Liquid, Style::Fluent, Style::Modern] {
            let skin = theme::Skin {
                style,
                ..theme::Skin::default()
            };
            let mut output = theme::with_skin(skin, || {
                let frame = |ui: &mut Ui| {
                    let palette = Palette::current(ui.ctx());
                    super::super::frame::show(ui, ui.max_rect(), "PkgDeck", &palette);
                    modal(ui.ctx(), "dim-test", true, 300.0, |ui| {
                        ui.label("Hello");
                    });
                };
                ctx.run_ui(input(), frame).textures_delta.clear();
                ctx.run_ui(input(), frame)
            });
            output.textures_delta.clear();
            fn flatten(shape: &Shape, out: &mut Vec<Shape>) {
                match shape {
                    Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
                    shape => out.push(shape.clone()),
                }
            }
            let mut shapes = Vec::new();
            for clipped in &output.shapes {
                flatten(&clipped.shape, &mut shapes);
            }
            let dim_color =
                Color32::from_black_alpha(if Palette::current(&ctx).dark { 130 } else { 70 });
            let dim = shapes
                .iter()
                .find_map(|shape| match shape {
                    Shape::Rect(rect) if rect.rect == screen && rect.fill == dim_color => {
                        Some(rect.corner_radius)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{style:?}: no dimmed page"));
            let corners = theme::with_skin(skin, || super::super::frame::window_corners(false));
            assert_eq!(dim, corners, "{style:?}");
        }
    }

    fn picker_harness(page: Page, duplicates: bool) -> egui_kittest::Harness<'static, (bool, App)> {
        use crate::{app::Launch, model::Source, platform::Platform, settings::Store};
        use egui_kittest::Harness;
        use pkgdeck_app::controller::ffi::create_synthetic_controller;

        let (store, settings) = Store::open(None, None);
        let mut app = App::with_controller(
            create_synthetic_controller(|_, _, _, _| Ok(pkgdeck_core::engine::Engine::default())),
            settings,
            store,
            Platform::start(|| {}),
            Launch::default(),
        );
        app.catalog = ["apt", "flatpak", "dnf"]
            .map(|source| Source {
                source: source.into(),
                available: true,
                capabilities: vec!["installed".into(), "upgrade".into(), "remove".into()],
                ..Source::default()
            })
            .to_vec();
        app.rows = ["apt", "flatpak"]
            .map(|source| {
                serde_json::from_value(json!({
                    "kind": "package", "name": "fixture", "display_name": "Fixture",
                    "source": source, "scope": "system", "installed": "1.0",
                    "same_app_from": if duplicates { vec![if source == "apt" { "flatpak" } else { "apt" }] } else { vec![] },
                }))
                .unwrap()
            })
            .to_vec();
        app.page = page;
        app.rows_page = Some(page);
        app.invalidate();
        app.refresh_items();
        open_picker(&mut app);
        app.ui.picker_open = true;
        let mut harness = Harness::builder()
            .with_size(vec2(600.0, 700.0))
            .build_ui_state(
                |ui, (ready, app): &mut (bool, App)| {
                    if !*ready {
                        let root =
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
                        crate::setup(ui.ctx(), &root, None);
                        set_reduce_motion(ui.ctx(), true);
                        *ready = true;
                        return;
                    }
                    let button = ui.allocate_rect(
                        Rect::from_min_size(pos2(420.0, 10.0), vec2(160.0, 34.0)),
                        Sense::click(),
                    );
                    source_picker(app, ui, &button);
                },
                (false, app),
            );
        harness.run_steps(4);
        harness
    }

    fn setup_harness() -> egui_kittest::Harness<'static, (bool, App)> {
        use crate::{app::Launch, platform::Platform, settings::Store};
        use egui_kittest::Harness;
        use pkgdeck_app::controller::ffi::create_synthetic_controller;

        let (store, settings) = Store::open(None, None);
        let mut app = App::with_controller(
            create_synthetic_controller(|_, _, _, _| Ok(pkgdeck_core::engine::Engine::default())),
            settings,
            store,
            Platform::start(|| {}),
            Launch::default(),
        );
        app.ask_about_setup();
        let mut harness = Harness::builder()
            .with_size(vec2(700.0, 600.0))
            .build_ui_state(
                |ui, (ready, app): &mut (bool, App)| {
                    if !*ready {
                        let root =
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
                        crate::setup(ui.ctx(), &root, None);
                        set_reduce_motion(ui.ctx(), true);
                        *ready = true;
                        return;
                    }
                    setup(app, &ui.ctx().clone());
                },
                (false, app),
            );
        harness.run_steps(4);
        harness
    }

    #[test]
    fn setup_turns_on_what_runs_on_its_own_and_closes() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let mut harness = setup_harness();
        use egui::accesskit::Role;
        let disabled = |harness: &egui_kittest::Harness<'_, (bool, App)>, label: &str| {
            harness
                .get_by_role_and_label(Role::CheckBox, label)
                .accesskit_node()
                .is_disabled()
        };
        let password = "Skip the password";
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(harness
                .query_by_role_and_label(Role::CheckBox, "Start at login")
                .is_some());
        }
        // Without automatic updates, no password setting to pick.
        assert!(disabled(&harness, password));
        assert!(!disabled(&harness, "Update automatically"));

        harness
            .get_by_role_and_label(Role::CheckBox, "Update automatically")
            .click();
        harness.run_steps(2);
        assert!(harness.state().1.settings.auto_update);
        assert!(harness.state_mut().1.c().auto_update());
        assert!(!disabled(&harness, password));

        harness
            .get_by_role_and_label(Role::CheckBox, "Clean automatically")
            .click();
        harness.run_steps(2);
        assert!(harness.state().1.settings.auto_clean);
        assert!(harness.state_mut().1.c().auto_clean());

        // Done closes it; the choices stay.
        harness.get_by_label("Done").click();
        harness.run_steps(2);
        let app = &harness.state().1;
        assert!(!app.ui.setup_open);
        assert!(app.settings.auto_update && app.settings.auto_clean);
    }

    #[test]
    fn setup_closes_with_its_cross_and_changes_nothing() {
        use egui_kittest::kittest::Queryable;
        let mut harness = setup_harness();
        harness.get_by_label("Close (Esc)").click();
        harness.run_steps(2);
        let app = &harness.state().1;
        assert!(!app.ui.setup_open);
        assert!(!app.settings.auto_update && !app.settings.auto_clean);
        assert!(app.settings.setup_asked);
    }

    fn flat_shapes(harness: &egui_kittest::Harness<'_, (bool, App)>) -> Vec<egui::Shape> {
        fn flatten(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
                shape => out.push(shape.clone()),
            }
        }
        let mut shapes = Vec::new();
        for clipped in &harness.output().shapes {
            flatten(&clipped.shape, &mut shapes);
        }
        shapes
    }

    #[test]
    fn picker_offers_duplicates_only_when_something_is_installed_twice() {
        use egui_kittest::kittest::Queryable;
        let harness = picker_harness(Page::Installed, false);
        assert!(harness.query_by_label("Only duplicates").is_none());
        assert!(harness.query_by_label(model::source_name("apt")).is_some());

        let mut harness = picker_harness(Page::Installed, true);
        harness.get_by_label("Only duplicates").click();
        harness.run_steps(2);
        let app = &harness.state().1;
        // On at once, and the picker stays open for more changes.
        assert!(app.duplicates_only);
        assert!(app.ui.picker_open);
        assert!(harness.query_by_label("Apply").is_none());
        assert!(harness.query_by_label("Reset").is_none());
        // Only the duplicates changed: the sources stay unfiltered, and the
        // loaded rows are filtered without a reload.
        assert!(app.page_sources.is_empty());
        assert!(app.retained.is_none());
        harness.get_by_label("Only duplicates").click();
        harness.run_steps(2);
        assert!(!harness.state().1.duplicates_only);

        // Other pages don't offer it, duplicates or not.
        let harness = picker_harness(Page::Updates, true);
        assert!(harness.query_by_label("Only duplicates").is_none());
        harness.get_by_label(model::source_name("apt"));
    }

    #[test]
    fn picker_applies_each_change_of_sources_at_once() {
        use egui_kittest::kittest::Queryable;
        let mut harness = picker_harness(Page::Installed, true);
        harness.get_by_label(model::source_name("flatpak")).click();
        harness.run_steps(2);
        let app = &harness.state().1;
        assert_eq!(
            app.page_sources.get(&Page::Installed),
            Some(&HashSet::from(["apt".to_owned(), "dnf".to_owned()]))
        );
        assert!(app.ui.picker_open);
        // Flatpak's rows leave now, not when the reload ends.
        assert!(app.shown_rows().iter().all(|row| row.source != "flatpak"));
        assert!(!app.shown_rows().is_empty());

        harness.get_by_label(model::source_name("flatpak")).click();
        harness.run_steps(2);
        // Every source again: no filter left.
        assert!(harness.state().1.page_sources.is_empty());
        assert!(!harness.state().1.duplicates_only);
    }

    #[test]
    fn clearing_the_filter_shows_every_source_and_package() {
        let mut harness = picker_harness(Page::Installed, true);
        let app = &mut harness.state_mut().1;
        app.duplicates_only = true;
        app.set_page_sources(Page::Installed, HashSet::from(["apt".to_owned()]));
        open_picker(app);
        assert!(app.ui.picker_duplicates);
        clear_filter(app);
        assert!(app.page_sources.is_empty());
        assert!(!app.duplicates_only);
        assert!(!app.ui.picker_duplicates);
        assert_eq!(
            app.ui.picker_draft,
            HashSet::from(["apt", "flatpak", "dnf"].map(str::to_owned))
        );

        // Elsewhere it leaves Installed's duplicates alone.
        let mut harness = picker_harness(Page::Updates, true);
        let app = &mut harness.state_mut().1;
        app.duplicates_only = true;
        app.set_page_sources(Page::Updates, HashSet::from(["apt".to_owned()]));
        clear_filter(app);
        assert!(app.page_sources.is_empty());
        assert!(app.duplicates_only);
    }

    #[test]
    fn picker_keeps_duplicates_on_while_turned_on() {
        use egui_kittest::kittest::Queryable;
        let mut harness = picker_harness(Page::Installed, false);
        harness.state_mut().1.duplicates_only = true;
        open_picker(&mut harness.state_mut().1);
        harness.state_mut().1.ui.picker_open = true;
        harness.run_steps(2);
        // No duplicates left, but it's on: still there to turn off.
        harness.get_by_label("Only duplicates").click();
        harness.run_steps(2);
        assert!(!harness.state().1.duplicates_only);
    }

    #[test]
    fn picker_rows_span_the_card_and_the_last_source_stays_ticked() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let mut harness = picker_harness(Page::Installed, true);
        let card_right = 420.0 + 160.0;
        for name in ["Only duplicates", model::source_name("apt")] {
            let rect = harness.get_by_label(name).rect();
            // Out to the card's inner margin, where the scroll bar sits.
            assert!(rect.right() > card_right - 30.0, "{name} {rect:?}");
            assert!(rect.right() <= card_right - 12.0, "{name} {rect:?}");
            assert!(rect.left() < card_right - 300.0, "{name} {rect:?}");
        }
        for source in ["flatpak", "dnf"] {
            harness.get_by_label(model::source_name(source)).click();
            harness.run_steps(2);
        }
        assert_eq!(
            harness.state().1.ui.picker_draft,
            HashSet::from(["apt".to_owned()])
        );
        let apt = harness.get_by_label(model::source_name("apt"));
        assert!(apt.accesskit_node().is_disabled());
        apt.click();
        harness.run_steps(2);
        assert_eq!(harness.state().1.ui.picker_draft.len(), 1);

        // Tick, mark, then name, left to right.
        let row = harness.get_by_label(model::source_name("apt")).rect();
        let shapes = flat_shapes(&harness);
        let name = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == model::source_name("apt") => {
                    Some(text.pos)
                }
                _ => None,
            })
            .expect("the name");
        assert!(
            (name.x - (row.left() + PICKER_TEXT_INSET)).abs() < 0.5,
            "{name:?} {row:?}"
        );
        let within = |rect: Rect| row.contains_rect(rect);
        assert!(shapes.iter().any(|shape| matches!(shape,
            egui::Shape::Rect(tick) if tick.rect.size() == Vec2::splat(18.0)
                && (tick.rect.center().x - (row.left() + 13.0)).abs() < 0.5 && within(tick.rect))));
        assert!(shapes.iter().any(|shape| matches!(shape,
        egui::Shape::Path(path) if {
            let mark = path.visual_bounding_rect();
            within(mark) && mark.left() >= row.left() + 31.0 && mark.right() <= row.left() + 53.0
        })));

        // Only rows that can change light up under the pointer.
        let lit = |harness: &egui_kittest::Harness<'_, (bool, App)>, row: Rect| {
            flat_shapes(harness)
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Rect(fill) if fill.rect == row))
        };
        harness.get_by_label(model::source_name("apt")).hover();
        harness.run_steps(2);
        assert!(!lit(&harness, row));
        let dnf = harness.get_by_label(model::source_name("dnf")).rect();
        harness.get_by_label(model::source_name("dnf")).hover();
        harness.run_steps(2);
        assert!(lit(&harness, dnf));

        // A reason sits just under its row, lined up with the name.
        harness.state_mut().1.ui.picker_unavailable = true;
        harness.run_steps(2);
        let firmware = harness.get_by_label(model::source_name("fwupd")).rect();
        let reason = harness
            .query_all_by_label("Unsupported on this platform")
            .next()
            .expect("a reason")
            .rect();
        // The text's own top: the row's spacing above it, less 6 points.
        let gap = reason.top() - firmware.bottom();
        assert!((0.0..14.0).contains(&gap), "{reason:?} {firmware:?}");
        assert!((reason.left() - (firmware.left() + PICKER_TEXT_INSET)).abs() < 1.0);
        if std::env::var_os("PKGDECK_PICKER_PNG").is_some() {
            let image = harness.render().unwrap();
            image
                .save(std::env::var("PKGDECK_PICKER_PNG").unwrap())
                .unwrap();
        }
    }

    #[test]
    fn repository_cards_stack_their_lines_inside_the_dialog() {
        use crate::{app::Launch, platform::Platform, settings::Store};
        use egui_kittest::{kittest::Queryable, Harness};
        use pkgdeck_app::controller::ffi::create_synthetic_controller;

        let (store, settings) = Store::open(None, None);
        let mut app = App::with_controller(
            create_synthetic_controller(|_, _, _, _| Ok(pkgdeck_core::engine::Engine::default())),
            settings,
            store,
            Platform::start(|| {}),
            Launch::default(),
        );
        let long = "io.github.example.a_very_long_bundle_name_that_goes_on.flatpak";
        app.repositories = serde_json::from_value(json!({
            "repositories": [
                {"backend": "flatpak", "name": "flathub", "title": "Flathub", "scope": "user",
                 "url": "https://dl.flathub.org/repo/", "enabled": true, "priority": 8},
                {"backend": "flatpak", "name": "bundle", "title": long, "scope": "system",
                 "url": "", "enabled": false, "priority": 0}
            ],
            "features": {"flatpak_user": true, "flatpak_system": true}
        }))
        .unwrap();
        app.ui.repos_open = true;
        let width = 1300.0;
        let mut harness = Harness::builder()
            .with_size(vec2(width, 900.0))
            .build_ui_state(
                |ui, (ready, app): &mut (bool, App)| {
                    let ctx = &ui.ctx().clone();
                    if !*ready {
                        let root =
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
                        crate::setup(ctx, &root, None);
                        set_reduce_motion(ctx, true);
                        *ready = true;
                        return;
                    }
                    show(app, ctx);
                },
                (false, app),
            );
        harness.run_steps(6);
        // The dialog keeps its 850 point width, centred.
        let right = (width + 850.0) / 2.0;
        let flathub = harness.get_by_label("Flathub").rect();
        let title = harness.get_by_label_contains("a_very_long").rect();
        for priority in ["Priority 8", "Priority 0"] {
            let priority = harness.get_by_label(priority).rect();
            assert!(priority.right() <= right, "{priority:?}");
            assert!(
                priority.top() >= flathub.bottom(),
                "{priority:?} {flathub:?}"
            );
        }
        assert!(title.right() <= right, "{title:?}");
        assert!(harness.query_by_label("Flatpak, User").is_some());
    }
}
