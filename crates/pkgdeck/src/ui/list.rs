//! The results card: its heading, column titles, rows, and what it shows
//! while loading or when there's nothing to list.

use super::{widgets::*, COMPACT_BELOW, MEDIUM_BELOW};
use crate::{
    app::App,
    model::{self, Column, Page, Row, Tone},
    theme::{self, Palette},
};
use eframe::egui::{
    self, pos2, vec2, Align2, CornerRadius, CursorIcon, Id, Key, Rect, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2,
};

const ROW: f32 = 60.0;
const ROW_COMPACT: f32 = 78.0;
const GROUP_HEADING: f32 = 36.0;

#[derive(Clone, Copy, PartialEq)]
enum Width {
    Wide,
    Medium,
    Compact,
}

pub fn list_card(app: &mut App, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let palette = Palette::current(&ctx);
    if app.page == Page::Search
        && app.query.trim().is_empty()
        && app.items.is_empty()
        && !app.busy
        && !app.opening
    {
        search_prompt(app, ui, rect);
        return;
    }
    let focused = app.ui.list_focused;
    let ring = ease(
        &ctx,
        Id::new("list-ring"),
        focused && app.ui.keyboard_nav,
        REVEAL,
    );
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
        Stroke::new(1.0, mix(palette.line, palette.accent, ring)),
        StrokeKind::Inside,
    );
    let width = if rect.width() < COMPACT_BELOW {
        Width::Compact
    } else if rect.width() < MEDIUM_BELOW {
        Width::Medium
    } else {
        Width::Wide
    };
    let mut top = rect.top();
    let heading_shown =
        !app.items.is_empty() || app.busy || app.writing || app.page == Page::Sources;
    if heading_shown {
        let heading = Rect::from_min_size(rect.min, vec2(rect.width(), 48.0));
        heading_row(app, ui, heading);
        top = heading.bottom();
        ui.painter().line_segment(
            [pos2(rect.left(), top), pos2(rect.right(), top)],
            Stroke::new(1.0, palette.line),
        );
    }
    if width != Width::Compact && !app.items.is_empty() {
        let header = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), 32.0));
        column_titles(app, ui, header, width);
        top = header.bottom();
    }
    let body = Rect::from_min_max(
        pos2(rect.left(), top),
        pos2(rect.right(), rect.bottom() - 4.0),
    );
    if app.items.is_empty() {
        if app.busy && !app.writing {
            skeleton_rows(ui, body, &palette);
        } else {
            empty_state(app, ui, body);
        }
        return;
    }
    rows(app, ui, body, width);
}

fn search_prompt(app: &mut App, ui: &mut Ui, rect: Rect) {
    let palette = Palette::current(ui.ctx());
    let center = pos2(rect.center().x, rect.top() + rect.height() * 0.38);
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(center - vec2(0.0, 40.0), Vec2::splat(40.0)),
        "search",
        alpha(palette.muted, 0.7),
    );
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        "Find apps and packages",
        theme::bold(17.0),
        palette.ink,
    );
    let hint = model::search_hint(
        &app.catalog,
        &app.enabled_sources(),
        &app.settings.search_hint,
    );
    ui.painter().text(
        center + vec2(0.0, 28.0),
        Align2::CENTER_CENTER,
        hint,
        theme::font(14.0),
        palette.muted,
    );
}

