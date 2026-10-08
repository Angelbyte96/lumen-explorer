//! Draws the right-click menu (`menu::Menu`) at the pointer: a search box on
//! top, like File Pilot's "Select a command", and the items below with their
//! shortcuts. Typing filters, Up/Down move, Right/Left open and close a
//! submenu, Enter or a click runs, Escape or a click outside closes.

use eframe::egui;

use crate::menu::{Command, Item, Key, Kind, Menu};
use crate::ui::path_popup::marked_name;

/// Width of the menu, search box included.
const WIDTH: f32 = 300.0;
/// Lines shown before the list scrolls.
const VISIBLE_ROWS: f32 = 18.0;
/// Marks a closed / open submenu.
pub const CLOSED: &str = "⏵";
pub const OPEN: &str = "⏷";

pub struct ContextMenu {
    menu: Menu,
    /// Top-left corner: where the right click was.
    pos: egui::Pos2,
}

/// What the open menu did this frame.
#[derive(Debug, PartialEq, Eq)]
pub enum MenuEvent {
    Run(Command),
    Closed,
}

impl ContextMenu {
    pub fn new(items: Vec<Item>, pos: egui::Pos2) -> Self {
        Self { menu: Menu::new(items), pos }
    }

    #[cfg(test)]
    pub fn items(&self) -> &[Item] {
        &self.menu.items
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<MenuEvent> {
        if let Some(event) = self.read_keys(ctx) {
            return Some(event);
        }
        let mut event = None;
        let area = egui::Area::new(egui::Id::new("lumen_context_menu_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(self.pos)
            .constrain(true);
        let modal = egui::Modal::new(egui::Id::new("lumen_context_menu"))
            .area(area)
            .frame(egui::Frame::popup(&ctx.global_style()))
            .backdrop_color(egui::Color32::TRANSPARENT)
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    crate::ui::icons::magnifier(ui);
                    let mut query = self.menu.query.clone();
                    let field = egui::TextEdit::singleline(&mut query)
                        .id(field_id())
                        .hint_text("Select a command…")
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE)
                        .show(ui);
                    if !field.response.has_focus() {
                        field.response.request_focus();
                    }
                    if field.response.changed() {
                        self.menu.set_query(query);
                    }
                });
                ui.separator();
                let row_height = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical()
                    .max_height(VISIBLE_ROWS * row_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if let Some(command) = self.rows(ui) {
                            event = Some(MenuEvent::Run(command));
                        }
                    });
            });
        if event.is_none() && modal.should_close() {
            event = Some(MenuEvent::Closed);
        }
        event
    }

    /// Arrows, Enter and Escape, read before the search box sees them. Left
    /// and Right stay with the box while it holds text.
    fn read_keys(&mut self, ctx: &egui::Context) -> Option<MenuEvent> {
        use egui::{Key as K, Modifiers as M};
        let mut bindings = vec![(K::ArrowUp, Key::Up), (K::ArrowDown, Key::Down), (K::Enter, Key::Enter)];
        if self.menu.query.is_empty() {
            bindings.extend([(K::ArrowLeft, Key::Left), (K::ArrowRight, Key::Right)]);
        }
        // In the order pressed: Down, Right, Down is not Down, Down, Right.
        let (escape, keys) = ctx.input_mut(|i| {
            let escape = i.consume_key(M::NONE, K::Escape);
            let mut keys = Vec::new();
            i.events.retain(|event| match event {
                egui::Event::Key { key, pressed: true, modifiers, .. } if modifiers.is_none() => {
                    match bindings.iter().find(|(bound, _)| bound == key) {
                        Some(&(_, menu_key)) => {
                            keys.push(menu_key);
                            false
                        }
                        None => true,
                    }
                }
                _ => true,
            });
            (escape, keys)
        });
        if escape {
            return Some(MenuEvent::Closed);
        }
        keys.into_iter().find_map(|key| self.menu.key(key)).map(MenuEvent::Run)
    }

    /// Draws the lines; a click on one returns its command.
    fn rows(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let mut clicked = None;
        let cursor = self.menu.cursor();
        let font = egui::TextStyle::Body.resolve(ui.style());
        for (index, row) in self.menu.rows().into_iter().enumerate() {
            let item = self.menu.item(&row.at);
            if item.kind == Kind::Separator {
                ui.separator();
                continue;
            }
            let visuals = ui.visuals();
            let color = if item.enabled { visuals.text_color() } else { visuals.weak_text_color() };
            let job = marked_name(&row.label, &row.marks, color, visuals.hyperlink_color, font.clone());
            let fill = if cursor == Some(index) { visuals.selection.bg_fill } else { egui::Color32::TRANSPARENT };
            let marker = match &item.kind {
                Kind::Sub(_) if self.menu.rows().get(index + 1).is_some_and(|next| next.depth > row.depth) => OPEN,
                Kind::Sub(_) => CLOSED,
                _ => "",
            };
            let shortcut = item.shortcut.unwrap_or(marker);
            let line = egui::UiBuilder::new().id_salt(("lumen_menu_row", index)).sense(egui::Sense::click());
            let response = ui
                .scope_builder(line, |ui| {
                    egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add_space(row.depth as f32 * 16.0);
                            ui.add(egui::Label::new(job).selectable(false).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(egui::RichText::new(shortcut).weak()).selectable(false));
                            });
                        });
                    });
                })
                .response;
            if cursor == Some(index) {
                response.scroll_to_me(None);
            }
            ui.ctx().data_mut(|d| d.insert_temp(row_id(index), response.rect));
            if response.clicked() {
                clicked = Some(index);
            }
        }
        clicked.and_then(|row| self.menu.click(row))
    }
}

