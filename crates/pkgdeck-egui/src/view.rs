//! Draws the window: the sidebar, then the Installed page as the Qt app
//! lays it out. A filter on top, the list in a card, and the open
//! package's details in a card below it, with a handle between the two.

use crate::{
    failure_line, row_label, shown_name, source_line,
    theme::{self, paint_icon, Palette, CARD_RADIUS, CONTROL_RADIUS, SMALL_RADIUS},
    update_version, Installed, SortBy,
};
use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Galley,
    Id, Key, Layout, Margin, Modifiers, Pos2, Rect, Response, RichText, ScrollArea, Sense, Stroke,
    TextFormat, TextWrapMode, Ui, Vec2, WidgetInfo, WidgetType,
};
use std::sync::Arc;

/// The sections of the Qt app's sidebar. Only Installed is in this trial.
const SECTIONS: [(&str, &str); 6] = [
    ("search", "Search"),
    ("installed", "Installed"),
    ("updates", "Updates"),
    ("remove", "Clean"),
    ("sources", "Sources"),
    ("settings", "Settings"),
];
/// Under this width the sidebar shows only icons and the list drops its
/// summary column, as the Qt app's compact layout does.
const NARROW: f32 = 820.0;
const ROW_HEIGHT: f32 = 58.0;
const GAP: f32 = 14.0;
const LIST_MIN: f32 = 170.0;
const DETAILS_MIN: f32 = 150.0;

fn filter_id() -> Id {
    Id::new("installed_filter")
}
fn details_height_id() -> Id {
    Id::new("installed_details_height")
}
fn row_id(id: &pkgdeck_core::package::PackageId) -> Id {
    // Include the whole identity, including Flatpak scope and native ref.
    Id::new(("installed_row", format!("{id:?}")))
}

pub fn window(ui: &mut Ui, page: &mut Installed) {
    page.poll();
    keyboard(ui, page);
    let palette = Palette::current(ui.ctx());
    let narrow = ui.available_width() < NARROW;
    egui::Panel::left("sidebar")
        .resizable(false)
        .exact_size(if narrow { 68.0 } else { 220.0 })
        .frame(
            Frame::new()
                .fill(palette.surface)
                .inner_margin(Margin::symmetric(if narrow { 10 } else { 16 }, 22)),
        )
        .show(ui, |ui| sidebar(ui, &palette, narrow));
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.canvas)
                .inner_margin(Margin::same(if narrow { 16 } else { 26 })),
        )
        .show(ui, |ui| content(ui, page, &palette, narrow));
}

/// Keys work wherever the focus is, as in the Qt app: arrows move the
/// highlight, Enter opens it, Esc closes details or clears the filter.
fn keyboard(ui: &Ui, page: &mut Installed) {
    let focused = ui.memory(|memory| memory.focused());
    let typing = focused == Some(filter_id());
    let in_list = focused.is_none()
        || typing
        || page
            .packages()
            .iter()
            .any(|package| focused == Some(row_id(&package.id)));
    let press =
        |modifiers: Modifiers, key: Key| ui.input_mut(|input| input.consume_key(modifiers, key));
    let none = Modifiers::NONE;
    for (key, by) in [
        (Key::ArrowDown, 1),
        (Key::ArrowUp, -1),
        (Key::PageDown, 10),
        (Key::PageUp, -10),
    ] {
        if in_list && press(none, key) {
            page.move_cursor(by);
        }
    }
    if in_list && !typing {
        for (key, by) in [(Key::Home, isize::MIN / 2), (Key::End, isize::MAX / 2)] {
            if press(none, key) {
                page.move_cursor(by);
            }
        }
        if press(none, Key::Space) {
            page.open_cursor();
        }
    }
    if in_list && press(none, Key::Enter) {
        if page.cursor().is_none() {
            page.move_cursor(1);
        }
        page.open_cursor();
    }
    if press(none, Key::Escape) {
        page.escape();
    }
    if press(Modifiers::COMMAND, Key::F) {
        ui.memory_mut(|memory| memory.request_focus(filter_id()));
    }
    if press(Modifiers::COMMAND, Key::R) && !page.loading() {
        page.reload();
    }
}

