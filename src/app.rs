//! Application state and the per-frame UI loop.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::fs::count::{Count, Totals};
use crate::fs::watch::Watcher;
use crate::fs::{self, Entry, Lister, Listing};
use crate::input::{self, Action, Confirm};
use crate::places::{self, Bookmarks, Mount, Place, Recents, Usage};
use crate::fs::ops::Renamed;
use crate::fs::preview::Previewer;
use crate::fs::transfer::{self, Job, Mode, Outcome, Plan};
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::tabs::TabEvent;
use crate::ui::toolbar::{Nav, ToolbarEvent};
use crate::ui::{self, file_list::{ListEvent, RenameEdit}, path_bar::PathBar, preview::PreviewPane, status_bar::Status};

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
    Type,
    Modified,
    Folders,
    Files,
    Size,
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

/// What is on screen. Pure data, no egui or threads, so it is unit-testable.
#[derive(Default)]
pub struct View {
    /// Folder whose entries are shown. Only changes when a listing succeeds.
    pub current: PathBuf,
    pub entries: Vec<Entry>,
    /// Indices into `entries` that pass the hidden-files and text filters, in display order.
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
    /// What was typed in the filter bar; rows must contain it, ignoring case.
    filter: String,
    /// Folder totals computed with Alt+R, by entry name. Kept while the folder
    /// stays on screen, dropped when another folder is shown.
    pub totals: HashMap<String, Totals>,
}

