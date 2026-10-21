//! The right-click menu, as data: what it offers for the selection or for the
//! folder's empty space, and how the search box and the keys walk it. Drawn by
//! `ui::context_menu`; File Pilot style, submenus open in place, indented.

use std::collections::HashSet;

use crate::app::{Sort, SortColumn};
use crate::fs::Entry;
use crate::ui::file_list::{COLUMNS, Order, SORT_ASCENDING, SORT_DESCENDING, Shown, shift};
use crate::ui::path_popup::word_ranges;

/// An application that can open a file ("Open With").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// Shown in the menu: the .desktop Name, in the user's language when it has one.
    pub name: String,
    /// The .desktop file, launched with `gio launch`.
    pub desktop: std::path::PathBuf,
}

/// What a menu item does once picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open,
    OpenWith(App),
    OpenInNewTab,
    Cut,
    Copy,
    CopyPath,
    Paste,
    /// Paste into the single selected folder (its own Paste, on Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    PasteInto,
    Rename,
    Trash,
    DeleteForever,
    /// Bookmark the selected folder, or remove its bookmark.
    ToggleBookmark,
    Count,
    NewFolder,
    /// An empty file, or a copy of this template.
    NewFile(Option<std::path::PathBuf>),
    /// Open a terminal in the selected folder, or in the folder on screen.
    OpenTerminal,
    /// A Windows shell item, by its offset from the first command id.
    #[cfg_attr(not(windows), allow(dead_code))]
    Shell(u32),
    Sort(SortColumn),
    Refresh,
    /// The list's header menu: this column's width to its content.
    FitColumn(usize),
    /// Every column's width to its content (Ctrl+Shift+F).
    FitAllColumns,
    RestoreColumns,
    /// Move the column one place left, past hidden ones.
    ShiftColumnLeft(usize),
    ShiftColumnRight(usize),
    /// Show the column again, or hide it.
    ToggleColumn(usize),
    /// The side panel's menu: these act on that item's path, never on the list's selection.
    OpenPlace(std::path::PathBuf),
    OpenPlaceInNewTab(std::path::PathBuf),
    CopyPlacePath(std::path::PathBuf),
    /// Bookmark the folder, or remove its bookmark.
    ToggleBookmarkOf(std::path::PathBuf),
}

/// The line icon drawn left of an item (see `ui::icons::menu_icon`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIcon {
    Open,
    OpenWith,
    NewTab,
    Cut,
    Copy,
    CopyPath,
    Paste,
    Rename,
    Trash,
    Delete,
    Bookmark,
    Size,
    New,
    NewFolder,
    NewFile,
    Terminal,
    Sort,
    Refresh,
}

impl MenuIcon {
    #[cfg(test)]
    pub const ALL: [Self; 18] = [
        Self::Open,
        Self::OpenWith,
        Self::NewTab,
        Self::Cut,
        Self::Copy,
        Self::CopyPath,
        Self::Paste,
        Self::Rename,
        Self::Trash,
        Self::Delete,
        Self::Bookmark,
        Self::Size,
        Self::New,
        Self::NewFolder,
        Self::NewFile,
        Self::Terminal,
        Self::Sort,
        Self::Refresh,
    ];

