//! The ⋮ at the right end of the toolbar, like File Pilot's: what can be done
//! with the folder on screen. Each row shows its shortcut, and a box when it
//! is a setting that is on or off. Rows Lumen does not have yet are greyed.

use eframe::egui;

use crate::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderMenuEvent {
    /// Toggle Subfolders (Ctrl+E).
    ToggleSubfolders,
    /// The preview panel (Space).
    TogglePreview,
    /// New Tab (Ctrl+T).
    NewTab,
    /// Close Tab (Ctrl+W).
    CloseTab,
    /// Hidden files (Ctrl+H).
    ToggleHidden,
}

/// What the rows show for the folder on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FolderState {
    /// Whether Ctrl+E lists what the subfolders hold; `None` where it cannot
    /// (This PC, the Recycle Bin).
    pub subfolders: Option<bool>,
    pub preview: bool,
    pub hidden: bool,
    /// More than one tab is open: Close Tab has one to close.
    pub can_close_tab: bool,
}

/// One row: its name, shortcut, box (on or off) when it is a setting, and
/// what it does; `None` greys it.
struct Item {
    name: &'static str,
    shortcut: &'static str,
    check: Option<bool>,
    event: Option<FolderMenuEvent>,
}

/// The rows top to bottom, in groups split by a line.
fn groups(state: &FolderState) -> [Vec<Item>; 4] {
    let item = |name, shortcut, check, event| Item { name, shortcut, check, event };
    let subfolders = state.subfolders.map(|_| FolderMenuEvent::ToggleSubfolders);
    let close_tab = state.can_close_tab.then_some(FolderMenuEvent::CloseTab);
    [
        vec![
            item(tr!("Toggle Subfolders"), "Ctrl+E", Some(state.subfolders.unwrap_or(false)), subfolders),
            item(tr!("Preview Panel"), "Space", Some(state.preview), Some(FolderMenuEvent::TogglePreview)),
        ],
        vec![item(tr!("New Tab"), "Ctrl+T", None, Some(FolderMenuEvent::NewTab)), item(tr!("Close Tab"), "Ctrl+W", None, close_tab)],
        vec![item(tr!("Hidden Files"), "Ctrl+H", Some(state.hidden), Some(FolderMenuEvent::ToggleHidden))],
        // Their own card adds them: greyed until then, as the ☰ menu's rows.
        vec![item(tr!("Open Console"), "", None, None), item(tr!("Open File Explorer"), "", None, None)],
    ]
}

pub(crate) fn button_id() -> egui::Id {
    egui::Id::new("lumen_folder_menu")
}

/// The menu, as `Popup::menu` names it.
fn menu_id() -> egui::Id {
    button_id().with("popup")
}

/// Set when the keyboard opened the menu: its first row takes the focus.
fn focus_first_id() -> egui::Id {
    button_id().with("focus_first")
}

pub fn is_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_id_open(ctx, menu_id())
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

/// Three dots one above the other.
fn paint_dots(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let gap = rect.height() * 0.2;
    for step in [-1.0, 0.0, 1.0] {
        painter.circle_filled(rect.center() + egui::vec2(0.0, step * gap), 1.6, color);
    }
}

/// A row: the box slot before the name (empty for a row that is no
/// setting, so the names line up), the shortcut on the right.
fn row(ui: &mut egui::Ui, item: &Item, shortcut: &str) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let slot = egui::Id::new(("lumen_folder_menu_box", item.name));
    let laid = egui::Button::new((egui::Atom::custom(slot, egui::vec2(side, side)), item.name)).shortcut_text(shortcut).atom_ui(ui);
    if let (Some(on), Some(rect)) = (item.check, laid.rect(slot)) {
        let rect = rect.shrink(2.0);
        let color = ui.style().interact(&laid.response).text_color();
        if on {
            ui.painter().rect_filled(rect, 2, crate::ui::theme::accent(ui.visuals()));
            // A tick: no font Lumen ships has a glyph for one.
            let tick = [rect.lerp_inside(egui::vec2(0.2, 0.5)), rect.lerp_inside(egui::vec2(0.42, 0.72)), rect.lerp_inside(egui::vec2(0.8, 0.28))];
            ui.painter().line(tick.to_vec(), egui::Stroke::new(1.5, ui.visuals().strong_text_color()));
        } else {
            ui.painter().rect_stroke(rect, 2, egui::Stroke::new(1.0, color), egui::StrokeKind::Inside);
        }
    }
    laid.response
}

