//! Application state and the per-frame UI loop.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::fs::watch::Watcher;
use crate::fs::{self, Entry, Lister, Listing};
use crate::input::{self, Action, Confirm};
use crate::fs::ops::Renamed;
use crate::fs::transfer::{self, Job, Mode, Outcome, Plan};
use crate::ui::{self, file_list::{ListEvent, RenameEdit}, path_bar::PathBar, status_bar::Status};

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

    /// Applies a listing of the folder already shown, after something changed
    /// it. Unlike a navigation it keeps the selection (the same row if the entry
    /// vanished), the type-to-search word, the error line and the scroll.
    pub fn apply_refresh(&mut self, listing: Listing) {
        let entries = match listing.result {
            Ok(entries) => entries,
            Err(err) => {
                self.error = Some(err.to_string());
                return;
            }
        };
        let keep = self.selected_entry().map(|e| e.name.clone());
        let row = self.selected;
        self.entries = entries;
        self.last_elapsed = Some(listing.elapsed);
        self.rebuild_rows(None);
        let last = self.rows.len().checked_sub(1);
        self.selected = keep
            .and_then(|name| self.row_of_name(&name))
            .or_else(|| Some(row?.min(last?)));
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

    /// Entry to select once the selected one is gone: the row below, or the
    /// row above when it was the last one.
    pub fn name_after_removal(&self) -> Option<String> {
        let sel = self.selected?;
        let row = if sel + 1 < self.rows.len() { sel + 1 } else { sel.checked_sub(1)? };
        Some(self.entries[self.rows[row]].name.clone())
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

/// How a navigation was started, so its result updates the history correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKind {
    /// Enter, Backspace, path bar: a new step that clears the forward list.
    New,
    Back,
    Forward,
    /// The open folder changed on disk; not a step in the history.
    Refresh,
}

/// Wait between the first change on disk and the reload. Later changes in that
/// window ride along, so a long copy reloads a few times a second, not per file.
pub const REFRESH_DELAY: Duration = Duration::from_millis(200);

/// Schedules the reload that follows changes on disk.
#[derive(Default)]
pub struct RefreshTimer {
    due: Option<Instant>,
}

impl RefreshTimer {
    /// A change happened; reload at most `REFRESH_DELAY` after the first one.
    pub fn note_change(&mut self, now: Instant) {
        self.due.get_or_insert(now + REFRESH_DELAY);
    }

    /// Whether it is time to reload; clears the schedule when it is.
    pub fn take_due(&mut self, now: Instant) -> bool {
        let due = self.due.is_some_and(|at| now >= at);
        if due {
            self.due = None;
        }
        due
    }

    /// How long until the scheduled reload, to wake the UI then.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.due.map(|at| at.saturating_duration_since(now))
    }
}

/// Browser-style back/forward lists. Only successful listings are recorded.
#[derive(Default)]
pub struct History {
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
}

impl History {
    pub fn back_target(&self) -> Option<&PathBuf> {
        self.back.last()
    }

    pub fn forward_target(&self) -> Option<&PathBuf> {
        self.forward.last()
    }

    /// Updates the lists once a listing started by `kind` finishes. `to` is the
    /// folder now shown, or `None` if the listing failed.
    pub fn settle(&mut self, kind: NavKind, from: &Path, to: Option<&Path>) {
        let Some(to) = to else {
            // A folder that vanished would block Back/Forward forever: drop it.
            match kind {
                NavKind::New | NavKind::Refresh => {}
                NavKind::Back => drop(self.back.pop()),
                NavKind::Forward => drop(self.forward.pop()),
            }
            return;
        };
        if from == to {
            return;
        }
        let from = from.to_path_buf();
        match kind {
            NavKind::Refresh => {}
            NavKind::New => {
                self.back.push(from);
                self.forward.clear();
            }
            NavKind::Back => {
                self.back.pop();
                self.forward.push(from);
            }
            NavKind::Forward => {
                self.forward.pop();
                self.back.push(from);
            }
        }
    }
}