    /// The icon a command shows; sort columns have none, like Dolphin's.
    fn of(command: &Command) -> Option<Self> {
        Some(match command {
            Command::Open | Command::OpenPlace(_) => Self::Open,
            Command::OpenWith(_) => return None,
            Command::OpenInNewTab | Command::OpenPlaceInNewTab(_) => Self::NewTab,
            Command::Cut => Self::Cut,
            Command::Copy => Self::Copy,
            Command::CopyPath | Command::CopyPlacePath(_) => Self::CopyPath,
            Command::Paste | Command::PasteInto => Self::Paste,
            Command::Rename => Self::Rename,
            Command::Trash => Self::Trash,
            Command::DeleteForever => Self::Delete,
            Command::ToggleBookmark | Command::ToggleBookmarkOf(_) => Self::Bookmark,
            Command::Count => Self::Size,
            Command::NewFolder => Self::NewFolder,
            Command::NewFile(_) => Self::NewFile,
            Command::OpenTerminal => Self::Terminal,
            Command::Refresh => Self::Refresh,
            Command::Sort(_) | Command::Shell(_) => return None,
            Command::FitColumn(_) | Command::FitAllColumns | Command::RestoreColumns | Command::ToggleColumn(_) => return None,
            Command::ShiftColumnLeft(_) | Command::ShiftColumnRight(_) => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Run(Command),
    Sub(Vec<Item>),
    Separator,
}

/// A picture an item brings along (a Windows shell item's own icon), in
/// premultiplied RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub icon: Option<MenuIcon>,
    /// Drawn instead of `icon` when set.
    pub picture: Option<std::sync::Arc<Bitmap>>,
    pub shortcut: Option<&'static str>,
    pub enabled: bool,
    /// A checkbox drawn in the icon column, ticked or not.
    pub checked: Option<bool>,
    pub kind: Kind,
}

impl Item {
    pub fn run(label: impl Into<String>, shortcut: Option<&'static str>, enabled: bool, command: Command) -> Self {
        Self { label: label.into(), icon: MenuIcon::of(&command), picture: None, shortcut, enabled, checked: None, kind: Kind::Run(command) }
    }

    pub fn sub(label: impl Into<String>, icon: MenuIcon, items: Vec<Item>) -> Self {
        let enabled = items.iter().any(|item| item.enabled);
        Self { label: label.into(), icon: Some(icon), picture: None, shortcut: None, enabled, checked: None, kind: Kind::Sub(items) }
    }

    /// A submenu without a line icon (the shell's).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn sub_plain(label: impl Into<String>, items: Vec<Item>) -> Self {
        Self { icon: None, ..Self::sub(label, MenuIcon::New, items) }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn with_picture(self, picture: Option<std::sync::Arc<Bitmap>>) -> Self {
        Self { picture, ..self }
    }

    /// An item with a checkbox, ticked when `checked`.
    pub fn check(label: impl Into<String>, checked: bool, enabled: bool, command: Command) -> Self {
        Self { checked: Some(checked), ..Self::run(label, None, enabled, command) }
    }

    pub fn separator() -> Self {
        Self { label: String::new(), icon: None, picture: None, shortcut: None, enabled: false, checked: None, kind: Kind::Separator }
    }
}

/// A line of the open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Indices from the top level down to this item.
    pub at: Vec<usize>,
    /// What the line says: while searching, with its submenus before it
    /// ("Sort by › Name") so they can be searched too.
    pub label: String,
    /// Bytes of `label` matching the typed words.
    pub marks: Vec<std::ops::Range<usize>>,
    pub depth: usize,
}

/// Arrow keys and Enter, as the menu reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    /// Opens the submenu under the cursor.
    Right,
    /// Closes the submenu the cursor is in (or on).
    Left,
    /// The first line, the last line.
    Home,
    End,
    /// `PAGE` lines up or down, stopping at the first or the last.
    PageUp,
    PageDown,
    Enter,
}

/// How many lines Page Up and Page Down move.
pub const PAGE: usize = 10;

/// The open menu: its items, the search text and the line under the cursor.
pub struct Menu {
    pub items: Vec<Item>,
    pub query: String,
    /// Submenus open in place, by `Row::at`.
    expanded: HashSet<Vec<usize>>,
    /// Index into `rows()`. While searching and none is picked, the first
    /// enabled row counts as picked, like the path box.
    cursor: Option<usize>,
}

pub const SEARCH_SEPARATOR: &str = " › ";

impl Menu {
    pub fn new(items: Vec<Item>) -> Self {
        Self { items, query: String::new(), expanded: HashSet::new(), cursor: None }
    }

    pub fn item(&self, at: &[usize]) -> &Item {
        let (first, rest) = at.split_first().expect("a row has a place");
        rest.iter().fold(&self.items[*first], |item, &i| match &item.kind {
            Kind::Sub(sub) => &sub[i],
            _ => unreachable!("only submenus have children"),
        })
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// The lines shown: the items with their open submenus, or, while
    /// searching, every runnable item matching all the typed words.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.walk(&self.items, &mut Vec::new(), "", &mut rows);
        rows
    }