/// The ⋮ button and its menu.
pub fn show(ui: &mut egui::Ui, state: &FolderState) -> Option<FolderMenuEvent> {
    let mut event = None;
    let ctx = ui.ctx().clone();
    let open = is_open(&ctx);
    let button = crate::ui::app_menu::square(ui, button_id(), open, paint_dots).on_hover_text(tr!("Folder options"));
    // Tab took the focus out of the button and its menu: nothing else closes it.
    let menu_layer = egui::LayerId::new(egui::Order::Foreground, menu_id());
    let outside = ctx
        .memory(|m| m.focused())
        .filter(|&id| id != button_id())
        .is_some_and(|id| ctx.read_response(id).is_none_or(|focused| focused.layer_id != menu_layer));
    if outside && open {
        egui::Popup::close_id(&ctx, menu_id());
    }
    // Opened from the keyboard (Enter or Space on the button): the first row
    // takes the focus, so the arrows and Enter work at once.
    let from_keys = button.clicked() && !open && ctx.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space));
    if from_keys {
        ctx.data_mut(|d| d.insert_temp(focus_first_id(), true));
    }
    egui::Popup::menu(&button).id(menu_id()).show(|ui| {
        ui.set_min_width(220.0);
        let focus_first = ctx.data_mut(|d| d.remove_temp::<bool>(focus_first_id())).unwrap_or(false);
        let mut enabled = Vec::new();
        for (n, group) in groups(state).iter().enumerate() {
            if n > 0 {
                ui.separator();
            }
            for item in group {
                // A greyed row's shortcut would do nothing: it shows once the row runs.
                let shortcut = if item.event.is_some() { item.shortcut } else { "" };
                let row = ui.add_enabled_ui(item.event.is_some(), |ui| row(ui, item, shortcut)).inner;
                crate::ui::focus_ring(ui, &row);
                if item.event.is_some() {
                    if focus_first && enabled.is_empty() {
                        row.request_focus();
                    }
                    enabled.push(row.clone());
                }
                if row.clicked() {
                    event = item.event;
                    // A click closes the menu by itself; Enter or Space does not.
                    egui::Popup::close_id(&ctx, menu_id());
                }
            }
        }
        crate::ui::app_menu::step_through(&ctx, &enabled);
    });
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ON: FolderState = FolderState { subfolders: Some(true), preview: true, hidden: true, can_close_tab: true };
    const ALL_OFF: FolderState = FolderState { subfolders: Some(false), preview: false, hidden: false, can_close_tab: true };

    /// One frame of the button at the right of a bar, with `events`; what it
    /// returned, the texts painted and the filled rects.
    fn bar(ctx: &egui::Context, state: &FolderState, events: Vec<egui::Event>) -> (Option<FolderMenuEvent>, Vec<(String, egui::Rect)>, Vec<egui::Rect>) {
        // A tenth of a second a frame: the menu fades in and the hint waits for the pointer to rest.
        let time = ctx.input(|i| i.time) + 0.1;
        let input = egui::RawInput { events, time: Some(time), screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut event = None;
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("toolbar").show(root, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| event = show(ui, state));
            });
        });
        output.textures_delta.clear();
        let accent = crate::ui::theme::accent(&ctx.global_style().visuals);
        let mut texts = Vec::new();
        let mut filled = Vec::new();
        for shape in &output.shapes {
            match &shape.shape {
                egui::Shape::Text(t) => texts.push((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                egui::Shape::Rect(r) if r.fill == accent => filled.push(r.rect),
                _ => {}
            }
        }
        (event, texts, filled)
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

    /// Opens the menu with a click on ⋮; the menu's texts and filled rects.
    fn open(ctx: &egui::Context, state: &FolderState) -> (Vec<(String, egui::Rect)>, Vec<egui::Rect>) {
        bar(ctx, state, vec![]);
        let button = ctx.read_response(button_id()).expect("⋮ drawn").rect;
        assert_eq!(bar(ctx, state, pointer_click(button.center())).0, None, "opening chooses nothing");
        // Until it has faded in, the menu's colors are paler.
        for _ in 0..5 {
            bar(ctx, state, vec![]);
        }
        let (_, texts, filled) = bar(ctx, state, vec![]);
        (texts, filled)
    }

    /// Clicks ⋮, then `row`; what the menu returned.
    fn choose(state: &FolderState, row: &str) -> Option<FolderMenuEvent> {
        let ctx = egui::Context::default();
        let (texts, _) = open(&ctx, state);
        bar(&ctx, state, pointer_click(at(&texts, row).center())).0
    }

    const ROWS: [&str; 7] = ["Toggle Subfolders", "Preview Panel", "New Tab", "Close Tab", "Hidden Files", "Open Console", "Open File Explorer"];

    #[test]
    fn the_menu_lists_its_rows_in_order_with_their_shortcuts_on_the_right() {
        let ctx = egui::Context::default();
        let (texts, _) = open(&ctx, &ALL_OFF);
        let tops: Vec<f32> = ROWS.iter().map(|row| at(&texts, row).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "top to bottom in this order: {tops:?}");
        for (row, shortcut) in [("Toggle Subfolders", "Ctrl+E"), ("Preview Panel", "Space"), ("New Tab", "Ctrl+T"), ("Close Tab", "Ctrl+W"), ("Hidden Files", "Ctrl+H")] {
            let (row, shortcut) = (at(&texts, row), at(&texts, shortcut));
            assert!(shortcut.left() > row.right() && (shortcut.center().y - row.center().y).abs() < 1.0, "{shortcut:?} on its row's right");
        }
    }

    #[test]
    fn each_setting_row_shows_a_box_filled_while_it_is_on() {
        let boxed = |texts: &[(String, egui::Rect)], filled: &[egui::Rect], row: &str| {
            let name = at(texts, row);
            filled.iter().any(|r| r.right() <= name.left() && r.right() > name.left() - name.height() * 2.0 && r.center().y > name.top() && r.center().y < name.bottom())
        };
        let ctx = egui::Context::default();
        let (texts, filled) = open(&ctx, &ALL_ON);
        for row in ["Toggle Subfolders", "Preview Panel", "Hidden Files"] {
            assert!(boxed(&texts, &filled, row), "{row} is on: its box is filled");
        }
        for row in ["New Tab", "Close Tab"] {
            assert!(!boxed(&texts, &filled, row), "{row} is no setting: no box");
        }
        let ctx = egui::Context::default();
        let (texts, filled) = open(&ctx, &ALL_OFF);
        for row in ["Toggle Subfolders", "Preview Panel", "Hidden Files"] {
            assert!(!boxed(&texts, &filled, row), "{row} is off: its box is empty");
        }
    }

    #[test]
    fn each_row_lumen_has_is_its_event() {
        for (row, event) in [
            ("Toggle Subfolders", FolderMenuEvent::ToggleSubfolders),
            ("Preview Panel", FolderMenuEvent::TogglePreview),
            ("New Tab", FolderMenuEvent::NewTab),
            ("Close Tab", FolderMenuEvent::CloseTab),
            ("Hidden Files", FolderMenuEvent::ToggleHidden),
        ] {
            assert_eq!(choose(&ALL_OFF, row), Some(event), "{row}");
        }
    }

    #[test]
    fn rows_that_cannot_run_are_greyed_without_their_shortcut() {
        for row in ["Open Console", "Open File Explorer"] {
            assert_eq!(choose(&ALL_OFF, row), None, "{row} does nothing yet");
        }
        let one_tab = FolderState { can_close_tab: false, ..ALL_OFF };
        assert_eq!(choose(&one_tab, "Close Tab"), None, "the last tab stays");
        let ctx = egui::Context::default();
        let (texts, _) = open(&ctx, &one_tab);
        assert!(!texts.iter().any(|(t, _)| t == "Ctrl+W"), "a greyed row shows no shortcut");
        let on_this_pc = FolderState { subfolders: None, ..ALL_OFF };
        assert_eq!(choose(&on_this_pc, "Toggle Subfolders"), None, "This PC has no subfolders to list");
    }

    #[test]
    fn the_arrows_move_through_the_rows_and_escape_closes_the_menu() {
        let ctx = egui::Context::default();
        bar(&ctx, &ALL_OFF, vec![]);
        // Enter on the focused button opens the menu on its first row.
        ctx.memory_mut(|m| m.request_focus(button_id()));
        bar(&ctx, &ALL_OFF, vec![]);
        assert_eq!(bar(&ctx, &ALL_OFF, key(egui::Key::Enter)).0, None, "opening chooses nothing");
        bar(&ctx, &ALL_OFF, vec![]);
        assert!(is_open(&ctx));
        for arrow in [egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::ArrowUp] {
            bar(&ctx, &ALL_OFF, key(arrow));
            bar(&ctx, &ALL_OFF, vec![]);
            assert!(is_open(&ctx), "{arrow:?} leaves the menu open");
        }
        assert_eq!(bar(&ctx, &ALL_OFF, key(egui::Key::Enter)).0, Some(FolderMenuEvent::TogglePreview), "Down, Down, Up from the first row");
        open(&ctx, &ALL_OFF);
        let escape = egui::RawInput { events: key(egui::Key::Escape), screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut left_for_the_app = true;
        let _ = ctx.run_ui(escape, |root| {
            assert!(close_on_escape(root.ctx()), "it was open");
            left_for_the_app = root.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        });
        assert!(!left_for_the_app, "that Escape is used up");
        assert!(!is_open(&ctx));
        let escape = egui::RawInput { events: key(egui::Key::Escape), screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let _ = ctx.run_ui(escape, |root| {
            assert!(!close_on_escape(root.ctx()), "closed: nothing to do");
            left_for_the_app = root.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        });
        assert!(left_for_the_app, "with the menu closed Escape is the app's");
    }

    #[test]
    fn hovering_the_button_names_it() {
        let ctx = egui::Context::default();
        bar(&ctx, &ALL_OFF, vec![]);
        let button = ctx.read_response(button_id()).expect("⋮ drawn").rect;
        let mut named = false;
        bar(&ctx, &ALL_OFF, vec![egui::Event::PointerMoved(button.center())]);
        // egui shows a hint once the pointer rests half a second.
        for _ in 0..10 {
            let (_, texts, _) = bar(&ctx, &ALL_OFF, vec![]);
            named |= texts.iter().any(|(t, _)| t == "Folder options");
        }
        assert!(named, "the hint shows");
    }
}
