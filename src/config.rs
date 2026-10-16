//! Settings kept between runs, in a plain text file (`~/.config/lumen/config`,
//! `%APPDATA%\lumen\config` on Windows):
//! one `key = value` per line, readable and editable by hand.
//! Unknown keys and broken lines are ignored, so a bad edit never stops Lumen.

use std::path::{Path, PathBuf};

use crate::app::{Sort, SortColumn};
use crate::folder_views::{FolderViews, Look, Rule, Scope};
use crate::places::{VISIT_LIMIT, Visit};
use crate::ui::file_list::{ALL_SHOWN, DEFAULT_ORDER, DEFAULT_WIDTHS, MIN_WIDTH, Order, Shown, Widths};
use crate::ui::view_size::ViewSize;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub sort: Sort,
    pub show_hidden: bool,
    pub sidebar: bool,
    pub preview: bool,
    /// Inner size of the window in points; `None` for the default size.
    pub window: Option<[f32; 2]>,
    pub bookmarks: Vec<PathBuf>,
    /// The view slider's position: Details, Tree, List or the icon size.
    pub view: ViewSize,
    /// The tabs' folders of the last window closed, in bar order, and the
    /// one on screen: what `--restore` opens.
    pub tabs: Vec<PathBuf>,
    pub active_tab: usize,
    /// The list's column widths, in the list's column order.
    pub columns: Widths,
    /// Which of the list's columns are shown.
    pub shown_columns: Shown,
    /// The list's columns from left to right.
    pub column_order: Order,
    /// Folders visited, for the quick jump. Written when Lumen closes.
    pub visited: Vec<Visit>,
    /// Saved view settings per folder; `view` and `sort` are the default.
    pub folder_views: FolderViews,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sort: Sort::default(),
            show_hidden: false,
            sidebar: true,
            preview: false,
            window: None,
            bookmarks: Vec::new(),
            view: ViewSize::default(),
            tabs: Vec::new(),
            active_tab: 0,
            columns: DEFAULT_WIDTHS,
            shown_columns: ALL_SHOWN,
            column_order: DEFAULT_ORDER,
            visited: Vec::new(),
            folder_views: FolderViews::default(),
        }
    }
}

/// Smallest window size accepted from the file, the window's own minimum.
pub const MIN_WINDOW: [f32; 2] = [480.0, 320.0];

