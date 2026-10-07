//! Application state and the per-frame UI loop.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::fs::{self, Entry, Lister, Listing};
use crate::input::{self, Action};
use crate::ui::{self, file_list::ListEvent, path_bar::PathBar, status_bar::Status};

/// How the table should bring the selection into view on the next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Short glide, for keyboard moves within the list.
    Smooth,
    /// Jump, for when the whole list changes (new folder, Ctrl+H); animating
    /// there makes rows appear to trickle in.
    Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Modified,
}

/// Column order of the list. Folders always stay above files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sort {
    pub column: SortColumn,
    pub descending: bool,
}

impl Sort {
    /// A header click: the same column flips direction, another starts ascending.
    fn clicked(self, column: SortColumn) -> Self {
        Self {
            column,
            descending: column == self.column && !self.descending,
        }
    }
}

/// Pause after which type-to-search starts a new word.
pub const SEARCH_RESET: Duration = Duration::from_secs(1);

/// What is on screen. Pure data, no egui or threads, so it is unit-testable.
#[derive(Default)]
pub struct View {
    /// Folder whose entries are shown. Only changes when a listing succeeds.
    pub current: PathBuf,
    pub entries: Vec<Entry>,
    /// Indices into `entries` that pass the hidden-files filter, in display order.
    pub rows: Vec<usize>,
    /// Index into `rows`.
    pub selected: Option<usize>,
    pub show_hidden: bool,
    /// Kept across folders, like the hidden-files toggle.
    pub sort: Sort,
    pub last_elapsed: Option<Duration>,
    pub error: Option<String>,
    /// Set when the selection changed via keyboard, so the table scrolls to it.
    pub scroll: Option<Scroll>,
    /// Type-to-search: lowercase prefix typed so far and when the last key came.
    search: String,
    search_at: Option<Instant>,
}

impl View {
    /// Applies a finished listing. On error the previous folder stays on screen.
    /// `select_name` re-selects an entry, e.g. the folder we just left with Backspace.
    pub fn apply_listing(&mut self, listing: Listing, select_name: Option<&str>) {
        match listing.result {
            Ok(entries) => {
                self.current = listing.path;
                self.entries = entries;
                self.last_elapsed = Some(listing.elapsed);
                self.error = None;
                self.search.clear();
                self.rebuild_rows(None);
                let wanted = select_name.and_then(|name| self.row_of_name(name));
                self.selected = wanted.or((!self.rows.is_empty()).then_some(0));
                self.scroll = Some(Scroll::Instant);
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    pub fn toggle_hidden(&mut self) {
        let keep = self.selected_entry().map(|e| e.name.clone());
        self.show_hidden = !self.show_hidden;
        self.rebuild_rows(keep.as_deref());
        self.scroll = Some(Scroll::Instant);
    }

    pub fn sort_by(&mut self, column: SortColumn) {
        let keep = self.selected_entry().map(|e| e.name.clone());
        self.sort = self.sort.clicked(column);
        self.rebuild_rows(keep.as_deref());
        self.scroll = Some(Scroll::Instant);
    }

    /// Selects the first visible row whose name starts with what was typed.
    /// Takes `now` so tests control the clock.
    pub fn type_char(&mut self, c: char, now: Instant) {
        if self.search_at.is_none_or(|at| now.duration_since(at) >= SEARCH_RESET) {
            self.search.clear();
        }
        self.search_at = Some(now);
        self.search.extend(c.to_lowercase());
        let found = self.rows.iter().position(|&i| {
            self.entries[i].name.to_lowercase().starts_with(&self.search)
        });
        if let Some(row) = found {
            self.selected = Some(row);
            self.scroll = Some(Scroll::Smooth);
        }
    }

    pub fn move_selection(&mut self, action: Action) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            self.selected = None;
            return;
        };
        let cur = self.selected.unwrap_or(0);
        self.selected = Some(match action {
            Action::Move(delta) => cur.saturating_add_signed(delta).min(last),
            Action::First => 0,
            Action::Last => last,
            _ => cur,
        });
        self.scroll = Some(Scroll::Smooth);
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.selected
            .and_then(|i| self.rows.get(i))
            .map(|&idx| &self.entries[idx])
    }

    pub fn hidden_count(&self) -> usize {
        self.entries.iter().filter(|e| e.hidden).count()
    }

    fn rebuild_rows(&mut self, keep_name: Option<&str>) {
        let show_hidden = self.show_hidden;
        self.rows = (0..self.entries.len())
            .filter(|&i| show_hidden || !self.entries[i].hidden)
            .collect();
        let (entries, sort) = (&self.entries, self.sort);
        self.rows.sort_by(|&a, &b| compare(entries, sort, a, b));
        self.selected = keep_name
            .and_then(|name| self.row_of_name(name))
            .or((!self.rows.is_empty()).then_some(0));
    }

    fn row_of_name(&self, name: &str) -> Option<usize> {
        self.rows.iter().position(|&i| self.entries[i].name == name)
    }
}

/// Orders two indices into `entries`. The lister already delivers entries
/// folders-first and by case-insensitive name, so the index itself is the name
/// order: it sorts the Name column and breaks ties without allocating.
fn compare(entries: &[Entry], sort: Sort, a: usize, b: usize) -> Ordering {
    let (ea, eb) = (&entries[a], &entries[b]);
    let by_column = match sort.column {
        SortColumn::Name => a.cmp(&b),
        // `None` (folders, unreadable metadata) sorts before any value.
        SortColumn::Size => ea.size.cmp(&eb.size),
        SortColumn::Modified => ea.modified.cmp(&eb.modified),
    };
    let by_column = if sort.descending { by_column.reverse() } else { by_column };
    eb.is_dir
        .cmp(&ea.is_dir)
        .then(by_column)
        .then(a.cmp(&b))
}

pub struct LumenApp {
    view: View,
    lister: Lister,
    path_bar: PathBar,
    /// Id of the listing currently on screen (or the last one that failed).
    shown_id: u64,
    /// Entry to select once the pending listing arrives.
    select_after_load: Option<String>,
}

/// Global look and feel. Scroll animations are kept subtle (50-90 ms) so the
/// keyboard feels instant; list-wide changes skip them entirely (see [`Scroll`]).
fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.scroll_animation =
            egui::style::ScrollAnimation::new(2000.0, egui::Rangef::new(0.05, 0.09));
    });
}

