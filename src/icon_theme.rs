//! The user's icon theme, found the way freedesktop desktops find it (the
//! Icon Theme Specification): the theme the desktop names, then the themes
//! it inherits, hicolor last, each searched for the icon closest in size.
//! Not on Windows: there the shell gives every icon.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where icon themes live, most important first: ~/.icons,
/// $XDG_DATA_HOME/icons, then each of $XDG_DATA_DIRS/icons.
pub fn base_dirs(home: &Path, data: &[PathBuf]) -> Vec<PathBuf> {
    std::iter::once(home.join(".icons")).chain(data.iter().map(|d| d.join("icons"))).collect()
}

/// The icon theme the desktop uses. KDE keeps it in kdeglobals; GNOME and
/// most others in gsettings (`gsettings` is asked only when needed), then
/// GTK's settings.ini.
pub fn theme_name(desktop: &str, config_home: &Path, gsettings: impl FnOnce() -> Option<String>) -> Option<String> {
    let read = |path: &str| std::fs::read_to_string(config_home.join(path)).unwrap_or_default();
    if desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE")) {
        let set = ini_value(&read("kdeglobals"), "Icons", "Theme");
        return Some(set.unwrap_or_else(|| "breeze".to_owned()));
    }
    gsettings()
        .or_else(|| ini_value(&read("gtk-3.0/settings.ini"), "Settings", "gtk-icon-theme-name"))
        .or_else(|| ini_value(&read("gtk-4.0/settings.ini"), "Settings", "gtk-icon-theme-name"))
}

/// What `gsettings get org.gnome.desktop.interface icon-theme` printed.
pub fn gsettings_value(out: &str) -> Option<String> {
    let value = out.trim().trim_matches('\'');
    (!value.is_empty()).then(|| value.to_owned())
}

/// Asks gsettings for GNOME's icon theme; `None` when it is not installed.
pub fn ask_gsettings() -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| gsettings_value(&String::from_utf8_lossy(&out.stdout)))?
}

/// The sections of an .ini-like file (index.theme, kdeglobals), each a map
/// of its keys, in file order.
fn parse_ini(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.to_owned(), HashMap::new()));
        } else if let (Some((key, value)), Some((_, keys))) = (line.split_once('='), sections.last_mut())
            && !line.starts_with('#')
        {
            keys.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    sections
}

fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    parse_ini(text)
        .into_iter()
        .find(|(name, _)| name == section)
        .and_then(|(_, mut keys)| keys.remove(key))
        .filter(|v| !v.is_empty())
}