fn heading_text(app: &App) -> String {
    let count = app.items.len();
    if app.writing {
        return "Applying changes…".into();
    }
    if app.busy {
        return match app.page {
            Page::Search => "Searching…",
            _ if count > 0 => "Refreshing…",
            _ => "Loading…",
        }
        .into();
    }
    if app.page == Page::Search {
        if let Some(matches) = app.report.matches.filter(|m| *m > count as u64) {
            return format!("Best {count} of {matches} packages. Type more to narrow.");
        }
    }
    let noun = match app.page {
        Page::Sources => "source",
        Page::Clean => "cleanup task",
        Page::Updates => "update",
        _ => "package",
    };
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// "Waiting for APT", "Waiting for APT and Snap", or a count past two.
fn waiting_text(names: &[String]) -> String {
    match names {
        [one] => format!("Waiting for {one}"),
        [one, two] => format!("Waiting for {one} and {two}"),
        _ => format!("Waiting for {} sources", names.len()),
    }
}

fn heading_row(app: &mut App, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let palette = Palette::current(&ctx);
    let inner = rect.shrink2(vec2(16.0, 0.0));
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.label(
                egui::RichText::new(heading_text(app))
                    .font(theme::font(13.5))
                    .color(palette.muted),
            );
            if (app.busy && !app.writing || app.refreshing) && !app.items.is_empty() {
                spinner(ui, 16.0, palette.accent);
            }
            if app.waiting_shown() {
                let names: Vec<String> = app
                    .pending
                    .iter()
                    .filter(|id| {
                        let kind = model::catalog_entry(&app.catalog, id).availability_kind;
                        kind != "unavailable" && kind != "platform"
                    })
                    .map(|id| model::source_name(id).to_owned())
                    .collect();
                if !names.is_empty() {
                    let galley = one_line(
                        ui,
                        &waiting_text(&names),
                        theme::font(13.0),
                        palette.muted,
                        ui.available_width() * 0.5,
                    );
                    let (r, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
                    ui.painter().galley(r.min, galley, palette.muted);
                    if names.len() > 2 {
                        response.on_hover_text(names.join("\n"));
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !app.busy || app.writing {
                    if icon_button(ui, "refresh", "Reload (Ctrl+R)", palette.muted, true).clicked()
                    {
                        app.reload(true, true);
                    }
                } else if Button::new(Look::Flat, "Cancel")
                    .small()
                    .icon("cancel")
                    .show(ui)
                    .clicked()
                {
                    app.cancel();
                }
                if app.page == Page::Sources {
                    let hidden = app
                        .rows
                        .iter()
                        .filter(|r| r.kind == "source" && !r.available.unwrap_or(false))
                        .count();
                    if hidden > 0 || app.show_unavailable {
                        let label = if app.show_unavailable {
                            "Hide unavailable".to_owned()
                        } else {
                            format!("Show unavailable ({hidden})")
                        };
                        if Button::new(Look::Flat, &label).small().show(ui).clicked() {
                            app.show_unavailable = !app.show_unavailable;
                            app.invalidate();
                        }
                    }
                }
            });
        },
    );
}

struct Columns {
    tick: Option<Rect>,
    icon: Option<Rect>,
    name: Rect,
    version: Option<Rect>,
    summary: Option<Rect>,
    actions: Rect,
}

fn columns(row: Rect, width: Width, page: Page, actions: f32) -> Columns {
    let mut x = row.left() + 14.0;
    let tick = (page == Page::Updates).then(|| {
        let r = Rect::from_min_size(pos2(x, row.top()), vec2(26.0, row.height()));
        x += 32.0;
        r
    });
    let icon = (row.width() >= 420.0).then(|| {
        let size = if width == Width::Compact { 40.0 } else { 36.0 };
        let r = Rect::from_min_size(pos2(x, row.top()), vec2(size, row.height()));
        x += size + 12.0;
        r
    });
    let actions_rect = Rect::from_min_max(
        pos2(row.right() - actions - 10.0, row.top()),
        pos2(row.right() - 10.0, row.bottom()),
    );
    let right = actions_rect.left() - 10.0;
    let available = (right - x).max(60.0);
    let (name_w, version_w, summary) = match width {
        Width::Wide => {
            let name = (available * 0.36).clamp(170.0, 340.0);
            let version = (available * 0.2).clamp(110.0, 180.0);
            (name, version, true)
        }
        Width::Medium => {
            let version = (available * 0.32).clamp(100.0, 170.0);
            (available - version - 12.0, version, false)
        }
        Width::Compact => (available, 0.0, false),
    };
    let name = Rect::from_min_size(pos2(x, row.top()), vec2(name_w, row.height()));
    x += name_w + 12.0;
    let version = (width != Width::Compact).then(|| {
        let r = Rect::from_min_size(pos2(x, row.top()), vec2(version_w, row.height()));
        x += version_w + 12.0;
        r
    });
    let summary = (summary && page != Page::Sources && right - x > 40.0)
        .then(|| Rect::from_min_max(pos2(x, row.top()), pos2(right, row.bottom())));
    Columns {
        tick,
        icon,
        name,
        version,
        summary,
        actions: actions_rect,
    }
}

fn column_titles(app: &mut App, ui: &mut Ui, rect: Rect, width: Width) {
    let palette = Palette::current(ui.ctx());
    let cols = columns(rect, width, app.page, 76.0);
    let version_title = match app.page {
        Page::Clean => "TYPE",
        Page::Sources => "STATUS",
        _ => "VERSION",
    };
    let name_title = if app.page == Page::Sources {
        "SOURCE"
    } else {
        "NAME"
    };
    let mut titles = vec![(Column::Name, name_title, cols.name)];
    if let Some(v) = cols.version {
        titles.push((Column::Version, version_title, v));
    }
    if let Some(s) = cols.summary {
        titles.push((Column::Summary, "SUMMARY", s));
    }
    if let (Some(tick), Page::Updates) = (cols.tick, app.page) {
        let rows = app.shown_rows();
        let all_checked = app.unchecked.is_empty();
        let none_checked = app
            .items
            .iter()
            .all(|i| app.unchecked.contains(&rows[i.raw].identity()));
        let id = Id::new("tick-all");
        let response = ui
            .interact(
                Rect::from_center_size(tick.center(), Vec2::splat(22.0)),
                id,
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(if all_checked {
                "Select none"
            } else {
                "Select all"
            });
        paint_tick(
            ui,
            Rect::from_center_size(tick.center(), Vec2::splat(22.0)),
            id,
            all_checked || !none_checked,
            true,
            response.hovered(),
        );
        if response.clicked() {
            if all_checked {
                let rows = app.shown_rows();
                app.unchecked = app.items.iter().map(|i| rows[i.raw].identity()).collect();
            } else {
                app.unchecked.clear();
            }
        }
    }
    for (column, title, area) in titles {
        let response = ui
            .interact(
                Rect::from_min_max(
                    pos2(area.left() - 4.0, rect.top()),
                    pos2(area.right(), rect.bottom()),
                ),
                Id::new(("column", title)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let sorted = app.sort.filter(|(c, _)| *c == column);
        let color = if sorted.is_some() || response.hovered() {
            palette.ink
        } else {
            palette.muted
        };
        let text = match sorted {
            Some((_, true)) => format!("{title}  ▲"),
            Some((_, false)) => format!("{title}  ▼"),
            None => title.to_owned(),
        };
        ui.painter().text(
            pos2(area.left(), rect.center().y),
            Align2::LEFT_CENTER,
            text,
            theme::bold(11.0),
            color,
        );
        if response.clicked() {
            app.sort = match app.sort {
                Some((c, ascending)) if c == column => Some((c, !ascending)),
                _ => Some((column, true)),
            };
            app.invalidate();
        }
    }
    ui.painter().line_segment(
        [
            pos2(rect.left() + 12.0, rect.bottom()),
            pos2(rect.right() - 12.0, rect.bottom()),
        ],
        Stroke::new(1.0, alpha(palette.line, 0.7)),
    );
}

fn row_height(app: &App, index: usize, width: Width) -> f32 {
    let base = if width == Width::Compact {
        ROW_COMPACT
    } else {
        ROW
    };
    base + if app.items[index].group.is_some() {
        GROUP_HEADING
    } else {
        0.0
    }
}

fn rows(app: &mut App, ui: &mut Ui, body: Rect, width: Width) {
    let ctx = ui.ctx().clone();
    // Where each row starts, so only the rows in view are drawn.
    let mut offsets = Vec::with_capacity(app.items.len() + 1);
    let mut y = 4.0;
    for index in 0..app.items.len() {
        offsets.push(y);
        y += row_height(app, index, width);
    }
    offsets.push(y);
    let total = y + 4.0;
    let list_id = Id::new(("list", app.page.name()));
    // Keyboard.
    let list_focus = ui.interact(
        body,
        list_id.with("focus"),
        Sense::focusable_noninteractive(),
    );
    if app.ui.focus_list {
        app.ui.focus_list = false;
        list_focus.request_focus();
        if app.selected.is_none() && !app.items.is_empty() {
            app.choose(0, false);
        }
    }
    let focused = list_focus.has_focus();
    app.ui.list_focused = focused;
    if focused && !app.items.is_empty() {
        let current = app.selected_index();
        let mut target: Option<usize> = None;
        let count = app.items.len();
        ctx.input(|i| {
            let at = current.unwrap_or(0);
            if i.key_pressed(Key::ArrowDown) {
                target = Some(current.map_or(0, |c| (c + 1).min(count - 1)));
            }
            if i.key_pressed(Key::ArrowUp) {
                target = Some(at.saturating_sub(1));
            }
            if i.key_pressed(Key::PageDown) {
                target = Some((at + 10).min(count - 1));
            }
            if i.key_pressed(Key::PageUp) {
                target = Some(at.saturating_sub(10));
            }
            if i.key_pressed(Key::Home) {
                target = Some(0);
            }
            if i.key_pressed(Key::End) {
                target = Some(count - 1);
            }
        });
        if let Some(target) = target {
            app.ui.keyboard_nav = true;
            app.ui.reveal_selection = true;
            app.choose(target, false);
        }
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            if let Some(index) = app.selected_index() {
                app.choose(index, true);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Enter) && i.modifiers.is_none()) {
            if let Some(index) = app.selected_index() {
                app.run_row_action(index);
            }
        }
    }
    let reveal = std::mem::take(&mut app.ui.reveal_selection)
        .then(|| app.selected_index())
        .flatten();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(list_id)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded);
    if let Some(index) = reveal {
        let state =
            egui::scroll_area::State::load(&ctx, ui.make_persistent_id(list_id).with("scroll"));
        let current = state.map_or(0.0, |s| s.offset.y);
        let (row_top, row_bottom) = (offsets[index], offsets[index + 1]);
        let view = body.height();
        if row_top < current {
            scroll = scroll.vertical_scroll_offset(row_top - 4.0);
        } else if row_bottom > current + view {
            scroll = scroll.vertical_scroll_offset(row_bottom - view + 4.0);
        }
    }
    let mut hovered_raw = None;
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(body)
            .id_salt(list_id.with("body")),
        |ui| {
            scroll.show_viewport(ui, |ui, viewport| {
                ui.set_height(total);
                let origin = ui.min_rect().top();
                let first = offsets
                    .partition_point(|&o| o < viewport.top())
                    .saturating_sub(1);
                let last = offsets
                    .partition_point(|&o| o < viewport.bottom())
                    .min(app.items.len());
                selection_highlight(app, ui, list_id, &offsets, origin);
                let since = shown_since(app, &ctx, list_id);
                for index in first..last {
                    let rect = Rect::from_min_max(
                        pos2(ui.min_rect().left(), origin + offsets[index]),
                        pos2(ui.max_rect().right(), origin + offsets[index + 1]),
                    );
                    // Rows arrive one after another, rising into place.
                    let delay = (index - first).min(12) as f32 * 0.025;
                    let appear = progress_since(&ctx, since - delay, 0.26);
                    let hovered = if appear < 1.0 {
                        let mut child = ui.new_child(
                            UiBuilder::new()
                                .max_rect(rect.translate(vec2(0.0, (1.0 - appear) * 10.0))),
                        );
                        child.multiply_opacity(appear);
                        let shifted = child.max_rect();
                        row(app, &mut child, index, shifted, width)
                    } else {
                        row(app, ui, index, rect, width)
                    };
                    if let Some(raw) = hovered {
                        hovered_raw = Some(raw);
                    }
                }
            });
        },
    );
    app.hovered_row(hovered_raw);
}

/// Seconds since this list last went from empty to showing rows. A reload
/// that keeps rows on screen doesn't count, so only new lists animate.
fn shown_since(app: &App, ctx: &egui::Context, list_id: Id) -> f32 {
    let key = list_id.with("shown-at");
    let now = ctx.input(|i| i.time);
    if app.items.is_empty() {
        ctx.data_mut(|d| d.remove::<f64>(key));
        return 0.0;
    }
    let at = ctx.data_mut(|d| *d.get_temp_mut_or(key, now));
    (now - at) as f32
}

/// The selected row's background, sliding from row to row as the selection
/// moves. A new selection starts in place instead of sliding in from the
/// last one.
fn selection_highlight(app: &App, ui: &Ui, list_id: Id, offsets: &[f32], origin: f32) {
    let ctx = ui.ctx();
    let palette = Palette::current(ctx);
    let session_key = list_id.with("selection-session");
    let selected = app.selected_index().filter(|&i| i + 1 < offsets.len());
    let mut session = ctx
        .data(|d| d.get_temp::<(u64, bool)>(session_key))
        .unwrap_or((0, false));
    if selected.is_some() && !session.1 {
        session.0 += 1;
    }
    session.1 = selected.is_some();
    ctx.data_mut(|d| d.insert_temp(session_key, session));
    let shown = ease(
        ctx,
        list_id.with("selection-shown"),
        selected.is_some(),
        REVEAL,
    );
    let Some(index) = selected.or_else(|| {
        ctx.data(|d| d.get_temp::<usize>(list_id.with("selection-last")))
            .filter(|&i| i + 1 < offsets.len())
    }) else {
        return;
    };
    ctx.data_mut(|d| d.insert_temp(list_id.with("selection-last"), index));
    if shown <= 0.0 {
        return;
    }
    let heading = if app
        .items
        .get(index)
        .is_some_and(|item| item.group.is_some())
    {
        GROUP_HEADING
    } else {
        0.0
    };
    let id = list_id.with(("selection", session.0));
    let top = glide(ctx, id.with("top"), offsets[index] + heading, LAYOUT);
    let bottom = glide(ctx, id.with("bottom"), offsets[index + 1], LAYOUT);
    let rect = Rect::from_min_max(
        pos2(ui.min_rect().left(), origin + top),
        pos2(ui.max_rect().right(), origin + bottom),
    )
    .shrink2(vec2(6.0, 2.0));
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(10),
        alpha(palette.selection, shown),
    );
    // The keyboard's outline moves with it.
    if selected.is_some() && app.ui.keyboard_nav && app.ui.list_focused {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.5, alpha(palette.accent, shown)),
            StrokeKind::Inside,
        );
    }
}