impl Config {
    /// Reads settings from `text`; anything missing or invalid keeps its default.
    pub fn parse(text: &str) -> Self {
        let mut config = Self::default();
        let mut rules = Vec::new();
        // Some editors start a UTF-8 file with a byte order mark.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "sort" => config.sort = parse_sort(value).unwrap_or(config.sort),
                "show_hidden" => config.show_hidden = parse_bool(value).unwrap_or(config.show_hidden),
                "sidebar" => config.sidebar = parse_bool(value).unwrap_or(config.sidebar),
                "preview" => config.preview = parse_bool(value).unwrap_or(config.preview),
                "window" => config.window = parse_window(value),
                "view" => config.view = value.parse::<u16>().map(|p| ViewSize::new(p.min(100) as u8)).unwrap_or(config.view),
                "bookmark" if !value.is_empty() => config.bookmarks.push(PathBuf::from(value)),
                "tab" if !value.is_empty() => config.tabs.push(PathBuf::from(value)),
                "active_tab" => config.active_tab = value.parse().unwrap_or(config.active_tab),
                "columns" => config.columns = parse_widths(value).unwrap_or(config.columns),
                "hidden_columns" => config.shown_columns = parse_hidden(value),
                "column_order" => config.column_order = parse_order(value).unwrap_or(config.column_order),
                "visited" => config.visited.extend(parse_visit(value)),
                "folder_view" => rules.extend(parse_folder_view(value)),
                _ => {}
            }
        }
        config.folder_views = FolderViews::from_rules(rules);
        config
    }

    /// The file's text. Bookmarks that cannot be written on one line (not
    /// UTF-8, or holding a line break) are left out.
    pub fn to_text(&self) -> String {
        let direction = if self.sort.descending { "desc" } else { "asc" };
        let mut text = String::from("# Lumen settings, written by Lumen. Edit them while Lumen is closed.\n");
        text += &format!("sort = {} {direction}\n", column_name(self.sort.column));
        text += &format!("show_hidden = {}\n", self.show_hidden);
        text += &format!("sidebar = {}\n", self.sidebar);
        text += &format!("preview = {}\n", self.preview);
        if let Some([w, h]) = self.window {
            text += &format!("window = {}x{}\n", w.round(), h.round());
        }
        text += &format!("view = {}\n", self.view.percent());
        let columns: Vec<String> = self.columns.iter().map(|w| w.round().to_string()).collect();
        text += &format!("columns = {}\n", columns.join(" "));
        let hidden: Vec<&str> = SORT_COLUMNS.into_iter().zip(self.shown_columns).filter(|(_, shown)| !shown).map(|(c, _)| column_name(c)).collect();
        if !hidden.is_empty() {
            text += &format!("hidden_columns = {}\n", hidden.join(" "));
        }
        if self.column_order != DEFAULT_ORDER {
            let order: Vec<&str> = self.column_order.iter().map(|&c| column_name(SORT_COLUMNS[c])).collect();
            text += &format!("column_order = {}\n", order.join(" "));
        }
        for path in self.bookmarks.iter().filter_map(|p| one_line(p)) {
            text += &format!("bookmark = {path}\n");
        }
        if !self.tabs.is_empty() {
            for path in self.tabs.iter().filter_map(|p| one_line(p)) {
                text += &format!("tab = {path}\n");
            }
            text += &format!("active_tab = {}\n", self.active_tab);
        }
        for visit in &self.visited {
            if let Some(path) = one_line(&visit.path) {
                text += &format!("visited = {} {} {path}\n", visit.score, visit.at);
            }
        }
        for rule in self.folder_views.rules() {
            if let Some(path) = one_line(&rule.folder) {
                let scope = match rule.scope {
                    Scope::Folder => "folder",
                    Scope::Tree => "tree",
                };
                let sort = rule.look.sort;
                let direction = if sort.descending { "desc" } else { "asc" };
                text += &format!("folder_view = {scope} {} {} {direction} {path}\n", rule.look.view.percent(), column_name(sort.column));
            }
        }
        text
    }

    /// Reads `path`; a missing file gives the defaults. Bytes that are not
    /// UTF-8 only spoil their own line. Any other read error is returned, so
    /// the caller never overwrites a file it could not read.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self::parse(&String::from_utf8_lossy(&bytes))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    /// `disk` with the changes from `before` to `after` applied: what another
    /// Lumen window saved meanwhile is kept, only this window's edits win.
    pub fn merge(disk: &Self, before: &Self, after: &Self) -> Self {
        let mut merged = disk.clone();
        if before.sort != after.sort {
            merged.sort = after.sort;
        }
        if before.show_hidden != after.show_hidden {
            merged.show_hidden = after.show_hidden;
        }
        if before.sidebar != after.sidebar {
            merged.sidebar = after.sidebar;
        }
        if before.preview != after.preview {
            merged.preview = after.preview;
        }
        if before.window != after.window {
            merged.window = after.window;
        }
        if before.view != after.view {
            merged.view = after.view;
        }
        if before.columns != after.columns {
            merged.columns = after.columns;
        }
        if before.shown_columns != after.shown_columns {
            merged.shown_columns = after.shown_columns;
        }
        if before.column_order != after.column_order {
            merged.column_order = after.column_order;
        }
        if (&before.tabs, before.active_tab) != (&after.tabs, after.active_tab) {
            (merged.tabs, merged.active_tab) = (after.tabs.clone(), after.active_tab);
        }
        if before.visited != after.visited {
            merged.visited = merge_visits(&disk.visited, &after.visited);
        }
        merged.folder_views = FolderViews::merge(&disk.folder_views, &before.folder_views, &after.folder_views);
        merged.bookmarks.retain(|p| !before.bookmarks.contains(p) || after.bookmarks.contains(p));
        for path in &after.bookmarks {
            if !before.bookmarks.contains(path) && !merged.bookmarks.contains(path) {
                merged.bookmarks.push(path.clone());
            }
        }
        merged
    }

    /// Writes `path` through a temporary file and a rename, so a crash midway
    /// leaves the old file whole.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Its own name per process: two Lumen windows may save at once.
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = path.with_file_name(format!("{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }
}

/// Both windows' visits, each folder's latest, most recent first; past the
/// limit the weakest go, as when visiting.
fn merge_visits(disk: &[Visit], here: &[Visit]) -> Vec<Visit> {
    let mut merged = disk.to_vec();
    for visit in here {
        match merged.iter_mut().find(|v| v.path == visit.path) {
            Some(kept) if kept.at < visit.at => *kept = visit.clone(),
            Some(_) => {}
            None => merged.push(visit.clone()),
        }
    }
    // Every score fades at the same rate, so which is stronger does not
    // depend on when they are compared: the latest visit will do.
    let now = merged.iter().map(|v| v.at).max().unwrap_or(0);
    merged.sort_by(|a, b| crate::places::faded(b, now).total_cmp(&crate::places::faded(a, now)));
    merged.truncate(VISIT_LIMIT);
    merged.sort_by_key(|v| std::cmp::Reverse(v.at));
    merged
}

/// `folder_view = <folder|tree> <view percent> <sort column> <asc|desc> <path>`.
fn parse_folder_view(value: &str) -> Option<Rule> {
    let mut rest = value;
    let mut word = || {
        let (word, after) = rest.trim_start().split_once(char::is_whitespace)?;
        rest = after;
        Some(word)
    };
    let scope = match word()? {
        "folder" => Scope::Folder,
        "tree" => Scope::Tree,
        _ => return None,
    };
    let view = ViewSize::new(word()?.parse::<u16>().ok()?.min(100) as u8);
    let sort = parse_sort(&format!("{} {}", word()?, word()?))?;
    let folder = rest.trim();
    (!folder.is_empty()).then(|| Rule { folder: PathBuf::from(folder), scope, look: Look { view, sort } })
}

/// `visited = <score> <when, Unix seconds> <path>`.
fn parse_visit(value: &str) -> Option<Visit> {
    let (score, rest) = value.split_once(char::is_whitespace)?;
    let (at, path) = rest.trim_start().split_once(char::is_whitespace)?;
    let score = score.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0)?;
    let at = at.parse().ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| Visit { path: PathBuf::from(path), score, at })
}

