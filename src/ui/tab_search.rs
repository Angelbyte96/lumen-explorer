//! Search Tabs, like File Pilot's: Ctrl+Shift+A (or the ▾ in the caption
//! bar) opens a box over the window that lists the open tabs, each with its
//! folder, its path and how long ago it was used. Typing narrows the rows,
//! Up/Down pick one, Enter or a click switches to it, Escape closes.

use std::path::Path;
use std::time::Duration;

use eframe::egui;

/// One open tab as the box lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The tab's title, as the tab bar shows it.
    pub label: String,
    /// Its folder, shortened (see `compact`); empty for a place with no path.
    pub path: String,
    /// How long since the tab was last on screen; `None` for the active tab.
    pub idle: Option<Duration>,
}

/// How long ago a tab was used: "Now" for the active one and under a minute,
/// then minutes, hours and days.
pub fn ago(idle: Option<Duration>) -> String {
    let minutes = idle.map_or(0, |idle| idle.as_secs() / 60);
    match minutes {
        0 => "Now".to_owned(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        _ => format!("{} d ago", minutes / 1440),
    }
}

/// `path` with the home folder shown as `~`.
pub fn compact(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// The indexes of the tabs whose name or path holds every word of `search`, in any case.
fn listed(entries: &[Entry], search: &str) -> Vec<usize> {
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    let holds = |entry: &Entry, word: &str| entry.label.to_lowercase().contains(word) || entry.path.to_lowercase().contains(word);
    (0..entries.len()).filter(|&i| words.iter().all(|w| holds(&entries[i], w))).collect()
}

/// Keys the box reads itself, before its search field sees them.
struct Keys {
    up: bool,
    down: bool,
    enter: bool,
    /// Escape, or Ctrl+Shift+A again.
    close: bool,
}

impl Keys {
    fn read(ctx: &egui::Context) -> Self {
        use egui::{Key, Modifiers};
        ctx.input_mut(|i| Self {
            up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
            down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
            enter: i.consume_key(Modifiers::NONE, Key::Enter),
            close: i.consume_key(Modifiers::NONE, Key::Escape) | i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::A),
        })
    }
}

fn field_id() -> egui::Id {
    egui::Id::new("lumen_tab_search_field")
}

const WIDTH: f32 = 520.0;
/// The box's height besides its rows: search field, separators, footer and margins.
const CHROME: f32 = 140.0;
const FOOTER: &str = "Up Down to navigate · Enter to switch · Escape to dismiss";

#[derive(Default)]
pub struct TabSearch {
    open: bool,
    search: String,
    /// The listed row Up/Down picked: Enter switches to it.
    picked: usize,
}

impl TabSearch {
    /// Opens the box with the active tab (`active`) picked.
    pub fn open(&mut self, active: usize) {
        *self = Self { open: true, picked: active, ..Self::default() };
    }

    /// Ctrl+Shift+A or the ▾: opens the box, or closes it.
    pub fn toggle(&mut self, active: usize) {
        if self.open {
            self.close();
        } else {
            self.open(active);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn close(&mut self) {
        *self = Self::default();
    }

    /// Draws the box while open, over `entries` (one per tab, in order);
    /// returns the tab to switch to, once chosen.
    pub fn show(&mut self, ctx: &egui::Context, entries: &[Entry]) -> Option<usize> {
        if !self.open {
            return None;
        }
        let keys = Keys::read(ctx);
        if keys.close {
            self.close();
            return None;
        }
        if keys.up || keys.down {
            // egui moves the focus with the arrows at the end of the frame:
            // cancel that move, or the box would lose the keyboard.
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        }
        let rows = listed(entries, &self.search);
        let count = rows.len();
        self.picked = self.picked.min(count.saturating_sub(1));
        if count > 0 && keys.down {
            self.picked = (self.picked + 1) % count;
        }
        if count > 0 && keys.up {
            self.picked = (self.picked + count - 1) % count;
        }
        let mut chosen = (keys.enter).then(|| rows.get(self.picked).copied()).flatten();

        let frame = egui::Frame::popup(&ctx.global_style());
        let width = WIDTH.min(ctx.content_rect().width() - frame.total_margin().sum().x).max(0.0);
        let picked = self.picked;
        let follow = keys.up || keys.down;
        let hint = format!("Search {} tabs...", entries.len());
        let search = &mut self.search;
        let mut typed = false;
        let modal = egui::Modal::new(egui::Id::new("lumen_tab_search")).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            ui.style_mut().interaction.selectable_labels = false;
            ui.horizontal(|ui| {
                crate::ui::icons::magnifier(ui);
                let field = egui::TextEdit::singleline(search).id(field_id()).hint_text(hint).desired_width(f32::INFINITY).frame(egui::Frame::NONE).show(ui);
                // The search keeps the keyboard while the box is open.
                if !field.response.has_focus() {
                    field.response.request_focus();
                }
                typed = field.response.changed();
            });
            ui.separator();
            // Rows past a short window scroll; the footer stays in view.
            let room = (ctx.content_rect().height() - CHROME).max(0.0);
            egui::ScrollArea::vertical().max_height(room).show(ui, |ui| {
                for (row, &index) in rows.iter().enumerate() {
                    let response = tab_row(ui, &entries[index], index, picked == row);
                    if follow && picked == row {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() {
                        chosen = Some(index);
                    }
                }
            });
            ui.separator();
            ui.weak(FOOTER);
        });
        if typed {
            self.picked = 0;
        }
        if chosen.is_some() || modal.should_close() {
            self.close();
        }
        chosen
    }
}

/// A row of the box: the folder icon, the tab's name and path, and on the
/// right how long ago it was used.
fn tab_row(ui: &mut egui::Ui, entry: &Entry, index: usize, picked: bool) -> egui::Response {
    let line = egui::UiBuilder::new().id(egui::Id::new(("lumen_tab_search_row", index))).sense(egui::Sense::click());
    ui.scope_builder(line, |ui| {
        ui.set_min_width(ui.available_width());
        let fill = if picked { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
        egui::Frame::new().fill(fill).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                crate::ui::icons::folder(ui);
                ui.label(egui::RichText::new(&entry.label).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(ago(entry.idle));
                    ui.add(egui::Label::new(egui::RichText::new(&entry.path).weak()).truncate());
                });
            });
        });
    })
    .response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing::run;
    use eframe::egui::{Event, Key, Modifiers};

    type Texts = Vec<(String, egui::Rect)>;

    fn texts_of(shapes: &[egui::epaint::ClippedShape]) -> Texts {
        fn collect(shape: &egui::epaint::Shape, texts: &mut Texts) {
            match shape {
                egui::epaint::Shape::Text(t) => texts.push((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, texts)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        shapes.iter().for_each(|s| collect(&s.shape, &mut texts));
        texts
    }

    fn at(texts: &Texts, text: &str) -> egui::Rect {
        texts.iter().find(|(t, _)| t == text).map(|(_, r)| *r).unwrap_or_else(|| panic!("{text:?} not painted: {texts:?}"))
    }

    fn painted(texts: &Texts, text: &str) -> bool {
        texts.iter().any(|(t, _)| t == text)
    }

    fn entry(label: &str, path: &str, minutes: Option<u64>) -> Entry {
        Entry { label: label.into(), path: path.into(), idle: minutes.map(|m| Duration::from_secs(m * 60)) }
    }

    /// Three tabs, the second on screen.
    fn tabs() -> Vec<Entry> {
        vec![entry("Documents", "~/Documents", Some(5)), entry("src", "~/code/lumen/src", None), entry("Pictures", "/media/Pictures", Some(90))]
    }

    /// One frame of the box; what it chose and what it painted.
    fn step(ctx: &egui::Context, search: &mut TabSearch, entries: &[Entry], events: Vec<Event>) -> (Option<usize>, Texts) {
        let (chosen, shapes) = run(ctx, events, |ui| search.show(ui.ctx(), entries));
        (chosen, texts_of(&shapes))
    }

    /// The box opened on the second tab and drawn, so its field has the focus.
    fn opened(entries: &[Entry]) -> (egui::Context, TabSearch) {
        let ctx = egui::Context::default();
        let mut search = TabSearch::default();
        search.open(1);
        step(&ctx, &mut search, entries, vec![]);
        step(&ctx, &mut search, entries, vec![]);
        (ctx, search)
    }

    fn key_with(key: Key, modifiers: Modifiers) -> Vec<Event> {
        let event = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        vec![event(true), event(false)]
    }

    fn key(key: Key) -> Vec<Event> {
        key_with(key, Modifiers::NONE)
    }

    fn click_at(pos: egui::Pos2) -> Vec<Event> {
        let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        vec![Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn ago_says_now_then_minutes_hours_and_days() {
        let after = |secs| ago(Some(Duration::from_secs(secs)));
        assert_eq!(ago(None), "Now", "the active tab");
        assert_eq!(after(0), "Now");
        assert_eq!(after(59), "Now", "under a minute");
        assert_eq!(after(60), "1 min ago");
        assert_eq!(after(59 * 60 + 59), "59 min ago");
        assert_eq!(after(3600), "1 h ago");
        assert_eq!(after(23 * 3600 + 3599), "23 h ago");
        assert_eq!(after(24 * 3600), "1 d ago");
        assert_eq!(after(10 * 24 * 3600), "10 d ago");
    }

    #[test]
    fn compact_shows_the_home_folder_as_a_tilde() {
        let home = Path::new("/home/ana");
        assert_eq!(compact(Path::new("/home/ana/code/lumen"), Some(home)), format!("~{}code/lumen", std::path::MAIN_SEPARATOR));
        assert_eq!(compact(home, Some(home)), "~");
        assert_eq!(compact(Path::new("/home/anabel/x"), Some(home)), "/home/anabel/x", "a sibling sharing the prefix is not home");
        assert_eq!(compact(Path::new("/media/disk"), Some(home)), "/media/disk");
        assert_eq!(compact(Path::new("/media/disk"), None), "/media/disk");
    }

    #[test]
    fn the_box_lists_every_tab_in_order_with_its_path_and_when_it_was_used() {
        let entries = tabs();
        let (ctx, mut search) = opened(&entries);
        let (_, texts) = step(&ctx, &mut search, &entries, vec![]);
        at(&texts, "Search 3 tabs...");
        at(&texts, FOOTER);
        for (name, path, when) in [("Documents", "~/Documents", "5 min ago"), ("src", "~/code/lumen/src", "Now"), ("Pictures", "/media/Pictures", "1 h ago")] {
            let (name, path, when) = (at(&texts, name), at(&texts, path), at(&texts, when));
            assert!(path.left() > name.right() && (path.center().y - name.center().y).abs() < 1.0, "the path on its name's row, after it");
            assert!(when.left() > path.right() && (when.center().y - name.center().y).abs() < 1.0, "when it was used, on the right");
        }
        let tops = ["Documents", "src", "Pictures"].map(|t| at(&texts, t).top());
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "in tab order: {tops:?}");
    }

    #[test]
    fn typing_narrows_the_rows_by_name_or_path_in_any_case() {
        let entries = tabs();
        let shown = |typed: &str| {
            let (ctx, mut search) = opened(&entries);
            step(&ctx, &mut search, &entries, vec![Event::Text(typed.into())]);
            let (_, texts) = step(&ctx, &mut search, &entries, vec![]);
            ["Documents", "src", "Pictures"].map(|t| painted(&texts, t))
        };
        assert_eq!(shown("docu"), [true, false, false], "by name");
        assert_eq!(shown("LUMEN"), [false, true, false], "by path, any case");
        assert_eq!(shown("media pict"), [false, false, true], "every word, in name or path");
        assert_eq!(shown("media docu"), [false, false, false], "a word nowhere hides the row");
        assert_eq!(shown(""), [true, true, true]);
    }

    #[test]
    fn enter_switches_to_the_active_tab_the_box_opened_on() {
        let entries = tabs();
        let (ctx, mut search) = opened(&entries);
        let (chosen, _) = step(&ctx, &mut search, &entries, key(Key::Enter));
        assert_eq!(chosen, Some(1));
        assert!(!search.is_open(), "switching closes the box");
    }

    #[test]
    fn up_and_down_pick_a_row_wrapping_around() {
        let entries = tabs();
        let pick = |keys: &[Key]| {
            let (ctx, mut search) = opened(&entries);
            for &k in keys {
                step(&ctx, &mut search, &entries, key(k));
                assert!(search.is_open(), "{k:?} leaves the box open");
            }
            step(&ctx, &mut search, &entries, key(Key::Enter)).0
        };
        assert_eq!(pick(&[Key::ArrowDown]), Some(2));
        assert_eq!(pick(&[Key::ArrowUp]), Some(0));
        assert_eq!(pick(&[Key::ArrowDown, Key::ArrowDown]), Some(0), "past the last row, the first");
        assert_eq!(pick(&[Key::ArrowUp, Key::ArrowUp]), Some(2), "before the first row, the last");
    }

    #[test]
    fn typing_picks_the_first_row_left_and_enter_switches_to_that_tab() {
        let entries = tabs();
        let (ctx, mut search) = opened(&entries);
        step(&ctx, &mut search, &entries, vec![Event::Text("pict".into())]);
        assert_eq!(step(&ctx, &mut search, &entries, key(Key::Enter)).0, Some(2), "the tab's own index, not the row's");
    }

    #[test]
    fn enter_with_no_row_left_switches_to_nothing_and_keeps_the_box() {
        let entries = tabs();
        let (ctx, mut search) = opened(&entries);
        step(&ctx, &mut search, &entries, vec![Event::Text("nothing here".into())]);
        assert_eq!(step(&ctx, &mut search, &entries, key(Key::Enter)).0, None);
        assert!(search.is_open());
    }

    #[test]
    fn a_click_on_a_row_switches_to_its_tab() {
        let entries = tabs();
        let (ctx, mut search) = opened(&entries);
        let (_, texts) = step(&ctx, &mut search, &entries, vec![]);
        let (chosen, _) = step(&ctx, &mut search, &entries, click_at(at(&texts, "Pictures").center()));
        assert_eq!(chosen, Some(2));
        assert!(!search.is_open());
    }

    #[test]
    fn escape_and_ctrl_shift_a_close_the_box_and_switch_to_nothing() {
        let entries = tabs();
        for keys in [key(Key::Escape), key_with(Key::A, Modifiers::COMMAND | Modifiers::SHIFT)] {
            let (ctx, mut search) = opened(&entries);
            let (chosen, _) = step(&ctx, &mut search, &entries, keys);
            assert_eq!(chosen, None);
            assert!(!search.is_open());
            let (_, texts) = step(&ctx, &mut search, &entries, vec![]);
            assert!(!painted(&texts, "Documents"), "nothing drawn once closed");
        }
    }

    #[test]
    fn toggle_opens_then_closes_and_a_new_box_starts_empty() {
        let entries = tabs();
        let (ctx, mut search) = opened(&entries);
        step(&ctx, &mut search, &entries, vec![Event::Text("docu".into())]);
        search.toggle(1);
        assert!(!search.is_open());
        search.toggle(0);
        assert!(search.is_open());
        step(&ctx, &mut search, &entries, vec![]);
        let (_, texts) = step(&ctx, &mut search, &entries, vec![]);
        assert!(painted(&texts, "Pictures"), "the old search is gone");
        assert_eq!(step(&ctx, &mut search, &entries, key(Key::Enter)).0, Some(0), "picked on the tab it opened on");
    }
}
