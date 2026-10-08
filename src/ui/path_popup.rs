//! The path box, like File Pilot's: Ctrl+L (or a click on the crumb bar) opens
//! it centred on the window with the current path selected, so typing a name
//! replaces it. Below the field it lists what is in the folder typed so far,
//! narrowed by the part being typed (its first match is picked); Up/Down pick
//! a row, Enter uses it (or the typed path), Tab completes it, Ctrl+Enter opens
//! it in a new tab, Escape closes.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::fs::{self, Entry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupEvent {
    /// Open `path`, selecting `select` in it; in a new tab with Ctrl+Enter.
    Go { path: PathBuf, select: Option<String>, new_tab: bool },
    /// The ← and → buttons: the tab's history.
    Back,
    Forward,
}

/// Keys the box reads itself, before its text field sees them.
#[derive(Default)]
struct Keys {
    up: bool,
    down: bool,
    tab: bool,
    enter: bool,
    ctrl_enter: bool,
    escape: bool,
}

impl Keys {
    fn read(ctx: &egui::Context) -> Self {
        use egui::{Key, Modifiers};
        ctx.input_mut(|i| {
            // Ctrl+Enter first: matching ignores extra modifiers.
            let ctrl_enter = i.consume_key(Modifiers::COMMAND, Key::Enter);
            Self {
                up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
                down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
                tab: i.consume_key(Modifiers::NONE, Key::Tab),
                enter: !ctrl_enter && i.consume_key(Modifiers::NONE, Key::Enter),
                ctrl_enter,
                escape: i.consume_key(Modifiers::NONE, Key::Escape),
            }
        })
    }
}

#[derive(Clone, Copy)]
enum Cursor {
    SelectAll,
    End,
}

/// Rows shown before the list scrolls.
const VISIBLE_ROWS: f32 = 10.0;

#[derive(Default)]
pub struct PathPopup {
    open: bool,
    text: String,
    /// The folder the text was filled from; another one refills it.
    filled_from: Option<PathBuf>,
    /// Where to put the cursor on the next frame, if anywhere.
    cursor: Option<Cursor>,
    /// The row picked with Up/Down, an index into the rows shown. While a
    /// name is typed and none is picked, the first row counts as picked.
    picked: Option<usize>,
    /// The folder last listed and what it holds (or why it could not be read).
    listing: Option<(PathBuf, Result<Vec<Entry>, String>)>,
}

