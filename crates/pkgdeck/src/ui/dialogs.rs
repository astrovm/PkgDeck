//! Dialogs and the source picker: confirming a change, repositories,
//! adding a Flatpak repository, opening a file or link, source checks and
//! a screenshot up close.

use super::widgets::*;
use crate::{
    app::App,
    model::{self, Page, Tone},
    theme::{self, Palette},
};
use eframe::egui::{
    self, vec2, Color32, CornerRadius, Id, Key, Modifiers, Response, Sense, Stroke, Ui, Vec2,
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
    screenshot(app, ctx);
}

/// A modal card that fades and grows in. Returns false when it should
/// close (Esc or a click outside).
fn modal(ctx: &egui::Context, id: &str, open: bool, width: f32, content: impl FnOnce(&mut Ui)) -> bool {
    let palette = Palette::current(ctx);
    let shown = ease(ctx, Id::new((id, "shown")), open, REVEAL);
    if shown <= 0.0 {
        return open;
    }
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - 32.0);
    let response = egui::Modal::new(Id::new(id))
        .backdrop_color(Color32::from_black_alpha(((if palette.dark { 130.0 } else { 70.0 }) * shown) as u8))
        .frame(
            card(&palette)
                .inner_margin(egui::Margin::same(22))
                .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 40, spread: 0, color: Color32::from_black_alpha(if palette.dark { 120 } else { 45 }) }),
        )
        .area(egui::Modal::default_area(Id::new(id)).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, (1.0 - shown) * 12.0)))
        .show(ctx, |ui| {
            ui.multiply_opacity(shown);
            ui.set_width(width - 44.0);
            ui.set_max_height(screen.height() - 80.0);
            content(ui);
        });
    !(open && response.should_close())
}

