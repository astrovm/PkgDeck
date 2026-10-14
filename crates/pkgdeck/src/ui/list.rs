//! The results card: its heading, column titles, rows, and what it shows
//! while loading or when there's nothing to list.

use super::{skin, widgets::*, COMPACT_BELOW, MEDIUM_BELOW};
use crate::{
    app::App,
    model::{self, Column, Page, Row, Tone},
    theme::{self, Palette},
};
use eframe::egui::{
    self, emath::GuiRounding as _, pos2, vec2, Align2, CursorIcon, Id, Key, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2,
};

const ROW: f32 = 60.0;
/// A wide row's height at the chosen density.
fn wide_row() -> f32 {
    (ROW * theme::skin().density.scale()).round()
}
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
    // A list box sits into the window in the styles with bevels.
    let line = skin::panel_stroke(&palette).color;
    skin::paint_panel(
        ui.painter(),
        rect,
        &palette,
        true,
        Stroke::new(1.0, mix(line, palette.accent, ring)),
    );
    let width = if rect.width() < COMPACT_BELOW {
        Width::Compact
    } else if rect.width() < MEDIUM_BELOW {
        Width::Medium
    } else {
        Width::Wide
    };
    let mut top = rect.top();
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
    // While writing, the card says what runs; the count stays.
    if app.busy && !app.writing {
        return match app.page {
            Page::Search => "Searching",
            _ if count > 0 => "Refreshing",
            _ => "Loading",
        }
        .into();
    }
    if app.page == Page::Search {
        if let Some(matches) = app.report.matches.filter(|m| *m > count as u64) {
            return format!("Best {count} of {matches} packages. Type more to narrow.");
        }
    }
    let noun = match app.page {
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

/// Whether the page has a list to talk about: its count, status and
/// reload sit beside the title.
pub fn list_shown(app: &App) -> bool {
    app.page != Page::Settings
        && app.opened.is_none()
        && (!app.items.is_empty() || app.busy || app.writing)
}

/// The line under the page title: how many, or what the page is doing,
/// and which sources it still waits for (named on hover past two).
pub fn subtitle(app: &App) -> (String, Option<String>) {
    if !list_shown(app) {
        return (String::new(), None);
    }
    let mut names = Vec::new();
    if app.waiting_shown() {
        names = app
            .pending
            .iter()
            .filter(|id| {
                let kind = model::catalog_entry(&app.catalog, id).availability_kind;
                kind != "unavailable" && kind != "platform"
            })
            .map(|id| model::source_name(id).to_owned())
            .collect();
    }
    let line = status_line(heading_text(app), app.page == Page::Search, &names);
    let hover = (names.len() > 2).then(|| names.join("\n"));
    (line, hover)
}

/// While waiting on sources, "Waiting for" already says the list is
/// loading, so it takes the place of "Refreshing" or "Loading".
/// "Searching" stays, since the search has its own job to name.
fn status_line(heading: String, searching: bool, names: &[String]) -> String {
    if names.is_empty() {
        heading
    } else if searching {
        format!("{heading} · {}", waiting_text(names))
    } else {
        waiting_text(names)
    }
}

struct Columns {
    /// Where the name starts and how wide the name, version and summary
    /// may be together, for the dividers in the headings.
    start: f32,
    available: f32,
    tick: Option<Rect>,
    icon: Option<Rect>,
    name: Rect,
    version: Option<Rect>,
    summary: Option<Rect>,
    actions: Rect,
}

/// Room kept for a row's buttons. Every row and the headings share it, so
/// the columns line up whatever buttons a row has.
const ACTIONS_ROOM: f32 = 76.0;

/// Shares of the row the name and version take, 0 for the default.
#[derive(Clone, Copy, Default)]
struct Shares {
    name: f32,
    version: f32,
}

impl Shares {
    fn of(app: &App) -> Self {
        Self {
            name: app.settings.name_column,
            version: app.settings.version_column,
        }
    }
}

fn columns(row: Rect, width: Width, page: Page, actions: f32, shares: Shares) -> Columns {
    let mut x = row.left() + 14.0;
    let tick = matches!(page, Page::Updates | Page::Clean).then(|| {
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
    let right = row.right() - ACTIONS_ROOM.max(actions) - 20.0;
    let available = (right - x).max(60.0);
    let start = x;
    let share = |dragged: f32, default: f32| if dragged > 0.0 { dragged } else { default };
    // Sizes are short, and task names long.
    let (name_share, version_share, medium_version, least) = if page == Page::Clean {
        // Room for the size and "Not in Clean all" under it.
        (0.42, 0.18, 0.26, 104.0)
    } else {
        (0.36, 0.2, 0.32, 80.0)
    };
    let (name_w, version_w, summary) = match width {
        Width::Wide => {
            let name = (available * share(shares.name, name_share))
                .clamp(120.0, (available - 200.0).max(120.0));
            let version = (available * share(shares.version, version_share))
                .clamp(least, (available - name - 64.0).max(least));
            (name, version, true)
        }
        Width::Medium => {
            let version = (available * share(shares.version, medium_version))
                .clamp(least, (available * 0.6).max(least));
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
    let summary = (summary && right - x > 40.0)
        .then(|| Rect::from_min_max(pos2(x, row.top()), pos2(right, row.bottom())));
    Columns {
        start,
        available,
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
    let cols = columns(rect, width, app.page, ACTIONS_ROOM, Shares::of(app));
    let version_title = match app.page {
        Page::Clean => "SIZE",
        _ => "VERSION",
    };
    let mut titles = vec![(Column::Name, "NAME", cols.name)];
    if let Some(v) = cols.version {
        titles.push((Column::Version, version_title, v));
    }
    if let Some(s) = cols.summary {
        titles.push((Column::Summary, "SUMMARY", s));
    }
    if let Some(tick) = cols.tick {
        let rows = app.shown_rows();
        let selectable: Vec<_> = app
            .items
            .iter()
            .filter_map(|i| rows.get(i.raw))
            .filter(|row| app.page != Page::Clean || row.batch_cleanable())
            .collect();
        let all_checked =
            !selectable.is_empty() && selectable.iter().all(|row| app.row_checked(row));
        let none_checked = selectable.iter().all(|row| !app.row_checked(row));
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
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::selected(
                egui::WidgetType::Checkbox,
                !selectable.is_empty(),
                all_checked,
                "Select all tasks",
            );
            info.selected = if !all_checked && !none_checked {
                None
            } else {
                Some(all_checked)
            };
            info
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
            app.set_all_checked(!all_checked);
        }
    }
    // The older styles draw each heading as a cell of a header bar.
    let band = skin::paint_column_band(ui.painter(), rect.shrink2(vec2(2.0, 0.0)), &palette);
    let starts: Vec<f32> = titles.iter().map(|(_, _, area)| area.left()).collect();
    for (index, (column, title, area)) in titles.into_iter().enumerate() {
        let response = ui
            .interact(
                Rect::from_min_max(
                    pos2(area.left() - 4.0, rect.top()),
                    pos2(area.right(), rect.bottom()),
                )
                .round_ui(),
                Id::new(("column", title)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let sorted = app.sort.filter(|(c, _)| *c == column);
        let mut color = if sorted.is_some() || response.hovered() || band {
            palette.ink
        } else {
            palette.muted
        };
        if band {
            let left = if index == 0 {
                rect.left() + 2.0
            } else {
                area.left() - 8.0
            };
            let right = starts
                .get(index + 1)
                .map_or(rect.right() - 2.0, |next| next - 8.0);
            let cell = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
            if let Some(ink) = skin::paint_column_cell(
                ui.painter(),
                cell,
                &palette,
                sorted.is_some(),
                response.hovered(),
            ) {
                color = ink;
            }
        }
        let title = skin::column_title(title);
        let text = match sorted {
            Some((_, true)) => format!("{title}  ▲"),
            Some((_, false)) => format!("{title}  ▼"),
            None => title,
        };
        ui.painter().text(
            pos2(area.left(), rect.center().y),
            Align2::LEFT_CENTER,
            text,
            if band {
                theme::font(13.0)
            } else {
                theme::bold(11.0)
            },
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
    column_dividers(app, ui, rect, &cols);
    if !band {
        ui.painter().line_segment(
            [
                pos2(rect.left() + 12.0, rect.bottom()),
                pos2(rect.right() - 12.0, rect.bottom()),
            ],
            Stroke::new(1.0, alpha(palette.line, 0.7)),
        );
    }
}

/// Drag the line after a column heading to resize it; double-click it for
/// the default width.
fn column_dividers(app: &mut App, ui: &mut Ui, rect: Rect, cols: &Columns) {
    let palette = Palette::current(ui.ctx());
    let mut dividers = Vec::new();
    if let Some(version) = cols.version {
        dividers.push(("name", version.left() - 6.0));
        if cols.summary.is_some() {
            dividers.push(("version", version.right() + 6.0));
        }
    }
    for (column, x) in dividers {
        // Snap to the UI grid: egui's debug overlay flags off-grid edges as "Unaligned".
        let handle =
            Rect::from_center_size(pos2(x, rect.center().y), vec2(10.0, rect.height())).round_ui();
        let response = ui
            .interact(
                handle,
                Id::new(("column-divider", column)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        response.widget_info(|| {
            let label = if column == "name" {
                "Resize Name column"
            } else {
                "Resize Version column"
            };
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, label)
        });
        let active = response.hovered() || response.dragged();
        ui.painter().line_segment(
            [pos2(x, rect.top() + 9.0), pos2(x, rect.bottom() - 9.0)],
            Stroke::new(
                if active { 2.0 } else { 1.0 },
                if active {
                    palette.accent
                } else {
                    alpha(palette.line, 0.9)
                },
            ),
        );
        if response.dragged() {
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                let share = dragged_share(column, pointer.x, cols);
                match column {
                    "name" => app.settings.name_column = share,
                    _ => app.settings.version_column = share,
                }
            }
        }
        if response.drag_stopped() {
            app.save_settings();
        }
        if response.double_clicked() {
            match column {
                "name" => app.settings.name_column = 0.0,
                _ => app.settings.version_column = 0.0,
            }
            app.save_settings();
        }
    }
}

/// The share of the row a column takes when its divider is at `x`: the
/// divider sits 6 past the column's right edge.
fn dragged_share(column: &str, x: f32, cols: &Columns) -> f32 {
    let left = match column {
        "name" => cols.start,
        _ => cols.version.map_or(cols.start, |v| v.left()),
    };
    ((x - 6.0 - left) / cols.available).clamp(0.05, 0.9)
}

fn row_height(app: &App, index: usize, width: Width) -> f32 {
    // Compact rows add a version and summary line.
    let base = if width == Width::Compact {
        ROW_COMPACT
    } else {
        wide_row()
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
        // Focus alone selects nothing: on Clean and Sources a selected row
        // opens its panel, so a new page would open its first row.
        list_focus.request_focus();
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
    let mut scroll = super::scrollbar::vertical()
        .id_salt(list_id)
        .auto_shrink([false, false]);
    if let Some(index) = reveal {
        let state =
            egui::scroll_area::State::load(&ctx, ui.make_persistent_id(list_id).with("scroll"));
        let current = state.map_or(0.0, |s| s.offset.y);
        let (row_top, row_bottom) = (offsets[index], offsets[index + 1]);
        let view = body.height();
        if row_top < current {
            scroll = scroll.scroll_offset(row_top - 4.0);
        } else if row_bottom > current + view {
            scroll = scroll.scroll_offset(row_bottom - view + 4.0);
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
                    let delay = (index - first).min(10) as f32 * 0.014;
                    let appear = progress_since(&ctx, since - delay, 0.2);
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

/// What a task that Clean all skips says under its size.
const LEFT_OUT: &str = "Not in Clean all";

fn left_out_of_clean_all(row: &Row) -> bool {
    row.kind == "cleanup" && row.cleanup_kind == "data"
}

/// Seconds since rows first showed after the page was opened. Reloads,
/// searches and filters that empty the list for a moment don't count, so
/// rows only rise into place when the person arrives.
fn shown_since(app: &App, ctx: &egui::Context, list_id: Id) -> f32 {
    let key = list_id.with("shown-at");
    let now = ctx.input(|i| i.time);
    let opened = app.page_changed;
    let (seen, at) = ctx
        .data(|d| d.get_temp::<(std::time::Instant, Option<f64>)>(key))
        .filter(|(seen, _)| *seen == opened)
        .unwrap_or((opened, None));
    let at = at.or((!app.items.is_empty()).then_some(now));
    ctx.data_mut(|d| d.insert_temp(key, (seen, at)));
    at.map_or(0.0, |at| (now - at) as f32)
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
    skin::paint_row_selection(ui.painter(), rect, &palette, shown);
    // The keyboard's outline moves with it.
    if selected.is_some() && app.ui.keyboard_nav && app.ui.list_focused {
        ui.painter().rect_stroke(
            rect,
            theme::round(10),
            Stroke::new(1.5, alpha(palette.accent, shown)),
            StrokeKind::Inside,
        );
    }
}

/// What a source row adds to its status: why it can't be used, or that
/// it's turned off.
/// The warning or lock beside an update that failed last time or needs
/// the password, which says why when hovered.
fn held_mark(ui: &mut Ui, held: &model::Held, rect: Rect, row: Id) {
    let palette = Palette::current(ui.ctx());
    theme::paint_icon(ui.painter(), rect, held.icon(), palette.warning);
    ui.interact(rect, row.with("held"), Sense::hover())
        .on_hover_text(held.tip());
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
    // Why the last change failed here: it shows in place of the summary.
    let failed = app.row_failure(&row).map(str::to_owned);
    // An update that failed last time or needs the password.
    let held = (app.page == Page::Updates && row.has_update())
        .then(|| app.held.iter().find(|held| held.matches(&row)).cloned())
        .flatten();
    let mut rect = rect;
    if let Some(group) = &item.group {
        let heading = Rect::from_min_size(
            rect.min + vec2(8.0, 4.0),
            vec2(rect.width() - 16.0, GROUP_HEADING - 6.0),
        );
        ui.painter()
            .rect_filled(heading, theme::round(8), alpha(palette.accent, 0.08));
        ui.painter().rect_filled(
            Rect::from_min_size(
                heading.min + vec2(0.0, 6.0),
                vec2(3.0, heading.height() - 12.0),
            ),
            theme::round(2),
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
    // On a solid selection, everything in the row takes its ink.
    let palette = match skin::row_selection_ink(&palette) {
        Some(ink) if selected => skin::on_fill(&palette, ink),
        _ => palette,
    };
    let surface = rect.shrink2(vec2(6.0, 2.0));
    let flash = app.flashes.get(&identity).map(|at| {
        1.0 - (at.elapsed().as_secs_f32() / crate::app::FLASH.as_secs_f32()).clamp(0.0, 1.0)
    });
    if let Some(stripe) = skin::row_stripe(&palette, index).filter(|_| !selected) {
        ui.painter()
            .rect_filled(rect.shrink2(vec2(1.0, 0.0)), 0.0, stripe);
    }
    // The selected row's own background slides in `selection_highlight`.
    skin::paint_row_hover(ui.painter(), surface, &palette, hover * (1.0 - chosen));
    if let Some(flash) = flash {
        ui.painter().rect_filled(
            surface,
            theme::round(10),
            alpha(palette.success, 0.2 * flash),
        );
        ctx.request_repaint();
    }
    let action = model::row_action(&row, app.page, &app.catalog);
    let running = app.writing
        && (app.active_rows.contains(&identity)
            || app.progress.for_row(&identity, &row.source).is_some());
    let batch_state = if app.writing && app.progress.total > 1 {
        app.progress.for_row(&identity, &row.source)
    } else {
        None
    };
    let pull = matches!(row.source.as_str(), "docker" | "podman")
        && row.is_installed()
        && row.reference.is_some()
        && app.page != Page::Updates;
    let page_open_here = selected && app.page_open;
    let action_count =
        usize::from(action.is_some() && !page_open_here) + usize::from(pull && !running);
    let cols = columns(
        rect,
        width,
        app.page,
        action_count as f32 * 38.0,
        Shares::of(app),
    );
    // Text laid out with its colour ignores a colour given when painting,
    // so old results dim through the painter.
    let mut painter = ui.painter().clone();
    if retaining {
        painter.multiply_opacity(0.55);
    }
    let compact = width == Width::Compact;

    // Checkboxes on Updates and Clean. Data tasks are cleaned individually.
    if let Some(tick) = cols.tick {
        let enabled = app.page != Page::Clean || row.batch_cleanable();
        let checked = enabled && app.row_checked(&row);
        let tick_rect = Rect::from_center_size(tick.center(), Vec2::splat(22.0));
        let response = ui
            .interact(tick_rect, id.with("tick"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Checkbox,
                enabled,
                checked,
                format!("Select {}", row.title()),
            )
        });
        paint_tick(ui, tick_rect, id, checked, enabled, response.hovered());
        if enabled && response.clicked() {
            app.set_row_checked(&row, !checked);
        }
    }
    // Icon.
    if let Some(icon) = cols.icon {
        let size = icon.width();
        let icon_rect =
            Rect::from_center_size(pos2(icon.center().x, rect.center().y), Vec2::splat(size));
        package_icon(app, ui, icon_rect, &row);
        if let Some(state) = batch_state {
            paint_batch_state(ui, icon_rect, state, &palette);
        }
    }
    // Name and source.
    let title = row.title().to_owned();
    let line = model::source_line(&row, item.variants.len() > 1);
    let top_y = if line.is_empty() {
        rect.center().y
    } else if compact {
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
        palette.ink,
    );
    let sub = one_line(
        ui,
        &line,
        theme::font(12.5),
        palette.muted,
        cols.name.width(),
    );
    painter.galley(pos2(cols.name.left(), top_y + 10.0), sub, palette.muted);
    let version = model::version_text(&row);
    let version_color = match row.kind.as_str() {
        "failure" => palette.danger,
        _ if row.has_update() => palette.accent,
        _ => palette.muted,
    };
    if compact {
        // Version in its own colour, then the summary in what's left. Every
        // task here is a cache unless it says otherwise.
        let version = if row.kind == "cleanup" && version == model::cleanup_type("") {
            String::new()
        } else if left_out_of_clean_all(&row) {
            format!("{version}, {LEFT_OUT}")
        } else {
            version.clone()
        };
        let width = cols.name.width();
        let version = one_line(ui, &version, theme::font(12.5), version_color, width * 0.5);
        let mut at = pos2(cols.name.left(), top_y + 30.0);
        if let Some(held) = &held {
            held_mark(ui, held, Rect::from_min_size(at, Vec2::splat(15.0)), id);
            at.x += 20.0;
        }
        // No version, no gap before the summary.
        let used = version.size().x;
        let gap = if used > 0.0 { 12.0 } else { 0.0 };
        painter.galley(at, version, version_color);
        if let Some(reason) = failed.as_deref().filter(|_| width - used > 40.0) {
            let place =
                Rect::from_min_size(at + vec2(used + gap, 0.0), vec2(width - used - gap, 16.0));
            failed_summary(ui, &painter, place, reason, id);
        } else if !row.summary.is_empty() && width - used > 40.0 {
            let summary = one_line(
                ui,
                &row.summary,
                theme::font(12.5),
                palette.muted,
                width - used - gap,
            );
            painter.galley(at + vec2(used + gap, 0.0), summary, palette.muted);
        }
    }
    if let Some(v) = cols.version {
        // An update reads the same on every page: the new version, and
        // what it replaces under it.
        if row.has_update() {
            let new = row.candidate.clone().unwrap_or_else(|| "Unknown".into());
            let mut left = v.left();
            if let Some(held) = &held {
                let mark = Rect::from_center_size(
                    pos2(left + 8.0, rect.center().y - 10.0),
                    Vec2::splat(16.0),
                );
                held_mark(ui, held, mark, id);
                left += 22.0;
            }
            let galley = one_line(ui, &new, theme::mono(13.0), palette.accent, v.width());
            painter.galley(
                pos2(left, rect.center().y - 10.0 - galley.size().y / 2.0),
                galley,
                palette.accent,
            );
            if let Some(old) = row.installed.as_deref().filter(|o| !o.is_empty()) {
                let galley = one_line(
                    ui,
                    &format!("from {old}"),
                    theme::font(12.0),
                    palette.muted,
                    v.width(),
                );
                painter.galley(pos2(left, rect.center().y + 2.0), galley, palette.muted);
            }
        } else {
            let font = if row.kind == "package" {
                theme::mono(13.0)
            } else {
                theme::font(13.0)
            };
            let galley = one_line(ui, &version, font, version_color, v.width());
            if left_out_of_clean_all(&row) {
                painter.galley(
                    pos2(v.left(), rect.center().y - 10.0 - galley.size().y / 2.0),
                    galley,
                    version_color,
                );
                let tag = one_line(ui, LEFT_OUT, theme::font(12.0), palette.warning, v.width());
                painter.galley(pos2(v.left(), rect.center().y + 2.0), tag, palette.warning);
            } else {
                painter.galley(
                    pos2(v.left(), rect.center().y - galley.size().y / 2.0),
                    galley,
                    version_color,
                );
            }
        }
    }
    if let Some(s) = cols.summary {
        if let Some(reason) = &failed {
            let place =
                Rect::from_min_size(pos2(s.left(), rect.center().y - 9.0), vec2(s.width(), 18.0));
            failed_summary(ui, &painter, place, reason, id);
        } else {
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
                palette.muted,
            );
        }
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
            // Faint until then too, so the names and versions lead.
            mix(
                alpha(palette.muted, 0.45),
                tone(&palette, action.tone()),
                near,
            )
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
        if batch_state.is_some() {
            // A batch is cancelled from its one card, not from every row.
            // Its marks sit over the icons.
        } else {
            let mut child = ui.new_child(UiBuilder::new().max_rect(button));
            if icon_button(&mut child, icon, &tip, color, enabled).clicked() {
                if running {
                    app.cancel();
                } else {
                    app.run_row_action(index);
                }
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
    // Divider.
    painter.line_segment(
        [
            pos2(rect.left() + 18.0, rect.bottom()),
            pos2(rect.right() - 18.0, rect.bottom()),
        ],
        Stroke::new(1.0, alpha(palette.line, 0.55)),
    );
    let accessible = format!(
        "{}{}{title}, {line}, {}",
        held.as_ref()
            .map_or(String::new(), |held| format!("{} ", held.tip())),
        if row.has_update() {
            "Update available. "
        } else if row.is_installed() {
            "Installed. "
        } else if row.is_package() {
            "Not installed. "
        } else {
            ""
        },
        // Screen readers hear why it failed, as the row shows it.
        failed
            .as_deref()
            .map_or_else(|| row.summary.clone(), |reason| format!("Failed: {reason}"))
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

/// Where a row stands in a batch: waiting, working, or done, over its icon.
/// One quiet mark per row, so the card's single bar is all that moves.
fn paint_batch_state(ui: &Ui, icon: Rect, state: model::RowState, palette: &Palette) {
    ui.painter().rect_filled(
        icon,
        theme::round((icon.width() * 0.27) as u8),
        palette.surface,
    );
    let mark = Rect::from_center_size(icon.center(), Vec2::splat(icon.width() * 0.6));
    match state {
        model::RowState::Queued => {
            theme::paint_icon(ui.painter(), mark, "clock", alpha(palette.muted, 0.8))
        }
        model::RowState::Running => paint_spinner(ui, mark, palette.accent),
        model::RowState::Done => {
            theme::paint_icon(ui.painter(), mark, "installed", palette.success)
        }
    }
}

/// An app's icon, or its source's mark.
pub fn package_icon(app: &App, ui: &mut Ui, rect: Rect, row: &Row) {
    let palette = Palette::current(ui.ctx());
    let fallback = |ui: &mut Ui| {
        let mark = if skin::bare_icons() {
            rect.shrink(rect.width() * 0.12)
        } else {
            ui.painter()
                .rect_filled(rect, theme::round(10), alpha(palette.accent, 0.1));
            rect.shrink(rect.width() * 0.24)
        };
        if row.kind == "failure" {
            theme::paint_colour_icon(ui.painter(), mark, "warning", palette.accent);
        } else {
            theme::paint_colour_source_icon(ui.painter(), mark, &row.source, palette.accent);
        }
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
                .corner_radius(theme::round(8));
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
                        theme::paint_source_icon(
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
        let row = wide_row();
        let top = body.top() + 8.0 + i as f32 * row;
        if top + row > body.bottom() {
            break;
        }
        let row = Rect::from_min_size(
            pos2(body.left() + 20.0, top),
            vec2(body.width() - 40.0, row),
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

/// A warning mark and why the last change failed on the row, in `place`.
/// The whole reason shows on hover.
fn failed_summary(ui: &Ui, painter: &egui::Painter, place: Rect, reason: &str, id: Id) {
    let palette = Palette::current(ui.ctx());
    let mark = Rect::from_min_size(
        pos2(place.left(), place.center().y - 7.0),
        Vec2::splat(14.0),
    );
    theme::paint_icon(painter, mark, "warning", palette.warning);
    let text = one_line(
        ui,
        reason,
        theme::font(12.5),
        palette.warning,
        (place.width() - 20.0).max(0.0),
    );
    painter.galley(
        pos2(place.left() + 20.0, place.center().y - text.size().y / 2.0),
        text,
        palette.warning,
    );
    ui.interact(place, id.with("failed"), Sense::hover())
        .on_hover_text(reason);
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
            "None of your sources work here.".into()
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
        // The button says what to do; the line only names what failed.
        if good_news {
            model::failure_title(&failures)
        } else {
            String::new()
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
            let labels = empty_actions(
                app.page,
                failures.len(),
                phase == "cancelled",
                filtered || installed_filters,
            );
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
                        "Retry" if failures.len() == 1 => {
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

/// The buttons under an empty list, with their icons. Anything that
/// failed or stopped offers Retry, whatever it was doing.
fn empty_actions(
    page: Page,
    failures: usize,
    cancelled: bool,
    filtered: bool,
) -> Vec<(&'static str, &'static str)> {
    if failures > 0 {
        return vec![("Retry", "refresh"), ("Details", "info")];
    }
    if cancelled {
        return vec![("Retry", "refresh")];
    }
    if page == Page::Search {
        return vec![];
    }
    let mut labels = vec![(
        if matches!(page, Page::Updates | Page::Clean) {
            "Check again"
        } else {
            "Reload"
        },
        "refresh",
    )];
    if filtered {
        labels.push(("Clear filters", "cancel"));
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::{status_line, waiting_text};

    #[test]
    fn failed_or_stopped_lists_always_offer_retry() {
        use super::{empty_actions, Page};
        for page in Page::ALL {
            for failures in [1, 3] {
                assert_eq!(
                    empty_actions(page, failures, false, true),
                    [("Retry", "refresh"), ("Details", "info")]
                );
            }
            assert_eq!(empty_actions(page, 0, true, true), [("Retry", "refresh")]);
        }
        assert!(empty_actions(Page::Search, 0, false, true).is_empty());
        assert_eq!(
            empty_actions(Page::Updates, 0, false, false),
            [("Check again", "refresh")]
        );
        assert_eq!(
            empty_actions(Page::Installed, 0, false, true),
            [("Reload", "refresh"), ("Clear filters", "cancel")]
        );
    }

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

    #[test]
    fn waiting_line_replaces_the_busy_word_but_not_searching() {
        let apt = vec!["APT".to_owned()];
        assert_eq!(
            status_line("Refreshing".into(), false, &apt),
            "Waiting for APT"
        );
        assert_eq!(
            status_line("Loading".into(), false, &apt),
            "Waiting for APT"
        );
        assert_eq!(
            status_line("Searching".into(), true, &apt),
            "Searching · Waiting for APT"
        );
        assert_eq!(status_line("Refreshing".into(), false, &[]), "Refreshing");
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    fn row(width: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 60.0))
    }

    #[test]
    fn compact_rows_make_room_for_a_second_line() {
        use crate::{app::Launch, platform::Platform, settings::Store};
        let (store, settings) = Store::open(None, None);
        let mut app = App::with_controller(
            pkgdeck_app::controller::ffi::create_synthetic_controller(|_, _, _, _| {
                Ok(pkgdeck_core::engine::Engine::default())
            }),
            settings,
            store,
            Platform::start(|| {}),
            Launch::default(),
        );
        app.rows = vec![serde_json::from_value(serde_json::json!({
            "kind": "package", "name": "apt", "source": "apt", "installed": "1.0"
        }))
        .unwrap()];
        app.page = Page::Installed;
        app.invalidate();
        app.refresh_items();
        assert!(!app.items.is_empty());
        assert_eq!(row_height(&app, 0, Width::Compact), ROW_COMPACT);
        assert_eq!(row_height(&app, 0, Width::Wide), ROW);
    }

    #[test]
    fn every_row_lines_up_with_the_headings_whatever_its_buttons() {
        for (width, kind) in [
            (1000.0, Width::Wide),
            (650.0, Width::Wide),
            (600.0, Width::Medium),
        ] {
            let heading = columns(
                row(width),
                kind,
                Page::Installed,
                ACTIONS_ROOM,
                Shares::default(),
            );
            for buttons in [0.0, 38.0, 76.0] {
                let cols = columns(
                    row(width),
                    kind,
                    Page::Installed,
                    buttons,
                    Shares::default(),
                );
                assert_eq!(cols.name, heading.name, "{width} {buttons}");
                assert_eq!(cols.version, heading.version, "{width} {buttons}");
                assert_eq!(cols.summary, heading.summary, "{width} {buttons}");
                assert_eq!(cols.actions.right(), heading.actions.right());
            }
        }
    }

    #[test]
    fn the_summary_fits_beside_the_list_of_a_4_by_3_window() {
        // The list in a 1040×780 window is about 700 wide.
        let cols = columns(
            row(700.0),
            Width::Wide,
            Page::Installed,
            38.0,
            Shares::default(),
        );
        assert!(cols.summary.is_some_and(|s| s.width() > 150.0));
        const { assert!(700.0 >= MEDIUM_BELOW) };
    }

    #[test]
    fn a_divider_dropped_where_a_column_ends_keeps_that_width() {
        let cols = columns(
            row(1000.0),
            Width::Wide,
            Page::Installed,
            38.0,
            Shares::default(),
        );
        let version = cols.version.unwrap();
        let name = dragged_share("name", version.left() - 6.0, &cols);
        assert!((name * cols.available - cols.name.width()).abs() < 0.5);
        let share = dragged_share("version", version.right() + 6.0, &cols);
        assert!((share * cols.available - version.width()).abs() < 0.5);
        assert_eq!(dragged_share("name", -500.0, &cols), 0.05);
        assert_eq!(dragged_share("version", 5000.0, &cols), 0.9);
    }

    #[test]
    fn dragged_shares_resize_the_columns_within_limits() {
        let default = columns(
            row(1000.0),
            Width::Wide,
            Page::Installed,
            38.0,
            Shares::default(),
        );
        let wide_name = Shares {
            name: 0.6,
            version: 0.0,
        };
        let cols = columns(row(1000.0), Width::Wide, Page::Installed, 38.0, wide_name);
        assert!(cols.name.width() > default.name.width());
        assert!((cols.name.width() - cols.available * 0.6).abs() < 0.5);
        // The summary keeps some room, and nothing overlaps.
        let greedy = Shares {
            name: 0.9,
            version: 0.9,
        };
        let cols = columns(row(1000.0), Width::Wide, Page::Installed, 38.0, greedy);
        let version = cols.version.unwrap();
        assert!(cols.name.right() < version.left());
        assert!(cols.summary.is_none_or(|s| version.right() < s.left()));
        assert!(version.right() < cols.actions.left());
        // Tiny shares still leave readable columns.
        let tiny = Shares {
            name: 0.01,
            version: 0.01,
        };
        let cols = columns(row(1000.0), Width::Wide, Page::Installed, 38.0, tiny);
        assert!(cols.name.width() >= 120.0);
        assert!(cols.version.unwrap().width() >= 80.0);
    }
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    #[test]
    fn rows_without_an_icon_show_their_source_mark_or_a_warning() {
        use crate::{app::Launch, platform::Platform, settings::Store};
        let (store, settings) = Store::open(None, None);
        let app = App::with_controller(
            pkgdeck_app::controller::ffi::create_synthetic_controller(|_, _, _, _| {
                Ok(pkgdeck_core::engine::Engine::default())
            }),
            settings,
            store,
            Platform::start(|| {}),
            Launch::default(),
        );
        let texts = |kind: &str| {
            let row: Row = serde_json::from_value(serde_json::json!({
                "kind": kind, "name": "fixture", "source": "dnf"
            }))
            .unwrap();
            theme::painted(|ui| {
                let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(50.0, 50.0));
                package_icon(&app, ui, rect, &row);
            })
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.job.sections[0].format.font_id.size,
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
        };
        // The mark takes the middle half: 50 less 24% each side.
        assert_eq!(texts("package"), [("Dn".to_owned(), 26.0 * 0.4)]);
        assert!(texts("failure").is_empty());
    }
}

#[cfg(test)]
mod grid_tests {
    use crate::{
        app::{App, Launch},
        platform::Platform,
        settings::Store,
    };
    use crate::{model::Page, settings::Style};
    use eframe::egui::{self, pos2, vec2, Rect};
    use egui_kittest::Harness;
    use pkgdeck_app::controller::ffi::create_synthetic_controller;
    use serde_json::json;

    /// Every text run drawn in `shape`, recursing into grouped shapes.
    fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }

    #[test]
    fn card_body_stays_on_the_ui_grid_when_the_card_is_off_grid() {
        // egui flags any Ui edge off the 1/32 grid with an orange "Unaligned" label.
        // A card can land off the grid while the details panel animates open, and
        // a long list keeps its scrollbar, whose strip must also stay on the grid.
        let rows: Vec<_> = (0..200)
            .map(|i| {
                json!({ "kind": "package", "name": format!("pkg{i}"), "display_name": format!("Package {i}"),
                    "source": "apt", "architecture": "x86_64", "scope": "system", "scope_label": "System",
                    "installed": "1.0", "candidate": null, "update": "current", "summary": "Summary" })
            })
            .collect();
        for style in Style::ALL {
            for (pixels_per_point, top) in [1.0, 1.25, 1.5, 1.75]
                .into_iter()
                .flat_map(|ppp| [20.0, 20.3, 20.7, 20.9].map(|top| (ppp, top)))
            {
                let (store, settings) = Store::open(None, None);
                let mut platform = Platform::start(|| {});
                platform.record_notifications();
                let mut app = App::with_controller(
                    create_synthetic_controller(|_, _, _, _| {
                        Ok(pkgdeck_core::engine::Engine::default())
                    }),
                    settings,
                    store,
                    platform,
                    Launch::default(),
                );
                app.settings.style = style;
                app.page = Page::Installed;
                app.c().set_rows(serde_json::Value::Array(rows.clone()).to_string().into());
                app.react();
                let mut harness = Harness::builder()
                    .with_size(vec2(900.0, 700.0))
                    .build_ui_state(
                        |ui, (ready, app): &mut (bool, App)| {
                            if !*ready {
                                crate::setup(ui.ctx(), std::path::Path::new("/"), None);
                                *ready = true;
                                return;
                            }
                            // The custom scrollbar only draws once the app has set the theme.
                            crate::ui::theme::set_scheme(ui.ctx(), crate::ui::theme::Scheme::of(&app.settings));
                            let card = Rect::from_min_size(pos2(20.0, top), vec2(600.0, 500.0));
                            super::list_card(app, ui, card);
                        },
                        (false, app),
                    );
                harness.set_pixels_per_point(pixels_per_point);
                harness.run_steps(3);
                let mut drawn = Vec::new();
                for clipped in &harness.output().shapes {
                    texts(&clipped.shape, &mut drawn);
                }
                // Without rows the card shows a skeleton, and the test would pass vacuously.
                assert!(drawn.iter().any(|t| t == "Package 0"), "rows were not drawn");
                let labels = drawn.iter().filter(|t| *t == "Unaligned").count();
                assert_eq!(
                    labels, 0,
                    "{labels} off-grid edges in the Installed card body, {style:?} at {pixels_per_point} ppp"
                );
            }
        }
    }
}