fn sidebar(ui: &mut Ui, palette: &Palette, narrow: bool) {
    ui.horizontal(|ui| {
        ui.add_space(if narrow { 8.0 } else { 6.0 });
        ui.add(
            egui::Image::new(egui::include_image!("../../pkgdeck/assets/logo.svg"))
                .fit_to_exact_size(Vec2::splat(30.0)),
        );
        if !narrow {
            ui.label(
                RichText::new("PkgDeck")
                    .font(theme::bold(22.0))
                    .color(palette.ink),
            );
        }
    });
    ui.add_space(30.0);
    for (icon, name) in SECTIONS {
        section(ui, palette, icon, name, name == "Installed", narrow);
        ui.add_space(4.0);
    }
    if !narrow {
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.add_space(6.0);
                ui.label(RichText::new("Made with").color(palette.muted));
                let (heart, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                paint_icon(ui.painter(), heart, "heart", palette.heart);
                ui.label(RichText::new("by astro").color(palette.muted));
            });
        });
    }
}

fn section(ui: &mut Ui, palette: &Palette, icon: &str, name: &str, current: bool, narrow: bool) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::hover());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, current, current, name));
    let painter = ui.painter();
    if current {
        painter.rect_filled(rect, CONTROL_RADIUS, palette.selection);
        painter.rect_filled(
            Rect::from_min_size(
                rect.min + Vec2::new(0.0, 11.0),
                Vec2::new(3.0, rect.height() - 22.0),
            ),
            2.0,
            palette.accent,
        );
    }
    let ink = if current { palette.ink } else { palette.muted };
    let glyph = if narrow {
        Rect::from_center_size(rect.center(), Vec2::splat(20.0))
    } else {
        Rect::from_center_size(
            Pos2::new(rect.left() + 26.0, rect.center().y),
            Vec2::splat(20.0),
        )
    };
    paint_icon(
        painter,
        glyph,
        icon,
        if current { palette.accent } else { ink },
    );
    if !narrow {
        let font = if current {
            theme::bold(15.0)
        } else {
            theme::font(15.0)
        };
        painter.text(
            Pos2::new(rect.left() + 52.0, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            font,
            ink,
        );
    }
    if !current {
        response.on_hover_text(format!("{name} isn't in the egui trial yet"));
    } else if narrow {
        response.on_hover_text(name);
    }
}

fn content(ui: &mut Ui, page: &mut Installed, palette: &Palette, narrow: bool) {
    ui.label(
        RichText::new("Installed")
            .font(theme::font(if narrow { 22.0 } else { 26.0 }))
            .color(palette.ink),
    );
    ui.add_space(GAP);
    filter_field(ui, page, palette);
    ui.add_space(GAP);
    let available = ui.available_height();
    if page.selected().is_some() && available < LIST_MIN + DETAILS_MIN + GAP {
        ScrollArea::vertical()
            .id_salt("installed_compact_cards")
            .max_height(available)
            .auto_shrink(false)
            .show(ui, |ui| {
                cards(ui, page, palette, LIST_MIN + DETAILS_MIN + GAP, narrow)
            });
    } else {
        cards(ui, page, palette, available, narrow);
    }
}

fn cards(ui: &mut Ui, page: &mut Installed, palette: &Palette, available: f32, narrow: bool) {
    if page.selected().is_none() {
        list_card(ui, page, palette, available, narrow);
        return;
    }
    // The details take the height chosen last, within what the window has.
    let saved: Option<f32> = ui.data_mut(|data| data.get_persisted(details_height_id()));
    let most = (available - LIST_MIN - GAP).max(DETAILS_MIN);
    let details = saved.unwrap_or(available * 0.45).clamp(DETAILS_MIN, most);
    list_card(ui, page, palette, available - details - GAP, narrow);
    handle(ui, palette, details, most);
    details_card(ui, page, palette, details);
}

/// The line between the list and the details. Drag it to resize them;
/// a double-click goes back to the usual split.
fn handle(ui: &mut Ui, palette: &Palette, details: f32, most: f32) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), GAP),
        Sense::click_and_drag(),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Resize details"));
    let response = response.on_hover_cursor(CursorIcon::ResizeVertical);
    if response.dragged() && response.drag_delta().y != 0.0 {
        let next = (details - response.drag_delta().y).clamp(DETAILS_MIN, most);
        ui.data_mut(|data| data.insert_persisted(details_height_id(), next));
    }
    if response.double_clicked() {
        ui.data_mut(|data| data.remove::<f32>(details_height_id()));
    }
    let active = response.hovered() || response.dragged();
    ui.painter().rect_filled(
        Rect::from_center_size(rect.center(), Vec2::new(36.0, 4.0)),
        2.0,
        if active {
            palette.accent
        } else {
            palette.strong_line
        },
    );
}