impl PathPopup {
    pub fn open(&mut self) {
        self.open = true;
        self.filled_from = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    fn set_text(&mut self, text: String, cursor: Cursor) {
        self.text = text;
        self.cursor = Some(cursor);
        self.picked = None;
    }

    fn close(&mut self) {
        self.open = false;
        self.listing = None;
    }

    /// What is in `folder`, read again only when the folder changes.
    fn entries(&mut self, folder: &Path) -> &Result<Vec<Entry>, String> {
        if self.listing.as_ref().is_none_or(|(at, _)| at != folder) {
            let entries = fs::lister::read_entries(folder).map_err(|e| e.to_string());
            self.listing = Some((folder.to_path_buf(), entries));
        }
        &self.listing.as_ref().expect("just listed").1
    }

    /// Draws the box over `anchor` (the list) while open.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        anchor: egui::Rect,
        current: &Path,
        home: Option<&Path>,
        show_hidden: bool,
    ) -> Option<PopupEvent> {
        if !self.open {
            return None;
        }
        if self.filled_from.as_deref() != Some(current) {
            self.filled_from = Some(current.to_path_buf());
            // Selected, like File Pilot: typing a name replaces the path.
            self.set_text(initial_text(current), Cursor::SelectAll);
        }
        let keys = Keys::read(ctx);
        if keys.escape {
            self.close();
            return None;
        }

        let (folder_text, typed) = split_input(&self.text);
        let (folder_text, typed) = (folder_text.to_owned(), typed.to_owned());
        let folder = fs::resolve_input_path(&folder_text, current, home);
        let entries = match self.entries(&folder) {
            Ok(entries) => entries.clone(),
            Err(_) => Vec::new(),
        };
        let error = self.listing.as_ref().and_then(|(_, r)| r.as_ref().err().cloned());
        let rows = matches(&entries, &typed, show_hidden);
        // A typed name picks its first match, so typing it and Enter opens it.
        let first = (!typed.is_empty() && !rows.is_empty()).then_some(0);

        if keys.down && !rows.is_empty() {
            self.picked = Some(self.picked.or(first).map_or(0, |p| (p + 1).min(rows.len() - 1)));
        }
        if keys.up {
            self.picked = self.picked.or(first).and_then(|p| p.checked_sub(1));
        }
        let picked = self.picked.or(first);
        if keys.tab
            && let Some(&row) = rows.get(picked.unwrap_or(0))
        {
            // The full path, also when only a name was typed.
            let folder_text = if folder_text.is_empty() { initial_text(current) } else { folder_text };
            self.set_text(complete(&folder_text, &entries[row]), Cursor::End);
            return None;
        }
        if keys.enter || keys.ctrl_enter {
            let (path, select) = match picked.and_then(|p| rows.get(p)) {
                Some(&row) => entry_target(&entries[row]),
                None => typed_target(&self.text, current, home),
            };
            self.close();
            return Some(PopupEvent::Go { path, select, new_tab: keys.ctrl_enter });
        }

        let mut event = None;
        // Centred on the window, three quarters of its width, just below the
        // toolbar (the top of `anchor`), like File Pilot, at any window size.
        let window = ctx.content_rect();
        let frame = egui::Frame::popup(&ctx.global_style());
        let outer = (window.width() * 0.75).max(320.0_f32.min(window.width()));
        let width = outer - frame.total_margin().sum().x;
        let left = window.center().x - outer / 2.0;
        let area = egui::Modal::default_area(egui::Id::new("lumen_path_popup_area"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(left, anchor.top() + 8.0));
        let modal = egui::Modal::new(egui::Id::new("lumen_path_popup"))
            .area(area)
            .frame(frame)
            .backdrop_color(egui::Color32::TRANSPARENT)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    crate::ui::icons::folder(ui);
                    let buttons = 4.0 * (ui.spacing().interact_size.x + ui.spacing().item_spacing.x);
                    let field = egui::TextEdit::singleline(&mut self.text)
                        .id(field_id())
                        .desired_width(ui.available_width() - buttons)
                        .frame(egui::Frame::NONE)
                        .show(ui);
                    if let Some(cursor) = self.cursor.take() {
                        let range = match cursor {
                            Cursor::SelectAll => egui::text::CCursorRange::select_all(&field.galley),
                            Cursor::End => egui::text::CCursorRange::one(egui::text::CCursor::new(self.text.chars().count())),
                        };
                        let mut state = field.state;
                        state.cursor.set_char_range(Some(range));
                        state.store(ui.ctx(), field_id());
                    }
                    // The field keeps the keyboard while the box is open.
                    if !field.response.has_focus() {
                        field.response.request_focus();
                    }
                    if field.response.changed() {
                        self.picked = None;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if button(ui, "close", CLOSE, "Close (Escape)").clicked() {
                            self.close();
                        }
                        ui.separator();
                        if button(ui, "up", toolbar::UP, "Parent folder").clicked() {
                            let parent = folder.parent().unwrap_or(&folder).to_path_buf();
                            self.set_text(initial_text(&parent), Cursor::End);
                        }
                        if button(ui, "forward", toolbar::FORWARD, "Forward").clicked() {
                            event = Some(PopupEvent::Forward);
                        }
                        if button(ui, "back", toolbar::BACK, "Back").clicked() {
                            event = Some(PopupEvent::Back);
                        }
                    });
                });
                ui.separator();
                let row_height = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical()
                    .max_height(VISIBLE_ROWS * row_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if let Some(error) = &error {
                            ui.weak(error);
                        } else if rows.is_empty() {
                            ui.weak("Nothing here matches");
                        }
                        for (shown, &row) in rows.iter().enumerate() {
                            let entry = &entries[row];
                            let picked = picked == Some(shown);
                            let line = egui::UiBuilder::new().id(row_id(shown)).sense(egui::Sense::click());
                            let response = ui
                                .scope_builder(line, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    let fill = if picked { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
                                    egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            crate::ui::icons::show(ui, entry, ui.text_style_height(&egui::TextStyle::Body));
                                            crate::ui::file_list::name_label(ui, entry);
                                        });
                                    });
                                })
                                .response;
                            if picked {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                let (path, select) = entry_target(entry);
                                event = Some(PopupEvent::Go { path, select, new_tab: false });
                            }
                        }
                    });
                ui.separator();
                hints(ui);
            });
        if modal.should_close() {
            self.close();
        }
        if matches!(event, Some(PopupEvent::Go { .. })) {
            self.close();
        }
        event
    }
}

