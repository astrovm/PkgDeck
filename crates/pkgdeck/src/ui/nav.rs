//! How each style moves between pages, as its desktop's own apps did:
//! Explorer's folder tree on Windows 95 and 98, the strip of big buttons
//! down the side of XP's Add or Remove Programs, the task pane's links in
//! Vista's and 7's Programs and Features, Mac OS 9's tabs, and the Finder's
//! source list on Mac OS X. Modern keeps its own sidebar, in `mod.rs`.

use super::skin::{self, darker, gray, lighter, retro_edge, rgb, Nav};
use super::{focus_for_page, widgets::*};
use crate::{app::App, model::Page, theme};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

/// The width the side navigation takes; Mac OS 9's tabs take none.
pub fn width(nav: Nav) -> f32 {
    match nav {
        Nav::Rail => 224.0,
        Nav::Tree | Nav::Links => 208.0,
        Nav::Strip => 116.0,
        Nav::Source => 184.0,
        Nav::Tabs => 0.0,
    }
}

/// A page's name, with the updates waiting after it where the style
/// counts them in words.
fn named(app: &App, page: Page) -> String {
    match (page, app.background.available) {
        (Page::Updates, count) if count > 0 => format!("{} ({count})", page.name()),
        _ => page.name().to_owned(),
    }
}

/// One page's place in the navigation: clicking it opens the page.
fn entry(app: &mut App, ui: &mut Ui, rect: Rect, page: Page) -> Response {
    let response = ui
        .interact(rect, Id::new(("nav", page.name())), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let current = page == app.page;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, current, page.name())
    });
    if response.clicked() {
        app.open_page(page);
        focus_for_page(app);
    }
    response
}

/// Draws the side navigation of the style on screen in `rect`.
pub fn side(app: &mut App, ui: &mut Ui, rect: Rect) {
    match skin::nav() {
        Nav::Tree => tree(app, ui, rect),
        Nav::Strip => strip(app, ui, rect),
        Nav::Links => links(app, ui, rect),
        Nav::Source => source(app, ui, rect),
        Nav::Rail | Nav::Tabs => {}
    }
}

/// A dotted line, one pixel on and one off, as tree views joined items.
fn dotted(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, color: Color32) {
    let steps = (to - from).length().round() as i32;
    let step = (to - from) / steps.max(1) as f32;
    for i in (0..=steps).step_by(2) {
        let at = from + step * i as f32;
        painter.rect_filled(
            Rect::from_min_size(at, Vec2::splat(1.0)),
            CornerRadius::ZERO,
            color,
        );
    }
}

