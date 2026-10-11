//! Draws the right-click menu (`menu::Menu`) at the pointer: a search box on
//! top, like File Pilot's "Select a command", and the items below with their
//! shortcuts. Typing filters, Up/Down move, Right/Left open and close a
//! submenu, Enter or a click runs, Escape or a click outside closes.

use eframe::egui;

use crate::menu::{Command, Item, Key, Kind, Menu};
use crate::ui::path_popup::marked_name;

/// Width of the menu, search box included.
const WIDTH: f32 = 340.0;
/// The widest the menu grows for long items; longer ones are cut.
const MAX_WIDTH: f32 = 640.0;
/// Lines shown before the list scrolls.
const VISIBLE_ROWS: f32 = 18.0;
/// Marks a closed / open submenu.
pub const CLOSED: &str = "⏵";
pub const OPEN: &str = "⏷";

pub struct ContextMenu {
    menu: Menu,
    /// Top-left corner: where the right click was.
    pos: egui::Pos2,
    /// Height of the rows as last laid out; the list is that tall up to a limit.
    content: Option<f32>,
    /// Items' own pictures, uploaded once, by the item's place in the menu.
    textures: std::collections::HashMap<Vec<usize>, egui::TextureHandle>,
    /// Where the pointer was last frame (`None` before the first one): a
    /// row takes the highlight only when the pointer moves onto it.
    pointer: Option<Option<egui::Pos2>>,
}

/// What the open menu did this frame.
#[derive(Debug, PartialEq, Eq)]
pub enum MenuEvent {
    Run(Command),
    Closed,
}

impl ContextMenu {
    pub fn new(items: Vec<Item>, pos: egui::Pos2) -> Self {
        Self { menu: Menu::new(items), pos, content: None, textures: std::collections::HashMap::new(), pointer: None }
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
                let width = self.width(ui);
                ui.set_width(width);
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
                // egui's default minimum is 64 points while the area's height is
                // unknown, which showed three lines: as tall as the rows, up to a limit.
                let limit = VISIBLE_ROWS * row_height;
                let tall = self.content.map_or(limit, |h| h.min(limit));
                let list = egui::ScrollArea::vertical()
                    .max_height(limit)
                    .min_scrolled_height(tall)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if let Some(command) = self.rows(ui) {
                            event = Some(MenuEvent::Run(command));
                        }
                    });
                ui.ctx().data_mut(|d| d.insert_temp(list_id(), list.inner_rect));
                let measured = list.content_size.y;
                if self.content.is_none_or(|h| (h - measured).abs() > 0.5) {
                    // The rows changed height (first frame, a search, a submenu): lay out again.
                    self.content = Some(measured);
                    ui.ctx().request_discard("context menu height");
                }
            });
        if event.is_none() && modal.should_close() {
            event = Some(MenuEvent::Closed);
        }
        event
    }

    /// Arrows, Home, End, Page Up, Page Down, Enter and Escape, read before
    /// the search box sees them. Left, Right, Home and End stay with the box
    /// while it holds text.
    fn read_keys(&mut self, ctx: &egui::Context) -> Option<MenuEvent> {
        use egui::{Key as K, Modifiers as M};
        let mut bindings = vec![
            (K::ArrowUp, Key::Up),
            (K::ArrowDown, Key::Down),
            (K::PageUp, Key::PageUp),
            (K::PageDown, Key::PageDown),
            (K::Enter, Key::Enter),
        ];
        if self.menu.query.is_empty() {
            bindings.extend([(K::ArrowLeft, Key::Left), (K::ArrowRight, Key::Right), (K::Home, Key::Home), (K::End, Key::End)]);
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

    /// As wide as the widest line shown (indent, icon, label, shortcut), at
    /// least [`WIDTH`] and at most [`MAX_WIDTH`] or the screen.
    fn width(&self, ui: &egui::Ui) -> f32 {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let side = ui.text_style_height(&egui::TextStyle::Body);
        let spacing = ui.spacing().item_spacing.x;
        let measure = |text: &str| ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE).size().x);
        let widest = self
            .menu
            .rows()
            .iter()
            .map(|row| {
                let item = self.menu.item(&row.at);
                let shortcut = item.shortcut.map_or(measure(CLOSED), measure);
                // Row margin, indent, icon column, its gap, the label, a gap, the shortcut.
                8.0 + row.depth as f32 * 16.0 + side + 4.0 + measure(&row.label) + 3.0 * spacing + 16.0 + shortcut
            })
            .fold(WIDTH, f32::max);
        let screen = ui.ctx().content_rect().width() - 32.0;
        widest.min(MAX_WIDTH).min(screen).max(WIDTH.min(screen))
    }

    /// Draws the lines; a click on one returns its command.
    fn rows(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let mut clicked = None;
        let mut hovered = None;
        // Only a moving pointer takes the highlight: one resting on a row
        // (where the right click was, or after the arrow keys) must not.
        let now = ui.input(|i| i.pointer.latest_pos());
        let moving = self.pointer.is_some_and(|before| before != now);
        self.pointer = Some(now);
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
                            // A fixed column, so labels line up with or without an icon.
                            let side = ui.text_style_height(&egui::TextStyle::Body);
                            let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                            if let Some(picture) = &item.picture {
                                let texture = self.textures.entry(row.at.clone()).or_insert_with(|| {
                                    let image = egui::ColorImage::from_rgba_premultiplied(picture.size, &picture.rgba);
                                    ui.ctx().load_texture(format!("lumen_menu_picture_{:?}", row.at), image, egui::TextureOptions::LINEAR)
                                });
                                let tint = if item.enabled { egui::Color32::WHITE } else { egui::Color32::from_white_alpha(110) };
                                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                                ui.painter().image(texture.id(), icon_rect.shrink(1.0), uv, tint);
                            } else if let Some(icon) = item.icon {
                                crate::ui::icons::menu_icon(ui.painter(), icon_rect.shrink(1.0), icon, color);
                            }
                            ui.add_space(4.0);
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
            if moving && response.hovered() && cursor != Some(index) {
                hovered = Some(index);
            }
            ui.ctx().data_mut(|d| d.insert_temp(row_id(index), response.rect));
            if response.clicked() {
                clicked = Some(index);
            }
        }
        if let Some(row) = hovered {
            self.menu.hover(row);
            ui.ctx().request_repaint();
        }
        clicked.and_then(|row| self.menu.click(row))
    }
}