fn filter_field(ui: &mut Ui, page: &mut Installed, palette: &Palette) {
    let focused = ui.memory(|memory| memory.has_focus(filter_id()));
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(
            1.0,
            if focused {
                palette.accent
            } else {
                palette.line
            },
        ))
        .corner_radius(CONTROL_RADIUS)
        .inner_margin(Margin {
            left: 14,
            right: 6,
            top: 6,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(34.0);
                let (glass, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                paint_icon(ui.painter(), glass, "search", palette.muted);
                ui.add_space(4.0);
                let clear = if page.filter().is_empty() { 0.0 } else { 40.0 };
                let mut text = page.filter().to_owned();
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .id(filter_id())
                        .frame(Frame::NONE)
                        .hint_text(RichText::new("Filter installed packages").color(palette.muted))
                        .font(theme::font(15.5))
                        .text_color(palette.ink)
                        .margin(Margin::ZERO)
                        .desired_width(ui.available_width() - clear),
                );
                edit.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::TextEdit, true, "Filter installed packages")
                });
                if edit.changed() {
                    page.set_filter(&text);
                }
                if !page.filter().is_empty()
                    && icon_button(ui, palette, "cancel", "Clear filter", palette.muted).clicked()
                {
                    page.set_filter("");
                    ui.memory_mut(|memory| memory.request_focus(filter_id()));
                }
            });
        });
}

/// A square button with an icon, named for screen readers and tooltips.
fn icon_button(ui: &mut Ui, palette: &Palette, icon: &str, name: &str, ink: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CONTROL_RADIUS, palette.hover);
    }
    paint_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(18.0)),
        icon,
        ink,
    );
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(name)
}

/// Where each column starts and how wide it is, inside a row.
struct Columns {
    icon: f32,
    name: f32,
    name_width: f32,
    version: f32,
    version_width: f32,
    summary: Option<(f32, f32)>,
}
impl Columns {
    fn new(row: Rect, narrow: bool) -> Self {
        let icon = row.left() + 18.0;
        let name = icon + 32.0 + 14.0;
        let room = row.right() - 18.0 - name;
        if narrow {
            let version_width = (room * 0.38).clamp(80.0, 170.0);
            return Self {
                icon,
                name,
                name_width: room - version_width - 12.0,
                version: row.right() - 18.0 - version_width,
                version_width,
                summary: None,
            };
        }
        let name_width = (room * 0.34).clamp(150.0, 330.0);
        let version_width = (room * 0.2).clamp(110.0, 210.0);
        let version = name + name_width + 16.0;
        let summary = version + version_width + 16.0;
        Self {
            icon,
            name,
            name_width,
            version,
            version_width,
            summary: Some((summary, row.right() - 18.0 - summary)),
        }
    }
}

/// `text` on one line, cut with "…" when wider than `width`.
fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap.max_width = width.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    ui.painter().layout_job(job)
}

fn count_text(shown: usize, total: usize) -> String {
    let noun = |n: usize| if n == 1 { "package" } else { "packages" };
    if shown == total {
        format!("{total} {}", noun(total))
    } else {
        format!("{shown} of {total} {}", noun(total))
    }
}

fn list_card(ui: &mut Ui, page: &mut Installed, palette: &Palette, height: f32, narrow: bool) {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(CARD_RADIUS)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((height - 2.0).max(0.0));
            let shown = page.visible().len();
            Frame::new()
                .inner_margin(Margin {
                    left: 18,
                    right: 10,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(34.0);
                        ui.label(
                            RichText::new(count_text(shown, page.packages().len()))
                                .color(palette.muted),
                        );
                        let waiting = page.waiting();
                        if !waiting.is_empty() {
                            ui.label(
                                RichText::new(format!("Waiting for {}", waiting.join(", ")))
                                    .color(palette.muted)
                                    .small(),
                            );
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if page.loading() {
                                ui.add_space(9.0);
                                ui.add(egui::Spinner::new().size(16.0).color(palette.accent));
                            } else if icon_button(ui, palette, "refresh", "Reload", palette.muted)
                                .clicked()
                            {
                                page.reload();
                            }
                        });
                    });
                });
            line(ui, palette);
            for failure in page.failures() {
                notice(ui, palette, &failure_line(failure));
            }
            if let Some(error) = page.error() {
                notice(
                    ui,
                    palette,
                    &format!("Couldn't read installed packages. {error}"),
                );
                if shown == 0 {
                    return;
                }
            }
            if shown == 0 {
                let message = if page.loading() {
                    "Reading installed packages…".to_owned()
                } else if page.packages().is_empty() {
                    "Nothing installed was found.".to_owned()
                } else {
                    format!("Nothing installed matches “{}”.", page.filter().trim())
                };
                ui.add_space(36.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(message).color(palette.muted)));
                return;
            }
            if !narrow {
                column_headers(ui, page, palette);
            }
            rows(ui, page, palette, narrow);
        });
}

