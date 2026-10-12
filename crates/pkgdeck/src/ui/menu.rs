//! The menu bar Windows 95, 98 and XP put under every window's title:
//! File, View, Package and Help, each item doing what its shortcut does.
//! Windows 95 sinks the open menu's title and lights items in navy; XP
//! lights both in its selection blue, on white menus.

use super::skin::{self, gray, retro_edge};
use super::focus_for_page;
use crate::{
    app::App,
    model::{self, Page},
    settings::Family,
    theme::{self, Palette},
};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, Id, Popup, PopupCloseBehavior, Rect, Response,
    Sense, SetOpenCommand, Stroke, Ui, UiBuilder,
};

/// How tall the bar is.
pub const HEIGHT: f32 = 22.0;

/// Whether the style on screen has a menu bar: the Windows styles before
/// Vista, which hid it.
pub fn shown() -> bool {
    matches!(theme::skin().family(), Family::Retro | Family::Luna)
}

/// A shortcut as the platform writes it.
fn keys(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// What a menu item does once chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    OpenFile,
    Exit,
    Show(Page),
    Activity,
    Reload,
    Install,
    Update,
    Remove,
    UpdateAll,
    RefreshSources,
    Repositories,
    Website,
    About,
}

/// One line of a menu: its name, shortcut, and whether it can be chosen
/// and is ticked. `None` is a separator.
pub struct Item {
    pub name: &'static str,
    pub keys: String,
    pub command: Command,
    pub enabled: bool,
    pub checked: bool,
}

fn item(name: &'static str, keys: String, command: Command, enabled: bool) -> Option<Item> {
    Some(Item {
        name,
        keys,
        command,
        enabled,
        checked: false,
    })
}

/// The menus, each with its items, as they stand for `app`.
pub fn menus(app: &App) -> Vec<(&'static str, Vec<Option<Item>>)> {
    let can_change = !app.busy || app.writing;
    let row = app.selected_row().filter(|_| can_change);
    let can_open_files = !model::file_patterns(&app.catalog).is_empty() && !app.writing;
    let pages = Page::ALL
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            Some(Item {
                name: page.name(),
                keys: keys(&(index + 1).to_string()),
                command: Command::Show(page),
                enabled: true,
                checked: page == app.page,
            })
        })
        .collect::<Vec<_>>();
    let mut view = pages;
    view.extend([
        None,
        item("Activity", keys("J"), Command::Activity, true),
        item("Refresh", keys("R"), Command::Reload, app.page.lists()),
    ]);
    vec![
        (
            "File",
            vec![
                item(
                    "Install from file…",
                    String::new(),
                    Command::OpenFile,
                    can_open_files,
                ),
                None,
                item("Exit", keys("Q"), Command::Exit, true),
            ],
        ),
        ("View", view),
        (
            "Package",
            vec![
                item(
                    "Install",
                    keys("I"),
                    Command::Install,
                    row.is_some_and(|row| !row.is_installed()),
                ),
                item(
                    "Update",
                    keys("U"),
                    Command::Update,
                    row.is_some_and(|row| row.has_update()),
                ),
                item(
                    "Remove",
                    keys("D"),
                    Command::Remove,
                    row.is_some_and(|row| model::can_remove(row, &app.catalog)),
                ),
                None,
                item(
                    "Update all",
                    keys("Shift+U"),
                    Command::UpdateAll,
                    app.page == Page::Updates && can_change,
                ),
                item(
                    "Refresh sources",
                    keys("M"),
                    Command::RefreshSources,
                    can_change,
                ),
                item("Repositories…", String::new(), Command::Repositories, true),
            ],
        ),
        (
            "Help",
            vec![
                item("PkgDeck website", String::new(), Command::Website, true),
                None,
                item("About PkgDeck", String::new(), Command::About, true),
            ],
        ),
    ]
}