/// When going up from `current` to `target`, the child we left, so it can be
/// re-selected. `None` for any other move.
fn came_from(current: &Path, target: &Path) -> Option<String> {
    (current.parent() == Some(target))
        .then(|| current.file_name())
        .flatten()
        .map(|n| n.to_string_lossy().into_owned())
}

/// A destructive operation waiting for the user's Enter or Escape.
enum Dialog {
    Trash { path: PathBuf, name: String, select: Option<String> },
    /// Renaming onto an existing name; the edit resumes if the user says no.
    Replace(RenameEdit),
    /// Pasting onto an existing name.
    Paste { source: PathBuf, target: PathBuf, mode: Mode },
}

impl Dialog {
    fn message(&self) -> String {
        match self {
            Self::Trash { name, .. } => format!("Move “{name}” to the trash?"),
            Self::Replace(edit) => format!(
                "“{}” already exists. Replace it? The existing one goes to the trash.",
                edit.text
            ),
            Self::Paste { target, .. } => format!(
                "“{}” already exists here. Replace it? The existing one goes to the trash.",
                file_name(target).unwrap_or_default()
            ),
        }
    }

    fn yes_label(&self) -> &'static str {
        match self {
            Self::Trash { .. } => "Move to Trash",
            Self::Replace(_) | Self::Paste { .. } => "Replace",
        }
    }
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Lumen's own clipboard (not the system one): one entry, copied or cut.
#[derive(Clone)]
struct Clip {
    path: PathBuf,
    mode: Mode,
}

/// A paste in progress and where it lands.
struct Running {
    job: Job,
    target: PathBuf,
}