impl LumenApp {
    pub fn new(ctx: &egui::Context, start: PathBuf, log: bool) -> Self {
        apply_style(ctx);
        let mut app = Self {
            view: View { current: start.clone(), ..View::default() },
            lister: Lister::new(log),
            path_bar: PathBar::default(),
            shown_id: 0,
            select_after_load: None,
        };
        app.navigate(ctx, start, None);
        app
    }

    fn navigate(&mut self, ctx: &egui::Context, path: PathBuf, select: Option<String>) {
        self.select_after_load = select;
        let ctx = ctx.clone();
        // Wake the UI as soon as the worker finishes, without polling in a loop.
        self.lister.request(path, move || ctx.request_repaint());
    }

    fn activate_selected(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.view.selected_entry() else { return };
        let path = entry.path.clone();
        if entry.is_dir {
            self.navigate(ctx, path, None);
        } else if let Err(err) = open::that_detached(&path) {
            self.view.error = Some(format!("Cannot open {}: {err}", path.display()));
        }
    }

    fn go_parent(&mut self, ctx: &egui::Context) {
        let current = self.view.current.clone();
        if let Some(parent) = current.parent() {
            let came_from = current.file_name().map(|n| n.to_string_lossy().into_owned());
            self.navigate(ctx, parent.to_path_buf(), came_from);
        }
    }

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::FocusPathBar => self.path_bar.request_focus(),
            Action::ToggleHidden => self.view.toggle_hidden(),
            Action::Activate => self.activate_selected(ctx),
            Action::GoParent => self.go_parent(ctx),
            Action::Move(_) | Action::First | Action::Last => self.view.move_selection(action),
            Action::TypeChar(c) => self.view.type_char(c, Instant::now()),
        }
    }
}

