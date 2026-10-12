//! The path box, like File Pilot's: Ctrl+L (or a click on the crumb bar) opens
//! it centred on the window with the current path selected, so typing a name
//! replaces it. Below the field it lists what is in the folder typed so far,
//! narrowed by the words being typed, in any order, matches highlighted (the
//! first match is picked); Up/Down pick
//! a row, Enter uses it (or the typed path), Tab completes it, Ctrl+Enter opens
//! it in a new tab, Escape closes.
//!
//! A word with no path ("down", no separator) is the quick jump instead: it
//! lists matches from the whole index, visited folders first, each with its
//! folder; the rows work the same way.

use std::ops::Range;
use std::path::{Path, PathBuf};

use eframe::egui;

use crate::fs::index::Crawler;
use crate::fs::search::{Hit, Searcher};
use crate::fs::{self, Entry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupEvent {
    /// Open `path`, selecting `select` in it; in a new tab with Ctrl+Enter.
    Go { path: PathBuf, select: Option<String>, new_tab: bool },
    /// Enter or a click on a file: open it with its default app.
    Open(PathBuf),
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

/// Global results kept: more than a person scrolls through.
const RESULTS: usize = 50;

/// How often the results are searched again while the crawl goes on.
const REFRESH_SECS: f64 = 0.5;

/// An index older than this (seconds since its crawl started) is crawled
/// again when the box opens.
const STALE_SECS: f64 = 600.0;

/// One row of the list: a folder's entry, or a hit of the whole index.
struct Row {
    entry: Entry,
    /// Name letters to mark.
    ranges: Vec<Range<usize>>,
    /// The folder it is in, shown beside a global hit.
    parent: Option<PathBuf>,
}

impl Row {
    fn of_hit(hit: &Hit) -> Self {
        let name = hit.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let entry = Entry { name, path: hit.path.clone(), is_dir: hit.is_dir, size: None, modified: None, hidden: hit.hidden };
        Self { entry, ranges: hit.ranges.clone(), parent: hit.path.parent().map(Path::to_path_buf) }
    }
}

/// What the last search asked, to know when to ask again.
struct Asked {
    query: String,
    show_hidden: bool,
    /// Entries indexed and whether the crawl was over when it was asked.
    items: usize,
    done: bool,
    at: f64,
}

/// Starts a crawl of the whole scope: the first one, and every re-crawl.
pub type Restart = Box<dyn Fn(&egui::Context) -> Crawler>;

/// The quick jump: the crawl and the searches over what it finds.
struct Jump {
    crawler: Crawler,
    /// Starts the next crawl.
    restart: Restart,
    /// The crawl whose index replaces `crawler`'s once it is done.
    recrawl: Option<Crawler>,
    /// When the crawl of `crawler` started, in seconds of the session.
    started: f64,
    /// Lumen changed files since that crawl started.
    changed: bool,
    searcher: Searcher,
    asked: Option<Asked>,
    /// The request whose answer `hits` holds.
    shown: u64,
    hits: Vec<Hit>,
    /// Entries indexed so far, for the progress line.
    items: usize,
}

impl Jump {
    fn new(crawler: Crawler, restart: Restart) -> Self {
        let searcher = Searcher::new(crawler.index().clone());
        // The first crawl starts with the app, at second 0 of its session.
        Self { crawler, restart, recrawl: None, started: 0.0, changed: false, searcher, asked: None, shown: 0, hits: Vec::new(), items: 0 }
    }

    /// Puts a finished re-crawl's index in place of the old one; the next
    /// frame searches the word again and the old index is dropped.
    fn swap_in_recrawl(&mut self) {
        if self.recrawl.as_ref().is_some_and(Crawler::is_done)
            && let Some(crawler) = self.recrawl.take()
        {
            self.searcher = Searcher::new(crawler.index().clone());
            self.crawler = crawler;
            self.shown = 0;
            // The old hits stay on screen until the new answer lands.
            self.asked = None;
        }
    }

    /// Starts a re-crawl when the index is older than [`STALE_SECS`] or Lumen
    /// changed files since it started. Never while a crawl is going on: the
    /// first one has no finished index to renew yet.
    fn renew_if_stale(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if self.recrawl.is_none() && self.crawler.is_done() && (self.changed || now - self.started >= STALE_SECS) {
            self.recrawl = Some((self.restart)(ctx));
            self.started = now;
            self.changed = false;
        }
    }

    /// Takes in a finished search and asks again when `query` changed or the
    /// crawl found more (at most twice a second) or ended since the last one.
    /// Returns whether an answer for the latest ask is still due.
    fn refresh(&mut self, ctx: &egui::Context, query: &str, show_hidden: bool, frecent: impl FnOnce() -> Vec<(PathBuf, f64)>) -> bool {
        if let Some(answer) = self.searcher.poll() {
            self.shown = answer.id;
            self.hits = answer.hits;
        }
        // Never wait for the crawl's write lock on the UI thread.
        if let Ok(index) = self.crawler.index().try_read() {
            self.items = index.len();
        }
        let done = self.crawler.is_done();
        let now = ctx.input(|i| i.time);
        let stale = self.asked.as_ref().is_none_or(|a| {
            a.query != query || a.show_hidden != show_hidden || (!a.done && (done || (self.items > a.items && now - a.at >= REFRESH_SECS)))
        });
        if stale {
            let repaint = ctx.clone();
            self.searcher.request(query.to_owned(), show_hidden, frecent(), RESULTS, move || repaint.request_repaint());
            self.asked = Some(Asked { query: query.to_owned(), show_hidden, items: self.items, done, at: now });
        }
        if !done {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(REFRESH_SECS));
        }
        self.searcher.is_pending(self.shown)
    }

    /// How many entries are indexed while the crawl is still going.
    fn indexing(&self) -> Option<usize> {
        (!self.crawler.is_done()).then_some(self.items)
    }

    fn reset(&mut self) {
        self.asked = None;
        self.hits.clear();
    }
}

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
    /// The quick jump; without it a word with no path filters the folder.
    jump: Option<Jump>,
    /// Keys that use a row, pressed while its answer was still being searched.
    held: Keys,
    /// Up/Down pressed while the answer was still being searched, in order
    /// (true: down), to move over the answer once it lands.
    held_moves: Vec<bool>,
    /// The box just opened: the index may need a re-crawl.
    check_index: bool,
}