/// Windows 95's Explorer: "All Folders" over a sunken white tree, PkgDeck
/// at its root and the pages under it, joined by dotted lines; the chosen
/// one's name in navy.
fn tree(app: &mut App, ui: &mut Ui, rect: Rect) {
    let palette = theme::Palette::current(ui.ctx());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, palette.canvas);
    let text = skin::list_look().text;
    let bar = Rect::from_min_max(
        rect.min + vec2(4.0, 4.0),
        pos2(rect.right() - 6.0, rect.top() + 26.0),
    );
    retro_edge(&painter, bar, palette.canvas, false, 1.0);
    painter.text(
        bar.left_center() + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        "All Folders",
        theme::font(text),
        palette.ink,
    );
    let pane = Rect::from_min_max(
        pos2(bar.left(), bar.bottom() + 2.0),
        pos2(bar.right(), rect.bottom() - 4.0),
    );
    painter.rect_filled(pane, CornerRadius::ZERO, palette.surface);
    retro_edge(&painter, pane, palette.canvas, true, 1.0);
    let lines = if palette.dark {
        palette.muted
    } else {
        gray(0x80)
    };
    let row = 20.0;
    let root = Rect::from_min_size(pane.min + vec2(6.0, 6.0), vec2(pane.width() - 12.0, row));
    // The root's box, open: a minus in a grey square.
    let toggle = Rect::from_center_size(pos2(root.left() + 5.5, root.center().y), Vec2::splat(9.0));
    painter.rect_filled(toggle, CornerRadius::ZERO, palette.surface);
    painter.rect_stroke(
        toggle,
        CornerRadius::ZERO,
        Stroke::new(1.0, lines),
        StrokeKind::Inside,
    );
    painter.hline(
        toggle.left() + 2.0..=toggle.right() - 2.0,
        toggle.center().y,
        Stroke::new(1.0, palette.ink),
    );
    let logo = Rect::from_center_size(pos2(root.left() + 22.0, root.center().y), Vec2::splat(16.0));
    egui::Image::new(egui::include_image!("../../assets/logo.svg"))
        .fit_to_exact_size(logo.size())
        .paint_at(ui, logo);
    painter.text(
        pos2(logo.right() + 5.0, root.center().y),
        Align2::LEFT_CENTER,
        "PkgDeck",
        theme::font(text),
        palette.ink,
    );
    let trunk = logo.center().x;
    let mut last = root.center().y;
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let item = Rect::from_min_size(
            pos2(trunk + 10.0, root.bottom() + index as f32 * row),
            vec2(pane.right() - trunk - 16.0, row),
        );
        last = item.center().y;
        dotted(
            &painter,
            pos2(trunk, item.center().y),
            pos2(item.left() - 1.0, item.center().y),
            lines,
        );
        let icon = Rect::from_min_size(
            pos2(item.left() + 1.0, item.center().y - 8.0),
            Vec2::splat(16.0),
        );
        let current = page == app.page;
        let ink = if current {
            skin::ink_on(palette.deep)
        } else {
            palette.ink
        };
        let label = one_line(
            ui,
            &named(app, page),
            theme::font(text),
            ink,
            item.right() - icon.right() - 8.0,
        );
        let name = Rect::from_min_size(
            pos2(icon.right() + 3.0, item.center().y - 8.0),
            vec2(label.size().x + 4.0, 16.0),
        );
        let response = entry(app, ui, item, page);
        theme::paint_colour_icon(&painter, icon, page.icon(), palette.ink);
        if current {
            painter.rect_filled(name, CornerRadius::ZERO, palette.deep);
        }
        // The keyboard's dotted outline round the chosen name.
        if current && response.has_focus() {
            painter.rect_stroke(
                name,
                CornerRadius::ZERO,
                Stroke::new(1.0, lighter(palette.deep, 0.6)),
                StrokeKind::Inside,
            );
        }
        painter.galley(
            name.left_center() + vec2(2.0, -label.size().y / 2.0),
            label,
            ink,
        );
    }
    dotted(
        &painter,
        pos2(trunk, logo.bottom() + 1.0),
        pos2(trunk, last),
        lines,
    );
}

/// XP's Add or Remove Programs: a blue strip down the side with a big
/// button for each page, its icon over its name; the chosen one lit.
fn strip(app: &mut App, ui: &mut Ui, rect: Rect) {
    let palette = theme::Palette::current(ui.ctx());
    let painter = ui.painter_at(rect);
    let base = super::luna::base(palette.deep);
    let (top, bottom) = if palette.dark {
        (
            skin::lerp(palette.canvas, palette.deep, 0.45),
            skin::lerp(palette.canvas, palette.deep, 0.25),
        )
    } else {
        (
            super::luna::recolor(0x7a96df, base),
            super::luna::recolor(0x5a7edc, base),
        )
    };
    skin::gradient(&painter, rect, CornerRadius::ZERO, top, bottom, false);
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        Stroke::new(1.0, darker(bottom, 0.35)),
    );
    let text = skin::list_look().text - 1.0;
    let button = 72.0;
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let item = Rect::from_min_size(
            pos2(
                rect.left() + 6.0,
                rect.top() + 8.0 + index as f32 * (button + 4.0),
            ),
            vec2(rect.width() - 12.0, button),
        );
        let response = entry(app, ui, item, page);
        let current = page == app.page;
        let hover = ease(
            ui.ctx(),
            response.id.with("hover"),
            response.hovered(),
            FEEDBACK,
        );
        if current || hover > 0.0 {
            // A lighter square with a white edge, as the chosen button.
            let strength = if current { 1.0 } else { hover * 0.6 };
            painter.rect_filled(
                item,
                theme::round(3),
                Color32::from_white_alpha((70.0 * strength) as u8),
            );
            painter.rect_stroke(
                item,
                theme::round(3),
                Stroke::new(1.0, Color32::from_white_alpha((200.0 * strength) as u8)),
                StrokeKind::Inside,
            );
        }
        theme::paint_colour_icon(
            &painter,
            Rect::from_center_size(pos2(item.center().x, item.top() + 22.0), Vec2::splat(28.0)),
            page.icon(),
            Color32::WHITE,
        );
        let label = wrapped(ui, &named(app, page), theme::font(text), item.width() - 8.0);
        painter.galley(
            // Centred lines are laid out about the galley's left edge.
            pos2(item.center().x, item.top() + 42.0),
            label,
            Color32::WHITE,
        );
    }
}