fn line(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.line);
}

/// Something went wrong, said on its own line with a warning sign.
fn notice(ui: &mut Ui, palette: &Palette, text: &str) {
    Frame::new()
        .inner_margin(Margin {
            left: 18,
            right: 18,
            top: 10,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (sign, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_icon(ui.painter(), sign, "warning", palette.warning);
                ui.add(egui::Label::new(RichText::new(text).color(palette.danger)).wrap());
            });
        });
}

fn column_headers(ui: &mut Ui, page: &mut Installed, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::hover());
    let columns = Columns::new(rect, false);
    let mut headings = vec![
        (
            SortBy::Name,
            "NAME / SOURCE",
            "Sort by name",
            columns.name,
            columns.name_width,
        ),
        (
            SortBy::Version,
            "VERSION",
            "Sort by version",
            columns.version,
            columns.version_width,
        ),
    ];
    if let Some((x, width)) = columns.summary {
        headings.push((SortBy::Summary, "SUMMARY", "Sort by summary", x, width));
    }
    let (sort, descending) = page.sorting();
    for (column, title, name, x, width) in headings {
        let mut job = LayoutJob::default();
        job.append(
            title,
            0.0,
            TextFormat {
                font_id: theme::bold(11.5),
                color: palette.muted,
                extra_letter_spacing: 0.8,
                ..Default::default()
            },
        );
        let galley = ui.painter().layout_job(job);
        let text = Rect::from_min_size(
            Pos2::new(x, rect.center().y - galley.size().y / 2.0),
            galley.size(),
        );
        let area = Rect::from_min_max(
            Pos2::new(x - 4.0, rect.top()),
            Pos2::new(x + width, rect.bottom()),
        );
        let response = ui.interact(area, Id::new(("sort", title)), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
        let ink = if response.hovered() {
            palette.ink
        } else {
            palette.muted
        };
        ui.painter().galley(text.min, galley, ink);
        if sort == column {
            // ▲ or ▼, drawn so it never depends on the font.
            let center = Pos2::new(text.right() + 9.0, rect.center().y);
            let (tip, base) = if descending { (3.0, -3.0) } else { (-3.0, 3.0) };
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(0.0, tip),
                    center + Vec2::new(4.0, base),
                    center + Vec2::new(-4.0, base),
                ],
                palette.accent,
                Stroke::NONE,
            ));
        }
        if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            page.sort_by(column);
        }
    }
}

fn rows(ui: &mut Ui, page: &mut Installed, palette: &Palette, narrow: bool) {
    let order = page
        .visible()
        .iter()
        .map(|package| package.id.clone())
        .collect::<Vec<_>>();
    let mut scroll = ScrollArea::vertical()
        .id_salt("installed_rows")
        .auto_shrink(false);
    // Bring the keyboard's row into view, from where the list was last frame.
    let view: Option<(f32, f32)> = ui.data(|data| data.get_temp(Id::new("installed_rows_view")));
    if page.reveal {
        page.reveal = false;
        if let (Some(at), Some((offset, height))) = (
            page.cursor()
                .and_then(|id| order.iter().position(|row| row == id)),
            view,
        ) {
            let top = at as f32 * ROW_HEIGHT;
            if top < offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + ROW_HEIGHT > offset + height {
                scroll = scroll.vertical_scroll_offset(top + ROW_HEIGHT - height);
            }
        }
    }
    let mut opened = None;
    let output = scroll.show_rows(ui, ROW_HEIGHT, order.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for index in range {
            if row(ui, page, palette, narrow, &order[index]) {
                opened = Some(order[index].clone());
            }
        }
    });
    ui.data_mut(|data| {
        data.insert_temp(
            Id::new("installed_rows_view"),
            (output.state.offset.y, output.inner_rect.height()),
        )
    });
    if let Some(id) = opened {
        page.open(&id);
    }
}