impl PathPopup {
    /// A box whose words with no path search what `crawler` indexes;
    /// `restart` starts the crawl that renews a stale index.
    pub fn with_crawler(crawler: Crawler, restart: Restart) -> Self {
        Self { jump: Some(Jump::new(crawler, restart)), ..Self::default() }
    }

    /// Lumen changed the files: the index no longer matches the disk.
    pub fn mark_stale(&mut self) {
        if let Some(jump) = &mut self.jump {
            jump.changed = true;
        }
    }

    /// Whether a re-crawl is going on behind the index that answers.
    #[cfg(test)]
    pub fn is_recrawling(&self) -> bool {
        self.jump.as_ref().is_some_and(|j| j.recrawl.is_some())
    }

    /// Whether the latest search has not answered yet.
    #[cfg(test)]
    pub fn is_searching(&self) -> bool {
        self.jump.as_ref().is_some_and(|j| j.searcher.is_pending(j.shown))
    }

    pub fn open(&mut self) {
        self.open = true;
        self.check_index = true;
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
        self.held = Keys::default();
        self.held_moves.clear();
        if let Some(jump) = &mut self.jump {
            jump.reset();
        }
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
        frecent: impl FnOnce() -> Vec<(PathBuf, f64)>,
    ) -> Option<PopupEvent> {
        if !self.open {
            return None;
        }
        if let Some(jump) = &mut self.jump {
            jump.swap_in_recrawl();
            if std::mem::take(&mut self.check_index) {
                jump.renew_if_stale(ctx);
            }
        }
        if self.filled_from.as_deref() != Some(current) {
            self.filled_from = Some(current.to_path_buf());
            // Selected, like File Pilot: typing a name replaces the path.
            self.set_text(initial_text(current), Cursor::SelectAll);
        }
        let mut keys = Keys::read(ctx);
        if keys.escape {
            self.close();
            return None;
        }

        let (folder_text, typed) = split_input(&self.text);
        let (folder_text, typed) = (folder_text.to_owned(), typed.to_owned());
        let folder = fs::resolve_input_path(&folder_text, current, home);
        let global = is_bare_word(&self.text);
        let mut error = None;
        let mut indexing = None;
        let mut moves = Vec::new();
        let rows: Vec<Row> = match &mut self.jump {
            Some(jump) if global => {
                let pending = jump.refresh(ctx, self.text.trim(), show_hidden, frecent);
                indexing = jump.indexing();
                if pending {
                    // A key that uses a row waits for the answer, or it would
                    // use a row of the word before.
                    self.held.tab |= std::mem::take(&mut keys.tab);
                    self.held.enter |= std::mem::take(&mut keys.enter);
                    self.held.ctrl_enter |= std::mem::take(&mut keys.ctrl_enter);
                    if std::mem::take(&mut keys.down) {
                        self.held_moves.push(true);
                    }
                    if std::mem::take(&mut keys.up) {
                        self.held_moves.push(false);
                    }
                } else {
                    moves = std::mem::take(&mut self.held_moves);
                    let held = std::mem::take(&mut self.held);
                    keys.tab |= held.tab;
                    keys.enter |= held.enter;
                    keys.ctrl_enter |= held.ctrl_enter;
                }
                jump.hits.iter().map(Row::of_hit).collect()
            }
            _ => {
                self.held = Keys::default();
                self.held_moves.clear();
                let entries = match self.entries(&folder) {
                    Ok(entries) => entries.clone(),
                    Err(_) => Vec::new(),
                };
                error = self.listing.as_ref().and_then(|(_, r)| r.as_ref().err().cloned());
                matches(&entries, &typed, show_hidden)
                    .into_iter()
                    .map(|i| Row { ranges: word_ranges(&entries[i].name, &typed).unwrap_or_default(), entry: entries[i].clone(), parent: None })
                    .collect()
            }
        };
        // A typed name picks its first match, so typing it and Enter opens it.
        let first = (!typed.is_empty() && !rows.is_empty()).then_some(0);

        // Held moves first, in the order they were pressed.
        moves.extend(keys.down.then_some(true));
        moves.extend(keys.up.then_some(false));
        for down in moves {
            if down && !rows.is_empty() {
                self.picked = Some(self.picked.or(first).map_or(0, |p| (p + 1).min(rows.len() - 1)));
            }
            if !down {
                self.picked = self.picked.or(first).and_then(|p| p.checked_sub(1));
            }
        }
        let picked = self.picked.or(first);
        if keys.tab
            && let Some(row) = rows.get(picked.unwrap_or(0))
        {
            // The full path, also when only a name was typed.
            let prefix = match &row.parent {
                Some(parent) => initial_text(parent),
                None if folder_text.is_empty() => initial_text(current),
                None => folder_text,
            };
            self.set_text(complete(&prefix, &row.entry), Cursor::End);
            return None;
        }
        if keys.enter || keys.ctrl_enter {
            let (path, select) = match picked.and_then(|p| rows.get(p)) {
                Some(row) => entry_target(&row.entry),
                None => typed_target(&self.text, current, home),
            };
            self.close();
            return Some(use_target(path, select, keys.ctrl_enter));
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
                    // A magnifier while a name is being searched, like File Pilot.
                    if typed.trim().is_empty() {
                        crate::ui::icons::folder(ui);
                    } else {
                        crate::ui::icons::magnifier(ui);
                    }
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
                if let Some(items) = indexing {
                    ui.weak(format!("Indexing… {items} items"));
                }
                let row_height = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical()
                    .max_height(VISIBLE_ROWS * row_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if let Some(error) = &error {
                            ui.weak(error);
                        } else if rows.is_empty() && !(global && indexing.is_some()) {
                            ui.weak("Nothing here matches");
                        }
                        for (shown, row) in rows.iter().enumerate() {
                            let entry = &row.entry;
                            let picked = picked == Some(shown);
                            let line = egui::UiBuilder::new().id(row_id(shown)).sense(egui::Sense::click());
                            let response = ui
                                .scope_builder(line, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    let fill = if picked { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
                                    egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            crate::ui::icons::show(ui, entry, ui.text_style_height(&egui::TextStyle::Body), None);
                                            match &row.parent {
                                                // The name keeps its half of the row, so the folders line up.
                                                Some(parent) => {
                                                    let size = egui::vec2(ui.available_width() / 2.0, ui.text_style_height(&egui::TextStyle::Body));
                                                    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                                        crate::ui::file_list::ranged_name_label(ui, entry, &row.ranges);
                                                    });
                                                    ui.add(egui::Label::new(egui::RichText::new(parent.display().to_string()).weak()).selectable(false).truncate());
                                                }
                                                None => crate::ui::file_list::ranged_name_label(ui, entry, &row.ranges),
                                            }
                                        });
                                    });
                                })
                                .response;
                            if picked {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                let (path, select) = entry_target(entry);
                                event = Some(use_target(path, select, false));
                            }
                        }
                    });
                ui.separator();
                hints(ui);
            });
        if modal.should_close() {
            self.close();
        }
        if matches!(event, Some(PopupEvent::Go { .. } | PopupEvent::Open(_))) {
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

/// Whether the text is a word with no path, searched in every folder.
fn is_bare_word(text: &str) -> bool {
    let text = text.trim();
    // "C:" names a drive on Windows; "." and ".." go to a folder, as before.
    let drive = cfg!(windows) && matches!(text.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic());
    !text.is_empty() && !text.contains(['/', '\\']) && !text.starts_with('~') && !drive && !matches!(text, "." | "..")
}

/// Splits the text after its last separator: the folder part (kept with its
/// separator) and the name being typed.
pub fn split_input(text: &str) -> (&str, &str) {
    match text.rfind(['/', '\\']) {
        Some(at) => text.split_at(at + 1),
        None => ("", text),
    }
}

/// Indices of the entries to list for `typed`: names holding every typed word
/// (see [`word_ranges`]); the name typed in full first, then those starting
/// with the first word, each group in folder order. Hidden entries only when
/// shown, or when typed with a dot.
pub fn matches(entries: &[Entry], typed: &str, show_hidden: bool) -> Vec<usize> {
    let first = typed.split_whitespace().next().unwrap_or("").to_lowercase();
    let hidden_ok = show_hidden || first.starts_with('.');
    let mut rows: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| (hidden_ok || !e.hidden) && word_ranges(&e.name, typed).is_some())
        .map(|(i, _)| i)
        .collect();
    rows.sort_by_key(|&i| {
        let name = &entries[i].name;
        (!is_exact(name, typed), !name.to_lowercase().starts_with(&first))
    });
    rows
}