fn field_id() -> egui::Id {
    egui::Id::new("lumen_context_menu_field")
}

/// Where row `index` was last drawn (for tests).
pub fn row_id(index: usize) -> egui::Id {
    egui::Id::new(("lumen_menu_row_rect", index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SortColumn;
    use crate::ui::testing::{click, frame};
    use eframe::egui::{Event, Key as K, Modifiers};

    fn sample() -> ContextMenu {
        ContextMenu::new(
            vec![
                Item::run("Open", Some("Enter"), true, Command::Open),
                Item::sub("Sort by", vec![Item::run("Size", None, true, Command::Sort(SortColumn::Size))]),
            ],
            egui::pos2(100.0, 100.0),
        )
    }

    fn step(ctx: &egui::Context, menu: &mut ContextMenu, events: Vec<Event>) -> Option<MenuEvent> {
        frame(ctx, events, |ui| menu.show(ui.ctx()))
    }

    fn key(key: K) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    fn row_rect(ctx: &egui::Context, index: usize) -> egui::Rect {
        ctx.data(|d| d.get_temp::<egui::Rect>(row_id(index))).expect("drawn")
    }

    #[test]
    fn a_click_on_an_item_runs_it_and_on_a_submenu_opens_it() {
        let ctx = egui::Context::default();
        let mut menu = sample();
        step(&ctx, &mut menu, vec![]);
        let sort = row_rect(&ctx, 1).center();
        assert_eq!(click(&ctx, sort, |ui| menu.show(ui.ctx())), None);
        step(&ctx, &mut menu, vec![]);
        let size = row_rect(&ctx, 2).center();
        assert_eq!(click(&ctx, size, |ui| menu.show(ui.ctx())), Some(MenuEvent::Run(Command::Sort(SortColumn::Size))));
    }

    #[test]
    fn typing_searches_and_enter_runs_the_first_match() {
        let ctx = egui::Context::default();
        let mut menu = sample();
        step(&ctx, &mut menu, vec![]);
        step(&ctx, &mut menu, vec![Event::Text("siz".into())]);
        assert_eq!(menu.menu.query, "siz");
        assert_eq!(step(&ctx, &mut menu, vec![key(K::Enter)]), Some(MenuEvent::Run(Command::Sort(SortColumn::Size))));
    }

    #[test]
    fn escape_or_a_click_outside_closes_it() {
        let ctx = egui::Context::default();
        let mut menu = sample();
        step(&ctx, &mut menu, vec![]);
        assert_eq!(step(&ctx, &mut menu, vec![key(K::Escape)]), Some(MenuEvent::Closed));
        let mut menu = sample();
        step(&ctx, &mut menu, vec![]);
        assert_eq!(click(&ctx, egui::pos2(700.0, 550.0), |ui| menu.show(ui.ctx())), Some(MenuEvent::Closed));
    }

    #[test]
    fn the_arrows_walk_the_items() {
        let ctx = egui::Context::default();
        let mut menu = sample();
        step(&ctx, &mut menu, vec![]);
        step(&ctx, &mut menu, vec![key(K::ArrowDown), key(K::ArrowDown), key(K::ArrowRight), key(K::ArrowDown)]);
        assert_eq!(step(&ctx, &mut menu, vec![key(K::Enter)]), Some(MenuEvent::Run(Command::Sort(SortColumn::Size))));
    }
}