const CLOSE: &str = "×";

use crate::ui::toolbar;

fn field_id() -> egui::Id {
    egui::Id::new("lumen_path_popup_field")
}

fn button(ui: &mut egui::Ui, name: &str, symbol: &str, hint: &str) -> egui::Response {
    let galley = egui::WidgetText::from(symbol).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button);
    let size = egui::vec2(ui.spacing().interact_size.y, ui.spacing().interact_size.y);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let response = ui.interact(rect, button_id(name), egui::Sense::click());
    let color = if response.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response.on_hover_text(hint)
}

/// The footer: each key in a small box, then what it does.
fn hints(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        for (keys, what) in [
            (&["Up", "Down"][..], "to navigate"),
            (&["Enter"][..], "to use"),
            (&["Tab"][..], "to expand"),
            (&["Ctrl+Enter"][..], "to open in a new tab"),
            (&["Escape"][..], "to dismiss"),
        ] {
            for key in keys {
                egui::Frame::new()
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .corner_radius(3)
                    .inner_margin(egui::Margin::symmetric(4, 0))
                    .show(ui, |ui| ui.small(*key));
            }
            ui.weak(what);
            ui.add_space(8.0);
        }
    });
}

/// Ids, so tests can find the rows and buttons.
pub fn row_id(index: usize) -> egui::Id {
    egui::Id::new(("lumen_path_popup_row", index))
}

pub fn button_id(name: &str) -> egui::Id {
    egui::Id::new(("lumen_path_popup_button", name))
}

/// The text the box starts with: the folder and a trailing separator, so its
/// contents are listed at once.
pub fn initial_text(current: &Path) -> String {
    let text = current.display().to_string();
    if text.ends_with(std::path::MAIN_SEPARATOR) { text } else { text + std::path::MAIN_SEPARATOR_STR }
}

/// Splits the text after its last separator: the folder part (kept with its
/// separator) and the name being typed.
pub fn split_input(text: &str) -> (&str, &str) {
    match text.rfind(['/', '\\']) {
        Some(at) => text.split_at(at + 1),
        None => ("", text),
    }
}

/// Indices of the entries to list for `typed`, case-insensitive: names that
/// start with it first, then names that contain it, each in folder order.
/// Hidden entries only when shown, or when `typed` starts with a dot.
pub fn matches(entries: &[Entry], typed: &str, show_hidden: bool) -> Vec<usize> {
    let typed = typed.to_lowercase();
    let hidden_ok = show_hidden || typed.starts_with('.');
    let names: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| hidden_ok || !e.hidden)
        .map(|(i, e)| (i, e.name.to_lowercase()))
        .collect();
    let starting = names.iter().filter(|(_, n)| n.starts_with(&typed)).map(|(i, _)| *i);
    let containing = names.iter().filter(|(_, n)| !n.starts_with(&typed) && n.contains(&typed)).map(|(i, _)| *i);
    starting.chain(containing).collect()
}

/// Tab: the folder part with `entry`'s name, and a separator after a folder
/// so its contents show next.
pub fn complete(folder: &str, entry: &Entry) -> String {
    let separator = if entry.is_dir { std::path::MAIN_SEPARATOR_STR } else { "" };
    format!("{folder}{}{separator}", entry.name)
}

/// Where using `entry` goes: a folder opens; a file opens its folder with
/// the file selected.
pub fn entry_target(entry: &Entry) -> (PathBuf, Option<String>) {
    match entry.path.parent().filter(|_| !entry.is_dir) {
        Some(folder) => (folder.to_path_buf(), Some(entry.name.clone())),
        None => (entry.path.clone(), None),
    }
}