/// Text centred in lines no wider than `width`.
fn wrapped(ui: &Ui, text: &str, font: egui::FontId, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, Color32::WHITE, width);
    job.halign = egui::Align::Center;
    ui.painter().layout_job(job)
}

/// Windows 7's Programs and Features: the task pane's links, PkgDeck's
/// home at the top in bold, the page on screen in black.
fn links(app: &mut App, ui: &mut Ui, rect: Rect) {
    let palette = theme::Palette::current(ui.ctx());
    let painter = ui.painter_at(rect);
    let ink = skin::paint_sidebar(&painter, rect, Rect::NOTHING, Rect::NOTHING, &palette);
    let text = skin::list_look().text;
    let link = ink.entries.unwrap_or(if palette.dark {
        palette.bright
    } else {
        rgb(0x1e3287)
    });
    let home = Rect::from_min_size(rect.min + vec2(20.0, 20.0), vec2(rect.width() - 30.0, 22.0));
    painter.text(
        home.left_center(),
        Align2::LEFT_CENTER,
        "PkgDeck",
        theme::bold(text),
        link,
    );
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let item = Rect::from_min_size(
            pos2(home.left(), home.bottom() + 12.0 + index as f32 * 26.0),
            vec2(home.width(), 22.0),
        );
        let response = entry(app, ui, item, page);
        let current = page == app.page;
        let color = if current {
            ink.entries.unwrap_or(palette.ink)
        } else if response.hovered() {
            lighter(link, 0.3)
        } else {
            link
        };
        let font = if current {
            theme::bold(text)
        } else {
            theme::font(text)
        };
        let galley = one_line(ui, &named(app, page), font, color, item.width());
        let at = pos2(item.left(), item.center().y - galley.size().y / 2.0);
        // Links underline under the pointer.
        if response.hovered() && !current {
            painter.hline(
                at.x..=at.x + galley.size().x,
                at.y + galley.size().y - 1.0,
                Stroke::new(1.0, color),
            );
        }
        painter.galley(at, galley, color);
    }
}