    fn walk(&self, items: &[Item], at: &mut Vec<usize>, parents: &str, rows: &mut Vec<Row>) {
        for (i, item) in items.iter().enumerate() {
            at.push(i);
            if self.searching() {
                let label = format!("{parents}{}", item.label);
                match &item.kind {
                    Kind::Run(_) => {
                        if let Some(marks) = word_ranges(&label, &self.query) {
                            rows.push(Row { at: at.clone(), label, marks, depth: 0 });
                        }
                    }
                    Kind::Sub(sub) => self.walk(sub, at, &format!("{label}{SEARCH_SEPARATOR}"), rows),
                    Kind::Separator => {}
                }
            } else {
                rows.push(Row { at: at.clone(), label: item.label.clone(), marks: Vec::new(), depth: at.len() - 1 });
                if let Kind::Sub(sub) = &item.kind
                    && self.expanded.contains(at)
                {
                    self.walk(sub, at, parents, rows);
                }
            }
            at.pop();
        }
    }

    /// The row under the cursor, if any.
    pub fn cursor(&self) -> Option<usize> {
        let rows = self.rows();
        match self.cursor {
            Some(row) => (row < rows.len()).then_some(row),
            None if self.searching() => rows.iter().position(|row| self.item(&row.at).enabled),
            None => None,
        }
    }

    /// The search text changed: the cursor goes back to the first match.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.cursor = None;
    }

    /// Moves, opens or closes; Enter on a runnable item returns its command.
    pub fn key(&mut self, key: Key) -> Option<Command> {
        let rows = self.rows();
        let line = |row: usize| self.item(&rows[row].at).kind != Kind::Separator;
        let cursor = self.cursor();
        match key {
            Key::Down => {
                let from = cursor.map_or(0, |c| c + 1);
                if let Some(row) = (from..rows.len()).find(|&row| line(row)) {
                    self.cursor = Some(row);
                }
            }
            Key::Up => {
                if let Some(row) = cursor.and_then(|c| (0..c).rfind(|&row| line(row))) {
                    self.cursor = Some(row);
                }
            }
            Key::Right => {
                if let Some(row) = cursor.filter(|_| !self.searching()) {
                    self.expand(&rows[row].at);
                }
            }
            Key::Left => {
                let row = cursor.filter(|_| !self.searching())?;
                let at = &rows[row].at;
                let target = if self.expanded.contains(at) { at.clone() } else { at[..at.len() - 1].to_vec() };
                if target.is_empty() {
                    return None;
                }
                self.collapse(&target);
                self.cursor = self.rows().iter().position(|row| row.at == target);
            }
            Key::Home | Key::End | Key::PageUp | Key::PageDown => {
                let lines: Vec<usize> = (0..rows.len()).filter(|&row| line(row)).collect();
                let at = cursor.and_then(|c| lines.iter().position(|&row| row == c));
                let to = match (key, at) {
                    (Key::Home, _) => Some(0),
                    (Key::End, _) => lines.len().checked_sub(1),
                    (Key::PageDown, None) => Some(PAGE - 1),
                    (Key::PageDown, Some(at)) => Some(at + PAGE),
                    (_, at) => Some(at.unwrap_or(0).saturating_sub(PAGE)),
                };
                if let Some(to) = to.filter(|_| !lines.is_empty()) {
                    self.cursor = Some(lines[to.min(lines.len() - 1)]);
                }
            }
            Key::Enter => return cursor.and_then(|row| self.pick(row)),
        }
        None
    }

    /// The pointer moved onto row `row`: it becomes the cursor's line.
    pub fn hover(&mut self, row: usize) {
        let rows = self.rows();
        if rows.get(row).is_some_and(|line| self.item(&line.at).kind != Kind::Separator) {
            self.cursor = Some(row);
        }
    }

    /// A click on row `row`: runs it, or opens / closes a submenu.
    pub fn click(&mut self, row: usize) -> Option<Command> {
        let at = self.rows().get(row)?.at.clone();
        self.cursor = Some(row);
        if matches!(self.item(&at).kind, Kind::Sub(_)) && self.expanded.contains(&at) {
            self.collapse(&at);
            return None;
        }
        self.pick(row)
    }

    /// Runs row `row`, or opens it when it is a submenu.
    fn pick(&mut self, row: usize) -> Option<Command> {
        let at = self.rows().get(row)?.at.clone();
        let item = self.item(&at);
        match &item.kind {
            Kind::Run(command) if item.enabled => Some(command.clone()),
            Kind::Sub(_) if !self.searching() => {
                self.expand(&at);
                None
            }
            _ => None,
        }
    }

    fn expand(&mut self, at: &[usize]) {
        if matches!(self.item(at).kind, Kind::Sub(_)) {
            self.expanded.insert(at.to_vec());
        }
    }

    /// Closes the submenu at `at` and every one inside it.
    fn collapse(&mut self, at: &[usize]) {
        self.expanded.retain(|open| !open.starts_with(at));
    }
}