pub struct LumenApp {
    view: View,
    lister: Lister,
    path_bar: PathBar,
    /// Id of the listing currently on screen (or the last one that failed).
    shown_id: u64,
    /// Entry to select once the pending listing arrives.
    select_after_load: Option<String>,
    /// How the pending listing was started.
    pending_nav: NavKind,
    history: History,
    dialog: Option<Dialog>,
    rename: Option<RenameEdit>,
    /// Folder just created: rename it once a listing shows it selected.
    rename_after_load: Option<PathBuf>,
    /// Error to show once the pending reload lands (a reload clears errors).
    error_after_load: Option<String>,
    clipboard: Option<Clip>,
    /// Copy or move running in the background; one at a time.
    job: Option<Running>,
    /// Informational line in the status bar ("1 item copied").
    notice: Option<String>,
    /// Watches the folder on screen; changes schedule a refresh.
    watcher: Watcher,
    refresh: RefreshTimer,
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
            pending_nav: NavKind::New,
            history: History::default(),
            dialog: None,
            rename: None,
            rename_after_load: None,
            error_after_load: None,
            clipboard: None,
            job: None,
            notice: None,
            watcher: Watcher::new({
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            }),
            refresh: RefreshTimer::default(),
        };
        app.navigate(ctx, start, None);
        app
    }

    fn navigate(&mut self, ctx: &egui::Context, path: PathBuf, select: Option<String>) {
        self.navigate_as(ctx, NavKind::New, path, select);
    }

    fn navigate_as(
        &mut self,
        ctx: &egui::Context,
        kind: NavKind,
        path: PathBuf,
        select: Option<String>,
    ) {
        self.pending_nav = kind;
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
            let select = came_from(&current, parent);
            self.navigate(ctx, parent.to_path_buf(), select);
        }
    }

    fn go_history(&mut self, ctx: &egui::Context, kind: NavKind) {
        let target = match kind {
            NavKind::Back => self.history.back_target(),
            NavKind::Forward => self.history.forward_target(),
            NavKind::New | NavKind::Refresh => None,
        };
        if let Some(target) = target.cloned() {
            let select = came_from(&self.view.current, &target);
            self.navigate_as(ctx, kind, target, select);
        }
    }

    /// Lists the current folder again after an operation changed it.
    fn reload(&mut self, ctx: &egui::Context, select: Option<String>) {
        self.navigate(ctx, self.view.current.clone(), select);
    }

    fn ask_trash(&mut self) {
        let Some(entry) = self.view.selected_entry() else { return };
        self.dialog = Some(Dialog::Trash {
            path: entry.path.clone(),
            name: entry.name.clone(),
            select: self.view.name_after_removal(),
        });
    }

    fn start_rename(&mut self) {
        let Some(entry) = self.view.selected_entry() else { return };
        self.rename = Some(RenameEdit::new(entry.path.clone(), &entry.name, entry.is_dir));
    }

    fn new_folder(&mut self, ctx: &egui::Context) {
        match fs::ops::create_folder(&self.view.current) {
            Ok(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.rename_after_load = Some(path);
                self.reload(ctx, name);
            }
            Err(err) => self.view.error = Some(err),
        }
    }

    /// Applies a finished in-place edit. Invalid names keep the field open.
    fn finish_rename(&mut self, ctx: &egui::Context, mut edit: RenameEdit, replace: bool) {
        match fs::ops::rename(&edit.path, &edit.text, replace) {
            Ok(Renamed::To(_)) => {
                self.view.error = None;
                self.reload(ctx, Some(edit.text));
            }
            Ok(Renamed::Unchanged) => {}
            Ok(Renamed::TargetExists) => self.dialog = Some(Dialog::Replace(edit)),
            Err(err) => {
                // A replace may have trashed the target before failing: reload.
                let keep = self.view.selected_entry().map(|e| e.name.clone());
                self.error_after_load = Some(err);
                edit.focus = true;
                self.rename = Some(edit);
                self.reload(ctx, keep);
            }
        }
    }

    fn set_clipboard(&mut self, mode: Mode) {
        let Some(entry) = self.view.selected_entry() else { return };
        self.clipboard = Some(Clip { path: entry.path.clone(), mode });
        self.notice = Some(match mode {
            Mode::Copy => "1 item copied".into(),
            Mode::Move => "1 item cut".into(),
        });
    }

    fn paste(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            self.view.error = Some("Another operation is still running".into());
            return;
        }
        let Some(Clip { path, mode }) = self.clipboard.clone() else {
            self.notice = Some("Nothing to paste: Ctrl+C or Ctrl+X first".into());
            return;
        };
        match transfer::plan(&path, &self.view.current, mode) {
            Ok(Plan::Ready(target)) => self.start_job(ctx, path, target, mode, false),
            Ok(Plan::Nothing) => {}
            Ok(Plan::TargetExists(target)) => {
                self.dialog = Some(Dialog::Paste { source: path, target, mode });
            }
            Err(err) => self.view.error = Some(err),
        }
    }

    fn start_job(
        &mut self,
        ctx: &egui::Context,
        source: PathBuf,
        target: PathBuf,
        mode: Mode,
        replace: bool,
    ) {
        self.notice = None;
        self.view.error = None;
        let ctx = ctx.clone();
        let job = Job::start(source, target.clone(), mode, replace, move || ctx.request_repaint());
        self.job = Some(Running { job, target });
    }

    fn finish_job(&mut self, ctx: &egui::Context, running: Running, result: Result<Outcome, String>) {
        let mode = running.job.mode;
        match result {
            Ok(Outcome::Done) => {
                // The cut entry has moved; pasting it again would find nothing.
                if mode == Mode::Move {
                    self.clipboard = None;
                }
            }
            Ok(Outcome::Cancelled) => {
                self.notice = Some(match mode {
                    Mode::Copy => "Copy cancelled".into(),
                    Mode::Move => "Move cancelled".into(),
                });
            }
            Err(err) => self.error_after_load = Some(err),
        }
        // A navigation started meanwhile shows fresh contents anyway; reloading
        // here would supersede it. A pending refresh may be replaced.
        if self.lister.is_pending(self.shown_id) && self.pending_nav != NavKind::Refresh {
            return;
        }
        let select = if running.target.parent() == Some(self.view.current.as_path()) {
            file_name(&running.target)
        } else {
            self.view.selected_entry().map(|e| e.name.clone())
        };
        self.reload(ctx, select);
    }

    fn answer_dialog(&mut self, ctx: &egui::Context, answer: Confirm) {
        let Some(dialog) = self.dialog.take() else { return };
        if answer == Confirm::No {
            if let Dialog::Replace(mut edit) = dialog {
                edit.focus = true;
                self.rename = Some(edit);
            }
            return;
        }
        match dialog {
            Dialog::Trash { path, name, select } => match fs::ops::trash(&path) {
                Ok(()) => self.reload(ctx, select),
                Err(err) => {
                    self.error_after_load = Some(err);
                    self.reload(ctx, Some(name));
                }
            },
            Dialog::Replace(edit) => self.finish_rename(ctx, edit, true),
            Dialog::Paste { source, target, mode } => {
                self.start_job(ctx, source, target, mode, true);
            }
        }
    }

    /// Reloads the open folder a moment after it changed on disk.
    fn refresh_if_changed(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self.watcher.take_changed() {
            self.refresh.note_change(now);
        }
        if self.refresh.take_due(now) {
            if self.lister.is_pending(self.shown_id) {
                // Never supersede a navigation; try again once it has landed.
                self.refresh.note_change(now);
            } else {
                self.navigate_as(ctx, NavKind::Refresh, self.view.current.clone(), None);
            }
        }
        if let Some(wait) = self.refresh.wait(now) {
            ctx.request_repaint_after(wait);
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
            Action::Back => self.go_history(ctx, NavKind::Back),
            Action::Forward => self.go_history(ctx, NavKind::Forward),
            Action::Trash => self.ask_trash(),
            Action::Rename => self.start_rename(),
            Action::NewFolder => self.new_folder(ctx),
            Action::Copy => self.set_clipboard(Mode::Copy),
            Action::Cut => self.set_clipboard(Mode::Move),
            Action::Paste => self.paste(ctx),
            Action::Cancel => {
                if let Some(running) = &self.job {
                    running.job.cancel();
                }
            }
        }
    }
}