/// What a source row adds to its status: why it can't be used, or that
/// it's turned off.
fn source_note(app: &App, row: &model::Row) -> String {
    if !row.available.unwrap_or(false) {
        return row
            .summary
            .strip_prefix("Unavailable: ")
            .unwrap_or(&row.summary)
            .to_owned();
    }
    if app.enabled_sources().contains(&row.source) {
        String::new()
    } else {
        "Turned off".into()
    }
}

/// Draws one row; returns its raw index when the pointer rests on it.
fn row(app: &mut App, ui: &mut Ui, index: usize, rect: Rect, width: Width) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let palette = Palette::current(&ctx);
    // Rows can be replaced earlier in this frame (a key that reloads the
    // page); the items catch up on the next one.
    let item = app.items.get(index)?.clone();
    let Some(row) = app.shown_rows().get(item.raw).cloned() else {
        ui.ctx().request_repaint();
        return None;
    };
    let identity = row.identity();
    let retaining = app.retained.is_some();
    let mut rect = rect;
    if let Some(group) = &item.group {
        let heading = Rect::from_min_size(
            rect.min + vec2(8.0, 4.0),
            vec2(rect.width() - 16.0, GROUP_HEADING - 6.0),
        );
        ui.painter()
            .rect_filled(heading, CornerRadius::same(8), alpha(palette.accent, 0.08));
        ui.painter().rect_filled(
            Rect::from_min_size(
                heading.min + vec2(0.0, 6.0),
                vec2(3.0, heading.height() - 12.0),
            ),
            CornerRadius::same(2),
            palette.accent,
        );
        ui.painter().text(
            heading.left_center() + vec2(14.0, 0.0),
            Align2::LEFT_CENTER,
            &group.title,
            theme::bold(13.5),
            palette.ink,
        );
        ui.painter().text(
            heading.right_center() - vec2(12.0, 0.0),
            Align2::RIGHT_CENTER,
            group.sources.join(", "),
            theme::font(12.0),
            palette.accent,
        );
        rect.min.y += GROUP_HEADING;
    }
    let id = Id::new(("row", &identity));
    let response = ui.interact(rect.shrink2(vec2(6.0, 2.0)), id, Sense::click());
    let selected = app.selected.as_deref() == Some(identity.as_str());
    let hover = ease(
        &ctx,
        id.with("hover"),
        response.hovered() && !retaining,
        FEEDBACK,
    );
    let chosen = ease(&ctx, id.with("selected"), selected, REVEAL);
    let surface = rect.shrink2(vec2(6.0, 2.0));
    let flash = app.flashes.get(&identity).map(|at| {
        1.0 - (at.elapsed().as_secs_f32() / crate::app::FLASH.as_secs_f32()).clamp(0.0, 1.0)
    });
    // The selected row's own background slides in `selection_highlight`.
    let fill = alpha(palette.ink, hover * 0.045 * (1.0 - chosen));
    let mut fill = fill;
    if let Some(flash) = flash {
        fill = mix(fill, alpha(palette.success, 0.2), flash);
        ctx.request_repaint();
    }
    ui.painter()
        .rect_filled(surface, CornerRadius::same(10), fill);
    let action = model::row_action(&row, app.page, &app.catalog).filter(|_| row.kind != "source");
    let running = app.writing
        && (app.active_rows.contains(&identity)
            || app.progress.for_row(&identity, &row.source).is_some());
    let pull = matches!(row.source.as_str(), "docker" | "podman")
        && row.is_installed()
        && row.reference.is_some()
        && app.page != Page::Updates;
    let page_open_here = selected && app.page_open;
    let action_count =
        usize::from(action.is_some() && !page_open_here) + usize::from(pull && !running);
    let cols = columns(rect, width, app.page, action_count as f32 * 38.0);
    let opacity = if retaining { 0.55 } else { 1.0 };
    let painter = ui.painter().clone();
    let compact = width == Width::Compact;

    // Check box on Updates.
    if let Some(tick) = cols.tick {
        let checked = !app.unchecked.contains(&identity);
        let tick_rect = Rect::from_center_size(tick.center(), Vec2::splat(22.0));
        let response = ui
            .interact(tick_rect, id.with("tick"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        paint_tick(ui, tick_rect, id, checked, true, response.hovered());
        if response.clicked() {
            if checked {
                app.unchecked.insert(identity.clone());
            } else {
                app.unchecked.remove(&identity);
            }
        }
    }
    // Icon.
    if let Some(icon) = cols.icon {
        let size = icon.width();
        let icon_rect =
            Rect::from_center_size(pos2(icon.center().x, rect.center().y), Vec2::splat(size));
        package_icon(app, ui, icon_rect, &row);
    }
    // Name and source.
    let title = row.title().to_owned();
    let source = row.kind == "source";
    let line = if source {
        // The status column already says whether it's available.
        source_note(app, &row)
    } else {
        model::source_line(&row, item.variants.len() > 1)
    };
    let top_y = if line.is_empty() {
        rect.center().y
    } else if compact && !source {
        rect.top() + 14.0
    } else {
        rect.center().y - 10.0
    };
    let name = one_line(
        ui,
        &title,
        theme::bold(14.5),
        palette.ink,
        cols.name.width(),
    );
    painter.galley(
        pos2(cols.name.left(), top_y - name.size().y / 2.0),
        name,
        alpha(palette.ink, opacity),
    );
    let sub = one_line(
        ui,
        &line,
        theme::font(12.5),
        palette.muted,
        cols.name.width(),
    );
    painter.galley(
        pos2(cols.name.left(), top_y + 10.0),
        sub,
        alpha(palette.muted, opacity),
    );
    let version = model::version_text(&row);
    let version_color = match row.kind.as_str() {
        "failure" => palette.danger,
        "source" if !row.available.unwrap_or(false) => palette.muted,
        "source" => palette.success,
        _ if row.has_update() => palette.accent,
        _ => palette.muted,
    };
    if compact && !source {
        // Version in its own colour, then the summary in what's left.
        let width = cols.name.width();
        let version = one_line(ui, &version, theme::font(12.5), version_color, width * 0.5);
        let at = pos2(cols.name.left(), top_y + 30.0);
        let used = version.size().x;
        painter.galley(at, version, alpha(version_color, opacity));
        if !row.summary.is_empty() && width - used > 40.0 {
            let summary = one_line(
                ui,
                &row.summary,
                theme::font(12.5),
                palette.muted,
                width - used - 12.0,
            );
            painter.galley(
                at + vec2(used + 12.0, 0.0),
                summary,
                alpha(palette.muted, opacity),
            );
        }
    }
    if let Some(v) = cols.version {
        if app.page == Page::Updates && row.has_update() {
            let new = row.candidate.clone().unwrap_or_else(|| "Unknown".into());
            let galley = one_line(ui, &new, theme::mono(13.0), palette.accent, v.width());
            painter.galley(
                pos2(v.left(), rect.center().y - 10.0 - galley.size().y / 2.0),
                galley,
                alpha(palette.accent, opacity),
            );
            if let Some(old) = row.installed.as_deref().filter(|o| !o.is_empty()) {
                let galley = one_line(
                    ui,
                    &format!("from {old}"),
                    theme::font(12.0),
                    palette.muted,
                    v.width(),
                );
                painter.galley(
                    pos2(v.left(), rect.center().y + 2.0),
                    galley,
                    alpha(palette.muted, opacity),
                );
            }
        } else {
            let font = if row.kind == "package" {
                theme::mono(13.0)
            } else {
                theme::font(13.0)
            };
            let galley = one_line(ui, &version, font, version_color, v.width());
            painter.galley(
                pos2(v.left(), rect.center().y - galley.size().y / 2.0),
                galley,
                alpha(version_color, opacity),
            );
        }
    }
    if let Some(s) = cols.summary.filter(|_| !source) {
        let galley = one_line(
            ui,
            &row.summary,
            theme::font(13.0),
            palette.muted,
            s.width(),
        );
        painter.galley(
            pos2(s.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            alpha(palette.muted, opacity),
        );
    }
    // Buttons.
    let mut x = cols.actions.right();
    if let Some(action) = action.filter(|_| !page_open_here) {
        let button = Rect::from_center_size(pos2(x - 17.0, rect.center().y), Vec2::splat(34.0));
        x -= 38.0;
        let enabled = running || (app.can_act() && !retaining);
        // Coloured only on the row in hand, so a long list isn't a wall of
        // red bins. The pointer may be on the button itself.
        let near = ease(
            &ctx,
            id.with("near"),
            ui.rect_contains_pointer(surface) && !retaining,
            FEEDBACK,
        )
        .max(chosen);
        let color = if !enabled || running {
            palette.muted
        } else {
            mix(palette.muted, tone(&palette, action.tone()), near)
        };
        let icon = if running { "cancel" } else { action.icon() };
        let tip = if running {
            let status = app.ctl.status().to_string();
            format!(
                "Cancel {}",
                if status.is_empty() {
                    "the change".into()
                } else {
                    status
                }
            )
        } else {
            match action {
                model::Action::Clean => format!("Run cleanup {title}"),
                model::Action::Adopt => format!("Manage {title} with Homebrew"),
                _ => format!("{} {title} from {line}", action.label()),
            }
        };
        let mut child = ui.new_child(UiBuilder::new().max_rect(button));
        if icon_button(&mut child, icon, &tip, color, enabled).clicked() {
            if running {
                app.cancel();
            } else {
                app.run_row_action(index);
            }
        }
    }
    if pull && !running {
        let button = Rect::from_center_size(pos2(x - 17.0, rect.center().y), Vec2::splat(34.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(button));
        let tip = format!("Pull {title} from {line}");
        if icon_button(
            &mut child,
            "updates",
            &tip,
            palette.accent,
            app.can_act() && !retaining,
        )
        .clicked()
        {
            app.review_on = crate::app::ReviewOn::Dialog;
            app.active_rows.insert(identity.clone());
            app.c().propose("upgrade".into(), item.raw as i32);
            app.react();
        }
    }
    // Progress along the bottom.
    if app.writing {
        if let Some(fraction) = app
            .progress
            .for_row(&identity, &row.source)
            .or_else(|| app.active_rows.contains(&identity).then_some(None))
        {
            let bar = Rect::from_min_max(
                pos2(surface.left() + 10.0, surface.bottom() - 4.0),
                pos2(surface.right() - 10.0, surface.bottom() - 1.0),
            );
            paint_bar(ui, bar, fraction, palette.accent, id.with("progress"));
        }
    }
    // Divider.
    painter.line_segment(
        [
            pos2(rect.left() + 18.0, rect.bottom()),
            pos2(rect.right() - 18.0, rect.bottom()),
        ],
        Stroke::new(1.0, alpha(palette.line, 0.55)),
    );
    let accessible = format!(
        "{}{title}, {line}, {}",
        if row.has_update() {
            "Update available. "
        } else if row.is_installed() {
            "Installed. "
        } else if row.is_package() {
            "Not installed. "
        } else {
            ""
        },
        row.summary
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            !retaining,
            selected,
            &accessible,
        )
    });
    if response.clicked() && !retaining {
        app.ui.keyboard_nav = false;
        app.ui.focus_list = true;
        if selected && app.page_open && row.is_package() {
            app.page_open = false;
        } else {
            app.choose(index, true);
        }
    }
    if response.double_clicked() && !retaining && row.is_package() {
        app.choose(index, true);
    }
    response.hovered().then_some(item.raw)
}

/// An app's icon, or its source's mark.
pub fn package_icon(app: &App, ui: &mut Ui, rect: Rect, row: &Row) {
    let palette = Palette::current(ui.ctx());
    let fallback = |ui: &mut Ui| {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(10), alpha(palette.accent, 0.1));
        let name = match row.kind.as_str() {
            "cleanup" => "remove",
            "failure" => "warning",
            _ => row.source.as_str(),
        };
        theme::paint_icon(
            ui.painter(),
            rect.shrink(rect.width() * 0.24),
            name,
            palette.accent,
        );
    };
    let uri = row
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map(|icon| {
            if icon.starts_with("https://") || icon.starts_with("file://") {
                icon.to_owned()
            } else {
                format!("file://{icon}")
            }
        });
    match uri.filter(|uri| !app.ui.failed_images.contains(uri)) {
        Some(uri) => {
            let image = egui::Image::new(uri.clone())
                .fit_to_exact_size(rect.size())
                .corner_radius(CornerRadius::same(8));
            match image.load_for_size(ui.ctx(), rect.size()) {
                Ok(egui::load::TexturePoll::Ready { .. }) => {
                    image.paint_at(ui, rect);
                    // The source, small, on the corner.
                    if row.kind == "package" && rect.width() >= 32.0 {
                        let badge = Rect::from_center_size(
                            rect.right_bottom() - vec2(3.0, 3.0),
                            Vec2::splat(18.0),
                        );
                        ui.painter().circle(
                            badge.center(),
                            9.5,
                            palette.surface,
                            Stroke::new(1.0, palette.line),
                        );
                        theme::paint_icon(
                            ui.painter(),
                            badge.shrink(3.5),
                            &row.source,
                            palette.muted,
                        );
                    }
                }
                Ok(egui::load::TexturePoll::Pending { .. }) => {
                    paint_skeleton(ui, rect, palette.hover_solid());
                }
                Err(_) => fallback(ui),
            }
        }
        None => fallback(ui),
    }
}