impl eframe::App for LumenApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if let Some(listing) = self.lister.poll() {
            self.shown_id = listing.id;
            let select = self.select_after_load.take();
            self.view.apply_listing(listing, select.as_deref());
        }

        for action in input::read_actions(&ctx) {
            self.handle_action(&ctx, action);
        }

        egui::Panel::top("path_bar").show(ui, |ui| {
            ui.add_space(4.0);
            let current = self.view.current.clone();
            if let Some(text) = self.path_bar.show(ui, &current) {
                let target = fs::resolve_input_path(&text, &current, fs::home_dir().as_deref());
                self.navigate(&ctx, target, None);
            }
            ui.add_space(2.0);
        });

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui::status_bar::show(
                ui,
                &Status {
                    item_count: self.view.rows.len(),
                    hidden_count: self.view.hidden_count(),
                    show_hidden: self.view.show_hidden,
                    loading: self.lister.is_pending(self.shown_id),
                    last_elapsed: self.view.last_elapsed,
                    error: self.view.error.as_deref(),
                },
            );
        });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            let scroll = self.view.scroll.take();
            let event = ui::file_list::show(
                ui,
                &self.view.entries,
                &self.view.rows,
                self.view.selected,
                scroll,
                self.view.sort,
            );
            match event {
                Some(ListEvent::Select(i)) => self.view.selected = Some(i),
                Some(ListEvent::Activate(i)) => {
                    self.view.selected = Some(i);
                    self.activate_selected(&ctx);
                }
                Some(ListEvent::Sort(column)) => self.view.sort_by(column),
                None => {}
            }
        });
    }
}