impl eframe::App for LumenApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if let Some(listing) = self.lister.poll() {
            self.shown_id = listing.id;
            let select = self.select_after_load.take();
            let from = self.view.current.clone();
            let to = listing.result.is_ok().then(|| listing.path.clone());
            if self.pending_nav == NavKind::Refresh {
                self.view.apply_refresh(listing);
            } else {
                self.view.apply_listing(listing, select.as_deref());
            }
            self.history.settle(self.pending_nav, &from, to.as_deref());
            if to.is_some() {
                self.watcher.watch(&self.view.current);
            } else if self.pending_nav == NavKind::Refresh {
                // The open folder is gone; watch it afresh if it comes back.
                self.watcher.forget();
            }
            if let Some(err) = self.error_after_load.take() {
                self.view.error = Some(err);
            }
            // The edited entry may be gone (another folder, a reload).
            if let Some(edit) = &self.rename
                && !self.view.entries.iter().any(|e| e.path == edit.path)
            {
                self.rename = None;
            }
            // A faster navigation may have superseded the reload: check the entry.
            if let Some(created) = self.rename_after_load.take()
                && self.view.selected_entry().is_some_and(|e| e.path == created)
            {
                self.start_rename();
            }
        }

        self.refresh_if_changed(&ctx);

        if let Some(result) = self.job.as_mut().and_then(|r| r.job.poll())
            && let Some(running) = self.job.take()
        {
            self.finish_job(&ctx, running, result);
        }

        // An open dialog takes the keyboard; the list must not react underneath.
        if self.dialog.is_some() {
            if let Some(answer) = input::read_confirm(&ctx) {
                self.answer_dialog(&ctx, answer);
            }
        } else {
            for action in input::read_actions(&ctx) {
                self.handle_action(&ctx, action);
            }
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
                    job: self
                        .job
                        .as_ref()
                        .map(|r| ui::status_bar::progress_text(r.job.mode, &r.job.progress)),
                    notice: self.notice.as_deref(),
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
                self.rename.as_mut(),
            );
            match event {
                Some(ListEvent::Select(i)) => self.view.selected = Some(i),
                Some(ListEvent::Activate(i)) => {
                    self.view.selected = Some(i);
                    self.activate_selected(&ctx);
                }
                Some(ListEvent::Sort(column)) => self.view.sort_by(column),
                Some(ListEvent::RenameEnded(submit)) => {
                    if let Some(edit) = self.rename.take()
                        && submit
                    {
                        self.finish_rename(&ctx, edit, false);
                    }
                }
                None => {}
            }
        });

        // A click on another row or a scroll can take focus without ending the
        // edit above (one event per frame); an unfocused field is a cancel.
        if self.rename.as_ref().is_some_and(|e| !e.focus)
            && !ctx.memory(|m| m.has_focus(ui::file_list::rename_id()))
        {
            self.rename = None;
        }

        if let Some(dialog) = &self.dialog
            && let Some(answer) = ui::confirm::show(&ctx, &dialog.message(), dialog.yes_label())
        {
            self.answer_dialog(&ctx, answer);
        }
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

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    /// Simulates the app: a successful listing moving from `from` to `to`.
    fn go(h: &mut History, kind: NavKind, from: &str, to: &str) {
        h.settle(kind, Path::new(from), Some(Path::new(to)));
    }

    #[test]
    fn new_navigation_records_back_and_clears_forward() {
        let mut h = History::default();
        go(&mut h, NavKind::New, "/a", "/b");
        go(&mut h, NavKind::New, "/b", "/c");
        assert_eq!(h.back_target(), Some(&p("/b")));
        go(&mut h, NavKind::Back, "/c", "/b");
        assert_eq!(h.forward_target(), Some(&p("/c")));
        go(&mut h, NavKind::New, "/b", "/d");
        assert_eq!(h.forward_target(), None);
        assert_eq!(h.back_target(), Some(&p("/b")));
    }

    #[test]
    fn back_and_forward_walk_like_a_browser() {
        let mut h = History::default();
        go(&mut h, NavKind::New, "/a", "/b");
        go(&mut h, NavKind::New, "/b", "/c");
        go(&mut h, NavKind::Back, "/c", "/b");
        go(&mut h, NavKind::Back, "/b", "/a");
        assert_eq!(h.back_target(), None);
        assert_eq!(h.forward_target(), Some(&p("/b")));
        go(&mut h, NavKind::Forward, "/a", "/b");
        go(&mut h, NavKind::Forward, "/b", "/c");
        assert_eq!(h.forward_target(), None);
        assert_eq!(h.back_target(), Some(&p("/b")));
    }

    #[test]
    fn failed_or_same_folder_listings_are_not_recorded() {
        let mut h = History::default();
        h.settle(NavKind::New, Path::new("/a"), None);
        go(&mut h, NavKind::New, "/a", "/a"); // startup or reload
        assert_eq!(h.back_target(), None);
        assert_eq!(h.forward_target(), None);
    }

    #[test]
    fn a_vanished_folder_is_dropped_so_back_goes_further() {
        let mut h = History::default();
        go(&mut h, NavKind::New, "/a", "/gone");
        go(&mut h, NavKind::New, "/gone", "/c");
        h.settle(NavKind::Back, Path::new("/c"), None);
        assert_eq!(h.back_target(), Some(&p("/a")));
        assert_eq!(h.forward_target(), None);
    }

    #[test]
    fn came_from_names_the_child_only_when_going_up() {
        assert_eq!(came_from(Path::new("/usr/bin"), Path::new("/usr")).as_deref(), Some("bin"));
        assert_eq!(came_from(Path::new("/usr/bin"), Path::new("/home")), None);
        assert_eq!(came_from(Path::new("/usr"), Path::new("/usr/bin")), None);
    }

    #[test]
    fn after_removal_the_row_below_is_selected() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.selected = Some(1);
        assert_eq!(v.name_after_removal().as_deref(), Some("b.txt"));
        v.selected = Some(0);
        assert_eq!(v.name_after_removal().as_deref(), Some("a.txt"));
    }

    #[test]
    fn after_removing_the_last_row_the_row_above_is_selected() {
        let mut v = loaded_view();
        v.selected = Some(2);
        assert_eq!(v.name_after_removal().as_deref(), Some("a.txt"));
        let mut single = View::default();
        single.apply_listing(ok_listing("/one", vec![entry("only", false)]), None);
        assert_eq!(single.name_after_removal(), None);
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
    fn a_refresh_keeps_the_selected_entry_when_rows_shift() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.selected = Some(2); // b.txt
        v.scroll = None;
        v.apply_refresh(ok_listing(
            "/x",
            vec![entry("src", true), entry("0.txt", false), entry("a.txt", false), entry("b.txt", false)],
        ));
        assert_eq!(row_names(&v), ["src", "0.txt", "a.txt", "b.txt"]);
        assert_eq!(v.selected_entry().unwrap().name, "b.txt");
        assert_eq!(v.scroll, None, "a refresh does not move the view");
    }

    #[test]
    fn when_the_selected_entry_vanishes_the_selection_stays_on_its_row() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.selected = Some(1); // a.txt
        v.apply_refresh(ok_listing("/x", vec![entry("src", true), entry("b.txt", false)]));
        assert_eq!(v.selected_entry().unwrap().name, "b.txt");
        v.apply_refresh(ok_listing("/x", vec![entry("src", true)]));
        assert_eq!(v.selected_entry().unwrap().name, "src", "clamped to the last row");
        v.apply_refresh(ok_listing("/x", vec![]));
        assert_eq!(v.selected, None);
    }

    #[test]
    fn a_refresh_keeps_sort_hidden_filter_search_and_error() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Size);
        v.sort_by(SortColumn::Size); // descending
        let t0 = Instant::now();
        v.type_char('b', t0);
        v.error = Some("Cannot copy x".into());
        v.apply_refresh(ok_listing(
            "/s",
            vec![
                entry("a_dir", true),
                file(".h", Some(99), None),
                file("b.txt", Some(10), Some(3)),
                file("e.txt", Some(50), None),
            ],
        ));
        assert_eq!(row_names(&v), ["a_dir", "e.txt", "b.txt"], "size descending, hidden filtered");
        assert_eq!(v.error.as_deref(), Some("Cannot copy x"));
        assert_eq!(v.last_elapsed, Some(Duration::from_millis(3)));
        // Still the same search word: "b" + "." finds b.txt, not a new search for ".".
        v.type_char('.', t0 + Duration::from_millis(100));
        assert_eq!(v.selected_entry().unwrap().name, "b.txt");
    }

    #[test]
    fn a_failed_refresh_keeps_the_list_and_shows_why() {
        let mut v = loaded_view();
        v.apply_refresh(Listing {
            id: 2,
            path: PathBuf::from("/x"),
            result: Err(ListError::NotFound(PathBuf::from("/x"))),
            elapsed: Duration::ZERO,
        });
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.error.as_deref(), Some("Path does not exist: /x"));
    }

    #[test]
    fn refresh_timer_fires_once_after_the_first_change() {
        let mut t = RefreshTimer::default();
        let t0 = Instant::now();
        assert!(!t.take_due(t0));
        assert_eq!(t.wait(t0), None);
        t.note_change(t0);
        t.note_change(t0 + Duration::from_millis(150)); // does not postpone
        assert_eq!(t.wait(t0 + Duration::from_millis(50)), Some(REFRESH_DELAY - Duration::from_millis(50)));
        assert!(!t.take_due(t0 + Duration::from_millis(199)));
        assert!(t.take_due(t0 + REFRESH_DELAY));
        assert!(!t.take_due(t0 + REFRESH_DELAY * 2), "fires once");
        assert_eq!(t.wait(t0 + REFRESH_DELAY * 2), None);
    }

    #[test]
    fn a_refresh_is_not_a_history_step_even_when_it_fails() {
        let mut h = History::default();
        go(&mut h, NavKind::New, "/a", "/b");
        go(&mut h, NavKind::Refresh, "/b", "/b");
        h.settle(NavKind::Refresh, Path::new("/b"), None);
        assert_eq!(h.back_target(), Some(&p("/a")));
        assert_eq!(h.forward_target(), None);
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