/// Does what `command` says, as its shortcut would.
pub fn run(app: &mut App, ctx: &egui::Context, command: Command) {
    match command {
        Command::OpenFile => {
            app.c().check_sources();
            app.react();
            app.ui.open_link.clear();
            app.ui.open_dialog = true;
        }
        Command::Exit => app.quit(),
        Command::Show(page) => {
            app.open_page(page);
            focus_for_page(app);
        }
        Command::Activity => {
            app.drawer_open = !app.drawer_open;
            if app.drawer_open {
                app.c().refresh_activity();
                app.react();
            }
        }
        Command::Reload => app.reload(true, true),
        Command::Install => app.propose("install"),
        Command::Update => app.propose("upgrade"),
        Command::Remove => app.propose("remove"),
        Command::UpdateAll => app.upgrade_updates(),
        Command::RefreshSources => app.refresh_sources(),
        Command::Repositories => app.ui.repos_open = true,
        Command::Website => {
            ctx.open_url(egui::OpenUrl::new_tab("https://github.com/astrovm/PkgDeck"));
        }
        Command::About => app.open_page(Page::Settings),
    }
}

/// The colours a menu takes: its face, its text, the lit item's fill and
/// text, and greyed text.
struct Colours {
    face: Color32,
    ink: Color32,
    lit: Color32,
    lit_ink: Color32,
    off: Color32,
}

fn colours(palette: &Palette, popup: bool) -> Colours {
    // XP's menus are white; 95's take the face of everything 3D.
    let luna = theme::skin().family() == Family::Luna;
    let face = match (luna && popup, palette.dark) {
        (true, false) => Color32::WHITE,
        (true, true) => palette.surface,
        _ => palette.canvas,
    };
    Colours {
        face,
        ink: palette.ink,
        lit: palette.deep,
        lit_ink: skin::ink_on(palette.deep),
        off: if palette.dark { gray(0x70) } else { gray(0x80) },
    }
}

/// The bar across `rect`, with a menu under each title.
pub fn bar(app: &mut App, ui: &mut Ui, rect: Rect) {
    let palette = Palette::current(ui.ctx());
    let retro = theme::skin().family() == Family::Retro;
    let colours = colours(&palette, false);
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, colours.face);
    let font = theme::font(skin::list_look().text);
    let menus = menus(app);
    let any_open = menus
        .iter()
        .any(|(title, _)| Popup::is_id_open(ui.ctx(), popup_id(title)));
    let mut x = rect.left() + 4.0;
    let mut chosen = None;
    for (title, items) in menus {
        let galley = ui
            .painter()
            .layout_no_wrap(title.to_owned(), font.clone(), colours.ink);
        let cell = Rect::from_min_size(
            pos2(x, rect.top() + 2.0),
            vec2(galley.size().x + 14.0, rect.height() - 4.0),
        );
        x = cell.right();
        let response = ui.interact(cell, Id::new(("menu", title)), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
        let id = popup_id(title);
        let open = Popup::is_id_open(ui.ctx(), id);
        // With one menu open, pointing at another title opens it instead.
        let command = if response.clicked() {
            Some(SetOpenCommand::Toggle)
        } else if any_open && !open && response.hovered() {
            Some(SetOpenCommand::Bool(true))
        } else {
            None
        };
        let ink = if open && !retro {
            ui.painter()
                .rect_filled(cell, CornerRadius::ZERO, colours.lit);
            colours.lit_ink
        } else if open {
            // Windows 95 sinks the open menu's title.
            skin::bevel(ui.painter(), cell, gray(0x80), Color32::WHITE);
            colours.ink
        } else {
            if response.hovered() && retro {
                skin::bevel(ui.painter(), cell, Color32::WHITE, gray(0x80));
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(cell, CornerRadius::ZERO, colours.lit);
            }
            if response.hovered() && !retro {
                colours.lit_ink
            } else {
                colours.ink
            }
        };
        let shift = if open && retro {
            vec2(1.0, 1.0)
        } else {
            vec2(0.0, 0.0)
        };
        ui.painter().text(
            cell.center() + shift,
            Align2::CENTER_CENTER,
            title,
            font.clone(),
            ink,
        );
        if let Some(command) = menu(ui, &response, id, command, &items, &palette) {
            chosen = Some(command);
        }
    }
    if let Some(command) = chosen {
        run(app, ui.ctx(), command);
    }
}

fn popup_id(title: &str) -> Id {
    Id::new(("menu-popup", title))
}

