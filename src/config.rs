//! Settings kept between runs, in a plain text file (`~/.config/lumen/config`):
//! one `key = value` per line, readable and editable by hand.
//! Unknown keys and broken lines are ignored, so a bad edit never stops Lumen.

use std::path::{Path, PathBuf};

use crate::app::{Sort, SortColumn};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub sort: Sort,
    pub show_hidden: bool,
    pub sidebar: bool,
    pub preview: bool,
    /// Inner size of the window in points; `None` for the default size.
    pub window: Option<[f32; 2]>,
    pub bookmarks: Vec<PathBuf>,
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
        }
    }
}

/// Smallest window size accepted from the file, the window's own minimum.
pub const MIN_WINDOW: [f32; 2] = [480.0, 320.0];

impl Config {
    /// Reads settings from `text`; anything missing or invalid keeps its default.
    pub fn parse(text: &str) -> Self {
        let mut config = Self::default();
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
                "bookmark" if !value.is_empty() => config.bookmarks.push(PathBuf::from(value)),
                _ => {}
            }
        }
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
        for path in &self.bookmarks {
            // A value is the rest of its line, trimmed: it must not hold a break
            // or start/end with spaces.
            if let Some(path) = path.to_str().filter(|p| !p.contains(['\n', '\r']) && p.trim() == *p) {
                text += &format!("bookmark = {path}\n");
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

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// "size desc", "name asc".
fn parse_sort(value: &str) -> Option<Sort> {
    let mut words = value.split_whitespace();
    let name = words.next()?;
    let column = [
        SortColumn::Name,
        SortColumn::Type,
        SortColumn::Modified,
        SortColumn::Folders,
        SortColumn::Files,
        SortColumn::Size,
    ]
    .into_iter()
    .find(|&c| column_name(c) == name)?;
    let descending = match words.next()? {
        "asc" => false,
        "desc" => true,
        _ => return None,
    };
    words.next().is_none().then_some(Sort { column, descending })
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
        }
    }

    #[test]
    fn settings_survive_a_round_trip_as_readable_lines() {
        let text = sample().to_text();
        assert!(text.contains("sort = size desc\n"), "{text}");
        assert!(text.contains("show_hidden = true\n"), "{text}");
        assert!(text.contains("window = 1200x800\n"), "{text}");
        assert!(text.contains("bookmark = /srv/my files\n"), "{text}");
        assert_eq!(Config::parse(&text), sample());
        assert_eq!(Config::parse(&Config::default().to_text()), Config::default());
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
    }

    #[test]
    fn the_file_lives_under_the_config_home() {
        let home = Path::new("/home/u");
        assert_eq!(default_path(Some(home), None), Some(PathBuf::from("/home/u/.config/lumen/config")));
        assert_eq!(default_path(Some(home), Some(Path::new("/cfg"))), Some(PathBuf::from("/cfg/lumen/config")));
        assert_eq!(default_path(None, None), None);
    }
}