impl View {
    /// Applies a finished listing. On error the previous folder stays on screen.
    /// `select_name` re-selects an entry, e.g. the folder we just left with Backspace.
    /// A new folder starts unfiltered; reloading the same one keeps the filter
    /// unless it would hide `select_name` (a new folder, a renamed entry).
    pub fn apply_listing(&mut self, listing: Listing, select_name: Option<&str>) {
        match listing.result {
            Ok(entries) => {
                if listing.path != self.current {
                    self.filter.clear();
                    self.totals.clear();
                }
                self.current = listing.path;
                self.entries = entries;
                self.last_elapsed = Some(listing.elapsed);
                self.error = None;
                self.rebuild_rows(None);
                if let Some(name) = select_name
                    && self.row_of_name(name).is_none()
                    && !self.filter.is_empty()
                {
                    self.filter.clear();
                    self.rebuild_rows(None);
                }
                let wanted = select_name.and_then(|name| self.row_of_name(name));
                self.selected = wanted.or((!self.rows.is_empty()).then_some(0));
                self.scroll = Some(Scroll::Instant);
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    /// Applies a listing of the folder already shown, after something changed
    /// it. Unlike a navigation it keeps the selection (the same row if the entry
    /// vanished), the filter, the error line and the scroll.
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

    /// Stores a finished count; a list sorted by a count column moves the row.
    pub fn set_totals(&mut self, name: String, totals: Totals) {
        self.totals.insert(name, totals);
        if matches!(self.sort.column, SortColumn::Folders | SortColumn::Files | SortColumn::Size) {
            let keep = self.selected_entry().map(|e| e.name.clone());
            self.rebuild_rows(keep.as_deref());
        }
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Adds a typed character to the filter and selects the first match.
    pub fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.refilter(None);
    }

    /// Erases the last filter character. `false` when there was none.
    pub fn pop_filter(&mut self) -> bool {
        if self.filter.pop().is_none() {
            return false;
        }
        self.refilter(None);
        true
    }

    /// Empties the filter, keeping the selected entry. `false` when it was empty.
    pub fn clear_filter(&mut self) -> bool {
        if self.filter.is_empty() {
            return false;
        }
        let keep = self.selected_entry().map(|e| e.name.clone());
        self.filter.clear();
        self.refilter(keep.as_deref());
        true
    }

    fn refilter(&mut self, keep_name: Option<&str>) {
        self.rebuild_rows(keep_name);
        self.scroll = Some(Scroll::Instant);
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

    /// Folders among the rows shown.
    pub fn folder_count(&self) -> usize {
        self.rows.iter().filter(|&&i| self.entries[i].is_dir).count()
    }

    pub fn hidden_count(&self) -> usize {
        self.entries.iter().filter(|e| e.hidden).count()
    }

    fn rebuild_rows(&mut self, keep_name: Option<&str>) {
        self.rows = visible_rows(&self.entries, &self.totals, self.show_hidden, &self.filter, self.sort);
        self.selected = keep_name
            .and_then(|name| self.row_of_name(name))
            .or((!self.rows.is_empty()).then_some(0));
    }

    fn row_of_name(&self, name: &str) -> Option<usize> {
        self.rows.iter().position(|&i| self.entries[i].name == name)
    }
}

/// Indices into `entries` that pass the hidden-files and text filters, in
/// display order. Shared by the list and the folder preview.
pub fn visible_rows(
    entries: &[Entry],
    totals: &HashMap<String, Totals>,
    show_hidden: bool,
    filter: &str,
    sort: Sort,
) -> Vec<usize> {
    let needle = filter.to_lowercase();
    let mut rows: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            let e = &entries[i];
            (show_hidden || !e.hidden) && (needle.is_empty() || e.name.to_lowercase().contains(&needle))
        })
        .collect();
    // Kinds are built once, not on every comparison.
    let kinds: Vec<String> = match sort.column {
        SortColumn::Type => entries.iter().map(Entry::kind).collect(),
        _ => Vec::new(),
    };
    rows.sort_by(|&a, &b| compare(entries, totals, &kinds, sort, a, b));
    rows
}

/// Orders two indices into `entries`. The lister already delivers entries
/// folders-first and by case-insensitive name, so the index itself is the name
/// order: it sorts the Name column and breaks ties without allocating.
fn compare(
    entries: &[Entry],
    totals: &HashMap<String, Totals>,
    kinds: &[String],
    sort: Sort,
    a: usize,
    b: usize,
) -> Ordering {
    let (ea, eb) = (&entries[a], &entries[b]);
    let counted = |e: &Entry, field: fn(&Totals) -> u64| totals.get(&e.name).map(field);
    // A counted folder's size is its contents; `None` (not counted, unreadable
    // metadata) sorts before any value.
    let size = |e: &Entry| e.size.or_else(|| counted(e, |t| t.bytes));
    let by_column = match sort.column {
        SortColumn::Name => a.cmp(&b),
        SortColumn::Type => kinds[a].cmp(&kinds[b]),
        SortColumn::Modified => ea.modified.cmp(&eb.modified),
        SortColumn::Folders => counted(ea, |t| t.folders).cmp(&counted(eb, |t| t.folders)),
        SortColumn::Files => counted(ea, |t| t.files).cmp(&counted(eb, |t| t.files)),
        SortColumn::Size => size(ea).cmp(&size(eb)),
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

/// How often the side panel reads the mounted disks again.
const DISKS_INTERVAL: Duration = Duration::from_secs(5);

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

/// What each tab keeps on its own: the folder on screen with its filter and
/// selection, the back/forward lists and the listing in flight.
pub struct Tab {
    view: View,
    lister: Lister,
    /// Id of the listing currently on screen (or the last one that failed).
    shown_id: u64,
    /// Entry to select once the pending listing arrives.
    select_after_load: Option<String>,
    /// How the pending listing was started.
    pending_nav: NavKind,
    history: History,
    /// Folder just created: rename it once a listing shows it selected.
    rename_after_load: Option<PathBuf>,
    /// Error to show once the pending reload lands (a reload clears errors).
    error_after_load: Option<String>,
}

impl Tab {
    /// A tab about to list `current`, sorted and filtered for hidden files like `like`.
    fn new(current: PathBuf, like: Option<&View>, log: bool) -> Self {
        let mut view = View { current, ..View::default() };
        if let Some(like) = like {
            view.sort = like.sort;
            view.show_hidden = like.show_hidden;
        }
        Self {
            view,
            lister: Lister::new(log),
            shown_id: 0,
            select_after_load: None,
            pending_nav: NavKind::New,
            history: History::default(),
            rename_after_load: None,
            error_after_load: None,
        }
    }
}

pub struct LumenApp {
    /// The tab on screen.
    tab: Tab,
    /// Every tab in bar order; the slot of the tab on screen is empty.
    tabs: Vec<Option<Tab>>,
    /// Index of the tab on screen in `tabs`.
    active: usize,
    /// Print listing times (LUMEN_LOG=1); new tabs inherit it.
    log: bool,
    path_bar: PathBar,
    dialog: Option<Dialog>,
    rename: Option<RenameEdit>,
    clipboard: Option<Clip>,
    /// Copy or move running in the background; one at a time.
    job: Option<Running>,
    bookmarks: Bookmarks,
    recents: Recents,
    /// Home, Downloads... and the trash, found once at startup.
    places: Vec<Place>,
    /// Where trashed files live; nothing in it may be trashed again.
    trash: Option<PathBuf>,
    /// Mounted disks with their usage, and when they were read.
    disks: (Option<Instant>, Vec<(Mount, Option<Usage>)>),
    sidebar_open: bool,
    /// Folder size and counts being computed (Alt+R); one at a time.
    counting: Option<Count>,
    /// Informational line in the status bar ("1 item copied").
    notice: Option<String>,
    /// Watches the folder on screen; changes schedule a refresh.
    watcher: Watcher,
    refresh: RefreshTimer,
    /// Side panel toggled with Space.
    preview_open: bool,
    /// The Backspace or Esc being held started by undoing something softer
    /// (filter text, a count), so the hold must not run on into the next thing.
    hold_absorbed: bool,
    previewer: Previewer,
    /// What the panel shows: the selected entry, once its preview loads.
    preview: Option<PreviewPane>,
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
            tab: Tab::new(start.clone(), None, log),
            tabs: vec![None],
            active: 0,
            log,
            path_bar: PathBar::default(),
            dialog: None,
            rename: None,
            clipboard: None,
            job: None,
            counting: None,
            bookmarks: Bookmarks::default(),
            recents: Recents::default(),
            places: Vec::new(),
            trash: None,
            disks: (None, Vec::new()),
            sidebar_open: true,
            notice: None,
            watcher: Watcher::new({
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            }),
            refresh: RefreshTimer::default(),
            preview_open: false,
            hold_absorbed: false,
            previewer: Previewer::new(),
            preview: None,
        };
        if let Some(home) = fs::home_dir() {
            let data_home = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
            let trash = places::trash_dir(&home, data_home.as_deref());
            let config_home = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map_or_else(|| home.join(".config"), PathBuf::from);
            let user_dirs = std::fs::read_to_string(config_home.join("user-dirs.dirs")).ok();
            let trash_place = cfg!(unix).then_some(trash.as_path());
            app.places = places::places(&home, user_dirs.as_deref(), trash_place);
            app.trash = cfg!(unix).then_some(trash);
        }
        app.navigate(ctx, start, None);
        app
    }

    /// Disks for the side panel, read again every few seconds so a plugged-in
    /// USB stick shows up. Reading them is quick: local devices only.
    fn refresh_disks(&mut self) {
        if self.disks.0.is_some_and(|at| at.elapsed() < DISKS_INTERVAL) {
            return;
        }
        let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
        self.disks = (
            Some(Instant::now()),
            places::parse_mounts(&mounts)
                .into_iter()
                .map(|m| {
                    let usage = places::usage(&m.path);
                    (m, usage)
                })
                .collect(),
        );
    }

    fn handle_sidebar(&mut self, ctx: &egui::Context, event: SidebarEvent) {
        match event {
            SidebarEvent::Open(path) => self.navigate(ctx, path, None),
            SidebarEvent::RemoveBookmark(path) => {
                self.bookmarks.toggle(&path);
                let name = file_name(&path).unwrap_or_else(|| path.display().to_string());
                self.notice = Some(format!("Removed “{name}” from bookmarks"));
            }
        }
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
        // Its result would only be dropped: totals belong to the folder on screen.
        if path != self.tab.view.current {
            self.cancel_count();
        }
        self.tab.pending_nav = kind;
        self.tab.select_after_load = select;
        let ctx = ctx.clone();
        // Wake the UI as soon as the worker finishes, without polling in a loop.
        self.tab.lister.request(path, move || ctx.request_repaint());
    }

    fn activate_selected(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.tab.view.selected_entry() else { return };
        let path = entry.path.clone();
        if entry.is_dir {
            self.navigate(ctx, path, None);
        } else if let Err(err) = open::that_detached(&path) {
            self.tab.view.error = Some(format!("Cannot open {}: {err}", path.display()));
        }
    }

    fn go_parent(&mut self, ctx: &egui::Context) {
        let current = self.tab.view.current.clone();
        if let Some(parent) = current.parent() {
            let select = came_from(&current, parent);
            self.navigate(ctx, parent.to_path_buf(), select);
        }
    }

    fn go_history(&mut self, ctx: &egui::Context, kind: NavKind) {
        let target = match kind {
            NavKind::Back => self.tab.history.back_target(),
            NavKind::Forward => self.tab.history.forward_target(),
            NavKind::New | NavKind::Refresh => None,
        };
        if let Some(target) = target.cloned() {
            let select = came_from(&self.tab.view.current, &target);
            self.navigate_as(ctx, kind, target, select);
        }
    }

    /// Lists the current folder again after an operation changed it.
    fn reload(&mut self, ctx: &egui::Context, select: Option<String>) {
        self.navigate(ctx, self.tab.view.current.clone(), select);
    }

    fn ask_trash(&mut self) {
        let Some(entry) = self.tab.view.selected_entry() else { return };
        // Trashing it again would lose the record that lets it be restored.
        if self.trash.as_deref().is_some_and(|trash| places::in_trash(&entry.path, trash)) {
            self.tab.view.error = Some("This is already in the trash".into());
            return;
        }
        self.dialog = Some(Dialog::Trash {
            path: entry.path.clone(),
            name: entry.name.clone(),
            select: self.tab.view.name_after_removal(),
        });
    }

    fn start_rename(&mut self) {
        let Some(entry) = self.tab.view.selected_entry() else { return };
        self.rename = Some(RenameEdit::new(entry.path.clone(), &entry.name, entry.is_dir));
    }

    fn new_folder(&mut self, ctx: &egui::Context) {
        match fs::ops::create_folder(&self.tab.view.current) {
            Ok(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.tab.rename_after_load = Some(path);
                self.reload(ctx, name);
            }
            Err(err) => self.tab.view.error = Some(err),
        }
    }

    /// Applies a finished in-place edit. Invalid names keep the field open.
    fn finish_rename(&mut self, ctx: &egui::Context, mut edit: RenameEdit, replace: bool) {
        match fs::ops::rename(&edit.path, &edit.text, replace) {
            Ok(Renamed::To(_)) => {
                self.tab.view.error = None;
                self.reload(ctx, Some(edit.text));
            }
            Ok(Renamed::Unchanged) => {}
            Ok(Renamed::TargetExists) => self.dialog = Some(Dialog::Replace(edit)),
            Err(err) => {
                // A replace may have trashed the target before failing: reload.
                let keep = self.tab.view.selected_entry().map(|e| e.name.clone());
                self.tab.error_after_load = Some(err);
                edit.focus = true;
                self.rename = Some(edit);
                self.reload(ctx, keep);
            }
        }
    }

    fn set_clipboard(&mut self, mode: Mode) {
        let Some(entry) = self.tab.view.selected_entry() else { return };
        self.clipboard = Some(Clip { path: entry.path.clone(), mode });
        self.notice = Some(match mode {
            Mode::Copy => "1 item copied".into(),
            Mode::Move => "1 item cut".into(),
        });
    }

    fn paste(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            self.tab.view.error = Some("Another operation is still running".into());
            return;
        }
        let Some(Clip { path, mode }) = self.clipboard.clone() else {
            self.notice = Some("Nothing to paste: Ctrl+C or Ctrl+X first".into());
            return;
        };
        match transfer::plan(&path, &self.tab.view.current, mode) {
            Ok(Plan::Ready(target)) => self.start_job(ctx, path, target, mode, false),
            Ok(Plan::Nothing) => {
                let name = file_name(&path).unwrap_or_default();
                self.notice = Some(format!("“{name}” is already in this folder"));
            }
            Ok(Plan::TargetExists(target)) => {
                self.dialog = Some(Dialog::Paste { source: path, target, mode });
            }
            Err(err) => self.tab.view.error = Some(err),
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
        self.tab.view.error = None;
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
            Err(err) => self.tab.error_after_load = Some(err),
        }
        // A navigation started meanwhile shows fresh contents anyway; reloading
        // here would supersede it. A pending refresh may be replaced.
        if self.tab.lister.is_pending(self.tab.shown_id) && self.tab.pending_nav != NavKind::Refresh {
            return;
        }
        let select = if running.target.parent() == Some(self.tab.view.current.as_path()) {
            file_name(&running.target)
        } else {
            self.tab.view.selected_entry().map(|e| e.name.clone())
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
                    self.tab.error_after_load = Some(err);
                    self.reload(ctx, Some(name));
                }
            },
            Dialog::Replace(edit) => self.finish_rename(ctx, edit, true),
            Dialog::Paste { source, target, mode } => {
                self.start_job(ctx, source, target, mode, true);
            }
        }
    }

    /// Titles of every tab, in bar order.
    fn tab_labels(&self) -> Vec<String> {
        self.tabs
            .iter()
            .map(|slot| {
                let tab = slot.as_ref().unwrap_or(&self.tab);
                ui::tabs::tab_label(&tab.view.current)
            })
            .collect()
    }

    /// Ctrl+T or +: opens the current folder in a new tab after this one.
    fn new_tab(&mut self, ctx: &egui::Context) {
        let tab = Tab::new(self.tab.view.current.clone(), Some(&self.tab.view), self.log);
        self.leave_tab();
        let old = std::mem::replace(&mut self.tab, tab);
        self.tabs[self.active] = Some(old);
        self.active += 1;
        self.tabs.insert(self.active, None);
        self.navigate(ctx, self.tab.view.current.clone(), None);
    }

    /// Ctrl+W or ×: closes tab `index`; the last tab stays open.
    fn close_tab(&mut self, ctx: &egui::Context, index: usize) {
        if self.tabs.len() < 2 || index >= self.tabs.len() {
            return;
        }
        if index != self.active {
            self.tabs.remove(index);
            if index < self.active {
                self.active -= 1;
            }
            return;
        }
        // Show the right neighbour, or the left one when closing the last tab.
        let next = if index + 1 < self.tabs.len() { index + 1 } else { index - 1 };
        self.select_tab(ctx, next);
        self.tabs.remove(index);
        if index < self.active {
            self.active -= 1;
        }
    }

    /// Puts tab `index` on screen and reloads it: its folder may have changed
    /// while it was not watched.
    fn select_tab(&mut self, ctx: &egui::Context, index: usize) {
        if index == self.active || index >= self.tabs.len() {
            return;
        }
        let Some(tab) = self.tabs[index].take() else { return };
        self.leave_tab();
        let old = std::mem::replace(&mut self.tab, tab);
        self.tabs[self.active] = Some(old);
        self.active = index;
        self.tab.view.scroll = Some(Scroll::Instant);
        self.watcher.watch(&self.tab.view.current);
        // A listing still in flight shows fresh contents anyway.
        if !self.tab.lister.is_pending(self.tab.shown_id) {
            self.navigate_as(ctx, NavKind::Refresh, self.tab.view.current.clone(), None);
        }
    }

    /// What belongs to the tab on screen and must not follow it into the background.
    fn leave_tab(&mut self) {
        self.cancel_count();
        self.rename = None;
        self.hold_absorbed = false;
    }

    /// Ctrl+D or the star: bookmarks the folder on screen, or removes it.
    fn toggle_bookmark(&mut self) {
        let current = self.tab.view.current.clone();
        let name = file_name(&current).unwrap_or_else(|| current.display().to_string());
        self.notice = Some(if self.bookmarks.toggle(&current) {
            format!("Added “{name}” to bookmarks")
        } else {
            format!("Removed “{name}” from bookmarks")
        });
    }

    fn handle_toolbar(&mut self, ctx: &egui::Context, event: ToolbarEvent) {
        match event {
            ToolbarEvent::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            ToolbarEvent::Back => self.go_history(ctx, NavKind::Back),
            ToolbarEvent::Forward => self.go_history(ctx, NavKind::Forward),
            // Unlike Backspace, the button means "up" whatever the filter holds.
            ToolbarEvent::Up => self.go_parent(ctx),
            ToolbarEvent::ToggleBookmark => self.toggle_bookmark(),
            ToolbarEvent::Go(path) => self.navigate(ctx, path, None),
            ToolbarEvent::Typed(text) => {
                let current = &self.tab.view.current;
                let target = fs::resolve_input_path(&text, current, fs::home_dir().as_deref());
                self.navigate(ctx, target, None);
            }
        }
    }

    /// Alt+R: counts what the selected folder holds, in the background.
    fn count_selected(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.tab.view.selected_entry().filter(|e| e.is_dir) else { return };
        let path = entry.path.clone();
        // Only the newest count matters; the old one would be dropped unread.
        self.cancel_count();
        self.notice = None;
        let ctx = ctx.clone();
        self.counting = Some(Count::start(path, move || ctx.request_repaint()));
    }

    /// Stops the running count. `false` when there was none.
    fn cancel_count(&mut self) -> bool {
        let Some(count) = self.counting.take() else { return false };
        count.cancel();
        true
    }

    fn poll_count(&mut self) {
        if let Some(result) = self.counting.as_mut().and_then(|c| c.poll())
            && let Some(count) = self.counting.take()
        {
            self.finish_count(count, result);
        }
    }

    /// Stores a finished count if its folder is still on screen.
    fn finish_count(&mut self, count: Count, result: Result<Option<Totals>, String>) {
        if count.path.parent() != Some(self.tab.view.current.as_path()) {
            return;
        }
        let Some(name) = file_name(&count.path) else { return };
        match result {
            Ok(Some(totals)) => {
                if totals.unreadable > 0 {
                    let n = totals.unreadable;
                    let folders = if n == 1 { "folder" } else { "folders" };
                    self.notice = Some(format!("“{name}”: {n} {folders} could not be read"));
                }
                self.tab.view.set_totals(name, totals);
            }
            Ok(None) => {}
            Err(err) => self.tab.view.error = Some(err),
        }
    }

    fn cancel_job(&self) {
        if let Some(running) = &self.job {
            running.job.cancel();
        }
    }

    /// Keeps the open panel on the selected entry, loading it again when the
    /// selection moves or the entry changes on disk.
    fn sync_preview(&mut self, ctx: &egui::Context) {
        let entry = self.tab.view.selected_entry().filter(|_| self.preview_open);
        let current = match (entry, &self.preview) {
            (Some(entry), Some(pane)) => pane.shows(entry),
            (None, None) => true,
            _ => false,
        };
        if !current {
            self.preview = entry.map(|entry| {
                let ctx = ctx.clone();
                self.previewer.request(entry.path.clone(), move || ctx.request_repaint());
                PreviewPane::new(entry)
            });
        }
        // Only the newest request is ever delivered, and it belongs to this pane.
        if let Some(preview) = self.previewer.poll()
            && let Some(pane) = &mut self.preview
            && pane.path == preview.path
        {
            pane.content = Some(preview.content);
        }
    }

    /// Shows a finished listing and settles what depended on it.
    fn apply(&mut self, listing: Listing) {
        self.tab.shown_id = listing.id;
        let select = self.tab.select_after_load.take();
        let from = self.tab.view.current.clone();
        let to = listing.result.is_ok().then(|| listing.path.clone());
        if self.tab.pending_nav == NavKind::Refresh {
            self.tab.view.apply_refresh(listing);
        } else {
            self.tab.view.apply_listing(listing, select.as_deref());
        }
        self.tab.history.settle(self.tab.pending_nav, &from, to.as_deref());
        if to.is_some() {
            self.watcher.watch(&self.tab.view.current);
            if self.tab.pending_nav != NavKind::Refresh {
                self.recents.visit(&self.tab.view.current);
            }
        } else if self.tab.pending_nav == NavKind::Refresh {
            // The open folder is gone; watch it afresh if it comes back.
            self.watcher.forget();
        }
        if let Some(err) = self.tab.error_after_load.take() {
            self.tab.view.error = Some(err);
        }
        // The edited entry may be gone (another folder, a reload).
        if let Some(edit) = &self.rename
            && !self.tab.view.entries.iter().any(|e| e.path == edit.path)
        {
            self.rename = None;
        }
        // A faster navigation may have superseded the reload: check the entry.
        if let Some(created) = self.tab.rename_after_load.take()
            && self.tab.view.selected_entry().is_some_and(|e| e.path == created)
        {
            self.start_rename();
        }
    }

    /// Reloads the open folder a moment after it changed on disk.
    fn refresh_if_changed(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self.watcher.take_changed() {
            self.refresh.note_change(now);
        }
        if self.refresh.take_due(now) {
            if self.tab.lister.is_pending(self.tab.shown_id) {
                // Never supersede a navigation; try again once it has landed.
                self.refresh.note_change(now);
            } else {
                self.navigate_as(ctx, NavKind::Refresh, self.tab.view.current.clone(), None);
            }
        }
        if let Some(wait) = self.refresh.wait(now) {
            ctx.request_repaint_after(wait);
        }
    }

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::FocusPathBar => self.path_bar.request_focus(),
            Action::ToggleHidden => self.tab.view.toggle_hidden(),
            Action::Activate => self.activate_selected(ctx),
            Action::GoParent => {
                self.hold_absorbed = self.tab.view.pop_filter();
                if !self.hold_absorbed {
                    self.go_parent(ctx);
                }
            }
            // A hold that started on the filter only erases; going up takes a new press.
            Action::GoParentHeld => {
                if self.hold_absorbed {
                    self.tab.view.pop_filter();
                } else {
                    self.go_parent(ctx);
                }
            }
            Action::Move(_) | Action::First | Action::Last => self.tab.view.move_selection(action),
            // A leading Space is the preview key; after other text it is just a space.
            Action::TypeChar(' ') if self.tab.view.filter().is_empty() => {
                self.preview_open = !self.preview_open;
            }
            Action::TypeChar(c) => self.tab.view.push_filter(c),
            Action::Back => self.go_history(ctx, NavKind::Back),
            Action::Forward => self.go_history(ctx, NavKind::Forward),
            Action::Trash => self.ask_trash(),
            Action::Rename => self.start_rename(),
            Action::NewFolder => self.new_folder(ctx),
            Action::Copy => self.set_clipboard(Mode::Copy),
            Action::Cut => self.set_clipboard(Mode::Move),
            Action::Paste => self.paste(ctx),
            Action::Count => self.count_selected(ctx),
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::NewTab => self.new_tab(ctx),
            Action::CloseTab => self.close_tab(ctx, self.active),
            Action::NextTab => self.select_tab(ctx, (self.active + 1) % self.tabs.len()),
            Action::PrevTab => {
                self.select_tab(ctx, (self.active + self.tabs.len() - 1) % self.tabs.len());
            }
            // Clearing a filter is the cheaper surprise than cancelling a copy.
            // A count is cheaper to lose than a copy, too.
            Action::Cancel => {
                self.hold_absorbed = self.tab.view.clear_filter();
                if !self.hold_absorbed && self.cancel_count() {
                    self.notice = Some("Calculation cancelled".into());
                    self.hold_absorbed = true;
                }
                if !self.hold_absorbed {
                    self.cancel_job();
                }
            }
            Action::CancelHeld => {
                if !self.hold_absorbed {
                    self.cancel_job();
                }
            }
        }
    }
}

