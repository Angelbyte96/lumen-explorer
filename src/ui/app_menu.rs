//! The window's top-left corner, like File Pilot's: ☰ opens the app menu
//! (F10) and the panel button beside it shows or hides the side panel
//! (Ctrl+B). Hovering either names its command and its shortcut.

use eframe::egui;

use crate::menu::MenuIcon;

pub const MENU: &str = "☰";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMenuEvent {
    /// The panel button (Ctrl+B): show or hide the side panel.
    ToggleSidebar,
    /// Go To: the path box (Ctrl+L).
    GoTo,
    /// Lumen Setup: the install and integrations screen.
    Setup,
    /// Options: the window with the color schemes (Ctrl+,).
    Options,
}

/// One row of the menu: its icon, name and shortcut, and what it does once
/// Lumen has it. File Pilot's other rows are kept, greyed and without their
/// shortcut, until then.
type Item = (MenuIcon, &'static str, &'static str, Option<AppMenuEvent>);

/// The menu's rows top to bottom, in groups split by a line.
const GROUPS: [&[Item]; 3] = [
    &[
        (MenuIcon::GoTo, "Go To", "Ctrl+L", Some(AppMenuEvent::GoTo)),
        (MenuIcon::Commands, "Commands", "Ctrl+Shift+P", None),
    ],
    &[
        (MenuIcon::LoadLayout, "Load Layout", "", None),
        (MenuIcon::SaveLayout, "Save Layout", "", None),
        (MenuIcon::Options, "Options", "Ctrl+,", Some(AppMenuEvent::Options)),
    ],
    &[(MenuIcon::Setup, "Lumen Setup", "", Some(AppMenuEvent::Setup))],
];

/// The line under the rows.
pub fn version() -> String {
    format!("LUMEN v{}", env!("CARGO_PKG_VERSION"))
}

pub(crate) fn button_id() -> egui::Id {
    egui::Id::new("lumen_app_menu")
}

/// The panel button beside ☰.
pub(crate) fn panel_button_id() -> egui::Id {
    button_id().with("side_panel")
}

/// The menu, as `Popup::menu` names it.
fn menu_id() -> egui::Id {
    button_id().with("popup")
}

/// Set by `open`: the first row takes the focus, so Enter runs it.
fn focus_first_id() -> egui::Id {
    button_id().with("focus_first")
}

pub fn is_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_id_open(ctx, menu_id())
}

/// F10: opens the menu with its first row focused, or closes it.
/// ↑/↓ then move through the rows Lumen has, round and round.
pub fn toggle(ctx: &egui::Context) {
    if is_open(ctx) {
        egui::Popup::close_id(ctx, menu_id());
    } else {
        egui::Popup::open_id(ctx, menu_id());
        // The focus may be elsewhere (the side panel): bring it to ☰ first, or
        // the menu would close at once for the focus being outside it.
        ctx.memory_mut(|m| m.request_focus(button_id()));
        ctx.data_mut(|d| d.insert_temp(focus_first_id(), true));
    }
}

/// Closes the menu, if open.
pub fn close(ctx: &egui::Context) {
    egui::Popup::close_id(ctx, menu_id());
}

/// Closes the menu on Escape and uses that Escape up; false when the menu was
/// closed. Runs before the app reads its keys, like the lock's.
pub fn close_on_escape(ctx: &egui::Context) -> bool {
    if !is_open(ctx) || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return false;
    }
    egui::Popup::close_id(ctx, menu_id());
    true
}

/// Hovering names the command and its shortcut: "Cmd: Menu [F10]".
pub fn command_hint(response: egui::Response, name: &str, shortcut: &str) -> egui::Response {
    response.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label("Cmd:");
            ui.weak(name);
            egui::Frame::new()
                .fill(ui.visuals().widgets.inactive.bg_fill)
                .corner_radius(3)
                .inner_margin(egui::Margin::symmetric(4, 1))
                .show(ui, |ui| ui.label(shortcut));
        });
    })
}