/// A path as a value: the rest of its line, trimmed, so it must not hold a
/// break or start/end with spaces.
fn one_line(path: &Path) -> Option<&str> {
    path.to_str().filter(|p| !p.contains(['\n', '\r']) && p.trim() == *p)
}

/// The settings to start with and where to save them.
pub struct Settings {
    pub config: Config,
    /// `None`: nothing is saved this session (no home, or the file is unreadable).
    pub path: Option<PathBuf>,
    /// Why the file could not be read, to show the user.
    pub error: Option<String>,
}

/// Loads the settings at `path`. If the file exists but cannot be read, Lumen
/// starts with defaults and saves nothing, so the file is left as it was.
pub fn open(path: Option<PathBuf>) -> Settings {
    let Some(path) = path else {
        return Settings { config: Config::default(), path: None, error: None };
    };
    match Config::load(&path) {
        Ok(config) => Settings { config, path: Some(path), error: None },
        Err(err) => Settings {
            config: Config::default(),
            path: None,
            error: Some(format!("Cannot read settings {}: {err}. Changes will not be saved", path.display())),
        },
    }
}

/// `$XDG_CONFIG_HOME/lumen/config`, by default `~/.config/lumen/config`.
pub fn default_path(home: Option<&Path>, config_home: Option<&Path>) -> Option<PathBuf> {
    let base = match config_home {
        Some(dir) => dir.to_path_buf(),
        None => home?.join(".config"),
    };
    Some(base.join("lumen/config"))
}

/// Where the settings file lives, and on Windows where older versions kept it.
#[derive(Debug, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    /// Where Lumen 0.1 kept it (`%USERPROFILE%\.config\lumen\config`, or under
    /// %XDG_CONFIG_HOME%), moved to `path` once.
    pub old: Option<PathBuf>,
}

/// Windows: `%APPDATA%\lumen\config`, where Windows apps keep their settings.
/// Elsewhere, or without %APPDATA%: [`default_path`].
pub fn location(windows: bool, home: Option<&Path>, config_home: Option<&Path>, appdata: Option<&Path>) -> Option<Location> {
    match appdata.filter(|_| windows) {
        Some(appdata) => Some(Location { path: appdata.join("lumen/config"), old: default_path(home, config_home) }),
        None => default_path(home, config_home).map(|path| Location { path, old: None }),
    }
}