impl eframe::App for LumenApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if let Some(listing) = self.tab.lister.poll() {
            self.apply(listing);
        }

        self.refresh_if_changed(&ctx);
        self.poll_count();

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

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(3.0);
            let labels = self.tab_labels();
            match ui::tabs::show(ui, &labels, self.active) {
                Some(TabEvent::Select(i)) => self.select_tab(&ctx, i),
                Some(TabEvent::Close(i)) => self.close_tab(&ctx, i),
                Some(TabEvent::New) => self.new_tab(&ctx),
                None => {}
            }
        });

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            let current = self.tab.view.current.clone();
            let home = fs::home_dir();
            let nav = Nav {
                current: &current,
                home: home.as_deref(),
                can_back: self.tab.history.back_target().is_some(),
                can_forward: self.tab.history.forward_target().is_some(),
                bookmarked: self.bookmarks.contains(&current),
            };
            if let Some(event) = ui::toolbar::show(ui, &nav, &mut self.path_bar) {
                self.handle_toolbar(&ctx, event);
            }
            ui.add_space(2.0);
        });

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui::status_bar::show(
                ui,
                &Status {
                    filter: self.tab.view.filter(),
                    item_count: self.tab.view.rows.len(),
                    hidden_count: self.tab.view.hidden_count(),
                    show_hidden: self.tab.view.show_hidden,
                    loading: self.tab.lister.is_pending(self.tab.shown_id),
                    last_elapsed: self.tab.view.last_elapsed,
                    error: self.tab.view.error.as_deref(),
                    job: self
                        .job
                        .as_ref()
                        .map(|r| ui::status_bar::progress_text(r.job.mode, &r.job.progress)),
                    notice: self.notice.as_deref(),
                    folder_count: self.tab.view.folder_count(),
                    selected: self.tab.view.selected,
                    counting: self.counting.as_ref().map(|c| {
                        let name = file_name(&c.path).unwrap_or_default();
                        ui::status_bar::counting_text(&name, &c.progress)
                    }),
                },
            );
        });

        if self.sidebar_open {
            self.refresh_disks();
            ctx.request_repaint_after(DISKS_INTERVAL);
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(200.0)
                .size_range(140.0..=360.0)
                .show(ui, |ui| {
                    let sidebar = Sidebar {
                        current: &self.tab.view.current,
                        bookmarks: self.bookmarks.paths(),
                        places: &self.places,
                        disks: &self.disks.1,
                        recents: self.recents.paths(),
                    };
                    if let Some(event) = ui::sidebar::show(ui, &sidebar) {
                        self.handle_sidebar(&ctx, event);
                    }
                });
        }

        self.sync_preview(&ctx);
        if self.preview_open {
            let width = ui.available_width();
            egui::Panel::right("preview")
                .resizable(true)
                .default_size(width * 0.4)
                .size_range(200.0..=width * 0.75)
                .show(ui, |ui| match (&mut self.preview, self.tab.view.selected_entry()) {
                    (Some(pane), Some(entry)) => {
                        ui::preview::show(
                            ui,
                            entry,
                            self.tab.view.totals.get(&entry.name),
                            pane,
                            self.tab.view.show_hidden,
                            self.tab.view.sort,
                        );
                    }
                    _ => {
                        ui.weak("Nothing selected");
                    }
                });
        }

        egui::CentralPanel::default_margins().show(ui, |ui| {
            let scroll = self.tab.view.scroll.take();
            let event = ui::file_list::show(ui, &self.tab.view, scroll, self.rename.as_mut());
            match event {
                Some(ListEvent::Select(i)) => self.tab.view.selected = Some(i),
                Some(ListEvent::Activate(i)) => {
                    self.tab.view.selected = Some(i);
                    self.activate_selected(&ctx);
                }
                Some(ListEvent::Sort(column)) => self.tab.view.sort_by(column),
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
    use crate::fs::preview::Content;
    use std::path::Path;
    use std::time::Instant;

    #[test]
    fn cut_and_paste_into_the_same_folder_says_why_nothing_happened() {
        let dir = crate::fs::ops::tests::fresh_fixture("app-paste-same");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        let ctx = egui::Context::default();
        let mut app = LumenApp::new(&ctx, dir.clone(), false);
        app.clipboard = Some(Clip { path: dir.join("a.txt"), mode: Mode::Move });
        app.paste(&ctx);
        assert_eq!(app.notice.as_deref(), Some("“a.txt” is already in this folder"));
        assert!(app.job.is_none());
        assert!(app.clipboard.is_some(), "still cut, ready to paste elsewhere");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "A");
    }

    /// An app showing `dir` with `a.txt`, `b.txt` and a `sub` folder, listed synchronously.
    fn app_in(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let dir = crate::fs::ops::tests::fresh_fixture(name);
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let ctx = egui::Context::default();
        let mut app = LumenApp::new(&ctx, dir.clone(), false);
        let entries = fs::lister::read_entries(&dir).unwrap();
        let listing = Listing { id: 0, path: dir.clone(), result: Ok(entries), elapsed: Duration::ZERO };
        app.tab.view.apply_listing(listing, None);
        // As if the startup listing had landed: nothing is in flight.
        app.tab.shown_id = app.tab.lister.latest_id();
        (ctx, app, dir)
    }

    #[test]
    fn space_with_an_empty_filter_toggles_the_preview_and_types_nothing() {
        let (ctx, mut app, _) = app_in("app-space-preview");
        assert!(!app.preview_open);
        app.handle_action(&ctx, Action::TypeChar(' '));
        assert!(app.preview_open);
        assert_eq!(app.tab.view.filter(), "");
        app.handle_action(&ctx, Action::TypeChar(' '));
        assert!(!app.preview_open, "the same Space closes it");
    }

    #[test]
    fn space_after_text_is_part_of_the_filter() {
        let (ctx, mut app, _) = app_in("app-space-filter");
        app.handle_action(&ctx, Action::TypeChar('a'));
        app.handle_action(&ctx, Action::TypeChar(' '));
        assert_eq!(app.tab.view.filter(), "a ");
        assert!(!app.preview_open);
    }

    #[test]
    fn backspace_erases_the_filter_before_going_up() {
        let (ctx, mut app, dir) = app_in("app-backspace");
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::GoParent);
        assert_eq!(app.tab.view.filter(), "");
        assert_eq!(app.tab.select_after_load, None, "no navigation started");
        app.handle_action(&ctx, Action::GoParent);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.tab.select_after_load, Some(name), "went up to the parent");
    }

    #[test]
    fn holding_backspace_stops_when_the_filter_is_empty() {
        let (ctx, mut app, dir) = app_in("app-backspace-held");
        app.handle_action(&ctx, Action::TypeChar('a'));
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::GoParent);
        for _ in 0..3 {
            app.handle_action(&ctx, Action::GoParentHeld);
        }
        assert_eq!(app.tab.view.filter(), "");
        assert_eq!(app.tab.select_after_load, None, "the held key did not climb a folder");
        app.handle_action(&ctx, Action::GoParent);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.tab.select_after_load, Some(name), "a new press goes up");
    }

    #[test]
    fn holding_backspace_with_no_filter_still_climbs_folders() {
        let (ctx, mut app, dir) = app_in("app-backspace-held-climb");
        app.handle_action(&ctx, Action::GoParent);
        app.tab.select_after_load = None;
        app.handle_action(&ctx, Action::GoParentHeld);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.tab.select_after_load, Some(name));
    }

    #[test]
    fn holding_escape_clears_the_filter_without_cancelling_the_copy() {
        let (ctx, mut app, dir) = app_in("app-escape-held");
        app.clipboard = Some(Clip { path: dir.join("a.txt"), mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::Cancel);
        app.handle_action(&ctx, Action::CancelHeld);
        assert_eq!(app.tab.view.filter(), "");
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "the copy goes on");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.job.as_ref().unwrap().job.cancel_requested(), "a new press cancels");
    }

    #[test]
    fn holding_escape_with_no_filter_still_cancels_the_copy() {
        let (ctx, mut app, dir) = app_in("app-escape-held-cancel");
        app.clipboard = Some(Clip { path: dir.join("a.txt"), mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::CancelHeld);
        assert!(app.job.as_ref().unwrap().job.cancel_requested());
    }

    #[test]
    fn escape_clears_the_filter_first_and_then_cancels_a_copy() {
        let (ctx, mut app, dir) = app_in("app-escape");
        app.clipboard = Some(Clip { path: dir.join("a.txt"), mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.filter(), "");
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "the copy goes on");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.job.as_ref().unwrap().job.cancel_requested());
    }

    fn listing_of(dir: &Path) -> Listing {
        let entries = fs::lister::read_entries(dir).unwrap();
        Listing { id: 0, path: dir.to_path_buf(), result: Ok(entries), elapsed: Duration::ZERO }
    }

    /// Lists what the active tab asked for, synchronously, as a later frame would.
    fn land(app: &mut LumenApp) {
        let path = app.tab.lister.last_requested().expect("a listing was requested");
        let mut listing = listing_of(&path);
        listing.id = app.tab.lister.latest_id();
        app.apply(listing);
    }

    #[test]
    fn a_new_tab_opens_the_same_folder_after_this_one_with_its_own_history() {
        let (ctx, mut app, dir) = app_in("app-tab-new");
        app.tab.view.sort_by(SortColumn::Size);
        app.tab.view.toggle_hidden();
        app.handle_action(&ctx, Action::Activate); // into sub
        land(&mut app);
        assert!(app.tab.history.back_target().is_some());
        app.handle_action(&ctx, Action::NewTab);
        assert_eq!((app.tabs.len(), app.active), (2, 1));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub")));
        land(&mut app);
        assert_eq!(app.tab.view.current, dir.join("sub"));
        assert_eq!(app.tab.view.sort.column, SortColumn::Size, "sort carried over");
        assert!(app.tab.view.show_hidden, "Ctrl+H carried over");
        assert!(app.tab.history.back_target().is_none(), "a fresh history");
        assert_eq!(app.tab_labels(), ["sub", "sub"]);
    }

    #[test]
    fn each_tab_keeps_its_own_folder_filter_and_selection() {
        let (ctx, mut app, dir) = app_in("app-tab-own");
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::TypeChar('.'));
        assert_eq!(app.tab.view.selected_entry().unwrap().name, "b.txt");
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        assert_eq!(app.tab.view.filter(), "", "the new tab starts unfiltered");
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        land(&mut app);
        app.handle_action(&ctx, Action::PrevTab);
        assert_eq!(app.active, 0);
        assert_eq!(app.tab.view.current, dir);
        assert_eq!(app.tab.view.filter(), "b.");
        assert_eq!(app.tab.view.selected_entry().unwrap().name, "b.txt");
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.tab_labels(), [name, "sub".to_owned()]);
        app.handle_action(&ctx, Action::NextTab);
        assert_eq!(app.tab.view.current, dir.join("sub"));
    }

    #[test]
    fn switching_tabs_wraps_around_and_reloads_the_tab_shown() {
        let (ctx, mut app, dir) = app_in("app-tab-cycle");
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        app.handle_action(&ctx, Action::NextTab);
        assert_eq!(app.active, 0, "past the last tab comes the first");
        assert_eq!(app.tab.pending_nav, NavKind::Refresh, "it may have changed meanwhile");
        assert_eq!(app.tab.lister.last_requested(), Some(dir));
        app.handle_action(&ctx, Action::PrevTab);
        assert_eq!(app.active, 1, "before the first comes the last");
    }

    #[test]
    fn switching_tabs_stops_the_count_and_the_rename() {
        let (ctx, mut app, _) = app_in("app-tab-switch-stops");
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        app.handle_action(&ctx, Action::Count);
        app.handle_action(&ctx, Action::Rename);
        assert!(app.counting.is_some() && app.rename.is_some());
        app.handle_action(&ctx, Action::NextTab);
        assert!(app.counting.is_none() && app.rename.is_none());
    }

    #[test]
    fn closing_a_tab_shows_its_right_neighbour_and_the_last_tab_stays() {
        let (ctx, mut app, dir) = app_in("app-tab-close");
        app.handle_action(&ctx, Action::CloseTab);
        assert_eq!(app.tabs.len(), 1, "the only tab cannot be closed");
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        land(&mut app);
        app.handle_action(&ctx, Action::NewTab); // third tab, in sub
        land(&mut app);
        app.select_tab(&ctx, 1);
        app.handle_action(&ctx, Action::CloseTab);
        assert_eq!((app.tabs.len(), app.active), (2, 1), "the right neighbour moved into its place");
        assert_eq!(app.tab.view.current, dir.join("sub"));
        app.close_tab(&ctx, 1);
        assert_eq!((app.tabs.len(), app.active), (1, 0), "no right neighbour: the left one");
        assert_eq!(app.tab.view.current, dir);
    }

    #[test]
    fn closing_a_tab_in_the_background_keeps_the_one_on_screen() {
        let (ctx, mut app, dir) = app_in("app-tab-close-other");
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        land(&mut app);
        app.close_tab(&ctx, 0);
        assert_eq!((app.tabs.len(), app.active), (1, 0));
        assert_eq!(app.tab.view.current, dir.join("sub"));
    }

    #[test]
    fn visited_folders_become_recents_but_refreshes_and_failures_do_not() {
        let (_ctx, mut app, dir) = app_in("app-recents");
        app.apply(listing_of(&dir.join("sub")));
        assert_eq!(app.recents.paths(), [dir.join("sub")]);
        app.tab.pending_nav = NavKind::Back;
        app.apply(listing_of(&dir));
        assert_eq!(app.recents.paths(), [dir.clone(), dir.join("sub")]);
        app.tab.pending_nav = NavKind::Refresh;
        app.apply(listing_of(&dir.join("sub")));
        assert_eq!(app.recents.paths()[0], dir, "a refresh is not a visit");
        app.tab.pending_nav = NavKind::New;
        let failed = Listing { id: 0, path: dir.join("gone"), result: Err(ListError::NotFound(dir.join("gone"))), elapsed: Duration::ZERO };
        app.apply(failed);
        assert_eq!(app.recents.paths().len(), 2);
    }

    #[test]
    fn nothing_inside_the_trash_can_be_trashed_again() {
        let (_ctx, mut app, dir) = app_in("app-trash-guard");
        app.trash = Some(dir.clone());
        app.ask_trash();
        assert!(app.dialog.is_none(), "no confirmation is offered");
        assert_eq!(app.tab.view.error.as_deref(), Some("This is already in the trash"));
        assert!(dir.join("sub").exists());
        app.trash = Some(dir.join("elsewhere"));
        app.tab.view.error = None;
        app.ask_trash();
        assert!(app.dialog.is_some(), "outside the trash Delete still asks");
    }

    #[test]
    fn the_menu_button_shows_and_hides_the_side_panel() {
        let (ctx, mut app, _) = app_in("app-sidebar-toggle");
        assert!(app.sidebar_open, "open by default");
        app.handle_toolbar(&ctx, ToolbarEvent::ToggleSidebar);
        assert!(!app.sidebar_open);
        app.handle_toolbar(&ctx, ToolbarEvent::ToggleSidebar);
        assert!(app.sidebar_open);
    }

    #[test]
    fn side_panel_items_open_and_bookmarks_can_be_removed() {
        let (ctx, mut app, dir) = app_in("app-sidebar");
        app.handle_sidebar(&ctx, SidebarEvent::Open(dir.join("sub")));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub")));
        app.bookmarks.toggle(&dir.join("sub"));
        app.handle_sidebar(&ctx, SidebarEvent::RemoveBookmark(dir.join("sub")));
        assert!(app.bookmarks.paths().is_empty());
        assert_eq!(app.notice.as_deref(), Some("Removed “sub” from bookmarks"));
    }

    #[test]
    fn ctrl_d_bookmarks_the_current_folder_and_again_removes_it() {
        let (ctx, mut app, dir) = app_in("app-bookmark");
        app.handle_action(&ctx, Action::ToggleBookmark);
        assert!(app.bookmarks.contains(&dir));
        assert_eq!(app.notice.as_deref(), Some("Added “app-bookmark” to bookmarks"));
        app.handle_toolbar(&ctx, ToolbarEvent::ToggleBookmark);
        assert!(!app.bookmarks.contains(&dir), "the star does the same");
        assert_eq!(app.notice.as_deref(), Some("Removed “app-bookmark” from bookmarks"));
    }

    #[test]
    fn the_up_button_goes_up_even_with_a_filter() {
        let (ctx, mut app, dir) = app_in("app-up-button");
        app.handle_action(&ctx, Action::TypeChar('a'));
        app.handle_toolbar(&ctx, ToolbarEvent::Up);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.tab.select_after_load, Some(name), "went up, selecting where we came from");
        assert_eq!(app.tab.view.filter(), "a", "the filter is not touched");
    }

    #[test]
    fn a_crumb_or_typed_path_navigates() {
        let (ctx, mut app, dir) = app_in("app-crumb");
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub")));
        app.handle_toolbar(&ctx, ToolbarEvent::Typed("sub/..".into()));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.clone()));
    }

    /// Lets the running count finish, as the next frames would.
    fn wait_count(app: &mut LumenApp) {
        for _ in 0..500 {
            app.poll_count();
            if app.counting.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("count did not finish");
    }

    #[test]
    fn alt_r_counts_the_selected_folder_in_the_background() {
        let (ctx, mut app, dir) = app_in("app-count");
        std::fs::write(dir.join("sub/c.txt"), "12345").unwrap();
        std::fs::create_dir(dir.join("sub/inner")).unwrap();
        assert_eq!(app.tab.view.selected_entry().unwrap().name, "sub");
        app.handle_action(&ctx, Action::Count);
        assert_eq!(app.counting.as_ref().map(|c| c.path.clone()), Some(dir.join("sub")));
        wait_count(&mut app);
        let expected = Totals { folders: 1, files: 1, bytes: 5, unreadable: 0 };
        assert_eq!(app.tab.view.totals.get("sub"), Some(&expected));
        assert_eq!(app.notice, None);
    }

    #[test]
    fn alt_r_on_a_file_does_nothing() {
        let (ctx, mut app, _) = app_in("app-count-file");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.view.selected_entry().unwrap().name, "a.txt");
        app.handle_action(&ctx, Action::Count);
        assert!(app.counting.is_none());
        assert!(app.tab.view.totals.is_empty());
    }

    #[test]
    fn escape_clears_the_filter_then_cancels_the_count_then_the_copy() {
        let (ctx, mut app, dir) = app_in("app-count-escape");
        app.clipboard = Some(Clip { path: dir.join("a.txt"), mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::Count);
        app.handle_action(&ctx, Action::TypeChar('s'));
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.filter(), "");
        assert!(app.counting.is_some(), "the count goes on");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.counting.is_none(), "the count stops");
        assert_eq!(app.notice.as_deref(), Some("Calculation cancelled"));
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "the copy goes on");
        app.handle_action(&ctx, Action::CancelHeld);
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "holding Esc stops at the count");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.job.as_ref().unwrap().job.cancel_requested(), "a new press cancels the copy");
    }

    #[test]
    fn leaving_the_folder_stops_the_count() {
        let (ctx, mut app, _) = app_in("app-count-leave");
        app.handle_action(&ctx, Action::Count);
        app.handle_action(&ctx, Action::GoParent);
        assert!(app.counting.is_none());
    }

    #[test]
    fn a_count_for_a_folder_no_longer_shown_is_dropped() {
        let (_ctx, mut app, dir) = app_in("app-count-stale");
        let count = Count::start(dir.join("elsewhere/sub"), || {});
        app.finish_count(count, Ok(Some(Totals { files: 3, ..Totals::default() })));
        assert!(app.tab.view.totals.is_empty());
    }

    #[test]
    fn a_count_reports_unreadable_folders_and_errors() {
        let (_ctx, mut app, dir) = app_in("app-count-errors");
        let partial = Totals { folders: 4, files: 1, bytes: 9, unreadable: 2 };
        app.finish_count(Count::start(dir.join("sub"), || {}), Ok(Some(partial)));
        assert_eq!(app.tab.view.totals.get("sub"), Some(&partial));
        assert_eq!(app.notice.as_deref(), Some("“sub”: 2 folders could not be read"));
        app.finish_count(Count::start(dir.join("sub"), || {}), Err("Cannot read sub: gone".into()));
        assert_eq!(app.tab.view.error.as_deref(), Some("Cannot read sub: gone"));
    }

    /// Lets the preview worker finish, as the next frames would.
    fn wait_preview<'a>(ctx: &egui::Context, app: &'a mut LumenApp) -> &'a Content {
        for _ in 0..500 {
            app.sync_preview(ctx);
            if app.preview.as_ref().is_some_and(|p| p.content.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        app.preview.as_ref().and_then(|p| p.content.as_ref()).expect("preview loaded")
    }

    fn preview_text(content: &Content) -> &str {
        match content {
            Content::Text { text, .. } => text,
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn the_preview_follows_the_selection_while_open() {
        let (ctx, mut app, dir) = app_in("app-preview-follow"); // sub, a.txt, b.txt
        app.sync_preview(&ctx);
        assert!(app.preview.is_none(), "closed: nothing is loaded");
        app.handle_action(&ctx, Action::TypeChar(' '));
        let Content::Folder(Ok(entries)) = wait_preview(&ctx, &mut app) else { panic!("folder") };
        assert!(entries.is_empty(), "sub is empty");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(preview_text(wait_preview(&ctx, &mut app)), "A");
        assert_eq!(app.preview.as_ref().unwrap().path, dir.join("a.txt"));
        app.handle_action(&ctx, Action::TypeChar(' '));
        app.sync_preview(&ctx);
        assert!(app.preview.is_none(), "Space closed it");
    }

    #[test]
    fn a_file_changed_on_disk_is_previewed_again() {
        let (ctx, mut app, dir) = app_in("app-preview-changed");
        app.handle_action(&ctx, Action::Move(1)); // a.txt
        app.handle_action(&ctx, Action::TypeChar(' '));
        assert_eq!(preview_text(wait_preview(&ctx, &mut app)), "A");
        std::fs::write(dir.join("a.txt"), "AAA").unwrap();
        let entries = fs::lister::read_entries(&dir).unwrap();
        app.tab.view.apply_refresh(Listing { id: 1, path: dir, result: Ok(entries), elapsed: Duration::ZERO });
        assert_eq!(preview_text(wait_preview(&ctx, &mut app)), "AAA");
    }

    #[test]
    fn with_nothing_selected_the_open_preview_is_empty() {
        let (ctx, mut app, _) = app_in("app-preview-none");
        app.handle_action(&ctx, Action::TypeChar(' '));
        for c in "zzz".chars() {
            app.handle_action(&ctx, Action::TypeChar(c));
        }
        app.sync_preview(&ctx);
        assert!(app.preview_open);
        assert!(app.preview.is_none());
    }

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
    fn folder_count_follows_the_rows_shown() {
        let mut v = loaded_view();
        assert_eq!(v.folder_count(), 1, "only src: .git is hidden");
        v.toggle_hidden();
        assert_eq!(v.folder_count(), 2);
        for c in "a.".chars() {
            v.push_filter(c);
        }
        assert_eq!(v.folder_count(), 0, "the filter left only a.txt");
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
    fn visible_rows_sort_and_hide_like_the_list_for_any_entries() {
        let v = sortable_view();
        let size_desc = Sort { column: SortColumn::Size, descending: true };
        let rows = visible_rows(&v.entries, &HashMap::new(), false, "", size_desc);
        let names: Vec<&str> = rows.iter().map(|&i| v.entries[i].name.as_str()).collect();
        assert_eq!(names, ["a_dir", "b_dir", "a.txt", "b.txt", "d.txt", "c.txt"]);
        let entries = vec![entry(".git", true), entry("src", true), entry("a.txt", false)];
        assert_eq!(visible_rows(&entries, &HashMap::new(), false, "", Sort::default()), [1, 2]);
        assert_eq!(visible_rows(&entries, &HashMap::new(), true, "", Sort::default()), [0, 1, 2]);
    }

    fn totals(bytes: u64, folders: u64, files: u64) -> Totals {
        Totals { folders, files, bytes, unreadable: 0 }
    }

    #[test]
    fn counted_folders_sort_by_their_totals() {
        let mut v = sortable_view();
        v.totals.insert("a_dir".into(), totals(500, 1, 9));
        v.totals.insert("b_dir".into(), totals(20, 3, 2));
        let by = |column| visible_rows(&v.entries, &v.totals, false, "", Sort { column, descending: false });
        let names = |rows: Vec<usize>| -> Vec<String> { rows.iter().map(|&i| v.entries[i].name.clone()).collect() };
        assert_eq!(names(by(SortColumn::Size))[..2], ["b_dir", "a_dir"]);
        assert_eq!(names(by(SortColumn::Folders))[..2], ["a_dir", "b_dir"]);
        assert_eq!(names(by(SortColumn::Files))[..2], ["b_dir", "a_dir"]);
        // Files have no counts: they tie and keep name order.
        assert_eq!(names(by(SortColumn::Files))[2..], ["a.txt", "b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn type_sorts_by_kind_then_name() {
        let mut v = View::default();
        let entries = vec![entry("z_dir", true), file("a.txt", None, None), file("b.md", None, None), file("z.md", None, None)];
        v.apply_listing(ok_listing("/t", entries), None);
        v.sort_by(SortColumn::Type);
        assert_eq!(row_names(&v), ["z_dir", "b.md", "z.md", "a.txt"]);
    }

    #[test]
    fn a_new_count_moves_its_row_when_sorted_by_a_count_column_and_keeps_the_selection() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Size);
        v.sort_by(SortColumn::Size); // descending
        v.selected = Some(0);
        assert_eq!(v.selected_entry().unwrap().name, "a_dir");
        v.set_totals("b_dir".into(), totals(100, 0, 1));
        assert_eq!(row_names(&v)[..2], ["b_dir", "a_dir"]);
        assert_eq!(v.selected_entry().unwrap().name, "a_dir", "selection follows the entry");
        assert_eq!(v.totals.get("b_dir"), Some(&totals(100, 0, 1)));
    }

    #[test]
    fn totals_survive_a_refresh_but_not_another_folder() {
        let mut v = sortable_view();
        v.set_totals("a_dir".into(), totals(1, 0, 1));
        let again = v.entries.clone();
        v.apply_refresh(ok_listing("/s", again.clone()));
        assert!(v.totals.contains_key("a_dir"));
        v.apply_listing(ok_listing("/s", again.clone()), None);
        assert!(v.totals.contains_key("a_dir"), "reloading the same folder keeps them");
        v.apply_listing(ok_listing("/other", again), None);
        assert!(v.totals.is_empty());
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

    fn type_str(v: &mut View, text: &str) {
        for c in text.chars() {
            v.push_filter(c);
        }
    }

    #[test]
    fn typing_filters_by_case_insensitive_contains_and_selects_the_first_match() {
        let mut v = search_view();
        v.selected = Some(3); // notes.txt
        type_str(&mut v, "Do");
        assert_eq!(v.filter(), "Do");
        assert_eq!(row_names(&v), ["Documents", "downloads"], "hidden .docker stays out");
        assert_eq!(v.selected_entry().unwrap().name, "Documents");
        assert_eq!(v.scroll, Some(Scroll::Instant));
        type_str(&mut v, "w");
        assert_eq!(row_names(&v), ["downloads"]);
        // Contains, not only prefix: "top" is inside "Desktop".
        v.clear_filter();
        type_str(&mut v, "TOP");
        assert_eq!(row_names(&v), ["Desktop"]);
    }

    #[test]
    fn a_space_after_other_characters_is_part_of_the_filter() {
        let mut v = View::default();
        let entries = vec![entry("my doc.txt", false), entry("mydoc.txt", false)];
        v.apply_listing(ok_listing("/s", entries), None);
        type_str(&mut v, "my d");
        assert_eq!(row_names(&v), ["my doc.txt"]);
    }

    #[test]
    fn no_match_empties_the_list_and_the_selection() {
        let mut v = search_view();
        type_str(&mut v, "zzz");
        assert_eq!(row_names(&v), Vec::<&str>::new());
        assert_eq!(v.selected, None);
        assert!(v.selected_entry().is_none());
        assert_eq!(v.entries.len(), 5, "entries are filtered, never dropped");
    }

    #[test]
    fn backspace_erases_one_character_and_reports_when_there_was_none() {
        let mut v = search_view();
        type_str(&mut v, "dow");
        assert!(v.pop_filter());
        assert_eq!(v.filter(), "do");
        assert_eq!(row_names(&v), ["Documents", "downloads"]);
        assert!(v.pop_filter());
        assert!(v.pop_filter());
        assert_eq!(v.filter(), "");
        assert_eq!(row_names(&v), ["Desktop", "Documents", "downloads", "notes.txt"]);
        assert!(!v.pop_filter(), "empty filter: Backspace goes up instead");
    }

    #[test]
    fn clearing_the_filter_keeps_the_selected_entry() {
        let mut v = search_view();
        type_str(&mut v, "load");
        assert!(v.clear_filter());
        assert_eq!(v.filter(), "");
        assert_eq!(row_names(&v), ["Desktop", "Documents", "downloads", "notes.txt"]);
        assert_eq!(v.selected_entry().unwrap().name, "downloads");
        assert_eq!(v.scroll, Some(Scroll::Instant));
        assert!(!v.clear_filter(), "nothing to clear");
    }

    #[test]
    fn hidden_rows_are_not_matched_until_shown_and_the_filter_survives_ctrl_h() {
        let mut v = search_view();
        type_str(&mut v, ".d");
        assert_eq!(row_names(&v), Vec::<&str>::new());
        v.toggle_hidden();
        assert_eq!(v.filter(), ".d");
        assert_eq!(row_names(&v), [".docker"]);
    }

    #[test]
    fn changing_folder_clears_the_filter() {
        let mut v = search_view();
        type_str(&mut v, "do");
        v.apply_listing(ok_listing("/y", vec![entry("notes", true), entry("a", false)]), None);
        assert_eq!(v.filter(), "");
        assert_eq!(row_names(&v), ["notes", "a"]);
    }

    #[test]
    fn reloading_the_same_folder_keeps_the_filter_unless_it_hides_the_entry_to_select() {
        let mut v = search_view();
        type_str(&mut v, "do");
        let mut entries: Vec<Entry> = v.entries.clone();
        entries.push(entry("docs", true));
        v.apply_listing(ok_listing("/home/u", entries.clone()), None);
        assert_eq!(v.filter(), "do");
        v.apply_listing(ok_listing("/home/u", entries.clone()), Some("docs"));
        assert_eq!(v.filter(), "do");
        assert_eq!(v.selected_entry().unwrap().name, "docs");
        // A new folder or a renamed entry that does not match must still show up.
        entries.push(entry("New Folder", true));
        v.apply_listing(ok_listing("/home/u", entries), Some("New Folder"));
        assert_eq!(v.filter(), "");
        assert_eq!(v.selected_entry().unwrap().name, "New Folder");
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
    fn a_refresh_keeps_sort_hidden_filter_text_filter_and_error() {
        let mut v = sortable_view();
        v.sort_by(SortColumn::Size);
        v.sort_by(SortColumn::Size); // descending
        v.push_filter('t');
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
        assert_eq!(row_names(&v), ["e.txt", "b.txt"], "size descending, hidden and \"t\" filtered");
        assert_eq!(v.filter(), "t");
        assert_eq!(v.error.as_deref(), Some("Cannot copy x"));
        assert_eq!(v.last_elapsed, Some(Duration::from_millis(3)));
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
