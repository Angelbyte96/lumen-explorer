//! Application state and the per-frame UI loop.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::{Config, Settings};
use crate::folder_views::{FolderViews, Look};
use crate::fs::count::{Count, Totals};
use crate::fs::index::Crawler;
use crate::fs::watch::Watcher;
use crate::fs::{self, Entry, Lister, Listing, Readers};
use crate::input::{self, Action, Confirm};
use crate::menu::Command;
#[cfg(not(windows))]
use crate::menu;
use crate::places::{self, Bookmarks, Disk, DiskReader, DriveInfo, DriveKind, Mount, Place, Recents, Usage, Visits};
use crate::fs::ops::Renamed;
use crate::fs::preview::Previewer;
use crate::fs::transfer::{self, Job, Mode, Outcome, Plan};
use crate::recycle_bin::{self, Recycled};
use crate::ui::drives_view::{self, DriveRow, DriveSort, DrivesEvent};
use crate::ui::recycle_view::{self, Picks, RecycleEvent, RecycleSort};
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::tabs::TabEvent;
use crate::ui::toolbar::{Nav, ToolbarEvent};
use crate::ui::context_menu::{ContextMenu, MenuEvent};
use crate::ui::tree::Tree;
use crate::ui::{self, file_list::{ListEvent, RenameEdit}, path_popup::{PathPopup, PopupEvent}, preview::PreviewPane, status_bar::{PickedSize, Status}};

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
    /// Index into `rows`; outlined only while `unselected`.
    pub selected: Option<usize>,
    /// A click on the empty space unselected everything: the cursor stays on
    /// `selected`, outlined, but nothing is selected until a click or a key.
    unselected: bool,
    pub show_hidden: bool,
    /// Kept across folders, like the hidden-files toggle.
    pub sort: Sort,
    pub last_elapsed: Option<Duration>,
    pub error: Option<String>,
    /// Set when the selection changed via keyboard, so the table scrolls to it.
    pub scroll: Option<Scroll>,
    /// What was typed in the filter bar; rows must contain each of its words,
    /// in any order, ignoring case (like the Ctrl+L box). A name equal to it
    /// is listed first.
    filter: String,
    /// Folder totals computed with Alt+R, by path. Kept while the folder
    /// stays on screen, dropped when another folder is shown.
    pub totals: HashMap<PathBuf, Totals>,
    /// Entries picked with Ctrl+A, Ctrl+click or Shift, by path so a refresh,
    /// a sort or the filter does not move them (and two unfolded folders can
    /// hold the same name). Only the rows shown count.
    /// Empty: the selection is the cursor row alone.
    picks: HashSet<PathBuf>,
    /// While a Shift band is dragged: what was selected before it, kept
    /// under the rows it touches.
    band_base: Option<HashSet<PathBuf>>,
    /// Entry a Shift range starts from.
    anchor: Option<PathBuf>,
    /// The Tree view's unfolded folders.
    pub tree: Tree,
    /// What the unfolded folders hold, as the rows show it: `rows` index it
    /// after `entries`.
    kids: Vec<Entry>,
    /// How deep each row is, as `rows`; empty (all 0) outside the Tree view.
    depths: Vec<usize>,
    /// An entry inside an unfolded folder to select once that folder's read
    /// lands (a renamed child is not listed under its new name before).
    want: Option<PathBuf>,
    /// Ctrl+E: the folders whose contents are listed all the way down, with
    /// the folder's own entries (the folder itself for all of it). Empty: not flattened.
    flat: Vec<PathBuf>,
    /// What the walk of `flat` found, once it landed.
    flat_found: Vec<Entry>,
}

impl View {
    /// Applies a finished listing. On error the previous folder stays on screen.
    /// `select_name` re-selects an entry, e.g. the folder we just left with Backspace.
    /// A new folder starts unfiltered; reloading the same one keeps the filter
    /// unless it would hide `select_name` (a new folder, a renamed entry).
    #[cfg(test)]
    pub fn apply_listing(&mut self, listing: Listing, select_name: Option<&str>) {
        self.apply_listing_at(listing, select_name, None);
    }

    /// The same as `apply_listing`, that can also select an entry by its full path: one
    /// of the folder's own or one inside an unfolded folder. A path wins over
    /// a name; the filter is only cleared to show an entry of the folder itself.
    pub fn apply_listing_at(&mut self, listing: Listing, select_name: Option<&str>, select_path: Option<&Path>) {
        match listing.result {
            Ok(entries) => {
                let same_folder = listing.path == self.current;
                if !same_folder {
                    self.filter.clear();
                    self.totals.clear();
                    self.drop_picks();
                    self.unselected = false;
                    self.tree.clear();
                    self.flat.clear();
                    self.flat_found.clear();
                }
                // Reloading with folders unfolded: the cursor stays where it was.
                let before = (same_folder && !self.tree.is_empty()).then(|| (self.selected_entry().map(|e| e.path.clone()), self.selected));
                self.current = listing.path;
                self.entries = entries;
                self.last_elapsed = Some(listing.elapsed);
                self.error = None;
                self.tree.prune(&self.entries);
                self.tree.refresh();
                self.rebuild_rows(None);
                let hidden_name = select_name.is_some_and(|name| self.row_of_name(name).is_none());
                let hidden_path = select_path
                    .is_some_and(|path| self.row_of_path(path).is_none() && path.parent() == Some(self.current.as_path()) && self.entries.iter().any(|e| e.path == path));
                if (hidden_name || hidden_path) && !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild_rows(None);
                }
                self.want = select_path
                    .filter(|path| self.row_of_path(path).is_none() && path.ancestors().skip(1).any(|folder| self.tree.is_open(folder)))
                    .map(Path::to_path_buf);
                let wanted = select_path
                    .and_then(|path| self.row_of_path(path))
                    .or_else(|| select_name.and_then(|name| self.row_of_name(name)));
                let stay = before.and_then(|(path, row)| self.row_of_path(&path?).or_else(|| Some(row?.min(self.rows.len().checked_sub(1)?))));
                self.selected = wanted.or(stay).or((!self.rows.is_empty()).then_some(0));
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
        let keep = self.selected_entry().map(|e| e.path.clone());
        let row = self.selected;
        self.entries = entries;
        self.last_elapsed = Some(listing.elapsed);
        self.tree.prune(&self.entries);
        self.tree.refresh();
        self.rebuild_rows(None);
        let last = self.rows.len().checked_sub(1);
        self.selected = keep
            .and_then(|path| self.row_of_path(&path))
            .or_else(|| Some(row?.min(last?)));
    }

    pub fn toggle_hidden(&mut self) {
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.show_hidden = !self.show_hidden;
        self.rebuild_rows(keep.as_deref());
        self.scroll = Some(Scroll::Instant);
    }

    pub fn sort_by(&mut self, column: SortColumn) {
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.sort = self.sort.clicked(column);
        self.rebuild_rows(keep.as_deref());
        self.scroll = Some(Scroll::Instant);
    }

    /// Stores a finished count; a list sorted by a count column moves the row.
    pub fn set_totals(&mut self, path: PathBuf, totals: Totals) {
        self.totals.insert(path, totals);
        if matches!(self.sort.column, SortColumn::Folders | SortColumn::Files | SortColumn::Size) {
            let keep = self.selected_entry().map(|e| e.path.clone());
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
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.filter.clear();
        self.refilter(keep.as_deref());
        true
    }

    fn refilter(&mut self, keep: Option<&Path>) {
        self.rebuild_rows(keep);
        self.scroll = Some(Scroll::Instant);
    }

    pub fn move_selection(&mut self, action: Action) {
        self.unselected = false;
        self.drop_picks();
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

    /// An arrow key in a view laid out in lines: the cursor to `row` alone.
    pub fn move_to(&mut self, row: usize) {
        self.unselected = false;
        self.drop_picks();
        self.selected = Some(row.min(self.rows.len().saturating_sub(1)));
        self.scroll = Some(Scroll::Smooth);
    }

    /// A plain click: this row alone.
    pub fn select_row(&mut self, row: usize) {
        self.unselected = false;
        self.drop_picks();
        self.selected = Some(row);
    }

    /// Ctrl+A: every row shown.
    /// Ctrl+A: every row of the folder the cursor is in. In the Tree view a
    /// cursor inside an unfolded folder picks what that folder holds, and one
    /// at the top picks the top rows, not the unfolded children.
    /// Flattened, every row shown.
    pub fn select_all(&mut self) {
        self.unselected = false;
        let folder = self.selected_entry().and_then(|e| e.path.parent()).unwrap_or(&self.current).to_path_buf();
        let flat = self.is_flat();
        self.picks = self
            .rows
            .iter()
            .map(|&i| &self.entry(i).path)
            .filter(|path| flat || path.parent() == Some(folder.as_path()))
            .cloned()
            .collect();
    }

    /// Ctrl+click: adds or removes `row`. The first one keeps the cursor row
    /// picked, as it was the selection until then.
    pub fn toggle_pick(&mut self, row: usize) {
        let Some(&idx) = self.rows.get(row) else { return };
        self.want = None;
        // An unselected cursor (only outlined) was not selected: it stays out.
        if self.picks.is_empty()
            && let Some(cursor) = self.selected_entry()
        {
            self.picks.insert(cursor.path.clone());
        }
        self.unselected = false;
        let path = self.entry(idx).path.clone();
        if !self.picks.remove(&path) {
            self.picks.insert(path.clone());
        }
        self.anchor = Some(path);
        self.selected = Some(row);
    }

    /// Shift+click: the rows from the anchor (the last plain or Ctrl click) to `row`.
    pub fn extend_to(&mut self, row: usize) {
        self.unselected = false;
        if row >= self.rows.len() {
            return;
        }
        self.want = None;
        let anchor = self
            .anchor
            .as_deref()
            .and_then(|path| self.row_of_path(path))
            .or(self.selected)
            .unwrap_or(row);
        self.anchor = Some(self.entry(self.rows[anchor]).path.clone());
        let (from, to) = (anchor.min(row), anchor.max(row));
        self.picks = self.rows[from..=to].iter().map(|&i| self.entry(i).path.clone()).collect();
        self.selected = Some(row);
    }

    /// Shift+arrows: moves the cursor `delta` rows, selecting the range.
    pub fn extend_by(&mut self, delta: isize) {
        let Some(last) = self.rows.len().checked_sub(1) else { return };
        let row = self.selected.unwrap_or(0).saturating_add_signed(delta).min(last);
        self.extend_to(row);
        self.scroll = Some(Scroll::Smooth);
    }

    /// A band dragged with the mouse from `start` to `end`, in row units (row
    /// `i` spans `[i, i + 1)`, so a scroll does not move the start): picks every
    /// row it touches and moves the cursor to the row at `end`. Over no row it
    /// drops the picks, like a click below the rows. With `add` (Shift held)
    /// the rows it touches join what was selected before it.
    pub fn select_band(&mut self, start: f32, end: f32, add: bool) {
        let base = self.band_base(add);
        self.unselected = false;
        let Some(last) = self.rows.len().checked_sub(1) else { return };
        let (low, high) = (start.min(end), start.max(end));
        if high < 0.0 || low >= self.rows.len() as f32 {
            self.keep_base(base);
            return;
        }
        self.want = None;
        // Saturating casts: anything above the rows is row 0.
        let row = |at: f32| (at.floor().max(0.0) as usize).min(last);
        let (from, to) = (row(low), row(high));
        let touched: Vec<PathBuf> = self.rows[from..=to].iter().map(|&i| self.entry(i).path.clone()).collect();
        self.picks = base;
        self.picks.extend(touched);
        self.anchor = Some(self.entry(self.rows[row(start)]).path.clone());
        self.selected = Some(row(end));
    }

    /// A band over the cards of a view laid out in lines: picks `cells` (rows
    /// shown) and moves the cursor to `cursor`. Over no card it drops the picks.
    /// With `add` the cards join what was selected before the band.
    pub fn select_cells(&mut self, cells: &[usize], cursor: Option<usize>, add: bool) {
        let base = self.band_base(add);
        self.unselected = false;
        let cells: Vec<usize> = cells.iter().copied().filter(|&row| row < self.rows.len()).collect();
        if cells.is_empty() {
            self.keep_base(base);
            return;
        }
        self.want = None;
        let touched: Vec<PathBuf> = cells.iter().map(|&row| self.entry(self.rows[row]).path.clone()).collect();
        self.picks = base;
        self.picks.extend(touched);
        let cursor = cursor.filter(|row| cells.contains(row)).unwrap_or(cells[cells.len() - 1]);
        self.anchor = Some(self.entry(self.rows[cells[0]]).path.clone());
        self.selected = Some(cursor);
    }

    /// The band was let go: the next Shift band starts from the selection then.
    pub fn end_band(&mut self) {
        self.band_base = None;
    }

    /// A band over nothing: what it started from, or no picks at all.
    fn keep_base(&mut self, base: HashSet<PathBuf>) {
        if base.is_empty() {
            self.drop_picks();
        } else {
            self.picks = base;
        }
    }

    /// With `add`, what the band started from (taken on its first frame):
    /// the picks, else the cursor row when it is drawn selected.
    fn band_base(&mut self, add: bool) -> HashSet<PathBuf> {
        if !add {
            self.band_base = None;
            return HashSet::new();
        }
        if self.band_base.is_none() {
            let mut base: HashSet<PathBuf> = self.picks.clone();
            if base.is_empty()
                && !self.unselected
                && let Some(&i) = self.selected.and_then(|row| self.rows.get(row))
            {
                base.insert(self.entry(i).path.clone());
            }
            self.band_base = Some(base);
        }
        self.band_base.clone().unwrap_or_default()
    }

    /// A click on the empty space or Escape: nothing is selected any more, the cursor
    /// stays where it was with a dashed outline.
    pub fn unselect(&mut self) {
        self.drop_picks();
        self.unselected = self.selected.is_some();
    }

    /// Whether `row` is drawn selected: picked, or the cursor when nothing is picked.
    pub fn is_highlighted(&self, row: usize) -> bool {
        if self.picked_count() > 0 { self.is_picked(row) } else { !self.unselected && self.selected == Some(row) }
    }

    /// Whether `row` is the unselected cursor, drawn with a dashed outline.
    pub fn is_outlined(&self, row: usize) -> bool {
        self.unselected && self.picked_count() == 0 && self.selected == Some(row)
    }

    /// Back to the cursor row alone. `false` when nothing was picked.
    pub fn drop_picks(&mut self) -> bool {
        self.anchor = None;
        self.want = None;
        !std::mem::take(&mut self.picks).is_empty()
    }

    pub fn is_picked(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|&i| self.picks.contains(&self.entry(i).path))
    }

    /// The folders picked among the rows shown (not the cursor row alone).
    pub fn picked_folders(&self) -> Vec<PathBuf> {
        (0..self.rows.len())
            .filter(|&row| self.is_picked(row))
            .map(|row| self.entry(self.rows[row]))
            .filter(|e| e.is_dir)
            .map(|e| e.path.clone())
            .collect()
    }

    /// Picked rows shown.
    pub fn picked_count(&self) -> usize {
        (0..self.rows.len()).filter(|&row| self.is_picked(row)).count()
    }

    /// The picked rows' total size, for the status bar.
    pub fn picked_size(&self) -> PickedSize {
        let mut size = PickedSize::default();
        for row in (0..self.rows.len()).filter(|&row| self.is_picked(row)) {
            let entry = self.entry(self.rows[row]);
            let bytes = if entry.is_dir { self.totals.get(&entry.path).map(|t| t.bytes) } else { entry.size };
            match bytes {
                Some(bytes) => size.bytes += bytes,
                None if entry.is_dir => size.uncounted += 1,
                // An unreadable file: nothing to add.
                None => {}
            }
        }
        size
    }

    /// What an operation acts on, in display order: the picked rows shown, or
    /// the cursor row when none is.
    /// What the actions act on: the picked rows, or the cursor row. An entry
    /// inside a picked unfolded folder goes with that folder, so it is left
    /// out: acting on it again would fail once the folder is gone, or copy it twice.
    pub fn selection(&self) -> Vec<&Entry> {
        let picked: Vec<&Entry> = (0..self.rows.len())
            .filter(|&row| self.is_picked(row))
            .map(|row| self.entry(self.rows[row]))
            .collect();
        if picked.is_empty() {
            return self.selected_entry().into_iter().collect();
        }
        let folders: Vec<&Path> = picked.iter().filter(|e| e.is_dir).map(|e| e.path.as_path()).collect();
        picked.into_iter().filter(|e| !folders.iter().any(|f| e.path != *f && e.path.starts_with(f))).collect()
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        if self.unselected {
            return None;
        }
        self.selected
            .and_then(|i| self.rows.get(i))
            .map(|&idx| self.entry(idx))
    }

    /// Entry to select once the selection is gone: the first row left below
    /// it, or the nearest one above when none is. Rows inside a removed
    /// unfolded folder go with it.
    pub fn path_after_removal(&self) -> Option<PathBuf> {
        let removed = |row: usize| if self.picked_count() > 0 { self.is_picked(row) } else { Some(row) == self.selected };
        let removed_paths: Vec<&Path> = (0..self.rows.len()).filter(|&row| removed(row)).map(|row| self.entry(self.rows[row]).path.as_path()).collect();
        let gone = |row: usize| removed_paths.iter().any(|path| self.entry(self.rows[row]).path.starts_with(path));
        let last = (0..self.rows.len()).rfind(|&row| gone(row))?;
        let row = (last + 1..self.rows.len())
            .find(|&row| !gone(row))
            .or_else(|| (0..last).rfind(|&row| !gone(row)))?;
        Some(self.entry(self.rows[row]).path.clone())
    }

    /// The selected entry when exactly one is (rename, count).
    pub fn single(&self) -> Option<&Entry> {
        match self.selection().as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// Folders among the rows shown.
    pub fn folder_count(&self) -> usize {
        self.rows.iter().filter(|&&i| self.entry(i).is_dir).count()
    }

    /// Hidden entries that could be shown: flattened, the ones inside too.
    pub fn hidden_count(&self) -> usize {
        let listed = if self.is_flat() { &self.kids } else { &self.entries };
        listed.iter().filter(|e| e.hidden).count()
    }

    /// The entry a `rows` index stands for: one of the folder's own, or one
    /// of what an unfolded folder holds.
    pub fn entry(&self, index: usize) -> &Entry {
        match index.checked_sub(self.entries.len()) {
            None => &self.entries[index],
            Some(kid) => &self.kids[kid],
        }
    }

    /// Whether `path` is listed on screen, in the folder or an unfolded one.
    pub fn lists(&self, path: &Path) -> bool {
        self.entries.iter().chain(&self.kids).any(|e| e.path == path)
    }

    /// How many unfolded folders lie between `row` and the folder on screen.
    pub fn depth(&self, row: usize) -> usize {
        self.depths.get(row).copied().unwrap_or(0)
    }

    /// Whether `row` is a folder unfolded in the Tree view.
    pub fn is_open(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|&i| self.entry(i).is_dir && self.tree.is_open(&self.entry(i).path))
    }

    /// Unfolds the folder in `row`, or folds it when it is unfolded. The
    /// cursor stays on its entry, or goes to the folder when it was inside it.
    pub fn toggle_open(&mut self, row: usize) {
        let Some(&index) = self.rows.get(row) else { return };
        let folder = self.entry(index);
        if !folder.is_dir {
            return;
        }
        let folder = folder.path.clone();
        let cursor = self.selected_entry().map(|e| e.path.clone());
        if self.tree.is_open(&folder) {
            self.tree.fold(&folder);
            self.want.take_if(|want| want.starts_with(&folder));
            self.picks.retain(|pick| !pick.starts_with(&folder) || *pick == folder);
            let keep = cursor.filter(|c| !c.starts_with(&folder) || *c == folder).unwrap_or(folder);
            self.rebuild_rows(Some(&keep));
        } else {
            self.tree.unfold(folder);
            self.rebuild_rows(cursor.as_deref());
        }
    }

    /// Right or Left in the Tree view (`delta` is 1 or -1) on the cursor row:
    /// Right unfolds a folded folder, else goes to the first child; Left folds
    /// an unfolded folder, else goes to the parent folder.
    pub fn tree_side(&mut self, delta: isize) {
        let Some(row) = self.selected.filter(|&row| row < self.rows.len()) else { return };
        let depth = self.depth(row);
        if delta > 0 && self.entry(self.rows[row]).is_dir {
            if !self.is_open(row) {
                self.toggle_open(row);
            } else if self.depth(row + 1) > depth {
                self.move_to(row + 1);
            }
        } else if delta < 0 {
            if self.is_open(row) {
                self.toggle_open(row);
            } else if let Some(parent) = (0..row).rfind(|&above| self.depth(above) < depth) {
                self.move_to(parent);
            }
        }
    }

    /// Stores what was read of an unfolded folder and shows it.
    pub fn apply_kids(&mut self, listing: Listing) {
        let listing_path = listing.path.clone();
        if !self.tree.set(&listing.path, listing.id, listing.result) {
            return;
        }
        self.tree.prune(&self.entries);
        let keep = self.selected_entry().map(|e| e.path.clone());
        let row = self.selected;
        self.rebuild_rows(None);
        let last = self.rows.len().checked_sub(1);
        self.selected = keep
            .and_then(|path| self.row_of_path(&path))
            .or_else(|| Some(row?.min(last?)));
        // The entry asked for, once its folder is read: listed now, or gone.
        if let Some(want) = self.want.take_if(|want| want.parent() == Some(listing_path.as_path()))
            && let Some(row) = self.row_of_path(&want)
        {
            self.selected = Some(row);
            self.scroll = Some(Scroll::Smooth);
        }
    }

    /// Whether the rows include what subfolders hold (Ctrl+E).
    pub fn is_flat(&self) -> bool {
        !self.flat.is_empty()
    }

    /// The folders listed all the way down; empty when not flattened.
    pub fn flat_roots(&self) -> &[PathBuf] {
        &self.flat
    }

    /// Flattens `roots`; what they hold shows once [`View::apply_flat`] brings
    /// their walk. The Tree view's unfolded folders are folded.
    pub fn set_flat(&mut self, roots: Vec<PathBuf>) {
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.tree.clear();
        self.want = None;
        self.flat = roots;
        self.flat_found.clear();
        self.rebuild_rows(keep.as_deref());
    }

    /// Lists the folder alone again. The cursor goes to the folder's own
    /// entry that held it, and picks inside subfolders are dropped.
    pub fn clear_flat(&mut self) {
        if !self.is_flat() {
            return;
        }
        let current = self.current.clone();
        let keep = self
            .selected_entry()
            .and_then(|e| e.path.ancestors().find(|a| a.parent() == Some(current.as_path())).map(Path::to_path_buf));
        self.flat.clear();
        self.flat_found.clear();
        self.picks.retain(|pick| pick.parent() == Some(current.as_path()));
        self.anchor.take_if(|anchor| anchor.parent() != Some(current.as_path()));
        self.rebuild_rows(keep.as_deref());
    }

    /// Shows a finished walk; one of other roots (an earlier Ctrl+E) is dropped.
    pub fn apply_flat(&mut self, roots: &[PathBuf], found: Vec<Entry>) {
        if roots != self.flat.as_slice() {
            return;
        }
        let keep = self.selected_entry().map(|e| e.path.clone());
        let row = self.selected;
        self.flat_found = found;
        self.rebuild_rows(None);
        let last = self.rows.len().checked_sub(1);
        self.selected = keep.and_then(|path| self.row_of_path(&path)).or_else(|| Some(row?.min(last?)));
    }

    /// Flattened, the folder an entry from a subfolder is in, relative to
    /// the one shown ("src > util"); `None` for the folder's own entries.
    pub fn relative_parent(&self, entry: &Entry) -> Option<String> {
        if !self.is_flat() {
            return None;
        }
        let rel = entry.path.parent()?.strip_prefix(&self.current).ok()?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        (!parts.is_empty()).then(|| parts.join(" > "))
    }

    /// Folds every folder (another view is shown).
    pub fn fold_all(&mut self) {
        if !self.tree.is_empty() {
            // The cursor goes to the folder row that held it, if it was inside one.
            let keep = self.selected_entry().map(|e| e.path.clone()).and_then(|path| {
                path.ancestors().find(|a| a.parent() == Some(self.current.as_path())).map(Path::to_path_buf)
            });
            self.tree.clear();
            self.want = None;
            // Picked children are hidden for good: they must not come back picked.
            let current = self.current.clone();
            self.picks.retain(|pick| pick.parent() == Some(current.as_path()));
            self.anchor.take_if(|anchor| anchor.parent() != Some(current.as_path()));
            self.rebuild_rows(keep.as_deref());
        }
    }

    fn rebuild_rows(&mut self, keep: Option<&Path>) {
        if self.is_flat() {
            // Flattened, `kids` holds every entry listed and `rows` index it.
            self.kids = flat_entries(&self.entries, &self.flat_found, &self.current);
            let offset = self.entries.len();
            self.rows = visible_rows(&self.kids, &self.totals, self.show_hidden, &self.filter, self.sort)
                .into_iter()
                .map(|i| offset + i)
                .collect();
            self.depths.clear();
        } else {
            let top = visible_rows(&self.entries, &self.totals, self.show_hidden, &self.filter, self.sort);
            (self.rows, self.kids, self.depths) = tree_rows(&self.entries, top, &self.tree, &self.totals, self.show_hidden, self.sort);
        }
        self.selected = keep
            .and_then(|path| self.row_of_path(path))
            .or((!self.rows.is_empty()).then_some(0));
    }

    /// The row of the folder's own entry called `name`.
    fn row_of_name(&self, name: &str) -> Option<usize> {
        // Flattened, every row is at depth 0: one from a subfolder can share the name.
        (0..self.rows.len()).find(|&row| {
            let entry = self.entry(self.rows[row]);
            self.depth(row) == 0 && entry.name == name && (!self.is_flat() || entry.path.parent() == Some(self.current.as_path()))
        })
    }

    fn row_of_path(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|&i| self.entry(i).path == path)
    }
}

/// The rows of the Tree view: the `top` rows with, under each unfolded
/// folder, what it holds (sorted like the rest, hidden files left out unless
/// shown, not filtered). Returns the rows, the entries of the unfolded
/// folders that they index after `entries`, and each row's depth. With no
/// folder unfolded that is `top` alone.
fn tree_rows(
    entries: &[Entry],
    top: Vec<usize>,
    tree: &Tree,
    totals: &HashMap<PathBuf, Totals>,
    show_hidden: bool,
    sort: Sort,
) -> (Vec<usize>, Vec<Entry>, Vec<usize>) {
    if tree.is_empty() {
        return (top, Vec::new(), Vec::new());
    }
    let (mut rows, mut kids, mut depths) = (Vec::new(), Vec::new(), Vec::new());
    // Each folder's children follow it, depth first.
    let mut pending: Vec<(usize, usize)> = top.into_iter().rev().map(|i| (i, 0)).collect();
    while let Some((index, depth)) = pending.pop() {
        rows.push(index);
        depths.push(depth);
        let entry = if index < entries.len() { &entries[index] } else { &kids[index - entries.len()] };
        let Some(inside) = tree.read(&entry.path).filter(|_| entry.is_dir) else { continue };
        let order = visible_rows(inside, totals, show_hidden, "", sort);
        let first = entries.len() + kids.len();
        kids.extend(order.iter().map(|&j| inside[j].clone()));
        pending.extend((0..order.len()).rev().map(|k| (first + k, depth + 1)));
    }
    (rows, kids, depths)
}

/// The folders to flatten after Ctrl+E, from those flattened now (`roots`)
/// and the folders picked: none picked, all of `current` or nothing; picked
/// ones all listed already, those taken out; else those added.
fn next_flat(roots: &[PathBuf], current: &Path, picked: &[PathBuf]) -> Vec<PathBuf> {
    let whole = roots.iter().any(|root| root == current);
    if picked.is_empty() || whole {
        return if roots.is_empty() && picked.is_empty() { vec![current.to_path_buf()] } else { Vec::new() };
    }
    let listed = |pick: &PathBuf| roots.iter().any(|root| pick.starts_with(root));
    if picked.iter().all(listed) {
        // Out with the roots holding a picked folder or held by one.
        return roots
            .iter()
            .filter(|root| !picked.iter().any(|pick| pick.starts_with(root) || root.starts_with(pick)))
            .cloned()
            .collect();
    }
    let mut next: Vec<PathBuf> = roots.to_vec();
    next.extend(picked.iter().filter(|pick| !listed(pick)).cloned());
    // A folder inside another one listed is listed with it.
    let all = next.clone();
    next.retain(|root| !all.iter().any(|other| other != root && root.starts_with(other)));
    next
}

/// The entries of a flattened view: the folder's own (as listed, fresher
/// than the walk's) and what the walk found inside its subfolders, in the
/// lister's order (folders first, by name) so `visible_rows` can rely on it.
/// What a hidden folder holds is hidden with it, so Ctrl+H hides it too.
fn flat_entries(entries: &[Entry], found: &[Entry], current: &Path) -> Vec<Entry> {
    let mut all: Vec<Entry> = entries.to_vec();
    all.extend(found.iter().filter(|e| e.path.parent() != Some(current)).map(|e| {
        let mut e = e.clone();
        let in_hidden = e.path.parent().and_then(|p| p.strip_prefix(current).ok()).is_some_and(|rel| {
            rel.components().any(|c| crate::fs::entry::is_hidden_name(&c.as_os_str().to_string_lossy()))
        });
        e.hidden |= in_hidden;
        e
    }));
    all.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase(), e.path.to_string_lossy().to_lowercase()));
    all
}

/// The folder every one of `paths` is in, when they all share one.
#[cfg(any(windows, test))]
fn shared_parent(paths: &[PathBuf]) -> Option<&Path> {
    let parent = paths.first()?.parent()?;
    paths.iter().all(|path| path.parent() == Some(parent)).then_some(parent)
}

/// Indices into `entries` that pass the hidden-files and text filters, in
/// display order. Shared by the list and the folder preview.
pub fn visible_rows(
    entries: &[Entry],
    totals: &HashMap<PathBuf, Totals>,
    show_hidden: bool,
    filter: &str,
    sort: Sort,
) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            let e = &entries[i];
            (show_hidden || !e.hidden) && crate::ui::path_popup::word_ranges(&e.name, filter).is_some()
        })
        .collect();
    // Kinds are built once, not on every comparison.
    let kinds: Vec<String> = match sort.column {
        SortColumn::Type => entries.iter().map(Entry::kind).collect(),
        _ => Vec::new(),
    };
    rows.sort_by(|&a, &b| compare(entries, totals, &kinds, sort, a, b));
    // The name typed in full goes above the partial matches (a stable sort,
    // so those keep the column order).
    rows.sort_by_key(|&i| !crate::ui::path_popup::is_exact(&entries[i].name, filter));
    rows
}

/// Orders two indices into `entries`. The lister already delivers entries
/// folders-first and by case-insensitive name, so the index itself is the name
/// order: it sorts the Name column and breaks ties without allocating.
fn compare(
    entries: &[Entry],
    totals: &HashMap<PathBuf, Totals>,
    kinds: &[String],
    sort: Sort,
    a: usize,
    b: usize,
) -> Ordering {
    let (ea, eb) = (&entries[a], &entries[b]);
    let counted = |e: &Entry, field: fn(&Totals) -> u64| totals.get(&e.path).map(field);
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

/// How long a newly picked view takes to fade in, in seconds: short, so the
/// switch still feels instant.
const VIEW_FADE: f64 = 0.12;
/// How often the side panel reads the mounted disks again (free space moves).
const DISKS_INTERVAL: Duration = Duration::from_secs(5);
/// How often the drive letters are looked at, which is cheap: a drive plugged
/// in or ejected shows up within about this.
const MASK_INTERVAL: Duration = Duration::from_millis(500);
/// How often the app wakes up, even idle, to ask the `DiskClock`: only where
/// the drive letters can be looked up.
const DISKS_WATCH: Duration = if cfg!(windows) { MASK_INTERVAL } else { DISKS_INTERVAL };

/// Wait between the first change on disk and the reload. Later changes in that
/// window ride along, so a long copy reloads a few times a second, not per file.
pub const REFRESH_DELAY: Duration = Duration::from_millis(200);

/// How long a notice stays in the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(4);

/// Clears `text` once it has been on screen for `NOTICE_TIME` (`shown`
/// remembers which text and since when); returns how long until then.
fn expire(text: &mut Option<String>, shown: &mut Option<(String, Instant)>, now: Instant) -> Option<Duration> {
    let Some(current) = text.as_ref() else {
        *shown = None;
        return None;
    };
    let since = match shown {
        Some((was, since)) if was == current => *since,
        _ => {
            *shown = Some((current.clone(), now));
            now
        }
    };
    let left = NOTICE_TIME.saturating_sub(now.saturating_duration_since(since));
    if left.is_zero() {
        *text = None;
        *shown = None;
        return None;
    }
    Some(left)
}

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

/// Decides when the side panel's disks are read again. Where the drive
/// letters can be looked up (Windows) they are checked every `MASK_INTERVAL`
/// and a change reads the disks at once; the free space is read every
/// `DISKS_INTERVAL` all the same.
#[derive(Default)]
struct DiskClock {
    mask: Option<u32>,
    checked: Option<Instant>,
}

impl DiskClock {
    /// Whether to read the disks now: `read` is when they last were, `mask`
    /// looks up the drive letters (called at most once per `MASK_INTERVAL`).
    fn due(&mut self, read: Option<Instant>, now: Instant, mask: impl FnOnce() -> Option<u32>) -> bool {
        let mut changed = false;
        if self.checked.is_none_or(|at| now.saturating_duration_since(at) >= MASK_INTERVAL) {
            self.checked = Some(now);
            if let Some(new) = mask() {
                // The first mask seen is only remembered.
                changed = self.mask.is_some_and(|old| places::drives_changed(old, new));
                self.mask = Some(new);
            }
        }
        changed || read.is_none_or(|at| now.saturating_duration_since(at) >= DISKS_INTERVAL)
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

/// What a Windows menu was opened on, kept to build it again once the
/// shell's items arrive.
#[cfg(windows)]
enum MenuBasis {
    Entries { selection: Vec<Entry>, bookmarked: bool, can_paste: bool },
    Background { can_paste: bool, sort: Sort },
    /// A side panel item: its own path, bookmarked or not.
    Place { path: PathBuf, bookmarked: bool },
}

#[cfg(windows)]
impl MenuBasis {
    /// The menu with these shell items (none: Lumen's own).
    fn items(&self, shell: Vec<crate::menu::Item>) -> Vec<crate::menu::Item> {
        match self {
            Self::Entries { selection, bookmarked, can_paste } => {
                let selection: Vec<&Entry> = selection.iter().collect();
                crate::shell_menu::entry_menu(shell, &selection, *bookmarked, *can_paste)
            }
            Self::Background { can_paste, sort } => crate::shell_menu::background_menu(shell, *can_paste, *sort),
            Self::Place { path, bookmarked } => crate::shell_menu::place_menu(shell, path, *bookmarked),
        }
    }
}

/// A destructive operation waiting for the user's Enter or Escape.
enum Dialog {
    /// The selected entries; `select` is the row to land on once they are gone.
    Trash { paths: Vec<PathBuf>, select: Option<PathBuf>, subject: ui::confirm::Subject },
    /// Shift+Delete: the same, deleted for good.
    Delete { paths: Vec<PathBuf>, select: Option<PathBuf>, subject: ui::confirm::Subject },
    /// Renaming onto an existing name; the edit resumes if the user says no.
    Replace(RenameEdit),
    /// Pasting onto an existing name.
    Paste { source: PathBuf, target: PathBuf, mode: Mode },
    /// The Recycle Bin view's Delete: the picked entries, gone for good.
    Purge { items: Vec<Recycled>, subject: ui::confirm::Subject },
}

impl Dialog {
    fn title(&self) -> &'static str {
        match self {
            Self::Trash { .. } => "Move to Trash",
            Self::Delete { .. } | Self::Purge { .. } => "Delete Permanently",
            Self::Replace(_) | Self::Paste { .. } => "Replace",
        }
    }

    /// What a trash or delete removes, shown under the message.
    fn subject(&self) -> Option<&ui::confirm::Subject> {
        match self {
            Self::Trash { subject, .. } | Self::Delete { subject, .. } | Self::Purge { subject, .. } => Some(subject),
            Self::Replace(_) | Self::Paste { .. } => None,
        }
    }

    /// It cannot be undone.
    fn danger(&self) -> bool {
        matches!(self, Self::Delete { .. } | Self::Purge { .. })
    }

    fn message(&self) -> String {
        match self {
            Self::Trash { paths, .. } => match paths.as_slice() {
                [one] => format!("Move “{}” to the trash?", file_name(one).unwrap_or_default()),
                many => format!("Move {} items to the trash?", many.len()),
            },
            Self::Delete { paths, .. } => match paths.as_slice() {
                [one] => format!("Permanently delete “{}”? This cannot be undone.", file_name(one).unwrap_or_default()),
                many => format!("Permanently delete {} items? This cannot be undone.", many.len()),
            },
            Self::Purge { items, .. } => match items.as_slice() {
                [one] => format!("Permanently delete “{}”? This cannot be undone.", one.name),
                many => format!("Permanently delete {} items? This cannot be undone.", many.len()),
            },
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
            Self::Delete { .. } | Self::Purge { .. } => "Delete",
            Self::Replace(_) | Self::Paste { .. } => "Replace",
        }
    }

    /// The line under the message saying what the keys do.
    fn hint(&self) -> &'static str {
        match self {
            Self::Paste { .. } => "Enter to replace · Esc to skip",
            Self::Delete { .. } | Self::Purge { .. } => "Enter to delete · Esc to cancel",
            Self::Trash { .. } | Self::Replace(_) => "Enter to confirm · Esc to cancel",
        }
    }

    /// The other answer (Escape): a paste goes on without this item.
    fn no_label(&self) -> &'static str {
        match self {
            Self::Paste { .. } => "Skip",
            Self::Trash { .. } | Self::Delete { .. } | Self::Purge { .. } | Self::Replace(_) => "Cancel",
        }
    }
}

/// Copy Path: one path per line.
fn paths_text(paths: &[&Path]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n")
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Lumen's own clipboard (not the system one): the entries copied or cut.
#[derive(Clone)]
struct Clip {
    paths: Vec<PathBuf>,
    mode: Mode,
}

/// A paste of the clipboard: its entries go one at a time, each as a [`Job`]
/// or a question about a taken name.
struct Batch {
    mode: Mode,
    /// The folder on screen when Ctrl+V was pressed; browsing elsewhere
    /// meanwhile does not move the rest.
    dest: PathBuf,
    /// Every entry of the paste, to tell its own from a clipboard set since.
    sources: Vec<PathBuf>,
    /// A taken name found while something else had the keyboard: asked once
    /// it is free, so a key meant for that never answers this.
    parked: Option<Dialog>,
    /// Entries not started yet, in order.
    queue: VecDeque<PathBuf>,
    total: usize,
    /// Where the last finished entry landed, to select it at the end.
    landed: Option<PathBuf>,
    /// Entries already in this folder (a cut pasted where it lives).
    in_place: Vec<PathBuf>,
}

impl Batch {
    /// Which entry is running, counted from 1, when there is more than one.
    fn item(&self) -> Option<(usize, usize)> {
        (self.total > 1).then(|| (self.total - self.queue.len(), self.total))
    }
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
    /// Reads the folders unfolded in the Tree view.
    readers: Readers,
    /// Walks what a flattened view (Ctrl+E) lists; `None` once it landed.
    flattener: Option<fs::flat::Flattener>,
    /// The flattened view needs a walk (Ctrl+E, a reload): started next frame.
    flat_due: bool,
    /// Id of the listing currently on screen (or the last one that failed).
    shown_id: u64,
    /// Entry to select once the pending listing arrives, by name.
    select_after_load: Option<String>,
    /// The same by full path, for entries that may be inside an unfolded folder.
    select_path_after_load: Option<PathBuf>,
    /// How the pending listing was started.
    pending_nav: NavKind,
    history: History,
    /// The view slider's position in this tab.
    view_size: ui::view_size::ViewSize,
    /// The folder whose saved view or default this tab shows, and that look
    /// as last applied or kept: a change of it goes to the folder's rule.
    look_for: Option<(PathBuf, Look)>,
    /// Folder just created: rename it once a listing shows it selected.
    rename_after_load: Option<PathBuf>,
    /// Error to show once the pending reload lands (a reload clears errors).
    error_after_load: Option<String>,
    /// The drive selected in the This PC view.
    drive: Option<PathBuf>,
    /// The entries picked in the Recycle Bin view.
    recycle: Picks,
}

impl Tab {
    /// A tab about to list `current`, shown, sorted and filtered for hidden files like `like`.
    fn new(current: PathBuf, like: Option<&Tab>, log: bool) -> Self {
        let mut view = View { current, ..View::default() };
        if let Some(like) = like {
            view.sort = like.view.sort;
            view.show_hidden = like.view.show_hidden;
        }
        Self {
            view,
            lister: Lister::new(log),
            readers: Readers::new(),
            flattener: None,
            flat_due: false,
            shown_id: 0,
            select_after_load: None,
            select_path_after_load: None,
            pending_nav: NavKind::New,
            history: History::default(),
            view_size: like.map_or_else(Default::default, |like| like.view_size),
            look_for: like.and_then(|like| like.look_for.clone()),
            rename_after_load: None,
            error_after_load: None,
            drive: None,
            recycle: Picks::default(),
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
    /// Where settings are saved; `None` keeps them in memory (tests).
    config_path: Option<PathBuf>,
    /// The settings last written, to save only when something changed.
    saved: Config,
    /// Inner window size in points, saved when Lumen closes.
    window: Option<[f32; 2]>,
    path_popup: PathPopup,
    /// Files' own icons (an .exe's), loaded off the UI thread.
    file_icons: crate::ui::file_icons::FileIcons,
    /// Image files' pictures for the Icons views, read off the UI thread.
    thumbnails: crate::ui::thumbnails::Thumbnails,
    /// The right-click menu, while open.
    context_menu: Option<ContextMenu>,
    /// The open menu is a row's (not the folder's): its commands act on the selection.
    context_on_entry: bool,
    /// The shell's menu behind the open one, loading or loaded, whose items it runs.
    #[cfg(windows)]
    shell_menu: Option<crate::shell_menu::native::Loader>,
    /// Where the menu shows once the shell's items are ready: it shows whole,
    /// never Lumen's items first and the shell's later.
    #[cfg(windows)]
    menu_pending: Option<egui::Pos2>,
    /// What the open menu was built from, to build it again with the shell's items.
    #[cfg(windows)]
    menu_basis: Option<MenuBasis>,
    /// A shell item running on the menu's thread: its error, once it ran.
    #[cfg(windows)]
    shell_run: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// Wakes the UI from the shell menu's thread.
    #[cfg(windows)]
    repaint: egui::Context,
    /// Lumen's window, for what the shell runs on its behalf.
    #[cfg(windows)]
    hwnd: Option<isize>,
    /// Files dropped from other apps (Wayland only; the other platforms deliver them through winit).
    #[cfg(target_os = "linux")]
    wayland_dnd: crate::wayland_dnd::Receiver,
    /// Where New ▸ finds templates (~/Templates or its translation).
    templates: Option<PathBuf>,
    dialog: Option<Dialog>,
    rename: Option<RenameEdit>,
    clipboard: Option<Clip>,
    /// Copy or move running in the background; one at a time.
    job: Option<Running>,
    /// The paste that job belongs to, with the entries still to go.
    batch: Option<Batch>,
    bookmarks: Bookmarks,
    recents: Recents,
    /// Folders visited, scored for the quick jump.
    visits: Visits,
    /// Home, Downloads... and the trash, found once at startup.
    places: Vec<Place>,
    /// Where trashed files live; nothing in it may be trashed again.
    trash: Option<PathBuf>,
    /// Mounted disks with their usage, and when they were read.
    disks: (Option<Instant>, Vec<(Mount, Option<Usage>)>),
    /// What each disk's last read found about its kind and file system.
    disk_infos: HashMap<PathBuf, DriveInfo>,
    /// The read of the disks under way on its own thread, if any.
    disk_reader: DiskReader,
    /// When they are read again.
    disk_clock: DiskClock,
    /// The This PC view's rows: `disks` with each drive's type and file system.
    drives: Vec<DriveRow>,
    /// How the This PC view's headers sorted the drives, if one was clicked.
    drive_sort: Option<DriveSort>,
    /// The This PC view's column widths (not saved).
    drive_columns: drives_view::Widths,
    /// Fits them while no border was dragged by hand.
    drive_fit: ui::file_list::AutoFit,
    /// The Recycle Bin view's entries of every drive, ordered as `recycle_sort` says.
    recycled: Vec<Recycled>,
    recycle_sort: Option<RecycleSort>,
    /// The Recycle Bin view's column widths (not saved).
    recycle_columns: recycle_view::Widths,
    recycle_fit: ui::file_list::AutoFit,
    /// The bins are to be read again as soon as they are shown.
    recycle_stale: bool,
    /// Whether the view was on screen last frame: showing it again reads the bins.
    recycle_shown: bool,
    recycle_reader: recycle_bin::Reader,
    /// A row to bring into view after a key moved the cursor.
    recycle_scroll: Option<usize>,
    sidebar_open: bool,
    /// The view and order of a folder with no saved rule.
    default_look: Look,
    /// Saved view settings per folder.
    folder_views: FolderViews,
    /// The cells' lines as last drawn, in views laid out in lines (Icons);
    /// the arrows move through them.
    grid: Option<ui::grid::GridShape>,
    /// When (egui time, seconds) a click on a grey folder opened its folder,
    /// so the second click of a double click there opens nothing.
    folder_opened_at: Option<f64>,
    /// The view last drawn and when (egui time, seconds) the list started
    /// showing it: a new view fades in for `VIEW_FADE`.
    shown_layout: (ui::view_size::Layout, f64),
    /// The list's column widths.
    columns: ui::file_list::Widths,
    /// Which list columns are shown.
    shown_columns: ui::file_list::Shown,
    /// The list's columns from left to right.
    column_order: ui::file_list::Order,
    /// Folder size and counts being computed (Alt+R); one at a time.
    counting: Option<Count>,
    /// Informational line in the status bar ("1 item copied").
    notice: Option<String>,
    /// The notice on screen and when it first showed, to clear it later.
    notice_shown: Option<(String, Instant)>,
    /// The same for the red error line.
    error_shown: Option<(String, Instant)>,
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
    // Ctrl+Plus / Ctrl+Minus move the view slider (Action::ZoomIn), they do
    // not zoom the whole window as egui would.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
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
            config_path: None,
            saved: Config::default(),
            window: None,
            path_popup: PathPopup::default(),
            file_icons: Default::default(),
            thumbnails: Default::default(),
            context_menu: None,
            context_on_entry: false,
            #[cfg(windows)]
            shell_menu: None,
            #[cfg(windows)]
            menu_pending: None,
            #[cfg(windows)]
            menu_basis: None,
            #[cfg(windows)]
            shell_run: None,
            #[cfg(windows)]
            repaint: ctx.clone(),
            #[cfg(windows)]
            hwnd: None,
            #[cfg(target_os = "linux")]
            wayland_dnd: Default::default(),
            templates: None,
            dialog: None,
            rename: None,
            clipboard: None,
            batch: None,
            job: None,
            counting: None,
            bookmarks: Bookmarks::default(),
            recents: Recents::default(),
            visits: Visits::default(),
            places: Vec::new(),
            trash: None,
            disks: (None, Vec::new()),
            disk_infos: HashMap::new(),
            disk_reader: DiskReader::default(),
            disk_clock: DiskClock::default(),
            drives: Vec::new(),
            drive_sort: None,
            drive_columns: drives_view::DEFAULT_WIDTHS,
            drive_fit: ui::file_list::AutoFit::default(),
            recycled: Vec::new(),
            recycle_sort: Some(RecycleSort::NEWEST),
            recycle_columns: recycle_view::DEFAULT_WIDTHS,
            recycle_fit: ui::file_list::AutoFit::default(),
            recycle_stale: false,
            recycle_shown: false,
            recycle_reader: recycle_bin::Reader::default(),
            recycle_scroll: None,
            sidebar_open: true,
            default_look: Look::default(),
            folder_views: FolderViews::default(),
            grid: None,
            folder_opened_at: None,
            shown_layout: (ui::view_size::Layout::Details, f64::NEG_INFINITY),
            columns: ui::file_list::DEFAULT_WIDTHS,
            shown_columns: ui::file_list::ALL_SHOWN,
            column_order: ui::file_list::DEFAULT_ORDER,
            notice: None,
            notice_shown: None,
            error_shown: None,
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
            app.places = places::with_recycle_bin(places::with_this_pc(places::places(&home, user_dirs.as_deref(), trash_place), cfg!(windows)), cfg!(windows));
            #[cfg(not(windows))]
            {
                let folders = app.places.iter().map(|p| (p.path.clone(), p.icon.to_owned())).collect();
                app.file_icons.set_setup(crate::ui::file_icons::native::Setup::from_env(folders));
            }
            app.templates = places::templates_dir(&home, user_dirs.as_deref());
            app.trash = cfg!(unix).then_some(trash);
        }
        app.navigate(ctx, start, None);
        app
    }

    /// Starts with saved settings, writing changes back to their file.
    pub fn with_config(ctx: &egui::Context, start: PathBuf, log: bool, settings: Settings) -> Self {
        let Settings { config, path, error } = settings;
        let mut app = Self::new(ctx, start, log);
        // Shown once the startup listing lands (it clears the error line).
        app.tab.error_after_load = error;
        // The startup listing has not landed yet: it is sorted with these.
        app.tab.view.sort = config.sort;
        app.tab.view.show_hidden = config.show_hidden;
        app.sidebar_open = config.sidebar;
        app.preview_open = config.preview;
        app.tab.view_size = config.view;
        app.default_look = Look { view: config.view, sort: config.sort };
        app.folder_views = config.folder_views;
        app.columns = config.columns;
        app.shown_columns = config.shown_columns;
        app.column_order = config.column_order;
        app.bookmarks = Bookmarks::from_paths(config.bookmarks);
        app.visits = Visits::from_list(config.visited);
        app.window = config.window;
        app.saved = app.settings(true);
        app.config_path = path;
        app
    }

    /// Opens what `start` says with the saved settings: one folder, or the
    /// last session's tabs (`home` stands in for folders that are gone).
    pub fn start(ctx: &egui::Context, start: Start, home: &Path, log: bool, settings: Settings) -> Self {
        let (tabs, active) = match start {
            Start::Folder(path) => (vec![path], 0),
            Start::Restore => restore_tabs(&settings.config.tabs, settings.config.active_tab, home),
        };
        let mut app = Self::with_config(ctx, tabs[active].clone(), log, settings);
        // Only here, never in `new`, and not under test: a test must not
        // crawl the machine.
        if !cfg!(test) {
            app.path_popup = PathPopup::with_crawler(start_index(ctx), Box::new(start_index));
        }
        // The others list when they are first shown (see `select_tab`).
        app.tabs = tabs
            .iter()
            .enumerate()
            .map(|(i, path)| (i != active).then(|| Tab::new(path.clone(), Some(&app.tab), log)))
            .collect();
        app.active = active;
        app
    }

    /// The tabs' folders in bar order and which one is on screen.
    fn session(&self) -> (Vec<PathBuf>, usize) {
        let folders = self.tabs.iter().map(|slot| slot.as_ref().unwrap_or(&self.tab).view.current.clone()).collect();
        (folders, self.active)
    }

    /// The settings as they are now. The window size and the session are
    /// the ones last saved unless `with_window`: they are written when Lumen closes.
    fn settings(&self, with_window: bool) -> Config {
        Config {
            sort: self.default_look.sort,
            show_hidden: self.tab.view.show_hidden,
            sidebar: self.sidebar_open,
            preview: self.preview_open,
            window: if with_window { self.window } else { self.saved.window },
            bookmarks: self.bookmarks.paths().to_vec(),
            view: self.default_look.view,
            columns: self.columns,
            shown_columns: self.shown_columns,
            column_order: self.column_order,
            tabs: if with_window { self.session().0 } else { self.saved.tabs.clone() },
            active_tab: if with_window { self.session().1 } else { self.saved.active_tab },
            visited: if with_window { self.visits.list().to_vec() } else { self.saved.visited.clone() },
            folder_views: self.folder_views.clone(),
        }
    }

    /// Writes the settings when they differ from the file.
    /// Another Lumen window may have saved meanwhile: the file is read again
    /// and only what changed here is written over it.
    fn save_settings(&mut self, with_window: bool) {
        self.keep_rule();
        let Some(path) = self.config_path.clone() else { return };
        let settings = self.settings(with_window);
        // Closing always writes: the last window closed leaves its session.
        if settings == self.saved && !with_window {
            return;
        }
        let merged = Config::load(&path).and_then(|disk| {
            let mut merged = Config::merge(&disk, &self.saved, &settings);
            if with_window {
                (merged.tabs, merged.active_tab) = (settings.tabs.clone(), settings.active_tab);
            }
            merged.save(&path).map(|()| merged)
        });
        match merged {
            Ok(merged) => {
                // Bookmarks are shared: list the other window's too. The rest
                // of this window (its sort, its panels) stays as the user left it.
                self.bookmarks = Bookmarks::from_paths(merged.bookmarks.clone());
                self.saved = Config { bookmarks: merged.bookmarks, ..settings };
            }
            Err(err) => {
                self.tab.view.error = Some(format!("Cannot save settings: {err}"));
                // The next change tries again, not every frame.
                self.saved = settings;
            }
        }
    }

    /// Fits This PC's columns to `width` while none was dragged by hand.
    fn fit_this_pc(&mut self, ctx: &egui::Context, width: f32) {
        if self.drives.is_empty() {
            return;
        }
        let (shown, drives) = (ui::file_list::ALL_SHOWN, &self.drives);
        let room = ui::file_list::room(ctx, width, &shown);
        let content = || std::array::from_fn(|column| drives_view::fit_width(ctx, drives, column));
        if let Some(widths) = self.drive_fit.fit(drives_view::signature(drives), room, &shown, &drives_view::ROLES, content) {
            self.drive_columns = widths;
        }
    }

    /// Fits the Recycle Bin's columns to `width` while none was dragged by hand.
    fn fit_recycle_bin(&mut self, ctx: &egui::Context, width: f32) {
        if self.recycled.is_empty() {
            return;
        }
        let (shown, rows, kept) = (recycle_view::SHOWN, &self.recycled, self.recycle_columns);
        let room = ui::file_list::room(ctx, width, &shown);
        // The list's six columns: the view's own four, the others unused.
        let content = || std::array::from_fn(|column| if column < recycle_view::COLUMNS.len() { recycle_view::fit_width(ctx, rows, column) } else { kept[column] });
        if let Some(widths) = self.recycle_fit.fit(recycle_view::signature(rows), room, &shown, &recycle_view::ROLES, content) {
            self.recycle_columns = widths;
        }
    }

    /// Disks for the side panel, read again when a drive comes or goes (on
    /// Windows) and every few seconds for the free space.
    fn refresh_disks(&mut self, ctx: &egui::Context) {
        self.refresh_disks_with(ctx, places::read_disks);
    }

    /// `refresh_disks` with `read` as the read. The read runs on a thread of
    /// its own, one at a time: a share that is down answers late, and until
    /// it does the rows of the last read stay.
    fn refresh_disks_with(&mut self, ctx: &egui::Context, read: impl FnOnce() -> Vec<Disk> + Send + 'static) {
        if let Some(disks) = self.disk_reader.poll() {
            self.apply_disks(disks);
        }
        // The clock is not asked while a read is out, so a drive change it would notice waits for it.
        if self.disk_reader.is_running() || !self.disk_clock.due(self.disks.0, Instant::now(), places::drive_mask) {
            return;
        }
        let ctx = ctx.clone();
        self.disk_reader.start(move || ctx.request_repaint(), read);
    }

    /// Takes in the disks a read found, in place of the ones shown.
    fn apply_disks(&mut self, disks: Vec<Disk>) {
        self.disk_infos = disks.iter().map(|disk| (disk.mount.path.clone(), disk.info.clone())).collect();
        self.disks = (Some(Instant::now()), disks.into_iter().map(|disk| (disk.mount, disk.usage)).collect());
        self.rebuild_drives();
    }

    /// The drives whose bins the Recycle Bin reads: a share's own bin is not
    /// worth waiting for, and a share that is down would hold the read.
    fn recycle_roots(&self) -> Vec<PathBuf> {
        let local = |path: &PathBuf| self.disk_infos.get(path).is_none_or(|info| info.kind != DriveKind::Network);
        self.disks.1.iter().map(|(mount, _)| mount.path.clone()).filter(local).collect()
    }

    /// Whether the tab shows This PC, whose location is no folder.
    fn on_this_pc(&self) -> bool {
        places::is_this_pc(&self.tab.view.current)
    }

    /// The This PC rows again, from the disks last read, sorted as the headers say.
    fn rebuild_drives(&mut self) {
        let infos = &self.disk_infos;
        self.drives = drives_view::rows(&self.disks.1, |path| infos.get(path).cloned().unwrap_or_default());
        drives_view::sort(&mut self.drives, self.drive_sort);
    }

    /// Whether the tab shows the Recycle Bin, whose location is no folder either.
    fn on_recycle_bin(&self) -> bool {
        places::is_recycle_bin(&self.tab.view.current)
    }

    /// Whether the tab shows a marker (This PC, the Recycle Bin), where no folder action applies.
    fn on_marker(&self) -> bool {
        places::is_marker(&self.tab.view.current)
    }

    /// Each frame: reads the bins when the view is shown (again) or was told to
    /// refresh, on a thread of its own, and takes in what a read found.
    fn poll_recycle(&mut self, ctx: &egui::Context) {
        let shown = self.on_recycle_bin();
        // Shown again: anything may have been recycled or restored meanwhile.
        if shown && !self.recycle_shown {
            self.recycle_stale = true;
        }
        self.recycle_shown = shown;
        if let Some(mut rows) = self.recycle_reader.poll() {
            recycle_view::sort(&mut rows, self.recycle_sort);
            self.recycled = rows;
            self.tab.recycle.keep(&self.recycled);
        }
        if shown && self.recycle_stale && !self.recycle_reader.is_running() {
            // The drives as they are now, though the side panel may be closed.
            self.refresh_disks(ctx);
            // No drive is known before the first read lands, which wakes the window to come back here.
            if self.disks.0.is_none() {
                return;
            }
            self.recycle_stale = false;
            let roots = self.recycle_roots();
            let ctx = ctx.clone();
            self.recycle_reader.start(move || ctx.request_repaint(), move || recycle_bin::read_all(&roots, recycle_bin::current_sid().as_deref()));
        }
    }

    /// The Recycle Bin view's click, right click or header click.
    fn handle_recycle(&mut self, ctx: &egui::Context, event: RecycleEvent) {
        // Where the right click was; mid-window if the pointer is gone.
        let pointer = |ctx: &egui::Context| ctx.input(|i| i.pointer.interact_pos()).unwrap_or_else(|| ctx.content_rect().center());
        match event {
            RecycleEvent::Select(row) => self.tab.recycle.select(&self.recycled, row),
            RecycleEvent::Toggle(row) => self.tab.recycle.toggle(&self.recycled, row),
            RecycleEvent::Extend(row) => self.tab.recycle.extend(&self.recycled, row),
            RecycleEvent::Menu(row) => {
                // A row among the picks keeps them; any other becomes the pick.
                if self.recycled.get(row).is_some_and(|item| !self.tab.recycle.is_picked(item)) {
                    self.tab.recycle.select(&self.recycled, row);
                }
                self.open_recycle_menu(pointer(ctx));
            }
            RecycleEvent::Sort(column) => {
                self.recycle_sort = Some(RecycleSort::clicked(self.recycle_sort, column));
                recycle_view::sort(&mut self.recycled, self.recycle_sort);
            }
            RecycleEvent::HeaderMenu(column) => {
                // The same two fits as This PC: its columns are fixed too.
                self.context_menu = Some(ContextMenu::new(crate::menu::drives_column_menu(column), pointer(ctx)));
                self.context_on_entry = false;
            }
            RecycleEvent::Resized(widths) => {
                self.recycle_columns = widths;
                self.recycle_fit.by_hand();
            }
            RecycleEvent::Fit(column) => self.run_command(ctx, Command::FitColumn(column)),
        }
    }

    /// The menu for the picked entries, if any are picked.
    fn open_recycle_menu(&mut self, pos: egui::Pos2) {
        if self.tab.recycle.count() > 0 {
            self.context_menu = Some(ContextMenu::new(crate::menu::recycle_menu(), pos));
            self.context_on_entry = false;
        }
    }

    /// An arrow, Home or End in the Recycle Bin view: moves the cursor, and the view follows it.
    fn move_recycle(&mut self, to: impl FnOnce(&mut Picks, &[Recycled])) {
        to(&mut self.tab.recycle, &self.recycled);
        self.recycle_scroll = self.tab.recycle.cursor_row(&self.recycled);
    }

    /// The picked entries, copied out of the view.
    fn picked_recycled(&self) -> Vec<Recycled> {
        self.tab.recycle.of(&self.recycled).into_iter().cloned().collect()
    }

    /// The picked entries' total size, for the status bar; a folder with no size in its record is uncounted.
    fn picked_recycled_size(&self) -> PickedSize {
        let mut size = PickedSize::default();
        for item in self.tab.recycle.of(&self.recycled) {
            if item.is_dir && item.size == 0 {
                size.uncounted += 1;
            } else {
                size.bytes += item.size;
            }
        }
        size
    }

    /// Restore: the picked entries go back where they were.
    fn restore_picked(&mut self) {
        let items = self.picked_recycled();
        if items.is_empty() {
            return;
        }
        let done = recycle_bin::restore_all(&items.iter().collect::<Vec<_>>());
        self.tab.view.error = done.error;
        if done.count > 0 {
            self.notice = Some(recycle_bin::restored_notice(done.count));
            // Folders may have come back.
            self.path_popup.mark_stale();
        }
        self.forget_recycled(&items);
    }

    /// Delete in the Recycle Bin view: asks, then deletes the picked entries for good.
    fn ask_purge(&mut self) {
        let items = self.picked_recycled();
        if !items.is_empty() {
            self.dialog = Some(Dialog::Purge { subject: ui::confirm::Subject::recycled(&items), items });
        }
    }

    /// The answer was yes: `items` are deleted for good.
    fn purge_recycled(&mut self, items: &[Recycled]) {
        let done = recycle_bin::purge_all(&items.iter().collect::<Vec<_>>());
        self.tab.view.error = done.error;
        self.forget_recycled(items);
    }

    /// `items` were restored or deleted: those no longer in the bin leave the
    /// view at once, and the bins are read again to be sure.
    fn forget_recycled(&mut self, items: &[Recycled]) {
        self.recycled.retain(|row| !items.iter().any(|item| item.data == row.data) || std::fs::symlink_metadata(&row.data).is_ok());
        self.tab.recycle.keep(&self.recycled);
        self.recycle_stale = true;
    }

    /// The drive picked in the This PC view, if it is still listed.
    fn picked_drive(&self) -> Option<PathBuf> {
        self.tab.drive.clone().filter(|path| self.drives.iter().any(|drive| drive.path == *path))
    }

    /// An arrow, Home or End in the This PC view: `to` is given the picked
    /// row (if any) and the last one, and returns the row to pick.
    fn move_drive(&mut self, to: impl FnOnce(Option<usize>, usize) -> usize) {
        let Some(last) = self.drives.len().checked_sub(1) else { return };
        let at = self.picked_drive().and_then(|path| self.drives.iter().position(|drive| drive.path == path));
        self.tab.drive = Some(self.drives[to(at, last).min(last)].path.clone());
    }

    /// A click, drag or key the folder's list reported this frame.
    fn handle_list(&mut self, ctx: &egui::Context, event: Option<ListEvent>) {
        if !matches!(event, Some(ListEvent::Band { .. } | ListEvent::Cells { .. })) {
            self.tab.view.end_band();
        }
        match event {
            Some(ListEvent::Select(i)) => self.tab.view.select_row(i),
            Some(ListEvent::Toggle(i)) => self.tab.view.toggle_pick(i),
            Some(ListEvent::Menu(i)) => {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    self.open_entry_menu(i, pos);
                }
            }
            Some(ListEvent::Unfold(i)) => {
                self.tab.view.toggle_open(i);
                self.read_kids(ctx);
            }
            Some(ListEvent::Extend(i)) => self.tab.view.extend_to(i),
            Some(ListEvent::Band { start, end, add }) => self.tab.view.select_band(start, end, add),
            Some(ListEvent::Cells { cells, cursor, add }) => self.tab.view.select_cells(&cells, cursor, add),
            // egui counts any click soon after another as a double click: the
            // second click of one on a grey folder lands on a row of the folder
            // it opened, which must not open.
            Some(ListEvent::Activate(_)) if self.ends_open_folder_click(ctx) => {}
            Some(ListEvent::Activate(i)) => {
                self.tab.view.select_row(i);
                self.activate_selected(ctx);
            }
            Some(ListEvent::OpenFolder(i)) => {
                self.folder_opened_at = Some(ctx.input(|i| i.time));
                self.open_folder_of(ctx, i);
            }
            Some(ListEvent::Sort(column)) => self.tab.view.sort_by(column),
            Some(ListEvent::Resized(widths)) => self.columns = widths,
            Some(ListEvent::Fit(column)) => self.run_command(ctx, Command::FitColumn(column)),
            Some(ListEvent::CloseColumn(column)) => self.run_command(ctx, Command::ToggleColumn(column)),
            Some(ListEvent::HeaderMenu(column)) => {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    self.context_menu = Some(ContextMenu::new(crate::menu::column_menu(column, &self.shown_columns, &self.column_order), pos));
                    self.context_on_entry = false;
                }
            }
            Some(ListEvent::RenameEnded(submit)) => {
                if let Some(edit) = self.rename.take()
                    && submit
                {
                    self.finish_rename(ctx, edit, false);
                }
            }
            // Below the rows: a click ends a multiple selection, a right click
            // opens the folder's menu.
            Some(ListEvent::Empty { menu: false }) => self.tab.view.unselect(),
            Some(ListEvent::Empty { menu: true }) => {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    self.open_background_menu(pos);
                }
            }
            None => {}
        }
    }

    /// The This PC view's click, double click or right click on a drive or a header.
    fn handle_drives(&mut self, ctx: &egui::Context, event: DrivesEvent) {
        let (DrivesEvent::Select(i) | DrivesEvent::Open(i) | DrivesEvent::Menu(i)) = event else {
            match event {
                DrivesEvent::Sort(column) => {
                    self.drive_sort = Some(DriveSort::clicked(self.drive_sort, column));
                    drives_view::sort(&mut self.drives, self.drive_sort);
                }
                DrivesEvent::HeaderMenu(column) => {
                    // Where the right click was; mid-window if the pointer is gone.
                    let pos = ctx.input(|i| i.pointer.interact_pos()).unwrap_or_else(|| ctx.content_rect().center());
                    self.context_menu = Some(ContextMenu::new(crate::menu::drives_column_menu(column), pos));
                    self.context_on_entry = false;
                }
                DrivesEvent::Resized(widths) => {
                    self.drive_columns = widths;
                    self.drive_fit.by_hand();
                }
                DrivesEvent::Fit(column) => self.run_command(ctx, Command::FitColumn(column)),
                DrivesEvent::Select(_) | DrivesEvent::Open(_) | DrivesEvent::Menu(_) => {}
            }
            return;
        };
        let Some(path) = self.drives.get(i).map(|drive| drive.path.clone()) else { return };
        self.tab.drive = Some(path.clone());
        match event {
            DrivesEvent::Open(_) => self.navigate(ctx, path, None),
            DrivesEvent::Menu(_) => self.handle_sidebar(ctx, SidebarEvent::Menu(path)),
            _ => {}
        }
    }

    fn handle_sidebar(&mut self, ctx: &egui::Context, event: SidebarEvent) {
        match event {
            SidebarEvent::Open(path) => self.navigate(ctx, path, None),
            SidebarEvent::Menu(path) => {
                // Where the right click was; mid-window if the pointer is gone.
                let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_else(|| ctx.content_rect().center());
                self.open_place_menu(path, pos);
            }
        }
    }

    /// Right click on a side panel item: the menu for that path, not for the
    /// list's selection.
    fn open_place_menu(&mut self, path: PathBuf, pos: egui::Pos2) {
        if places::is_marker(&path) {
            return;
        }
        let bookmarked = self.bookmarks.contains(&path);
        #[cfg(not(windows))]
        {
            self.context_menu = Some(ContextMenu::new(menu::place_menu(&path, bookmarked), pos));
        }
        // The shell's items for it, as for a list row (see `open_entry_menu`).
        // A drive's top has no parent: the drive is bound to itself.
        #[cfg(windows)]
        {
            let folder = path.parent().map_or_else(|| path.clone(), Path::to_path_buf);
            let repaint = self.repaint.clone();
            self.shell_menu = Some(crate::shell_menu::native::Loader::entries(vec![path.clone()], folder, self.hwnd, move || {
                repaint.request_repaint()
            }));
            self.menu_basis = Some(MenuBasis::Place { path, bookmarked });
            self.context_menu = None;
            self.menu_pending = Some(pos);
        }
        self.context_on_entry = false;
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
        self.tab.select_path_after_load = None;
        let ctx = ctx.clone();
        // Wake the UI as soon as the worker finishes, without polling in a loop.
        self.tab.lister.request(path, move || ctx.request_repaint());
    }

    fn activate_selected(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.tab.view.selected_entry() else { return };
        self.activate(ctx, entry.path.clone(), entry.is_dir);
    }

    /// Flattened, opens the folder row `row` is in, with that row selected.
    /// Whether a double click now is the grey folder's click followed by a
    /// second one, by egui's own double click delay.
    fn ends_open_folder_click(&mut self, ctx: &egui::Context) -> bool {
        let Some(at) = self.folder_opened_at.take() else { return false };
        let delay = ctx.options(|o| o.input_options.max_double_click_delay);
        ctx.input(|i| i.time) - at < delay
    }

    fn open_folder_of(&mut self, ctx: &egui::Context, row: usize) {
        let Some(&index) = self.tab.view.rows.get(row) else { return };
        let entry = self.tab.view.entry(index);
        if let Some(folder) = entry.path.parent() {
            let (folder, name) = (folder.to_path_buf(), entry.name.clone());
            self.navigate(ctx, folder, Some(name));
        }
    }

    fn activate(&mut self, ctx: &egui::Context, path: PathBuf, is_dir: bool) {
        if is_dir {
            self.navigate(ctx, path, None);
        } else {
            self.open_file(&path);
        }
    }

    /// Right click on row `row`: the menu for the selection, which becomes
    /// that row unless the row is already part of it.
    fn open_entry_menu(&mut self, row: usize, pos: egui::Pos2) {
        let view = &mut self.tab.view;
        let in_selection = if view.picked_count() > 0 { view.is_picked(row) } else { view.selected == Some(row) };
        if !in_selection {
            view.select_row(row);
        }
        let selection = view.selection();
        let bookmarked = matches!(selection.as_slice(), [one] if one.is_dir && self.bookmarks.contains(&one.path));
        #[cfg(not(windows))]
        let items = {
            let apps = match selection.as_slice() {
                [one] => crate::apps::apps_for(&crate::apps::Dirs::from_env(), &one.name, one.is_dir),
                _ => Vec::new(),
            };
            menu::entry_menu(&selection, bookmarked, &apps)
        };
        // The shell's items, as File Explorer shows them, come from a thread
        // of their own; the menu shows whole when they are ready (see
        // `poll_shell_menu`), and the window keeps drawing meanwhile.
        #[cfg(windows)]
        {
            let paths: Vec<PathBuf> = selection.iter().map(|e| e.path.clone()).collect();
            // The shell binds every item to one folder: entries spread over
            // several (unfolded children) get Lumen's own menu.
            let Some(folder) = shared_parent(&paths).map(Path::to_path_buf) else {
                let items = crate::shell_menu::entry_menu(Vec::new(), &selection, bookmarked, self.clipboard.is_some());
                self.context_menu = Some(ContextMenu::new(items, pos));
                self.context_on_entry = true;
                return;
            };
            let repaint = self.repaint.clone();
            self.shell_menu = Some(crate::shell_menu::native::Loader::entries(paths, folder, self.hwnd, move || {
                repaint.request_repaint()
            }));
            self.menu_basis = Some(MenuBasis::Entries {
                selection: selection.iter().map(|&e| e.clone()).collect(),
                bookmarked,
                can_paste: self.clipboard.is_some(),
            });
            self.context_menu = None;
            self.menu_pending = Some(pos);
        }
        #[cfg(not(windows))]
        {
            self.context_menu = Some(ContextMenu::new(items, pos));
        }
        self.context_on_entry = true;
    }

    /// Right click on the list's empty space: the menu for the folder itself.
    fn open_background_menu(&mut self, pos: egui::Pos2) {
        if self.on_marker() {
            return;
        }
        #[cfg(not(windows))]
        let items = {
            let templates = self.templates.as_deref().map(places::templates).unwrap_or_default();
            menu::background_menu(self.clipboard.is_some(), self.tab.view.sort, &templates)
        };
        #[cfg(windows)]
        {
            let repaint = self.repaint.clone();
            self.shell_menu = Some(crate::shell_menu::native::Loader::folder(self.tab.view.current.clone(), self.hwnd, move || {
                repaint.request_repaint()
            }));
            self.menu_basis = Some(MenuBasis::Background { can_paste: self.clipboard.is_some(), sort: self.tab.view.sort });
            self.context_menu = None;
            self.menu_pending = Some(pos);
        }
        #[cfg(not(windows))]
        {
            self.context_menu = Some(ContextMenu::new(items, pos));
        }
        self.context_on_entry = false;
    }

    /// While the menu waits for the shell: Escape or a left click cancels it
    /// (read first, so the list never sees that Escape and an answer arriving
    /// in the same frame does not show it); else it shows once ready.
    #[cfg(windows)]
    fn read_pending_menu(&mut self, ctx: &egui::Context) {
        if self.menu_pending.is_some() {
            let cancel = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) || i.pointer.primary_pressed());
            if cancel {
                self.menu_pending = None;
                self.shell_menu = None;
                self.menu_basis = None;
                return;
            }
        }
        self.poll_shell_menu();
        if self.menu_pending.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Progress);
        }
    }

    /// The shell's items arrived: the menu shows, whole. When the shell
    /// failed, it shows with Lumen's own items.
    #[cfg(windows)]
    fn poll_shell_menu(&mut self) {
        if let Some(run) = &self.shell_run {
            match run.try_recv() {
                Ok(result) => {
                    self.shell_run = None;
                    if let Err(err) = result {
                        self.tab.view.error = Some(err);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.shell_run = None,
            }
        }
        let (Some(loader), Some(pos), Some(basis)) = (&self.shell_menu, self.menu_pending, &self.menu_basis) else {
            return;
        };
        let items = match loader.items() {
            None => return,
            Some(Ok(shell)) => basis.items(shell),
            Some(Err(_)) => {
                let items = basis.items(Vec::new());
                self.shell_menu = None;
                items
            }
        };
        self.menu_pending = None;
        self.context_menu = Some(ContextMenu::new(items, pos));
    }

    /// Copy Path (Ctrl+Shift+C): the selected entries' paths as text; with
    /// nothing selected, nothing.
    fn copy_paths(&mut self, ctx: &egui::Context) {
        let selection = self.tab.view.selection();
        if selection.is_empty() {
            return;
        }
        let paths: Vec<&Path> = selection.iter().map(|e| e.path.as_path()).collect();
        ctx.copy_text(paths_text(&paths));
        self.notice = Some(if paths.len() == 1 { "Path copied".into() } else { format!("{} paths copied", paths.len()) });
    }

    fn run_command(&mut self, ctx: &egui::Context, command: Command) {
        let single = self.tab.view.single().map(|e| (e.path.clone(), e.is_dir));
        match command {
            Command::Open => {
                if let Some((path, is_dir)) = single {
                    self.activate(ctx, path, is_dir);
                }
            }
            Command::OpenWith(app) => {
                #[cfg(not(windows))]
                if let Some((path, _)) = single
                    && let Err(err) = crate::apps::open_with(&app, &path)
                {
                    self.tab.view.error = Some(err);
                }
                #[cfg(windows)]
                let _ = app;
            }
            Command::OpenInNewTab => {
                if let Some((path, true)) = single {
                    self.new_tab(ctx);
                    self.navigate(ctx, path, None);
                }
            }
            Command::Cut => self.set_clipboard(Mode::Move),
            Command::Copy => self.set_clipboard(Mode::Copy),
            Command::CopyPath => self.copy_paths(ctx),
            Command::Paste => self.paste(ctx),
            Command::PasteInto => {
                if let Some((folder, true)) = single {
                    self.paste_into(ctx, folder);
                }
            }
            Command::Rename => self.start_rename(),
            Command::Restore => self.restore_picked(),
            // In the Recycle Bin both mean the same: delete the picked entries for good.
            Command::Trash | Command::DeleteForever if self.on_recycle_bin() => self.ask_purge(),
            Command::Trash => self.ask_trash(),
            Command::DeleteForever => self.ask_delete_forever(),
            Command::ToggleBookmark => {
                if let Some((path, true)) = single {
                    self.toggle_bookmark(path);
                }
            }
            Command::Count => self.count_selected(ctx),
            Command::NewFolder => self.new_folder(ctx),
            Command::NewFile(template) => self.new_file(ctx, template.as_deref()),
            Command::OpenTerminal => {
                // The folder clicked, else the one on screen.
                let folder = match single {
                    Some((path, true)) if self.context_on_entry => path,
                    _ => self.tab.view.current.clone(),
                };
                if places::is_marker(&folder) {
                    return;
                }
                #[cfg(not(windows))]
                if let Err(err) = crate::apps::open_terminal(&folder) {
                    self.tab.view.error = Some(err);
                }
                #[cfg(windows)]
                let _ = folder;
            }
            Command::Shell(offset) => {
                #[cfg(windows)]
                if let Some(shell) = self.shell_menu.take() {
                    self.shell_run = Some(shell.invoke(offset, self.hwnd));
                }
                #[cfg(not(windows))]
                let _ = offset;
            }
            Command::Sort(column) => self.tab.view.sort_by(column),
            // This PC lists no folder: the drives are read again instead.
            Command::Refresh if self.on_this_pc() => self.disks.0 = None,
            // The Recycle Bin lists no folder either: its bins are read again.
            Command::Refresh if self.on_recycle_bin() => self.recycle_stale = true,
            Command::Refresh => {
                let keep = self.tab.view.selected_entry().map(|e| e.path.clone());
                self.reload(ctx, keep);
            }
            // This PC has columns of its own, fitted by the list's rule.
            Command::FitColumn(column) if self.on_this_pc() => {
                self.drive_columns[column] = drives_view::fit_width(ctx, &self.drives, column);
                self.drive_fit.by_hand();
            }
            Command::FitAllColumns if self.on_this_pc() => {
                let content = std::array::from_fn(|column| drives_view::fit_width(ctx, &self.drives, column));
                let shown = ui::file_list::ALL_SHOWN;
                self.drive_columns = match ui::file_list::visible_width(ctx, &shown) {
                    Some(room) => ui::file_list::fit_all(&content, &shown, &drives_view::ROLES, room),
                    None => content,
                };
                self.drive_fit.auto();
            }
            // So has the Recycle Bin: four columns, the list's first four for the rule.
            Command::FitColumn(column) if self.on_recycle_bin() => {
                self.recycle_columns[column] = recycle_view::fit_width(ctx, &self.recycled, column);
                self.recycle_fit.by_hand();
            }
            Command::FitAllColumns if self.on_recycle_bin() => {
                let content = std::array::from_fn(|column| match column < recycle_view::COLUMNS.len() {
                    true => recycle_view::fit_width(ctx, &self.recycled, column),
                    false => self.recycle_columns[column],
                });
                let shown = recycle_view::SHOWN;
                self.recycle_columns = match ui::file_list::visible_width(ctx, &shown) {
                    Some(room) => ui::file_list::fit_all(&content, &shown, &recycle_view::ROLES, room),
                    None => content,
                };
                self.recycle_fit.auto();
            }
            Command::FitColumn(column) => {
                self.columns[column] = ui::file_list::fit_width(ctx, &self.tab.view, column);
            }
            Command::FitAllColumns => {
                let mut content = self.columns;
                for (column, width) in content.iter_mut().enumerate() {
                    *width = ui::file_list::fit_width(ctx, &self.tab.view, column);
                }
                // Inside the list's width when it is known: no sideways scroll.
                self.columns = match ui::file_list::visible_width(ctx, &self.shown_columns) {
                    Some(room) => ui::file_list::fit_all(&content, &self.shown_columns, &ui::file_list::LIST_ROLES, room),
                    None => content,
                };
            }
            Command::RestoreColumns => {
                self.columns = ui::file_list::DEFAULT_WIDTHS;
                self.shown_columns = ui::file_list::ALL_SHOWN;
                self.column_order = ui::file_list::DEFAULT_ORDER;
            }
            // Name is always shown.
            Command::ShiftColumnLeft(column) => {
                self.column_order = ui::file_list::shift(&self.column_order, &self.shown_columns, column, true);
            }
            Command::ShiftColumnRight(column) => {
                self.column_order = ui::file_list::shift(&self.column_order, &self.shown_columns, column, false);
            }
            Command::ToggleColumn(column) => self.shown_columns[column] = column == 0 || !self.shown_columns[column],
            Command::OpenPlace(path) => self.navigate(ctx, path, None),
            Command::OpenPlaceInNewTab(path) => {
                self.new_tab(ctx);
                self.navigate(ctx, path, None);
            }
            Command::CopyPlacePath(path) => {
                ctx.copy_text(paths_text(&[path.as_path()]));
                self.notice = Some("Path copied".into());
            }
            Command::ToggleBookmarkOf(path) => self.toggle_bookmark(path),
        }
    }

    /// Opens a file with its default app (Enter in the list or the path box).
    fn open_file(&mut self, path: &Path) {
        if let Err(err) = fs::ops::open_with_default_app(path) {
            self.tab.view.error = Some(err);
        }
    }

    fn go_parent(&mut self, ctx: &egui::Context) {
        let current = self.tab.view.current.clone();
        if let Some(parent) = current.parent().filter(|_| !self.on_marker()) {
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

    /// Lists the current folder again after an operation changed it; `select`
    /// is the entry to land on, one of the folder's or inside an unfolded one.
    fn reload(&mut self, ctx: &egui::Context, select: Option<PathBuf>) {
        self.navigate(ctx, self.tab.view.current.clone(), None);
        self.tab.select_path_after_load = select;
    }

    fn ask_trash(&mut self) {
        if let Some((paths, select, subject)) = self.removable_selection() {
            self.dialog = Some(Dialog::Trash { paths, select, subject });
        }
    }

    /// Shift+Delete: the same question, for deleting without the trash.
    fn ask_delete_forever(&mut self) {
        if let Some((paths, select, subject)) = self.removable_selection() {
            self.dialog = Some(Dialog::Delete { paths, select, subject });
        }
    }

    /// The selection to remove, the row to land on after it and what the
    /// question shows about it, unless it is empty or something in it is
    /// already in the trash.
    fn removable_selection(&mut self) -> Option<(Vec<PathBuf>, Option<PathBuf>, ui::confirm::Subject)> {
        let selection = self.tab.view.selection();
        let paths: Vec<PathBuf> = selection.iter().map(|e| e.path.clone()).collect();
        let subject = ui::confirm::Subject::of(&selection);
        if paths.is_empty() {
            return None;
        }
        // Trashing it again would lose the record that lets it be restored.
        if let Some(trash) = self.trash.as_deref()
            && paths.iter().any(|path| places::in_trash(path, trash))
        {
            self.tab.view.error = Some("This is already in the trash".into());
            return None;
        }
        Some((paths, self.tab.view.path_after_removal(), subject))
    }

    /// Trashes or deletes `paths` in order, stopping at the first failure; the
    /// list then lands on the entry that failed.
    fn remove_all(
        &mut self,
        ctx: &egui::Context,
        paths: Vec<PathBuf>,
        select: Option<PathBuf>,
        remove: fn(&Path) -> Result<(), String>,
    ) {
        for path in paths {
            if let Err(err) = remove(&path) {
                self.tab.error_after_load = Some(err);
                self.reload(ctx, Some(path));
                return;
            }
            self.path_popup.mark_stale();
        }
        self.reload(ctx, select);
    }

    fn start_rename(&mut self) {
        let Some(entry) = self.tab.view.single() else { return };
        self.rename = Some(RenameEdit::new(entry.path.clone(), &entry.name, entry.is_dir));
    }

    /// New ▸ Empty File or a template: created here, then renamed in place.
    fn new_file(&mut self, ctx: &egui::Context, template: Option<&Path>) {
        if self.on_marker() {
            return;
        }
        match fs::ops::create_file(&self.tab.view.current, template) {
            Ok(path) => {
                self.path_popup.mark_stale();
                self.tab.rename_after_load = Some(path.clone());
                self.reload(ctx, Some(path));
            }
            Err(err) => self.tab.view.error = Some(err),
        }
    }

    fn new_folder(&mut self, ctx: &egui::Context) {
        if self.on_marker() {
            return;
        }
        match fs::ops::create_folder(&self.tab.view.current) {
            Ok(path) => {
                self.path_popup.mark_stale();
                self.tab.rename_after_load = Some(path.clone());
                self.reload(ctx, Some(path));
            }
            Err(err) => self.tab.view.error = Some(err),
        }
    }

    /// Applies a finished in-place edit. Invalid names keep the field open.
    fn finish_rename(&mut self, ctx: &egui::Context, mut edit: RenameEdit, replace: bool) {
        match fs::ops::rename(&edit.path, &edit.text, replace) {
            Ok(Renamed::To(renamed)) => {
                self.path_popup.mark_stale();
                self.tab.view.error = None;
                self.reload(ctx, Some(renamed));
            }
            Ok(Renamed::Unchanged) => {}
            Ok(Renamed::TargetExists) => self.dialog = Some(Dialog::Replace(edit)),
            Err(err) => {
                // A replace may have trashed the target before failing: reload.
                let keep = self.tab.view.selected_entry().map(|e| e.path.clone());
                self.tab.error_after_load = Some(err);
                edit.focus = true;
                self.rename = Some(edit);
                self.reload(ctx, keep);
            }
        }
    }

    /// Entries cut and waiting to be pasted: the list greys them.
    fn cut_paths(&self) -> &[PathBuf] {
        match &self.clipboard {
            Some(Clip { paths, mode: Mode::Move }) => paths,
            _ => &[],
        }
    }

    fn set_clipboard(&mut self, mode: Mode) {
        let paths: Vec<PathBuf> = self.tab.view.selection().iter().map(|e| e.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        let items = if paths.len() == 1 { "1 item".to_owned() } else { format!("{} items", paths.len()) };
        self.notice = Some(match mode {
            Mode::Copy => format!("{items} copied"),
            Mode::Move => format!("{items} cut"),
        });
        self.clipboard = Some(Clip { paths, mode });
    }

    /// Copies or moves `paths` into `dest` as a paste does (the same
    /// questions, progress and cancel), leaving the clipboard alone.
    fn drop_into(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>, dest: PathBuf, mode: Mode) {
        if places::is_this_pc(&dest) {
            return;
        }
        // Moved in by hand it would have no record to be restored from.
        if places::is_recycle_bin(&dest) || places::in_recycle_bin(&dest) || self.trash.as_deref().is_some_and(|trash| places::in_trash(&dest, trash)) {
            self.tab.view.error = Some("Nothing can be dropped into the trash".into());
            return;
        }
        if self.refuse_when_busy() {
            return;
        }
        self.start_batch(ctx, paths, dest, mode);
    }

    /// Runs a drop a target took: as a paste into the folder it landed on,
    /// with the action the keys (held, or agreed with the app the files come
    /// from) and the disks settle, refused where any drop is.
    fn run_drop(&mut self, ctx: &egui::Context, drop: ui::dnd::Dropped) {
        if let Some(mode) = drop.drag.mode_for(&drop.target, drop.keys) {
            self.drop_into(ctx, drop.drag.paths, drop.target, mode);
        }
    }

    fn paste(&mut self, ctx: &egui::Context) {
        self.paste_into(ctx, self.tab.view.current.clone());
    }

    /// Pastes the clipboard into `dest`: the folder on screen, or a folder's
    /// own Paste in the Windows menu.
    fn paste_into(&mut self, ctx: &egui::Context, dest: PathBuf) {
        if places::is_marker(&dest) || self.refuse_when_busy() {
            return;
        }
        let Some(Clip { paths, mode }) = self.clipboard.clone() else {
            self.notice = Some("Nothing to paste: Ctrl+C or Ctrl+X first".into());
            return;
        };
        self.start_batch(ctx, paths, dest, mode);
    }

    /// Clears the notice and the error once on screen for `NOTICE_TIME`; returns
    /// how long until then, or `None` when there is none left.
    fn expire_notice(&mut self, now: Instant) -> Option<Duration> {
        let notice = expire(&mut self.notice, &mut self.notice_shown, now);
        let error = expire(&mut self.tab.view.error, &mut self.error_shown, now);
        notice.into_iter().chain(error).min()
    }

    /// A copy or move is already running: says so in the error line.
    fn refuse_when_busy(&mut self) -> bool {
        let busy = self.job.is_some() || self.batch.is_some();
        if busy {
            self.tab.view.error = Some("Another operation is still running".into());
        }
        busy
    }

    /// Copies or moves `paths` into `dest`, one entry at a time.
    fn start_batch(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>, dest: PathBuf, mode: Mode) {
        self.notice = None;
        self.tab.view.error = None;
        let total = paths.len();
        self.batch = Some(Batch {
            mode,
            dest,
            sources: paths.clone(),
            parked: None,
            queue: paths.into(),
            total,
            landed: None,
            in_place: Vec::new(),
        });
        self.next_paste(ctx);
    }

    /// Starts the batch's next entry, asks about a taken name, or ends the
    /// batch when no entry is left.
    fn next_paste(&mut self, ctx: &egui::Context) {
        loop {
            let Some(batch) = self.batch.as_mut() else { return };
            let Some(source) = batch.queue.pop_front() else {
                self.end_paste(ctx);
                return;
            };
            let mode = batch.mode;
            match transfer::plan(&source, &batch.dest, mode) {
                Ok(Plan::Ready(target)) => return self.start_job(ctx, source, target, mode, false),
                Ok(Plan::Nothing) => batch.in_place.push(source),
                Ok(Plan::TargetExists(target)) => {
                    batch.parked = Some(Dialog::Paste { source, target, mode });
                    self.raise_parked_question();
                    return;
                }
                Err(err) => {
                    batch.queue.clear();
                    self.tab.error_after_load = Some(err);
                }
            }
        }
    }

    /// Something other than the list reads the keys: a question, the path
    /// box, the menu or a rename.
    fn keys_taken(&self) -> bool {
        self.dialog.is_some() || self.path_popup.is_open() || self.context_menu.is_some() || self.rename.is_some()
    }

    /// Asks the paste's parked question once nothing else has the keyboard.
    fn raise_parked_question(&mut self) {
        if !self.keys_taken()
            && let Some(question) = self.batch.as_mut().and_then(|b| b.parked.take())
        {
            self.dialog = Some(question);
        }
    }

    /// The batch is over (done, cancelled or failed): what was cut and has
    /// moved leaves the clipboard, and the folder is listed again.
    fn end_paste(&mut self, ctx: &egui::Context) {
        let Some(batch) = self.batch.take() else { return };
        // Only while the clipboard is still this cut: a newer Ctrl+C or Ctrl+X stays whole.
        if let Some(clip) = self.clipboard.as_mut()
            && clip.mode == Mode::Move
            && clip.paths == batch.sources
        {
            clip.paths.retain(|path| fs::ops::exists(path));
            if clip.paths.is_empty() {
                self.clipboard = None;
            }
        }
        if batch.in_place.len() == batch.total {
            self.notice = Some(match batch.in_place.as_slice() {
                [one] => format!("“{}” is already in this folder", file_name(one).unwrap_or_default()),
                _ => "These items are already in this folder".into(),
            });
        }
        // A navigation started meanwhile shows fresh contents anyway; reloading
        // here would supersede it. A pending refresh may be replaced.
        if self.tab.lister.is_pending(self.tab.shown_id) && self.tab.pending_nav != NavKind::Refresh {
            if let Some(err) = self.tab.error_after_load.take() {
                self.tab.view.error = Some(err);
            }
            return;
        }
        let select = match batch.landed {
            Some(target) if target.parent() == Some(self.tab.view.current.as_path()) => Some(target),
            _ => self.tab.view.selected_entry().map(|e| e.path.clone()),
        };
        self.reload(ctx, select);
    }

    fn start_job(
        &mut self,
        ctx: &egui::Context,
        source: PathBuf,
        target: PathBuf,
        mode: Mode,
        replace: bool,
    ) {
        let ctx = ctx.clone();
        let job = Job::start(source, target.clone(), mode, replace, move || ctx.request_repaint());
        self.job = Some(Running { job, target });
    }

    fn finish_job(&mut self, ctx: &egui::Context, running: Running, result: Result<Outcome, String>) {
        let mode = running.job.mode;
        let stop = match result {
            Ok(Outcome::Done) => {
                self.path_popup.mark_stale();
                if let Some(batch) = self.batch.as_mut() {
                    batch.landed = Some(running.target);
                }
                false
            }
            Ok(Outcome::Cancelled) => {
                self.notice = Some(match mode {
                    Mode::Copy => "Copy cancelled".into(),
                    Mode::Move => "Move cancelled".into(),
                });
                true
            }
            Err(err) => {
                self.tab.error_after_load = Some(err);
                true
            }
        };
        if stop && let Some(batch) = self.batch.as_mut() {
            batch.queue.clear();
        }
        self.next_paste(ctx);
    }

    fn answer_dialog(&mut self, ctx: &egui::Context, answer: Confirm) {
        let Some(dialog) = self.dialog.take() else { return };
        if answer == Confirm::No {
            match dialog {
                Dialog::Replace(mut edit) => {
                    edit.focus = true;
                    self.rename = Some(edit);
                }
                Dialog::Paste { .. } => self.next_paste(ctx),
                Dialog::Trash { .. } | Dialog::Delete { .. } | Dialog::Purge { .. } => {}
            }
            return;
        }
        match dialog {
            Dialog::Trash { paths, select, .. } => self.remove_all(ctx, paths, select, fs::ops::trash),
            Dialog::Delete { paths, select, .. } => self.remove_all(ctx, paths, select, fs::ops::delete_forever),
            Dialog::Replace(edit) => self.finish_rename(ctx, edit, true),
            Dialog::Paste { source, target, mode } => {
                self.start_job(ctx, source, target, mode, true);
            }
            Dialog::Purge { items, .. } => self.purge_recycled(&items),
        }
    }

    /// Titles of every tab, in bar order.
    fn tab_labels(&self) -> Vec<String> {
        let label = |folder: &PathBuf| places::marker_label(folder).map_or_else(|| ui::tabs::tab_label(folder), str::to_owned);
        self.tab_folders().iter().map(label).collect()
    }

    /// The folder each tab shows (the active tab's is in `self.tab`).
    fn tab_folders(&self) -> Vec<PathBuf> {
        self.tabs.iter().map(|slot| slot.as_ref().unwrap_or(&self.tab).view.current.clone()).collect()
    }

    /// Ctrl+T or +: opens the current folder in a new tab after this one.
    fn new_tab(&mut self, ctx: &egui::Context) {
        let tab = Tab::new(self.tab.view.current.clone(), Some(&self.tab), self.log);
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
        // A flattened tab's folders are watched once its reload walks them.
        self.watcher.watch(&self.tab.view.current, &[]);
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
    /// Ctrl+D (the folder on screen) or the menu (the folder clicked).
    fn toggle_bookmark(&mut self, folder: PathBuf) {
        if places::is_marker(&folder) {
            return;
        }
        let name = file_name(&folder).unwrap_or_else(|| folder.display().to_string());
        self.notice = Some(if self.bookmarks.toggle(&folder) {
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
            ToolbarEvent::ToggleBookmark => self.toggle_bookmark(self.tab.view.current.clone()),
            ToolbarEvent::Go(path) => self.navigate(ctx, path, None),
            ToolbarEvent::EditPath => {
                // Pressed with Enter (the bar had the focus): the box drawn
                // below must not read that Enter too and close at once.
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                self.path_popup.open();
            }
            ToolbarEvent::SaveView(Some(scope)) => {
                let (folder, look) = (self.tab.view.current.clone(), self.shown_look());
                self.folder_views.set(&folder, scope, look);
                self.tab.look_for = Some((folder, look));
            }
            ToolbarEvent::SaveView(None) => self.folder_views.remove(&self.tab.view.current),
            ToolbarEvent::SetDefaultView => self.default_look = self.shown_look(),
        }
    }

    /// The view and order on screen.
    fn shown_look(&self) -> Look {
        Look { view: self.tab.view_size, sort: self.tab.view.sort }
    }

    /// What the lock's menu shows for the folder on screen.
    fn saved_view(&self) -> ui::toolbar::SavedView {
        let current = &self.tab.view.current;
        let rule = self.folder_views.rule_for(current);
        ui::toolbar::SavedView {
            own: rule.filter(|rule| rule.folder == *current).map(|rule| rule.scope),
            covered_by: rule.filter(|rule| rule.folder != *current).map(|rule| rule.folder.clone()),
            is_default: self.shown_look() == self.default_look,
        }
    }

    fn handle_popup(&mut self, ctx: &egui::Context, event: PopupEvent) {
        match event {
            PopupEvent::Go { path, select, new_tab } => {
                if new_tab {
                    self.new_tab(ctx);
                }
                self.navigate(ctx, path, select);
            }
            PopupEvent::Open(path) => self.open_file(&path),
            PopupEvent::Back => self.go_history(ctx, NavKind::Back),
            PopupEvent::Forward => self.go_history(ctx, NavKind::Forward),
        }
    }

    /// Alt+R: counts what the selected folder holds, in the background.
    fn count_selected(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.tab.view.single().filter(|e| e.is_dir) else { return };
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
        if !self.tab.view.lists(&count.path) {
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
                self.tab.view.set_totals(count.path.clone(), totals);
            }
            Ok(None) => {}
            Err(err) => self.tab.view.error = Some(err),
        }
    }

    /// Stops the running copy or move and the entries queued after it.
    fn cancel_job(&mut self) {
        if let Some(running) = &self.job {
            running.job.cancel();
            if let Some(batch) = self.batch.as_mut() {
                batch.queue.clear();
            }
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

    /// A change of view or order since the tab's look was last applied or
    /// kept goes to the rule its folder opens with; without one it lasts
    /// only this visit.
    fn keep_rule(&mut self) {
        let look = self.shown_look();
        let Some((folder, kept)) = &mut self.tab.look_for else { return };
        // No folder to keep a rule for.
        if places::is_marker(folder) {
            return;
        }
        if look != *kept {
            *kept = look;
            self.folder_views.update(folder, look);
        }
    }

    /// Shows a finished listing and settles what depended on it.
    fn apply(&mut self, listing: Listing) {
        self.keep_rule();
        // Another folder opens with its saved view and order, or the default;
        // set before the listing lands so it is sorted with them.
        if listing.result.is_ok() && self.tab.look_for.as_ref().is_none_or(|(folder, _)| *folder != listing.path) {
            let look = self.folder_views.rule_for(&listing.path).map_or(self.default_look, |rule| rule.look);
            self.tab.view_size = look.view;
            self.tab.view.sort = look.sort;
            self.tab.look_for = Some((listing.path.clone(), look));
        }
        self.tab.shown_id = listing.id;
        let select = self.tab.select_after_load.take();
        let select_path = self.tab.select_path_after_load.take();
        let from = self.tab.view.current.clone();
        let to = listing.result.is_ok().then(|| listing.path.clone());
        if self.tab.pending_nav == NavKind::Refresh {
            self.tab.view.apply_refresh(listing);
        } else {
            self.tab.view.apply_listing_at(listing, select.as_deref(), select_path.as_deref());
        }
        self.tab.history.settle(self.tab.pending_nav, &from, to.as_deref());
        if to.is_some() {
            // Opening the Recycle Bin again, even from itself, reads the bins again.
            self.recycle_stale |= self.on_recycle_bin();
            if self.tab.view.is_flat() {
                // A reload: its folders stay watched until the walk tells them anew.
                self.watcher.watch_folder(&self.tab.view.current);
            } else {
                self.watcher.watch(&self.tab.view.current, &[]);
            }
            // A reload of a flattened folder walks it again; another folder is not flattened.
            self.tab.flat_due = self.tab.view.is_flat();
            if !self.tab.flat_due {
                self.tab.flattener = None;
            }
            if self.tab.pending_nav != NavKind::Refresh && !self.on_marker() {
                self.recents.visit(&self.tab.view.current);
                self.visits.visit(&self.tab.view.current, unix_now());
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
            && !self.tab.view.lists(&edit.path)
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

    /// Starts reading the unfolded folders whose read is due, in the background.
    fn read_kids(&mut self, ctx: &egui::Context) {
        for folder in self.tab.view.tree.take_wanted() {
            let ctx = ctx.clone();
            let id = self.tab.readers.request(folder.clone(), move || ctx.request_repaint());
            self.tab.view.tree.started(&folder, id);
        }
    }

    /// Starts the walk the flattened view is waiting for.
    fn walk_flat(&mut self, ctx: &egui::Context) {
        if !std::mem::take(&mut self.tab.flat_due) || !self.tab.view.is_flat() {
            return;
        }
        let ctx = ctx.clone();
        // Replacing an earlier walk drops it, which cancels it.
        self.tab.flattener = Some(fs::flat::Flattener::start(self.tab.view.flat_roots().to_vec(), move || ctx.request_repaint()));
    }

    /// Shows the flatten walk once it is done.
    fn poll_flat(&mut self) {
        if let Some((roots, walked)) = self.tab.flattener.as_ref().and_then(fs::flat::Flattener::poll) {
            self.tab.flattener = None;
            if roots != self.tab.view.flat_roots() {
                return;
            }
            self.tab.view.apply_flat(&roots, walked.found);
            // A change inside a folder listed reloads the view too.
            let current = &self.tab.view.current;
            let folders: Vec<PathBuf> = walked.folders.into_iter().filter(|folder| folder != current).collect();
            self.watcher.watch(current, &folders);
            self.check_refused();
        }
    }

    /// Too many folders to watch, or the system refused one: the view stays
    /// as it is until F5, so a reload does not walk them all again and again.
    fn check_refused(&mut self) {
        if self.watcher.take_refused() {
            self.notice = Some("Too many folders to watch: press F5 to refresh".into());
            self.refresh = RefreshTimer::default();
            self.watcher.take_changed();
        }
    }

    /// Ctrl+E: lists what the subfolders hold too, or the folder alone again.
    /// With folders picked, it lists or takes out what those hold alone.
    fn toggle_flat(&mut self) {
        if self.on_marker() {
            return;
        }
        let view = &self.tab.view;
        let next = next_flat(view.flat_roots(), &view.current, &view.picked_folders());
        if next.is_empty() {
            self.tab.view.clear_flat();
            self.tab.flattener = None;
            self.tab.flat_due = false;
        } else {
            self.tab.view.set_flat(next);
            self.tab.flat_due = true;
        }
        // The folder alone, until the walk tells the folders it lists.
        self.watcher.watch(&self.tab.view.current, &[]);
    }

    /// Shows the unfolded folders the readers finished.
    fn poll_kids(&mut self) {
        for listing in self.tab.readers.poll() {
            self.tab.view.apply_kids(listing);
        }
    }

    /// Lists the open folder again, keeping the selection and the sort order
    /// (a flattened view walks again once it lands). Never supersedes a
    /// navigation, nor restarts a flatten walk (a busy tree would never
    /// finish): returns whether it listed.
    fn reload_if_idle(&mut self, ctx: &egui::Context) -> bool {
        if self.tab.lister.is_pending(self.tab.shown_id) || self.tab.flattener.is_some() {
            return false;
        }
        self.navigate_as(ctx, NavKind::Refresh, self.tab.view.current.clone(), None);
        true
    }

    /// Reloads the open folder a moment after it changed on disk.
    fn refresh_if_changed(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.check_refused();
        if self.watcher.take_changed() {
            self.refresh.note_change(now);
        }
        if self.refresh.take_due(now) && !self.reload_if_idle(ctx) {
            // Try again once the listing or the walk has landed.
            self.refresh.note_change(now);
        }
        if let Some(wait) = self.refresh.wait(now) {
            ctx.request_repaint_after(wait);
        }
    }

    /// Where an arrow key lands in a view laid out in lines: in Icons, Up and
    /// Down cross the rows and Left and Right walk the cards; in List, Up and
    /// Down walk the column and Left and Right cross the columns. `None` in Details.
    fn grid_target(&self, action: Action) -> Option<usize> {
        let shape = self.grid?;
        let last = self.tab.view.rows.len().checked_sub(1)?;
        let cur = self.tab.view.selected.unwrap_or(0).min(last);
        let rows_are_lines = self.tab.view_size.layout() == ui::view_size::Layout::Icons;
        let (down, side) = match action {
            Action::Extend(isize::MIN) => return Some(0),
            Action::Extend(isize::MAX) => return Some(last),
            Action::Move(n) | Action::Extend(n) => (n, 0),
            Action::Side(n) | Action::ExtendSide(n) => (0, n),
            _ => return None,
        };
        let (across, along) = if rows_are_lines { (down, side) } else { (side, down) };
        Some(if across != 0 { shape.across(cur, across) } else { shape.along(cur, along) })
    }

    /// How opaque the list is at time `now`: a view just picked (Details,
    /// Tree, List or Icons) fades in over `VIEW_FADE`, like File Pilot's quick
    /// switch; icons growing inside the grid are the same view and never fade.
    fn list_opacity(&mut self, now: f64) -> f32 {
        let layout = self.tab.view_size.layout();
        if layout != self.shown_layout.0 {
            self.shown_layout = (layout, now);
        }
        let done = ((now - self.shown_layout.1) / VIEW_FADE).clamp(0.0, 1.0) as f32;
        0.35 + 0.65 * done
    }

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            // The This PC view has drives, not entries: the keys move over those.
            Action::Move(delta) if self.on_this_pc() => self.move_drive(|at, last| match at {
                Some(at) => at.saturating_add_signed(delta).min(last),
                None if delta < 0 => last,
                None => 0,
            }),
            Action::First if self.on_this_pc() => self.move_drive(|_, _| 0),
            Action::Last if self.on_this_pc() => self.move_drive(|_, last| last),
            Action::Activate if self.on_this_pc() => {
                if let Some(path) = self.picked_drive() {
                    self.navigate(ctx, path, None);
                }
            }
            Action::Menu if self.on_this_pc() => {
                if let Some(path) = self.picked_drive() {
                    self.open_place_menu(path, ctx.content_rect().center() - egui::vec2(170.0, 120.0));
                }
            }
            // The Recycle Bin view has entries of its own: the keys move over those.
            Action::Move(delta) if self.on_recycle_bin() => self.move_recycle(|picks, rows| picks.step(rows, delta, false)),
            Action::Extend(delta) if self.on_recycle_bin() => self.move_recycle(|picks, rows| picks.step(rows, delta, true)),
            Action::First if self.on_recycle_bin() => self.move_recycle(|picks, rows| picks.edge(rows, false)),
            Action::Last if self.on_recycle_bin() => self.move_recycle(|picks, rows| picks.edge(rows, true)),
            Action::SelectAll if self.on_recycle_bin() => self.tab.recycle.select_all(&self.recycled),
            // Delete there is the only delete: the entries are in the trash already.
            Action::Trash | Action::DeleteForever if self.on_recycle_bin() => self.ask_purge(),
            Action::Menu if self.on_recycle_bin() => self.open_recycle_menu(ctx.content_rect().center() - egui::vec2(170.0, 120.0)),
            Action::Cancel if self.on_recycle_bin() && self.tab.recycle.count() > 0 => self.tab.recycle.clear(),
            Action::FocusPathBar => self.path_popup.open(),
            Action::ToggleHidden => self.tab.view.toggle_hidden(),
            Action::ToggleFlat => self.toggle_flat(),
            // The drives or the bins are read again there, as the menu's Refresh does.
            Action::Reload if self.on_marker() => self.run_command(ctx, Command::Refresh),
            Action::Reload => {
                self.reload_if_idle(ctx);
            }
            Action::ZoomIn => self.tab.view_size = self.tab.view_size.bigger(),
            Action::ZoomOut => self.tab.view_size = self.tab.view_size.smaller(),
            Action::ZoomReset => self.tab.view_size = ui::view_size::ViewSize::default(),
            Action::FitColumns => self.run_command(ctx, Command::FitAllColumns),
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
            Action::Side(delta) if self.tab.view_size.layout() == ui::view_size::Layout::Tree => {
                self.tab.view.tree_side(delta);
                self.read_kids(ctx);
            }
            Action::Move(_) | Action::Extend(_) | Action::Side(_) | Action::ExtendSide(_)
                if let Some(target) = self.grid_target(action) =>
            {
                if matches!(action, Action::Extend(_) | Action::ExtendSide(_)) {
                    self.tab.view.extend_to(target);
                    self.tab.view.scroll = Some(Scroll::Smooth);
                } else {
                    self.tab.view.move_to(target);
                }
            }
            Action::Move(_) | Action::First | Action::Last => self.tab.view.move_selection(action),
            Action::Extend(rows) => self.tab.view.extend_by(rows),
            Action::Side(_) | Action::ExtendSide(_) => {}
            Action::SelectAll => self.tab.view.select_all(),
            Action::Menu => {
                // Under the cursor row, or mid-window when it is scrolled out of view.
                let at = ui::file_list::cursor_row_rect(ctx)
                    .map_or_else(|| ctx.content_rect().center() - egui::vec2(170.0, 120.0), |r| r.left_bottom() + egui::vec2(24.0, 0.0));
                match self.tab.view.selected {
                    Some(row) if self.tab.view.picked_count() == 0 || self.tab.view.is_picked(row) => self.open_entry_menu(row, at),
                    Some(_) => {
                        let row = (0..self.tab.view.rows.len()).find(|&r| self.tab.view.is_picked(r)).unwrap_or(0);
                        self.open_entry_menu(row, at);
                    }
                    None => self.open_background_menu(at),
                }
            }
            // A leading Space is the preview key; after other text it is just a space.
            Action::TypeChar(' ') if self.tab.view.filter().is_empty() => {
                self.preview_open = !self.preview_open;
            }
            Action::TypeChar(c) => self.tab.view.push_filter(c),
            Action::Up => self.go_parent(ctx),
            Action::Back => self.go_history(ctx, NavKind::Back),
            Action::Forward => self.go_history(ctx, NavKind::Forward),
            Action::Trash => self.ask_trash(),
            Action::DeleteForever => self.ask_delete_forever(),
            Action::Rename => self.start_rename(),
            Action::NewFolder => self.new_folder(ctx),
            Action::Copy => self.set_clipboard(Mode::Copy),
            Action::Cut => self.set_clipboard(Mode::Move),
            Action::CopyPath => self.copy_paths(ctx),
            Action::Paste => self.paste(ctx),
            Action::Count => self.count_selected(ctx),
            Action::ToggleBookmark => self.toggle_bookmark(self.tab.view.current.clone()),
            Action::NewTab => self.new_tab(ctx),
            Action::CloseTab => self.close_tab(ctx, self.active),
            Action::NextTab => self.select_tab(ctx, (self.active + 1) % self.tabs.len()),
            Action::PrevTab => {
                self.select_tab(ctx, (self.active + self.tabs.len() - 1) % self.tabs.len());
            }
            // Clearing a filter is the cheaper surprise than cancelling a copy.
            // A count is cheaper to lose than a copy, too.
            Action::Cancel => {
                // Picks the user cannot see are dropped too, but do not use up the press.
                let seen = self.tab.view.picked_count() > 0;
                self.hold_absorbed = self.tab.view.clear_filter() || (self.tab.view.drop_picks() && seen);
                if !self.hold_absorbed && self.cancel_count() {
                    self.notice = Some("Calculation cancelled".into());
                    self.hold_absorbed = true;
                }
                if !self.hold_absorbed {
                    if self.job.is_some() {
                        self.cancel_job();
                    } else if self.clipboard.as_ref().is_some_and(|clip| clip.mode == Mode::Move) {
                        // Last: a cut waiting to be pasted is forgotten, as in other explorers.
                        self.clipboard = None;
                        self.notice = Some("Cut cancelled".into());
                    } else {
                        // With nothing else to undo, the cursor keeps only its outline.
                        self.tab.view.unselect();
                    }
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = frame.window_handle()
                && let RawWindowHandle::Win32(win32) = handle.as_raw()
            {
                self.hwnd = Some(win32.hwnd.get());
            }
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = frame;
        let ctx = ui.ctx().clone();
        // Files dragged over the window from another app, if any.
        #[cfg(target_os = "linux")]
        ui::dnd::set_outside(&ctx, self.wayland_dnd.poll(frame, &ctx));
        // Before any drop target is drawn: none in the trash takes a drop.
        ui::dnd::set_trash(&ctx, self.trash.as_deref());

        if let Some(listing) = self.tab.lister.poll() {
            self.apply(listing);
        }
        self.poll_kids();
        self.read_kids(&ctx);
        self.poll_flat();
        self.walk_flat(&ctx);

        self.refresh_if_changed(&ctx);
        self.poll_recycle(&ctx);
        self.poll_count();
        #[cfg(windows)]
        self.read_pending_menu(&ctx);

        if let Some(left) = self.expire_notice(Instant::now()) {
            ctx.request_repaint_after(left);
        }
        ui::skip_panel_borders(&ctx);
        if self.dialog.is_none() && !self.path_popup.is_open() && self.context_menu.is_none() {
            ui::shift_tab_from_list(&ctx);
        }

        // An open dialog takes the keyboard; the list must not react underneath.
        if self.dialog.is_some() {
            if let Some(answer) = input::read_confirm(&ctx) {
                self.answer_dialog(&ctx, answer);
            }
        } else if self.path_popup.is_open() || self.context_menu.is_some() {
            // The path box and the menu read their own keys when drawn.
        } else if ui::toolbar::close_lock_menu_on_escape(&ctx) {
            // That Escape closed the lock's menu and did nothing else.
        } else {
            for action in input::read_actions(&ctx) {
                self.handle_action(&ctx, action);
            }
        }

        // After the keys: a question this raises is drawn before any key can answer it.
        if let Some(result) = self.job.as_mut().and_then(|r| r.job.poll())
            && let Some(running) = self.job.take()
        {
            self.finish_job(&ctx, running, result);
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
            ui::tabs::drop_targets(ui, &self.tab_folders());
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
                saved_view: self.saved_view(),
            };
            if let Some(event) = ui::toolbar::show(ui, &nav) {
                self.handle_toolbar(&ctx, event);
            }
            ui.add_space(2.0);
        });

        if self.sidebar_open {
            self.refresh_disks(&ctx);
            ctx.request_repaint_after(DISKS_WATCH);
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
                        icons: &self.file_icons,
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
                            self.tab.view.totals.get(&entry.path),
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

        let list = egui::CentralPanel::default_margins().show(ui, |ui| {
            // Room at the bottom for the floating islands, so they never hide a row.
            let room = ui::status_bar::island_room(&ctx);
            ui::status_bar::above_islands(ui, room, |ui| {
            if self.on_this_pc() {
                // The drives, read again as they change, instead of a folder's list.
                self.refresh_disks(&ctx);
                ctx.request_repaint_after(DISKS_WATCH);
                self.fit_this_pc(&ctx, ui.available_rect_before_wrap().width());
                if let Some(event) = ui::drives_view::show(ui, &self.drives, self.tab.drive.as_deref(), self.drive_sort, &self.drive_columns) {
                    self.handle_drives(&ctx, event);
                }
                return;
            }
            if self.on_recycle_bin() {
                // The bins' entries, read when shown, instead of a folder's list.
                self.fit_recycle_bin(&ctx, ui.available_rect_before_wrap().width());
                let look = recycle_view::Look {
                    picks: &self.tab.recycle,
                    sort: self.recycle_sort,
                    widths: &self.recycle_columns,
                    loading: self.recycle_stale || self.recycle_reader.is_running(),
                    scroll_to: self.recycle_scroll.take(),
                };
                if let Some(event) = recycle_view::show(ui, &self.recycled, &look) {
                    self.handle_recycle(&ctx, event);
                }
                return;
            }
            let scroll = self.tab.view.scroll.take();
            let cut = self.cut_paths().to_vec();
            let opacity = self.list_opacity(ctx.input(|i| i.time));
            if opacity < 1.0 {
                ui.multiply_opacity(opacity);
                ctx.request_repaint();
            }
            let tree = self.tab.view_size.layout() == ui::view_size::Layout::Tree;
            if !tree {
                // The unfolded folders belong to the Tree view alone.
                self.tab.view.fold_all();
            }
            let sizes = ui::file_list::Sizes { widths: self.columns, shown: self.shown_columns, order: self.column_order, tree };
            let lined = match (self.tab.view_size.layout(), self.tab.view_size.growth()) {
                (_, Some(growth)) => Some(ui::icon_grid::show(ui, &self.tab.view, &cut, growth, scroll, self.rename.as_mut(), ui::icon_grid::Art { file_icons: &self.file_icons, thumbs: &self.thumbnails })),
                (ui::view_size::Layout::List, _) => Some(ui::list_view::show(ui, &self.tab.view, &cut, scroll, self.rename.as_mut(), &self.file_icons)),
                _ => None,
            };
            self.grid = lined.as_ref().map(|out| out.shape);
            let event = match lined {
                Some(out) => out.event,
                None => ui::file_list::show(ui, &self.tab.view, &cut, sizes, scroll, self.rename.as_mut(), &self.file_icons),
            };
            // After drawing: the icons the list just asked for go out this frame.
            self.file_icons.poll(&ctx);
            self.thumbnails.poll(&ctx);
            self.handle_list(&ctx, event);
            });
        });

        // Floating over the list's bottom, in the room left for them there.
        let picked = ui::status_bar::show(
            &ctx,
            list.response.rect,
            &Status {
                filter: self.tab.view.filter(),
                caret: !self.path_popup.is_open() && self.rename.is_none(),
                item_count: if self.on_this_pc() {
                    self.drives.len()
                } else if self.on_recycle_bin() {
                    self.recycled.len()
                } else {
                    self.tab.view.rows.len()
                },
                hidden_count: self.tab.view.hidden_count(),
                show_hidden: self.tab.view.show_hidden,
                loading: self.tab.lister.is_pending(self.tab.shown_id),
                last_elapsed: self.tab.view.last_elapsed,
                error: self.tab.view.error.as_deref(),
                job: self
                    .job
                    .as_ref()
                    .map(|r| {
                        let item = self.batch.as_ref().and_then(Batch::item);
                        ui::status_bar::progress_text(r.job.mode, &r.job.progress, item)
                    }),
                notice: self.notice.as_deref(),
                folder_count: self.tab.view.folder_count(),
                selected: self.tab.view.selected,
                picked: if self.on_recycle_bin() { self.tab.recycle.count() } else { self.tab.view.picked_count() },
                picked_size: if self.on_recycle_bin() { self.picked_recycled_size() } else { self.tab.view.picked_size() },
                counting: self.counting.as_ref().map(|c| {
                    let name = file_name(&c.path).unwrap_or_default();
                    ui::status_bar::counting_text(&name, &c.progress)
                }),
                view: self.tab.view_size,
            },
        );
        if let Some(view) = picked {
            self.tab.view_size = view;
        }

        #[cfg(windows)]
        self.poll_shell_menu();
        if let Some(menu) = self.context_menu.as_mut()
            && let Some(event) = menu.show(&ctx)
        {
            self.context_menu = None;
            if let MenuEvent::Run(command) = event {
                self.run_command(&ctx, command);
            }
            // Handlers expect their menu alive until an item runs or it closes.
            #[cfg(windows)]
            {
                self.shell_menu = None;
                self.menu_basis = None;
            }
        }

        let home = fs::home_dir();
        let shown = self.tab.view.show_hidden;
        let current = self.tab.view.current.clone();
        if let Some(event) = self.path_popup.show(&ctx, list.response.rect, &current, home.as_deref(), shown, || self.visits.scores(unix_now())) {
            self.handle_popup(&ctx, event);
        }

        // A click on another row or a scroll can take focus without ending the
        // edit above (one event per frame); an unfocused field is a cancel.
        if self.rename.as_ref().is_some_and(|e| !e.focus)
            && !ctx.memory(|m| m.has_focus(ui::file_list::rename_id()))
        {
            self.rename = None;
        }

        self.raise_parked_question();
        if let Some(dialog) = &self.dialog
            && let Some(answer) = ui::confirm::show(
                &ctx,
                &ui::confirm::Prompt {
                    title: dialog.title(),
                    message: &dialog.message(),
                    subject: dialog.subject(),
                    hint: dialog.hint(),
                    yes: dialog.yes_label(),
                    no: dialog.no_label(),
                    danger: dialog.danger(),
                },
            )
        {
            self.answer_dialog(&ctx, answer);
        }

        // A drop a target took: run as a paste into the folder it landed on.
        let dropped = ui::dnd::take_drop(&ctx);
        let took = dropped.is_some();
        if let Some(drop) = dropped {
            self.run_drop(&ctx, drop);
        }
        // A drag whose pointer went past the window's edge goes to the
        // compositor, to be dropped on another app.
        #[cfg(target_os = "linux")]
        self.wayland_dnd.carry_out(&ctx);
        // On Windows, to the system (OLE), to be dropped on another app.
        #[cfg(windows)]
        crate::windows_drag::carry_out(&ctx, self.hwnd);
        // After every target: the label by the pointer while entries are dragged,
        // and what a drag from another app is told: take it, or refuse.
        let preferred = ui::dnd::finish(&ctx);
        let _ = (preferred, took);
        #[cfg(target_os = "linux")]
        self.wayland_dnd.answer(preferred, took);

        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.window = Some([rect.width(), rect.height()]);
        }
        self.save_settings(false);
    }

    /// The release of a drag handed to the compositor, which egui never sees.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        ui::dnd::end_hand_off(ctx, raw_input);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(target_os = "linux")]
        self.wayland_dnd.stop();
        self.save_settings(true);
    }
}

/// Starts the background crawl of every disk behind the path box's quick jump.
/// The disks and shares are read on the crawl's thread: a hung mount must
/// not freeze the window.
fn start_index(ctx: &egui::Context) -> Crawler {
    let scope = || {
        let disks = places::disks().into_iter().map(|mount| mount.path).collect();
        fs::index::scope(cfg!(windows), fs::home_dir().as_deref(), disks, places::network_mounts())
    };
    let ctx = ctx.clone();
    Crawler::start(scope, move || ctx.request_repaint())
}

/// What Lumen opens at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    Folder(PathBuf),
    /// The tabs of the last window closed.
    Restore,
}

/// From the command line (without the program name): a folder argument
/// opens it; otherwise `--restore` (the desktop entry), Windows, or Linux
/// without a terminal (a double click in a file manager) restores the last
/// session, and Linux in a terminal opens the working directory.
pub fn start(args: &[String]) -> Start {
    use std::io::IsTerminal;
    let terminal = std::io::stdin().is_terminal();
    start_from(args, std::env::current_dir().ok(), fs::home_dir(), cfg!(windows), terminal)
}

fn start_from(args: &[String], cwd: Option<PathBuf>, home: Option<PathBuf>, windows: bool, terminal: bool) -> Start {
    let folder = args.iter().find(|a| *a != RESTORE).cloned();
    if folder.is_none() && (windows || !terminal || args.iter().any(|a| a == RESTORE)) {
        return Start::Restore;
    }
    // Windows hands a GUI app whatever working directory its launcher had
    // (often C:\Windows\System32), not one the user chose.
    Start::Folder(start_dir_from(folder, cwd, home, !windows))
}

/// The flag the desktop entry passes.
const RESTORE: &str = "--restore";

/// The saved tabs as they can open now: a folder that is gone gives way to
/// its nearest parent still there, else `home`; no tabs at all is `home`.
pub fn restore_tabs(tabs: &[PathBuf], active: usize, home: &Path) -> (Vec<PathBuf>, usize) {
    if tabs.is_empty() {
        return (vec![home.to_path_buf()], 0);
    }
    let usable = |path: &PathBuf| {
        path.ancestors()
            .find(|p| p.is_absolute() && p.is_dir())
            .map_or_else(|| home.to_path_buf(), Path::to_path_buf)
    };
    (tabs.iter().map(usable).collect(), active.min(tabs.len() - 1))
}

fn start_dir_from(arg: Option<String>, cwd: Option<PathBuf>, home: Option<PathBuf>, use_cwd: bool) -> PathBuf {
    let root = || PathBuf::from("/");
    match arg {
        Some(arg) => {
            let base = cwd.or_else(|| home.clone()).unwrap_or_else(root);
            fs::resolve_input_path(&arg, &base, home.as_deref())
        }
        None if use_cwd => cwd.or(home).unwrap_or_else(root),
        None => home.or(cwd).unwrap_or_else(root),
    }
}


/// Seconds since the Unix epoch; 0 if the clock is before it.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::lister::ListError;
    use crate::fs::preview::Content;
    use crate::ui::drives_view::DriveColumn;
    use std::path::Path;
    use std::time::Instant;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_session_is_restored_on_windows_and_with_restore_and_never_over_a_folder() {
        let cwd = Some(PathBuf::from("/srv/work"));
        let home = Some(PathBuf::from("/home/u"));
        let start = |list: &[&str], windows| start_from(&args(list), cwd.clone(), home.clone(), windows, true);
        assert_eq!(start(&[], true), Start::Restore, "Windows without a folder");
        assert_eq!(start(&["--restore"], false), Start::Restore, "the desktop entry on Linux");
        assert_eq!(start(&[], false), Start::Folder("/srv/work".into()), "a terminal on Linux");
        assert_eq!(start(&["--restore", "docs"], false), Start::Folder("/srv/work/docs".into()), "a folder opened with Lumen");
        assert_eq!(start(&["docs"], true), Start::Folder("/srv/work/docs".into()));
    }

    #[test]
    fn without_a_terminal_linux_restores_the_session_unless_given_a_folder() {
        let cwd = Some(PathBuf::from("/srv/work"));
        let home = Some(PathBuf::from("/home/u"));
        let start = |list: &[&str]| start_from(&args(list), cwd.clone(), home.clone(), false, false);
        assert_eq!(start(&[]), Start::Restore, "a double click in a file manager");
        assert_eq!(start(&["--restore"]), Start::Restore);
        assert_eq!(start(&["docs"]), Start::Folder("/srv/work/docs".into()), "a folder argument still wins");
    }

    #[test]
    fn restored_tabs_fall_back_to_the_nearest_folder_left_then_home() {
        let dir = crate::fs::ops::tests::fresh_fixture("app-restore-tabs");
        std::fs::create_dir_all(dir.join("kept/inner")).unwrap();
        let home = dir.join("kept");
        let tabs = vec![dir.join("kept/inner"), dir.join("kept/gone/deeper"), PathBuf::from("relative/nowhere")];
        assert_eq!(restore_tabs(&tabs, 1, &home), (vec![dir.join("kept/inner"), dir.join("kept"), home.clone()], 1));
        assert_eq!(restore_tabs(&tabs, 9, &home).1, 2, "an active tab past the end is the last");
        assert_eq!(restore_tabs(&[], 0, &home), (vec![home.clone()], 0), "no session: home");
    }

    #[test]
    fn a_restored_session_opens_its_tabs_and_lists_the_active_one() {
        let dir = crate::fs::ops::tests::fresh_fixture("app-restore-start");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let ctx = egui::Context::default();
        let config = Config { tabs: vec![dir.join("a"), dir.join("b")], active_tab: 1, ..Config::default() };
        let settings = Settings { config, path: None, error: None };
        let app = LumenApp::start(&ctx, Start::Restore, &dir, false, settings);
        assert_eq!(app.tab_labels(), ["a", "b"]);
        assert_eq!(app.active, 1);
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("b")));
        assert_eq!(app.session(), (vec![dir.join("a"), dir.join("b")], 1));
    }

    #[test]
    fn a_folder_start_ignores_the_saved_session() {
        let dir = crate::fs::ops::tests::fresh_fixture("app-restore-folder");
        let ctx = egui::Context::default();
        let config = Config { tabs: vec![dir.join("x"), dir.join("y")], active_tab: 1, ..Config::default() };
        let app = LumenApp::start(&ctx, Start::Folder(dir.clone()), &dir, false, Settings { config, path: None, error: None });
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tab.lister.last_requested(), Some(dir));
    }

    #[test]
    fn closing_saves_this_windows_tabs_over_any_other_windows() {
        let (ctx, mut app, dir) = app_in("app-session-save");
        let path = dir.join("config");
        app.config_path = Some(path.clone());
        app.saved = app.settings(true);
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.save_settings(false);
        assert!(!std::fs::read_to_string(&path).unwrap_or_default().contains("tab ="), "not while running");
        // Another window closed meanwhile and saved its own session.
        std::fs::write(&path, "tab = /elsewhere\nactive_tab = 0\n").unwrap();
        app.save_settings(true);
        let saved = Config::load(&path).unwrap();
        assert_eq!((saved.tabs, saved.active_tab), (vec![dir.clone(), dir.join("sub")], 1), "the last window closed wins");
        // Even with nothing changed since the start.
        std::fs::write(&path, "tab = /elsewhere\n").unwrap();
        app.save_settings(true);
        assert_eq!(Config::load(&path).unwrap().tabs, [dir.clone(), dir.join("sub")]);
    }

    #[test]
    fn without_an_argument_windows_starts_in_home_and_linux_where_it_was_launched() {
        let cwd = Some(PathBuf::from("/srv/work"));
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(start_dir_from(None, cwd.clone(), home.clone(), true), PathBuf::from("/srv/work"));
        assert_eq!(start_dir_from(None, cwd.clone(), home.clone(), false), PathBuf::from("/home/u"));
        // Each falls back on the other, then on the root.
        assert_eq!(start_dir_from(None, None, home.clone(), true), PathBuf::from("/home/u"));
        assert_eq!(start_dir_from(None, cwd.clone(), None, false), PathBuf::from("/srv/work"));
        assert_eq!(start_dir_from(None, None, None, false), PathBuf::from("/"));
    }

    #[test]
    fn a_folder_argument_is_resolved_against_the_working_directory() {
        let cwd = Some(PathBuf::from("/srv/work"));
        let home = Some(PathBuf::from("/home/u"));
        for use_cwd in [true, false] {
            let got = start_dir_from(Some("docs".into()), cwd.clone(), home.clone(), use_cwd);
            assert_eq!(got, Path::new("/srv/work").join("docs"));
        }
    }

    #[test]
    fn cut_and_paste_into_the_same_folder_says_why_nothing_happened() {
        let dir = crate::fs::ops::tests::fresh_fixture("app-paste-same");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        let ctx = egui::Context::default();
        let mut app = LumenApp::new(&ctx, dir.clone(), false);
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Move });
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
        // As if the startup listing had landed: nothing is in flight, and
        // the folder shows the default look.
        app.tab.shown_id = app.tab.lister.latest_id();
        app.tab.look_for = Some((dir.clone(), Look::default()));
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
    fn alt_up_goes_to_the_parent_even_with_filter_text() {
        let (ctx, mut app, dir) = app_in("app-alt-up");
        app.handle_action(&ctx, Action::TypeChar('a'));
        app.handle_action(&ctx, Action::Up);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.tab.select_after_load, Some(name), "went up, selecting the folder it came from");
        assert_eq!(app.tab.view.filter(), "a", "the filter is not edited");
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
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
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
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::CancelHeld);
        assert!(app.job.as_ref().unwrap().job.cancel_requested());
    }

    #[test]
    fn escape_clears_the_filter_first_and_then_cancels_a_copy() {
        let (ctx, mut app, dir) = app_in("app-escape");
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.filter(), "");
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "the copy goes on");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.job.as_ref().unwrap().job.cancel_requested());
    }

    /// Runs the paste in progress until it ends or stops at a question.
    fn wait_paste(ctx: &egui::Context, app: &mut LumenApp) {
        for _ in 0..500 {
            if let Some(result) = app.job.as_mut().and_then(|r| r.job.poll()) {
                let running = app.job.take().unwrap();
                app.finish_job(ctx, running, result);
            }
            if app.dialog.is_some() || (app.job.is_none() && app.batch.is_none()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("paste did not finish");
    }

    /// A click on the first of `names`, then Ctrl+click on the rest.
    fn pick(app: &mut LumenApp, names: &[&str]) {
        let view = &mut app.tab.view;
        let row = |view: &View, name: &str| view.row_of_name(name).expect("listed");
        view.select_row(row(view, names[0]));
        for name in &names[1..] {
            view.toggle_pick(row(view, name));
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn copying_several_entries_pastes_them_one_after_another() {
        let (ctx, mut app, dir) = app_in("app-batch-copy");
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Copy);
        assert_eq!(app.notice.as_deref(), Some("2 items copied"));
        app.handle_action(&ctx, Action::Paste);
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir), ["a (copy).txt", "a.txt", "b (copy).txt", "b.txt", "sub"]);
        assert_eq!(std::fs::read_to_string(dir.join("b (copy).txt")).unwrap(), "B");
        assert!(app.clipboard.is_some(), "a copy can be pasted again");
        assert_eq!(app.tab.view.error, None);
    }

    #[test]
    fn cutting_several_entries_moves_them_all_and_empties_the_clipboard() {
        let (ctx, mut app, dir) = app_in("app-batch-move");
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Cut);
        assert_eq!(app.notice.as_deref(), Some("2 items cut"));
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir), ["sub"]);
        assert_eq!(names_in(&dir.join("sub")), ["a.txt", "b.txt"]);
        assert!(app.clipboard.is_none(), "nothing left to move");
    }

    #[test]
    fn a_taken_name_asks_for_that_item_and_skip_goes_on_with_the_rest() {
        let (ctx, mut app, dir) = app_in("app-batch-skip");
        std::fs::write(dir.join("sub/a.txt"), "OLD").unwrap();
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Cut);
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        wait_paste(&ctx, &mut app);
        let dialog = app.dialog.as_ref().expect("asks about a.txt");
        assert_eq!(dialog.message(), "“a.txt” already exists here. Replace it? The existing one goes to the trash.");
        assert_eq!((dialog.yes_label(), dialog.no_label()), ("Replace", "Skip"));
        app.answer_dialog(&ctx, Confirm::No);
        wait_paste(&ctx, &mut app);
        assert!(app.dialog.is_none());
        assert_eq!(names_in(&dir.join("sub")), ["a.txt", "b.txt"]);
        assert_eq!(std::fs::read_to_string(dir.join("sub/a.txt")).unwrap(), "OLD", "skipped");
        assert_eq!(names_in(&dir), ["a.txt", "sub"]);
        assert_eq!(crate::fs::ops::tests::trashed_names_from(&dir.join("sub")), Vec::<String>::new());
        let Some(Clip { paths, .. }) = &app.clipboard else { panic!("the skipped one stays cut") };
        assert_eq!(paths, &[dir.join("a.txt")]);
    }

    #[test]
    fn a_taken_name_replaced_sends_the_old_one_to_the_trash() {
        let (ctx, mut app, dir) = app_in("app-batch-replace");
        let sub = dir.join("sub");
        std::fs::write(sub.join("a.txt"), "OLD").unwrap();
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Copy);
        app.navigate(&ctx, sub.clone(), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        wait_paste(&ctx, &mut app);
        app.answer_dialog(&ctx, Confirm::Yes);
        wait_paste(&ctx, &mut app);
        assert_eq!(std::fs::read_to_string(sub.join("a.txt")).unwrap(), "A");
        assert_eq!(names_in(&sub), ["a.txt", "b.txt"]);
        assert_eq!(crate::fs::ops::tests::trashed_names_from(&sub), crate::fs::ops::tests::as_trashed(&["a.txt"]));
        crate::fs::ops::tests::purge_trashed_from(&sub);
    }

    #[test]
    fn escape_stops_the_whole_paste_not_just_the_current_item() {
        let (ctx, mut app, dir) = app_in("app-batch-cancel");
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Copy);
        app.handle_action(&ctx, Action::Paste);
        app.handle_action(&ctx, Action::Cancel);
        app.handle_action(&ctx, Action::Cancel);
        wait_paste(&ctx, &mut app);
        assert!(!dir.join("b (copy).txt").exists(), "the second item never starts");
        assert!(app.batch.is_none());
    }

    /// Finishes the entry being copied, without starting a frame.
    fn finish_current(ctx: &egui::Context, app: &mut LumenApp) {
        for _ in 0..500 {
            if let Some(result) = app.job.as_mut().and_then(|r| r.job.poll()) {
                let running = app.job.take().unwrap();
                app.finish_job(ctx, running, result);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("item did not finish");
    }

    #[test]
    fn navigating_mid_paste_keeps_the_rest_in_the_paste_folder() {
        let (ctx, mut app, dir) = app_in("app-batch-drift-nav");
        std::fs::create_dir(dir.join("other")).unwrap();
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Copy);
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        app.navigate(&ctx, dir.join("other"), None);
        land(&mut app);
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir.join("other")), Vec::<String>::new());
        assert_eq!(names_in(&dir.join("sub")), ["a.txt", "b.txt"]);
    }

    #[test]
    fn switching_tabs_mid_paste_keeps_the_rest_in_the_paste_folder() {
        let (ctx, mut app, dir) = app_in("app-batch-drift-tab");
        std::fs::create_dir(dir.join("other")).unwrap();
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Cut);
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        app.navigate(&ctx, dir.join("other"), None);
        land(&mut app);
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir.join("other")), Vec::<String>::new());
        assert_eq!(names_in(&dir.join("sub")), ["a.txt", "b.txt"]);
    }

    #[test]
    fn a_paste_question_waits_for_the_open_trash_question() {
        let (ctx, mut app, dir) = app_in("app-batch-dialog-swap");
        let sub = dir.join("sub");
        std::fs::write(sub.join("b.txt"), "OLD").unwrap();
        std::fs::write(sub.join("c.txt"), "C").unwrap();
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Copy);
        app.navigate(&ctx, sub.clone(), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        let row = app.tab.view.row_of_name("c.txt").unwrap();
        app.tab.view.select_row(row);
        app.handle_action(&ctx, Action::Trash);
        finish_current(&ctx, &mut app);
        assert_eq!(app.dialog.as_ref().unwrap().message(), "Move “c.txt” to the trash?");
        app.answer_dialog(&ctx, Confirm::Yes);
        let trashed = crate::fs::ops::tests::trashed_names_from(&sub);
        crate::fs::ops::tests::purge_trashed_from(&sub);
        assert_eq!(trashed, crate::fs::ops::tests::as_trashed(&["c.txt"]));
        assert_eq!(std::fs::read_to_string(sub.join("b.txt")).unwrap(), "OLD", "not replaced unasked");
        app.raise_parked_question();
        let dialog = app.dialog.as_ref().expect("asked once the other question is gone");
        assert_eq!(dialog.message(), "“b.txt” already exists here. Replace it? The existing one goes to the trash.");
        app.answer_dialog(&ctx, Confirm::No);
        wait_paste(&ctx, &mut app);
        assert!(app.batch.is_none());
    }

    #[test]
    fn a_paste_question_waits_while_the_path_box_menu_or_rename_has_the_keys() {
        let (ctx, mut app, dir) = app_in("app-batch-park");
        std::fs::write(dir.join("sub/a.txt"), "OLD").unwrap();
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.path_popup.open();
        app.paste(&ctx);
        assert!(app.dialog.is_none(), "typing a path must not answer it");
        app.raise_parked_question();
        assert!(app.dialog.is_none());
        app.path_popup = PathPopup::default();
        app.raise_parked_question();
        assert!(app.dialog.is_some());
    }

    #[test]
    fn a_folder_made_by_lumen_marks_the_index_stale_so_the_next_opening_renews_it() {
        let (ctx, mut app, dir) = app_in("app-index-stale");
        let starts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = starts.clone();
        let restart: Box<dyn Fn(&egui::Context) -> Crawler> = Box::new(move |_| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Crawler::fixed(fs::index::Index::default(), false)
        });
        app.path_popup = PathPopup::with_crawler(Crawler::fixed(fs::index::Index::default(), true), restart);
        app.new_folder(&ctx);
        app.path_popup.open();
        let input = egui::RawInput { time: Some(5.0), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..egui::RawInput::default() };
        ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| app.path_popup.show(ui.ctx(), ui.max_rect(), &dir, None, false, Vec::new));
        })
        .drop_without_applying_deltas();
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1, "five seconds old, yet renewed");
    }

    #[test]
    fn the_dialogs_say_what_enter_and_escape_do() {
        let subject = ui::confirm::Subject::of(&[]);
        let trash = Dialog::Trash { paths: vec![PathBuf::from("/x/a")], select: None, subject };
        assert_eq!(trash.hint(), "Enter to confirm · Esc to cancel", "as before");
        let paste = Dialog::Paste { source: "/x/a".into(), target: "/y/a".into(), mode: Mode::Copy };
        assert_eq!(paste.hint(), "Enter to replace · Esc to skip");
    }

    /// Runs the frames that start and land the tab's flatten walk.
    fn land_flat(ctx: &egui::Context, app: &mut LumenApp) {
        app.walk_flat(ctx);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.tab.flattener.is_some() {
            assert!(Instant::now() < deadline, "the walk never landed");
            app.poll_flat();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_nested_rows_grey_folder_opens_that_folder_with_the_row_selected() {
        let (ctx, mut app, dir) = app_in("app-flat-open-folder");
        std::fs::create_dir(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join("sub/deeper/d.txt"), "D").unwrap();
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        let row = app.tab.view.rows.iter().position(|&i| app.tab.view.entry(i).name == "d.txt").unwrap();
        app.open_folder_of(&ctx, row);
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub/deeper")), "opens the folder it is in");
        land(&mut app);
        assert!(!app.tab.view.is_flat(), "the folder alone");
        assert_eq!(app.tab.view.selected_entry().map(|e| e.name.as_str()), Some("d.txt"), "the row it came from is selected");
        app.handle_action(&ctx, Action::Back);
        assert_eq!(app.tab.lister.last_requested(), Some(dir), "Back returns");
    }

    /// A frame at egui time `time`, so what the app reads from `ctx.input` moves on.
    fn frame_at(ctx: &egui::Context, time: f64) {
        ctx.run_ui(egui::RawInput { time: Some(time), ..egui::RawInput::default() }, |_| {}).drop_without_applying_deltas();
    }

    #[test]
    fn the_second_click_of_a_double_click_on_a_grey_folder_opens_nothing_there() {
        let (ctx, mut app, dir) = app_in("app-flat-open-folder-double");
        std::fs::create_dir_all(dir.join("sub/deeper/inner")).unwrap();
        std::fs::write(dir.join("sub/deeper/d.txt"), "D").unwrap();
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        let row = app.tab.view.rows.iter().position(|&i| app.tab.view.entry(i).name == "d.txt").unwrap();
        frame_at(&ctx, 10.0);
        app.handle_list(&ctx, Some(ListEvent::OpenFolder(row)));
        land(&mut app);
        let inner = app.tab.view.rows.iter().position(|&i| app.tab.view.entry(i).name == "inner").unwrap();
        // The second click lands on another row of the folder just opened.
        frame_at(&ctx, 10.2);
        app.handle_list(&ctx, Some(ListEvent::Activate(inner)));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub/deeper")), "nothing else opened");
        assert_eq!(app.tab.view.selected_entry().map(|e| e.name.as_str()), Some("d.txt"), "the cursor stays on the row it came from");
        // A double click of its own, later, opens the row as ever.
        frame_at(&ctx, 11.0);
        app.handle_list(&ctx, Some(ListEvent::Activate(inner)));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub/deeper/inner")), "a later double click opens it");
    }

    #[test]
    fn the_lists_open_folder_event_opens_the_folder_the_row_is_in() {
        let (ctx, mut app, dir) = app_in("app-flat-open-folder-event");
        std::fs::create_dir(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join("sub/deeper/d.txt"), "D").unwrap();
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        let row = app.tab.view.rows.iter().position(|&i| app.tab.view.entry(i).name == "d.txt").unwrap();
        app.handle_list(&ctx, Some(ListEvent::OpenFolder(row)));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub/deeper")), "opens the folder it is in");
        land(&mut app);
        assert!(!app.tab.view.is_flat(), "the folder alone");
        assert_eq!(app.tab.view.selected_entry().map(|e| e.name.as_str()), Some("d.txt"), "the row it came from is selected");
    }

    #[test]
    fn ctrl_e_lists_what_the_subfolders_hold_and_again_lists_the_folder_alone() {
        let (ctx, mut app, dir) = app_in("app-flat-toggle");
        std::fs::create_dir(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join("sub/c.txt"), "C").unwrap();
        std::fs::write(dir.join("sub/deeper/d.txt"), "D").unwrap();
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        let names: Vec<&str> = app.tab.view.rows.iter().map(|&i| app.tab.view.entry(i).name.as_str()).collect();
        assert_eq!(names, ["deeper", "sub", "a.txt", "b.txt", "c.txt", "d.txt"]);
        assert_eq!(app.tab.view.folder_count(), 2);
        let d = app.tab.view.entry(app.tab.view.rows[5]).clone();
        assert_eq!(app.tab.view.relative_parent(&d).as_deref(), Some("sub > deeper"));
        app.handle_action(&ctx, Action::ToggleFlat);
        let names: Vec<&str> = app.tab.view.rows.iter().map(|&i| app.tab.view.entry(i).name.as_str()).collect();
        assert_eq!(names, ["sub", "a.txt", "b.txt"]);
        assert!(app.tab.flattener.is_none(), "nothing left walking");
    }

    #[test]
    fn ctrl_e_watches_the_subfolders_listed_and_again_only_the_folder() {
        let (ctx, mut app, dir) = app_in("app-flat-watch");
        std::fs::create_dir(dir.join("sub/deeper")).unwrap();
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        app.watcher.settle();
        std::fs::write(dir.join("sub/deeper/new.txt"), "").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(app.watcher.take_changed(), "a change in a subfolder of the flattened view");
        app.handle_action(&ctx, Action::ToggleFlat);
        app.watcher.settle();
        // Not on Windows: there the subfolder's own date changes too, which the folder reports.
        #[cfg(not(windows))]
        {
            std::fs::write(dir.join("sub/deeper/other.txt"), "").unwrap();
            std::thread::sleep(Duration::from_millis(300));
            assert!(!app.watcher.take_changed(), "listed alone again, the subfolders are not watched");
        }
    }

    /// End to end, as on screen: the watcher's news schedule the reload, the
    /// reload walks again, and the new file deep down becomes a row.
    #[test]
    fn a_file_created_deep_inside_a_flattened_folder_shows_up_by_itself() {
        let (ctx, mut app, dir) = app_in("app-flat-watch-e2e");
        std::fs::create_dir(dir.join("sub/deeper")).unwrap();
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        app.watcher.settle();
        let names = |app: &LumenApp| app.tab.view.rows.iter().map(|&i| app.tab.view.entry(i).name.clone()).collect::<Vec<_>>();
        assert!(!names(&app).contains(&"new.txt".to_string()));
        std::fs::write(dir.join("sub/deeper/new.txt"), "N").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !names(&app).contains(&"new.txt".to_string()) {
            assert!(Instant::now() < deadline, "the flattened view never showed the new file");
            app.refresh_if_changed(&ctx);
            if app.tab.lister.is_pending(app.tab.shown_id) {
                land(&mut app);
                land_flat(&ctx, &mut app);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let row = app.tab.view.rows.iter().map(|&i| app.tab.view.entry(i)).find(|e| e.name == "new.txt").unwrap().clone();
        assert_eq!(app.tab.view.relative_parent(&row).as_deref(), Some("sub > deeper"), "with its folder dimmed after it");
        assert!(app.tab.view.is_flat(), "still flattened after the reload");
    }

    #[test]
    fn a_change_while_the_flatten_walk_runs_waits_for_it_to_land() {
        let (ctx, mut app, _dir) = app_in("app-flat-busy");
        app.handle_action(&ctx, Action::ToggleFlat);
        app.walk_flat(&ctx);
        assert!(app.tab.flattener.is_some(), "walking");
        // A change already due.
        app.refresh.note_change(Instant::now() - REFRESH_DELAY);
        app.refresh_if_changed(&ctx);
        assert!(!app.tab.lister.is_pending(app.tab.shown_id), "no reload restarts the walk");
        land_flat(&ctx, &mut app);
        std::thread::sleep(REFRESH_DELAY);
        app.refresh_if_changed(&ctx);
        assert!(app.tab.lister.is_pending(app.tab.shown_id), "the change reloads once the walk landed");
    }

    #[test]
    fn a_flattened_folder_with_more_folders_than_the_limit_is_listed_but_not_watched() {
        let (ctx, mut app, dir) = app_in("app-flat-too-many");
        for i in 0..fs::watch::WATCH_LIMIT {
            std::fs::create_dir(dir.join(format!("sub/d{i}"))).unwrap();
        }
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        assert!(app.tab.view.lists(&dir.join("sub/d0")), "listed all the same");
        assert_eq!(app.notice.as_deref(), Some("Too many folders to watch: press F5 to refresh"));
        app.watcher.settle();
        std::fs::write(dir.join("top.txt"), "").unwrap();
        std::fs::write(dir.join("sub/d0/x.txt"), "").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        app.refresh_if_changed(&ctx);
        std::thread::sleep(REFRESH_DELAY);
        app.refresh_if_changed(&ctx);
        assert!(!app.tab.lister.is_pending(app.tab.shown_id), "no reload walks them all again");
    }

    #[test]
    fn a_flattened_folder_at_the_limit_is_watched() {
        let (ctx, mut app, dir) = app_in("app-flat-at-limit");
        // `sub` and these: the limit, not one more.
        for i in 1..fs::watch::WATCH_LIMIT {
            std::fs::create_dir(dir.join(format!("sub/d{i}"))).unwrap();
        }
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        assert_eq!(app.notice, None);
        app.watcher.settle();
        std::fs::write(dir.join("sub/d1/x.txt"), "").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(app.watcher.take_changed(), "a change deep inside reloads the view");
    }

    #[test]
    fn a_flattened_folder_reloaded_after_a_change_walks_again() {
        let (ctx, mut app, dir) = app_in("app-flat-reload");
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        std::fs::write(dir.join("sub/new.txt"), "N").unwrap();
        app.navigate_as(&ctx, NavKind::Refresh, dir.clone(), None);
        land(&mut app);
        land_flat(&ctx, &mut app);
        assert!(app.tab.view.lists(&dir.join("sub/new.txt")));
    }

    #[test]
    fn ctrl_e_on_picked_folders_lists_only_what_they_hold() {
        let (ctx, mut app, dir) = app_in("app-flat-partial");
        std::fs::create_dir(dir.join("other")).unwrap();
        std::fs::write(dir.join("sub/c.txt"), "C").unwrap();
        std::fs::write(dir.join("other/o.txt"), "O").unwrap();
        app.tab.view.apply_refresh(listing_of(&dir));
        // A click on a file, then a Ctrl+click on `sub` (the first one keeps the cursor row picked).
        let a = app.tab.view.row_of_path(&dir.join("a.txt")).unwrap();
        app.tab.view.select_row(a);
        let sub = app.tab.view.row_of_path(&dir.join("sub")).unwrap();
        app.tab.view.toggle_pick(sub);
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        assert!(app.tab.view.lists(&dir.join("sub/c.txt")));
        assert!(!app.tab.view.lists(&dir.join("other/o.txt")), "a folder not picked stays folded");
        assert_eq!(app.tab.view.rows.len(), 5, "other, sub, a.txt, b.txt, c.txt");
        // The same folders again: the folder alone.
        app.handle_action(&ctx, Action::ToggleFlat);
        assert!(!app.tab.view.is_flat());
        assert_eq!(app.tab.view.rows.len(), 4);
    }

    #[test]
    fn picked_folders_are_flattened_the_cursor_alone_flattens_everything() {
        let x = PathBuf::from("/x");
        let (a, b) = (x.join("a"), x.join("b"));
        assert_eq!(next_flat(&[], &x, &[]), vec![x.clone()], "nothing picked: all of it");
        assert_eq!(next_flat(std::slice::from_ref(&x), &x, &[]), Vec::<PathBuf>::new(), "again: off");
        assert_eq!(next_flat(&[], &x, std::slice::from_ref(&a)), vec![a.clone()]);
        assert_eq!(next_flat(std::slice::from_ref(&a), &x, std::slice::from_ref(&b)), [a.clone(), b.clone()], "another folder is added");
        assert_eq!(next_flat(&[a.clone(), b.clone()], &x, std::slice::from_ref(&a)), vec![b.clone()], "one already listed is taken out");
        assert_eq!(next_flat(std::slice::from_ref(&a), &x, &[a.join("deep")]), Vec::<PathBuf>::new(), "inside a listed one: already listed, so out with it");
        assert_eq!(next_flat(std::slice::from_ref(&a), &x, &[]), Vec::<PathBuf>::new(), "nothing picked: off");
        assert_eq!(next_flat(std::slice::from_ref(&x), &x, std::slice::from_ref(&a)), Vec::<PathBuf>::new(), "all of it listed: off");
    }

    #[test]
    fn ctrl_e_does_nothing_on_this_pc() {
        let (ctx, mut app, _dir) = app_in("app-flat-marker");
        app.tab.view.apply_listing(ok_listing(&crate::places::this_pc().to_string_lossy(), Vec::new()), None);
        app.handle_action(&ctx, Action::ToggleFlat);
        assert!(!app.tab.view.is_flat());
    }

    #[test]
    fn hidden_and_filtered_picks_are_not_trashed() {
        let (ctx, mut app, dir) = app_in("app-batch-hidden-picks");
        std::fs::write(dir.join(".h"), "H").unwrap();
        app.tab.view.apply_refresh(listing_of(&dir));
        app.handle_action(&ctx, Action::ToggleHidden);
        app.handle_action(&ctx, Action::SelectAll);
        app.handle_action(&ctx, Action::ToggleHidden);
        app.handle_action(&ctx, Action::Trash);
        assert_eq!(app.dialog.as_ref().unwrap().message(), "Move 3 items to the trash?");
        app.answer_dialog(&ctx, Confirm::No);
        app.handle_action(&ctx, Action::TypeChar('a'));
        app.handle_action(&ctx, Action::Trash);
        assert_eq!(app.dialog.as_ref().unwrap().message(), "Move “a.txt” to the trash?");
        app.answer_dialog(&ctx, Confirm::No);
        assert_eq!(names_in(&dir), [".h", "a.txt", "b.txt", "sub"]);
    }

    #[test]
    fn escape_with_only_unseen_picks_goes_on_to_the_copy() {
        let (ctx, mut app, dir) = app_in("app-escape-unseen");
        std::fs::write(dir.join(".g"), "G").unwrap();
        std::fs::write(dir.join(".h"), "H").unwrap();
        app.tab.view.apply_refresh(listing_of(&dir));
        app.handle_action(&ctx, Action::ToggleHidden);
        pick(&mut app, &[".g", ".h"]);
        app.handle_action(&ctx, Action::ToggleHidden);
        assert_eq!(app.tab.view.picked_count(), 0, "the picks are hidden");
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.job.as_ref().unwrap().job.cancel_requested(), "no picks shown: Esc reaches the copy");
    }

    #[test]
    fn copying_during_a_cut_paste_keeps_the_new_clipboard_whole() {
        let (ctx, mut app, dir) = app_in("app-batch-recopy");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Cut);
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt"), dir.join("b.txt")], mode: Mode::Copy });
        wait_paste(&ctx, &mut app);
        let Some(Clip { paths, .. }) = &app.clipboard else { panic!("kept") };
        assert_eq!(paths, &[dir.join("a.txt"), dir.join("b.txt")], "not the paste's own clipboard");
    }

    #[test]
    fn pasting_while_a_paste_waits_for_an_answer_is_refused() {
        let (ctx, mut app, dir) = app_in("app-batch-busy");
        std::fs::write(dir.join("sub/a.txt"), "OLD").unwrap();
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.paste(&ctx);
        assert!(app.dialog.is_some());
        app.dialog = None; // as if a second Ctrl+V got past the modal
        app.paste(&ctx);
        assert_eq!(app.tab.view.error.as_deref(), Some("Another operation is still running"));
    }

    #[test]
    fn delete_with_several_picked_asks_once_and_trashes_them_all() {
        let (ctx, mut app, dir) = app_in("app-batch-trash");
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Trash);
        let dialog = app.dialog.as_ref().expect("asks first");
        assert_eq!(dialog.message(), "Move 2 items to the trash?");
        assert_eq!((dialog.yes_label(), dialog.no_label()), ("Move to Trash", "Cancel"));
        app.answer_dialog(&ctx, Confirm::Yes);
        assert_eq!(names_in(&dir), ["sub"]);
        assert_eq!(crate::fs::ops::tests::trashed_names_from(&dir), crate::fs::ops::tests::as_trashed(&["a.txt", "b.txt"]));
        crate::fs::ops::tests::purge_trashed_from(&dir);
    }

    #[test]
    fn the_trash_and_delete_questions_say_what_they_remove() {
        let (ctx, mut app, _dir) = app_in("app-dialog-subject");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Trash);
        let dialog = app.dialog.as_ref().expect("asks first");
        assert_eq!((dialog.title(), dialog.danger()), ("Move to Trash", false));
        let subject = dialog.subject().expect("what it removes");
        assert_eq!((subject.name.as_str(), &subject.facts[0]), ("a.txt", &("Type", "TXT file".to_owned())));
        app.answer_dialog(&ctx, Confirm::No);
        pick(&mut app, &["a.txt", "sub"]);
        app.handle_action(&ctx, Action::DeleteForever);
        let dialog = app.dialog.as_ref().expect("asks first");
        assert_eq!((dialog.title(), dialog.danger()), ("Delete Permanently", true));
        assert_eq!(dialog.subject().unwrap().name, "2 items");
        app.answer_dialog(&ctx, Confirm::No);
    }

    #[test]
    fn shift_delete_asks_once_and_deletes_without_the_trash() {
        let (ctx, mut app, dir) = app_in("app-delete-forever");
        pick(&mut app, &["a.txt", "sub"]);
        app.handle_action(&ctx, Action::DeleteForever);
        let dialog = app.dialog.as_ref().expect("asks first");
        assert_eq!(dialog.message(), "Permanently delete 2 items? This cannot be undone.");
        assert_eq!((dialog.yes_label(), dialog.no_label(), dialog.hint()), ("Delete", "Cancel", "Enter to delete · Esc to cancel"));
        app.answer_dialog(&ctx, Confirm::No);
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"], "Esc keeps everything");
        app.handle_action(&ctx, Action::DeleteForever);
        app.answer_dialog(&ctx, Confirm::Yes);
        assert_eq!(names_in(&dir), ["b.txt"]);
        assert_eq!(crate::fs::ops::tests::trashed_names_from(&dir), Vec::<String>::new());
        assert_eq!(app.tab.lister.last_requested(), Some(dir.clone()), "listed again");
    }

    #[test]
    fn shift_delete_of_one_entry_names_it() {
        let (ctx, mut app, _dir) = app_in("app-delete-forever-one");
        pick(&mut app, &["b.txt"]);
        app.handle_action(&ctx, Action::DeleteForever);
        assert_eq!(app.dialog.as_ref().unwrap().message(), "Permanently delete “b.txt”? This cannot be undone.");
    }

    #[test]
    fn nothing_inside_the_trash_is_deleted_forever_from_lumen() {
        let (ctx, mut app, dir) = app_in("app-delete-forever-trash");
        app.trash = Some(dir.clone());
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::DeleteForever);
        assert!(app.dialog.is_none());
        assert_eq!(app.tab.view.error.as_deref(), Some("This is already in the trash"));
    }

    #[test]
    fn delete_of_one_entry_still_names_it_and_no_keeps_it() {
        let (ctx, mut app, dir) = app_in("app-trash-one");
        pick(&mut app, &["b.txt"]);
        app.handle_action(&ctx, Action::Trash);
        assert_eq!(app.dialog.as_ref().unwrap().message(), "Move “b.txt” to the trash?");
        app.answer_dialog(&ctx, Confirm::No);
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"]);
    }

    #[test]
    fn rename_and_count_do_nothing_with_several_picked() {
        let (ctx, mut app, _dir) = app_in("app-batch-single");
        pick(&mut app, &["a.txt", "sub"]);
        app.handle_action(&ctx, Action::Rename);
        app.handle_action(&ctx, Action::Count);
        assert!(app.rename.is_none() && app.counting.is_none());
        pick(&mut app, &["sub"]);
        app.handle_action(&ctx, Action::Rename);
        assert_eq!(app.rename.as_ref().unwrap().text, "sub", "one picked row is the selection");
    }

    fn menu_labels(app: &LumenApp) -> Vec<String> {
        app.context_menu.as_ref().expect("menu open").items().iter().map(|item| item.label.clone()).collect()
    }

    /// The open menu's top-level item that runs `command`. Its label is the
    /// shell's on Windows ("Delete", "Eliminar"), so tests look for commands.
    fn menu_item<'a>(app: &'a LumenApp, command: &Command) -> Option<&'a crate::menu::Item> {
        let items = app.context_menu.as_ref().expect("menu open").items();
        items.iter().find(|item| item.kind == crate::menu::Kind::Run(command.clone()))
    }

    /// Polls until the menu shows (the shell answered), or 20 s pass. Off
    /// Windows the menu shows at once.
    fn wait_for_menu(app: &mut LumenApp) {
        #[cfg(windows)]
        {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while app.context_menu.is_none() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
                app.poll_shell_menu();
            }
        }
        let _ = app;
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_menus_show_whole_once_the_shell_answers() {
        let (_ctx, mut app, _dir) = app_in("app-menu-async");
        let shell_item = |app: &LumenApp| {
            let items = app.context_menu.as_ref().expect("menu open").items();
            items.iter().any(|item| matches!(item.kind, crate::menu::Kind::Run(Command::Shell(_))))
        };
        for background in [false, true] {
            let started = std::time::Instant::now();
            if background {
                app.open_background_menu(egui::pos2(10.0, 10.0));
            } else {
                app.open_entry_menu(1, egui::pos2(10.0, 10.0));
            }
            let took = started.elapsed();
            assert!(took < Duration::from_millis(100), "the window never waits for the shell: {took:?}");
            assert!(app.context_menu.is_none(), "nothing shows before the shell answered");
            wait_for_menu(&mut app);
            let own = if background { Command::Refresh } else { Command::Trash };
            assert!(menu_item(&app, &own).is_some(), "Lumen's own items: {:?}", menu_labels(&app));
            assert!(shell_item(&app), "the shell's items, in the same menu: {:?}", menu_labels(&app));
            assert!(!menu_labels(&app).iter().any(|label| label.starts_with("Loading")), "no loading line");
            app.context_menu = None;
            app.shell_menu = None;
        }
    }

    #[cfg(windows)]
    #[test]
    fn escape_or_a_left_click_before_the_menu_shows_cancels_it_and_goes_no_further() {
        use crate::ui::testing::frame;
        let (ctx, mut app, _dir) = app_in("app-menu-cancel");
        let press = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let escape = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        for cancel in [escape, press(true)] {
            app.open_entry_menu(1, egui::pos2(10.0, 10.0));
            let passed_on = frame(&ctx, vec![cancel], |ui| {
                app.read_pending_menu(ui.ctx());
                ui.input(|i| i.key_pressed(egui::Key::Escape))
            });
            assert!(!passed_on, "the list never sees that Escape");
            assert!(app.menu_pending.is_none() && app.shell_menu.is_none(), "cancelled");
            std::thread::sleep(Duration::from_millis(300));
            frame(&ctx, vec![], |ui| app.read_pending_menu(ui.ctx()));
            assert!(app.context_menu.is_none(), "and it never shows");
        }
    }

    #[test]
    fn a_right_click_outside_the_selection_selects_that_row_first() {
        let (_ctx, mut app, _dir) = app_in("app-menu-row"); // sub, a.txt, b.txt
        pick(&mut app, &["a.txt", "b.txt"]);
        app.open_entry_menu(0, egui::pos2(10.0, 10.0));
        assert_eq!(selection_names(&app.tab.view), ["sub"]);
        wait_for_menu(&mut app);
        assert!(menu_labels(&app).contains(&"Open in New Tab".to_owned()), "the menu of a folder");
    }

    #[test]
    fn shift_f10_opens_the_menu_of_the_selection() {
        let (ctx, mut app, _dir) = app_in("app-menu-key");
        pick(&mut app, &["a.txt", "b.txt"]);
        app.handle_action(&ctx, Action::Menu);
        assert_eq!(selection_names(&app.tab.view), ["a.txt", "b.txt"], "the picks stay");
        wait_for_menu(&mut app);
        assert!(menu_item(&app, &Command::Trash).is_some(), "{:?}", menu_labels(&app));
    }

    #[test]
    fn a_right_click_on_a_picked_row_keeps_every_pick() {
        let (_ctx, mut app, _dir) = app_in("app-menu-picked");
        pick(&mut app, &["a.txt", "b.txt"]);
        app.open_entry_menu(1, egui::pos2(10.0, 10.0));
        assert_eq!(selection_names(&app.tab.view), ["a.txt", "b.txt"]);
    }

    #[test]
    fn a_right_click_on_the_empty_space_keeps_the_selection() {
        let (_ctx, mut app, _dir) = app_in("app-menu-background");
        pick(&mut app, &["b.txt"]);
        app.open_background_menu(egui::pos2(10.0, 10.0));
        assert_eq!(selection_names(&app.tab.view), ["b.txt"]);
        wait_for_menu(&mut app);
        #[cfg(not(windows))]
        assert_eq!(menu_labels(&app), ["Paste", "New", "Open Terminal Here", "", "Sort by", "Refresh"]);
        // On Windows the shell's own items come first, whatever the PC has.
        let labels = menu_labels(&app);
        assert!(labels.contains(&"Sort by".to_owned()) && labels.contains(&"Refresh".to_owned()), "{labels:?}");
        let paste = menu_item(&app, &Command::Paste).expect("Paste is always offered");
        assert!(!paste.enabled, "nothing to paste yet");
    }

    #[test]
    fn menu_commands_act_on_the_selection() {
        let (ctx, mut app, dir) = app_in("app-menu-run");
        pick(&mut app, &["sub"]);
        app.run_command(&ctx, Command::ToggleBookmark);
        assert!(app.bookmarks.contains(&dir.join("sub")), "the folder clicked, not the one on screen");
        assert!(!app.bookmarks.contains(&dir));
        app.run_command(&ctx, Command::OpenInNewTab);
        assert_eq!((app.tabs.len(), app.active), (2, 1));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub")));
        land(&mut app);
        assert_eq!(app.tab.view.current, dir.join("sub"));
    }

    #[test]
    fn a_new_file_from_the_menu_is_created_and_renamed() {
        let (ctx, mut app, dir) = app_in("app-menu-new-file");
        app.run_command(&ctx, Command::NewFile(None));
        assert!(dir.join("New File").is_file());
        land(&mut app);
        assert_eq!(app.rename.as_ref().map(|e| e.text.as_str()), Some("New File"), "named right away");
    }

    #[test]
    fn paste_into_pastes_in_the_selected_folder_not_the_one_on_screen() {
        let (ctx, mut app, dir) = app_in("app-paste-into");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Copy);
        pick(&mut app, &["sub"]);
        app.run_command(&ctx, Command::PasteInto);
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir.join("sub")), ["a.txt"]);
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"], "no copy here");
    }

    #[test]
    fn a_drop_moves_into_the_folder_and_leaves_the_clipboard_alone() {
        let (ctx, mut app, dir) = app_in("app-drop-move");
        pick(&mut app, &["b.txt"]);
        app.handle_action(&ctx, Action::Copy);
        app.drop_into(&ctx, vec![dir.join("a.txt")], dir.join("sub"), Mode::Move);
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir.join("sub")), ["a.txt"]);
        assert_eq!(names_in(&dir), ["b.txt", "sub"]);
        assert_eq!(app.clipboard.as_ref().map(|c| (c.paths.clone(), c.mode)), Some((vec![dir.join("b.txt")], Mode::Copy)));
        assert!(app.tab.view.error.is_none());
    }

    /// A drop on `target` of `paths` from another app, with the keys that
    /// stand for the action agreed with it.
    fn from_outside(paths: Vec<PathBuf>, target: PathBuf, mode: Mode) -> ui::dnd::Dropped {
        let keys = crate::drag::Keys { ctrl: mode == Mode::Copy, shift: mode == Mode::Move };
        ui::dnd::Dropped { drag: crate::drag::Drag { paths }, target, keys }
    }

    #[test]
    fn files_dropped_from_another_app_land_in_the_folder_on_screen() {
        let (ctx, mut app, dir) = app_in("app-drop-outside");
        let other = crate::fs::ops::tests::fresh_fixture("app-drop-outside-source");
        std::fs::write(other.join("n.txt"), "N").unwrap();
        std::fs::write(other.join("m.txt"), "M").unwrap();
        app.clipboard = Some(Clip { paths: vec![dir.join("b.txt")], mode: Mode::Copy });
        app.run_drop(&ctx, from_outside(vec![other.join("n.txt"), other.join("m.txt")], dir.clone(), Mode::Move));
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "m.txt", "n.txt", "sub"]);
        assert!(names_in(&other).is_empty(), "a move: gone from the source");
        assert!(app.clipboard.is_some(), "the clipboard is left alone");
        assert!(app.tab.view.error.is_none());
    }

    #[test]
    fn files_dropped_from_another_app_on_a_folder_land_in_that_folder_with_the_agreed_action() {
        let (ctx, mut app, dir) = app_in("app-drop-outside-named");
        let other = crate::fs::ops::tests::fresh_fixture("app-drop-outside-named-source");
        std::fs::write(other.join("n.txt"), "N").unwrap();
        std::fs::write(other.join("m.txt"), "M").unwrap();
        // The folder on screen is `dir`; the drop is on `sub`, a move.
        app.run_drop(&ctx, from_outside(vec![other.join("n.txt")], dir.join("sub"), Mode::Move));
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir.join("sub")), ["n.txt"], "in the folder under the pointer");
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"], "not in the folder on screen");
        assert_eq!(names_in(&other), ["m.txt"], "a move: gone from the source");
        // The agreed copy is a copy even where the disks would move.
        app.run_drop(&ctx, from_outside(vec![other.join("m.txt")], dir.join("sub"), Mode::Copy));
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir.join("sub")), ["m.txt", "n.txt"]);
        assert_eq!(names_in(&other), ["m.txt"], "a copy: the source keeps its file");
    }

    #[test]
    fn a_copy_dropped_from_another_app_on_the_same_disk_leaves_the_source_alone() {
        let (ctx, mut app, dir) = app_in("app-drop-outside-copy");
        let other = crate::fs::ops::tests::fresh_fixture("app-drop-outside-copy-source");
        std::fs::write(other.join("n.txt"), "N").unwrap();
        app.run_drop(&ctx, from_outside(vec![other.join("n.txt")], dir.clone(), Mode::Copy));
        wait_paste(&ctx, &mut app);
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "n.txt", "sub"]);
        assert_eq!(names_in(&other), ["n.txt"], "the source keeps its file");
    }

    #[test]
    fn files_dropped_from_another_app_into_the_trash_view_are_refused() {
        let (ctx, mut app, dir) = app_in("app-drop-outside-trash");
        let other = crate::fs::ops::tests::fresh_fixture("app-drop-outside-trash-source");
        std::fs::write(other.join("n.txt"), "N").unwrap();
        app.trash = Some(dir.clone());
        app.run_drop(&ctx, from_outside(vec![other.join("n.txt")], dir.clone(), Mode::Move));
        assert_eq!(app.tab.view.error.as_deref(), Some("Nothing can be dropped into the trash"));
        assert_eq!(names_in(&other), ["n.txt"]);
    }

    #[test]
    fn a_folder_dropped_from_another_app_onto_itself_does_nothing() {
        let (ctx, mut app, dir) = app_in("app-drop-outside-self");
        app.run_drop(&ctx, from_outside(vec![dir.clone()], dir.clone(), Mode::Move));
        assert!(app.batch.is_none() && app.job.is_none(), "nothing started");
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"]);
    }

    #[test]
    fn a_drop_with_a_taken_name_asks_like_a_paste() {
        let (ctx, mut app, dir) = app_in("app-drop-taken");
        std::fs::write(dir.join("sub").join("a.txt"), "old").unwrap();
        app.drop_into(&ctx, vec![dir.join("a.txt")], dir.join("sub"), Mode::Copy);
        wait_paste(&ctx, &mut app);
        assert!(matches!(&app.dialog, Some(Dialog::Paste { mode: Mode::Copy, .. })), "the replace question");
        assert_eq!(std::fs::read_to_string(dir.join("sub").join("a.txt")).unwrap(), "old", "nothing replaced yet");
    }

    #[test]
    fn a_drop_into_the_trash_is_refused() {
        let (ctx, mut app, dir) = app_in("app-drop-trash");
        let trash = dir.join("sub");
        app.trash = Some(trash.clone());
        std::fs::create_dir(trash.join("old")).unwrap();
        for dest in [trash.clone(), trash.join("old")] {
            app.drop_into(&ctx, vec![dir.join("a.txt")], dest, Mode::Move);
            assert!(app.batch.is_none() && app.job.is_none(), "nothing started");
            assert_eq!(app.tab.view.error.as_deref(), Some("Nothing can be dropped into the trash"));
            app.tab.view.error = None;
        }
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"]);
        assert_eq!(names_in(&trash), ["old"]);
    }

    #[cfg(windows)]
    #[test]
    fn a_drop_into_a_recycle_bin_is_refused_on_windows() {
        let (ctx, mut app, dir) = app_in("app-drop-recycle-bin");
        assert!(app.trash.is_none(), "Windows has no trash place");
        for dest in [r"C:\$Recycle.Bin", r"d:\$RECYCLE.BIN\S-1-5-21"] {
            app.drop_into(&ctx, vec![dir.join("a.txt")], PathBuf::from(dest), Mode::Move);
            assert!(app.batch.is_none() && app.job.is_none(), "nothing started for {dest}");
            assert_eq!(app.tab.view.error.as_deref(), Some("Nothing can be dropped into the trash"));
            app.tab.view.error = None;
        }
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"]);
    }

    #[test]
    fn a_drop_while_a_paste_runs_is_refused() {
        let (ctx, mut app, dir) = app_in("app-drop-busy");
        app.batch = Some(Batch {
            mode: Mode::Copy,
            dest: dir.join("sub"),
            sources: Vec::new(),
            parked: None,
            queue: VecDeque::new(),
            total: 0,
            landed: None,
            in_place: Vec::new(),
        });
        app.drop_into(&ctx, vec![dir.join("a.txt")], dir.join("sub"), Mode::Move);
        assert_eq!(app.tab.view.error.as_deref(), Some("Another operation is still running"));
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"]);
    }

    #[test]
    fn sort_and_refresh_from_the_menu_keep_the_selected_entry() {
        let (ctx, mut app, dir) = app_in("app-menu-sort");
        pick(&mut app, &["b.txt"]);
        app.run_command(&ctx, Command::Sort(SortColumn::Name));
        assert!(app.tab.view.sort.descending, "Name again flips the order");
        assert_eq!(selection_names(&app.tab.view), ["b.txt"]);
        app.run_command(&ctx, Command::Refresh);
        assert_eq!(app.tab.lister.last_requested(), Some(dir.clone()));
        land(&mut app);
        assert_eq!(selection_names(&app.tab.view), ["b.txt"]);
    }

    #[test]
    fn f5_reads_the_open_folder_again_and_keeps_the_selection() {
        let (ctx, mut app, dir) = app_in("app-f5-reload");
        pick(&mut app, &["b.txt"]);
        let asked = app.tab.lister.latest_id();
        app.handle_action(&ctx, Action::Reload);
        assert_ne!(app.tab.lister.latest_id(), asked, "the folder is listed again");
        assert_eq!(app.tab.lister.last_requested(), Some(dir.clone()));
        assert_eq!(app.tab.pending_nav, NavKind::Refresh, "like a reload after a change on disk");
        let asked = app.tab.lister.latest_id();
        app.handle_action(&ctx, Action::Reload);
        assert_eq!(app.tab.lister.latest_id(), asked, "a listing in flight is not superseded");
        std::fs::write(dir.join("c.txt"), "C").unwrap();
        land(&mut app);
        assert_eq!(selection_names(&app.tab.view), ["b.txt"]);
        assert!(row_names(&app.tab.view).contains(&"c.txt"), "and it shows what changed");
    }

    #[test]
    fn f5_on_a_flattened_view_lists_and_walks_it_again() {
        let (ctx, mut app, dir) = app_in("app-f5-flat");
        app.handle_action(&ctx, Action::ToggleFlat);
        land_flat(&ctx, &mut app);
        std::fs::write(dir.join("sub/new.txt"), "N").unwrap();
        app.handle_action(&ctx, Action::Reload);
        assert_eq!(app.tab.pending_nav, NavKind::Refresh);
        land(&mut app);
        assert!(app.tab.view.is_flat(), "still flattened");
        assert!(app.tab.flat_due, "and walked again");
        land_flat(&ctx, &mut app);
        assert!(row_names(&app.tab.view).contains(&"new.txt"), "what the walk found shows up");
    }

    #[test]
    fn f5_on_this_pc_reads_the_drives_again() {
        let (ctx, mut app, _) = app_on_this_pc("app-f5-this-pc");
        app.disks.0 = Some(Instant::now());
        let asked = app.tab.lister.latest_id();
        app.handle_action(&ctx, Action::Reload);
        assert_eq!(app.disks.0, None, "read again on the next frame");
        assert_eq!(app.tab.lister.latest_id(), asked, "no folder listed");
    }

    #[test]
    fn ctrl_shift_c_copies_the_selected_paths_and_leaves_lumens_clipboard_alone() {
        let (ctx, mut app, dir) = app_in("app-copy-path-key");
        let copied = |ctx: &egui::Context| {
            ctx.output(|o| {
                o.commands.iter().find_map(|c| match c {
                    egui::OutputCommand::CopyText(text) => Some(text.clone()),
                    _ => None,
                })
            })
        };
        // Nothing selected (Escape unselects): nothing copied, nothing said.
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.tab.view.selection().is_empty());
        app.handle_action(&ctx, Action::CopyPath);
        assert_eq!((copied(&ctx), app.notice.as_deref()), (None, None));
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::CopyPath);
        assert_eq!(copied(&ctx), Some(dir.join("a.txt").display().to_string()));
        assert_eq!(app.notice.as_deref(), Some("Path copied"));
        assert!(app.clipboard.is_none(), "no entry is put up for Paste");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_path_writes_every_selected_path_one_per_line() {
        let a = Path::new("/x/a.txt");
        let b = Path::new("/x/b b.txt");
        assert_eq!(paths_text(&[a, b]), "/x/a.txt\n/x/b b.txt");
        assert_eq!(paths_text(&[a]), "/x/a.txt");
    }

    #[test]
    fn escape_cancels_a_pending_cut_but_keeps_a_copy() {
        let (ctx, mut app, dir) = app_in("app-escape-cut");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Cut);
        assert_eq!(app.cut_paths(), [dir.join("a.txt")], "shown greyed");
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.clipboard.is_some(), "the filter goes first");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.clipboard.is_none());
        assert_eq!(app.notice.as_deref(), Some("Cut cancelled"));
        assert!(app.cut_paths().is_empty());
        app.handle_action(&ctx, Action::Copy);
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.clipboard.is_some(), "a copy is not undone by Esc");
        assert!(app.cut_paths().is_empty(), "copies are not greyed");
    }

    #[test]
    fn escape_stops_a_running_cut_paste_before_forgetting_the_cut() {
        let (ctx, mut app, dir) = app_in("app-escape-cut-running");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Cut);
        app.navigate(&ctx, dir.join("sub"), None);
        land(&mut app);
        app.handle_action(&ctx, Action::Paste);
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.job.as_ref().unwrap().job.cancel_requested());
        assert!(app.clipboard.is_some(), "one Esc, one thing undone");
    }

    #[test]
    fn escape_drops_the_multi_selection_after_the_filter_and_before_the_copy() {
        let (ctx, mut app, dir) = app_in("app-escape-picks");
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
        app.paste(&ctx);
        app.handle_action(&ctx, Action::SelectAll);
        app.handle_action(&ctx, Action::TypeChar('t'));
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.filter(), "");
        assert_eq!(app.tab.view.picked_count(), 3, "the filter went first");
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.picked_count(), 0);
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "the copy goes on");
        app.handle_action(&ctx, Action::CancelHeld);
        assert!(!app.job.as_ref().unwrap().job.cancel_requested(), "a held Esc stops at the picks");
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
        app.handle_action(&ctx, Action::Activate); // into sub
        land(&mut app);
        app.tab.view.sort_by(SortColumn::Size);
        app.tab.view.toggle_hidden();
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

    fn saved_config() -> Config {
        Config {
            sort: Sort { column: SortColumn::Size, descending: true },
            show_hidden: true,
            sidebar: false,
            preview: true,
            window: Some([1000.0, 700.0]),
            bookmarks: vec![PathBuf::from("/srv"), PathBuf::from("/srv")],
            view: ui::view_size::ViewSize::new(65),
            columns: [250.0, 90.0, 140.0, 55.0, 60.0, 95.0],
            ..Config::default()
        }
    }

    fn look_of(app: &LumenApp) -> (u8, Sort) {
        (app.tab.view_size.percent(), app.tab.view.sort)
    }

    #[test]
    fn a_folder_opens_with_its_saved_view_and_one_without_with_the_default() {
        use crate::folder_views::{Rule, Scope};
        use ui::view_size::ViewSize;
        let (ctx, _, dir) = app_in("app-folder-rule");
        std::fs::create_dir(dir.join("other")).unwrap();
        let size_desc = Sort { column: SortColumn::Size, descending: true };
        let rule = Rule { folder: dir.join("sub"), scope: Scope::Folder, look: Look { view: ViewSize::new(100), sort: size_desc } };
        let config = Config { view: ViewSize::new(20), folder_views: FolderViews::from_rules(vec![rule]), ..Config::default() };
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, Settings { config, path: None, error: None });
        app.apply(listing_of(&dir));
        assert_eq!(look_of(&app), (20, Sort::default()), "no rule: the default");
        app.handle_action(&ctx, Action::ZoomIn);
        app.tab.view.sort_by(SortColumn::Size);
        let changed = (30, Sort { column: SortColumn::Size, descending: false });
        assert_eq!(look_of(&app), changed);
        app.apply(listing_of(&dir));
        assert_eq!(look_of(&app), changed, "a reload of the same folder keeps the visit's change");
        app.apply(listing_of(&dir.join("sub")));
        assert_eq!(look_of(&app), (100, size_desc), "its saved view");
        let names: Vec<_> = app.tab.view.rows.iter().map(|&i| app.tab.view.entry(i).name.clone()).collect();
        assert!(names.is_empty(), "sub is empty here: {names:?}");
        app.apply(listing_of(&dir.join("other")));
        assert_eq!(look_of(&app), (20, Sort::default()), "the change in the first folder lasted only that visit");
        let failed = Listing { id: 0, path: dir.join("gone"), result: Err(ListError::NotFound(dir.join("gone"))), elapsed: Duration::ZERO };
        app.handle_action(&ctx, Action::ZoomIn);
        app.apply(failed);
        assert_eq!(look_of(&app), (30, Sort::default()), "a folder that could not be read changes nothing");
    }

    #[test]
    fn the_listing_that_lands_is_sorted_with_the_folders_saved_order() {
        use crate::folder_views::{Rule, Scope};
        let (ctx, _, dir) = app_in("app-folder-rule-sort");
        std::fs::write(dir.join("sub/a-small.txt"), "1").unwrap();
        std::fs::write(dir.join("sub/z-big.txt"), "1234567890").unwrap();
        let size_desc = Sort { column: SortColumn::Size, descending: true };
        let rule = Rule { folder: dir.join("sub"), scope: Scope::Folder, look: Look { view: Default::default(), sort: size_desc } };
        let config = Config { folder_views: FolderViews::from_rules(vec![rule]), ..Config::default() };
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, Settings { config, path: None, error: None });
        app.apply(listing_of(&dir.join("sub")));
        let names: Vec<_> = app.tab.view.rows.iter().map(|&i| app.tab.view.entry(i).name.clone()).collect();
        assert_eq!(names, ["z-big.txt", "a-small.txt"], "biggest first, as saved");
    }

    #[test]
    fn a_change_under_a_rule_updates_it_and_one_elsewhere_never_moves_the_default() {
        use crate::folder_views::{Rule, Scope};
        use ui::view_size::ViewSize;
        let (ctx, _, dir) = app_in("app-folder-rule-keep");
        std::fs::create_dir(dir.join("sub/deep")).unwrap();
        let path = dir.join("cfg/lumen/config");
        let rule = Rule { folder: dir.join("sub"), scope: Scope::Tree, look: Look { view: ViewSize::new(100), sort: Sort::default() } };
        Config { folder_views: FolderViews::from_rules(vec![rule]), ..Config::default() }.save(&path).unwrap();
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.apply(listing_of(&dir.join("sub/deep")));
        assert_eq!(look_of(&app).0, 100, "covered by the rule above");
        app.handle_action(&ctx, Action::ZoomOut);
        app.save_settings(false);
        let saved = Config::load(&path).unwrap();
        let rules: Vec<_> = saved.folder_views.rules().iter().map(|r| (r.folder.clone(), r.look.view.percent())).collect();
        assert_eq!(rules, [(dir.join("sub"), 95)], "the rule the folder opens with keeps the change");
        app.apply(listing_of(&dir));
        assert_eq!(look_of(&app).0, 0, "no rule: the default");
        app.handle_action(&ctx, Action::ZoomIn);
        app.save_settings(false);
        let saved = Config::load(&path).unwrap();
        assert_eq!(saved.view, ViewSize::new(0), "the default did not move");
        assert_eq!(saved.folder_views.rules().len(), 1, "a change makes no rule");
    }

    #[test]
    fn each_tab_shows_its_own_view_and_a_new_one_carries_the_visits() {
        use ui::view_size::ViewSize;
        let (ctx, mut app, _dir) = app_in("app-tab-view");
        app.tab.view_size = ViewSize::new(100);
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        assert_eq!(app.tab.view_size, ViewSize::new(100), "the same folder, the same visit");
        app.handle_action(&ctx, Action::ZoomReset);
        app.select_tab(&ctx, 0);
        assert_eq!(app.tab.view_size, ViewSize::new(100), "the first tab kept its own");
        app.select_tab(&ctx, 1);
        assert_eq!(app.tab.view_size, ViewSize::new(0));
    }

    #[test]
    fn the_lock_saves_the_view_shown_forgets_it_and_sets_the_default() {
        use crate::folder_views::Scope;
        use ui::toolbar::SavedView;
        use ui::view_size::ViewSize;
        let (ctx, _, dir) = app_in("app-lock");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.apply(listing_of(&dir));
        assert_eq!(app.saved_view(), SavedView { is_default: true, ..SavedView::default() });
        app.tab.view_size = ViewSize::new(65);
        app.tab.view.sort_by(SortColumn::Size);
        let shown = (65, Sort { column: SortColumn::Size, descending: false });
        assert!(!app.saved_view().is_default);
        app.handle_toolbar(&ctx, ToolbarEvent::SaveView(Some(Scope::Tree)));
        assert_eq!(app.saved_view().own, Some(Scope::Tree));
        app.save_settings(false);
        let rules = |path: &Path| -> Vec<_> {
            Config::load(path).unwrap().folder_views.rules().iter().map(|r| (r.folder.clone(), r.scope, r.look.view.percent(), r.look.sort)).collect()
        };
        assert_eq!(rules(&path), [(dir.clone(), Scope::Tree, shown.0, shown.1)], "written at once");
        app.apply(listing_of(&dir.join("sub")));
        assert_eq!(look_of(&app), shown, "its subfolders open with it");
        assert_eq!(app.saved_view(), SavedView { own: None, covered_by: Some(dir.clone()), is_default: false });
        app.apply(listing_of(&dir));
        app.handle_toolbar(&ctx, ToolbarEvent::SaveView(None));
        assert_eq!(app.saved_view().own, None);
        app.save_settings(false);
        assert!(rules(&path).is_empty(), "the rule is forgotten");
        assert_eq!(look_of(&app), shown, "forgetting keeps what is on screen");
        app.handle_toolbar(&ctx, ToolbarEvent::SetDefaultView);
        assert!(app.saved_view().is_default);
        app.save_settings(false);
        let saved = Config::load(&path).unwrap();
        assert_eq!((saved.view.percent(), saved.sort), shown, "the new default is written");
        app.apply(listing_of(&dir.join("sub")));
        assert_eq!(look_of(&app), shown, "a folder with no rule opens with the new default");
    }

    #[test]
    fn saved_settings_are_applied_at_startup() {
        let dir = crate::fs::ops::tests::fresh_fixture("app-config-start");
        let ctx = egui::Context::default();
        let settings = Settings { config: saved_config(), path: None, error: None };
        let app = LumenApp::with_config(&ctx, dir, false, settings);
        assert_eq!(app.tab.view.sort, Sort { column: SortColumn::Size, descending: true });
        assert!(app.tab.view.show_hidden);
        assert!(!app.sidebar_open);
        assert!(app.preview_open);
        assert_eq!(app.bookmarks.paths(), [PathBuf::from("/srv")], "repeats dropped");
        assert_eq!(app.window, Some([1000.0, 700.0]));
        assert_eq!(app.tab.view_size, ui::view_size::ViewSize::new(65));
        assert_eq!(app.columns, [250.0, 90.0, 140.0, 55.0, 60.0, 95.0]);
    }

    #[test]
    fn the_header_menu_sizes_columns_to_fit_restores_them_and_saves_them() {
        let (ctx, _, dir) = app_in("app-fit-columns");
        std::fs::write(dir.join("a rather long name for a file in this folder.txt"), "x").unwrap();
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        let entries = fs::lister::read_entries(&dir).unwrap();
        let listing = Listing { id: 0, path: dir.clone(), result: Ok(entries), elapsed: Duration::ZERO };
        app.tab.view.apply_listing(listing, None);
        // Fonts are loaded by a first frame.
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let fit = |app: &LumenApp, column| ui::file_list::fit_width(&ctx, &app.tab.view, column);
        app.columns = [100.0; 6];
        app.run_command(&ctx, Command::FitColumn(0));
        assert_eq!(app.columns[0], fit(&app, 0));
        assert!(app.columns[0] > 250.0, "the long name fits: {}", app.columns[0]);
        assert_eq!(app.columns[1..], [100.0; 5], "only that column");
        app.run_command(&ctx, Command::RestoreColumns);
        assert_eq!(app.columns, ui::file_list::DEFAULT_WIDTHS);
        app.handle_action(&ctx, Action::FitColumns);
        let all: Vec<f32> = (0..6).map(|c| fit(&app, c)).collect();
        assert_eq!(app.columns.to_vec(), all, "Ctrl+Shift+F fits every column");
        app.run_command(&ctx, Command::RestoreColumns);
        app.run_command(&ctx, Command::FitAllColumns);
        assert_eq!(app.columns.to_vec(), all, "the menu's Size All Columns To Fit too");
        app.save_settings(false);
        assert_eq!(Config::load(&path).unwrap().columns, app.columns);
    }

    #[test]
    fn fit_all_columns_with_a_long_name_stays_inside_the_lists_width() {
        let (ctx, _, dir) = app_in("app-fit-all-inside");
        let long = format!("{}.txt", "a very long name ".repeat(12));
        std::fs::write(dir.join(&long), "x").unwrap();
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(dir.join("cfg/lumen/config"))));
        let entries = fs::lister::read_entries(&dir).unwrap();
        app.tab.view.apply_listing(Listing { id: 0, path: dir.clone(), result: Ok(entries), elapsed: Duration::ZERO }, None);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let fit = |app: &LumenApp, column| ui::file_list::fit_width(&ctx, &app.tab.view, column);
        assert!(fit(&app, 0) > 1000.0, "the long name needs {}", fit(&app, 0));
        ui::file_list::remember_width(&ctx, 1000.0);
        let room = ui::file_list::visible_width(&ctx, &app.shown_columns).unwrap();
        app.run_command(&ctx, Command::FitAllColumns);
        let total: f32 = app.columns.iter().sum();
        assert!((total - room).abs() < 0.5, "the shown columns share the room: {total} in {room}");
        assert_eq!((app.columns[1], app.columns[2]), (fit(&app, 1), fit(&app, 2)), "Type and Date Modified whole");
        assert!(app.columns[0] < fit(&app, 0) && app.columns[0] >= ui::file_list::NAME_MIN);
        // A hidden column takes no room.
        app.run_command(&ctx, Command::RestoreColumns);
        app.run_command(&ctx, Command::ToggleColumn(3));
        let room = ui::file_list::visible_width(&ctx, &app.shown_columns).unwrap();
        app.run_command(&ctx, Command::FitAllColumns);
        let shown: f32 = app.columns.iter().enumerate().filter(|&(c, _)| c != 3).map(|(_, w)| w).sum();
        assert!((shown - room).abs() < 0.5, "{shown} in {room}");
        assert_eq!(app.columns[3], fit(&app, 3));
        // A single column still fits its own content, whatever the room.
        app.run_command(&ctx, Command::FitColumn(0));
        assert_eq!(app.columns[0], fit(&app, 0));
    }

    #[test]
    fn columns_are_hidden_shown_again_restored_and_saved_but_name_stays() {
        let (ctx, _, dir) = app_in("app-hide-columns");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.run_command(&ctx, Command::ToggleColumn(2));
        assert_eq!(app.shown_columns, [true, true, false, true, true, true]);
        app.run_command(&ctx, Command::ToggleColumn(0));
        assert!(app.shown_columns[0], "Name is never hidden");
        app.save_settings(false);
        assert_eq!(Config::load(&path).unwrap().shown_columns, app.shown_columns);
        app.run_command(&ctx, Command::ToggleColumn(2));
        assert!(app.shown_columns[2], "shown again");
        app.shown_columns[4] = false;
        app.columns[1] = 300.0;
        app.run_command(&ctx, Command::RestoreColumns);
        assert_eq!((app.shown_columns, app.columns), (ui::file_list::ALL_SHOWN, ui::file_list::DEFAULT_WIDTHS));
        let reopened = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        assert_eq!(reopened.shown_columns, [true, true, false, true, true, true], "loaded at startup");
    }

    #[test]
    fn columns_shift_left_and_right_are_restored_and_saved() {
        let (ctx, _, dir) = app_in("app-shift-columns");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.run_command(&ctx, Command::ShiftColumnLeft(2));
        assert_eq!(app.column_order, [0, 2, 1, 3, 4, 5]);
        app.run_command(&ctx, Command::ShiftColumnRight(0));
        assert_eq!(app.column_order, [2, 0, 1, 3, 4, 5]);
        app.save_settings(false);
        assert_eq!(Config::load(&path).unwrap().column_order, [2, 0, 1, 3, 4, 5]);
        let reopened = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        assert_eq!(reopened.column_order, [2, 0, 1, 3, 4, 5], "loaded at startup");
        app.run_command(&ctx, Command::RestoreColumns);
        assert_eq!(app.column_order, ui::file_list::DEFAULT_ORDER);
    }

    #[test]
    fn resized_columns_are_saved() {
        let (ctx, _, dir) = app_in("app-columns");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.columns[0] = 410.0;
        app.save_settings(false);
        assert_eq!(Config::load(&path).unwrap().columns[0], 410.0);
    }

    #[test]
    fn ctrl_plus_and_minus_move_the_view_slider_and_leave_the_default_alone() {
        use ui::view_size::ViewSize;
        let (ctx, _, dir) = app_in("app-zoom");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.handle_action(&ctx, Action::ZoomOut);
        assert_eq!(app.tab.view_size, ViewSize::new(0), "already Details");
        let mut walked = Vec::new();
        for _ in 0..5 {
            app.handle_action(&ctx, Action::ZoomIn);
            walked.push(app.tab.view_size.name());
        }
        assert_eq!(walked, ["Tree", "List", "M Icons", "M Icons", "M Icons"]);
        assert_eq!(app.tab.view_size, ViewSize::new(40));
        app.handle_action(&ctx, Action::ZoomOut);
        assert_eq!(app.tab.view_size, ViewSize::new(35));
        app.save_settings(false);
        assert!(!path.exists(), "a folder with no rule: the default did not move, nothing to write");
        app.handle_action(&ctx, Action::ZoomReset);
        assert_eq!(app.tab.view_size, ViewSize::new(0), "Ctrl+0 goes back to Details");
        assert_eq!(ctx.zoom_factor(), 1.0, "the rest of the window never zooms");
    }

    #[test]
    fn in_the_icons_view_up_and_down_cross_rows_and_left_right_walk_the_cards() {
        use ui::grid::GridShape;
        let (ctx, mut app, _dir) = app_in("app-grid-keys");
        app.tab.view_size = ui::view_size::ViewSize::new(30);
        // sub / a.txt b.txt: the folder alone on the first row.
        app.grid = Some(GridShape::new(2, 1, 3));
        let mut at = |action| {
            app.handle_action(&ctx, action);
            app.tab.view.selected_entry().map(|e| e.name.clone()).unwrap()
        };
        assert_eq!(at(Action::First), "sub");
        assert_eq!(at(Action::Move(1)), "a.txt", "down: same place on the next row");
        assert_eq!(at(Action::Side(1)), "b.txt");
        assert_eq!(at(Action::Move(-1)), "sub", "up onto a shorter row: its last card");
        assert_eq!(at(Action::Side(-1)), "sub", "already the first");
        assert_eq!(at(Action::Move(5)), "a.txt", "a page down stops on the last row");
        app.handle_action(&ctx, Action::ExtendSide(1));
        assert_eq!(app.tab.view.picked_count(), 2, "Shift+Right picks on the way");
        app.handle_action(&ctx, Action::Extend(isize::MIN));
        assert_eq!(app.tab.view.selected, Some(0), "Shift+Home: the first card");
        assert!(app.tab.view.is_picked(0) && app.tab.view.is_picked(1) && !app.tab.view.is_picked(2), "from the anchor a.txt back to sub");
    }

    #[test]
    fn in_the_list_view_up_and_down_walk_a_column_and_left_right_cross_columns() {
        use ui::grid::GridShape;
        let (ctx, mut app, _dir) = app_in("app-list-keys");
        app.tab.view_size = ui::view_size::ViewSize::new(20);
        // Columns of 2: sub / a.txt b.txt.
        app.grid = Some(GridShape::new(2, 1, 3));
        let mut at = |action| {
            app.handle_action(&ctx, action);
            app.tab.view.selected_entry().map(|e| e.name.clone()).unwrap()
        };
        assert_eq!(at(Action::First), "sub");
        assert_eq!(at(Action::Move(1)), "a.txt", "down: the next cell, here the next column's top");
        assert_eq!(at(Action::Move(1)), "b.txt");
        assert_eq!(at(Action::Side(-1)), "sub", "left: the column before, held to its last cell");
        assert_eq!(at(Action::Side(1)), "a.txt", "right: same place in the next column");
    }

    #[test]
    fn in_details_left_and_right_do_not_move_the_selection() {
        let (ctx, mut app, _dir) = app_in("app-details-side");
        app.handle_action(&ctx, Action::First);
        app.handle_action(&ctx, Action::Side(1));
        assert_eq!(app.tab.view.selected, Some(0));
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.view.selected, Some(1), "Down still moves one row");
    }

    #[test]
    fn a_new_view_fades_in_briefly_and_growing_icons_do_not() {
        use ui::view_size::ViewSize;
        let (_ctx, mut app, _dir) = app_in("app-view-fade");
        assert_eq!(app.list_opacity(0.0), 1.0, "nothing changed: drawn whole");
        app.tab.view_size = ViewSize::new(20);
        let start = app.list_opacity(10.0);
        assert!(start < 1.0, "List just picked: still fading in ({start})");
        let mid = app.list_opacity(10.0 + VIEW_FADE / 2.0);
        assert!(mid > start && mid < 1.0, "half way: {mid}");
        assert_eq!(app.list_opacity(10.0 + VIEW_FADE), 1.0, "done after the fade");
        app.tab.view_size = ViewSize::new(30);
        assert!(app.list_opacity(20.0) < 1.0, "List to M Icons is another view");
        app.tab.view_size = ViewSize::new(80);
        assert_eq!(app.list_opacity(20.0 + VIEW_FADE), 1.0, "M to XL grows the icons, no new fade");
    }

    #[test]
    fn eguis_own_whole_window_zoom_is_off() {
        let (ctx, _app, _dir) = app_in("app-zoom-egui");
        assert!(!ctx.options(|o| o.zoom_with_keyboard), "Ctrl+Plus must zoom only the list");
    }

    #[test]
    fn a_changed_setting_is_written_at_once_but_a_resize_only_on_close() {
        let (ctx, _, dir) = app_in("app-config-save");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.save_settings(false);
        assert!(!path.exists(), "nothing changed, nothing written");
        app.handle_action(&ctx, Action::ToggleBookmark);
        app.handle_action(&ctx, Action::ToggleHidden);
        app.tab.view.sort_by(SortColumn::Modified);
        app.handle_toolbar(&ctx, ToolbarEvent::ToggleSidebar);
        app.handle_action(&ctx, Action::TypeChar(' ')); // preview
        app.save_settings(false);
        let written = Config::load(&path).unwrap();
        assert_eq!(written.bookmarks, std::slice::from_ref(&dir));
        assert!(written.show_hidden && !written.sidebar && written.preview);
        assert_eq!(written.sort, Sort::default(), "a sort in a folder with no rule lasts the visit");
        app.window = Some([900.0, 600.0]);
        app.save_settings(false);
        assert_eq!(Config::load(&path).unwrap().window, None, "a resize alone is not written");
        app.save_settings(true);
        assert_eq!(Config::load(&path).unwrap().window, Some([900.0, 600.0]), "closing writes it");
    }

    #[test]
    fn a_settings_path_through_a_file_is_reported() {
        let (ctx, _, dir) = app_in("app-config-blocked");
        std::fs::write(dir.join("blocker"), "not a folder").unwrap();
        let path = dir.join("blocker/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path)));
        app.handle_action(&ctx, Action::ToggleHidden);
        app.save_settings(false);
        // Linux refuses to read through a file (ENOTDIR). Windows reports it as
        // missing, so the problem shows up when the change is saved instead.
        let error = if cfg!(unix) {
            app.tab.error_after_load.clone().expect("told once the folder lists")
        } else {
            app.tab.view.error.clone().expect("told when saving")
        };
        let told = if cfg!(unix) { "Cannot read settings" } else { "Cannot save settings" };
        assert!(error.starts_with(told), "{error}");
        assert_eq!(std::fs::read_to_string(dir.join("blocker")).unwrap(), "not a folder");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_save_says_so_once() {
        use std::os::unix::fs::PermissionsExt;
        let (ctx, _, dir) = app_in("app-config-fail");
        let locked = dir.join("locked");
        std::fs::create_dir(&locked).unwrap();
        let mut app = LumenApp::with_config(&ctx, dir, false, crate::config::open(Some(locked.join("config"))));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        app.handle_action(&ctx, Action::ToggleHidden);
        app.save_settings(false);
        let first = app.tab.view.error.take();
        app.save_settings(false);
        let second = app.tab.view.error.take();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = first.expect("the failure is shown");
        assert!(error.starts_with("Cannot save settings: "), "{error}");
        assert_eq!(second, None, "not repeated every frame");
    }

    #[test]
    fn two_windows_keep_each_others_bookmarks() {
        let (ctx, _, dir) = app_in("app-config-two");
        let path = dir.join("cfg/lumen/config");
        let open = || crate::config::open(Some(path.clone()));
        let mut a = LumenApp::with_config(&ctx, dir.clone(), false, open());
        let mut b = LumenApp::with_config(&ctx, dir.clone(), false, open());
        a.handle_action(&ctx, Action::ToggleBookmark);
        a.save_settings(false);
        b.handle_action(&ctx, Action::ToggleHidden);
        b.save_settings(false);
        b.window = Some([900.0, 600.0]);
        b.save_settings(true); // B closes after a resize
        let written = Config::load(&path).unwrap();
        assert_eq!(written.bookmarks, std::slice::from_ref(&dir), "A's bookmark survives B");
        assert!(written.show_hidden, "B's own change is there too");
        assert_eq!(b.bookmarks.paths(), std::slice::from_ref(&dir), "B now lists A's bookmark");
        a.save_settings(false);
        assert!(Config::load(&path).unwrap().show_hidden, "A does not undo B's Ctrl+H");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_settings_file_is_left_alone_on_close() {
        use std::os::unix::fs::PermissionsExt;
        let (ctx, _, dir) = app_in("app-config-unreadable");
        let path = dir.join("config");
        std::fs::write(&path, "bookmark = /srv\nsidebar = false\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.handle_action(&ctx, Action::ToggleBookmark);
        app.window = Some([900.0, 600.0]);
        app.save_settings(true);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bookmark = /srv\nsidebar = false\n");
        let error = app.tab.error_after_load.clone().expect("told once the folder lists");
        assert!(error.starts_with("Cannot read settings"), "{error}");
    }

    /// A long, fixed pseudo-random run of tab operations keeps the bookkeeping sound.
    #[test]
    fn random_tab_operations_keep_the_slots_consistent() {
        let (ctx, mut app, dir) = app_in("app-tab-fuzz");
        let mut seed: u64 = 0x5eed;
        let mut next = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        for _ in 0..2000 {
            match next(8) {
                0 => app.handle_action(&ctx, Action::NewTab),
                1 => app.handle_action(&ctx, Action::NextTab),
                2 => app.handle_action(&ctx, Action::PrevTab),
                3 => app.handle_action(&ctx, Action::CloseTab),
                4 => app.close_tab(&ctx, next(app.tabs.len() + 1)),
                5 => app.select_tab(&ctx, next(app.tabs.len() + 1)),
                6 => app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub"))),
                _ => {}
            }
            if next(2) == 0 && app.tab.lister.last_requested().is_some() {
                land(&mut app);
            }
            assert!(app.active < app.tabs.len());
            for (i, slot) in app.tabs.iter().enumerate() {
                assert_eq!(slot.is_none(), i == app.active, "only the tab on screen has an empty slot");
            }
            assert_eq!(app.tab_labels().len(), app.tabs.len());
        }
    }

    #[test]
    fn visited_folders_are_scored_for_the_quick_jump_and_saved_on_closing() {
        let (ctx, _, dir) = app_in("app-visits");
        let saved = vec![places::Visit { path: dir.join("old"), score: 3.0, at: 5 }];
        let config = Config { visited: saved.clone(), ..Config::default() };
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, Settings { config, path: None, error: None });
        assert_eq!(app.visits.list(), saved, "loaded from the settings");
        app.apply(listing_of(&dir.join("sub")));
        app.tab.pending_nav = NavKind::Refresh;
        app.apply(listing_of(&dir.join("sub")));
        app.tab.pending_nav = NavKind::New;
        let failed = Listing { id: 0, path: dir.join("gone"), result: Err(ListError::NotFound(dir.join("gone"))), elapsed: Duration::ZERO };
        app.apply(failed);
        let paths: Vec<PathBuf> = app.visits.list().iter().map(|v| v.path.clone()).collect();
        assert_eq!(paths, [dir.join("sub"), dir.join("old")], "a refresh or a failure is no visit");
        assert_eq!(app.visits.list()[0].score, 1.0);
        assert_eq!(app.settings(false).visited, saved, "written when Lumen closes");
        assert_eq!(app.settings(true).visited, app.visits.list());
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
        app.run_command(&ctx, Command::ToggleBookmarkOf(dir.join("sub")));
        assert!(app.bookmarks.paths().is_empty());
        assert_eq!(app.notice.as_deref(), Some("Removed “sub” from bookmarks"));
    }

    #[test]
    fn the_side_panel_menu_commands_act_on_their_path_and_leave_the_list_selection_alone() {
        let (ctx, mut app, dir) = app_in("app-place-commands");
        let sub = dir.join("sub");
        pick(&mut app, &["a.txt"]);
        app.run_command(&ctx, Command::ToggleBookmarkOf(sub.clone()));
        assert!(app.bookmarks.contains(&sub) && !app.bookmarks.contains(&dir.join("a.txt")));
        assert_eq!(app.notice.as_deref(), Some("Added “sub” to bookmarks"));
        app.run_command(&ctx, Command::ToggleBookmarkOf(sub.clone()));
        assert!(app.bookmarks.paths().is_empty());
        app.notice = None;
        app.run_command(&ctx, Command::CopyPlacePath(sub.clone()));
        let copied = ctx.output(|o| {
            o.commands.iter().find_map(|c| match c {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
        });
        assert_eq!(copied, Some(sub.display().to_string()));
        assert_eq!(app.notice.as_deref(), Some("Path copied"));
        assert!(app.clipboard.is_none(), "no entry is put up for Paste");
        app.run_command(&ctx, Command::OpenPlace(sub.clone()));
        assert_eq!(app.tab.lister.last_requested(), Some(sub.clone()));
        assert_eq!(selection_names(&app.tab.view), ["a.txt"], "the list kept its selection");
        app.run_command(&ctx, Command::OpenPlaceInNewTab(sub.clone()));
        assert_eq!((app.tabs.len(), app.active), (2, 1));
        assert_eq!(app.tab.lister.last_requested(), Some(sub));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_right_click_on_a_side_panel_item_opens_the_menu_for_that_path_not_the_selection() {
        let (ctx, mut app, dir) = app_in("app-place-menu");
        let sub = dir.join("sub");
        pick(&mut app, &["a.txt"]);
        app.context_on_entry = true;
        app.handle_sidebar(&ctx, SidebarEvent::Menu(sub.clone()));
        wait_for_menu(&mut app);
        assert!(menu_item(&app, &Command::OpenPlace(sub.clone())).is_some(), "{:?}", menu_labels(&app));
        assert!(menu_item(&app, &Command::OpenPlaceInNewTab(sub.clone())).is_some());
        assert!(menu_item(&app, &Command::CopyPlacePath(sub.clone())).is_some());
        let bookmark = menu_item(&app, &Command::ToggleBookmarkOf(sub.clone())).expect("the bookmark item");
        assert_eq!(bookmark.label, "Add to Bookmarks");
        for list in [Command::Open, Command::Cut, Command::Copy, Command::Trash, Command::Rename, Command::DeleteForever] {
            assert!(menu_item(&app, &list).is_none(), "{list:?} would act on the list selection");
        }
        assert!(!app.context_on_entry);
        assert_eq!(selection_names(&app.tab.view), ["a.txt"], "the selection is untouched");
        app.bookmarks.toggle(&sub);
        app.context_menu = None;
        #[cfg(windows)]
        {
            app.shell_menu = None;
        }
        app.handle_sidebar(&ctx, SidebarEvent::Menu(sub.clone()));
        wait_for_menu(&mut app);
        assert_eq!(menu_item(&app, &Command::ToggleBookmarkOf(sub)).expect("the bookmark item").label, "Remove from Bookmarks");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_right_click_on_a_drive_asks_the_shell_for_its_own_menu() {
        let (ctx, mut app, _dir) = app_in("app-place-drive");
        let drive = PathBuf::from(r"C:\");
        app.handle_sidebar(&ctx, SidebarEvent::Menu(drive.clone()));
        assert!(app.context_menu.is_none() && app.menu_pending.is_some(), "waits for the shell like the list's menus");
        wait_for_menu(&mut app);
        assert!(menu_item(&app, &Command::OpenPlace(drive)).is_some(), "{:?}", menu_labels(&app));
        let items = app.context_menu.as_ref().expect("menu open").items();
        assert!(items.iter().any(|item| matches!(item.kind, crate::menu::Kind::Run(Command::Shell(_)))), "the shell's items: {:?}", menu_labels(&app));
    }

    fn drive_row(path: PathBuf, name: &str) -> DriveRow {
        DriveRow { path, name: name.into(), kind: "Local Disk", fs: "NTFS".into(), free: "1.0 GB".into(), size: "2.0 GB".into(), used: Some(0.5), percent: "50%".into(), usage: None }
    }

    /// An app on This PC, listing two drives (folders `sub` and `drive2` of the fixture).
    fn app_on_this_pc(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let (ctx, mut app, dir) = app_in(name);
        std::fs::create_dir(dir.join("drive2")).unwrap();
        app.drives = vec![drive_row(dir.join("sub"), "Windows (C:)"), drive_row(dir.join("drive2"), "Unidad de USB (D:)")];
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::this_pc()));
        land(&mut app);
        (ctx, app, dir)
    }

    #[test]
    fn this_pc_opens_in_the_tab_is_titled_so_and_back_and_forward_come_to_it() {
        let (ctx, mut app, dir) = app_in("app-this-pc-history");
        let pc = places::this_pc();
        assert!(!app.on_this_pc());
        app.handle_sidebar(&ctx, SidebarEvent::Open(pc.clone()));
        assert_eq!(app.tab.lister.last_requested(), Some(pc.clone()), "in the current tab");
        land(&mut app);
        assert_eq!((app.tabs.len(), app.tab.view.current.clone()), (1, pc.clone()));
        assert!(app.on_this_pc());
        assert_eq!(app.tab_labels(), ["This PC"]);
        assert_eq!(app.tab.history.back_target(), Some(&dir));
        assert!(app.tab.view.error.is_none() && app.tab.view.entries.is_empty());
        assert!(!app.recents.paths().contains(&pc) && app.visits.list().iter().all(|v| v.path != pc), "not a folder to come back to");
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        land(&mut app);
        assert_eq!(app.tab.view.current, dir.join("sub"));
        assert!(!app.on_this_pc());
        app.handle_action(&ctx, Action::Back);
        assert_eq!(app.tab.lister.last_requested(), Some(pc.clone()));
        land(&mut app);
        assert!(app.on_this_pc(), "back shows This PC again");
        app.handle_action(&ctx, Action::Forward);
        land(&mut app);
        assert_eq!(app.tab.view.current, dir.join("sub"));
        app.handle_action(&ctx, Action::Back);
        land(&mut app);
        app.handle_action(&ctx, Action::Back);
        assert_eq!(app.tab.lister.last_requested(), Some(dir), "and back again, to where it was opened from");
    }

    #[test]
    fn this_pc_is_the_first_place_on_windows_only() {
        let (_, app, _) = app_in("app-this-pc-place");
        assert_eq!(app.places.first().is_some_and(|p| places::is_this_pc(&p.path)), cfg!(windows));
    }

    #[test]
    fn the_drive_rows_come_from_the_disks_the_side_panel_lists() {
        let (_, mut app, dir) = app_in("app-this-pc-rows");
        let usage = Usage { total: 2 * 1024 * 1024 * 1024, free: 1024 * 1024 * 1024 };
        app.disks = (Some(Instant::now()), vec![(Mount { label: "Windows (C:)".into(), path: dir.clone() }, Some(usage))]);
        app.rebuild_drives();
        let rows: Vec<(&str, &Path, &str, &str)> = app.drives.iter().map(|r| (r.name.as_str(), r.path.as_path(), r.free.as_str(), r.percent.as_str())).collect();
        assert_eq!(rows, [("Windows (C:)", dir.as_path(), "1.0 GB", "50%")]);
    }

    fn disk(label: &str, path: &str, kind: DriveKind, fs: &str, usage: Option<Usage>) -> Disk {
        Disk { mount: Mount { label: label.into(), path: PathBuf::from(path) }, usage, info: DriveInfo { kind, fs: fs.into() } }
    }

    const GB: u64 = 1024 * 1024 * 1024;

    /// The frames' calls to `refresh_disks_with` (never reading again) until the read in flight lands.
    fn land_disks(ctx: &egui::Context, app: &mut LumenApp) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            app.refresh_disks_with(ctx, || panic!("no new read was due"));
            if !app.disk_reader.is_running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the disks were not read in time");
    }

    #[test]
    fn a_slow_disk_read_never_blocks_the_frame_and_the_old_rows_stay_until_the_new_ones_land() {
        let (ctx, mut app, _) = app_in("app-disks-slow");
        app.apply_disks(vec![disk("Windows (C:)", "C:\\", DriveKind::Local, "NTFS", Some(Usage { total: 2 * GB, free: GB }))]);
        app.disks.0 = None;
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let asked = Instant::now();
        app.refresh_disks_with(&ctx, move || {
            let _ = blocked.recv_timeout(Duration::from_secs(3));
            vec![
                disk("Windows (C:)", "C:\\", DriveKind::Local, "NTFS", Some(Usage { total: 2 * GB, free: GB })),
                disk("Ubuntu (\\\\wsl$) (Z:)", "Z:\\", DriveKind::Network, "9P", Some(Usage { total: 1000 * GB, free: 990 * GB })),
            ]
        });
        assert!(asked.elapsed() < Duration::from_secs(1), "the frame did not wait for the read");
        assert!(app.disk_reader.is_running());
        assert_eq!(drive_names(&app), ["Windows (C:)"], "the rows of the last read are still shown");
        assert_eq!(app.disks.1.len(), 1, "and the side panel's");
        app.refresh_disks_with(&ctx, || panic!("one read at a time"));
        assert_eq!(drive_names(&app), ["Windows (C:)"]);

        release.send(()).unwrap();
        land_disks(&ctx, &mut app);
        assert_eq!(drive_names(&app), ["Windows (C:)", "Ubuntu (\\\\wsl$) (Z:)"], "the late rows replace them");
        assert_eq!(app.disks.1.len(), 2, "in the side panel too");
        assert!(app.disks.0.is_some());
    }

    #[test]
    fn only_one_disk_read_is_in_flight_however_often_the_clock_asks() {
        let (ctx, mut app, _) = app_in("app-disks-one");
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let count = runs.clone();
        app.refresh_disks_with(&ctx, move || {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = blocked.recv_timeout(Duration::from_secs(3));
            vec![disk("Windows (C:)", "C:\\", DriveKind::Local, "NTFS", None)]
        });
        for _ in 0..3 {
            app.disks.0 = None;
            let count = runs.clone();
            app.refresh_disks_with(&ctx, move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Vec::new()
            });
        }
        release.send(()).unwrap();
        land_disks(&ctx, &mut app);
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(drive_names(&app), ["Windows (C:)"], "the one read is what lands");
    }

    #[test]
    fn network_drives_show_in_this_pc_with_their_type_file_system_and_space_or_without_numbers() {
        let (_, mut app, _) = app_in("app-disks-network");
        app.apply_disks(vec![
            disk("Windows (C:)", "C:\\", DriveKind::Local, "NTFS", Some(Usage { total: 2 * GB, free: GB })),
            disk("Ubuntu (\\\\wsl$) (Z:)", "Z:\\", DriveKind::Network, "9P", Some(Usage { total: 1000 * GB, free: 500 * GB })),
            disk("Network Drive (Y:)", "Y:\\", DriveKind::Network, "", None),
        ]);
        let rows: Vec<(&str, &str, &str, &str, Option<f32>)> =
            app.drives.iter().map(|r| (r.name.as_str(), r.kind, r.fs.as_str(), r.free.as_str(), r.used)).collect();
        assert_eq!(
            rows,
            [
                ("Windows (C:)", "Local Disk", "NTFS", "1.0 GB", Some(0.5)),
                ("Ubuntu (\\\\wsl$) (Z:)", "Network Drive", "9P", "500.0 GB", Some(0.5)),
                ("Network Drive (Y:)", "Network Drive", "", "", None),
            ]
        );
        assert_eq!((app.drives[2].size.as_str(), app.drives[2].percent.as_str()), ("", ""), "no numbers, no bar");
        let storage: Vec<&str> = app.disks.1.iter().map(|(mount, _)| mount.label.as_str()).collect();
        assert_eq!(storage, ["Windows (C:)", "Ubuntu (\\\\wsl$) (Z:)", "Network Drive (Y:)"], "and in the side panel's Storage");
    }

    #[test]
    fn the_recycle_bin_does_not_read_a_network_drive() {
        let (_, mut app, _) = app_in("app-disks-bin-roots");
        app.apply_disks(vec![
            disk("Windows (C:)", "C:\\", DriveKind::Local, "NTFS", None),
            disk("Ubuntu (\\\\wsl$) (Z:)", "Z:\\", DriveKind::Network, "9P", None),
            disk("USB Drive (F:)", "F:\\", DriveKind::Removable, "FAT32", None),
        ]);
        assert_eq!(app.recycle_roots(), [PathBuf::from("C:\\"), PathBuf::from("F:\\")]);
    }

    #[test]
    fn the_arrows_pick_a_drive_and_enter_opens_it() {
        let (ctx, mut app, dir) = app_on_this_pc("app-this-pc-keys");
        assert_eq!(app.tab.drive, None);
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.drive, Some(dir.join("sub")), "the first one");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.drive, Some(dir.join("drive2")));
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.drive, Some(dir.join("drive2")), "stops at the last");
        app.handle_action(&ctx, Action::Move(-1));
        assert_eq!(app.tab.drive, Some(dir.join("sub")));
        app.handle_action(&ctx, Action::Last);
        assert_eq!(app.tab.drive, Some(dir.join("drive2")));
        app.handle_action(&ctx, Action::First);
        assert_eq!(app.tab.drive, Some(dir.join("sub")));
        app.handle_action(&ctx, Action::Activate);
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub")), "Enter opens the drive");
        land(&mut app);
        assert!(!app.on_this_pc());
    }

    #[test]
    fn enter_with_no_drive_picked_opens_nothing() {
        let (ctx, mut app, _) = app_on_this_pc("app-this-pc-enter");
        let asked = app.tab.lister.latest_id();
        app.handle_action(&ctx, Action::Activate);
        assert_eq!(app.tab.lister.latest_id(), asked);
    }

    #[test]
    fn the_views_events_select_open_and_ask_for_the_drives_menu() {
        let (ctx, mut app, dir) = app_on_this_pc("app-this-pc-events");
        app.handle_drives(&ctx, DrivesEvent::Select(1));
        assert_eq!(app.tab.drive, Some(dir.join("drive2")));
        app.handle_drives(&ctx, DrivesEvent::Menu(0));
        assert_eq!(app.tab.drive, Some(dir.join("sub")), "a right click picks the row too");
        wait_for_menu(&mut app);
        assert!(menu_item(&app, &Command::OpenPlace(dir.join("sub"))).is_some(), "the side panel's menu for the drive: {:?}", menu_labels(&app));
        assert!(menu_item(&app, &Command::Cut).is_none() && menu_item(&app, &Command::Paste).is_none());
        app.context_menu = None;
        #[cfg(windows)]
        {
            app.shell_menu = None;
        }
        app.handle_drives(&ctx, DrivesEvent::Open(1));
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("drive2")));
    }

    #[test]
    fn a_menu_key_on_a_picked_drive_opens_its_menu() {
        let (ctx, mut app, dir) = app_on_this_pc("app-this-pc-menu-key");
        app.handle_action(&ctx, Action::Menu);
        assert!(app.context_menu.is_none() && !app.menu_asked(), "no drive picked: no menu");
        app.handle_action(&ctx, Action::Move(1));
        app.handle_action(&ctx, Action::Menu);
        wait_for_menu(&mut app);
        assert!(menu_item(&app, &Command::OpenPlace(dir.join("sub"))).is_some(), "{:?}", menu_labels(&app));
    }

    /// Whether a menu is open or on its way.
    impl LumenApp {
        fn menu_asked(&self) -> bool {
            #[cfg(windows)]
            return self.menu_pending.is_some();
            #[cfg(not(windows))]
            false
        }
    }

    #[test]
    fn no_folder_action_runs_on_this_pc_because_it_is_no_folder() {
        let (ctx, mut app, dir) = app_in("app-this-pc-guards");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Copy);
        assert!(app.clipboard.is_some());
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::this_pc()));
        land(&mut app);
        let pc = places::this_pc();
        let before = names_in(&dir);
        let output_before = ctx.output(|o| o.commands.len());
        let asked = app.tab.lister.latest_id();
        app.notice = None;

        app.handle_action(&ctx, Action::Paste);
        app.run_command(&ctx, Command::Paste);
        app.run_command(&ctx, Command::PasteInto);
        app.paste_into(&ctx, pc.clone());
        app.handle_action(&ctx, Action::NewFolder);
        app.run_command(&ctx, Command::NewFile(None));
        app.drop_into(&ctx, vec![dir.join("a.txt")], pc.clone(), Mode::Copy);
        app.drop_into(&ctx, vec![dir.join("a.txt")], pc.clone(), Mode::Move);
        assert!(app.batch.is_none() && app.job.is_none() && app.dialog.is_none(), "nothing started");
        assert!(app.tab.rename_after_load.is_none() && app.rename.is_none());
        assert!(app.tab.view.error.is_none() && app.notice.is_none(), "not even tried: {:?} {:?}", app.tab.view.error, app.notice);
        assert_eq!(names_in(&dir), before, "nothing created or moved");
        assert!(!Path::new("This PC").exists(), "and nothing next to the program");

        app.handle_action(&ctx, Action::Menu);
        assert!(app.context_menu.is_none() && !app.menu_asked(), "no menu for the empty space");
        app.run_command(&ctx, Command::OpenTerminal);
        app.handle_action(&ctx, Action::CopyPath);
        assert_eq!(ctx.output(|o| o.commands.len()), output_before, "no path copied");
        app.handle_action(&ctx, Action::ToggleBookmark);
        app.handle_toolbar(&ctx, ToolbarEvent::ToggleBookmark);
        assert!(app.bookmarks.paths().is_empty(), "This PC is not bookmarked");
        app.handle_sidebar(&ctx, SidebarEvent::Menu(pc.clone()));
        assert!(app.context_menu.is_none() && !app.menu_asked(), "nor given a place menu");

        app.handle_action(&ctx, Action::Up);
        app.handle_toolbar(&ctx, ToolbarEvent::Up);
        app.handle_action(&ctx, Action::GoParent);
        assert_eq!(app.tab.lister.latest_id(), asked, "there is nothing above it");
        assert_eq!(app.tab.view.current, pc);
    }

    #[test]
    fn refresh_on_this_pc_reads_the_drives_again_instead_of_a_folder() {
        let (ctx, mut app, _) = app_on_this_pc("app-this-pc-refresh");
        app.disks.0 = Some(Instant::now());
        let asked = app.tab.lister.latest_id();
        app.run_command(&ctx, Command::Refresh);
        assert_eq!(app.disks.0, None, "read again on the next frame");
        assert_eq!(app.tab.lister.latest_id(), asked, "no folder listed");
    }

    /// `app_on_this_pc` with the fonts loaded, which fitting a column measures with.
    fn app_on_this_pc_with_fonts(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let (ctx, app, dir) = app_on_this_pc(name);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        (ctx, app, dir)
    }

    fn drive_names(app: &LumenApp) -> Vec<&str> {
        app.drives.iter().map(|drive| drive.name.as_str()).collect()
    }

    #[test]
    fn a_header_click_in_this_pc_sorts_the_drives_and_the_same_one_again_reverses_them() {
        let (ctx, mut app, _) = app_on_this_pc("app-this-pc-sort");
        assert_eq!(app.drive_sort, None);
        assert_eq!(drive_names(&app), ["Windows (C:)", "Unidad de USB (D:)"], "letter order to begin with");
        app.handle_drives(&ctx, DrivesEvent::Sort(DriveColumn::Name));
        assert_eq!(drive_names(&app), ["Unidad de USB (D:)", "Windows (C:)"]);
        assert_eq!(app.drive_sort, Some(DriveSort { column: DriveColumn::Name, descending: false }));
        app.handle_drives(&ctx, DrivesEvent::Sort(DriveColumn::Name));
        assert_eq!(drive_names(&app), ["Windows (C:)", "Unidad de USB (D:)"], "reversed");
        assert_eq!(app.drive_sort.map(|s| s.descending), Some(true));
        assert!(app.tab.drive.is_none(), "sorting picks nothing");
        app.handle_drives(&ctx, DrivesEvent::Sort(DriveColumn::Type));
        assert_eq!(app.drive_sort, Some(DriveSort { column: DriveColumn::Type, descending: false }), "another column starts ascending");
        assert_eq!(app.tab.view.sort, Sort::default(), "the folders' sort is another thing");
        assert_eq!(app.tab.view.current, places::this_pc());
    }

    #[test]
    fn sorting_keeps_the_picked_drive_and_the_arrows_walk_the_new_order() {
        let (ctx, mut app, dir) = app_on_this_pc("app-this-pc-sort-keys");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.drive, Some(dir.join("sub")), "the first, Windows");
        app.handle_drives(&ctx, DrivesEvent::Sort(DriveColumn::Name));
        assert_eq!(app.tab.drive, Some(dir.join("sub")), "still Windows, now second");
        app.handle_action(&ctx, Action::Move(-1));
        assert_eq!(app.tab.drive, Some(dir.join("drive2")), "the arrow goes up the sorted list to the USB");
        app.handle_action(&ctx, Action::First);
        assert_eq!(app.tab.drive, Some(dir.join("drive2")));
    }

    #[test]
    fn the_drives_stay_sorted_when_they_are_read_again() {
        let (_, mut app, dir) = app_in("app-this-pc-sort-reread");
        let usage = Usage { total: 2 * 1024 * 1024 * 1024, free: 1024 * 1024 * 1024 };
        let mount = |label: &str, name: &str| (Mount { label: label.into(), path: dir.join(name) }, Some(usage));
        app.disks = (Some(Instant::now()), vec![mount("Windows (C:)", "c"), mount("Datos (E:)", "e")]);
        app.drive_sort = Some(DriveSort { column: DriveColumn::Name, descending: false });
        app.rebuild_drives();
        assert_eq!(drive_names(&app), ["Datos (E:)", "Windows (C:)"]);
        app.drive_sort = None;
        app.rebuild_drives();
        assert_eq!(drive_names(&app), ["Windows (C:)", "Datos (E:)"], "unsorted: as the disks come");
    }

    #[test]
    fn a_dragged_border_in_this_pc_resizes_the_drive_columns_only() {
        let (ctx, mut app, _) = app_on_this_pc("app-this-pc-resize");
        let widths = [300.0, 90.0, 100.0, 110.0, 110.0, 150.0];
        app.handle_drives(&ctx, DrivesEvent::Resized(widths));
        assert_eq!(app.drive_columns, widths);
        assert_eq!(app.columns, ui::file_list::DEFAULT_WIDTHS, "not the list's");
    }

    #[test]
    fn a_double_clicked_border_in_this_pc_fits_that_drive_column_only() {
        let (ctx, mut app, _) = app_on_this_pc_with_fonts("app-this-pc-border-fit");
        app.drive_columns = [100.0; 6];
        app.handle_drives(&ctx, DrivesEvent::Fit(0));
        assert_eq!(app.drive_columns[0], drives_view::fit_width(&ctx, &app.drives, 0));
        assert_eq!(app.drive_columns[1..], [100.0; 5], "only that column");
        assert_eq!(app.columns, ui::file_list::DEFAULT_WIDTHS, "the list's widths are not touched");
    }

    #[test]
    fn size_column_to_fit_in_this_pc_fits_that_drive_column_and_leaves_the_lists() {
        let (ctx, mut app, _) = app_on_this_pc_with_fonts("app-this-pc-fit-one");
        app.drive_columns = [100.0; 6];
        app.run_command(&ctx, Command::FitColumn(0));
        assert_eq!(app.drive_columns[0], drives_view::fit_width(&ctx, &app.drives, 0));
        assert_eq!(app.drive_columns[1..], [100.0; 5], "only that column");
        assert_eq!(app.columns, ui::file_list::DEFAULT_WIDTHS, "the list's widths are not touched");
    }

    #[test]
    fn ctrl_shift_f_in_this_pc_fits_every_drive_column_inside_the_visible_width() {
        let (ctx, mut app, _) = app_on_this_pc_with_fonts("app-this-pc-fit-all");
        ui::file_list::remember_width(&ctx, 1000.0);
        let room = ui::file_list::visible_width(&ctx, &ui::file_list::ALL_SHOWN).unwrap();
        app.drive_columns = [100.0; 6];
        app.handle_action(&ctx, Action::FitColumns);
        let fit = |column| drives_view::fit_width(&ctx, &app.drives, column);
        let total: f32 = app.drive_columns.iter().sum();
        assert!((total - room).abs() < 0.5, "the room is shared, no more: {total} in {room}");
        assert!((0..6).all(|c| app.drive_columns[c] >= fit(c)), "none is cut: {:?}", app.drive_columns);
        assert!(app.drive_columns[0] < room / 2.0, "and Name does not take it all: {:?}", app.drive_columns);
        assert_eq!(app.columns, ui::file_list::DEFAULT_WIDTHS, "the list's widths are not touched");
        // Too many columns for the room: the rule of the list squeezes them to fit.
        app.drives[0].name = "x".repeat(300);
        ui::file_list::remember_width(&ctx, 600.0);
        app.run_command(&ctx, Command::FitAllColumns);
        let room = ui::file_list::visible_width(&ctx, &ui::file_list::ALL_SHOWN).unwrap();
        let total: f32 = app.drive_columns.iter().sum();
        assert!((total - room).abs() < 0.5, "inside the width: {total} in {room}");
    }

    #[test]
    fn this_pc_fits_its_columns_when_first_shown_and_again_when_the_window_resizes() {
        let (ctx, mut app, _) = app_on_this_pc_with_fonts("app-this-pc-auto-fit");
        app.fit_this_pc(&ctx, 1080.0);
        let room = ui::file_list::room(&ctx, 1080.0, &ui::file_list::ALL_SHOWN);
        assert!((app.drive_columns.iter().sum::<f32>() - room).abs() < 0.5, "{:?} in {room}", app.drive_columns);
        assert!(app.drive_columns[0] < room / 2.0, "Name does not take the room: {:?}", app.drive_columns);
        assert_ne!(app.drive_columns, drives_view::DEFAULT_WIDTHS);
        app.fit_this_pc(&ctx, 900.0);
        let room = ui::file_list::room(&ctx, 900.0, &ui::file_list::ALL_SHOWN);
        assert!((app.drive_columns.iter().sum::<f32>() - room).abs() < 0.5, "resized: {:?} in {room}", app.drive_columns);
        assert_eq!(app.columns, ui::file_list::DEFAULT_WIDTHS, "the list's widths are not touched");
        assert_eq!(app.recycle_columns, recycle_view::DEFAULT_WIDTHS, "nor the Recycle Bin's");
    }

    #[test]
    fn this_pc_keeps_widths_set_by_hand_until_fit_all_columns_gives_it_back_its_own() {
        let (ctx, mut app, _) = app_on_this_pc_with_fonts("app-this-pc-auto-fit-by-hand");
        app.fit_this_pc(&ctx, 1080.0);
        let widths = [300.0, 90.0, 100.0, 110.0, 110.0, 150.0];
        app.handle_drives(&ctx, DrivesEvent::Resized(widths));
        app.fit_this_pc(&ctx, 700.0);
        assert_eq!(app.drive_columns, widths, "a dragged border is not overwritten when the window resizes");
        app.run_command(&ctx, Command::FitAllColumns);
        app.fit_this_pc(&ctx, 700.0);
        let room = ui::file_list::room(&ctx, 700.0, &ui::file_list::ALL_SHOWN);
        assert!((app.drive_columns.iter().sum::<f32>() - room).abs() < 0.5, "Fit All Columns: {:?}", app.drive_columns);
        // Fitting a single column is by hand too.
        app.run_command(&ctx, Command::FitColumn(1));
        let kept = app.drive_columns;
        app.fit_this_pc(&ctx, 500.0);
        assert_eq!(app.drive_columns, kept);
    }

    // ---- The Recycle Bin view ----

    /// The folder every bin of the fixture holds its SID's entries in: the real
    /// SID where there is one (Windows), else any (all are scanned).
    fn bin_sid() -> String {
        recycle_bin::current_sid().unwrap_or_else(|| "S-1-5-21-1-2-3-1001".into())
    }

    /// Fake drive `root` has `original` in its Recycle Bin, deleted at `unix`.
    fn recycled_on(root: &Path, id: &str, original: &Path, folder: bool, unix: u64) {
        let bin = root.join("$Recycle.Bin").join(bin_sid());
        recycle_bin::tests::recycle_at(&bin, id, &original.display().to_string(), 7, folder, unix);
    }

    /// Reads the bins the way the frames do, until the read is done.
    fn load_recycle(ctx: &egui::Context, app: &mut LumenApp) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            app.poll_recycle(ctx);
            if app.recycle_shown && !app.recycle_stale && !app.recycle_reader.is_running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the bins were not read in time");
    }

    /// An app on the Recycle Bin, over two fake drives (`C` and `D` of the fixture)
    /// holding `Work` (a folder, newest), `pic.png` and `Docs/notes.txt` (oldest).
    fn app_on_recycle_bin(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let (ctx, mut app, dir) = app_in(name);
        let (c, d) = (dir.join("C"), dir.join("D"));
        recycled_on(&c, "AAA111", &dir.join("Docs").join("notes.txt"), false, 1_600_000_000);
        recycled_on(&c, "BBB222", &dir.join("Work"), true, 1_700_000_000);
        recycled_on(&d, "CCC333", &dir.join("pic.png"), false, 1_650_000_000);
        let mount = |label: &str, path: &Path| (Mount { label: label.into(), path: path.to_path_buf() }, None);
        app.disks = (Some(Instant::now()), vec![mount("Windows (C:)", &c), mount("Data (D:)", &d)]);
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::recycle_bin()));
        land(&mut app);
        load_recycle(&ctx, &mut app);
        (ctx, app, dir)
    }

    #[test]
    fn the_recycle_bin_waits_for_the_first_disk_read_instead_of_reading_no_drives() {
        let (ctx, mut app, dir) = app_in("app-bin-waits-for-disks");
        let c = dir.join("C");
        recycled_on(&c, "GGG777", &dir.join("late.txt"), false, 1_800_000_000);
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let root = c.to_string_lossy().into_owned();
        app.disk_reader.start(
            || {},
            move || {
                let _ = blocked.recv_timeout(Duration::from_secs(10));
                vec![disk("Windows (C:)", &root, DriveKind::Local, "NTFS", None)]
            },
        );
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::recycle_bin()));
        land(&mut app);
        app.poll_recycle(&ctx);
        assert!(app.recycle_stale && !app.recycle_reader.is_running(), "no drive is known yet");
        release.send(()).unwrap();
        load_recycle(&ctx, &mut app);
        assert_eq!(recycled_names(&app), ["late.txt"]);
    }

    fn recycled_names(app: &LumenApp) -> Vec<&str> {
        app.recycled.iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn the_recycle_bin_opens_in_the_tab_is_titled_so_and_back_and_forward_come_to_it() {
        let (ctx, mut app, dir) = app_in("app-bin-history");
        let bin = places::recycle_bin();
        assert!(!app.on_recycle_bin());
        app.handle_sidebar(&ctx, SidebarEvent::Open(bin.clone()));
        assert_eq!(app.tab.lister.last_requested(), Some(bin.clone()), "in the current tab");
        land(&mut app);
        assert_eq!((app.tabs.len(), app.tab.view.current.clone()), (1, bin.clone()));
        assert!(app.on_recycle_bin() && !app.on_this_pc());
        assert_eq!(app.tab_labels(), ["Recycle Bin"]);
        assert_eq!(app.tab.history.back_target(), Some(&dir));
        assert!(app.tab.view.error.is_none() && app.tab.view.entries.is_empty());
        assert!(!app.recents.paths().contains(&bin) && app.visits.list().iter().all(|v| v.path != bin), "not a folder to come back to");
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        land(&mut app);
        assert!(!app.on_recycle_bin());
        app.handle_action(&ctx, Action::Back);
        assert_eq!(app.tab.lister.last_requested(), Some(bin.clone()));
        land(&mut app);
        assert!(app.on_recycle_bin(), "back shows the Recycle Bin again");
        app.handle_action(&ctx, Action::Forward);
        land(&mut app);
        assert_eq!(app.tab.view.current, dir.join("sub"));
    }

    #[test]
    fn the_recycle_bin_is_the_last_place_on_windows_only() {
        let (_, app, _) = app_in("app-bin-place");
        assert_eq!(app.places.last().is_some_and(|p| places::is_recycle_bin(&p.path)), cfg!(windows));
        assert!(app.places.iter().filter(|p| places::is_recycle_bin(&p.path)).count() <= 1);
    }

    #[test]
    fn no_folder_action_runs_in_the_recycle_bin_because_it_is_no_folder() {
        let (ctx, mut app, dir) = app_in("app-bin-guards");
        pick(&mut app, &["a.txt"]);
        app.handle_action(&ctx, Action::Copy);
        assert!(app.clipboard.is_some());
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::recycle_bin()));
        land(&mut app);
        let bin = places::recycle_bin();
        let before = names_in(&dir);
        let output_before = ctx.output(|o| o.commands.len());
        let asked = app.tab.lister.latest_id();
        app.notice = None;

        app.handle_action(&ctx, Action::Paste);
        app.run_command(&ctx, Command::Paste);
        app.run_command(&ctx, Command::PasteInto);
        app.paste_into(&ctx, bin.clone());
        app.handle_action(&ctx, Action::NewFolder);
        app.run_command(&ctx, Command::NewFile(None));
        assert!(app.batch.is_none() && app.job.is_none() && app.dialog.is_none(), "nothing started");
        assert!(app.tab.rename_after_load.is_none() && app.rename.is_none());
        assert!(app.tab.view.error.is_none() && app.notice.is_none(), "not even tried: {:?} {:?}", app.tab.view.error, app.notice);
        assert_eq!(names_in(&dir), before, "nothing created");
        assert!(!Path::new("Recycle Bin").exists(), "and nothing next to the program");

        // A drop is refused with the trash's own message.
        for mode in [Mode::Copy, Mode::Move] {
            app.tab.view.error = None;
            app.drop_into(&ctx, vec![dir.join("a.txt")], bin.clone(), mode);
            assert_eq!(app.tab.view.error.as_deref(), Some("Nothing can be dropped into the trash"));
        }
        assert!(app.batch.is_none() && app.job.is_none() && app.dialog.is_none());
        assert_eq!(names_in(&dir), before, "nothing moved");
        app.tab.view.error = None;

        app.handle_action(&ctx, Action::Menu);
        assert!(app.context_menu.is_none() && !app.menu_asked(), "no menu for the empty space");
        app.run_command(&ctx, Command::OpenTerminal);
        app.handle_action(&ctx, Action::CopyPath);
        assert_eq!(ctx.output(|o| o.commands.len()), output_before, "no path copied");
        app.handle_action(&ctx, Action::ToggleBookmark);
        app.handle_toolbar(&ctx, ToolbarEvent::ToggleBookmark);
        assert!(app.bookmarks.paths().is_empty(), "the Recycle Bin is not bookmarked");
        app.handle_sidebar(&ctx, SidebarEvent::Menu(bin.clone()));
        assert!(app.context_menu.is_none() && !app.menu_asked(), "nor given a place menu");

        app.handle_action(&ctx, Action::Up);
        app.handle_toolbar(&ctx, ToolbarEvent::Up);
        app.handle_action(&ctx, Action::GoParent);
        assert_eq!(app.tab.lister.latest_id(), asked, "there is nothing above it");
        assert_eq!(app.tab.view.current, bin);
    }

    #[test]
    fn the_bins_of_every_drive_are_read_when_shown_newest_first_with_where_they_came_from() {
        let (_, app, dir) = app_on_recycle_bin("app-bin-read");
        assert_eq!(recycled_names(&app), ["Work", "pic.png", "notes.txt"], "the latest deleted first");
        let places: Vec<&str> = app.recycled.iter().map(|item| item.location.as_str()).collect();
        let (here, docs) = (dir.display().to_string(), dir.join("Docs").display().to_string());
        assert_eq!(places, [here.as_str(), here.as_str(), docs.as_str()]);
        assert_eq!(app.recycled.iter().map(|i| i.is_dir).collect::<Vec<_>>(), [true, false, false]);
    }

    #[test]
    fn the_bins_are_read_again_on_refresh_and_when_the_view_is_shown_again_but_not_every_frame() {
        let (ctx, mut app, dir) = app_on_recycle_bin("app-bin-reread");
        recycled_on(&dir.join("D"), "DDD444", &dir.join("late.txt"), false, 1_800_000_000);
        app.poll_recycle(&ctx);
        app.poll_recycle(&ctx);
        assert_eq!(app.recycled.len(), 3, "a frame on its own does not read");
        app.run_command(&ctx, Command::Refresh);
        load_recycle(&ctx, &mut app);
        assert_eq!(recycled_names(&app), ["late.txt", "Work", "pic.png", "notes.txt"], "Refresh reads them again");
        assert!(!app.tab.lister.is_pending(app.tab.shown_id), "and lists no folder");

        recycled_on(&dir.join("C"), "EEE555", &dir.join("later.txt"), false, 1_900_000_000);
        app.handle_toolbar(&ctx, ToolbarEvent::Go(dir.join("sub")));
        land(&mut app);
        app.poll_recycle(&ctx);
        app.handle_action(&ctx, Action::Back);
        land(&mut app);
        load_recycle(&ctx, &mut app);
        assert_eq!(app.recycled.first().map(|i| i.name.as_str()), Some("later.txt"), "shown again: read again");
    }

    #[test]
    fn clicking_the_recycle_bin_place_while_in_it_reads_the_bins_again() {
        let (ctx, mut app, dir) = app_on_recycle_bin("app-bin-reclick");
        recycled_on(&dir.join("C"), "FFF666", &dir.join("again.txt"), false, 1_800_000_000);
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::recycle_bin()));
        land(&mut app);
        load_recycle(&ctx, &mut app);
        assert_eq!(app.recycled.first().map(|i| i.name.as_str()), Some("again.txt"));
    }

    #[test]
    fn a_bin_with_nothing_in_it_reads_as_empty_and_is_not_read_over_and_over() {
        let (ctx, mut app, dir) = app_in("app-bin-empty");
        app.disks = (Some(Instant::now()), vec![(Mount { label: "Windows (C:)".into(), path: dir.join("C") }, None)]);
        app.handle_sidebar(&ctx, SidebarEvent::Open(places::recycle_bin()));
        land(&mut app);
        load_recycle(&ctx, &mut app);
        assert!(app.recycled.is_empty());
        app.poll_recycle(&ctx);
        assert!(!app.recycle_reader.is_running() && !app.recycle_stale);
    }

    #[test]
    fn clicks_arrows_and_select_all_pick_entries_and_escape_drops_the_picks() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-picks");
        let picked = |app: &LumenApp| app.tab.recycle.of(&app.recycled).iter().map(|i| i.name.clone()).collect::<Vec<_>>();
        app.handle_recycle(&ctx, RecycleEvent::Select(1));
        assert_eq!(picked(&app), ["pic.png"]);
        app.handle_recycle(&ctx, RecycleEvent::Toggle(2));
        assert_eq!(picked(&app), ["pic.png", "notes.txt"]);
        app.handle_recycle(&ctx, RecycleEvent::Extend(0));
        assert_eq!(picked(&app), ["Work", "pic.png", "notes.txt"], "from where Ctrl+click left the anchor to the click");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!((picked(&app), app.recycle_scroll), (["pic.png".to_owned()].to_vec(), Some(1)), "the arrow moves and brings the row into view");
        app.handle_action(&ctx, Action::Extend(1));
        assert_eq!(picked(&app), ["pic.png", "notes.txt"]);
        app.handle_action(&ctx, Action::First);
        assert_eq!(picked(&app), ["Work"]);
        app.handle_action(&ctx, Action::Last);
        assert_eq!(picked(&app), ["notes.txt"]);
        app.handle_action(&ctx, Action::SelectAll);
        assert_eq!(picked(&app).len(), 3);
        app.handle_action(&ctx, Action::Cancel);
        assert!(picked(&app).is_empty(), "Escape drops them");
    }

    #[test]
    fn enter_opens_nothing_in_the_recycle_bin() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-enter");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        let asked = app.tab.lister.latest_id();
        app.handle_action(&ctx, Action::Activate);
        assert_eq!(app.tab.lister.latest_id(), asked);
        assert!(app.tab.view.error.is_none() && app.dialog.is_none() && app.rename.is_none());
    }

    #[test]
    fn restore_puts_the_picked_entries_back_and_says_how_many() {
        let (ctx, mut app, dir) = app_on_recycle_bin("app-bin-restore");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        app.handle_recycle(&ctx, RecycleEvent::Toggle(2));
        app.run_command(&ctx, Command::Restore);
        assert!(dir.join("Work").join("inner.txt").exists(), "the folder with what it held");
        assert_eq!(std::fs::read_to_string(dir.join("Docs").join("notes.txt")).unwrap(), "content", "its folder made again");
        assert_eq!(app.notice.as_deref(), Some("2 items restored"));
        assert_eq!(app.tab.view.error, None);
        assert_eq!(recycled_names(&app), ["pic.png"], "gone from the view at once");
        assert_eq!(app.tab.recycle.count(), 0);
        load_recycle(&ctx, &mut app);
        assert_eq!(recycled_names(&app), ["pic.png"], "and from the bin");
        assert!(!dir.join("pic.png").exists(), "what was not picked stays");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        app.run_command(&ctx, Command::Restore);
        assert_eq!(app.notice.as_deref(), Some("1 item restored"));
        assert!(dir.join("pic.png").exists() && app.recycled.is_empty());
        app.notice = None;
        app.run_command(&ctx, Command::Restore);
        assert_eq!(app.notice, None, "nothing picked: nothing said");
    }

    #[test]
    fn restoring_onto_a_taken_name_keeps_both_and_a_failure_shows_in_the_red_line() {
        let (ctx, mut app, dir) = app_on_recycle_bin("app-bin-restore-taken");
        std::fs::write(dir.join("pic.png"), "newer").unwrap();
        app.handle_recycle(&ctx, RecycleEvent::Select(1));
        app.run_command(&ctx, Command::Restore);
        assert_eq!(std::fs::read_to_string(dir.join("pic.png")).unwrap(), "newer", "never overwritten");
        assert_eq!(std::fs::read_to_string(dir.join("pic (restored).png")).unwrap(), "content");
        assert_eq!(app.notice.as_deref(), Some("1 item restored"));

        // Docs is a file now: notes.txt has no folder to go back to.
        std::fs::write(dir.join("Docs"), "a file").unwrap();
        app.notice = None;
        let row = app.recycled.iter().position(|i| i.name == "notes.txt").unwrap();
        app.handle_recycle(&ctx, RecycleEvent::Select(row));
        app.run_command(&ctx, Command::Restore);
        assert!(app.tab.view.error.as_deref().is_some_and(|e| e.starts_with("Cannot restore notes.txt: ")), "{:?}", app.tab.view.error);
        assert_eq!(app.notice, None, "nothing was restored");
        assert_eq!(recycled_names(&app).len(), 2, "it stays listed");
        assert_eq!(app.tab.recycle.count(), 1, "and picked");
    }

    #[test]
    fn delete_in_the_recycle_bin_asks_first_and_then_deletes_for_good() {
        let (ctx, mut app, dir) = app_on_recycle_bin("app-bin-purge");
        app.handle_action(&ctx, Action::Trash);
        assert!(app.dialog.is_none(), "nothing picked: nothing to ask");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        app.handle_action(&ctx, Action::Trash);
        let dialog = app.dialog.as_ref().expect("Delete asks, as Shift+Delete does elsewhere");
        assert_eq!(dialog.title(), "Delete Permanently");
        assert_eq!(dialog.message(), "Permanently delete “Work”? This cannot be undone.");
        assert!(dialog.danger() && dialog.subject().is_some_and(|s| s.name == "Work"));
        assert_eq!((dialog.yes_label(), dialog.no_label(), dialog.hint()), ("Delete", "Cancel", "Enter to delete · Esc to cancel"));
        let bin_files = |app: &LumenApp| app.recycled.iter().map(|i| (i.data.exists(), i.info.exists())).collect::<Vec<_>>();
        app.answer_dialog(&ctx, Confirm::No);
        assert_eq!(bin_files(&app), [(true, true); 3], "No: nothing deleted");
        let work = app.recycled[0].clone();

        app.handle_action(&ctx, Action::DeleteForever);
        assert!(matches!(app.dialog, Some(Dialog::Purge { .. })), "Shift+Delete asks the same");
        app.answer_dialog(&ctx, Confirm::Yes);
        assert!(!work.data.exists() && !work.info.exists(), "the content and the record are gone");
        assert_eq!(recycled_names(&app), ["pic.png", "notes.txt"]);
        assert!(!dir.join("Work").exists(), "deleted, not restored");
        assert_eq!(app.tab.view.error, None);

        app.handle_action(&ctx, Action::SelectAll);
        app.run_command(&ctx, Command::DeleteForever);
        assert_eq!(app.dialog.as_ref().map(|d| d.message()), Some("Permanently delete 2 items? This cannot be undone.".to_owned()));
        app.answer_dialog(&ctx, Confirm::Yes);
        load_recycle(&ctx, &mut app);
        assert!(app.recycled.is_empty() && read_bins(&dir).is_empty(), "every bin is empty now");
    }

    /// What the fixture's two bins hold.
    fn read_bins(dir: &Path) -> Vec<Recycled> {
        recycle_bin::read_all(&[dir.join("C"), dir.join("D")], Some(&bin_sid()))
    }

    #[test]
    fn a_right_click_picks_its_row_unless_picked_and_offers_restore_and_delete_permanently() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-menu");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        app.handle_recycle(&ctx, RecycleEvent::Menu(2));
        assert_eq!(menu_labels(&app), ["Restore", "Delete Permanently"]);
        assert_eq!(app.tab.recycle.of(&app.recycled).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["notes.txt"], "a row outside the picks becomes the pick");
        assert!(menu_item(&app, &Command::Restore).is_some() && menu_item(&app, &Command::DeleteForever).is_some());
        app.context_menu = None;
        app.handle_recycle(&ctx, RecycleEvent::Extend(0));
        app.handle_recycle(&ctx, RecycleEvent::Menu(1));
        assert_eq!(app.tab.recycle.count(), 3, "a row among the picks keeps them all");
        assert!(app.context_menu.is_some());
    }

    #[test]
    fn the_menu_key_opens_the_menu_for_the_picks_and_nothing_without_any() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-menu-key");
        app.handle_action(&ctx, Action::Menu);
        assert!(app.context_menu.is_none(), "nothing picked: no menu");
        app.handle_recycle(&ctx, RecycleEvent::Select(1));
        app.handle_action(&ctx, Action::Menu);
        assert_eq!(menu_labels(&app), ["Restore", "Delete Permanently"]);
    }

    #[test]
    fn a_header_click_sorts_the_entries_and_again_reverses_them_and_the_borders_resize() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-sort");
        app.handle_recycle(&ctx, RecycleEvent::Sort(recycle_view::RecycleColumn::Name));
        assert_eq!(recycled_names(&app), ["notes.txt", "pic.png", "Work"]);
        app.handle_recycle(&ctx, RecycleEvent::Sort(recycle_view::RecycleColumn::Name));
        assert_eq!(recycled_names(&app), ["Work", "pic.png", "notes.txt"]);
        app.handle_recycle(&ctx, RecycleEvent::Sort(recycle_view::RecycleColumn::Deleted));
        assert_eq!(recycled_names(&app), ["notes.txt", "pic.png", "Work"], "another column starts ascending");
        app.handle_recycle(&ctx, RecycleEvent::Select(0));
        app.handle_recycle(&ctx, RecycleEvent::Sort(recycle_view::RecycleColumn::Name));
        assert_eq!(app.tab.recycle.of(&app.recycled).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["notes.txt"], "the pick follows its entry");
        app.handle_recycle(&ctx, RecycleEvent::Resized([1.0, 2.0, 3.0, 4.0, 0.0, 0.0]));
        assert_eq!(app.recycle_columns, [1.0, 2.0, 3.0, 4.0, 0.0, 0.0]);
        assert_eq!((app.columns, app.drive_columns), (ui::file_list::DEFAULT_WIDTHS, drives_view::DEFAULT_WIDTHS), "the other views keep theirs");
        // A double clicked border fits its column to the entries, the others stay.
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        app.handle_recycle(&ctx, RecycleEvent::Fit(1));
        assert_eq!(app.recycle_columns[1], recycle_view::fit_width(&ctx, &app.recycled, 1));
        assert_ne!(app.recycle_columns[1], 2.0, "fitted to the entries");
        assert_eq!([app.recycle_columns[0], app.recycle_columns[2], app.recycle_columns[3]], [1.0, 3.0, 4.0], "only that column");
        assert_eq!((app.columns, app.drive_columns), (ui::file_list::DEFAULT_WIDTHS, drives_view::DEFAULT_WIDTHS), "the other views keep theirs");
        // A fresh read keeps the order the headers gave.
        app.run_command(&ctx, Command::Refresh);
        load_recycle(&ctx, &mut app);
        assert_eq!(recycled_names(&app), ["notes.txt", "pic.png", "Work"]);
    }

    #[test]
    fn the_header_menu_and_the_fit_commands_in_the_recycle_bin_fit_its_columns_only() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-fit");
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        app.handle_recycle(&ctx, RecycleEvent::HeaderMenu(1));
        assert_eq!(menu_labels(&app), ["Size Column To Fit", "Size All Columns To Fit"]);
        assert!(!app.context_on_entry);
        assert_eq!(app.tab.recycle.count(), 0, "no entry is picked by it");
        app.recycle_columns = [100.0; 6];
        app.columns = [100.0; 6];
        app.drive_columns = [100.0; 6];
        app.run_command(&ctx, Command::FitColumn(1));
        assert_eq!(app.recycle_columns[1], recycle_view::fit_width(&ctx, &app.recycled, 1));
        app.run_command(&ctx, Command::FitAllColumns);
        let fits: Vec<f32> = (0..recycle_view::COLUMNS.len()).map(|column| recycle_view::fit_width(&ctx, &app.recycled, column)).collect();
        assert_eq!(app.recycle_columns[..5], fits[..], "every column of the view, to its content");
        assert_eq!((app.columns, app.drive_columns), ([100.0; 6], [100.0; 6]), "the lists and the drives keep theirs");
    }

    #[test]
    fn the_recycle_bin_fits_its_columns_when_first_shown_and_when_the_window_resizes_unless_dragged() {
        let (ctx, mut app, _) = app_on_recycle_bin("app-bin-auto-fit");
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let shown = recycle_view::SHOWN;
        let fitted = |app: &LumenApp, width| {
            let room = ui::file_list::room(&ctx, width, &shown);
            let total: f32 = app.recycle_columns[..5].iter().sum();
            assert!((total - room).abs() < 0.5, "{:?} in {room}", app.recycle_columns);
        };
        app.fit_recycle_bin(&ctx, 1080.0);
        fitted(&app, 1080.0);
        assert_eq!(app.recycle_columns[5], 0.0, "the unused column stays out");
        app.fit_recycle_bin(&ctx, 760.0);
        fitted(&app, 760.0);
        assert_eq!((app.columns, app.drive_columns), (ui::file_list::DEFAULT_WIDTHS, drives_view::DEFAULT_WIDTHS), "the other views keep theirs");
        let widths = [200.0, 220.0, 100.0, 150.0, 90.0, 0.0];
        app.handle_recycle(&ctx, RecycleEvent::Resized(widths));
        app.fit_recycle_bin(&ctx, 1200.0);
        assert_eq!(app.recycle_columns, widths, "a dragged border is not overwritten");
        app.run_command(&ctx, Command::FitAllColumns);
        app.fit_recycle_bin(&ctx, 1200.0);
        fitted(&app, 1200.0);
    }

    #[test]
    fn the_header_menu_in_this_pc_offers_the_two_fits_and_they_work() {
        let (ctx, mut app, _) = app_on_this_pc_with_fonts("app-this-pc-header-menu");
        app.handle_drives(&ctx, DrivesEvent::HeaderMenu(2));
        assert_eq!(menu_labels(&app), ["Size Column To Fit", "Size All Columns To Fit"]);
        assert_eq!(app.tab.drive, None, "no drive is picked by it");
        assert!(!app.context_on_entry);
        let fit = menu_item(&app, &Command::FitColumn(2)).map(|i| i.label.clone());
        assert_eq!(fit.as_deref(), Some("Size Column To Fit"), "for the column clicked");
        app.drive_columns = [100.0; 6];
        app.run_command(&ctx, Command::FitColumn(2));
        assert_eq!(app.drive_columns[2], drives_view::fit_width(&ctx, &app.drives, 2));
    }

    #[test]
    fn on_a_folder_the_fits_still_fit_the_lists_columns_and_not_the_drives() {
        let (ctx, mut app, _) = app_in("app-fit-not-on-this-pc");
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        app.drive_columns = [100.0; 6];
        app.columns = [100.0; 6];
        app.run_command(&ctx, Command::FitColumn(0));
        assert_eq!(app.columns[0], ui::file_list::fit_width(&ctx, &app.tab.view, 0));
        assert_eq!(app.drive_columns, [100.0; 6]);
        app.handle_action(&ctx, Action::FitColumns);
        assert_eq!(app.drive_columns, [100.0; 6], "Ctrl+Shift+F neither");
    }

    #[test]
    fn a_notice_goes_away_after_a_few_seconds_and_a_new_one_starts_its_own_time() {
        let (ctx, mut app, _) = app_in("app-notice-expiry");
        let start = Instant::now();
        app.handle_action(&ctx, Action::ToggleBookmark);
        assert_eq!(app.expire_notice(start), Some(NOTICE_TIME), "a new notice asks to be looked at again once its time is up");
        assert_eq!(app.expire_notice(start + NOTICE_TIME / 2), Some(NOTICE_TIME / 2));
        assert_eq!(app.notice.as_deref(), Some("Added “app-notice-expiry” to bookmarks"), "still shown before its time");
        app.handle_action(&ctx, Action::ToggleBookmark);
        let later = start + NOTICE_TIME;
        assert_eq!(app.expire_notice(later), Some(NOTICE_TIME), "a different notice gets its full time");
        assert_eq!(app.notice.as_deref(), Some("Removed “app-notice-expiry” from bookmarks"));
        assert_eq!(app.expire_notice(later + NOTICE_TIME), None);
        assert_eq!(app.notice, None, "gone once its time is up");
        assert_eq!(app.expire_notice(later + NOTICE_TIME * 2), None, "nothing to clear");
    }

    #[test]
    fn an_error_goes_away_after_a_few_seconds_and_a_new_one_starts_its_own_time() {
        let (ctx, mut app, dir) = app_in("app-error-expiry");
        let start = Instant::now();
        app.trash = Some(dir.join("sub"));
        app.drop_into(&ctx, vec![dir.join("a.txt")], dir.join("sub"), Mode::Move);
        let told = Some("Nothing can be dropped into the trash");
        assert_eq!(app.expire_notice(start), Some(NOTICE_TIME), "an error asks to be looked at again once its time is up");
        assert_eq!(app.expire_notice(start + NOTICE_TIME / 2), Some(NOTICE_TIME / 2));
        assert_eq!(app.tab.view.error.as_deref(), told, "still shown before its time");
        app.tab.view.error = Some("Another operation is still running".into());
        let later = start + NOTICE_TIME;
        assert_eq!(app.expire_notice(later), Some(NOTICE_TIME), "a different error gets its full time");
        assert_eq!(app.expire_notice(later + NOTICE_TIME), None);
        assert_eq!(app.tab.view.error, None, "gone once its time is up");
        assert_eq!(names_in(&dir), ["a.txt", "b.txt", "sub"]);
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
        let (path, select) = ui::path_popup::typed_target("sub/..", &dir, None);
        app.handle_popup(&ctx, PopupEvent::Go { path, select, new_tab: false });
        assert_eq!(app.tab.lister.last_requested(), Some(dir.clone()));
    }

    #[test]
    fn ctrl_l_and_the_crumb_bar_open_the_path_box_and_ctrl_enter_uses_a_new_tab() {
        let (ctx, mut app, dir) = app_in("app-path-box");
        app.handle_action(&ctx, Action::FocusPathBar);
        assert!(app.path_popup.is_open());
        let mut app2 = app_in("app-path-box-2").1;
        app2.handle_toolbar(&ctx, ToolbarEvent::EditPath);
        assert!(app2.path_popup.is_open());
        app.handle_popup(&ctx, PopupEvent::Go { path: dir.join("sub"), select: None, new_tab: true });
        assert_eq!(app.tabs.len(), 2, "a new tab");
        assert_eq!(app.tab.lister.last_requested(), Some(dir.join("sub")), "showing the folder");
        app.handle_popup(&ctx, PopupEvent::Go { path: dir.clone(), select: Some("a.txt".into()), new_tab: false });
        assert_eq!(app.tab.select_after_load.as_deref(), Some("a.txt"), "a file opens its folder, selected");
    }

    #[test]
    fn enter_on_the_focused_path_bar_opens_the_path_box_and_leaves_it_open() {
        let (ctx, mut app, dir) = app_in("app-path-bar-enter");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let enter = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        for (n, events) in [vec![enter(true)], vec![enter(false)], vec![]].into_iter().enumerate() {
            let input = egui::RawInput { events, screen_rect: Some(screen), ..egui::RawInput::default() };
            ctx.run_ui(input, |root| {
                // The toolbar answers the Enter first, as the window draws it above the box.
                if n == 0 {
                    app.handle_toolbar(root.ctx(), ToolbarEvent::EditPath);
                }
                egui::CentralPanel::default().show(root, |ui| {
                    if let Some(event) = app.path_popup.show(ui.ctx(), ui.max_rect(), &dir, None, false, Vec::new) {
                        app.handle_popup(ui.ctx(), event);
                    }
                });
            })
            .drop_without_applying_deltas();
            assert!(app.path_popup.is_open(), "still open after frame {n}");
        }
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
        assert_eq!(app.tab.view.totals.get(&dir.join("sub")), Some(&expected));
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
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Copy });
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
        assert_eq!(app.tab.view.totals.get(&dir.join("sub")), Some(&partial));
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

    /// An entry below `/x`, at `rel` ("src/util/a.txt").
    fn nested(rel: &str, is_dir: bool) -> Entry {
        let path = PathBuf::from("/x").join(rel);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        Entry { hidden: name.starts_with('.'), size: (!is_dir).then_some(1), modified: None, name, path, is_dir }
    }

    /// `loaded_view` flattened, with what its walk found below `/x`.
    fn flat_view() -> View {
        let mut v = loaded_view();
        v.set_flat(vec![PathBuf::from("/x")]);
        let found = vec![
            nested("src", true),
            nested("src/main.rs", false),
            nested("src/util", true),
            nested("src/util/a.txt", false),
            nested("src/.cache", false),
            nested("a.txt", false),
        ];
        v.apply_flat(&[PathBuf::from("/x")], found);
        v
    }

    fn row_paths(v: &View) -> Vec<String> {
        (0..v.rows.len())
            .map(|row| v.entry(v.rows[row]).path.strip_prefix("/x").unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn a_flattened_view_lists_what_the_subfolders_hold_sorted_with_the_rest() {
        let v = flat_view();
        // Folders first, then by name; each entry once, hidden ones left out.
        assert_eq!(row_paths(&v), ["src", "src/util", "a.txt", "src/util/a.txt", "b.txt", "src/main.rs"]);
        assert!(v.is_flat());
    }

    #[test]
    fn a_flattened_view_counts_every_row_it_lists() {
        let v = flat_view();
        assert_eq!(v.rows.len(), 6);
        assert_eq!(v.folder_count(), 2);
        assert_eq!(v.hidden_count(), 3, ".git and .env above, .cache inside src");
    }

    #[test]
    fn what_a_hidden_folder_holds_is_hidden_with_it() {
        let mut v = loaded_view();
        v.set_flat(vec![PathBuf::from("/x")]);
        v.apply_flat(&[PathBuf::from("/x")], vec![nested(".git", true), nested(".git/objects", true), nested(".git/objects/pack", false), nested("src/main.rs", false)]);
        assert_eq!(row_paths(&v), ["src", "a.txt", "b.txt", "src/main.rs"], "nothing from .git while hidden files are");
        assert_eq!(v.hidden_count(), 4, ".git, .env, and the two inside .git");
        v.toggle_hidden();
        assert!(row_paths(&v).contains(&".git/objects/pack".to_owned()), "Ctrl+H shows them");
    }

    #[test]
    fn a_nested_row_names_its_folder_relative_to_the_one_shown() {
        let v = flat_view();
        let parent = |rel: &str| v.relative_parent(&nested(rel, false));
        assert_eq!(parent("a.txt"), None, "the folder's own entries name none");
        assert_eq!(parent("src/main.rs").as_deref(), Some("src"));
        assert_eq!(parent("src/util/a.txt").as_deref(), Some("src > util"));
        assert_eq!(loaded_view().relative_parent(&nested("src/main.rs", false)), None, "only while flattened");
    }

    #[test]
    fn a_walk_of_other_roots_is_ignored() {
        let mut v = loaded_view();
        v.set_flat(vec![PathBuf::from("/x")]);
        v.apply_flat(&[PathBuf::from("/elsewhere")], vec![nested("src/main.rs", false)]);
        assert_eq!(row_paths(&v), ["src", "a.txt", "b.txt"]);
    }

    #[test]
    fn turning_the_flattened_view_off_lists_the_folder_alone_again() {
        let mut v = flat_view();
        v.clear_flat();
        assert!(!v.is_flat());
        assert_eq!(row_paths(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.hidden_count(), 2);
    }

    #[test]
    fn another_folder_is_not_flattened_but_a_reload_stays_so() {
        let mut v = flat_view();
        v.apply_refresh(ok_listing("/x", vec![entry("src", true), entry("a.txt", false), entry("b.txt", false)]));
        assert!(v.is_flat());
        assert_eq!(v.rows.len(), 6);
        v.apply_listing(ok_listing("/y", vec![entry("c.txt", false)]), None);
        assert!(!v.is_flat());
        assert_eq!(v.rows.len(), 1);
    }

    #[test]
    fn select_all_in_a_flattened_view_picks_every_row() {
        let mut v = flat_view();
        v.select_all();
        assert_eq!(v.picked_count(), 6);
    }

    #[test]
    fn the_cursor_stays_on_its_entry_when_the_walk_lands() {
        let mut v = loaded_view();
        let b = v.row_of_path(Path::new("/x/b.txt")).unwrap();
        v.select_row(b);
        v.set_flat(vec![PathBuf::from("/x")]);
        v.apply_flat(&[PathBuf::from("/x")], vec![nested("src/main.rs", false), nested("src/a.txt", false)]);
        assert_eq!(v.selected_entry().unwrap().path, Path::new("/x/b.txt"));
    }

    fn row_names(v: &View) -> Vec<&str> {
        v.rows.iter().map(|&i| v.entry(i).name.as_str()).collect()
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

    fn selection_names(v: &View) -> Vec<&str> {
        v.selection().iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn without_picks_the_selection_is_the_cursor_row() {
        let mut v = loaded_view();
        v.select_row(1);
        assert_eq!(selection_names(&v), ["a.txt"]);
        assert_eq!(v.picked_count(), 0);
        assert!(!v.is_picked(1), "the cursor alone is not a pick");
    }

    #[test]
    fn select_all_picks_every_row_shown_and_only_those() {
        let mut v = loaded_view();
        v.select_all();
        assert_eq!(selection_names(&v), ["src", "a.txt", "b.txt"], "hidden entries stay out");
        assert_eq!(v.picked_count(), 3);
        v.drop_picks();
        for c in ".txt".chars() {
            v.push_filter(c);
        }
        v.select_all();
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"], "only what the filter left");
    }

    #[test]
    fn ctrl_click_adds_and_removes_rows_starting_from_the_cursor() {
        let mut v = loaded_view();
        v.select_row(0);
        v.toggle_pick(2);
        assert_eq!(selection_names(&v), ["src", "b.txt"]);
        assert_eq!(v.selected, Some(2), "the cursor follows the click");
        v.toggle_pick(0);
        assert_eq!(selection_names(&v), ["b.txt"]);
        assert_eq!(v.picked_count(), 1);
    }

    #[test]
    fn shift_selects_the_range_from_the_anchor_both_ways() {
        let mut v = loaded_view();
        v.select_row(1);
        v.extend_by(1);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"]);
        assert_eq!(v.selected, Some(2));
        v.extend_by(isize::MIN);
        assert_eq!(selection_names(&v), ["src", "a.txt"], "the anchor stays on a.txt");
        assert_eq!(v.selected, Some(0));
        v.extend_to(2);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"]);
        v.extend_by(isize::MAX);
        assert_eq!(v.selected, Some(2), "clamped to the last row");
    }

    #[test]
    fn a_band_picks_every_row_it_touches_both_ways() {
        let mut v = loaded_view();
        v.select_row(0);
        // Row i spans [i, i + 1) in row units.
        v.select_band(1.5, 2.2, false);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"]);
        assert_eq!(v.selected, Some(2), "the cursor follows the pointer's end");
        v.select_band(2.9, 0.1, false);
        assert_eq!(selection_names(&v), ["src", "a.txt", "b.txt"], "dragged upwards");
        assert_eq!(v.selected, Some(0));
        v.select_band(1.2, 1.4, false);
        assert_eq!(selection_names(&v), ["a.txt"], "a short drag inside one row picks it");
        assert_eq!(v.picked_count(), 1);
        v.select_band(-3.0, 9.0, false);
        assert_eq!(selection_names(&v), ["src", "a.txt", "b.txt"], "clamped to the rows shown");
        assert_eq!(v.selected, Some(2));
        v.extend_to(1);
        assert_eq!(selection_names(&v), ["src", "a.txt"], "Shift+click ranges from where the band started");
    }

    fn five_files() -> View {
        let mut v = View::default();
        let names = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"];
        v.apply_listing(ok_listing("/x", names.iter().map(|n| entry(n, false)).collect()), None);
        v
    }

    #[test]
    fn a_shift_band_adds_its_rows_to_the_selection() {
        let mut v = five_files();
        v.select_band(0.5, 1.5, false);
        v.end_band();
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"]);
        v.select_band(3.5, 3.6, true);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt", "d.txt"], "the first frame of the band");
        v.select_band(3.5, 4.5, true);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt", "d.txt", "e.txt"], "growing it keeps the earlier rows");
        v.select_band(3.5, 3.6, true);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt", "d.txt"], "shrinking it lets go of what it no longer touches");
        v.select_band(3.5, 7.0, true);
        v.select_band(6.0, 7.0, true);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"], "over no row it keeps what was there before it");
        v.end_band();
        v.select_band(2.5, 2.6, false);
        assert_eq!(selection_names(&v), ["c.txt"], "without Shift a band still replaces the selection");
    }

    #[test]
    fn a_shift_band_adds_to_the_cursor_row_when_nothing_is_picked() {
        let mut v = five_files();
        v.select_row(0);
        v.select_band(2.5, 2.6, true);
        assert_eq!(selection_names(&v), ["a.txt", "c.txt"]);
        v.end_band();
        v.unselect();
        v.select_band(4.5, 4.6, true);
        assert_eq!(selection_names(&v), ["e.txt"], "an unselected cursor is not added");
    }

    #[test]
    fn a_shift_band_over_cards_adds_them_to_the_selection() {
        let mut v = five_files();
        v.select_cells(&[0], Some(0), false);
        v.end_band();
        v.select_cells(&[3, 4], Some(4), true);
        assert_eq!(selection_names(&v), ["a.txt", "d.txt", "e.txt"]);
        assert_eq!(v.selected, Some(4));
        v.select_cells(&[], None, true);
        assert_eq!(selection_names(&v), ["a.txt"], "over no card it keeps what was there before it");
    }

    #[test]
    fn a_band_over_no_row_drops_the_picks_and_keeps_the_cursor() {
        let mut v = loaded_view();
        v.select_all();
        v.select_row(1);
        v.select_all();
        v.select_band(3.5, 5.0, false);
        assert_eq!(v.picked_count(), 0, "below the rows");
        assert_eq!(selection_names(&v), ["a.txt"], "like a click below the rows");
        v.select_all();
        v.select_band(-2.0, -0.5, false);
        assert_eq!(v.picked_count(), 0, "above the rows");
        let mut empty = View::default();
        empty.select_band(0.0, 2.0, false);
        assert_eq!(empty.selected, None);
    }

    #[test]
    fn a_plain_move_or_click_goes_back_to_one_row() {
        let mut v = loaded_view();
        v.select_all();
        v.move_selection(Action::Move(1));
        assert_eq!(selection_names(&v), ["a.txt"]);
        v.select_all();
        v.select_row(2);
        assert_eq!(selection_names(&v), ["b.txt"]);
        v.extend_by(-1);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"], "a new range anchors on the clicked row");
    }

    #[test]
    fn picks_survive_a_refresh_and_reset_in_another_folder() {
        let mut v = loaded_view();
        v.select_row(1);
        v.toggle_pick(2);
        let again = vec![entry("src", true), entry("a.txt", false), entry("b.txt", false), entry("c.txt", false)];
        v.apply_refresh(ok_listing("/x", again));
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"]);
        v.apply_listing(ok_listing("/y", vec![entry("a.txt", false), entry("b.txt", false)]), None);
        assert_eq!(v.picked_count(), 0, "same names, another folder");
        assert_eq!(selection_names(&v), ["a.txt"]);
    }

    #[test]
    fn picked_size_adds_files_and_counted_folders_and_tells_the_uncounted_ones() {
        let mut v = View::default();
        let mut big = entry("big.iso", false);
        big.size = Some(3 << 20);
        let entries = vec![entry("src", true), entry("docs", true), big, entry("a.txt", false), entry("b.txt", false)];
        v.apply_listing(ok_listing("/x", entries), None);
        v.set_totals(p("/x/src"), Totals { folders: 1, files: 2, bytes: 1000, ..Totals::default() });
        let row = |v: &View, name: &str| (0..v.rows.len()).find(|&r| v.entries[v.rows[r]].name == name).unwrap();
        assert_eq!(v.picked_size(), PickedSize { bytes: 0, uncounted: 0 }, "nothing picked");
        for name in ["src", "docs", "big.iso", "a.txt"] {
            let r = row(&v, name);
            v.toggle_pick(r);
        }
        // toggle_pick keeps the cursor row picked too: drop whatever else got in.
        let extra: Vec<usize> = (0..v.rows.len())
            .filter(|&r| v.is_picked(r) && !["src", "docs", "big.iso", "a.txt"].contains(&v.entries[v.rows[r]].name.as_str()))
            .collect();
        for r in extra {
            v.toggle_pick(r);
        }
        assert_eq!(v.picked_count(), 4);
        assert_eq!(v.picked_size(), PickedSize { bytes: 1000 + (3 << 20) + 1, uncounted: 1 });
    }

    #[test]
    fn picks_survive_sort_and_hidden_toggle() {
        let mut v = loaded_view();
        v.select_row(1);
        v.toggle_pick(2);
        v.sort_by(SortColumn::Name);
        let mut got = selection_names(&v);
        got.sort();
        assert_eq!(got, ["a.txt", "b.txt"]);
        v.toggle_hidden();
        v.toggle_hidden();
        assert_eq!(v.picked_count(), 2);
    }

    #[test]
    fn drop_picks_reports_whether_there_were_any() {
        let mut v = loaded_view();
        assert!(!v.drop_picks());
        v.select_all();
        assert!(v.drop_picks());
        assert_eq!(selection_names(&v), ["src"], "back to the cursor row");
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
        v.totals.insert(p("/x/a_dir"), totals(500, 1, 9));
        v.totals.insert(p("/x/b_dir"), totals(20, 3, 2));
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
        v.set_totals(p("/x/b_dir"), totals(100, 0, 1));
        assert_eq!(row_names(&v)[..2], ["b_dir", "a_dir"]);
        assert_eq!(v.selected_entry().unwrap().name, "a_dir", "selection follows the entry");
        assert_eq!(v.totals.get(&p("/x/b_dir")), Some(&totals(100, 0, 1)));
    }

    #[test]
    fn totals_survive_a_refresh_but_not_another_folder() {
        let mut v = sortable_view();
        v.set_totals(p("/x/a_dir"), totals(1, 0, 1));
        let again = v.entries.clone();
        v.apply_refresh(ok_listing("/s", again.clone()));
        assert!(v.totals.contains_key(&p("/x/a_dir")));
        v.apply_listing(ok_listing("/s", again.clone()), None);
        assert!(v.totals.contains_key(&p("/x/a_dir")), "reloading the same folder keeps them");
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
    fn an_exact_name_comes_first_and_partial_matches_keep_the_sort() {
        let mut v = View { sort: Sort { column: SortColumn::Name, descending: true }, ..View::default() };
        let entries = vec![entry("f6", true), entry("f6-config", true), entry("verify-f6", true), entry("f6.png", false)];
        v.apply_listing(ok_listing("/p", entries), None);
        type_str(&mut v, "F6");
        assert_eq!(row_names(&v), ["f6", "verify-f6", "f6-config", "f6.png"]);
        assert_eq!(v.selected_entry().unwrap().name, "f6", "the exact match is selected");
        type_str(&mut v, " -");
        assert_eq!(row_names(&v), ["verify-f6", "f6-config"], "no exact match: just the sort");
        v.clear_filter();
        assert_eq!(row_names(&v), ["verify-f6", "f6-config", "f6", "f6.png"], "no filter: just the sort");
    }

    #[test]
    fn a_space_after_other_characters_starts_another_word() {
        let mut v = View::default();
        let entries = vec![entry("my doc.txt", false), entry("mydoc.txt", false), entry("yd.txt", false)];
        v.apply_listing(ok_listing("/s", entries), None);
        type_str(&mut v, "my d");
        assert_eq!(v.filter(), "my d", "the space is kept in what was typed");
        assert_eq!(row_names(&v), ["my doc.txt", "mydoc.txt"]);
    }

    #[test]
    fn the_filter_finds_every_word_in_any_order_and_keeps_the_sort() {
        let mut v = View::default();
        let entries = vec![
            entry("Slides de contenido", true),
            entry("contenido", true),
            entry("slides.pdf", false),
            entry("Contenido de slides.txt", false),
        ];
        v.apply_listing(ok_listing("/s", entries), None);
        type_str(&mut v, "slides contenido");
        assert_eq!(row_names(&v), ["Slides de contenido", "Contenido de slides.txt"]);
        assert_eq!(v.selected_entry().unwrap().name, "Slides de contenido");
        v.clear_filter();
        type_str(&mut v, "CONTENIDO  slides");
        assert_eq!(row_names(&v), ["Slides de contenido", "Contenido de slides.txt"], "any order, case, extra spaces");
        v.clear_filter();
        // "Slides de contenido" holds "de" twice (sli-de-s, de), not three times.
        type_str(&mut v, "de de de");
        assert_eq!(row_names(&v), Vec::<&str>::new(), "a repeated word needs its own letters");
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
        assert_eq!(v.path_after_removal().as_deref(), Some(Path::new("/x/b.txt")));
        v.selected = Some(0);
        assert_eq!(v.path_after_removal().as_deref(), Some(Path::new("/x/a.txt")));
    }

    #[test]
    fn after_removing_several_rows_the_first_row_left_below_them_is_selected() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.select_row(0);
        v.toggle_pick(1);
        assert_eq!(v.path_after_removal().as_deref(), Some(Path::new("/x/b.txt")));
        v.select_row(1);
        v.extend_by(1);
        assert_eq!(v.path_after_removal().as_deref(), Some(Path::new("/x/src")), "none below: the nearest above");
        v.select_all();
        assert_eq!(v.path_after_removal(), None);
    }

    #[test]
    fn after_removing_the_last_row_the_row_above_is_selected() {
        let mut v = loaded_view();
        v.selected = Some(2);
        assert_eq!(v.path_after_removal().as_deref(), Some(Path::new("/x/a.txt")));
        let mut single = View::default();
        single.apply_listing(ok_listing("/one", vec![entry("only", false)]), None);
        assert_eq!(single.path_after_removal(), None);
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

    /// `due` with these drive letters (`None`: not looked up); counts the lookups.
    fn due_with(clock: &mut DiskClock, read: Option<Instant>, now: Instant, mask: Option<u32>, lookups: &std::cell::Cell<u32>) -> bool {
        clock.due(read, now, || {
            lookups.set(lookups.get() + 1);
            mask
        })
    }

    #[test]
    fn the_disks_are_read_at_once_the_first_time_and_then_every_five_seconds() {
        let (mut clock, t0, lookups) = (DiskClock::default(), Instant::now(), std::cell::Cell::new(0));
        let at = |secs: f64| t0 + Duration::from_secs_f64(secs);
        assert!(due_with(&mut clock, None, t0, Some(0b11), &lookups), "never read");
        assert!(!due_with(&mut clock, Some(t0), at(1.0), Some(0b11), &lookups));
        assert!(!due_with(&mut clock, Some(t0), at(4.9), Some(0b11), &lookups), "same drives, free space can wait");
        assert!(due_with(&mut clock, Some(t0), at(5.0), Some(0b11), &lookups), "free space moves");
    }

    #[test]
    fn a_drive_plugged_in_or_ejected_is_read_without_waiting_for_the_five_seconds() {
        let (mut clock, t0, lookups) = (DiskClock::default(), Instant::now(), std::cell::Cell::new(0));
        let at = |secs: f64| t0 + Duration::from_secs_f64(secs);
        assert!(due_with(&mut clock, None, t0, Some(0b0100), &lookups));
        assert!(!due_with(&mut clock, Some(t0), at(0.6), Some(0b0100), &lookups), "nothing changed");
        assert!(due_with(&mut clock, Some(t0), at(1.2), Some(0b1100), &lookups), "D: plugged in");
        assert!(!due_with(&mut clock, Some(at(1.2)), at(1.8), Some(0b1100), &lookups), "and noticed once");
        assert!(due_with(&mut clock, Some(at(1.2)), at(2.4), Some(0b0100), &lookups), "D: ejected");
    }

    #[test]
    fn the_first_look_at_the_drive_letters_is_no_change() {
        let (mut clock, t0, lookups) = (DiskClock::default(), Instant::now(), std::cell::Cell::new(0));
        // Read a moment ago by someone else (Refresh): the first mask seen is only remembered.
        assert!(!due_with(&mut clock, Some(t0), t0, Some(0b11), &lookups));
        assert!(due_with(&mut clock, Some(t0), t0 + Duration::from_millis(600), Some(0b111), &lookups));
    }

    #[test]
    fn the_drive_letters_are_looked_at_twice_a_second_not_every_frame() {
        let (mut clock, t0, lookups) = (DiskClock::default(), Instant::now(), std::cell::Cell::new(0));
        for ms in [0, 16, 100, 499] {
            due_with(&mut clock, Some(t0), t0 + Duration::from_millis(ms), Some(0b11), &lookups);
        }
        assert_eq!(lookups.get(), 1, "once in the first half second");
        due_with(&mut clock, Some(t0), t0 + Duration::from_millis(500), Some(0b11), &lookups);
        assert_eq!(lookups.get(), 2);
    }

    #[test]
    fn without_drive_letters_only_the_five_seconds_count() {
        let (mut clock, t0, lookups) = (DiskClock::default(), Instant::now(), std::cell::Cell::new(0));
        assert!(!due_with(&mut clock, Some(t0), t0 + Duration::from_secs(1), None, &lookups));
        assert!(!due_with(&mut clock, Some(t0), t0 + Duration::from_secs(4), None, &lookups));
        assert!(due_with(&mut clock, Some(t0), t0 + Duration::from_secs(5), None, &lookups));
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

    /// What the folder `parent` holds, as the reader of an unfolded folder delivers it.
    fn inside(parent: &str, items: &[(&str, bool)]) -> Listing {
        let mut entries: Vec<Entry> = items
            .iter()
            .map(|&(name, is_dir)| Entry { path: p(parent).join(name), ..entry(name, is_dir) })
            .collect();
        fs::lister::sort_entries(&mut entries);
        ok_listing(parent, entries)
    }

    /// `/x` (src, a.txt, b.txt shown) with `src` unfolded and read: deep/, main.rs, .hidden.
    fn unfolded_view() -> View {
        let mut v = loaded_view();
        v.toggle_open(0);
        v.apply_kids(inside("/x/src", &[("main.rs", false), ("deep", true), (".hidden", false)]));
        v
    }

    #[test]
    fn removing_an_unfolded_child_folder_lands_after_what_it_held() {
        let mut v = unfolded_view(); // src, deep, main.rs, a.txt, b.txt
        v.toggle_open(1);
        v.apply_kids(inside("/x/src/deep", &[("c1", false)]));
        v.select_row(1);
        assert_eq!(v.path_after_removal(), Some(p("/x/src/main.rs")), "not deep/c1, which goes with it");
    }

    #[test]
    fn a_band_over_cards_picks_those_cards_and_over_none_drops_the_picks() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.select_cells(&[0, 2, 9], Some(2), false);
        assert_eq!(v.picked_count(), 2, "a cell past the rows is ignored");
        assert!(v.is_picked(0) && v.is_picked(2) && !v.is_picked(1));
        assert_eq!(v.selected, Some(2));
        v.select_cells(&[], None, false);
        assert_eq!(v.picked_count(), 0, "a band over no card drops the picks");
        assert_eq!(v.selected, Some(2), "the cursor stays");
    }

    #[test]
    fn ctrl_a_in_the_tree_picks_what_the_cursors_folder_holds() {
        let mut v = unfolded_view(); // src, deep, main.rs, a.txt, b.txt
        v.select_row(2); // src/main.rs
        v.select_all();
        let picked: Vec<usize> = (0..v.rows.len()).filter(|&r| v.is_picked(r)).collect();
        assert_eq!(picked, [1, 2], "src's deep and main.rs, nothing of the top level");
        v.select_row(3); // a.txt, top level
        v.select_all();
        let picked: Vec<usize> = (0..v.rows.len()).filter(|&r| v.is_picked(r)).collect();
        assert_eq!(picked, [0, 3, 4], "the top level, not the unfolded children");
        let mut flat = loaded_view();
        flat.select_all();
        assert_eq!(flat.picked_count(), flat.rows.len(), "without unfolded folders: every row, as before");
    }

    #[test]
    fn a_click_on_the_empty_space_unselects_but_keeps_the_cursor_and_an_arrow_selects_again() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.select_row(1);
        v.toggle_pick(2);
        v.unselect();
        assert_eq!(v.picked_count(), 0);
        assert!(v.selection().is_empty() && v.selected_entry().is_none(), "nothing is selected");
        assert_eq!(v.selected, Some(2), "the cursor stays where it was, outlined");
        assert!(!v.is_highlighted(2) && v.is_outlined(2));
        v.move_selection(Action::Move(1));
        assert_eq!(v.selected_entry().map(|e| e.name.as_str()), Some("b.txt"), "Down from the last row keeps it, selected again");
        assert!(v.is_highlighted(2) && !v.is_outlined(2));
        v.unselect();
        v.select_row(0);
        assert!(v.is_highlighted(0), "a click selects as usual");
    }

    #[test]
    fn a_ctrl_click_after_unselecting_picks_only_the_clicked_row() {
        let mut v = loaded_view(); // src, a.txt, b.txt
        v.select_row(1);
        v.unselect();
        v.toggle_pick(2);
        let paths: Vec<&Path> = v.selection().iter().map(|e| e.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/x/b.txt")], "not the outlined a.txt, which nobody picked");
        assert!(!v.is_highlighted(1));
    }

    #[test]
    fn with_nothing_selected_delete_rename_and_enter_do_nothing() {
        let (ctx, mut app, dir) = app_in("app-unselected");
        app.tab.view.select_row(1);
        app.tab.view.unselect();
        for action in [Action::Trash, Action::DeleteForever, Action::Rename, Action::Copy] {
            app.handle_action(&ctx, action);
        }
        assert!(app.dialog.is_none() && app.rename.is_none() && app.clipboard.is_none());
        app.handle_action(&ctx, Action::Activate);
        assert_eq!(app.tab.view.current, dir, "Enter enters nothing");
    }

    #[test]
    fn escape_with_nothing_else_to_undo_unselects_and_keeps_the_cursor() {
        let (ctx, mut app, dir) = app_in("app-escape-unselect");
        app.tab.view.select_row(1);
        let cursor = app.tab.view.selected;
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.tab.view.selected_entry().is_none() && app.tab.view.selection().is_empty());
        assert!(app.tab.view.is_outlined(1), "the cursor keeps its place, outlined");
        app.handle_action(&ctx, Action::Trash);
        assert!(app.dialog.is_none(), "Delete acts on nothing");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(app.tab.view.selected, cursor.map(|row| row + 1), "an arrow selects again from the cursor");
        assert!(app.tab.view.selected_entry().is_some());
        assert_eq!(app.tab.view.current, dir);
    }

    #[test]
    fn escape_drops_a_filter_picks_a_copy_and_a_cut_before_it_unselects() {
        let (ctx, mut app, dir) = app_in("app-escape-unselect-last");
        app.tab.view.select_row(1);
        app.handle_action(&ctx, Action::TypeChar('b'));
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.filter(), "");
        assert!(app.tab.view.selected_entry().is_some(), "clearing the filter keeps the selection");
        app.tab.view.select_row(1);
        app.tab.view.toggle_pick(2);
        app.handle_action(&ctx, Action::Cancel);
        assert_eq!(app.tab.view.picked_count(), 0);
        assert!(app.tab.view.selected_entry().is_some(), "dropping the picks keeps the cursor selected");
        app.clipboard = Some(Clip { paths: vec![dir.join("a.txt")], mode: Mode::Move });
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.clipboard.is_none());
        assert!(app.tab.view.selected_entry().is_some(), "forgetting the cut keeps the selection");
        app.handle_action(&ctx, Action::Cancel);
        assert!(app.tab.view.selected_entry().is_none(), "then Escape unselects");
    }

    fn depths(v: &View) -> Vec<usize> {
        (0..v.rows.len()).map(|row| v.depth(row)).collect()
    }

    #[test]
    fn unfolding_a_folder_shows_what_it_holds_under_it_one_step_deeper() {
        let mut v = loaded_view();
        v.toggle_open(0);
        assert!(v.is_open(0) && v.tree.is_open(Path::new("/x/src")));
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"], "nothing under it while it is read");
        assert_eq!(v.tree.take_wanted(), [p("/x/src")], "and its read is due");
        v.apply_kids(inside("/x/src", &[("main.rs", false), ("deep", true), (".hidden", false)]));
        assert_eq!(row_names(&v), ["src", "deep", "main.rs", "a.txt", "b.txt"], "folders first, hidden left out");
        assert_eq!(depths(&v), [0, 1, 1, 0, 0]);
        assert!(!v.is_open(1), "deep is folded");
    }

    #[test]
    fn folding_a_folder_hides_its_children_and_a_cursor_inside_goes_to_the_folder() {
        let mut v = unfolded_view();
        v.select_row(2);
        assert_eq!(v.selected_entry().unwrap().name, "main.rs");
        v.toggle_open(0);
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.selected_entry().unwrap().name, "src");
        assert!(!v.is_open(0));
        v.select_row(2);
        v.toggle_open(0);
        assert_eq!(v.selected_entry().unwrap().name, "b.txt", "a cursor elsewhere stays on its entry");
    }

    #[test]
    fn unfolding_above_the_cursor_keeps_it_on_its_entry() {
        let mut v = loaded_view();
        v.select_row(2);
        v.toggle_open(0);
        v.apply_kids(inside("/x/src", &[("main.rs", false), ("lib.rs", false)]));
        assert_eq!(v.selected_entry().unwrap().name, "b.txt");
        assert_eq!(v.selected, Some(4));
    }

    #[test]
    fn children_follow_the_sort_and_the_hidden_rule_of_the_top_level() {
        let mut v = loaded_view();
        v.toggle_open(0);
        v.apply_kids(inside("/x/src", &[("lib.rs", false), ("main.rs", false), ("deep", true), (".hidden", false)]));
        assert_eq!(row_names(&v), ["src", "deep", "lib.rs", "main.rs", "a.txt", "b.txt"]);
        v.sort_by(SortColumn::Name);
        assert_eq!(row_names(&v), ["src", "deep", "main.rs", "lib.rs", "b.txt", "a.txt"], "descending inside too, folders first");
        v.toggle_hidden();
        assert_eq!(row_names(&v), ["src", "deep", "main.rs", "lib.rs", ".hidden", ".git", "b.txt", "a.txt", ".env"]);
        assert_eq!(depths(&v), [0, 1, 1, 1, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn the_filter_filters_the_top_rows_and_a_folder_it_hides_hides_its_children() {
        let mut v = unfolded_view();
        for c in "src".chars() {
            v.push_filter(c);
        }
        assert_eq!(row_names(&v), ["src", "deep", "main.rs"], "the children are not filtered");
        v.clear_filter();
        for c in "a.t".chars() {
            v.push_filter(c);
        }
        assert_eq!(row_names(&v), ["a.txt"], "src is filtered out, and what it holds with it");
        v.clear_filter();
        assert_eq!(row_names(&v), ["src", "deep", "main.rs", "a.txt", "b.txt"], "and back when the filter goes");
    }

    #[test]
    fn an_unreadable_folder_shows_unfolded_with_no_children() {
        let mut v = loaded_view();
        v.toggle_open(0);
        let denied = Listing {
            id: 1,
            path: p("/x/src"),
            result: Err(ListError::PermissionDenied(p("/x/src"))),
            elapsed: Duration::ZERO,
        };
        v.apply_kids(denied);
        assert!(v.is_open(0));
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert_eq!(v.error, None, "no error line for a folder only peeked into");
        v.tree_side(1);
        assert_eq!(v.selected, Some(0), "Right has no first child to go to");
    }

    #[test]
    fn a_result_for_a_folder_folded_meanwhile_is_not_shown() {
        let mut v = loaded_view();
        v.toggle_open(0);
        v.toggle_open(0);
        v.apply_kids(inside("/x/src", &[("main.rs", false)]));
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
    }

    #[test]
    fn right_unfolds_then_goes_to_the_first_child_and_left_folds_then_goes_to_the_parent() {
        let mut v = loaded_view();
        v.select_row(0);
        v.tree_side(1);
        assert!(v.is_open(0), "Right on a folded folder unfolds it");
        assert_eq!(v.selected, Some(0), "and stays on it");
        v.apply_kids(inside("/x/src", &[("main.rs", false), ("deep", true)]));
        v.tree_side(1);
        assert_eq!(v.selected_entry().unwrap().name, "deep", "on an unfolded one, Right goes to its first child");
        v.tree_side(1);
        assert!(v.is_open(1), "a folder inside unfolds the same way");
        v.tree_side(-1);
        assert!(!v.is_open(1), "Left on an unfolded folder folds it");
        assert_eq!(v.selected_entry().unwrap().name, "deep");
        v.tree_side(-1);
        assert_eq!(v.selected_entry().unwrap().name, "src", "Left on a child goes to its parent folder");
        v.tree_side(-1);
        assert!(!v.is_open(0));
        v.tree_side(-1);
        assert_eq!(v.selected, Some(0), "a folded folder at the top has nowhere to go");
        v.select_row(1);
        v.tree_side(1);
        v.tree_side(-1);
        assert_eq!(v.selected_entry().unwrap().name, "a.txt", "a file does nothing either way");
    }

    #[test]
    fn two_unfolded_folders_can_hold_the_same_name_and_each_is_picked_by_its_path() {
        let mut v = View::default();
        v.apply_listing(ok_listing("/x", vec![entry("docs", true), entry("src", true), entry("a.txt", false)]), None);
        for row in [1, 0] {
            v.toggle_open(row);
        }
        v.apply_kids(inside("/x/docs", &[("notes.txt", false)]));
        v.apply_kids(inside("/x/src", &[("notes.txt", false)]));
        assert_eq!(row_names(&v), ["docs", "notes.txt", "src", "notes.txt", "a.txt"]);
        v.select_row(1);
        v.toggle_pick(3);
        assert_eq!(v.picked_count(), 2);
        assert!(v.is_picked(1) && v.is_picked(3) && !v.is_picked(2) && !v.is_picked(0));
        let paths: Vec<&Path> = v.selection().iter().map(|e| e.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/x/docs/notes.txt"), Path::new("/x/src/notes.txt")]);
        v.toggle_pick(1);
        assert!(!v.is_picked(1) && v.is_picked(3), "one is let go without the other");
        v.select_row(1);
        v.extend_to(3);
        assert_eq!(v.picked_count(), 3, "a range from the anchor crosses the folders");
        v.select_all();
        assert_eq!(v.picked_count(), 1, "Ctrl+A on src's notes.txt picks what src holds, not the rest");
        assert!(v.is_picked(3));
        v.select_band(1.2, 3.5, false);
        assert_eq!(v.picked_count(), 3, "a band picks the children it touches");
        assert!(v.is_picked(1) && v.is_picked(3) && !v.is_picked(4));
    }

    #[test]
    fn picks_inside_a_folded_folder_are_not_acted_on() {
        let mut v = unfolded_view();
        v.select_row(1);
        v.toggle_pick(2);
        assert_eq!(v.picked_count(), 2);
        v.toggle_open(0);
        assert_eq!(v.picked_count(), 0, "the picked children are hidden");
        assert_eq!(selection_names(&v), ["src"], "the cursor row is the selection again");
    }

    #[test]
    fn a_counted_child_folder_has_its_own_total_and_the_picked_size_adds_it() {
        let mut v = unfolded_view();
        v.set_totals(p("/x/src/deep"), totals(700, 0, 3));
        v.select_row(1);
        assert_eq!(v.picked_size(), PickedSize { bytes: 0, uncounted: 0 }, "nothing picked");
        v.toggle_pick(1);
        v.toggle_pick(2);
        assert_eq!(v.picked_size(), PickedSize { bytes: 700 + 1, uncounted: 0 });
        assert_eq!(v.totals.get(&p("/x/deep")), None, "not under the folder on screen's name");
    }

    #[test]
    fn unfolded_folders_are_forgotten_in_another_folder_and_kept_and_read_again_on_a_reload_or_refresh() {
        let mut v = unfolded_view();
        v.tree.take_wanted();
        v.apply_refresh(ok_listing("/x", vec![entry("src", true), entry("a.txt", false)]));
        assert_eq!(row_names(&v), ["src", "deep", "main.rs", "a.txt"], "still unfolded after a refresh");
        assert_eq!(v.tree.take_wanted(), [p("/x/src")], "and read again");
        v.apply_listing(ok_listing("/x", vec![entry("src", true), entry("a.txt", false)]), None);
        assert!(v.is_open(0), "a reload of the same folder keeps it too");
        assert_eq!(v.tree.take_wanted(), [p("/x/src")]);
        v.apply_listing(ok_listing("/x", vec![entry("a.txt", false)]), None);
        assert!(v.tree.is_empty(), "a folder that is gone is forgotten");
        let mut v = unfolded_view();
        v.apply_listing(ok_listing("/y", vec![entry("src", true)]), None);
        assert!(v.tree.is_empty() && row_names(&v) == ["src"], "another folder starts folded");
        assert!(!v.is_open(0));
    }

    #[test]
    fn a_reload_with_folders_unfolded_keeps_the_cursor_on_its_child() {
        let mut v = unfolded_view();
        v.select_row(2);
        v.apply_listing(ok_listing("/x", vec![entry("src", true), entry("a.txt", false), entry("b.txt", false)]), None);
        assert_eq!(v.selected_entry().unwrap().name, "main.rs");
    }

    #[test]
    fn folding_everything_goes_back_to_the_folders_own_rows() {
        let mut v = unfolded_view();
        v.select_row(2);
        v.fold_all();
        assert_eq!(row_names(&v), ["src", "a.txt", "b.txt"]);
        assert!(v.tree.is_empty());
        assert_eq!(v.depth(1), 0);
    }

    /// `sub` holds deeper/ and inner.txt; the view is Tree: rows sub, a.txt, b.txt.
    fn tree_app(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let (ctx, mut app, dir) = app_in(name);
        std::fs::create_dir(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "i").unwrap();
        app.tab.view_size = ui::view_size::ViewSize::new(10);
        (ctx, app, dir)
    }

    /// Lets the unfolded folders' readers finish, as the next frames would,
    /// until `done` holds.
    fn wait_kids(app: &mut LumenApp, done: impl Fn(&View) -> bool) {
        for _ in 0..500 {
            app.poll_kids();
            if done(&app.tab.view) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the unfolded folder was not read: {:?}", row_names(&app.tab.view));
    }

    /// The Tree view with `sub` unfolded and read: sub, deeper, inner.txt, a.txt, b.txt.
    fn unfolded_app(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let (ctx, mut app, dir) = tree_app(name);
        app.handle_action(&ctx, Action::Side(1));
        wait_kids(&mut app, |v| v.rows.len() == 5);
        (ctx, app, dir)
    }

    #[test]
    fn in_the_tree_view_right_unfolds_a_folder_without_waiting_for_its_read() {
        let (ctx, mut app, _dir) = tree_app("app-tree-unfold");
        app.handle_action(&ctx, Action::Side(1));
        assert!(app.tab.view.is_open(0), "unfolded at once");
        assert_eq!(app.tab.view.rows.len(), 3, "the folder is read on another thread: nothing yet");
        wait_kids(&mut app, |v| v.rows.len() == 5);
        assert_eq!(row_names(&app.tab.view), ["sub", "deeper", "inner.txt", "a.txt", "b.txt"]);
        assert_eq!(depths(&app.tab.view), [0, 1, 1, 0, 0]);
    }

    #[test]
    fn in_the_tree_view_the_arrows_walk_into_and_out_of_the_unfolded_folder() {
        let (ctx, mut app, _dir) = unfolded_app("app-tree-keys");
        let at = |app: &LumenApp| app.tab.view.selected_entry().map(|e| e.name.clone()).unwrap();
        app.handle_action(&ctx, Action::Side(1));
        assert_eq!(at(&app), "deeper", "Right on an unfolded folder: its first child");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(at(&app), "inner.txt", "Down walks through the children");
        app.handle_action(&ctx, Action::Move(1));
        assert_eq!(at(&app), "a.txt", "and out of them");
        app.handle_action(&ctx, Action::Move(-1));
        app.handle_action(&ctx, Action::Side(-1));
        assert_eq!(at(&app), "sub", "Left on a child: its parent folder");
        app.handle_action(&ctx, Action::Side(-1));
        assert_eq!(row_names(&app.tab.view), ["sub", "a.txt", "b.txt"], "Left on an unfolded folder folds it");
        app.handle_action(&ctx, Action::ExtendSide(1));
        assert_eq!(app.tab.view.picked_count(), 0, "Shift+Right does nothing in Tree");
    }

    #[test]
    fn in_details_right_unfolds_nothing() {
        let (ctx, mut app, _dir) = app_in("app-details-no-tree");
        app.handle_action(&ctx, Action::Side(1));
        assert!(app.tab.view.tree.is_empty());
        assert_eq!(app.tab.view.rows.len(), 3);
    }

    #[test]
    fn the_actions_on_a_child_act_on_its_full_path() {
        let (ctx, mut app, dir) = unfolded_app("app-tree-actions");
        let child = dir.join("sub/inner.txt");
        app.tab.view.select_row(2);
        app.handle_action(&ctx, Action::Trash);
        let Some(Dialog::Trash { paths, .. }) = app.dialog.take() else { panic!("asks first") };
        assert_eq!(paths, std::slice::from_ref(&child));
        app.handle_action(&ctx, Action::DeleteForever);
        let Some(Dialog::Delete { paths, .. }) = app.dialog.take() else { panic!("asks first") };
        assert_eq!(paths, std::slice::from_ref(&child));
        app.handle_action(&ctx, Action::Rename);
        assert_eq!(app.rename.take().map(|r| r.path), Some(child.clone()));
        app.handle_action(&ctx, Action::Copy);
        assert_eq!(app.clipboard.as_ref().map(|c| (c.paths.clone(), c.mode)), Some((vec![child.clone()], Mode::Copy)));
        app.handle_action(&ctx, Action::Cut);
        assert_eq!(app.cut_paths(), std::slice::from_ref(&child));
        app.open_entry_menu(1, egui::pos2(10.0, 10.0));
        assert_eq!(selection_names(&app.tab.view), ["deeper"], "a right click on another child selects it");
        app.handle_action(&ctx, Action::TypeChar(' '));
        app.sync_preview(&ctx);
        assert_eq!(app.preview.as_ref().map(|p| p.path.clone()), Some(dir.join("sub/deeper")), "Space previews the child");
    }

    #[test]
    fn a_picked_folder_takes_its_picked_children_with_it_so_each_is_acted_on_once() {
        let (ctx, mut app, dir) = unfolded_app("app-tree-nested-picks");
        app.tab.view.select_row(0);
        app.tab.view.extend_to(4);
        assert_eq!(app.tab.view.picked_count(), 5, "a Shift range over every row");
        let tops = vec![dir.join("sub"), dir.join("a.txt"), dir.join("b.txt")];
        app.handle_action(&ctx, Action::Copy);
        assert_eq!(app.clipboard.as_ref().map(|c| c.paths.clone()), Some(tops.clone()), "sub's children go with sub, not twice");
        app.handle_action(&ctx, Action::DeleteForever);
        let Some(Dialog::Delete { paths, .. }) = &app.dialog else { panic!("asks first") };
        assert_eq!(paths, &tops);
        app.answer_dialog(&ctx, Confirm::Yes);
        land(&mut app);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).filter(|n| n != "cfg").collect();
        assert!(left.is_empty(), "everything picked is gone, nothing left behind by an error: {left:?}");
        assert_eq!(app.tab.view.error, None);
    }

    #[test]
    fn alt_r_counts_an_unfolded_folder_under_its_own_path() {
        let (ctx, mut app, dir) = unfolded_app("app-tree-count");
        std::fs::write(dir.join("sub/deeper/c.txt"), "12345").unwrap();
        app.tab.view.select_row(1);
        app.handle_action(&ctx, Action::Count);
        assert_eq!(app.counting.as_ref().map(|c| c.path.clone()), Some(dir.join("sub/deeper")));
        wait_count(&mut app);
        let expected = Totals { folders: 0, files: 1, bytes: 5, unreadable: 0 };
        assert_eq!(app.tab.view.totals.get(&dir.join("sub/deeper")), Some(&expected));
    }

    #[test]
    fn a_refresh_keeps_the_folder_unfolded_and_reads_it_again_and_entering_another_forgets_it() {
        let (ctx, mut app, dir) = unfolded_app("app-tree-refresh");
        std::fs::write(dir.join("sub/new.txt"), "n").unwrap();
        app.navigate_as(&ctx, NavKind::Refresh, dir.clone(), None);
        land(&mut app);
        app.read_kids(&ctx);
        wait_kids(&mut app, |v| v.rows.len() == 6);
        assert_eq!(row_names(&app.tab.view), ["sub", "deeper", "inner.txt", "new.txt", "a.txt", "b.txt"]);
        app.tab.view.select_row(0);
        app.handle_action(&ctx, Action::Activate);
        land(&mut app);
        assert_eq!(app.tab.view.current, dir.join("sub"));
        assert!(app.tab.view.tree.is_empty(), "another folder starts folded");
    }

    #[test]
    fn each_tab_keeps_its_own_unfolded_folders() {
        let (ctx, mut app, _dir) = unfolded_app("app-tree-tabs");
        app.handle_action(&ctx, Action::NewTab);
        land(&mut app);
        assert!(app.tab.view.tree.is_empty(), "the new tab starts folded");
        app.handle_action(&ctx, Action::PrevTab);
        land(&mut app);
        app.read_kids(&ctx);
        wait_kids(&mut app, |v| v.rows.len() == 5);
        assert!(app.tab.view.is_open(0), "and the first one kept its own");
    }

    #[test]
    fn renaming_a_child_survives_a_refresh_of_the_folder() {
        let (ctx, mut app, dir) = unfolded_app("app-tree-rename-refresh");
        app.tab.view.select_row(2);
        app.handle_action(&ctx, Action::Rename);
        app.navigate_as(&ctx, NavKind::Refresh, dir, None);
        land(&mut app);
        assert!(app.rename.is_some(), "the child is still listed, so its edit goes on");
    }

    /// `dir` {sub, a.txt, b.txt, README.md} with sub {a.md, README.md}, as the Tree view,
    /// `sub` unfolded and read: rows sub, a.md, README.md, a.txt, b.txt, README.md.
    fn same_names_app(name: &str) -> (egui::Context, LumenApp, PathBuf) {
        let (ctx, mut app, dir) = app_in(name);
        std::fs::write(dir.join("README.md"), "top").unwrap();
        std::fs::write(dir.join("sub/a.md"), "a").unwrap();
        std::fs::write(dir.join("sub/README.md"), "child").unwrap();
        app.tab.view_size = ui::view_size::ViewSize::new(10);
        let mut listing = listing_of(&dir);
        listing.id = app.tab.lister.latest_id();
        app.apply(listing);
        app.handle_action(&ctx, Action::Side(1));
        wait_kids(&mut app, |v| v.rows.len() == 6);
        (ctx, app, dir)
    }

    fn selected_path(app: &LumenApp) -> PathBuf {
        app.tab.view.selected_entry().expect("a selection").path.clone()
    }

    #[test]
    fn removing_a_child_lands_on_the_next_child_not_a_same_named_top_entry() {
        let (ctx, mut app, dir) = same_names_app("app-tree-remove-same-name");
        app.tab.view.select_row(1);
        assert_eq!(selected_path(&app), dir.join("sub/a.md"));
        app.handle_action(&ctx, Action::DeleteForever);
        app.answer_dialog(&ctx, Confirm::Yes);
        land(&mut app);
        assert_eq!(selected_path(&app), dir.join("sub/README.md"));
        app.read_kids(&ctx);
        wait_kids(&mut app, |v| v.rows.len() == 5);
        assert_eq!(selected_path(&app), dir.join("sub/README.md"), "and it stays once the folder is read again");
    }

    #[test]
    fn reloading_with_a_child_selected_keeps_that_child_not_a_same_named_top_entry() {
        let (ctx, mut app, dir) = same_names_app("app-tree-refresh-same-name");
        app.tab.view.select_row(2);
        assert_eq!(selected_path(&app), dir.join("sub/README.md"));
        app.run_command(&ctx, Command::Refresh);
        land(&mut app);
        assert_eq!(selected_path(&app), dir.join("sub/README.md"));
    }

    #[test]
    fn removing_a_child_while_filtering_keeps_the_filter() {
        let (ctx, mut app, dir) = same_names_app("app-tree-remove-filtering");
        for c in "sub".chars() {
            app.handle_action(&ctx, Action::TypeChar(c));
        }
        assert_eq!(row_names(&app.tab.view), ["sub", "a.md", "README.md"]);
        app.tab.view.select_row(1);
        app.handle_action(&ctx, Action::DeleteForever);
        app.answer_dialog(&ctx, Confirm::Yes);
        land(&mut app);
        assert_eq!(app.tab.view.filter(), "sub", "a child is never a reason to clear the filter");
        assert_eq!(selected_path(&app), dir.join("sub/README.md"));
    }

    #[test]
    fn a_renamed_child_is_selected_under_its_new_name_even_when_the_row_numbers_shift() {
        let (ctx, mut app, dir) = same_names_app("app-tree-rename-child");
        // sub: a.md, README.md; the cursor on a.md, renamed to sort last.
        app.tab.view.select_row(1);
        app.handle_action(&ctx, Action::Rename);
        let mut edit = app.rename.take().expect("renaming");
        edit.text = "zzz.md".into();
        app.finish_rename(&ctx, edit, false);
        land(&mut app);
        app.read_kids(&ctx);
        wait_kids(&mut app, |v| v.lists(&dir.join("sub/zzz.md")));
        assert_eq!(selected_path(&app), dir.join("sub/zzz.md"));
    }

    #[test]
    fn leaving_the_tree_view_lets_go_of_picked_children_but_not_picked_folder_rows() {
        let mut v = unfolded_view();
        v.select_row(0);
        v.toggle_pick(1);
        v.toggle_pick(2);
        assert_eq!(v.picked_count(), 3);
        v.fold_all();
        assert_eq!(v.picked_count(), 1);
        assert_eq!(selection_names(&v), ["src"]);
        v.toggle_open(0);
        v.apply_kids(inside("/x/src", &[("main.rs", false), ("deep", true), (".hidden", false)]));
        assert_eq!(v.picked_count(), 1, "the children are not picked when it comes back");
    }

    #[test]
    fn an_older_read_arriving_after_a_newer_one_is_ignored() {
        let mut v = loaded_view();
        v.toggle_open(0);
        v.tree.started(&p("/x/src"), 1);
        v.tree.started(&p("/x/src"), 2);
        let mut newer = inside("/x/src", &[("new.rs", false)]);
        newer.id = 2;
        let mut older = inside("/x/src", &[("old.rs", false)]);
        older.id = 1;
        v.apply_kids(newer);
        v.apply_kids(older);
        assert_eq!(row_names(&v), ["src", "new.rs", "a.txt", "b.txt"]);
        let mut late = inside("/x/src", &[("old.rs", false)]);
        late.id = 1;
        v.tree.refresh();
        v.tree.started(&p("/x/src"), 3);
        v.apply_kids(late);
        assert_eq!(row_names(&v), ["src", "new.rs", "a.txt", "b.txt"], "nor does one older than a read now under way replace it");
    }

    #[test]
    fn only_entries_sharing_one_folder_go_to_the_shell_menu() {
        let same = [p("/x/a/one"), p("/x/a/two")];
        assert_eq!(shared_parent(&same), Some(Path::new("/x/a")));
        assert_eq!(shared_parent(&[p("/x/a/one")]), Some(Path::new("/x/a")), "a single child: its own folder");
        assert_eq!(shared_parent(&[p("/x/a/one"), p("/x/two")]), None, "a child and a top entry");
        assert_eq!(shared_parent(&[p("/x/a/n"), p("/x/b/n")]), None, "same name, different folders");
        assert_eq!(shared_parent(&[]), None);
    }
}