/// One package. Returns whether it was clicked.
fn row(
    ui: &mut Ui,
    page: &Installed,
    palette: &Palette,
    narrow: bool,
    id: &pkgdeck_core::package::PackageId,
) -> bool {
    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), ROW_HEIGHT));
    let response = ui.interact(rect, row_id(id), Sense::click());
    let package = page
        .packages()
        .iter()
        .find(|package| package.id == *id)
        .expect("visible package");
    let open = page.selected() == Some(id);
    let highlighted = open || page.cursor() == Some(id);
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            true,
            highlighted,
            row_label(package),
        )
    });
    let painter = ui.painter();
    let inner = rect.shrink2(Vec2::new(8.0, 2.0));
    if highlighted {
        painter.rect_filled(inner, CONTROL_RADIUS, palette.selection);
        if !open {
            painter.rect_stroke(
                inner,
                CONTROL_RADIUS,
                Stroke::new(1.5, palette.accent),
                egui::StrokeKind::Inside,
            );
        }
    } else if response.hovered() {
        painter.rect_filled(inner, CONTROL_RADIUS, palette.hover);
    } else {
        painter.hline(
            (inner.left() + 12.0)..=(inner.right() - 12.0),
            rect.bottom() - 0.5,
            Stroke::new(1.0, palette.line.gamma_multiply(0.6)),
        );
    }
    let columns = Columns::new(rect, narrow);
    let tile = Rect::from_min_size(
        Pos2::new(columns.icon, rect.center().y - 16.0),
        Vec2::splat(32.0),
    );
    package_icon(ui, palette, tile, package, 18.0);

    let name = one_line(
        ui,
        shown_name(package),
        theme::bold(15.0),
        palette.ink,
        columns.name_width,
    );
    let source = one_line(
        ui,
        &source_line(&package.id),
        theme::font(13.0),
        palette.muted,
        columns.name_width,
    );
    let top = rect.center().y - (name.size().y + 2.0 + source.size().y) / 2.0;
    let painter = ui.painter();
    let source_top = top + name.size().y + 2.0;
    painter.galley(Pos2::new(columns.name, top), name, palette.ink);
    painter.galley(Pos2::new(columns.name, source_top), source, palette.muted);

    let installed = package.installed_version.as_deref().unwrap_or("");
    let version = one_line(
        ui,
        installed,
        theme::mono(13.5),
        palette.ink,
        columns.version_width,
    );
    match update_version(package) {
        Some(next) => {
            let text = if next.is_empty() {
                "Update available".to_owned()
            } else {
                format!("Update {next}")
            };
            let update = one_line(
                ui,
                &text,
                theme::font(12.5),
                palette.accent,
                columns.version_width,
            );
            let top = rect.center().y - (version.size().y + 2.0 + update.size().y) / 2.0;
            let below = top + version.size().y + 2.0;
            ui.painter()
                .galley(Pos2::new(columns.version, top), version, palette.ink);
            ui.painter()
                .galley(Pos2::new(columns.version, below), update, palette.accent);
        }
        None => {
            let top = rect.center().y - version.size().y / 2.0;
            ui.painter()
                .galley(Pos2::new(columns.version, top), version, palette.ink);
        }
    }
    if let Some((x, width)) = columns.summary {
        let summary = one_line(ui, &package.summary, theme::font(14.5), palette.ink, width);
        let top = rect.center().y - summary.size().y / 2.0;
        ui.painter().galley(Pos2::new(x, top), summary, palette.ink);
    }
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// The package's own icon, or its source's on a tile.
fn package_icon(
    ui: &Ui,
    palette: &Palette,
    tile: Rect,
    package: &pkgdeck_core::package::Package,
    glyph: f32,
) {
    let painter = ui.painter();
    match &package.icon {
        Some(path) => {
            egui::Image::new(format!("file://{}", path.display()))
                .corner_radius(SMALL_RADIUS)
                .paint_at(ui, tile);
        }
        None => {
            painter.rect_filled(tile, CONTROL_RADIUS, palette.hover);
            paint_icon(
                painter,
                Rect::from_center_size(tile.center(), Vec2::splat(glyph)),
                &package.id.backend,
                palette.muted,
            );
        }
    }
}