/// Where Enter goes with no row picked: the typed path, resolved like the
/// old path bar (`~`, relative to the current folder, `..`); a file opens its
/// folder with the file selected.
pub fn typed_target(text: &str, current: &Path, home: Option<&Path>) -> (PathBuf, Option<String>) {
    let path = fs::resolve_input_path(text, current, home);
    match (path.is_file(), path.parent(), path.file_name()) {
        (true, Some(folder), Some(name)) => (folder.to_path_buf(), Some(name.to_string_lossy().into_owned())),
        _ => (path, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/home/u").join(name),
            is_dir,
            size: None,
            modified: None,
            hidden: name.starts_with('.'),
        }
    }

    use crate::ui::testing::{SCREEN, click, frame};
    use eframe::egui::{Event, Key, Modifiers};

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    fn text(t: &str) -> Event {
        Event::Text(t.into())
    }

    /// A folder with sub/, a.txt and b.txt, and the box opened on it.
    fn opened(name: &str) -> (egui::Context, PathPopup, PathBuf) {
        let dir = crate::fs::ops::tests::fresh_fixture(name);
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        let ctx = egui::Context::default();
        let mut popup = PathPopup::default();
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        (ctx, popup, dir)
    }

    fn step(ctx: &egui::Context, popup: &mut PathPopup, dir: &Path, events: Vec<Event>) -> Option<PopupEvent> {
        frame(ctx, events, |ui| popup.show(ui.ctx(), SCREEN, dir, None, false))
    }

    fn go(path: PathBuf, select: Option<&str>, new_tab: bool) -> Option<PopupEvent> {
        Some(PopupEvent::Go { path, select: select.map(Into::into), new_tab })
    }

    #[test]
    fn it_opens_with_the_path_selected_so_typing_replaces_it() {
        let (ctx, mut popup, dir) = opened("popup-type");
        assert_eq!(popup.text(), initial_text(&dir));
        assert_eq!(step(&ctx, &mut popup, &dir, vec![text("sub")]), None);
        assert_eq!(popup.text(), "sub", "the selected path was replaced");
        assert_eq!(step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]), go(dir.join("sub"), None, false));
        assert!(!popup.is_open(), "it closes once used");
    }

    #[test]
    fn a_typed_name_picks_its_first_match_so_enter_opens_it() {
        let (ctx, mut popup, dir) = opened("popup-first");
        step(&ctx, &mut popup, &dir, vec![text("SU")]);
        assert_eq!(step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]), go(dir.join("sub"), None, false));
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        step(&ctx, &mut popup, &dir, vec![text("a.")]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, go(dir.clone(), Some("a.txt"), false));
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        step(&ctx, &mut popup, &dir, vec![text("nothing-like-it")]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, go(dir.join("nothing-like-it"), None, false), "no match: the typed path");
    }

    #[test]
    fn the_box_is_centred_on_the_window_whatever_the_list_size() {
        let (ctx, mut popup, dir) = opened("popup-centre");
        let list = egui::Rect::from_min_max(egui::pos2(200.0, 60.0), SCREEN.max);
        for _ in 0..3 {
            frame(&ctx, vec![], |ui| popup.show(ui.ctx(), list, &dir, None, false));
        }
        let area = ctx.memory(|m| m.area_rect(egui::Id::new("lumen_path_popup_area"))).expect("shown");
        assert!((area.center().x - SCREEN.center().x).abs() < 1.0, "centred: {area:?}");
        assert!((area.width() - 0.75 * SCREEN.width()).abs() < 1.0, "75% wide: {area:?}");
        assert!(area.top() >= list.top(), "below the toolbar: {area:?}");
    }

    #[test]
    fn down_picks_a_row_and_enter_uses_it() {
        let (ctx, mut popup, dir) = opened("popup-down");
        step(&ctx, &mut popup, &dir, vec![text("t")]); // a.txt, b.txt contain "t"
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowUp, Modifiers::NONE)]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, go(dir.clone(), Some("a.txt"), false), "a file opens its folder, selected");
    }

    #[test]
    fn tab_completes_the_first_row_and_ctrl_enter_opens_a_new_tab() {
        let (ctx, mut popup, dir) = opened("popup-tab");
        step(&ctx, &mut popup, &dir, vec![text("su")]);
        step(&ctx, &mut popup, &dir, vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(popup.text(), format!("{}/sub/", dir.display()));
        assert!(popup.is_open(), "Tab does not leave the box");
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::COMMAND)]);
        assert_eq!(used, go(dir.join("sub"), None, true));
    }

    #[test]
    fn escape_or_a_click_outside_closes_without_going() {
        let (ctx, mut popup, dir) = opened("popup-esc");
        step(&ctx, &mut popup, &dir, vec![text("x")]);
        assert_eq!(step(&ctx, &mut popup, &dir, vec![key(Key::Escape, Modifiers::NONE)]), None);
        assert!(!popup.is_open());
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        assert_eq!(popup.text(), initial_text(&dir), "opened again: the path, not the old text");
        let outside = SCREEN.left_bottom() + egui::vec2(5.0, -5.0);
        assert_eq!(click(&ctx, outside, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false)), None);
        assert!(!popup.is_open());
    }

    #[test]
    fn a_click_on_a_row_uses_it_and_the_buttons_work() {
        let (ctx, mut popup, dir) = opened("popup-click");
        let at = |id| ctx.read_response(id).expect("drawn").rect.center();
        let up = at(button_id("up"));
        click(&ctx, up, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false));
        assert_eq!(popup.text(), initial_text(dir.parent().unwrap()), "↑ edits the field to the parent");
        assert!(popup.is_open());
        let back = at(button_id("back"));
        assert_eq!(click(&ctx, back, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false)), Some(PopupEvent::Back));
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        step(&ctx, &mut popup, &dir, vec![]);
        let row = at(row_id(0)); // sub/, folders first
        assert_eq!(click(&ctx, row, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false)), go(dir.join("sub"), None, false));
    }

    #[test]
    fn the_box_starts_with_the_folder_and_a_separator() {
        assert_eq!(initial_text(Path::new("/home/u/Descargas")), "/home/u/Descargas/");
        assert_eq!(initial_text(Path::new("/")), "/");
    }

    #[test]
    fn the_text_splits_into_folder_and_typed_name() {
        assert_eq!(split_input("/home/u/De"), ("/home/u/", "De"));
        assert_eq!(split_input("/home/u/"), ("/home/u/", ""));
        assert_eq!(split_input("De"), ("", "De"));
        assert_eq!(split_input("~/Do"), ("~/", "Do"));
    }

    #[test]
    fn rows_starting_with_the_typed_name_come_first_then_those_containing_it() {
        let entries = [e("Descargas", true), e("Documentos", true), e(".config", true), e("desc.txt", false), e("my_desc.md", false)];
        let names = |idx: Vec<usize>| idx.into_iter().map(|i| entries[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(matches(&entries, "DESC", false)), ["Descargas", "desc.txt", "my_desc.md"]);
        assert_eq!(names(matches(&entries, "", false)), ["Descargas", "Documentos", "desc.txt", "my_desc.md"]);
        assert_eq!(names(matches(&entries, ".c", false)), [".config"], "a leading dot asks for hidden ones");
        assert_eq!(matches(&entries, "", true).len(), 5);
        assert!(matches(&entries, "zzz", false).is_empty());
    }

    #[test]
    fn tab_completes_a_folder_with_a_separator_and_a_file_without() {
        assert_eq!(complete("/home/u/", &e("Descargas", true)), "/home/u/Descargas/");
        assert_eq!(complete("/home/u/", &e("desc.txt", false)), "/home/u/desc.txt");
        assert_eq!(complete("", &e("Descargas", true)), "Descargas/");
    }

    #[test]
    fn a_folder_opens_and_a_file_opens_its_folder_selected() {
        assert_eq!(entry_target(&e("Descargas", true)), (PathBuf::from("/home/u/Descargas"), None));
        assert_eq!(entry_target(&e("desc.txt", false)), (PathBuf::from("/home/u"), Some("desc.txt".into())));
    }

    #[test]
    fn a_typed_path_resolves_and_a_typed_file_opens_its_folder() {
        let dir = crate::fs::ops::tests::fresh_fixture("popup-typed");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        assert_eq!(typed_target("sub/..", &dir, None), (dir.clone(), None));
        assert_eq!(typed_target(&format!("{}/sub/", dir.display()), &dir, None), (dir.join("sub"), None));
        assert_eq!(typed_target("a.txt", &dir, None), (dir.clone(), Some("a.txt".into())));
        assert_eq!(typed_target("missing", &dir, None), (dir.join("missing"), None), "the listing reports it");
    }
}