/// The menu under a title: its items, lit under the pointer. Returns the
/// one chosen.
fn menu(
    ui: &mut Ui,
    title: &Response,
    id: Id,
    command: Option<SetOpenCommand>,
    items: &[Option<Item>],
    palette: &Palette,
) -> Option<Command> {
    let colours = colours(palette, true);
    let retro = theme::skin().family() == Family::Retro;
    let font = theme::font(skin::list_look().text);
    let frame = egui::Frame::NONE
        .fill(colours.face)
        .inner_margin(egui::Margin::same(3))
        .stroke(if retro {
            Stroke::NONE
        } else {
            Stroke::new(1.0, gray(0xac))
        });
    Popup::new(
        id,
        ui.ctx().clone(),
        title.rect.left_bottom(),
        title.layer_id,
    )
    .kind(egui::PopupKind::Menu)
    .open_memory(command)
    .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
    .frame(frame)
    .show(|ui| {
        let width = items
            .iter()
            .flatten()
            .map(|item| {
                let measure = |text: &str| {
                    ui.painter()
                        .layout_no_wrap(text.to_owned(), font.clone(), colours.ink)
                        .size()
                        .x
                };
                measure(item.name) + measure(&item.keys)
            })
            .fold(0.0, f32::max)
            + 72.0;
        let mut chosen = None;
        for line in items {
            let Some(item) = line else {
                let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
                let y = rect.center().y.round() - 0.5;
                ui.painter().hline(
                    rect.x_range(),
                    y,
                    Stroke::new(1.0, if retro { gray(0x80) } else { gray(0xac) }),
                );
                if retro {
                    ui.painter()
                        .hline(rect.x_range(), y + 1.0, Stroke::new(1.0, Color32::WHITE));
                }
                continue;
            };
            let sense = if item.enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let (rect, response) = ui.allocate_exact_size(vec2(width, 20.0), sense);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    item.enabled,
                    item.checked,
                    item.name,
                )
            });
            let lit = response.hovered() && item.enabled;
            if lit {
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, colours.lit);
            }
            let ink = if !item.enabled {
                colours.off
            } else if lit {
                colours.lit_ink
            } else {
                colours.ink
            };
            // Greyed items are etched, with a white shadow, on 95.
            let write = |at: egui::Pos2, align: Align2, text: &str| {
                if retro && !item.enabled {
                    ui.painter().text(
                        at + vec2(1.0, 1.0),
                        align,
                        text,
                        font.clone(),
                        Color32::WHITE,
                    );
                }
                ui.painter().text(at, align, text, font.clone(), ink);
            };
            write(
                pos2(rect.left() + 22.0, rect.center().y),
                Align2::LEFT_CENTER,
                item.name,
            );
            write(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                &item.keys,
            );
            if item.checked {
                theme::paint_icon(
                    ui.painter(),
                    Rect::from_center_size(
                        pos2(rect.left() + 11.0, rect.center().y),
                        egui::Vec2::splat(12.0),
                    ),
                    "installed",
                    ink,
                );
            }
            if response.clicked() {
                chosen = Some(item.command);
                ui.close();
            }
        }
        // Windows 95's raised edge round the menu.
        if retro {
            let edge = ui.min_rect().expand(3.0);
            let painter = ui.painter().with_clip_rect(edge);
            retro_edge(&painter, edge, colours.face, false, 1.0);
        }
        chosen
    })
    .and_then(|shown| shown.inner)
}

/// Lays the bar out across the top of `full` when the style has one, and
/// returns what's left under it.
pub fn take(app: &mut App, ui: &mut Ui, full: Rect) -> Rect {
    if !shown() {
        return full;
    }
    let band = Rect::from_min_size(full.min, vec2(full.width(), HEIGHT));
    ui.scope_builder(UiBuilder::new().max_rect(band).id_salt("menu-bar"), |ui| {
        bar(app, ui, band);
    });
    Rect::from_min_max(pos2(full.left(), band.bottom()), full.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_read_as_the_platform_writes_them() {
        let written = keys("Q");
        assert!(written == "Ctrl+Q" || written == "⌘Q");
    }
}