fn details_card(ui: &mut Ui, page: &mut Installed, palette: &Palette, height: f32) {
    let Some(package) = page
        .selected()
        .and_then(|id| page.packages().iter().find(|package| package.id == *id))
        .cloned()
    else {
        return;
    };
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(CARD_RADIUS)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((height - 38.0).max(0.0));
            ui.horizontal(|ui| {
                let (tile, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
                package_icon(ui, palette, tile, &package, 28.0);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(shown_name(&package))
                            .font(theme::bold(20.0))
                            .color(palette.ink),
                    );
                    ui.label(RichText::new(source_line(&package.id)).color(palette.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if icon_button(ui, palette, "cancel", "Close details", palette.muted).clicked()
                    {
                        page.close_details();
                    }
                });
            });
            ui.add_space(10.0);
            ScrollArea::vertical()
                .id_salt("installed_details")
                .auto_shrink(false)
                .show(ui, |ui| details_body(ui, page, palette, &package));
        });
}

fn details_body(
    ui: &mut Ui,
    page: &mut Installed,
    palette: &Palette,
    package: &pkgdeck_core::package::Package,
) {
    let text = |ui: &mut Ui, text: &str| {
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .color(palette.ink)
                    .font(theme::font(14.5)),
            )
            .wrap(),
        );
    };
    match page.details() {
        None => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(palette.accent));
                ui.label(RichText::new("Loading details…").color(palette.muted));
            });
            text(ui, &package.summary);
        }
        Some(Err(error)) => {
            text(ui, &package.summary);
            ui.horizontal(|ui| {
                let (sign, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_icon(ui.painter(), sign, "warning", palette.warning);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Details couldn't be loaded. {error}"))
                            .color(palette.danger),
                    )
                    .wrap(),
                );
            });
        }
        Some(Ok(details)) => {
            let description = details.description.trim();
            text(
                ui,
                if description.is_empty() {
                    &package.summary
                } else {
                    description
                },
            );
        }
    }
    ui.add_space(12.0);
    let fact = |ui: &mut Ui, name: &str| {
        ui.label(RichText::new(name).color(palette.muted));
    };
    egui::Grid::new("installed_facts")
        .num_columns(2)
        .spacing([28.0, 8.0])
        .min_row_height(20.0)
        .show(ui, |ui| {
            if let Some(version) = &package.installed_version {
                fact(ui, "Version");
                ui.label(
                    RichText::new(version)
                        .font(theme::mono(13.5))
                        .color(palette.ink),
                );
                ui.end_row();
            }
            if let Some(next) = update_version(package) {
                fact(ui, "Update");
                ui.label(
                    RichText::new(if next.is_empty() { "Available" } else { next })
                        .color(palette.accent),
                );
                ui.end_row();
            }
            if let Some(Ok(details)) = page.details() {
                if let Some(homepage) = &details.homepage {
                    fact(ui, "Homepage");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let shown = homepage
                            .trim_start_matches("https://")
                            .trim_start_matches("http://")
                            .trim_end_matches('/');
                        ui.hyperlink_to(RichText::new(shown).color(palette.accent), homepage);
                        let (arrow, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                        paint_icon(ui.painter(), arrow, "external", palette.accent);
                    });
                    ui.end_row();
                }
            }
        });
    let dependencies = match page.details() {
        Some(Ok(details)) => details.dependencies.clone(),
        _ => vec![],
    };
    if dependencies.is_empty() {
        return;
    }
    ui.add_space(12.0);
    let title = format!("Dependencies ({})", dependencies.len());
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().min(260.0), 26.0),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::CollapsingHeader, true, &title));
    let ink = if response.hovered() {
        palette.ink
    } else {
        palette.muted
    };
    paint_icon(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 7.0, rect.center().y),
            Vec2::splat(13.0),
        ),
        if page.dependencies_open {
            "down"
        } else {
            "right"
        },
        ink,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 20.0, rect.center().y),
        Align2::LEFT_CENTER,
        &title,
        theme::bold(13.0),
        ink,
    );
    if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        page.dependencies_open = !page.dependencies_open;
    }
    if page.dependencies_open {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            for dependency in &dependencies {
                Frame::new()
                    .stroke(Stroke::new(1.0, palette.line))
                    .corner_radius(CornerRadius::same(SMALL_RADIUS))
                    .inner_margin(Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(dependency)
                                    .font(theme::font(12.5))
                                    .color(palette.ink),
                            )
                            .wrap_mode(TextWrapMode::Truncate),
                        );
                    });
            }
        });
    }
}