fn skeleton_rows(ui: &mut Ui, body: Rect, palette: &Palette) {
    let color = palette.hover_solid();
    for i in 0..8 {
        let top = body.top() + 8.0 + i as f32 * ROW;
        if top + ROW > body.bottom() {
            break;
        }
        let row = Rect::from_min_size(
            pos2(body.left() + 20.0, top),
            vec2(body.width() - 40.0, ROW),
        );
        paint_skeleton(
            ui,
            Rect::from_min_size(pos2(row.left(), row.center().y - 18.0), Vec2::splat(36.0)),
            color,
        );
        let width = row.width() - 60.0;
        let widths = [0.28, 0.18, 0.4];
        paint_skeleton(
            ui,
            Rect::from_min_size(
                pos2(row.left() + 50.0, row.center().y - 11.0),
                vec2(width * widths[i % 3] + 60.0, 10.0),
            ),
            color,
        );
        paint_skeleton(
            ui,
            Rect::from_min_size(
                pos2(row.left() + 50.0, row.center().y + 5.0),
                vec2(width * 0.16 + 30.0, 8.0),
            ),
            color,
        );
    }
}

fn empty_state(app: &mut App, ui: &mut Ui, body: Rect) {
    let palette = Palette::current(ui.ctx());
    let failures = app.read_failures();
    let phase = app.report.phase.clone();
    let phase = phase.as_str();
    let filtered = app.page_sources.contains_key(&app.page);
    let installed_filters =
        app.page == Page::Installed && (!app.filter.is_empty() || app.duplicates_only);
    let good_news;
    let message: String = {
        let up_to_date = |names: &[String]| match names {
            [one] => format!("{one} is up to date"),
            [a, b] => format!("{a} and {b} are up to date"),
            many => format!("{} other sources are up to date", many.len()),
        };
        let answered: Vec<String> = app
            .report
            .successful_sources
            .iter()
            .map(|id| model::source_name(id).to_owned())
            .collect();
        if app.writing && app.page == Page::Search {
            good_news = false;
            "Search will resume shortly.".into()
        } else if !failures.is_empty()
            && (answered.is_empty()
                || phase == "failed"
                || !matches!(app.page, Page::Updates | Page::Clean))
        {
            good_news = false;
            model::failure_title(&failures)
        } else if !failures.is_empty() {
            good_news = true;
            match app.page {
                Page::Updates => up_to_date(&answered),
                _ => format!("Nothing to clean in {}", answered.join(", ")),
            }
        } else if phase == "cancelled" {
            good_news = false;
            if app.page == Page::Search {
                "Search stopped"
            } else {
                "Loading stopped"
            }
            .into()
        } else if phase == "unsupported" {
            good_news = false;
            "None of your enabled sources support this page.".into()
        } else if installed_filters {
            good_news = false;
            "No packages match these filters.".into()
        } else if filtered && !app.rows.is_empty() {
            good_news = false;
            "No results from selected sources.".into()
        } else {
            good_news = matches!(app.page, Page::Updates | Page::Clean);
            match app.page {
                Page::Updates => "You're up to date",
                Page::Clean => "Nothing to clean",
                Page::Installed => "No installed packages.",
                Page::Sources => "No available sources.",
                Page::Search => "No matching packages.",
                Page::Settings => "No results to show.",
            }
            .into()
        }
    };
    let (icon, color) = if !failures.is_empty() && !good_news {
        ("warning", palette.warning)
    } else if good_news {
        ("installed", palette.success)
    } else if app.page == Page::Search {
        ("search", palette.muted)
    } else {
        ("package", palette.muted)
    };
    let center = pos2(
        body.center().x,
        body.top() + (body.height() * 0.42).max(80.0),
    );
    // A soft disc behind the icon.
    let pop = ui.ctx().animate_bool_with_time_and_easing(
        Id::new(("empty", &message)),
        true,
        secs(ui.ctx(), 0.3),
        egui::emath::easing::back_out,
    );
    ui.painter()
        .circle_filled(center - vec2(0.0, 46.0), 30.0 * pop, alpha(color, 0.12));
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(center - vec2(0.0, 46.0), Vec2::splat(30.0 * pop)),
        icon,
        color,
    );
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        &message,
        theme::bold(16.5),
        palette.ink,
    );
    let hint = if !failures.is_empty() {
        let retry = if app.page == Page::Updates {
            "Retry to check for updates."
        } else {
            "Retry to check again."
        };
        if good_news {
            format!("{}. {retry}", model::failure_title(&failures))
        } else {
            retry.to_owned()
        }
    } else if matches!(app.page, Page::Updates | Page::Clean) {
        app.report
            .checked_at
            .map(|at| model::checked_ago(at, model::now()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    if !hint.is_empty() {
        ui.painter().text(
            center + vec2(0.0, 26.0),
            Align2::CENTER_CENTER,
            hint,
            theme::font(13.5),
            if good_news && !failures.is_empty() {
                palette.warning
            } else {
                palette.muted
            },
        );
    }
    let buttons = Rect::from_center_size(
        center + vec2(0.0, 70.0),
        vec2(body.width() - 40.0, CONTROL_HEIGHT),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(buttons)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            let mut labels: Vec<(&str, &str)> = vec![];
            if !failures.is_empty() {
                labels.push((
                    if failures.len() == 1 {
                        "Retry"
                    } else {
                        "Reload"
                    },
                    "refresh",
                ));
                labels.push(("Details", "info"));
            } else if phase == "cancelled" {
                labels.push((
                    if app.page == Page::Search {
                        "Search again"
                    } else {
                        "Load again"
                    },
                    "refresh",
                ));
            } else if app.page != Page::Search {
                labels.push((
                    if matches!(app.page, Page::Updates | Page::Clean) {
                        "Check again"
                    } else {
                        "Reload"
                    },
                    "refresh",
                ));
                if filtered || installed_filters {
                    labels.push(("Clear filters", "cancel"));
                }
            }
            let total: f32 = labels.len() as f32 * 130.0;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            for (label, icon) in labels {
                let look = if icon == "refresh" {
                    Look::Soft(Tone::Accent)
                } else {
                    Look::Secondary
                };
                if Button::new(look, label)
                    .icon(icon)
                    .enabled(!app.busy || label == "Details")
                    .show(ui)
                    .clicked()
                {
                    match label {
                        "Retry" => {
                            let source = failures[0].source.clone();
                            app.retry_source(&source);
                        }
                        "Details" => app.ui.checks_open = true,
                        "Clear filters" => {
                            app.filter.clear();
                            app.duplicates_only = false;
                            if app.page_sources.remove(&app.page).is_some() {
                                app.reload(false, false);
                            }
                            app.invalidate();
                        }
                        _ => app.reload(true, false),
                    }
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::waiting_text;

    #[test]
    fn waiting_names_one_or_two_sources_and_counts_more() {
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert_eq!(waiting_text(&names(&["APT"])), "Waiting for APT");
        assert_eq!(
            waiting_text(&names(&["APT", "Snap"])),
            "Waiting for APT and Snap"
        );
        assert_eq!(
            waiting_text(&names(&["APT", "Snap", "npm"])),
            "Waiting for 3 sources"
        );
    }
}