/// The menu for the selected entries (`selection`, never empty).
/// `bookmarked`: the single selected folder is a bookmark. `apps`: what can
/// open the single selected entry, best first.
pub fn entry_menu(selection: &[&Entry], bookmarked: bool, apps: &[App]) -> Vec<Item> {
    let single = selection.len() == 1;
    let folder = single && selection[0].is_dir;
    let mut items = vec![Item::run("Open", Some("Enter"), single, Command::Open)];
    if single && !apps.is_empty() {
        let apps = apps.iter().map(|app| Item::run(app.name.clone(), None, true, Command::OpenWith(app.clone()))).collect();
        items.push(Item::sub("Open With", MenuIcon::OpenWith, apps));
    }
    if folder {
        items.push(Item::run("Open in New Tab", None, true, Command::OpenInNewTab));
    }
    items.extend([
        Item::separator(),
        Item::run("Cut", Some("Ctrl+X"), true, Command::Cut),
        Item::run("Copy", Some("Ctrl+C"), true, Command::Copy),
        Item::run("Copy Path", Some("Ctrl+Shift+C"), true, Command::CopyPath),
        Item::separator(),
        Item::run("Rename", Some("F2"), single, Command::Rename),
        Item::run("Move to Trash", Some("Delete"), true, Command::Trash),
        Item::run("Delete Permanently", Some("Shift+Delete"), true, Command::DeleteForever),
    ]);
    if folder {
        let bookmark = if bookmarked { "Remove from Bookmarks" } else { "Add to Bookmarks" };
        items.extend([
            Item::separator(),
            Item::run(bookmark, None, true, Command::ToggleBookmark),
            Item::run("Calculate Size", Some("Alt+R"), true, Command::Count),
        ]);
        if TERMINAL {
            items.push(Item::run("Open Terminal Here", None, true, Command::OpenTerminal));
        }
    }
    items
}

/// The menu for a side panel item (a bookmark, place, disk or recent folder):
/// its own path, whatever the list has selected. `bookmarked`: it is a bookmark.
pub fn place_menu(path: &std::path::Path, bookmarked: bool) -> Vec<Item> {
    let mut items = vec![Item::run("Open", None, true, Command::OpenPlace(path.to_path_buf())), Item::separator()];
    items.extend(place_own(path, bookmarked));
    items
}

/// What only Lumen adds to a side panel item's menu.
pub fn place_own(path: &std::path::Path, bookmarked: bool) -> [Item; 3] {
    let bookmark = if bookmarked { "Remove from Bookmarks" } else { "Add to Bookmarks" };
    [
        Item::run("Open in New Tab", None, true, Command::OpenPlaceInNewTab(path.to_path_buf())),
        Item::run("Copy Path", None, true, Command::CopyPlacePath(path.to_path_buf())),
        Item::run(bookmark, None, true, Command::ToggleBookmarkOf(path.to_path_buf())),
    ]
}

/// Lumen opens terminals itself only off Windows; there the shell's menu does.
const TERMINAL: bool = cfg!(not(windows));

/// "Sort by" with the list's columns, the one sorted by marked with its direction.
pub fn sort_menu(sort: Sort) -> Item {
    let columns = COLUMNS
        .iter()
        .map(|&(label, column)| {
            let label = match (sort.column == column, sort.descending) {
                (false, _) => label.to_owned(),
                (true, false) => format!("{label} {SORT_ASCENDING}"),
                (true, true) => format!("{label} {SORT_DESCENDING}"),
            };
            Item::run(label, None, true, Command::Sort(column))
        })
        .collect();
    Item::sub("Sort by", MenuIcon::Sort, columns)
}