/// The Finder's source list: small icons and names, the chosen row in a
/// blue band, Settings set apart under a line.
fn source(app: &mut App, ui: &mut Ui, rect: Rect) {
    let palette = theme::Palette::current(ui.ctx());
    let painter = ui.painter_at(rect);
    skin::paint_sidebar(&painter, rect, Rect::NOTHING, Rect::NOTHING, &palette);
    let text = skin::list_look().text - 1.0;
    let row = 24.0;
    let mut y = rect.top() + 14.0;
    for page in Page::ALL {
        if page == Page::Settings {
            painter.hline(
                rect.left() + 12.0..=rect.right() - 12.0,
                y + 5.5,
                Stroke::new(1.0, if palette.dark { gray(0x40) } else { gray(0xc8) }),
            );
            y += 12.0;
        }
        let item = Rect::from_min_size(pos2(rect.left() + 1.0, y), vec2(rect.width() - 2.0, row));
        y += row;
        entry(app, ui, item, page);
        let current = page == app.page;
        let ink = if current {
            skin::paint_selection(&painter, item, &palette).unwrap_or(palette.ink)
        } else {
            palette.ink
        };
        theme::paint_colour_icon(
            &painter,
            Rect::from_min_size(
                pos2(item.left() + 14.0, item.center().y - 8.0),
                Vec2::splat(16.0),
            ),
            page.icon(),
            if current { ink } else { palette.accent },
        );
        let font = if current {
            theme::bold(text)
        } else {
            theme::font(text)
        };
        let galley = one_line(ui, &named(app, page), font, ink, item.width() - 46.0);
        painter.galley(
            pos2(item.left() + 38.0, item.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
}

/// Mac OS 9's tabs across the top of `rect`, one per page, the chosen one
/// joined to the panel under it. Returns the panel's inside.
pub fn tabs(app: &mut App, ui: &mut Ui, rect: Rect) -> Rect {
    let palette = theme::Palette::current(ui.ctx());
    let painter = ui.painter_at(rect.expand(1.0));
    let tone = |level| super::platinum::tone(level, palette.dark);
    let height = 24.0;
    let panel = Rect::from_min_max(pos2(rect.left(), rect.top() + height - 1.0), rect.max);
    // The panel: a dark edge, white just inside at the top and left, grey
    // at the bottom and right.
    painter.rect_filled(panel, CornerRadius::ZERO, tone(0xdd));
    painter.rect_stroke(
        panel,
        CornerRadius::ZERO,
        Stroke::new(1.0, tone(0x55)),
        StrokeKind::Inside,
    );
    skin::bevel(&painter, panel.shrink(1.0), tone(0xff), tone(0x99));
    let font = theme::title(skin::list_look().text);
    let mut x = rect.left() + 10.0;
    let mut chosen = None;
    for page in Page::ALL {
        let label = one_line(ui, &named(app, page), font.clone(), palette.ink, 240.0);
        let tab = Rect::from_min_size(pos2(x, rect.top()), vec2(label.size().x + 28.0, height));
        x = tab.right() - 1.0;
        let response = entry(app, ui, tab, page);
        if page == app.page {
            chosen = Some((tab, label));
            continue;
        }
        let face = if response.is_pointer_button_down_on() {
            tone(0xaa)
        } else {
            tone(0xcc)
        };
        paint_tab(
            &painter,
            Rect::from_min_max(tab.min + vec2(0.0, 2.0), tab.max),
            face,
            &palette,
            false,
        );
        painter.galley(
            tab.center() + vec2(-label.size().x / 2.0, 1.0 - label.size().y / 2.0),
            label,
            palette.ink,
        );
    }
    // The chosen tab last, over its neighbours and the panel's edge.
    if let Some((tab, label)) = chosen {
        paint_tab(&painter, tab, tone(0xdd), &palette, true);
        painter.galley(tab.center() - label.size() / 2.0, label, palette.ink);
    }
    panel.shrink(3.0)
}

/// One tab: rounded at the top, a dark edge, lit along its top and left.
/// The chosen one runs into the panel, with no edge along its foot.
fn paint_tab(
    painter: &egui::Painter,
    tab: Rect,
    face: Color32,
    palette: &theme::Palette,
    open: bool,
) {
    let tone = |level| super::platinum::tone(level, palette.dark);
    let radius = CornerRadius {
        nw: 4,
        ne: 4,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(tab, radius, face);
    painter.rect_stroke(
        tab,
        radius,
        Stroke::new(1.0, tone(0x55)),
        StrokeKind::Inside,
    );
    painter.hline(
        tab.left() + 3.0..=tab.right() - 3.0,
        tab.top() + 1.5,
        Stroke::new(1.0, tone(0xff)),
    );
    painter.vline(
        tab.left() + 1.5,
        tab.top() + 3.0..=tab.bottom() - 1.0,
        Stroke::new(1.0, tone(0xff)),
    );
    painter.vline(
        tab.right() - 1.5,
        tab.top() + 3.0..=tab.bottom() - 1.0,
        Stroke::new(1.0, tone(0x99)),
    );
    if open {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(tab.left() + 1.0, tab.bottom() - 2.0),
                pos2(tab.right() - 1.0, tab.bottom() + 1.0),
            ),
            CornerRadius::ZERO,
            face,
        );
        painter.vline(
            tab.left() + 1.5,
            tab.bottom() - 2.0..=tab.bottom() + 1.0,
            Stroke::new(1.0, tone(0xff)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_style_moves_between_pages_its_own_way() {
        assert_eq!(width(Nav::Tabs), 0.0);
        assert!(width(Nav::Strip) < width(Nav::Tree));
        assert_eq!(width(Nav::Rail), 224.0);
    }
}