impl Location {
    /// The file to use: `path`, after moving the old file there if `path` does
    /// not exist yet. If the move fails the old file stays where it was and is
    /// the one used, so no bookmark or tab is lost.
    pub fn resolve(self) -> PathBuf {
        let Some(old) = self.old.filter(|old| !self.path.exists() && old.is_file()) else {
            return self.path;
        };
        let moved = self.path.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
            && std::fs::rename(&old, &self.path).is_ok();
        if !moved {
            // Another window starting at the same time may have moved it first.
            return if self.path.exists() { self.path } else { old };
        }
        // Only removed when empty: the old `lumen` folder, never `.config`.
        if let Some(dir) = old.parent() {
            let _ = std::fs::remove_dir(dir);
        }
        self.path
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// "size desc", "name asc".
/// Every column, in the list's order.
const SORT_COLUMNS: [SortColumn; 6] = [
    SortColumn::Name,
    SortColumn::Type,
    SortColumn::Modified,
    SortColumn::Folders,
    SortColumn::Files,
    SortColumn::Size,
];

fn parse_sort(value: &str) -> Option<Sort> {
    let mut words = value.split_whitespace();
    let name = words.next()?;
    let column = SORT_COLUMNS.into_iter().find(|&c| column_name(c) == name)?;
    let descending = match words.next()? {
        "asc" => false,
        "desc" => true,
        _ => return None,
    };
    words.next().is_none().then_some(Sort { column, descending })
}

/// The columns named (as in `sort`) are hidden; unknown names are skipped
/// and Name is always shown.
fn parse_hidden(value: &str) -> Shown {
    let mut shown = ALL_SHOWN;
    for word in value.split_whitespace() {
        if let Some(i) = SORT_COLUMNS.into_iter().position(|c| column_name(c) == word) {
            shown[i] = i == 0;
        }
    }
    shown
}

/// Every column's name once, left to right.
fn parse_order(value: &str) -> Option<Order> {
    let order: Vec<usize> = value
        .split_whitespace()
        .map(|word| SORT_COLUMNS.into_iter().position(|c| column_name(c) == word))
        .collect::<Option<_>>()?;
    let order: Order = order.try_into().ok()?;
    let mut sorted = order;
    sorted.sort_unstable();
    (sorted == DEFAULT_ORDER).then_some(order)
}

/// One width per column, separated by spaces; one too narrow gets the minimum.
fn parse_widths(value: &str) -> Option<Widths> {
    let widths: Vec<f32> = value.split_whitespace().map(|w| w.parse().ok()).collect::<Option<_>>()?;
    let widths: Widths = widths.try_into().ok()?;
    widths.iter().all(|w| w.is_finite()).then(|| widths.map(|w| w.max(MIN_WIDTH)))
}

/// "1200x800", at least the window's minimum size.
fn parse_window(value: &str) -> Option<[f32; 2]> {
    let (w, h) = value.split_once('x')?;
    let size = [w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?];
    let fits = size.iter().zip(MIN_WINDOW).all(|(&got, min)| got.is_finite() && got >= min && got <= 16384.0);
    fits.then_some(size)
}

fn column_name(column: SortColumn) -> &'static str {
    match column {
        SortColumn::Name => "name",
        SortColumn::Type => "type",
        SortColumn::Modified => "modified",
        SortColumn::Folders => "folders",
        SortColumn::Files => "files",
        SortColumn::Size => "size",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        Config {
            sort: Sort { column: SortColumn::Size, descending: true },
            show_hidden: true,
            sidebar: false,
            preview: true,
            window: Some([1200.0, 800.0]),
            bookmarks: vec![PathBuf::from("/home/u/Descargas"), PathBuf::from("/srv/my files")],
            view: ViewSize::new(65),
            tabs: vec![PathBuf::from("/home/u"), PathBuf::from("/srv/my files")],
            active_tab: 1,
            columns: [250.0, 90.0, 140.0, 55.0, 60.0, 95.0],
            shown_columns: [true, false, true, true, false, true],
            column_order: [1, 0, 2, 3, 5, 4],
            visited: vec![
                Visit { path: PathBuf::from("/home/u/Descargas"), score: 2.5, at: 1_760_000_000 },
                Visit { path: PathBuf::from("/srv/my files"), score: 0.1, at: 1 },
            ],
            folder_views: FolderViews::default(),
        }
    }

    #[test]
    fn settings_survive_a_round_trip_as_readable_lines() {
        let text = sample().to_text();
        assert!(text.contains("sort = size desc\n"), "{text}");
        assert!(text.contains("show_hidden = true\n"), "{text}");
        assert!(text.contains("window = 1200x800\n"), "{text}");
        assert!(text.contains("view = 65\n"), "{text}");
        assert!(!text.contains("list_zoom"), "{text}");
        assert!(text.contains("columns = 250 90 140 55 60 95\n"), "{text}");
        assert!(text.contains("hidden_columns = type files\n"), "{text}");
        assert!(text.contains("column_order = type name modified folders size files\n"), "{text}");
        assert!(text.contains("bookmark = /srv/my files\n"), "{text}");
        assert!(text.contains("tab = /home/u\ntab = /srv/my files\nactive_tab = 1\n"), "{text}");
        assert!(text.contains("visited = 2.5 1760000000 /home/u/Descargas\nvisited = 0.1 1 /srv/my files\n"), "{text}");
        assert_eq!(Config::parse(&text), sample());
        assert_eq!(Config::parse(&Config::default().to_text()), Config::default());
    }

    #[test]
    fn a_broken_visited_line_is_skipped() {
        let text = "visited = x 1 /a\nvisited = 1 /a\nvisited = 1 2\nvisited = -1 2 /a\nvisited = NaN 2 /a\nvisited = 1.5 2 /a b\n";
        assert_eq!(Config::parse(text).visited, [Visit { path: PathBuf::from("/a b"), score: 1.5, at: 2 }]);
    }

    #[test]
    fn the_session_is_written_only_when_there_is_one() {
        let text = Config::default().to_text();
        assert!(!text.contains("tab ="), "{text}");
        assert_eq!(Config::parse("tab = /a\nactive_tab = x\ntab =\n").tabs, [PathBuf::from("/a")], "empty lines skipped");
        assert_eq!(Config::parse("tab = /a\nactive_tab = x\n").active_tab, 0, "a bad number keeps the first");
    }

    #[test]
    fn missing_broken_and_unknown_lines_keep_the_defaults() {
        assert_eq!(Config::parse(""), Config::default());
        let text = "\
# a comment
sort = colour up
show_hidden = maybe
sidebar
window = 10x10
window_maximized = true
preview = true
bookmark =
";
        let config = Config::parse(text);
        assert_eq!(config, Config { preview: true, ..Config::default() });
        assert_eq!(Config::parse("window = 100x2000").window, None, "below the window minimum");
        assert_eq!(Config::parse("window = 800x600").window, Some([800.0, 600.0]));
        assert_eq!(Config::parse("  sort=name   asc ").sort, Sort::default(), "spaces are tolerated");
    }

    #[cfg(unix)]
    #[test]
    fn bookmarks_that_do_not_fit_on_a_line_are_left_out() {
        use std::os::unix::ffi::OsStrExt;
        let config = Config {
            bookmarks: vec![
                PathBuf::from(std::ffi::OsStr::from_bytes(b"/bad/\xff")),
                PathBuf::from("/two\nlines"),
                PathBuf::from("/ok"),
            ],
            ..Config::default()
        };
        assert_eq!(Config::parse(&config.to_text()).bookmarks, [PathBuf::from("/ok")]);
    }

    #[test]
    fn an_older_file_without_a_view_opens_details_and_its_row_zoom_is_ignored() {
        let c = Config::parse("sort = size desc\nsidebar = false\nlist_zoom = 3\nbookmark = /srv\n");
        assert_eq!((c.view, c.sidebar, c.bookmarks.len()), (ViewSize::default(), false, 1));
        assert!(!c.to_text().contains("list_zoom"), "the old key is dropped on the next save");
    }

    #[test]
    fn folder_views_are_written_one_per_line_and_read_back() {
        let size_desc = Sort { column: SortColumn::Size, descending: true };
        let config = Config {
            folder_views: FolderViews::from_rules(vec![
                Rule { folder: PathBuf::from("/home/me/My Photos"), scope: Scope::Tree, look: Look { view: ViewSize::new(100), sort: size_desc } },
                Rule { folder: PathBuf::from("/srv"), scope: Scope::Folder, look: Look { view: ViewSize::new(20), sort: Sort::default() } },
            ]),
            ..Config::default()
        };
        let text = config.to_text();
        assert!(text.contains("folder_view = tree 100 size desc /home/me/My Photos\n"), "{text}");
        assert!(text.contains("folder_view = folder 20 name asc /srv\n"), "{text}");
        assert_eq!(Config::parse(&text).folder_views, config.folder_views);
        assert!(!Config::default().to_text().contains("folder_view"), "no rules, no lines");
    }

    #[test]
    fn a_broken_folder_view_line_is_skipped_and_the_others_are_kept() {
        let text = "folder_view = tree 100 size desc /ok\n\
                    folder_view = forever 100 size desc /bad-scope\n\
                    folder_view = folder big name asc /bad-view\n\
                    folder_view = folder 20 colour asc /bad-column\n\
                    folder_view = folder 20 name up /bad-direction\n\
                    folder_view = folder 20 name asc\n\
                    folder_view = folder 250 name asc /capped\n";
        let views = Config::parse(text).folder_views;
        let folders: Vec<_> = views.rules().iter().map(|r| (r.folder.to_string_lossy().into_owned(), r.look.view.percent())).collect();
        assert_eq!(folders, [("/ok".to_string(), 100), ("/capped".to_string(), 100)]);
    }

    #[test]
    fn merging_settings_keeps_the_other_windows_folder_views() {
        let rule = |folder: &str, percent| Rule { folder: PathBuf::from(folder), scope: Scope::Folder, look: Look { view: ViewSize::new(percent), sort: Sort::default() } };
        let before = Config::default();
        let after = Config { folder_views: FolderViews::from_rules(vec![rule("/mine", 30)]), ..before.clone() };
        let disk = Config { folder_views: FolderViews::from_rules(vec![rule("/theirs", 65)]), ..before.clone() };
        let merged = Config::merge(&disk, &before, &after);
        assert_eq!(merged.folder_views.rules(), [rule("/theirs", 65), rule("/mine", 30)]);
        let untouched = Config::merge(&disk, &before, &before);
        assert_eq!(untouched.folder_views, disk.folder_views, "no edits here: the file's rules stay");
    }

    #[test]
    fn a_broken_view_keeps_details_and_one_past_100_is_capped() {
        assert_eq!(Config::parse("view = 40").view, ViewSize::new(40));
        assert_eq!(Config::parse("view = 12").view, ViewSize::new(10), "below the grid: the nearest step");
        assert_eq!(Config::parse("view = 250").view, ViewSize::new(100));
        assert_eq!(Config::parse("view = -1").view, ViewSize::default());
        assert_eq!(Config::parse("view = big").view, ViewSize::default());
        let before = Config::default();
        let after = Config { view: ViewSize::new(65), ..before.clone() };
        let disk = Config { sidebar: false, ..before.clone() };
        let merged = Config::merge(&disk, &before, &after);
        assert_eq!((merged.view, merged.sidebar), (ViewSize::new(65), false), "this window's view, the other's panel");
    }

    #[test]
    fn column_widths_that_are_missing_short_or_broken_keep_the_defaults() {
        assert_eq!(Config::parse("list_zoom = 1\n").columns, DEFAULT_WIDTHS, "an older file");
        for bad in ["columns = 250 90 140", "columns = 250 90 140 55 60 95 7", "columns = 250 x 140 55 60 95", "columns = 250 inf 140 55 60 95"] {
            assert_eq!(Config::parse(bad).columns, DEFAULT_WIDTHS, "{bad:?}");
        }
        assert_eq!(
            Config::parse("columns = 250 10 140 55 60 95\n").columns,
            [250.0, MIN_WIDTH, 140.0, 55.0, 60.0, 95.0],
            "a column too narrow gets the minimum"
        );
        assert!(Config::default().to_text().contains("columns = 320 110 130 60 60 80\n"));
    }

    #[test]
    fn hidden_columns_are_named_and_name_is_never_hidden() {
        assert!(!Config::default().to_text().contains("hidden_columns"), "all shown: no line");
        assert_eq!(Config::parse("list_zoom = 1\n").shown_columns, ALL_SHOWN, "an older file");
        let parsed = Config::parse("hidden_columns = name size bogus modified\n").shown_columns;
        assert_eq!(parsed, [true, true, false, true, true, false], "unknown names skipped, Name always shown");
        let before = Config::default();
        let after = Config { shown_columns: parsed, ..before.clone() };
        assert_eq!(Config::merge(&before, &before, &after).shown_columns, parsed);
        assert_eq!(Config::merge(&after, &before, &before).shown_columns, parsed, "another window's stays");
    }

    #[test]
    fn a_column_order_that_is_not_every_column_once_keeps_the_default() {
        assert!(!Config::default().to_text().contains("column_order"), "default order: no line");
        assert_eq!(Config::parse("list_zoom = 1\n").column_order, DEFAULT_ORDER, "an older file");
        for bad in [
            "column_order = type name modified folders files",
            "column_order = type name modified folders files size size",
            "column_order = type type modified folders files size",
            "column_order = type name bogus folders files size",
        ] {
            assert_eq!(Config::parse(bad).column_order, DEFAULT_ORDER, "{bad:?}");
        }
        let order = Config::parse("column_order = size files folders modified type name").column_order;
        assert_eq!(order, [5, 4, 3, 2, 1, 0]);
        let before = Config::default();
        let after = Config { column_order: order, ..before.clone() };
        assert_eq!(Config::merge(&before, &before, &after).column_order, order);
        assert_eq!(Config::merge(&after, &before, &before).column_order, order, "another window's stays");
    }

    #[test]
    fn save_then_load_and_a_missing_file_gives_defaults() {
        let dir = crate::fs::ops::tests::fresh_fixture("config-save");
        let path = dir.join("lumen/config");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        sample().save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), sample());
        let left: Vec<_> = std::fs::read_dir(dir.join("lumen")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["config"], "the temporary file is renamed away");
        Config::default().save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), Config::default(), "saving again replaces it");
    }

    #[test]
    fn a_line_that_is_not_utf8_spoils_only_itself() {
        let dir = crate::fs::ops::tests::fresh_fixture("config-latin1");
        let path = dir.join("config");
        std::fs::write(&path, b"sidebar = false\nbookmark = /srv\nbookmark = /home/u/M\xfasica\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(!config.sidebar);
        assert_eq!(config.bookmarks[0], PathBuf::from("/srv"));
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_line() {
        assert!(!Config::parse("\u{feff}sidebar = false\n").sidebar);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_an_error_and_opening_it_saves_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::fs::ops::tests::fresh_fixture("config-unreadable");
        let path = dir.join("config");
        std::fs::write(&path, "bookmark = /srv\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let loaded = Config::load(&path);
        let settings = open(Some(path.clone()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(loaded.is_err());
        assert_eq!(settings.path, None, "nothing will be written over it");
        assert_eq!(settings.config, Config::default());
        let error = settings.error.expect("the user is told");
        assert!(error.starts_with("Cannot read settings"), "{error}");
        let missing = open(Some(dir.join("nope/config")));
        assert_eq!((missing.path, missing.error), (Some(dir.join("nope/config")), None), "a first run saves");
    }

    #[test]
    fn merging_keeps_what_another_window_saved() {
        let disk = Config {
            bookmarks: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            sort: Sort { column: SortColumn::Size, descending: false },
            ..Config::default()
        };
        // This window started with only /a, removed it, added /c and turned the sidebar off.
        let before = Config { bookmarks: vec![PathBuf::from("/a")], ..Config::default() };
        let after = Config { bookmarks: vec![PathBuf::from("/c")], sidebar: false, ..Config::default() };
        let merged = Config::merge(&disk, &before, &after);
        assert_eq!(merged.bookmarks, [PathBuf::from("/b"), PathBuf::from("/c")]);
        assert!(!merged.sidebar, "this window's change wins");
        assert_eq!(merged.sort.column, SortColumn::Size, "untouched here: the other window's sort stays");
        let resized = Config { window: Some([900.0, 600.0]), ..before.clone() };
        assert_eq!(Config::merge(&disk, &before, &resized).window, Some([900.0, 600.0]));
        let columns = Config { columns: [200.0, 90.0, 140.0, 55.0, 60.0, 95.0], ..before.clone() };
        assert_eq!(Config::merge(&disk, &before, &columns).columns, columns.columns, "resized here");
        let other = Config { columns: [500.0, 90.0, 140.0, 55.0, 60.0, 95.0], ..disk.clone() };
        assert_eq!(Config::merge(&other, &before, &after).columns, other.columns, "resized in another window");
    }

    #[test]
    fn merging_visits_keeps_each_folders_latest_visit_from_either_window() {
        let visit = |path: &str, at| Visit { path: PathBuf::from(path), score: at as f64, at };
        let disk = Config { visited: vec![visit("/a", 10), visit("/b", 10)], ..Config::default() };
        let after = Config { visited: vec![visit("/a", 20), visit("/c", 20)], ..Config::default() };
        let merged = Config::merge(&disk, &Config::default(), &after);
        assert_eq!(merged.visited, [visit("/a", 20), visit("/c", 20), visit("/b", 10)]);
        let older = Config { visited: vec![visit("/b", 5)], ..Config::default() };
        assert_eq!(Config::merge(&disk, &Config::default(), &older).visited, disk.visited, "the other window's later visit stays");
        assert_eq!(Config::merge(&disk, &after, &after).visited, disk.visited, "nothing visited here");
    }

    #[test]
    fn merging_visits_past_the_limit_drops_the_weakest_not_the_oldest() {
        use crate::places::{VISIT_HALF_LIFE, VISIT_LIMIT};
        let now = 10 * VISIT_HALF_LIFE;
        let weak = |i| Visit { path: PathBuf::from(format!("/weak{i}")), score: 1.0, at: now };
        let disk = Config { visited: (0..VISIT_LIMIT).map(weak).collect(), ..Config::default() };
        // Used a lot a half-life ago: still far stronger than one visit now.
        let strong = Visit { path: PathBuf::from("/strong"), score: 50.0, at: now - VISIT_HALF_LIFE };
        let after = Config { visited: vec![strong.clone()], ..Config::default() };
        let merged = Config::merge(&disk, &Config::default(), &after).visited;
        assert_eq!(merged.len(), VISIT_LIMIT);
        assert!(merged.contains(&strong), "the strongest folder stays");
        assert!(merged.iter().filter(|v| v.score == 1.0).count() == VISIT_LIMIT - 1, "one weak visit goes");
        assert!(merged.windows(2).all(|w| w[0].at >= w[1].at), "still most recent first");
    }

    #[test]
    fn the_file_lives_under_the_config_home() {
        let home = Path::new("/home/u");
        assert_eq!(default_path(Some(home), None), Some(PathBuf::from("/home/u/.config/lumen/config")));
        assert_eq!(default_path(Some(home), Some(Path::new("/cfg"))), Some(PathBuf::from("/cfg/lumen/config")));
        assert_eq!(default_path(None, None), None);
    }

    #[test]
    fn windows_keeps_the_file_in_appdata_and_linux_under_the_config_home() {
        let home = Path::new("/home/u");
        let appdata = Path::new("/appdata");
        let linux = location(false, Some(home), Some(Path::new("/cfg")), Some(appdata));
        assert_eq!(linux, Some(Location { path: PathBuf::from("/cfg/lumen/config"), old: None }));
        let windows = location(true, Some(home), Some(Path::new("/cfg")), Some(appdata));
        let old = Some(PathBuf::from("/cfg/lumen/config"));
        assert_eq!(windows, Some(Location { path: PathBuf::from("/appdata/lumen/config"), old }), "where 0.1 kept it");
        let old = Some(PathBuf::from("/home/u/.config/lumen/config"));
        let windows = location(true, Some(home), None, Some(appdata));
        assert_eq!(windows, Some(Location { path: PathBuf::from("/appdata/lumen/config"), old }));
        assert_eq!(location(true, Some(home), None, None), location(false, Some(home), None, None), "no %APPDATA%");
    }

    #[test]
    fn the_old_file_moves_to_the_new_place_once() {
        let dir = crate::fs::ops::tests::fresh_fixture("config-move");
        let old = dir.join("profile/.config/lumen/config");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "bookmark = /x\n").unwrap();
        let new = dir.join("appdata/lumen/config");

        let at = Location { path: new.clone(), old: Some(old.clone()) }.resolve();
        assert_eq!(at, new);
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "bookmark = /x\n", "bookmarks and tabs kept");
        assert!(!old.exists(), "moved, not copied");
        assert!(!dir.join("profile/.config/lumen").exists(), "its emptied folder goes too");
        assert!(dir.join("profile/.config").exists(), ".config may hold other apps' settings");
    }

    #[test]
    fn an_existing_new_file_wins_and_the_old_one_stays() {
        let dir = crate::fs::ops::tests::fresh_fixture("config-both");
        let (old, new) = (dir.join("old/config"), dir.join("new/config"));
        for (path, text) in [(&old, "sidebar = false\n"), (&new, "sidebar = true\n")] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        assert_eq!(Location { path: new.clone(), old: Some(old.clone()) }.resolve(), new);
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "sidebar = true\n");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "sidebar = false\n", "left alone");
    }

    #[test]
    fn without_an_old_file_nothing_is_created() {
        let dir = crate::fs::ops::tests::fresh_fixture("config-fresh");
        let new = dir.join("appdata/lumen/config");
        assert_eq!(Location { path: new.clone(), old: Some(dir.join("old/config")) }.resolve(), new);
        assert!(!dir.join("appdata").exists(), "written on the first save, as before");
    }

    #[test]
    fn a_window_that_loses_the_move_race_uses_the_new_file() {
        for i in 0..50 {
            let dir = crate::fs::ops::tests::fresh_fixture(&format!("config-race-{i}"));
            let (old, new) = (dir.join("old/lumen/config"), dir.join("appdata/lumen/config"));
            std::fs::create_dir_all(old.parent().unwrap()).unwrap();
            std::fs::write(&old, "bookmark = /r\n").unwrap();
            let start = std::sync::Arc::new(std::sync::Barrier::new(2));
            let windows: Vec<_> = (0..2)
                .map(|_| {
                    let (start, old, new) = (start.clone(), old.clone(), new.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        Location { path: new, old: Some(old) }.resolve()
                    })
                })
                .collect();
            for window in windows {
                assert_eq!(window.join().unwrap(), new, "both windows read the moved file");
            }
            assert_eq!(std::fs::read_to_string(&new).unwrap(), "bookmark = /r\n");
        }
    }

    #[test]
    fn a_failed_move_keeps_reading_the_old_file() {
        let dir = crate::fs::ops::tests::fresh_fixture("config-move-fails");
        let old = dir.join("old/config");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "sidebar = false\n").unwrap();
        std::fs::write(dir.join("appdata"), "a file where the folder should be").unwrap();
        let new = dir.join("appdata/lumen/config");

        assert_eq!(Location { path: new, old: Some(old.clone()) }.resolve(), old);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "sidebar = false\n", "untouched");
    }
}