/// Whether `name` is exactly what was typed, ignoring case and the spaces
/// around it. Nothing typed is no name.
pub fn is_exact(name: &str, typed: &str) -> bool {
    let typed = typed.trim();
    !typed.is_empty() && name.to_lowercase() == typed.to_lowercase()
}

/// Where each typed word is in `name`, as byte ranges sorted by position,
/// or `None` when a word is missing. Words are split on spaces and found in
/// any order, ignoring case and accents; each word takes its own characters, so "a a"
/// needs two a's.
pub fn word_ranges(name: &str, typed: &str) -> Option<Vec<std::ops::Range<usize>>> {
    // Letter by letter, so a byte range in the folded name is one in `name`.
    let fold = crate::fs::search::fold;
    let letters: Vec<(usize, char)> = name.char_indices().map(|(at, c)| (at, fold(c))).collect();
    let mut taken = vec![false; letters.len()];
    let mut ranges = Vec::new();
    for word in typed.split_whitespace() {
        let word: Vec<char> = word.chars().map(fold).collect();
        let fits = |start: usize| {
            start + word.len() <= letters.len()
                && word.iter().enumerate().all(|(k, c)| !taken[start + k] && letters[start + k].1 == *c)
        };
        let start = (0..letters.len()).find(|&start| fits(start))?;
        let end = start + word.len();
        taken[start..end].fill(true);
        let end_byte = letters.get(end).map_or(name.len(), |(at, _)| *at);
        ranges.push(letters[start].0..end_byte);
    }
    ranges.sort_by_key(|r| r.start);
    Some(ranges)
}

