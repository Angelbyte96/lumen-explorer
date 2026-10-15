//! Application state and the per-frame UI loop.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::{Config, Settings};
use crate::fs::count::{Count, Totals};
use crate::fs::index::Crawler;
use crate::fs::watch::Watcher;
use crate::fs::{self, Entry, Lister, Listing, Readers};
use crate::input::{self, Action, Confirm};
use crate::menu::Command;
#[cfg(not(windows))]
use crate::menu;
use crate::places::{self, Bookmarks, Mount, Place, Recents, Usage, Visits};
use crate::fs::ops::Renamed;
use crate::fs::preview::Previewer;
use crate::fs::transfer::{self, Job, Mode, Outcome, Plan};
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
    pub fn select_all(&mut self) {
        self.unselected = false;
        let folder = self.selected_entry().and_then(|e| e.path.parent()).unwrap_or(&self.current).to_path_buf();
        self.picks = self
            .rows
            .iter()
            .map(|&i| &self.entry(i).path)
            .filter(|path| path.parent() == Some(folder.as_path()))
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
    /// drops the picks, like a click below the rows.
    pub fn select_band(&mut self, start: f32, end: f32) {
        self.unselected = false;
        let Some(last) = self.rows.len().checked_sub(1) else { return };
        let (low, high) = (start.min(end), start.max(end));
        if high < 0.0 || low >= self.rows.len() as f32 {
            self.drop_picks();
            return;
        }
        self.want = None;
        // Saturating casts: anything above the rows is row 0.
        let row = |at: f32| (at.floor().max(0.0) as usize).min(last);
        let (from, to) = (row(low), row(high));
        self.picks = self.rows[from..=to].iter().map(|&i| self.entry(i).path.clone()).collect();
        self.anchor = Some(self.entry(self.rows[row(start)]).path.clone());
        self.selected = Some(row(end));
    }

    /// A band over the cards of a view laid out in lines: picks `cells` (rows
    /// shown) and moves the cursor to `cursor`. Over no card it drops the picks.
    pub fn select_cells(&mut self, cells: &[usize], cursor: Option<usize>) {
        self.unselected = false;
        let cells: Vec<usize> = cells.iter().copied().filter(|&row| row < self.rows.len()).collect();
        if cells.is_empty() {
            self.drop_picks();
            return;
        }
        self.want = None;
        self.picks = cells.iter().map(|&row| self.entry(self.rows[row]).path.clone()).collect();
        let cursor = cursor.filter(|row| cells.contains(row)).unwrap_or(cells[cells.len() - 1]);
        self.anchor = Some(self.entry(self.rows[cells[0]]).path.clone());
        self.selected = Some(cursor);
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

    pub fn hidden_count(&self) -> usize {
        self.entries.iter().filter(|e| e.hidden).count()
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
        let top = visible_rows(&self.entries, &self.totals, self.show_hidden, &self.filter, self.sort);
        (self.rows, self.kids, self.depths) = tree_rows(&self.entries, top, &self.tree, &self.totals, self.show_hidden, self.sort);
        self.selected = keep
            .and_then(|path| self.row_of_path(path))
            .or((!self.rows.is_empty()).then_some(0));
    }

    /// The row of the folder's own entry called `name`.
    fn row_of_name(&self, name: &str) -> Option<usize> {
        (0..self.rows.len()).find(|&row| self.depth(row) == 0 && self.entry(self.rows[row]).name == name)
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

/// What a Windows menu was opened on, kept to build it again once the
/// shell's items arrive.
#[cfg(windows)]
enum MenuBasis {
    Entries { selection: Vec<Entry>, bookmarked: bool, can_paste: bool },
    Background { can_paste: bool, sort: Sort },
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
}

impl Dialog {
    fn title(&self) -> &'static str {
        match self {
            Self::Trash { .. } => "Move to Trash",
            Self::Delete { .. } => "Delete Permanently",
            Self::Replace(_) | Self::Paste { .. } => "Replace",
        }
    }

    /// What a trash or delete removes, shown under the message.
    fn subject(&self) -> Option<&ui::confirm::Subject> {
        match self {
            Self::Trash { subject, .. } | Self::Delete { subject, .. } => Some(subject),
            Self::Replace(_) | Self::Paste { .. } => None,
        }
    }

    /// It cannot be undone.
    fn danger(&self) -> bool {
        matches!(self, Self::Delete { .. })
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
            Self::Delete { .. } => "Delete",
            Self::Replace(_) | Self::Paste { .. } => "Replace",
        }
    }

    /// The line under the message saying what the keys do.
    fn hint(&self) -> &'static str {
        match self {
            Self::Paste { .. } => "Enter to replace · Esc to skip",
            Self::Delete { .. } => "Enter to delete · Esc to cancel",
            Self::Trash { .. } | Self::Replace(_) => "Enter to confirm · Esc to cancel",
        }
    }

    /// The other answer (Escape): a paste goes on without this item.
    fn no_label(&self) -> &'static str {
        match self {
            Self::Paste { .. } => "Skip",
            Self::Trash { .. } | Self::Delete { .. } | Self::Replace(_) => "Cancel",
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
    /// Id of the listing currently on screen (or the last one that failed).
    shown_id: u64,
    /// Entry to select once the pending listing arrives, by name.
    select_after_load: Option<String>,
    /// The same by full path, for entries that may be inside an unfolded folder.
    select_path_after_load: Option<PathBuf>,
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
            readers: Readers::new(),
            shown_id: 0,
            select_after_load: None,
            select_path_after_load: None,
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
    /// Where settings are saved; `None` keeps them in memory (tests).
    config_path: Option<PathBuf>,
    /// The settings last written, to save only when something changed.
    saved: Config,
    /// Inner window size in points, saved when Lumen closes.
    window: Option<[f32; 2]>,
    path_popup: PathPopup,
    /// Files' own icons (an .exe's), loaded off the UI thread.
    file_icons: crate::ui::file_icons::FileIcons,
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
    sidebar_open: bool,
    /// The view slider's position, shared by every tab.
    view_size: ui::view_size::ViewSize,
    /// The cells' lines as last drawn, in views laid out in lines (Icons);
    /// the arrows move through them.
    grid: Option<ui::grid::GridShape>,
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
            sidebar_open: true,
            view_size: ui::view_size::ViewSize::default(),
            grid: None,
            shown_layout: (ui::view_size::Layout::Details, f64::NEG_INFINITY),
            columns: ui::file_list::DEFAULT_WIDTHS,
            shown_columns: ui::file_list::ALL_SHOWN,
            column_order: ui::file_list::DEFAULT_ORDER,
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
        app.view_size = config.view;
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
            .map(|(i, path)| (i != active).then(|| Tab::new(path.clone(), Some(&app.tab.view), log)))
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
            sort: self.tab.view.sort,
            show_hidden: self.tab.view.show_hidden,
            sidebar: self.sidebar_open,
            preview: self.preview_open,
            window: if with_window { self.window } else { self.saved.window },
            bookmarks: self.bookmarks.paths().to_vec(),
            view: self.view_size,
            columns: self.columns,
            shown_columns: self.shown_columns,
            column_order: self.column_order,
            tabs: if with_window { self.session().0 } else { self.saved.tabs.clone() },
            active_tab: if with_window { self.session().1 } else { self.saved.active_tab },
            visited: if with_window { self.visits.list().to_vec() } else { self.saved.visited.clone() },
        }
    }

    /// Writes the settings when they differ from the file.
    /// Another Lumen window may have saved meanwhile: the file is read again
    /// and only what changed here is written over it.
    fn save_settings(&mut self, with_window: bool) {
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

    /// Disks for the side panel, read again every few seconds so a plugged-in
    /// USB stick shows up. Reading them is quick: local devices only.
    fn refresh_disks(&mut self) {
        if self.disks.0.is_some_and(|at| at.elapsed() < DISKS_INTERVAL) {
            return;
        }
        self.disks = (
            Some(Instant::now()),
            places::disks()
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
        self.tab.select_path_after_load = None;
        let ctx = ctx.clone();
        // Wake the UI as soon as the worker finishes, without polling in a loop.
        self.tab.lister.request(path, move || ctx.request_repaint());
    }

    fn activate_selected(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.tab.view.selected_entry() else { return };
        self.activate(ctx, entry.path.clone(), entry.is_dir);
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
            Command::CopyPath => {
                let selection = self.tab.view.selection();
                let paths: Vec<&Path> = selection.iter().map(|e| e.path.as_path()).collect();
                ctx.copy_text(paths_text(&paths));
                self.notice = Some(if paths.len() == 1 { "Path copied".into() } else { format!("{} paths copied", paths.len()) });
            }
            Command::Paste => self.paste(ctx),
            Command::PasteInto => {
                if let Some((folder, true)) = single {
                    self.paste_into(ctx, folder);
                }
            }
            Command::Rename => self.start_rename(),
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
            Command::Refresh => {
                let keep = self.tab.view.selected_entry().map(|e| e.path.clone());
                self.reload(ctx, keep);
            }
            Command::FitColumn(column) => {
                self.columns[column] = ui::file_list::fit_width(ctx, &self.tab.view, column);
            }
            Command::FitAllColumns => {
                for column in 0..self.columns.len() {
                    self.columns[column] = ui::file_list::fit_width(ctx, &self.tab.view, column);
                }
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

    fn paste(&mut self, ctx: &egui::Context) {
        self.paste_into(ctx, self.tab.view.current.clone());
    }

    /// Pastes the clipboard into `dest`: the folder on screen, or a folder's
    /// own Paste in the Windows menu.
    fn paste_into(&mut self, ctx: &egui::Context, dest: PathBuf) {
        if self.job.is_some() || self.batch.is_some() {
            self.tab.view.error = Some("Another operation is still running".into());
            return;
        }
        let Some(Clip { paths, mode }) = self.clipboard.clone() else {
            self.notice = Some("Nothing to paste: Ctrl+C or Ctrl+X first".into());
            return;
        };
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
                Dialog::Trash { .. } | Dialog::Delete { .. } => {}
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
    /// Ctrl+D (the folder on screen) or the menu (the folder clicked).
    fn toggle_bookmark(&mut self, folder: PathBuf) {
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

    /// Shows a finished listing and settles what depended on it.
    fn apply(&mut self, listing: Listing) {
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
            self.watcher.watch(&self.tab.view.current);
            if self.tab.pending_nav != NavKind::Refresh {
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

    /// Shows the unfolded folders the readers finished.
    fn poll_kids(&mut self) {
        for listing in self.tab.readers.poll() {
            self.tab.view.apply_kids(listing);
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

    /// Where an arrow key lands in a view laid out in lines: in Icons, Up and
    /// Down cross the rows and Left and Right walk the cards; in List, Up and
    /// Down walk the column and Left and Right cross the columns. `None` in Details.
    fn grid_target(&self, action: Action) -> Option<usize> {
        let shape = self.grid?;
        let last = self.tab.view.rows.len().checked_sub(1)?;
        let cur = self.tab.view.selected.unwrap_or(0).min(last);
        let rows_are_lines = self.view_size.layout() == ui::view_size::Layout::Icons;
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
        let layout = self.view_size.layout();
        if layout != self.shown_layout.0 {
            self.shown_layout = (layout, now);
        }
        let done = ((now - self.shown_layout.1) / VIEW_FADE).clamp(0.0, 1.0) as f32;
        0.35 + 0.65 * done
    }

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::FocusPathBar => self.path_popup.open(),
            Action::ToggleHidden => self.tab.view.toggle_hidden(),
            Action::ZoomIn => self.view_size = self.view_size.bigger(),
            Action::ZoomOut => self.view_size = self.view_size.smaller(),
            Action::ZoomReset => self.view_size = ui::view_size::ViewSize::default(),
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
            Action::Side(delta) if self.view_size.layout() == ui::view_size::Layout::Tree => {
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
        #[cfg(not(windows))]
        let _ = frame;
        let ctx = ui.ctx().clone();

        if let Some(listing) = self.tab.lister.poll() {
            self.apply(listing);
        }
        self.poll_kids();
        self.read_kids(&ctx);

        self.refresh_if_changed(&ctx);
        self.poll_count();
        #[cfg(windows)]
        self.read_pending_menu(&ctx);

        ui::skip_panel_borders(&ctx);

        // An open dialog takes the keyboard; the list must not react underneath.
        if self.dialog.is_some() {
            if let Some(answer) = input::read_confirm(&ctx) {
                self.answer_dialog(&ctx, answer);
            }
        } else if self.path_popup.is_open() || self.context_menu.is_some() {
            // The path box and the menu read their own keys when drawn.
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
            if let Some(event) = ui::toolbar::show(ui, &nav) {
                self.handle_toolbar(&ctx, event);
            }
            ui.add_space(2.0);
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
            let scroll = self.tab.view.scroll.take();
            let cut = self.cut_paths().to_vec();
            let opacity = self.list_opacity(ctx.input(|i| i.time));
            if opacity < 1.0 {
                ui.multiply_opacity(opacity);
                ctx.request_repaint();
            }
            let tree = self.view_size.layout() == ui::view_size::Layout::Tree;
            if !tree {
                // The unfolded folders belong to the Tree view alone.
                self.tab.view.fold_all();
            }
            let sizes = ui::file_list::Sizes { widths: self.columns, shown: self.shown_columns, order: self.column_order, tree };
            let lined = match (self.view_size.layout(), self.view_size.growth()) {
                (_, Some(growth)) => Some(ui::icon_grid::show(ui, &self.tab.view, &cut, growth, scroll, self.rename.as_mut(), &self.file_icons)),
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
                    self.read_kids(&ctx);
                }
                Some(ListEvent::Extend(i)) => self.tab.view.extend_to(i),
                Some(ListEvent::Band { start, end }) => self.tab.view.select_band(start, end),
                Some(ListEvent::Cells { cells, cursor }) => self.tab.view.select_cells(&cells, cursor),
                Some(ListEvent::Activate(i)) => {
                    self.tab.view.select_row(i);
                    self.activate_selected(&ctx);
                }
                Some(ListEvent::Sort(column)) => self.tab.view.sort_by(column),
                Some(ListEvent::Resized(widths)) => self.columns = widths,
                Some(ListEvent::CloseColumn(column)) => self.run_command(&ctx, Command::ToggleColumn(column)),
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
                        self.finish_rename(&ctx, edit, false);
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
            });
        });

        // Floating over the list's bottom, in the room left for them there.
        let picked = ui::status_bar::show(
            &ctx,
            list.response.rect,
            &Status {
                filter: self.tab.view.filter(),
                caret: !self.path_popup.is_open() && self.rename.is_none(),
                item_count: self.tab.view.rows.len(),
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
                picked: self.tab.view.picked_count(),
                picked_size: self.tab.view.picked_size(),
                counting: self.counting.as_ref().map(|c| {
                    let name = file_name(&c.path).unwrap_or_default();
                    ui::status_bar::counting_text(&name, &c.progress)
                }),
                view: self.view_size,
            },
        );
        if let Some(view) = picked {
            self.view_size = view;
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

        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.window = Some([rect.width(), rect.height()]);
        }
        self.save_settings(false);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
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
        assert_eq!(app.view_size, ui::view_size::ViewSize::new(65));
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
    fn ctrl_plus_and_minus_move_the_view_slider_and_it_is_saved_at_once() {
        use ui::view_size::ViewSize;
        let (ctx, _, dir) = app_in("app-zoom");
        let path = dir.join("cfg/lumen/config");
        let mut app = LumenApp::with_config(&ctx, dir.clone(), false, crate::config::open(Some(path.clone())));
        app.handle_action(&ctx, Action::ZoomOut);
        assert_eq!(app.view_size, ViewSize::new(0), "already Details");
        let mut walked = Vec::new();
        for _ in 0..5 {
            app.handle_action(&ctx, Action::ZoomIn);
            walked.push(app.view_size.name());
        }
        assert_eq!(walked, ["Tree", "List", "M Icons", "M Icons", "M Icons"]);
        assert_eq!(app.view_size, ViewSize::new(40));
        app.handle_action(&ctx, Action::ZoomOut);
        assert_eq!(app.view_size, ViewSize::new(35));
        app.save_settings(false);
        assert_eq!(Config::load(&path).unwrap().view, ViewSize::new(35));
        app.handle_action(&ctx, Action::ZoomReset);
        assert_eq!(app.view_size, ViewSize::new(0), "Ctrl+0 goes back to Details");
        assert_eq!(ctx.zoom_factor(), 1.0, "the rest of the window never zooms");
    }

    #[test]
    fn in_the_icons_view_up_and_down_cross_rows_and_left_right_walk_the_cards() {
        use ui::grid::GridShape;
        let (ctx, mut app, _dir) = app_in("app-grid-keys");
        app.view_size = ui::view_size::ViewSize::new(30);
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
        app.view_size = ui::view_size::ViewSize::new(20);
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
        app.view_size = ViewSize::new(20);
        let start = app.list_opacity(10.0);
        assert!(start < 1.0, "List just picked: still fading in ({start})");
        let mid = app.list_opacity(10.0 + VIEW_FADE / 2.0);
        assert!(mid > start && mid < 1.0, "half way: {mid}");
        assert_eq!(app.list_opacity(10.0 + VIEW_FADE), 1.0, "done after the fade");
        app.view_size = ViewSize::new(30);
        assert!(app.list_opacity(20.0) < 1.0, "List to M Icons is another view");
        app.view_size = ViewSize::new(80);
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
        assert_eq!(written.sort.column, SortColumn::Modified);
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
        v.select_band(1.5, 2.2);
        assert_eq!(selection_names(&v), ["a.txt", "b.txt"]);
        assert_eq!(v.selected, Some(2), "the cursor follows the pointer's end");
        v.select_band(2.9, 0.1);
        assert_eq!(selection_names(&v), ["src", "a.txt", "b.txt"], "dragged upwards");
        assert_eq!(v.selected, Some(0));
        v.select_band(1.2, 1.4);
        assert_eq!(selection_names(&v), ["a.txt"], "a short drag inside one row picks it");
        assert_eq!(v.picked_count(), 1);
        v.select_band(-3.0, 9.0);
        assert_eq!(selection_names(&v), ["src", "a.txt", "b.txt"], "clamped to the rows shown");
        assert_eq!(v.selected, Some(2));
        v.extend_to(1);
        assert_eq!(selection_names(&v), ["src", "a.txt"], "Shift+click ranges from where the band started");
    }

    #[test]
    fn a_band_over_no_row_drops_the_picks_and_keeps_the_cursor() {
        let mut v = loaded_view();
        v.select_all();
        v.select_row(1);
        v.select_all();
        v.select_band(3.5, 5.0);
        assert_eq!(v.picked_count(), 0, "below the rows");
        assert_eq!(selection_names(&v), ["a.txt"], "like a click below the rows");
        v.select_all();
        v.select_band(-2.0, -0.5);
        assert_eq!(v.picked_count(), 0, "above the rows");
        let mut empty = View::default();
        empty.select_band(0.0, 2.0);
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
        v.select_cells(&[0, 2, 9], Some(2));
        assert_eq!(v.picked_count(), 2, "a cell past the rows is ignored");
        assert!(v.is_picked(0) && v.is_picked(2) && !v.is_picked(1));
        assert_eq!(v.selected, Some(2));
        v.select_cells(&[], None);
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
        v.select_band(1.2, 3.5);
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
        app.view_size = ui::view_size::ViewSize::new(10);
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
        app.view_size = ui::view_size::ViewSize::new(10);
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