fn field_id() -> egui::Id {
    egui::Id::new("lumen_context_menu_field")
}

/// Where the list of rows was last shown, scrolled or not.
pub fn list_id() -> egui::Id {
    egui::Id::new("lumen_menu_list_rect")
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
                Item::sub("Sort by", crate::menu::MenuIcon::Sort, vec![Item::run("Size", None, true, Command::Sort(SortColumn::Size))]),
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
    fn a_long_menu_shows_its_items_without_scrolling() {
        let ctx = egui::Context::default();
        let items = (0..12).map(|i| Item::run(format!("Item {i}"), None, true, Command::Open)).collect();
        let mut menu = ContextMenu::new(items, egui::pos2(100.0, 50.0));
        for _ in 0..3 {
            step(&ctx, &mut menu, vec![]);
        }
        let (first, last) = (row_rect(&ctx, 0), row_rect(&ctx, 11));
        let shown = ctx.data(|d| d.get_temp::<egui::Rect>(list_id())).expect("list drawn");
        assert!(shown.contains_rect(first) && shown.contains_rect(last), "{first:?} and {last:?} inside {shown:?}");
        let mut short = sample();
        for _ in 0..3 {
            step(&ctx, &mut short, vec![]);
        }
        let shown = ctx.data(|d| d.get_temp::<egui::Rect>(list_id())).unwrap();
        assert!(shown.height() < 3.0 * row_rect(&ctx, 0).height(), "two items, no empty space: {shown:?}");
    }

    #[test]
    fn an_item_with_its_own_picture_draws_it() {
        let ctx = egui::Context::default();
        let picture = std::sync::Arc::new(crate::menu::Bitmap { size: [2, 2], rgba: vec![255; 16] });
        let item = Item::run("Open with Code", None, true, Command::Shell(0)).with_picture(Some(picture));
        let mut menu = ContextMenu::new(vec![item], egui::pos2(100.0, 100.0));
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                menu.show(ui.ctx());
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let font = egui::TextureId::default();
        let images = shapes
            .iter()
            .filter(|s| matches!(&s.shape, egui::Shape::Mesh(m) if m.texture_id != font))
            .count();
        assert_eq!(images, 1, "the picture is painted from its own texture");
    }

    #[test]
    fn moving_the_pointer_over_a_row_highlights_it_and_a_still_pointer_leaves_the_keys_alone() {
        let ctx = egui::Context::default();
        let mut menu = sample();
        step(&ctx, &mut menu, vec![]);
        let sort = row_rect(&ctx, 1).center();
        step(&ctx, &mut menu, vec![Event::PointerMoved(sort)]);
        step(&ctx, &mut menu, vec![]);
        assert_eq!(menu.menu.cursor(), Some(1), "the hovered row is the highlighted one");
        step(&ctx, &mut menu, vec![key(K::ArrowUp)]);
        step(&ctx, &mut menu, vec![]);
        assert_eq!(menu.menu.cursor(), Some(0), "a pointer resting on a row does not pull the cursor back");
    }

    fn list_width(ctx: &egui::Context, menu: &mut ContextMenu) -> f32 {
        for _ in 0..3 {
            step(ctx, menu, vec![]);
        }
        ctx.data(|d| d.get_temp::<egui::Rect>(list_id())).expect("list drawn").width()
    }

    #[test]
    fn the_menu_widens_for_a_long_item_up_to_a_limit() {
        let ctx = egui::Context::default();
        let short = list_width(&ctx, &mut sample());
        assert!((short - WIDTH).abs() < 1.0, "short items keep the usual width: {short}");
        let long = "Add to archive and email with a long 7-Zip name";
        let mut menu = ContextMenu::new(vec![Item::run(long, Some("Ctrl+Shift+E"), true, Command::Open)], egui::pos2(10.0, 10.0));
        let wide = list_width(&ctx, &mut menu);
        let font = egui::FontId::proportional(14.0);
        let text = ctx.fonts_mut(|f| f.layout_no_wrap(format!("{long}Ctrl+Shift+E"), font, egui::Color32::WHITE).size().x);
        assert!(wide > WIDTH && wide >= text, "{wide} fits the {text} of label and shortcut");
        let huge = "x".repeat(400);
        let mut menu = ContextMenu::new(vec![Item::run(huge, None, true, Command::Open)], egui::pos2(10.0, 10.0));
        let capped = list_width(&ctx, &mut menu);
        assert!(capped <= MAX_WIDTH + 1.0, "never wider than {MAX_WIDTH}: {capped}");
    }

    /// "0" to "29", a separator after "0" and before "29"; each runs Shell(n).
    fn long() -> ContextMenu {
        let mut items = vec![Item::run("0", None, true, Command::Shell(0)), Item::separator()];
        items.extend((1..29).map(|n| Item::run(n.to_string(), None, true, Command::Shell(n))));
        items.extend([Item::separator(), Item::run("29", None, true, Command::Shell(29))]);
        ContextMenu::new(items, egui::pos2(100.0, 100.0))
    }

    fn run_after(ctx: &egui::Context, menu: &mut ContextMenu, keys: &[K]) -> Option<MenuEvent> {
        step(ctx, menu, vec![]);
        step(ctx, menu, keys.iter().map(|&k| key(k)).collect());
        step(ctx, menu, vec![key(K::Enter)])
    }

    #[test]
    fn home_and_end_go_to_the_first_and_the_last_item() {
        let ctx = egui::Context::default();
        let shell = |n| Some(MenuEvent::Run(Command::Shell(n)));
        assert_eq!(run_after(&ctx, &mut long(), &[K::End]), shell(29));
        assert_eq!(run_after(&ctx, &mut long(), &[K::End, K::Home]), shell(0));
        assert_eq!(run_after(&ctx, &mut long(), &[K::ArrowDown, K::ArrowDown, K::Home]), shell(0));
    }

    #[test]
    fn page_down_and_page_up_move_ten_items_and_stop_at_the_ends() {
        let ctx = egui::Context::default();
        let shell = |n| Some(MenuEvent::Run(Command::Shell(n)));
        assert_eq!(run_after(&ctx, &mut long(), &[K::PageDown]), shell(9), "from no cursor: the tenth item");
        assert_eq!(run_after(&ctx, &mut long(), &[K::PageDown, K::PageDown]), shell(19));
        assert_eq!(run_after(&ctx, &mut long(), &[K::PageDown, K::PageDown, K::PageDown, K::PageDown]), shell(29), "stops on the last");
        assert_eq!(run_after(&ctx, &mut long(), &[K::End, K::PageUp]), shell(19));
        assert_eq!(run_after(&ctx, &mut long(), &[K::PageDown, K::PageUp]), shell(0), "stops on the first");
    }

    #[test]
    fn while_searching_home_and_end_stay_with_the_search_box() {
        let ctx = egui::Context::default();
        let mut menu = long();
        step(&ctx, &mut menu, vec![]);
        step(&ctx, &mut menu, vec![Event::Text("2".into())]);
        step(&ctx, &mut menu, vec![key(K::End)]);
        assert_eq!(step(&ctx, &mut menu, vec![key(K::Enter)]), Some(MenuEvent::Run(Command::Shell(2))), "the cursor stayed on the first match");
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