/// A row's name with the typed words in `marked`, the rest in `normal`.
pub fn marked_name(name: &str, ranges: &[std::ops::Range<usize>], normal: egui::Color32, marked: egui::Color32, font: egui::FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for range in ranges {
        if range.start > at {
            job.append(&name[at..range.start], 0.0, egui::TextFormat::simple(font.clone(), normal));
        }
        job.append(&name[range.clone()], 0.0, egui::TextFormat::simple(font.clone(), marked));
        at = range.end;
    }
    if at < name.len() || name.is_empty() {
        job.append(&name[at..], 0.0, egui::TextFormat::simple(font, normal));
    }
    job
}

/// Tab: the folder part with `entry`'s name, and a separator after a folder
/// so its contents show next.
pub fn complete(folder: &str, entry: &Entry) -> String {
    let separator = if entry.is_dir { std::path::MAIN_SEPARATOR_STR } else { "" };
    format!("{folder}{}{separator}", entry.name)
}

/// What using a target does: a file (a folder with a name to select) opens in
/// its app, like Enter in the list; with Ctrl+Enter its folder opens in a new
/// tab with it selected. A folder just opens.
fn use_target(path: PathBuf, select: Option<String>, new_tab: bool) -> PopupEvent {
    match select {
        Some(name) if !new_tab => PopupEvent::Open(path.join(name)),
        select => PopupEvent::Go { path, select, new_tab },
    }
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
        frame(ctx, events, |ui| popup.show(ui.ctx(), SCREEN, dir, None, false, Vec::new))
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
        assert_eq!(used, Some(PopupEvent::Open(dir.join("a.txt"))), "a file opens in its app");
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
            frame(&ctx, vec![], |ui| popup.show(ui.ctx(), list, &dir, None, false, Vec::new));
        }
        let area = ctx.memory(|m| m.area_rect(egui::Id::new("lumen_path_popup_area"))).expect("shown");
        assert!((area.center().x - SCREEN.center().x).abs() < 1.0, "centred: {area:?}");
        assert!((area.width() - 0.75 * SCREEN.width()).abs() < 1.0, "75% wide: {area:?}");
        assert!(area.top() >= list.top(), "below the toolbar: {area:?}");
    }

    #[test]
    fn a_typed_file_opens_and_ctrl_enter_shows_it_in_a_new_tab() {
        let (ctx, mut popup, dir) = opened("popup-file");
        step(&ctx, &mut popup, &dir, vec![text(&format!("{}/b.txt", dir.display()))]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, Some(PopupEvent::Open(dir.join("b.txt"))));
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        step(&ctx, &mut popup, &dir, vec![text("b.t")]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::COMMAND)]);
        assert_eq!(used, go(dir.clone(), Some("b.txt"), true), "its folder in a new tab, selected");
        assert!(!popup.is_open());
    }

    #[test]
    fn a_click_on_a_file_row_opens_it() {
        let (ctx, mut popup, dir) = opened("popup-click-file");
        step(&ctx, &mut popup, &dir, vec![]);
        let row = ctx.read_response(row_id(1)).expect("drawn").rect.center(); // sub/, a.txt, b.txt
        assert_eq!(
            click(&ctx, row, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false, Vec::new)),
            Some(PopupEvent::Open(dir.join("a.txt")))
        );
        assert!(!popup.is_open());
    }

    #[test]
    fn down_picks_a_row_and_enter_uses_it() {
        let (ctx, mut popup, dir) = opened("popup-down");
        step(&ctx, &mut popup, &dir, vec![text("t")]); // a.txt, b.txt contain "t"
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowUp, Modifiers::NONE)]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, Some(PopupEvent::Open(dir.join("a.txt"))), "a file opens in its app");
    }

    #[test]
    fn tab_completes_the_first_row_and_ctrl_enter_opens_a_new_tab() {
        let (ctx, mut popup, dir) = opened("popup-tab");
        step(&ctx, &mut popup, &dir, vec![text("su")]);
        step(&ctx, &mut popup, &dir, vec![key(Key::Tab, Modifiers::NONE)]);
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(popup.text(), format!("{}{sep}sub{sep}", dir.display()));
        assert!(popup.is_open(), "Tab does not leave the box");
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::COMMAND)]);
        assert_eq!(used, go(dir.join("sub"), None, true));
    }

    use crate::fs::index::{Index, Rules, Scope, crawl};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use eframe::egui::{Color32, epaint::ClippedShape};

    /// One frame with these events, and what it painted.
    fn shown(ctx: &egui::Context, popup: &mut PathPopup, dir: &Path, events: Vec<Event>, frecent: &[(PathBuf, f64)]) -> (Option<PopupEvent>, Vec<ClippedShape>) {
        let mut event = None;
        let input = egui::RawInput { events, screen_rect: Some(SCREEN), ..egui::RawInput::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| event = popup.show(ui.ctx(), SCREEN, dir, None, false, || frecent.to_vec()));
        });
        output.textures_delta.clear();
        (event, output.shapes)
    }

    /// Types `query` and runs frames until the search it started has answered;
    /// returns what one more frame painted (rows are read back from the frame before).
    fn typed(ctx: &egui::Context, popup: &mut PathPopup, dir: &Path, query: &str, frecent: &[(PathBuf, f64)]) -> Vec<ClippedShape> {
        shown(ctx, popup, dir, vec![text(query)], frecent);
        answered(ctx, popup, dir, frecent)
    }

    /// Runs frames until the search asked for has answered.
    fn answered(ctx: &egui::Context, popup: &mut PathPopup, dir: &Path, frecent: &[(PathBuf, f64)]) -> Vec<ClippedShape> {
        for _ in 0..400 {
            shown(ctx, popup, dir, vec![], frecent);
            if !popup.is_searching() {
                return shown(ctx, popup, dir, vec![], frecent).1;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the search never answered");
    }

    /// What was painted inside row `i`, left to right: each text with its
    /// pieces and their colours.
    type RowTexts = Vec<(String, Vec<(String, Color32)>)>;

    fn rows(ctx: &egui::Context, shapes: &[ClippedShape]) -> Vec<RowTexts> {
        let mut rows = Vec::new();
        while let Some(response) = ctx.read_response(row_id(rows.len())) {
            let mut texts: Vec<_> = shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if response.rect.contains(t.pos) => Some(t),
                    _ => None,
                })
                .collect();
            texts.sort_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
            rows.push(
                texts
                    .iter()
                    .map(|t| {
                        let job = &t.galley.job;
                        let pieces = job.sections.iter().map(|s| (job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.color));
                        (t.galley.text().to_string(), pieces.collect())
                    })
                    .collect(),
            );
        }
        rows
    }

    fn painted_texts(shapes: &[ClippedShape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The names in the global rows.
    fn names(ctx: &egui::Context, shapes: &[ClippedShape]) -> Vec<String> {
        rows(ctx, shapes).iter().map(|r| r[0].0.clone()).collect()
    }

    /// The box opened over a small crawled tree: Downloads/ (song.mp3), Windows NT/,
    /// docs/ (down-notes.txt, win-notes.txt), readme.md. `done`: whether the
    /// crawl counts as finished. Also the number of entries indexed.
    fn indexed(name: &str, done: bool) -> (egui::Context, PathPopup, PathBuf, usize) {
        indexed_with(name, done, Box::new(|_| Crawler::fixed(Index::default(), true)))
    }

    /// [`indexed`] with the crawl that renews the index chosen.
    fn indexed_with(name: &str, done: bool, restart: Restart) -> (egui::Context, PathPopup, PathBuf, usize) {
        let dir = crate::fs::ops::tests::fresh_fixture(name);
        for folder in ["Downloads", "Windows NT", "docs"] {
            std::fs::create_dir(dir.join(folder)).unwrap();
        }
        for file in ["Downloads/song.mp3", "docs/down-notes.txt", "docs/win-notes.txt", "readme.md"] {
            std::fs::write(dir.join(file), "x").unwrap();
        }
        let index = std::sync::RwLock::new(Index::default());
        crawl(std::slice::from_ref(&dir), &Rules::default(), &index, &std::sync::atomic::AtomicBool::new(false));
        let index = index.into_inner().unwrap();
        let entries = index.len();
        let ctx = egui::Context::default();
        let mut popup = PathPopup::with_crawler(Crawler::fixed(index, done), restart);
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        (ctx, popup, dir, entries)
    }

    #[test]
    fn only_a_word_with_no_path_is_searched_in_every_folder() {
        assert!(is_bare_word("downloads"));
        assert!(is_bare_word("  down nt "), "several words are still no path");
        assert!(!is_bare_word(""));
        assert!(!is_bare_word("sub/"));
        assert!(!is_bare_word(r"a\b"));
        assert!(!is_bare_word("/home/u/"));
        assert!(!is_bare_word("~"));
        assert!(!is_bare_word("~/Do"));
        assert!(!is_bare_word(".."), "up one folder, as before");
        assert_eq!(is_bare_word("C:"), cfg!(not(windows)), "a bare drive is a path on Windows");
    }

    #[test]
    fn a_word_with_no_path_lists_matches_from_every_folder_visited_first_with_letters_marked_and_the_parent_weaker() {
        let (ctx, mut popup, dir, _) = indexed("popup-global", true);
        let frecent = [(dir.join("Windows NT"), 5.0)];
        let shapes = typed(&ctx, &mut popup, &dir, "down", &frecent);
        assert_eq!(names(&ctx, &shapes), ["Windows NT", "Downloads", "down-notes.txt"], "the visited folder, then the best matches");
        let visuals = ctx.global_style().visuals.clone();
        let rows = rows(&ctx, &shapes);
        let marked = |row: &RowTexts| row[0].1.iter().filter(|(_, c)| *c == visuals.hyperlink_color).map(|(p, _)| p.clone()).collect::<Vec<_>>();
        assert_eq!(marked(&rows[0]), ["dow", "N"], "letters apart are marked apart");
        assert_eq!(marked(&rows[1]), ["Down"]);
        assert_eq!(rows[1][1].0, dir.display().to_string(), "each row also shows its folder");
        assert_eq!(rows[2][1].0, dir.join("docs").display().to_string());
        assert_eq!(rows[1][1].1[0].1, visuals.weak_text_color(), "weaker than the name");
    }

    #[test]
    fn a_typed_path_still_filters_that_folder() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-path", true);
        let shapes = typed(&ctx, &mut popup, &dir, &format!("{}/win", dir.display()), &[]);
        let rows = rows(&ctx, &shapes);
        assert_eq!(rows.len(), 1, "only that folder's match, not docs/win-notes.txt");
        assert_eq!(rows[0].iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["Windows NT"], "no parent folder on a folder row");
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, go(dir.join("Windows NT"), None, false));
    }

    #[test]
    fn enter_uses_a_global_row_a_folder_opens_and_a_file_opens_in_its_app() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-enter", true);
        typed(&ctx, &mut popup, &dir, "down", &[]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, go(dir.join("Downloads"), None, false), "the first row is picked");
        assert!(!popup.is_open());
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        typed(&ctx, &mut popup, &dir, "down", &[]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(used, Some(PopupEvent::Open(dir.join("docs/down-notes.txt"))));
    }

    #[test]
    fn ctrl_enter_opens_a_global_row_in_a_new_tab_a_file_with_its_folder_selected() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-ctrl", true);
        typed(&ctx, &mut popup, &dir, "down", &[]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::COMMAND)]);
        assert_eq!(used, go(dir.join("docs"), Some("down-notes.txt"), true));
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        typed(&ctx, &mut popup, &dir, "down", &[]);
        let used = step(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::COMMAND)]);
        assert_eq!(used, go(dir.join("Downloads"), None, true));
    }

    #[test]
    fn tab_completes_to_the_full_path_of_a_global_row_with_a_separator_after_a_folder() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-tab", true);
        let sep = std::path::MAIN_SEPARATOR;
        typed(&ctx, &mut popup, &dir, "down", &[]);
        step(&ctx, &mut popup, &dir, vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(popup.text(), format!("{}{sep}Downloads{sep}", dir.display()));
        assert!(popup.is_open(), "Tab does not leave the box");
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        typed(&ctx, &mut popup, &dir, "down", &[]);
        step(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        step(&ctx, &mut popup, &dir, vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(popup.text(), format!("{}{sep}docs{sep}down-notes.txt", dir.display()), "a file gets no separator");
    }

    #[test]
    fn a_click_on_a_global_row_uses_it() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-click", true);
        typed(&ctx, &mut popup, &dir, "down", &[]);
        let row = ctx.read_response(row_id(1)).expect("drawn").rect.center();
        let used = click(&ctx, row, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false, Vec::new));
        assert_eq!(used, Some(PopupEvent::Open(dir.join("docs/down-notes.txt"))));
        assert!(!popup.is_open());
    }

    #[test]
    fn the_indexing_line_and_partial_results_show_while_the_crawl_runs_and_the_line_goes_when_done() {
        let (ctx, mut popup, dir, entries) = indexed("popup-global-indexing", false);
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert!(painted_texts(&shapes).contains(&format!("Indexing… {entries} items")), "{:?}", painted_texts(&shapes));
        assert_eq!(names(&ctx, &shapes)[0], "Downloads", "what is found so far is listed");
        let (ctx, mut popup, dir, _) = indexed("popup-global-indexed", true);
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert!(painted_texts(&shapes).iter().all(|t| !t.starts_with("Indexing")), "{:?}", painted_texts(&shapes));
        assert_eq!(names(&ctx, &shapes)[0], "Downloads");
    }

    #[test]
    fn the_results_are_searched_again_when_the_crawl_ends() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-finishes", false);
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert!(!names(&ctx, &shapes).contains(&"download-stash".to_string()));
        {
            // The crawl finds one more folder and ends.
            let crawler = &popup.jump.as_ref().expect("with a crawl").crawler;
            crawler.index().write().unwrap().add(Some(0), "download-stash", true);
            crawler.finish();
        }
        let shapes = answered(&ctx, &mut popup, &dir, &[]);
        assert!(names(&ctx, &shapes).contains(&"download-stash".to_string()), "{:?}", names(&ctx, &shapes));
        assert!(painted_texts(&shapes).iter().all(|t| !t.starts_with("Indexing")), "the line goes with the crawl");
    }

    #[test]
    fn enter_pressed_before_the_answer_lands_uses_the_answer_not_the_word_before() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-held", true);
        shown(&ctx, &mut popup, &dir, vec![text("down")], &[]);
        let (used, _) = shown(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)], &[]);
        assert_eq!(used, None, "the search is still running");
        assert!(popup.is_open());
        let mut used = None;
        for _ in 0..400 {
            used = shown(&ctx, &mut popup, &dir, vec![], &[]).0;
            if used.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(used, go(dir.join("Downloads"), None, false));
    }

    #[test]
    fn up_and_down_pressed_before_the_answer_lands_move_over_the_answer() {
        let (ctx, mut popup, dir, _) = indexed("popup-global-held-arrows", true);
        shown(&ctx, &mut popup, &dir, vec![text("down")], &[]);
        shown(&ctx, &mut popup, &dir, vec![key(Key::ArrowDown, Modifiers::NONE)], &[]);
        assert!(popup.is_searching(), "the arrow came before the answer");
        // One key a frame: each frame consumes one press of a key.
        for arrow in [Key::ArrowDown, Key::ArrowUp] {
            shown(&ctx, &mut popup, &dir, vec![key(arrow, Modifiers::NONE)], &[]);
        }
        for _ in 0..400 {
            if !popup.is_searching() {
                break;
            }
            shown(&ctx, &mut popup, &dir, vec![], &[]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (used, _) = shown(&ctx, &mut popup, &dir, vec![key(Key::Enter, Modifiers::NONE)], &[]);
        assert_eq!(used, Some(PopupEvent::Open(dir.join("docs/down-notes.txt"))), "down, down, up: the second row");
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
        assert_eq!(click(&ctx, outside, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false, Vec::new)), None);
        assert!(!popup.is_open());
    }

    #[test]
    fn a_click_on_a_row_uses_it_and_the_buttons_work() {
        let (ctx, mut popup, dir) = opened("popup-click");
        let at = |id| ctx.read_response(id).expect("drawn").rect.center();
        let up = at(button_id("up"));
        click(&ctx, up, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false, Vec::new));
        assert_eq!(popup.text(), initial_text(dir.parent().unwrap()), "↑ edits the field to the parent");
        assert!(popup.is_open());
        let back = at(button_id("back"));
        assert_eq!(click(&ctx, back, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false, Vec::new)), Some(PopupEvent::Back));
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        step(&ctx, &mut popup, &dir, vec![]);
        let row = at(row_id(0)); // sub/, folders first
        assert_eq!(click(&ctx, row, |ui| popup.show(ui.ctx(), SCREEN, &dir, None, false, Vec::new)), go(dir.join("sub"), None, false));
    }

    #[test]
    fn the_box_starts_with_the_folder_and_a_separator() {
        #[cfg(unix)]
        {
            assert_eq!(initial_text(Path::new("/home/u/Descargas")), "/home/u/Descargas/");
            assert_eq!(initial_text(Path::new("/")), "/");
        }
        #[cfg(windows)]
        {
            assert_eq!(initial_text(Path::new(r"C:\Users\u")), r"C:\Users\u\");
            assert_eq!(initial_text(Path::new(r"C:\")), r"C:\");
        }
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
    fn every_word_is_found_in_any_order_and_marked() {
        assert_eq!(word_ranges("Descargas", "des"), Some(vec![std::ops::Range { start: 0, end: 3 }]));
        assert_eq!(word_ranges("Descargas", "des gas"), Some(vec![0..3, 6..9]));
        assert_eq!(word_ranges("Descargas", "GAS  des "), Some(vec![0..3, 6..9]), "any order, extra spaces");
        assert_eq!(word_ranges("revision tareas.pdf", "tareas revision"), Some(vec![0..8, 9..15]));
        assert_eq!(word_ranges("Descargas", "des xyz"), None, "every word must be there");
        assert_eq!(word_ranges("banana", "an an"), Some(vec![1..3, 3..5]), "each word its own letters");
        assert_eq!(word_ranges("ban", "an an"), None);
        assert_eq!(word_ranges("Vídeos", "VÍD"), Some(vec![std::ops::Range { start: 0, end: 4 }]), "byte ranges around a multibyte letter");
        assert_eq!(word_ranges("Descargas", ""), Some(vec![]), "nothing typed: everything matches");
        assert_eq!(word_ranges("Vídeos", "vid"), Some(vec![std::ops::Range { start: 0, end: 4 }]), "accents ignored");
        assert_eq!(word_ranges("Videos", "vÍd"), Some(vec![std::ops::Range { start: 0, end: 3 }]), "either way");
    }

    #[test]
    fn only_the_matched_letters_are_coloured() {
        let (normal, marked) = (egui::Color32::GRAY, egui::Color32::LIGHT_BLUE);
        let job = marked_name("Descargas", &[0..3, 6..9], normal, marked, egui::FontId::default());
        let parts: Vec<(&str, egui::Color32)> =
            job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.color)).collect();
        assert_eq!(parts, [("Des", marked), ("car", normal), ("gas", marked)]);
        let plain = marked_name("Vídeos", &[], normal, marked, egui::FontId::default());
        assert_eq!(plain.sections.len(), 1);
        assert_eq!(plain.text, "Vídeos");
    }

    #[test]
    fn rows_match_words_in_any_order() {
        let entries = [e("Descargas", true), e("revision tareas.pdf", false), e("tareas.txt", false)];
        let names = |idx: Vec<usize>| idx.into_iter().map(|i| entries[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(matches(&entries, "tareas revision", false)), ["revision tareas.pdf"]);
        assert_eq!(names(matches(&entries, "tar", false)), ["tareas.txt", "revision tareas.pdf"], "starting with it first");
        assert_eq!(names(matches(&entries, "des gas", false)), ["Descargas"]);
    }

    #[test]
    fn an_exact_name_comes_first() {
        let entries = [e("f6-config", true), e("verify-f6", true), e("F6", false)];
        let names = |idx: Vec<usize>| idx.into_iter().map(|i| entries[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(matches(&entries, "f6", false)), ["F6", "f6-config", "verify-f6"]);
    }

    #[test]
    fn tab_completes_a_folder_with_a_separator_and_a_file_without() {
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(complete("/home/u/", &e("Descargas", true)), format!("/home/u/Descargas{sep}"));
        assert_eq!(complete("/home/u/", &e("desc.txt", false)), "/home/u/desc.txt");
        assert_eq!(complete("", &e("Descargas", true)), format!("Descargas{sep}"));
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

    /// A re-crawl that never ends, and how many times one was started.
    fn endless() -> (Arc<AtomicUsize>, Restart) {
        let starts = Arc::new(AtomicUsize::new(0));
        let counter = starts.clone();
        let restart: Restart = Box::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Crawler::fixed(Index::default(), false)
        });
        (starts, restart)
    }

    /// Closes the box and opens it again on a frame `time` seconds into the session.
    fn reopened_at(ctx: &egui::Context, popup: &mut PathPopup, dir: &Path, time: f64) {
        popup.close();
        popup.open();
        let input = egui::RawInput { time: Some(time), screen_rect: Some(SCREEN), ..egui::RawInput::default() };
        ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| popup.show(ui.ctx(), SCREEN, dir, None, false, Vec::new));
        })
        .drop_without_applying_deltas();
        // The field takes focus on the first frame; only then does it take typing.
        step(ctx, popup, dir, vec![]);
    }

    #[test]
    fn opening_the_box_on_an_index_older_than_ten_minutes_starts_a_recrawl_and_the_old_results_show_meanwhile() {
        let (starts, restart) = endless();
        let (ctx, mut popup, dir, _) = indexed_with("popup-stale-age", true, restart);
        assert_eq!(starts.load(Ordering::SeqCst), 0, "the index is as old as the app: nothing to renew");
        reopened_at(&ctx, &mut popup, &dir, 601.0);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(popup.is_recrawling());
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert_eq!(names(&ctx, &shapes)[0], "Downloads", "the old index keeps answering");
        assert!(painted_texts(&shapes).iter().all(|t| !t.starts_with("Indexing")), "{:?}", painted_texts(&shapes));
        reopened_at(&ctx, &mut popup, &dir, 1300.0);
        assert_eq!(starts.load(Ordering::SeqCst), 1, "one crawl at a time");
    }

    #[test]
    fn a_file_created_after_the_first_crawl_is_found_once_the_recrawl_ends() {
        let dir = crate::fs::ops::tests::fresh_fixture("popup-stale-found");
        std::fs::create_dir(dir.join("Downloads")).unwrap();
        let roots = vec![dir.clone()];
        let restart: Restart = Box::new(move |ctx| {
            let ctx = ctx.clone();
            let scope = Scope { roots: roots.clone(), ..Scope::default() };
            Crawler::start(move || scope, move || ctx.request_repaint())
        });
        let ctx = egui::Context::default();
        let first = restart(&ctx);
        while !first.is_done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut popup = PathPopup::with_crawler(first, restart);
        std::fs::write(dir.join("download-new.txt"), "x").unwrap();
        popup.open();
        step(&ctx, &mut popup, &dir, vec![]);
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert!(!names(&ctx, &shapes).contains(&"download-new.txt".to_string()), "the first crawl did not see it");
        reopened_at(&ctx, &mut popup, &dir, 601.0);
        for _ in 0..400 {
            shown(&ctx, &mut popup, &dir, vec![], &[]);
            if !popup.is_recrawling() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!popup.is_recrawling(), "the re-crawl never ended");
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert!(names(&ctx, &shapes).contains(&"download-new.txt".to_string()), "{:?}", names(&ctx, &shapes));
        assert!(names(&ctx, &shapes).contains(&"Downloads".to_string()));
    }

    #[test]
    fn a_marked_index_is_renewed_at_the_next_opening_even_if_young() {
        let (starts, restart) = endless();
        let (ctx, mut popup, dir, _) = indexed_with("popup-stale-marked", true, restart);
        popup.mark_stale();
        reopened_at(&ctx, &mut popup, &dir, 5.0);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(popup.is_recrawling());
    }

    #[test]
    fn a_young_index_nobody_changed_is_not_crawled_again_on_opening() {
        let (starts, restart) = endless();
        let (ctx, mut popup, dir, _) = indexed_with("popup-stale-fresh", true, restart);
        reopened_at(&ctx, &mut popup, &dir, 599.0);
        reopened_at(&ctx, &mut popup, &dir, 599.0);
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert!(!popup.is_recrawling());
    }

    #[test]
    fn the_first_crawl_is_never_restarted_and_keeps_its_indexing_line() {
        let (starts, restart) = endless();
        let (ctx, mut popup, dir, entries) = indexed_with("popup-stale-first", false, restart);
        popup.mark_stale();
        reopened_at(&ctx, &mut popup, &dir, 9999.0);
        assert_eq!(starts.load(Ordering::SeqCst), 0, "there is no finished index to renew yet");
        let shapes = typed(&ctx, &mut popup, &dir, "down", &[]);
        assert!(painted_texts(&shapes).contains(&format!("Indexing… {entries} items")), "{:?}", painted_texts(&shapes));
    }
}
