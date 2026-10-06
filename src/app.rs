//! Application state and the per-frame UI loop.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::fs::{self, Entry, Lister, Listing};
use crate::input::{self, Action};
use crate::ui::{self, file_list::ListEvent, path_bar::PathBar, status_bar::Status};

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
    pub last_elapsed: Option<Duration>,
    pub error: Option<String>,
    /// Set when the selection changed via keyboard, so the table scrolls to it.
    pub scroll_to_selected: bool,
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
                self.rebuild_rows(None);
                let wanted = select_name.and_then(|name| self.row_of_name(name));
                self.selected = wanted.or((!self.rows.is_empty()).then_some(0));
                self.scroll_to_selected = true;
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    pub fn toggle_hidden(&mut self) {
        let keep = self.selected_entry().map(|e| e.name.clone());
        self.show_hidden = !self.show_hidden;
        self.rebuild_rows(keep.as_deref());
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
        self.scroll_to_selected = true;
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
        self.selected = keep_name
            .and_then(|name| self.row_of_name(name))
            .or((!self.rows.is_empty()).then_some(0));
    }

    fn row_of_name(&self, name: &str) -> Option<usize> {
        self.rows.iter().position(|&i| self.entries[i].name == name)
    }
}

pub struct RumboApp {
    view: View,
    lister: Lister,
    path_bar: PathBar,
    /// Id of the listing currently on screen (or the last one that failed).
    shown_id: u64,
    /// Entry to select once the pending listing arrives.
    select_after_load: Option<String>,
}

impl RumboApp {
    pub fn new(ctx: &egui::Context, start: PathBuf, log: bool) -> Self {
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
        }
    }
}

impl eframe::App for RumboApp {
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
            let scroll = std::mem::take(&mut self.view.scroll_to_selected);
            let event = ui::file_list::show(
                ui,
                &self.view.entries,
                &self.view.rows,
                self.view.selected,
                scroll,
            );
            match event {
                Some(ListEvent::Select(i)) => self.view.selected = Some(i),
                Some(ListEvent::Activate(i)) => {
                    self.view.selected = Some(i);
                    self.activate_selected(&ctx);
                }
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
        assert!(v.scroll_to_selected);
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