/// A comma-separated list, empty items dropped.
fn list(value: Option<&String>) -> Vec<String> {
    value.map_or_else(Vec::new, |v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
}

/// One theme directory's sizes, from index.theme.
#[derive(Debug, Clone, PartialEq)]
struct Dir {
    path: String,
    size: u32,
    scale: u32,
    kind: Kind,
    min: u32,
    max: u32,
    threshold: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Fixed,
    Scalable,
    Threshold,
}

impl Dir {
    fn from_keys(path: &str, keys: &HashMap<String, String>) -> Option<Self> {
        let number = |key: &str| keys.get(key).and_then(|v| v.parse::<u32>().ok());
        let size = number("Size")?;
        let kind = match keys.get("Type").map(String::as_str) {
            Some("Fixed") => Kind::Fixed,
            Some("Scalable") => Kind::Scalable,
            _ => Kind::Threshold,
        };
        Some(Self {
            path: path.to_owned(),
            size,
            scale: number("Scale").unwrap_or(1).max(1),
            kind,
            min: number("MinSize").unwrap_or(size),
            max: number("MaxSize").unwrap_or(size),
            threshold: number("Threshold").unwrap_or(2),
        })
    }

    /// The pixel range the directory's icons are drawn for.
    fn range(&self) -> (u32, u32) {
        let (low, high) = match self.kind {
            Kind::Fixed => (self.size, self.size),
            Kind::Scalable => (self.min, self.max),
            Kind::Threshold => (self.size.saturating_sub(self.threshold), self.size + self.threshold),
        };
        (low * self.scale, high * self.scale)
    }

    /// How far `size` pixels are from what the directory holds; 0 inside it.
    fn distance(&self, size: u32) -> u32 {
        let (low, high) = self.range();
        if size < low {
            low - size
        } else {
            size.saturating_sub(high)
        }
    }
}

/// Image formats Lumen can draw, in the order the spec prefers them.
const EXTENSIONS: [&str; 2] = ["png", "svg"];

/// A loaded theme chain, ready to answer lookups.
pub struct IconTheme {
    themes: Vec<Theme>,
}

struct Theme {
    dirs: Vec<Dir>,
    /// Icon name -> (index into `dirs`, file), every file the theme has.
    files: HashMap<String, Vec<(usize, PathBuf)>>,
}

impl IconTheme {
    /// Loads `name` and every theme it inherits (hicolor last) from `bases`.
    pub fn load(name: &str, bases: &[PathBuf]) -> Self {
        let mut themes = Vec::new();
        let mut seen = Vec::new();
        let mut todo = vec![name.to_owned()];
        // Depth first: a theme, then what it inherits in order; each once.
        while let Some(name) = todo.pop() {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let Some(text) = bases.iter().find_map(|b| std::fs::read_to_string(b.join(&name).join("index.theme")).ok()) else {
                continue;
            };
            let (theme, inherits) = Theme::read(&name, &text, bases);
            themes.push(theme);
            todo.extend(inherits.into_iter().rev());
        }
        if !seen.iter().any(|n| n == "hicolor")
            && let Some(text) = bases.iter().find_map(|b| std::fs::read_to_string(b.join("hicolor/index.theme")).ok())
        {
            themes.push(Theme::read("hicolor", &text, bases).0);
        }
        Self { themes }
    }

    /// The file closest to `size` pixels (a 32 px icon at scale 2 is 64
    /// pixels) of the first theme in the chain having any of `names`, the
    /// first of them it has: as GTK looks, so Lumen shows what Nautilus does.
    pub fn find(&self, names: &[&str], size: u32) -> Option<PathBuf> {
        self.themes.iter().find_map(|theme| names.iter().find_map(|name| theme.closest(name, size)))
    }
}

impl Theme {
    /// The theme `name` described by `text` (its index.theme), with every
    /// icon file its directories hold in any of `bases`; and what it inherits.
    fn read(name: &str, text: &str, bases: &[PathBuf]) -> (Self, Vec<String>) {
        let sections = parse_ini(text);
        let head = sections.iter().find(|(s, _)| s == "Icon Theme").map(|(_, keys)| keys);
        let inherits = list(head.and_then(|k| k.get("Inherits")));
        let mut paths = list(head.and_then(|k| k.get("Directories")));
        paths.extend(list(head.and_then(|k| k.get("ScaledDirectories"))));
        let dirs: Vec<Dir> = paths
            .iter()
            .filter_map(|path| sections.iter().find(|(s, _)| s == path).and_then(|(_, keys)| Dir::from_keys(path, keys)))
            .collect();
        let mut files: HashMap<String, Vec<(usize, PathBuf)>> = HashMap::new();
        for (index, dir) in dirs.iter().enumerate() {
            for base in bases {
                let Ok(read) = std::fs::read_dir(base.join(name).join(&dir.path)) else { continue };
                let mut found: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
                // PNG before SVG when a directory has both.
                found.sort_by_key(|p| EXTENSIONS.iter().position(|e| p.extension().is_some_and(|x| x == *e)).unwrap_or(usize::MAX));
                for path in found {
                    let drawable = path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e));
                    if let (true, Some(stem)) = (drawable, path.file_stem().and_then(|s| s.to_str())) {
                        files.entry(stem.to_owned()).or_default().push((index, path.clone()));
                    }
                }
            }
        }
        (Self { dirs, files }, inherits)
    }

    /// This theme's file for `name`: one made for `size`, else the nearest.
    fn closest(&self, name: &str, size: u32) -> Option<PathBuf> {
        let files = self.files.get(name)?;
        // Among those that fit, the one drawn nearest that size (Yaru's
        // 256 px icons are Scalable down to 64; its 32 px at scale 2 are
        // drawn for 64); then directory order (`min_by_key` keeps the first).
        files
            .iter()
            .min_by_key(|(dir, _)| {
                let dir = &self.dirs[*dir];
                (dir.distance(size), (dir.size * dir.scale).abs_diff(size))
            })
            .map(|(_, path)| path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    /// Writes `files` (relative path -> text) under `dir`.
    fn write(dir: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    const COLOR: &str = "[Icon Theme]\nName=Color\nInherits=Base\nDirectories=48x48/places\n\n[48x48/places]\nSize=48\nType=Fixed\n";
    const BASE: &str = "[Icon Theme]\nName=Base\nDirectories=16x16/mimetypes,256x256/mimetypes,32x32@2x/mimetypes,48x48/mimetypes,48x48/places,scalable/places\n\n\
        [16x16/mimetypes]\nSize=16\n\n\
        [256x256/mimetypes]\nSize=256\nMinSize=64\nMaxSize=256\nType=Scalable\n\n\
        [32x32@2x/mimetypes]\nSize=32\nScale=2\nType=Fixed\n\n\
        [48x48/mimetypes]\nSize=48\nType=Fixed\n\n\
        [48x48/places]\nSize=48\nType=Fixed\n\n\
        [scalable/places]\nSize=16\nMinSize=8\nMaxSize=512\nType=Scalable\n";
    const HICOLOR: &str = "[Icon Theme]\nName=Hicolor\nDirectories=48x48/apps\n\n[48x48/apps]\nSize=48\nType=Threshold\n";

    /// Two base dirs: the user's (holding Color) and the system's.
    fn fixture(name: &str) -> (PathBuf, Vec<PathBuf>) {
        let dir = fresh_fixture(name);
        let (user, system) = (dir.join("user/icons"), dir.join("system/icons"));
        write(&user, &[("Color/index.theme", COLOR), ("Color/48x48/places/folder.png", "")]);
        write(
            &system,
            &[
                ("Base/index.theme", BASE),
                ("Base/48x48/places/folder.png", ""),
                ("Base/scalable/places/folder-download-symbolic.svg", ""),
                ("Base/16x16/mimetypes/text-plain.png", ""),
                ("Base/256x256/mimetypes/text-plain.png", ""),
                ("Base/32x32@2x/mimetypes/text-plain.png", ""),
                ("Base/48x48/mimetypes/text-plain.png", ""),
                ("Base/48x48/mimetypes/application-pdf.png", ""),
                ("hicolor/index.theme", HICOLOR),
                ("hicolor/48x48/apps/lumen.png", ""),
            ],
        );
        (dir, vec![user, system])
    }

    #[test]
    fn the_theme_wins_over_what_it_inherits_and_hicolor_comes_last() {
        let (dir, bases) = fixture("icon-theme-chain");
        let theme = IconTheme::load("Color", &bases);
        assert_eq!(theme.find(&["folder"], 48), Some(dir.join("user/icons/Color/48x48/places/folder.png")));
        assert_eq!(theme.find(&["application-pdf"], 48), Some(dir.join("system/icons/Base/48x48/mimetypes/application-pdf.png")), "inherited");
        assert_eq!(theme.find(&["lumen"], 48), Some(dir.join("system/icons/hicolor/48x48/apps/lumen.png")), "hicolor, never named");
        assert_eq!(theme.find(&["nothing-here"], 48), None);
        assert_eq!(
            theme.find(&["lumen", "folder"], 48),
            Some(dir.join("user/icons/Color/48x48/places/folder.png")),
            "like GTK: each theme tries every name before the next theme, so the user's theme wins over a better name in hicolor"
        );
    }

    #[test]
    fn the_first_name_found_wins_and_the_size_closest_in_pixels() {
        let (dir, bases) = fixture("icon-theme-size");
        let theme = IconTheme::load("Color", &bases);
        let base = dir.join("system/icons/Base");
        assert_eq!(theme.find(&["no-such-type", "application-pdf", "text-plain"], 48), Some(base.join("48x48/mimetypes/application-pdf.png")));
        assert_eq!(theme.find(&["text-plain"], 64), Some(base.join("32x32@2x/mimetypes/text-plain.png")), "32 at scale 2 is 64 pixels, over a big one scaled down");
        assert_eq!(theme.find(&["text-plain"], 128), Some(base.join("256x256/mimetypes/text-plain.png")), "only the scalable one fits");
        assert_eq!(theme.find(&["text-plain"], 16), Some(base.join("16x16/mimetypes/text-plain.png")), "Type defaults to Threshold");
        assert_eq!(theme.find(&["text-plain"], 40), Some(base.join("48x48/mimetypes/text-plain.png")), "nearest when none matches");
        assert_eq!(theme.find(&["folder-download-symbolic"], 24), Some(base.join("scalable/places/folder-download-symbolic.svg")), "scalable");
    }

    #[test]
    fn an_unknown_theme_still_finds_hicolor_and_a_loop_of_inherits_ends() {
        let (dir, bases) = fixture("icon-theme-unknown");
        assert_eq!(IconTheme::load("Missing", &bases).find(&["lumen"], 48), Some(dir.join("system/icons/hicolor/48x48/apps/lumen.png")));
        write(&bases[0], &[("Loop/index.theme", "[Icon Theme]\nInherits=Loop,Color\nDirectories=\n")]);
        assert!(IconTheme::load("Loop", &bases).find(&["folder"], 48).is_some());
    }

    #[test]
    fn the_desktop_names_its_theme() {
        let dir = fresh_fixture("icon-theme-name");
        let none = || None;
        assert_eq!(theme_name("GNOME", &dir, || Some("Yaru-dark".into())).as_deref(), Some("Yaru-dark"));
        write(&dir, &[("gtk-3.0/settings.ini", "[Settings]\ngtk-icon-theme-name = Papirus\n")]);
        assert_eq!(theme_name("XFCE", &dir, none).as_deref(), Some("Papirus"), "GTK's settings without gsettings");
        assert_eq!(theme_name("GNOME", &dir, || Some("Yaru".into())).as_deref(), Some("Yaru"), "gsettings first");
        write(&dir, &[("kdeglobals", "[General]\nTheme=x\n\n[Icons]\nTheme=breeze-dark\n")]);
        assert_eq!(theme_name("KDE", &dir, || Some("Adwaita".into())).as_deref(), Some("breeze-dark"), "KDE reads kdeglobals");
        assert_eq!(theme_name("ubuntu:GNOME", &dir, none).as_deref(), Some("Papirus"), "GNOME ignores kdeglobals");
        let empty = fresh_fixture("icon-theme-name-none");
        assert_eq!(theme_name("KDE", &empty, none).as_deref(), Some("breeze"), "KDE's default");
        assert_eq!(theme_name("GNOME", &empty, none), None);
    }

    #[test]
    fn gsettings_prints_the_name_quoted() {
        assert_eq!(gsettings_value("'Yaru-prussiangreen-dark'\n").as_deref(), Some("Yaru-prussiangreen-dark"));
        assert_eq!(gsettings_value("''\n"), None);
        assert_eq!(gsettings_value(""), None);
    }

    #[test]
    fn base_dirs_put_the_users_first() {
        let data = [PathBuf::from("/home/u/.local/share"), PathBuf::from("/usr/share")];
        assert_eq!(
            base_dirs(Path::new("/home/u"), &data),
            [PathBuf::from("/home/u/.icons"), PathBuf::from("/home/u/.local/share/icons"), PathBuf::from("/usr/share/icons")]
        );
    }
}