/// A square button with a known id that paints itself: the hover fill, then `paint`.
fn square(ui: &mut egui::Ui, id: egui::Id, lit: bool, paint: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32)) -> egui::Response {
    let side = ui.spacing().interact_size.y + 4.0;
    let (_, rect) = ui.allocate_space(egui::vec2(side, side));
    let response = ui.interact(rect, id, egui::Sense::click());
    if response.hovered() || lit {
        ui.painter().rect_filled(rect, 4, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let color = if response.hovered() || lit { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    paint(ui.painter(), rect, color);
    crate::ui::focus_ring(ui, &response);
    response
}

/// A menu row: the icon in a slot before the name, the shortcut on the right.
/// A greyed row's painter is faded, so its icon greys with the name.
fn row(ui: &mut egui::Ui, icon: MenuIcon, name: &str, shortcut: &str) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let slot = egui::Id::new(("lumen_app_menu_icon", name));
    let laid = egui::Button::new((egui::Atom::custom(slot, egui::vec2(side, side)), name)).shortcut_text(shortcut).atom_ui(ui);
    if let Some(rect) = laid.rect(slot) {
        let color = ui.style().interact(&laid.response).text_color();
        crate::ui::icons::menu_icon(ui.painter(), rect.shrink(1.0), icon, color);
    }
    laid.response
}

/// While a row has the focus, ↑/↓ move it to the next enabled row, wrapping
/// around, and no arrow takes it out: egui would hand it to the nearest widget
/// that way (the side panel), and the menu would close for losing it.
fn step_through(ctx: &egui::Context, rows: &[egui::Response]) {
    let Some(at) = rows.iter().position(egui::Response::has_focus) else { return };
    let arrows = [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::ArrowLeft, egui::Key::ArrowRight];
    if ctx.input(|i| arrows.iter().any(|&k| i.key_pressed(k))) {
        // egui moves the focus at the end of the frame: cancel that move (not Tab's).
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    }
    let step = ctx.input_mut(|i| {
        let down = i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
        let up = i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
        down as isize - up as isize
    });
    if step != 0 {
        let next = (at as isize + step).rem_euclid(rows.len() as isize) as usize;
        rows[next].request_focus();
    }
}

/// Hovering a greyed Lumen Setup says why.
const SETUP_HELD: &str = "Lumen Setup returns once Lumen is code-signed";

/// The ☰ and the panel button, side by side; `setup` is whether the Lumen
/// Setup row can open its screen (greyed when not).
pub fn show(ui: &mut egui::Ui, setup: bool) -> Option<AppMenuEvent> {
    let mut event = None;
    let ctx = ui.ctx().clone();
    let open = is_open(&ctx);
    let menu = square(ui, button_id(), open, |painter, rect, color| {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, MENU, egui::FontId::proportional(16.0), color);
    });
    let menu = command_hint(menu, "Menu", "F10");
    // Tab took the focus out of the button and its menu: nothing else closes it.
    let menu_layer = egui::LayerId::new(egui::Order::Foreground, menu_id());
    let outside = ctx
        .memory(|m| m.focused())
        .filter(|&id| id != button_id())
        .is_some_and(|id| ctx.read_response(id).is_none_or(|focused| focused.layer_id != menu_layer));
    if outside && open {
        egui::Popup::close_id(&ctx, menu_id());
    }
    egui::Popup::menu(&menu).show(|ui| {
        ui.set_min_width(220.0);
        let focus_first = ctx.data_mut(|d| d.remove_temp::<bool>(focus_first_id())).unwrap_or(false);
        let mut enabled = Vec::new();
        for (n, group) in GROUPS.iter().enumerate() {
            if n > 0 {
                ui.separator();
            }
            for &(icon, name, shortcut, on_click) in *group {
                let held = on_click == Some(AppMenuEvent::Setup) && !setup;
                let on_click = if held { None } else { on_click };
                // A greyed row's shortcut would do nothing yet: it shows once the command exists.
                let shortcut = if on_click.is_some() { shortcut } else { "" };
                let mut row = ui.add_enabled_ui(on_click.is_some(), |ui| row(ui, icon, name, shortcut)).inner;
                if held {
                    row = row.on_disabled_hover_text(SETUP_HELD);
                }
                if focus_first && n == 0 && name == GROUPS[0][0].1 {
                    row.request_focus();
                }
                crate::ui::focus_ring(ui, &row);
                if on_click.is_some() {
                    enabled.push(row.clone());
                }
                if row.clicked() {
                    event = on_click;
                    // A click closes the menu by itself; Enter or Space does not.
                    egui::Popup::close_id(&ctx, menu_id());
                }
            }
        }
        ui.separator();
        ui.weak(version());
        step_through(&ctx, &enabled);
    });
    let panel = square(ui, panel_button_id(), false, crate::ui::icons::paint_side_panel);
    if command_hint(panel, "Toggle Side Panel", "Ctrl+B").clicked() {
        event = Some(AppMenuEvent::ToggleSidebar);
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of the corner with `events`; what it returned and the texts painted.
    fn corner(ctx: &egui::Context, events: Vec<egui::Event>) -> (Option<AppMenuEvent>, Vec<(String, egui::Rect)>) {
        corner_with(ctx, events, true)
    }

    /// `corner`, with the Setup row enabled or not.
    fn corner_with(ctx: &egui::Context, events: Vec<egui::Event>, setup: bool) -> (Option<AppMenuEvent>, Vec<(String, egui::Rect)>) {
        let input = egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut event = None;
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("tabs").show(root, |ui| {
                ui.horizontal(|ui| event = show(ui, setup));
            });
            // Something below the menu that can take the focus, like the side panel.
            egui::CentralPanel::default().show(root, |ui| {
                ui.add_space(200.0);
                let other = ui.button("other part").id;
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("other part"), other));
            });
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect();
        (event, texts)
    }

    fn at(texts: &[(String, egui::Rect)], text: &str) -> egui::Rect {
        texts.iter().find(|(t, _)| t == text).map(|(_, r)| *r).unwrap_or_else(|| panic!("{text:?} not painted: {texts:?}"))
    }

    fn pointer_click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    fn key(key: egui::Key) -> Vec<egui::Event> {
        let event = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        vec![event(true), event(false)]
    }

    /// Clicks ☰, then `item` in its menu; returns what the corner said and the menu's texts.
    fn choose(item: &str) -> (Option<AppMenuEvent>, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        let (_, texts) = corner(&ctx, vec![]);
        assert_eq!(corner(&ctx, pointer_click(at(&texts, MENU).center())).0, None, "opening chooses nothing");
        let (_, menu) = corner(&ctx, vec![]);
        let (event, _) = corner(&ctx, pointer_click(at(&menu, item).center()));
        (event, menu)
    }

    #[test]
    fn the_menu_lists_file_pilots_rows_with_their_shortcuts_and_the_version() {
        let (_, menu) = choose("Go To");
        let rows = ["Go To", "Commands", "Load Layout", "Save Layout", "Options", "Lumen Setup", &version()];
        let tops: Vec<f32> = rows.iter().map(|row| at(&menu, row).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "top to bottom in this order: {tops:?}");
        for (row, shortcut) in [("Go To", "Ctrl+L"), ("Options", "Ctrl+,")] {
            let (row, shortcut) = (at(&menu, row), at(&menu, shortcut));
            assert!(shortcut.left() > row.right() && (shortcut.center().y - row.center().y).abs() < 1.0, "{shortcut:?} on its row's right");
        }
        // A greyed row's shortcut would do nothing yet: it shows once the command exists.
        assert!(!menu.iter().any(|(text, _)| text == "Ctrl+Shift+P"), "Commands is greyed: no shortcut");
        assert_eq!(version(), format!("LUMEN v{}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn every_row_draws_its_icon_left_of_its_name() {
        let ctx = egui::Context::default();
        let (_, texts) = corner(&ctx, vec![]);
        corner(&ctx, pointer_click(at(&texts, MENU).center()));
        let input = egui::RawInput { screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("tabs").show(root, |ui| {
                ui.horizontal(|ui| show(ui, true));
            });
        });
        output.textures_delta.clear();
        let mut names = Vec::new();
        let mut strokes = Vec::new();
        for shape in &output.shapes {
            match &shape.shape {
                egui::Shape::Text(t) => names.push((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                egui::Shape::Path(_) | egui::Shape::LineSegment { .. } | egui::Shape::Circle(_) | egui::Shape::Rect(_) => strokes.push(shape.shape.visual_bounding_rect()),
                _ => {}
            }
        }
        for row in ["Go To", "Commands", "Load Layout", "Save Layout", "Options", "Lumen Setup"] {
            let name = at(&names, row);
            let icon = strokes.iter().any(|r| r.width() < name.height() * 1.5 && r.right() <= name.left() && r.right() > name.left() - name.height() * 2.0 && r.center().y > name.top() && r.center().y < name.bottom());
            assert!(icon, "{row} has an icon on its left");
        }
    }

    #[test]
    fn the_arrows_keep_the_focus_in_the_menu() {
        let ctx = egui::Context::default();
        corner(&ctx, vec![]);
        toggle(&ctx);
        corner(&ctx, vec![]);
        for arrow in [egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::ArrowRight, egui::Key::ArrowLeft] {
            corner(&ctx, key(arrow));
            corner(&ctx, vec![]);
            assert!(is_open(&ctx), "{arrow:?} leaves the menu open");
        }
        // Down, Down, Up from Go To (over the three rows Lumen has: Go To, Options, Lumen Setup) ends on Options.
        assert_eq!(corner(&ctx, key(egui::Key::Enter)).0, Some(AppMenuEvent::Options), "the rows kept the focus");
    }

    #[test]
    fn tab_still_takes_the_focus_out_of_the_menu_and_closes_it() {
        let ctx = egui::Context::default();
        corner(&ctx, vec![]);
        toggle(&ctx);
        corner(&ctx, vec![]);
        // The first two Tabs go to the next rows, Options and Lumen Setup, still in the menu.
        for _ in 0..2 {
            corner(&ctx, key(egui::Key::Tab));
            corner(&ctx, vec![]);
            assert!(is_open(&ctx), "the next row is in the menu");
        }
        corner(&ctx, key(egui::Key::Tab));
        corner(&ctx, vec![]);
        assert!(!is_open(&ctx), "the focus left the menu, so it closed");
    }

    #[test]
    fn f10_opens_the_menu_while_another_part_has_the_focus() {
        let ctx = egui::Context::default();
        corner(&ctx, vec![]);
        let other = ctx.data(|d| d.get_temp::<egui::Id>(egui::Id::new("other part"))).expect("drawn");
        ctx.memory_mut(|m| m.request_focus(other));
        corner(&ctx, vec![]);
        assert!(ctx.memory(|m| m.focused()).is_some_and(|id| id != button_id()), "another part has the focus");
        toggle(&ctx);
        for frame in 0..3 {
            corner(&ctx, vec![]);
            assert!(is_open(&ctx), "still open on frame {frame}");
        }
        assert_eq!(corner(&ctx, key(egui::Key::Enter)).0, Some(AppMenuEvent::GoTo));
    }

    #[test]
    fn a_click_opens_the_menu_while_another_part_has_the_focus() {
        let ctx = egui::Context::default();
        let (_, texts) = corner(&ctx, vec![]);
        let other = ctx.data(|d| d.get_temp::<egui::Id>(egui::Id::new("other part"))).expect("drawn");
        ctx.memory_mut(|m| m.request_focus(other));
        corner(&ctx, vec![]);
        corner(&ctx, pointer_click(at(&texts, MENU).center()));
        for frame in 0..3 {
            corner(&ctx, vec![]);
            assert!(is_open(&ctx), "still open on frame {frame}");
        }
    }

    #[test]
    fn go_to_options_and_setup_are_events_and_the_rows_lumen_lacks_are_greyed() {
        assert_eq!(choose("Go To").0, Some(AppMenuEvent::GoTo));
        assert_eq!(choose("Options").0, Some(AppMenuEvent::Options));
        assert_eq!(choose("Lumen Setup").0, Some(AppMenuEvent::Setup));
        for row in ["Commands", "Load Layout", "Save Layout"] {
            assert_eq!(choose(row).0, None, "{row} does nothing yet");
        }
    }

    #[test]
    fn the_setup_row_is_greyed_when_setup_cannot_open() {
        let ctx = egui::Context::default();
        let (_, texts) = corner_with(&ctx, vec![], false);
        corner_with(&ctx, pointer_click(at(&texts, MENU).center()), false);
        let (_, menu) = corner_with(&ctx, vec![], false);
        let (event, _) = corner_with(&ctx, pointer_click(at(&menu, "Lumen Setup").center()), false);
        assert_eq!(event, None, "a greyed Lumen Setup does nothing");
    }

    #[test]
    fn the_panel_button_sits_after_the_menu_and_toggles_the_side_panel() {
        let ctx = egui::Context::default();
        let (_, texts) = corner(&ctx, vec![]);
        let menu = ctx.read_response(button_id()).expect("☰ drawn").rect;
        let panel = ctx.read_response(panel_button_id()).expect("panel button drawn").rect;
        assert!(panel.left() >= menu.right(), "beside ☰, on its right");
        assert!(at(&texts, MENU).center().x < panel.left());
        assert_eq!(corner(&ctx, pointer_click(panel.center())).0, Some(AppMenuEvent::ToggleSidebar));
        assert!(!is_open(&ctx), "the menu stays closed");
    }

    #[test]
    fn f10_opens_the_menu_on_go_to_so_enter_runs_it_and_escape_closes_it() {
        let ctx = egui::Context::default();
        corner(&ctx, vec![]);
        toggle(&ctx);
        corner(&ctx, vec![]);
        assert!(is_open(&ctx));
        assert_eq!(corner(&ctx, key(egui::Key::Enter)).0, Some(AppMenuEvent::GoTo), "Go To had the focus");
        assert!(!is_open(&ctx), "running a row closes the menu");
        toggle(&ctx);
        corner(&ctx, vec![]);
        let escape = egui::RawInput { events: key(egui::Key::Escape), screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut left_for_the_app = true;
        let _ = ctx.run_ui(escape, |root| {
            assert!(close_on_escape(root.ctx()), "it was open");
            left_for_the_app = root.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        });
        assert!(!left_for_the_app, "that Escape is used up");
        assert!(!is_open(&ctx));
        toggle(&ctx);
        toggle(&ctx);
        assert!(!is_open(&ctx), "F10 again closes it");
    }
}