/// Folder to open at startup: first CLI argument, else the working directory, else home.
pub fn start_dir(arg: Option<String>) -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let base = cwd.clone().or_else(fs::home_dir).unwrap_or_else(|| PathBuf::from("/"));
    match arg {
        Some(arg) => fs::resolve_input_path(&arg, &base, fs::home_dir().as_deref()),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::lister::ListError;
    use std::path::Path;
    use std::time::Instant;

    #[test]
    fn scroll_animation_is_subtle_in_both_themes() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let anim = ctx.style_of(theme).scroll_animation;
            assert!(anim.duration.min > 0.0, "keyboard scrolling should glide");
            assert!(anim.duration.max <= 0.1, "but never longer than 100 ms");
        }
    }

    #[test]
    fn loading_a_folder_scrolls_instantly() {
        let v = loaded_view();
        assert_eq!(v.scroll, Some(Scroll::Instant));
    }

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            is_dir,
            size: (!is_dir).then_some(1),
            modified: None,
            hidden: name.starts_with('.'),
        }
    }

    fn ok_listing(path: &str, entries: Vec<Entry>) -> Listing {
        Listing {
            id: 1,
            path: PathBuf::from(path),
            result: Ok(entries),
            elapsed: Duration::from_millis(3),
        }
    }

    fn loaded_view() -> View {
        let mut v = View::default();
        let entries = vec![
            entry(".git", true),
            entry("src", true),
            entry(".env", false),
            entry("a.txt", false),
            entry("b.txt", false),
        ];
        v.apply_listing(ok_listing("/x", entries), None);
        v
    }

    fn row_names(v: &View) -> Vec<&str> {
        v.rows.iter().map(|&i| v.entries[i].name.as_str()).collect()
    }

    #[test]
    fn hidden_entries_are_filtered_by_default() {
        let v = loaded_view();
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.selected, Some(0));
        assert_eq!(v.hidden_count(), 2);
        assert_eq!(v.current, Path::new("/x"));
        assert_eq!(v.last_elapsed, Some(Duration::from_millis(3)));
    }

    #[test]
    fn toggle_hidden_shows_them_and_keeps_selection_on_same_entry() {
        let mut v = loaded_view();
        v.selected = Some(1); // a.txt
        v.toggle_hidden();
        assert_eq!(row_names(&v), [".git", "src", ".env", "a.txt", "b.txt"]);
        assert_eq!(v.selected_entry().unwrap().name, "a.txt");
        v.toggle_hidden();
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.selected_entry().unwrap().name, "a.txt");
    }

    #[test]
    fn toggle_hidden_requests_scroll_to_kept_selection() {
        let mut v = loaded_view();
        v.scroll = None;
        v.toggle_hidden();
        assert_eq!(
            v.scroll,
            Some(Scroll::Instant),
            "animating here makes rows trickle in"
        );
    }

    #[test]
    fn hiding_the_selected_entry_falls_back_to_first_row() {
        let mut v = loaded_view();
        v.toggle_hidden();
        v.selected = Some(0); // .git
        v.toggle_hidden();
        assert_eq!(v.selected_entry().unwrap().name, "src");
    }

    #[test]
    fn arrows_move_and_clamp_at_both_ends() {
        let mut v = loaded_view();
        v.move_selection(Action::Move(-1));
        assert_eq!(v.selected, Some(0));
        v.move_selection(Action::Move(1));
        assert_eq!(v.selected, Some(1));
        v.move_selection(Action::Move(20));
        assert_eq!(v.selected, Some(2));
        v.move_selection(Action::First);
        assert_eq!(v.selected, Some(0));
        v.move_selection(Action::Last);
        assert_eq!(v.selected, Some(2));
        assert_eq!(v.scroll, Some(Scroll::Smooth));
    }

    fn file(name: &str, size: Option<u64>, secs: Option<u64>) -> Entry {
        Entry {
            size,
            modified: secs.map(|s| std::time::UNIX_EPOCH + Duration::from_secs(s)),
            ..entry(name, false)
        }
    }

    /// Entries arrive sorted like the lister does: folders first, then by name.
    fn sortable_view() -> View {
        let mut v = View::default();
        let mut dir_b = entry("b_dir", true);
        dir_b.modified = Some(std::time::UNIX_EPOCH + Duration::from_secs(5));
        let entries = vec![
            entry("a_dir", true),
            dir_b,
            file("a.txt", Some(30), Some(1)),
            file("b.txt", Some(10), Some(3)),
            file("c.txt", None, None),
            file("d.txt", Some(10), Some(2)),
        ];
        v.apply_listing(ok_listing("/s", entries), None);
        v
    }

    #[test]
    fn default_sort_is_name_ascending() {
        let v = sortable_view();
        assert_eq!(v.sort, Sort { column: SortColumn::Name, descending: false });
        assert_eq!(row_names(&v), ["a_dir", "b_dir", "a.txt", "b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn clicking_name_again_reverses_but_folders_stay_on_top() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Name);
        assert_eq!(v.sort, Sort { column: SortColumn::Name, descending: true });
        assert_eq!(row_names(&v), ["b_dir", "a_dir", "d.txt", "c.txt", "b.txt", "a.txt"]);
        v.sort_by(SortColumn::Name);
        assert_eq!(row_names(&v), ["a_dir", "b_dir", "a.txt", "b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn size_sorts_ascending_then_descending_ties_by_name() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Size);
        assert_eq!(v.sort, Sort { column: SortColumn::Size, descending: false });
        // Unknown size first; equal sizes (b, d) keep name order.
        assert_eq!(row_names(&v), ["a_dir", "b_dir", "c.txt", "b.txt", "d.txt", "a.txt"]);
        v.sort_by(SortColumn::Size);
        assert_eq!(row_names(&v), ["a_dir", "b_dir", "a.txt", "b.txt", "d.txt", "c.txt"]);
    }

    #[test]
    fn modified_sorts_folders_among_themselves() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Modified);
        assert_eq!(row_names(&v), ["a_dir", "b_dir", "c.txt", "a.txt", "d.txt", "b.txt"]);
        v.sort_by(SortColumn::Modified);
        assert_eq!(row_names(&v), ["b_dir", "a_dir", "b.txt", "d.txt", "a.txt", "c.txt"]);
    }

    #[test]
    fn switching_column_starts_ascending() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Size);
        v.sort_by(SortColumn::Size);
        v.sort_by(SortColumn::Modified);
        assert_eq!(v.sort, Sort { column: SortColumn::Modified, descending: false });
    }

    #[test]
    fn sorting_keeps_selection_on_same_entry_and_scrolls_to_it() {
        let mut v = sortable_view();
        v.selected = Some(2); // a.txt
        v.scroll = None;
        v.sort_by(SortColumn::Size);
        assert_eq!(v.selected_entry().unwrap().name, "a.txt");
        assert_eq!(v.selected, Some(5));
        assert_eq!(v.scroll, Some(Scroll::Instant));
    }

    #[test]
    fn sort_survives_new_listing_and_hidden_toggle() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Size);
        v.sort_by(SortColumn::Size);
        v.apply_listing(
            ok_listing("/t", vec![file("x", Some(1), None), file("y", Some(2), None)]),
            None,
        );
        assert_eq!(row_names(&v), ["y", "x"]);
        v.toggle_hidden();
        assert_eq!(row_names(&v), ["y", "x"]);
    }

    fn search_view() -> View {
        let entries = vec![
            entry(".docker", true),
            entry("Desktop", true),
            entry("Documents", true),
            entry("downloads", true),
            entry("notes.txt", false),
        ];
        let mut v = View::default();
        v.apply_listing(ok_listing("/home/u", entries), None);
        v
    }

    fn type_str(v: &mut View, text: &str, start: Instant, gap: Duration) -> Instant {
        let mut now = start;
        for c in text.chars() {
            v.type_char(c, now);
            now += gap;
        }
        now
    }

    #[test]
    fn typing_jumps_to_first_case_insensitive_prefix_match() {
        let mut v = search_view();
        let t0 = Instant::now();
        type_str(&mut v, "d", t0, Duration::ZERO);
        assert_eq!(v.selected_entry().unwrap().name, "Desktop");
        type_str(&mut v, "oc", t0 + Duration::from_millis(300), Duration::from_millis(300));
        assert_eq!(v.selected_entry().unwrap().name, "Documents");
        assert_eq!(v.scroll, Some(Scroll::Smooth));
    }

    #[test]
    fn pausing_restarts_the_search() {
        let mut v = search_view();
        let t0 = Instant::now();
        let after = type_str(&mut v, "do", t0, Duration::from_millis(200));
        v.type_char('n', after + SEARCH_RESET);
        assert_eq!(v.selected_entry().unwrap().name, "notes.txt");
    }

    #[test]
    fn no_match_keeps_the_selection() {
        let mut v = search_view();
        let t0 = Instant::now();
        type_str(&mut v, "no", t0, Duration::ZERO);
        v.type_char('x', t0);
        assert_eq!(v.selected_entry().unwrap().name, "notes.txt");
    }

    #[test]
    fn hidden_rows_are_not_matched_until_shown() {
        let mut v = search_view();
        let t0 = Instant::now();
        type_str(&mut v, ".d", t0, Duration::ZERO);
        assert_eq!(v.selected_entry().unwrap().name, "Desktop", "selection unchanged");
        v.toggle_hidden();
        type_str(&mut v, ".d", t0 + SEARCH_RESET * 2, Duration::ZERO);
        assert_eq!(v.selected_entry().unwrap().name, ".docker");
    }

    #[test]
    fn changing_folder_restarts_the_search() {
        let mut v = search_view();
        let t0 = Instant::now();
        type_str(&mut v, "do", t0, Duration::ZERO);
        v.apply_listing(ok_listing("/y", vec![entry("notes", true)]), None);
        v.type_char('n', t0);
        assert_eq!(v.selected_entry().unwrap().name, "notes");
    }

    #[test]
    fn empty_folder_has_no_selection() {
        let mut v = View::default();
        v.apply_listing(ok_listing("/empty", vec![]), None);
        v.move_selection(Action::Move(1));
        assert_eq!(v.selected, None);
        assert!(v.selected_entry().is_none());
    }

    #[test]
    fn going_up_reselects_the_folder_we_came_from() {
        let mut v = View::default();
        let entries = vec![entry("a", true), entry("b", true), entry("c", true)];
        v.apply_listing(ok_listing("/x", entries), Some("b"));
        assert_eq!(v.selected_entry().unwrap().name, "b");
    }

    #[test]
    fn failed_listing_keeps_previous_folder_and_shows_error() {
        let mut v = loaded_view();
        v.apply_listing(
            Listing {
                id: 2,
                path: PathBuf::from("/root"),
                result: Err(ListError::PermissionDenied(PathBuf::from("/root"))),
                elapsed: Duration::ZERO,
            },
            None,
        );
        assert_eq!(v.current, Path::new("/x"));
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.error.as_deref(), Some("Permission denied: /root"));
        // The next successful listing clears the error.
        v.apply_listing(ok_listing("/y", vec![entry("z", false)]), None);
        assert_eq!(v.error, None);
        assert_eq!(v.current, Path::new("/y"));
    }
}