/// Right click on the list's header `column`, like File Pilot's.
/// The checkboxes show or hide each column; Name is always shown.
pub fn column_menu(column: usize, shown: &Shown, order: &Order) -> Vec<Item> {
    let can_shift = |left| shift(order, shown, column, left) != *order;
    let mut items = vec![
        Item::run("Shift Column Left", None, can_shift(true), Command::ShiftColumnLeft(column)),
        Item::run("Shift Column Right", None, can_shift(false), Command::ShiftColumnRight(column)),
        Item::run("Size Column To Fit", None, true, Command::FitColumn(column)),
        Item::run("Close Column", Some("Middle click"), column != 0, Command::ToggleColumn(column)),
        Item::run("Size All Columns To Fit", Some("Ctrl+Shift+F"), true, Command::FitAllColumns),
        Item::separator(),
        Item::run("Restore Default Columns", None, true, Command::RestoreColumns),
        Item::separator(),
    ];
    for (i, (label, _)) in COLUMNS.into_iter().enumerate() {
        items.push(Item::check(label, shown[i], i != 0, Command::ToggleColumn(i)));
    }
    items
}

/// The menu for the folder's empty space. `can_paste`: the clipboard holds
/// something; `templates`: the files New ▸ offers copies of.
pub fn background_menu(can_paste: bool, sort: Sort, templates: &[std::path::PathBuf]) -> Vec<Item> {
    let mut new = vec![
        Item::run("Folder", Some("Ctrl+Shift+N"), true, Command::NewFolder),
        Item::run("Empty File", None, true, Command::NewFile(None)),
    ];
    if !templates.is_empty() {
        new.push(Item::separator());
    }
    new.extend(templates.iter().map(|t| {
        // The name without its extension, as file managers list templates.
        let label = t.file_stem().map_or_else(|| t.display().to_string(), |s| s.to_string_lossy().into_owned());
        Item::run(label, None, true, Command::NewFile(Some(t.clone())))
    }));
    let mut items = vec![Item::run("Paste", Some("Ctrl+V"), can_paste, Command::Paste), Item::sub("New", MenuIcon::New, new)];
    if TERMINAL {
        items.push(Item::run("Open Terminal Here", None, true, Command::OpenTerminal));
    }
    items.extend([Item::separator(), sort_menu(sort), Item::run("Refresh", None, true, Command::Refresh)]);
    items
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn run(label: &str, command: Command) -> Item {
        Item::run(label, None, true, command)
    }

    /// Open, a separator, "Sort by" (Name, Size) and a disabled Paste.
    fn sample() -> Menu {
        Menu::new(vec![
            run("Open", Command::Open),
            Item::separator(),
            Item::sub("Sort by", MenuIcon::Sort, vec![run("Name", Command::Sort(SortColumn::Name)), run("Size", Command::Sort(SortColumn::Size))]),
            Item::run("Paste", Some("Ctrl+V"), false, Command::Paste),
        ])
    }

    fn labels(menu: &Menu) -> Vec<String> {
        menu.rows().into_iter().map(|row| format!("{}{}", "  ".repeat(row.depth), row.label)).collect()
    }

    #[test]
    fn submenus_start_closed_and_open_in_place_indented() {
        let mut menu = sample();
        assert_eq!(labels(&menu), ["Open", "", "Sort by", "Paste"]);
        assert_eq!(menu.cursor(), None, "nothing picked until a key or a search");
        menu.click(2);
        assert_eq!(labels(&menu), ["Open", "", "Sort by", "  Name", "  Size", "Paste"]);
        menu.click(2);
        assert_eq!(labels(&menu), ["Open", "", "Sort by", "Paste"], "a second click closes it");
    }

    #[test]
    fn hovering_a_line_moves_the_cursor_but_not_onto_a_separator() {
        let mut menu = sample();
        menu.hover(2);
        assert_eq!(menu.cursor(), Some(2));
        menu.hover(1);
        assert_eq!(menu.cursor(), Some(2), "a separator is not a line");
        menu.hover(9);
        assert_eq!(menu.cursor(), Some(2), "past the end");
        menu.key(Key::Down);
        assert_eq!(menu.cursor(), Some(3), "the keys go on from the hovered line");
    }

    #[test]
    fn the_keys_skip_separators_open_submenus_and_run_items() {
        let mut menu = sample();
        assert_eq!(menu.key(Key::Down), None);
        assert_eq!(menu.cursor(), Some(0));
        menu.key(Key::Down);
        assert_eq!(menu.cursor(), Some(2), "over the separator");
        menu.key(Key::Right);
        menu.key(Key::Down);
        assert_eq!(menu.rows()[menu.cursor().unwrap()].label, "Name");
        menu.key(Key::Down);
        assert_eq!(menu.key(Key::Enter), Some(Command::Sort(SortColumn::Size)));
        menu.key(Key::Left);
        assert_eq!(labels(&menu), ["Open", "", "Sort by", "Paste"], "Left closes the submenu it is in");
        assert_eq!(menu.rows()[menu.cursor().unwrap()].label, "Sort by", "and goes back to it");
        assert_eq!(menu.key(Key::Enter), None, "Enter on a submenu opens it");
        assert_eq!(labels(&menu).len(), 6);
        for _ in 0..10 {
            menu.key(Key::Down);
        }
        assert_eq!(menu.rows()[menu.cursor().unwrap()].label, "Paste", "stops at the end");
        menu.key(Key::Up);
        menu.key(Key::Up);
        menu.key(Key::Up);
        menu.key(Key::Up);
        assert_eq!(menu.cursor(), Some(0), "stops at the top");
    }

    #[test]
    fn disabled_items_never_run() {
        let mut menu = sample();
        assert_eq!(menu.click(3), None);
        for _ in 0..3 {
            menu.key(Key::Down);
        }
        assert_eq!(menu.rows()[menu.cursor().unwrap()].label, "Paste");
        assert_eq!(menu.key(Key::Enter), None);
    }

    #[test]
    fn a_search_lists_matching_items_of_every_level_with_their_submenu() {
        let mut menu = sample();
        menu.set_query("size".into());
        assert_eq!(labels(&menu), ["Sort by › Size"]);
        assert_eq!(menu.rows()[0].marks, vec![12..16]);
        assert_eq!(menu.cursor(), Some(0), "the first match is picked");
        assert_eq!(menu.key(Key::Enter), Some(Command::Sort(SortColumn::Size)));
        menu.set_query("sort n".into());
        assert_eq!(labels(&menu), ["Sort by › Name"], "words in any order, submenu names count");
        menu.set_query("pa".into());
        assert_eq!(labels(&menu), ["Paste"]);
        assert_eq!(menu.cursor(), None, "a disabled match is listed but not picked");
        menu.set_query("zzz".into());
        assert_eq!(labels(&menu), Vec::<String>::new());
        assert_eq!(menu.key(Key::Enter), None);
    }

    pub(crate) fn entry(name: &str, is_dir: bool) -> Entry {
        Entry { name: name.into(), path: format!("/x/{name}").into(), is_dir, size: None, modified: None, hidden: false }
    }

    /// `expected` without the items Lumen adds only off Windows.
    fn here(expected: &[&str]) -> Vec<String> {
        expected.iter().filter(|line| TERMINAL || !line.contains("Open Terminal Here")).map(|s| s.to_string()).collect()
    }

    /// Label and whether it is enabled, sub-items indented; separators as "-".
    pub(crate) fn outline(items: &[Item]) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(items: &[Item], depth: usize, out: &mut Vec<String>) {
            for item in items {
                let off = if item.enabled || item.kind == Kind::Separator { "" } else { " (off)" };
                let shortcut = item.shortcut.map(|s| format!(" [{s}]")).unwrap_or_default();
                let label = if item.kind == Kind::Separator { "-" } else { &item.label };
                out.push(format!("{}{label}{shortcut}{off}", "  ".repeat(depth)));
                if let Kind::Sub(sub) = &item.kind {
                    walk(sub, depth + 1, out);
                }
            }
        }
        walk(items, 0, &mut out);
        out
    }

    /// Every item has an icon but the sort columns and the apps, which Dolphin leaves bare too.
    fn without_icon(items: &[Item]) -> Vec<String> {
        let mut out = Vec::new();
        for item in items {
            match &item.kind {
                Kind::Separator => {}
                Kind::Run(Command::Sort(_) | Command::OpenWith(_)) => {}
                _ if item.icon.is_none() => out.push(item.label.clone()),
                _ => {}
            }
            if let Kind::Sub(sub) = &item.kind {
                out.extend(without_icon(sub));
            }
        }
        out
    }

    #[test]
    fn every_item_has_an_icon_except_the_sort_columns() {
        let (dir, file) = (entry("src", true), entry("a.txt", false));
        let templates = [std::path::PathBuf::from("/t/Report.odt")];
        let place = place_menu(std::path::Path::new("/x/src"), false);
        for items in [entry_menu(&[&dir], false, &[]), entry_menu(&[&file], true, &[]), background_menu(true, Sort::default(), &templates), place] {
            assert_eq!(without_icon(&items), Vec::<String>::new());
        }
    }

    #[test]
    fn a_file_offers_open_clipboard_rename_and_trash() {
        let file = entry("a.txt", false);
        assert_eq!(
            outline(&entry_menu(&[&file], false, &[])),
            [
                "Open [Enter]",
                "-",
                "Cut [Ctrl+X]",
                "Copy [Ctrl+C]",
                "Copy Path [Ctrl+Shift+C]",
                "-",
                "Rename [F2]",
                "Move to Trash [Delete]",
                "Delete Permanently [Shift+Delete]",
            ]
        );
    }

    #[test]
    fn a_folder_adds_new_tab_bookmark_and_size() {
        let dir = entry("src", true);
        assert_eq!(
            outline(&entry_menu(&[&dir], false, &[])),
            here(&[
                "Open [Enter]",
                "Open in New Tab",
                "-",
                "Cut [Ctrl+X]",
                "Copy [Ctrl+C]",
                "Copy Path [Ctrl+Shift+C]",
                "-",
                "Rename [F2]",
                "Move to Trash [Delete]",
                "Delete Permanently [Shift+Delete]",
                "-",
                "Add to Bookmarks",
                "Calculate Size [Alt+R]",
                "Open Terminal Here",
            ])
        );
        assert!(outline(&entry_menu(&[&dir], true, &[])).contains(&"Remove from Bookmarks".to_owned()));
    }

    #[test]
    fn a_side_panel_item_offers_open_new_tab_copy_path_and_its_bookmark_all_on_its_own_path() {
        let path = std::path::PathBuf::from("/x/src");
        assert_eq!(outline(&place_menu(&path, false)), ["Open", "-", "Open in New Tab", "Copy Path", "Add to Bookmarks"]);
        let items = place_menu(&path, true);
        assert_eq!(outline(&items)[4], "Remove from Bookmarks");
        let commands: Vec<Command> = lines(&items).into_iter().filter_map(|line| line.2.cloned()).collect();
        let expected = [Command::OpenPlace(path.clone()), Command::OpenPlaceInNewTab(path.clone()), Command::CopyPlacePath(path.clone()), Command::ToggleBookmarkOf(path)];
        assert_eq!(commands, expected);
    }

    #[test]
    fn open_with_lists_the_apps_for_one_entry() {
        let file = entry("a.txt", false);
        let apps = [App { name: "Editor".into(), desktop: "/a/editor.desktop".into() }, App { name: "Vim".into(), desktop: "/a/vim.desktop".into() }];
        let items = entry_menu(&[&file], false, &apps);
        assert_eq!(outline(&items)[..4], ["Open [Enter]", "Open With", "  Editor", "  Vim"]);
        let Kind::Sub(sub) = &items[1].kind else { panic!("a submenu") };
        assert_eq!(sub[1].kind, Kind::Run(Command::OpenWith(apps[1].clone())));
        assert_eq!(outline(&entry_menu(&[&file], false, &[]))[1], "-", "no apps, no submenu");
    }

    #[test]
    fn several_entries_keep_only_what_works_on_all() {
        let (dir, file) = (entry("src", true), entry("a.txt", false));
        assert_eq!(
            outline(&entry_menu(&[&dir, &file], false, &[])),
            [
                "Open [Enter] (off)",
                "-",
                "Cut [Ctrl+X]",
                "Copy [Ctrl+C]",
                "Copy Path [Ctrl+Shift+C]",
                "-",
                "Rename [F2] (off)",
                "Move to Trash [Delete]",
                "Delete Permanently [Shift+Delete]",
            ]
        );
    }

    #[test]
    fn the_empty_space_offers_paste_new_sort_and_refresh() {
        let sort = Sort { column: SortColumn::Size, descending: true };
        let templates = [std::path::PathBuf::from("/t/Report.odt"), std::path::PathBuf::from("/t/Note")];
        let items = background_menu(true, sort, &templates);
        assert_eq!(
            outline(&items),
            here(&[
                "Paste [Ctrl+V]",
                "New",
                "  Folder [Ctrl+Shift+N]",
                "  Empty File",
                "  -",
                "  Report",
                "  Note",
                "Open Terminal Here",
                "-",
                "Sort by",
                "  Name",
                "  Type",
                "  Date Modified",
                "  Folders",
                "  Files",
                &format!("  Size {SORT_DESCENDING}"),
                "Refresh",
            ])
        );
        let Kind::Sub(new) = &items[1].kind else { panic!("New is a submenu") };
        assert_eq!(new[1].kind, Kind::Run(Command::NewFile(None)));
        assert_eq!(new[3].kind, Kind::Run(Command::NewFile(Some(templates[0].clone()))));
        let bare = outline(&background_menu(false, Sort::default(), &[]));
        assert_eq!(bare[0], "Paste [Ctrl+V] (off)");
        let shown = here(&["New", "  Folder [Ctrl+Shift+N]", "  Empty File", "Open Terminal Here", "-", "Sort by"]);
        assert_eq!(bare[1..=shown.len()], shown, "no templates, no separator");
        assert_eq!(bare[shown.len() + 1], format!("  Name {SORT_ASCENDING}"));
    }

    /// A line of a menu: label, shortcut, command, enabled, checkbox.
    type Line<'a> = (&'a str, Option<&'a str>, Option<&'a Command>, bool, Option<bool>);

    fn lines(items: &[Item]) -> Vec<Line<'_>> {
        items
            .iter()
            .map(|item| {
                let command = match &item.kind {
                    Kind::Run(command) => Some(command),
                    _ => None,
                };
                (item.label.as_str(), item.shortcut, command, item.enabled, item.checked)
            })
            .collect()
    }

    #[test]
    fn the_header_menu_fits_closes_restores_and_shows_or_hides_each_column() {
        let shown = [true, true, false, true, true, true];
        let order = crate::ui::file_list::DEFAULT_ORDER;
        assert_eq!(
            lines(&column_menu(1, &shown, &order)),
            [
                ("Shift Column Left", None, Some(&Command::ShiftColumnLeft(1)), true, None),
                ("Shift Column Right", None, Some(&Command::ShiftColumnRight(1)), true, None),
                ("Size Column To Fit", None, Some(&Command::FitColumn(1)), true, None),
                ("Close Column", Some("Middle click"), Some(&Command::ToggleColumn(1)), true, None),
                ("Size All Columns To Fit", Some("Ctrl+Shift+F"), Some(&Command::FitAllColumns), true, None),
                ("", None, None, false, None),
                ("Restore Default Columns", None, Some(&Command::RestoreColumns), true, None),
                ("", None, None, false, None),
                ("Name", None, Some(&Command::ToggleColumn(0)), false, Some(true)),
                ("Type", None, Some(&Command::ToggleColumn(1)), true, Some(true)),
                ("Date Modified", None, Some(&Command::ToggleColumn(2)), true, Some(false)),
                ("Folders", None, Some(&Command::ToggleColumn(3)), true, Some(true)),
                ("Files", None, Some(&Command::ToggleColumn(4)), true, Some(true)),
                ("Size", None, Some(&Command::ToggleColumn(5)), true, Some(true)),
            ]
        );
        let name = column_menu(0, &shown, &order);
        let enabled = |items: &[Item], label: &str| items.iter().find(|i| i.label == label).expect("listed").enabled;
        assert!(!enabled(&name, "Close Column"), "Name is never closed");
        assert!(!enabled(&name, "Shift Column Left"), "Name is first");
        assert!(enabled(&name, "Shift Column Right"));
        let size = column_menu(5, &shown, &order);
        assert!(!enabled(&size, "Shift Column Right"), "Size is last");
        let moved = column_menu(5, &shown, &[0, 1, 2, 3, 5, 4]);
        assert!(enabled(&moved, "Shift Column Right"), "follows the order");
    }
}