fn title(ui: &mut Ui, text: &str) -> bool {
    let palette = Palette::current(ui.ctx());
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(text).font(theme::bold(19.0)).color(palette.ink));
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
    let mut details = app.ui.confirm_details;
    if open {
        let verb = data.verb();
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::Enter) {
                answer = Some(true);
            }
            if i.consume_key(Modifiers::ALT, Key::C) {
                answer = Some(false);
            }
            if let Some(key) = verb.chars().map(|c| c.to_ascii_uppercase()).find(|c| *c != 'C').and_then(|c| Key::from_name(&c.to_string())) {
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
                    let look = if system == value { Look::Soft(Tone::Accent) } else { Look::Secondary };
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
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if let Some(icon) = data.icon.as_deref().filter(|i| !i.is_empty()) {
                        let uri = if icon.starts_with("https://") || icon.starts_with("file://") { icon.to_owned() } else { format!("file://{icon}") };
                        ui.add(egui::Image::new(uri).fit_to_exact_size(Vec2::splat(44.0)).corner_radius(CornerRadius::same(8)));
                    }
                    ui.vertical(|ui| {
                        if let Some(first) = lines.next() {
                            ui.add(egui::Label::new(egui::RichText::new(first).font(theme::bold(15.0)).color(palette.ink)).wrap());
                        }
                        let rest: Vec<&str> = lines.collect();
                        if !rest.is_empty() {
                            ui.add(egui::Label::new(egui::RichText::new(rest.join("\n")).color(palette.muted)).wrap());
                        }
                    });
                });
            });
        if !data.details.is_empty() {
            ui.add_space(6.0);
            if Button::new(Look::Flat, if details { "Hide details" } else { "Show details" }).small().show(ui).clicked() {
                details = !details;
            }
            if details {
                egui::Frame::new()
                    .fill(palette.canvas)
                    .stroke(Stroke::new(1.0, palette.line))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(egui::RichText::new(&data.details).font(theme::mono(12.5)).color(palette.ink)).selectable(true));
                        });
                    });
            }
        }
        ui.add_space(14.0);
        right(ui, |ui| {
            let verb = data.verb();
            let look = if data.removes() { Look::Solid(Tone::Danger) } else { Look::Primary };
            if Button::new(look, &verb).min_width(96.0).tooltip("Ctrl+Enter").show(ui).clicked() {
                answer = Some(true);
            }
            if Button::new(Look::Secondary, "Cancel").min_width(96.0).show(ui).clicked() {
                answer = Some(false);
            }
        });
    });
    app.ui.confirm_details = details && open;
    if let Some(system) = scope {
        app.c().set_open_flatpak_scope(system);
        app.react();
    }
    if open && !still && answer.is_none() {
        answer = Some(false);
    }
    if let Some(approved) = answer {
        app.ui.confirm_details = false;
        app.confirm(approved);
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
        let field = Field::new(&mut app.ui.open_link, "Paste an HTTPS link", Id::new("open-link-field")).icon("external").show(ui);
        if ui.ctx().animate_bool(Id::new("open-link-focus"), true) < 0.5 {
            field.response.request_focus();
        }
        let valid = link_valid(app.ui.open_link.trim());
        preview = field.submitted && valid;
        ui.add_space(14.0);
        right(ui, |ui| {
            if Button::new(Look::Primary, "Preview link").enabled(valid).show(ui).clicked() {
                preview = true;
            }
            if Button::new(Look::Secondary, "Choose file…").icon("package").show(ui).clicked() {
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
        egui::ScrollArea::vertical().max_height(ui.ctx().content_rect().height() * 0.62).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let columns = if ui.available_width() >= 620.0 { 2 } else { 1 };
            let width = (ui.available_width() - 10.0 * (columns - 1) as f32) / columns as f32;
            for chunk in repos.repositories.chunks(columns) {
                ui.horizontal(|ui| {
                    for repo in chunk {
                        egui::Frame::new()
                            .fill(palette.canvas)
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_width(width - 24.0);
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
                                ui.horizontal(|ui| {
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
                                    if flatpak_editable {
                                        right(ui, |ui| {
                                            if icon_button(ui, "remove", "Remove repository", palette.danger, !app.busy).clicked() {
                                                let mut change = target.clone();
                                                change["action"] = json!("remove");
                                                changes.push(change);
                                            }
                                        });
                                    }
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
        if !status.is_empty() && status != "Ready" && status != "Repositories loaded." {
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
        app.ui.add_repo = Some(AddRepo { user: !repos.features.flatpak_system, ..AddRepo::default() });
    }
    for change in changes {
        app.change_repository(change);
    }
}

fn name_valid(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}
fn repo_url_valid(url: &str) -> bool {
    url.strip_prefix("https://")
        .is_some_and(|rest| rest.len() > ".flatpakrepo".len() && rest.ends_with(".flatpakrepo") && !rest.chars().any(char::is_whitespace))
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
        Field::new(&mut draft.name, "flathub", Id::new("repo-name")).show(ui);
        let name_ok = name_valid(&draft.name);
        if !draft.name.is_empty() && !name_ok {
            ui.label(egui::RichText::new("Use letters, numbers, dots, dashes, or underscores. Do not start with a dash.").color(palette.danger));
        }
        ui.label(egui::RichText::new("Address").color(palette.muted));
        Field::new(&mut draft.url, "https://…/repository.flatpakrepo", Id::new("repo-url")).show(ui);
        let url_ok = repo_url_valid(&draft.url);
        if !draft.url.is_empty() && !url_ok {
            ui.label(egui::RichText::new("Enter an HTTPS .flatpakrepo URL.").color(palette.danger));
        }
        if features.flatpak_user && features.flatpak_system {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("For").color(palette.muted));
                for (label, user) in [("User", true), ("System", false)] {
                    let look = if draft.user == user { Look::Soft(Tone::Accent) } else { Look::Secondary };
                    if Button::new(look, label).small().show(ui).clicked() {
                        draft.user = user;
                    }
                }
            });
        }
        ui.add_space(12.0);
        right(ui, |ui| {
            if Button::new(Look::Primary, "Add").enabled(name_ok && url_ok).show(ui).clicked() {
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
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for failure in &failures {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(model::source_name(&failure.source)).font(theme::bold(14.5)).color(palette.ink));
                    right(ui, |ui| {
                        let dirty = app.page == Page::Search && app.search_due.is_some();
                        if Button::new(Look::Soft(Tone::Accent), "Retry").small().enabled(!app.busy && failure.kind != "unsupported" && !dirty).show(ui).clicked() {
                            retry = Some(failure.source.clone());
                        }
                        if failure.kind != "partial" && app.can_turn_off(&failure.source) && Button::new(Look::Flat, "Turn off").small().show(ui).clicked() {
                            turn_off = Some(failure.source.clone());
                        }
                    });
                });
                ui.add(egui::Label::new(egui::RichText::new(app.failure_reason(&failure.source)).color(palette.muted)).wrap());
                ui.add_space(10.0);
            }
        });
        right(ui, |ui| {
            if Button::new(Look::Secondary, "Copy diagnostics").show(ui).clicked() {
                let mut text = format!("View: {}\nState: {}\n", app.page.name(), app.report.phase);
                for failure in &failures {
                    text.push_str(&format!("{} ({}): {}\n", failure.source, failure.kind, app.failure_reason(&failure.source)));
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

fn screenshot(app: &mut App, ctx: &egui::Context) {
    let open = app.ui.screenshot.is_some();
    let (url, caption) = app.ui.screenshot.clone().unwrap_or_default();
    let mut close = false;
    let screen = ctx.content_rect();
    let still = modal(ctx, "screenshot", open, 1040.0f32.min(screen.width() - 32.0), |ui| {
        let palette = Palette::current(ui.ctx());
        if title(ui, if caption.is_empty() { "Screenshot" } else { &caption }) {
            close = true;
        }
        let max = vec2(ui.available_width(), (screen.height() - 160.0).min(680.0));
        let image = egui::Image::new(url.clone()).max_size(max).corner_radius(CornerRadius::same(8));
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
    });
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
    let page = app.page;
    app.ui.picker_draft = app.effective_sources(page).into_iter().filter(|id| app.usable(id, page)).collect();
}

pub fn source_picker(app: &mut App, ui: &mut Ui, button: &Response) {
    let ctx = ui.ctx().clone();
    let shown = ease(&ctx, Id::new("picker-shown"), app.ui.picker_open, REVEAL);
    if shown <= 0.0 {
        return;
    }
    let palette = Palette::current(&ctx);
    let page = app.page;
    if app.ui.picker_draft.is_empty() && !app.catalog.is_empty() && app.ui.picker_open {
        app.ui.picker_draft = app.effective_sources(page).into_iter().filter(|id| app.usable(id, page)).collect();
    }
    let width = 340.0f32.min(ctx.content_rect().width() - 24.0);
    let pos = egui::pos2(button.rect.right() - width, button.rect.bottom() + 6.0 - (1.0 - shown) * 6.0);
    let mut apply = false;
    let mut reset = false;
    let area = egui::Area::new(Id::new("source-picker"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(&ctx, |ui| {
            ui.multiply_opacity(shown);
            card(&palette)
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if palette.dark { 110 } else { 40 }) })
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
                    let field = Field::new(&mut app.ui.picker_search, "Find a source", Id::new("picker-search")).icon("search").show(ui);
                    if ui.ctx().animate_bool(Id::new("picker-focus"), true) < 0.5 {
                        field.response.request_focus();
                    }
                    ui.add_space(6.0);
                    let search = app.ui.picker_search.trim().to_lowercase();
                    let enabled = app.enabled_sources();
                    let ids = model::all_sources(&app.catalog);
                    egui::ScrollArea::vertical().max_height((ctx.content_rect().height() * 0.6).min(440.0)).show(ui, |ui| {
                        let mut sections: Vec<(&str, Vec<String>)> = model::CATEGORIES.iter().map(|c| (*c, vec![])).collect();
                        let mut unavailable = vec![];
                        for id in ids {
                            if !search.is_empty() && !format!("{} {id}", model::source_name(&id)).to_lowercase().contains(&search) {
                                continue;
                            }
                            if !app.available(&id) {
                                unavailable.push(id);
                                continue;
                            }
                            let category = model::category(&id);
                            if let Some(section) = sections.iter_mut().find(|(c, _)| *c == category) {
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
                                ui.horizontal(|ui| {
                                    let tick = tickbox(ui, checked && usable, can, model::source_name(&id));
                                    let label = ui.add(egui::Label::new(egui::RichText::new(model::source_name(&id)).color(if usable { palette.ink } else { palette.muted })).sense(if can { Sense::click() } else { Sense::hover() }));
                                    if (tick.clicked() || label.clicked()) && can {
                                        if checked {
                                            app.ui.picker_draft.remove(&id);
                                        } else {
                                            app.ui.picker_draft.insert(id.clone());
                                        }
                                    }
                                });
                                if !usable {
                                    let reason = if !app.available(&id) {
                                        model::catalog_entry(&app.catalog, &id).summary
                                    } else if !enabled.contains(&id) {
                                        "Turned off in Sources".into()
                                    } else {
                                        "Not supported in this view".into()
                                    };
                                    ui.indent(("reason", &id), |ui| {
                                        ui.label(egui::RichText::new(reason).font(theme::font(12.0)).color(palette.muted));
                                    });
                                }
                            }
                        }
                        if !unavailable.is_empty() {
                            ui.add_space(4.0);
                            let label = if app.ui.picker_unavailable { "Hide unavailable".to_owned() } else { format!("Show unavailable ({})", unavailable.len()) };
                            if Button::new(Look::Flat, &label).small().show(ui).clicked() {
                                app.ui.picker_unavailable = !app.ui.picker_unavailable;
                            }
                        }
                    });
                    ui.add_space(8.0);
                    right(ui, |ui| {
                        if Button::new(Look::Primary, "Apply").small().enabled(!app.ui.picker_draft.is_empty()).show(ui).clicked() {
                            apply = true;
                        }
                        if Button::new(Look::Secondary, "Reset").small().show(ui).clicked() {
                            reset = true;
                        }
                    });
                });
        });
    if reset {
        let page = app.page;
        app.ui.picker_draft = app.enabled_sources().into_iter().filter(|id| app.usable(id, page)).collect();
    }
    if apply {
        app.ui.picker_open = false;
        let draft: HashSet<String> = app.ui.picker_draft.clone();
        app.set_page_sources(page, draft);
    }
    let clicked_outside = ctx.input(|i| i.pointer.any_click())
        && !area.response.rect.contains(ctx.pointer_interact_pos().unwrap_or_default())
        && !button.rect.contains(ctx.pointer_interact_pos().unwrap_or_default());
    let escape = app.ui.picker_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    if app.ui.picker_open && (clicked_outside || escape) {
        app.ui.picker_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_names_are_checked() {
        assert!(link_valid("https://dl.flathub.org/repo/appstream/org.gimp.GIMP.flatpakref"));
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
        assert!(repo_url_valid("https://dl.flathub.org/repo/flathub.flatpakrepo"));
        assert!(!repo_url_valid("https://.flatpakrepo"));
        assert!(!repo_url_valid("http://x/y.flatpakrepo"));
        assert!(!repo_url_valid("https://x/y.flatpakref"));
    }
}
