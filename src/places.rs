//! Folders the user keeps coming back to, for the side panel: bookmarks,
//! the usual places (home, Downloads...), mounted disks and recent folders.
//! No UI here, so it is unit-testable.

use std::path::{Path, PathBuf};

/// Bookmarked folders, in the order they were added.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Bookmarks {
    paths: Vec<PathBuf>,
}

impl Bookmarks {
    /// Bookmarks read back from the settings, without repeats.
    pub fn from_paths(paths: Vec<PathBuf>) -> Self {
        let mut bookmarks = Self::default();
        for path in paths {
            if !bookmarks.contains(&path) {
                bookmarks.paths.push(path);
            }
        }
        bookmarks
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }

    /// Adds `path`, or removes it when already there. Returns `true` when added.
    pub fn toggle(&mut self, path: &Path) -> bool {
        let before = self.paths.len();
        self.paths.retain(|p| p != path);
        if self.paths.len() < before {
            return false;
        }
        self.paths.push(path.to_path_buf());
        true
    }
}

/// How many recent folders are kept.
pub const RECENT_LIMIT: usize = 10;

/// Folders visited, most recent first, without repeats.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recents {
    paths: Vec<PathBuf>,
}

impl Recents {
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Puts `path` first, dropping its older visit and the oldest beyond the limit.
    pub fn visit(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(RECENT_LIMIT);
    }
}

/// A folder in the Places section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub trash: bool,
}

/// The XDG user folders this side panel lists, in File Pilot's order.
const USER_DIRS: [&str; 6] = ["DESKTOP", "DOWNLOAD", "DOCUMENTS", "MUSIC", "PICTURES", "VIDEOS"];

/// Reads `~/.config/user-dirs.dirs` lines such as
/// `XDG_DOWNLOAD_DIR="$HOME/Descargas"` into (`DOWNLOAD`, path). A folder set
/// to the home folder itself means "none" there, so it is left out.
#[cfg(test)]
pub fn parse_user_dirs(text: &str, home: &Path) -> Vec<(String, PathBuf)> {
    named_user_dirs(text, home)
        .into_iter()
        .filter_map(|(key, path)| Some((key, path?)))
        .collect()
}

/// Every key the file names; `None` for a key set to the home folder.
fn named_user_dirs(text: &str, home: &Path) -> Vec<(String, Option<PathBuf>)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let key = key.strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            let value = value.trim().trim_matches('"');
            let path = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None if value.starts_with('/') => PathBuf::from(value),
                None => return None,
            };
            Some((key.to_owned(), (path != home).then_some(path)))
        })
        .collect()
}

/// Home, the user folders that exist, and the trash. Folders named in
/// `user_dirs` (the text of user-dirs.dirs) win over the English defaults.
pub fn places(home: &Path, user_dirs: Option<&str>, trash: Option<&Path>) -> Vec<Place> {
    let named = user_dirs.map(|text| named_user_dirs(text, home)).unwrap_or_default();
    let label_of = |p: &Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
    let mut places = vec![Place { label: label_of(home), path: home.to_path_buf(), trash: false }];
    for (key, english) in USER_DIRS.iter().zip(ENGLISH_DIRS) {
        let path = match named.iter().find(|(k, _)| k == key) {
            Some((_, path)) => path.clone(),
            None => Some(home.join(english)),
        };
        if let Some(path) = path.filter(|p| p.is_dir()) {
            places.push(Place { label: label_of(&path), path, trash: false });
        }
    }
    if let Some(trash) = trash {
        places.push(Place { label: "Trash".into(), path: trash.to_path_buf(), trash: true });
    }
    places
}

/// Folder names used when user-dirs.dirs does not name them.
const ENGLISH_DIRS: [&str; 6] = ["Desktop", "Downloads", "Documents", "Music", "Pictures", "Videos"];

/// The folder holding trashed files: `$XDG_DATA_HOME/Trash/files`, by default
/// `~/.local/share/Trash/files`.
pub fn trash_dir(home: &Path, data_home: Option<&Path>) -> PathBuf {
    let data = data_home.map_or_else(|| home.join(".local/share"), Path::to_path_buf);
    data.join("Trash/files")
}

/// Whether `path` is the trash or inside it.
pub fn in_trash(path: &Path, trash: &Path) -> bool {
    path.starts_with(trash)
}

/// A mounted disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub label: String,
    pub path: PathBuf,
}

/// Disks worth listing from `/proc/self/mounts`: real devices (`/dev/...`),
/// not snap loop images, `/boot` or a second mount of the same device.
pub fn parse_mounts(text: &str) -> Vec<Mount> {
    let mut seen = Vec::new();
    let mut mounts = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(point)) = (fields.next(), fields.next()) else { continue };
        if !device.starts_with("/dev/") || device.starts_with("/dev/loop") || seen.contains(&device) {
            continue;
        }
        let path = PathBuf::from(unescape_mount(point));
        if path.starts_with("/boot") {
            continue;
        }
        seen.push(device);
        let label = match path.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => "File System".into(),
        };
        mounts.push(Mount { label, path });
    }
    mounts
}

/// The kernel writes spaces, tabs, newlines and backslashes in mount points
/// as octal escapes (`\040`).
fn unescape_mount(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Three octal digits, the first at most 3 so the value fits a byte.
        let octal = bytes.get(i + 1..i + 4).filter(|d| {
            bytes[i] == b'\\' && (b'0'..=b'3').contains(&d[0]) && d.iter().all(|b| (b'0'..=b'7').contains(b))
        });
        match octal {
            Some(d) => {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Bytes used and in total on the disk holding `path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub total: u64,
    pub free: u64,
}

#[cfg(unix)]
pub fn usage(path: &Path) -> Option<Usage> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c_path` is a valid NUL-terminated string and `stats` is a
    // properly sized out-parameter that statvfs fills on success.
    let stats = unsafe {
        let mut stats: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stats) != 0 {
            return None;
        }
        stats
    };
    let unit = stats.f_frsize as u64;
    Some(Usage { total: stats.f_blocks as u64 * unit, free: stats.f_bavail as u64 * unit })
}

#[cfg(not(unix))]
pub fn usage(_path: &Path) -> Option<Usage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_adds_at_the_end_then_removes() {
        let mut b = Bookmarks::default();
        assert!(b.toggle(Path::new("/home/u/Descargas")));
        assert!(b.toggle(Path::new("/srv")));
        assert!(b.contains(Path::new("/srv")));
        assert_eq!(b.paths(), [PathBuf::from("/home/u/Descargas"), PathBuf::from("/srv")]);
        assert!(!b.toggle(Path::new("/home/u/Descargas")), "second toggle removes");
        assert!(!b.contains(Path::new("/home/u/Descargas")));
        assert_eq!(b.paths(), [PathBuf::from("/srv")]);
    }

    #[test]
    fn recents_put_the_newest_first_without_repeats_and_keep_ten() {
        let mut r = Recents::default();
        for name in ["/a", "/b", "/a"] {
            r.visit(Path::new(name));
        }
        assert_eq!(r.paths(), [PathBuf::from("/a"), PathBuf::from("/b")]);
        for i in 0..12 {
            r.visit(&PathBuf::from(format!("/n{i}")));
        }
        assert_eq!(r.paths().len(), RECENT_LIMIT);
        assert_eq!(r.paths()[0], PathBuf::from("/n11"));
        assert_eq!(r.paths()[9], PathBuf::from("/n2"));
    }

    const USER_DIRS_ES: &str = r#"# This file is written by xdg-user-dirs-update
XDG_DESKTOP_DIR="$HOME/Escritorio"
XDG_DOWNLOAD_DIR="$HOME/Descargas"
XDG_TEMPLATES_DIR="$HOME/Plantillas"
XDG_DOCUMENTS_DIR="$HOME/Documentos"
XDG_MUSIC_DIR="$HOME/"
XDG_PICTURES_DIR="/media/fotos"
XDG_VIDEOS_DIR="$HOME/Vídeos"
"#;

    #[test]
    fn user_dirs_are_read_with_home_expanded() {
        let dirs = parse_user_dirs(USER_DIRS_ES, Path::new("/home/u"));
        let get = |key: &str| dirs.iter().find(|(k, _)| k == key).map(|(_, p)| p.clone());
        assert_eq!(get("DOWNLOAD"), Some(PathBuf::from("/home/u/Descargas")));
        assert_eq!(get("VIDEOS"), Some(PathBuf::from("/home/u/Vídeos")));
        assert_eq!(get("PICTURES"), Some(PathBuf::from("/media/fotos")), "absolute paths stay");
        assert_eq!(get("MUSIC"), None, "set to home itself: disabled");
        assert_eq!(get("TEMPLATES"), Some(PathBuf::from("/home/u/Plantillas")));
    }

    #[test]
    fn places_list_home_the_existing_user_folders_and_the_trash() {
        let home = crate::fs::ops::tests::fresh_fixture("places-home");
        for name in ["Escritorio", "Descargas", "Documentos", "Vídeos", "Music"] {
            std::fs::create_dir(home.join(name)).unwrap();
        }
        let text = USER_DIRS_ES.replace("$HOME", &home.display().to_string());
        let trash = home.join(".local/share/Trash/files");
        let places = places(&home, Some(&text), Some(&trash));
        let labels: Vec<&str> = places.iter().map(|p| p.label.as_str()).collect();
        // Pictures points to a missing folder; Music is disabled, and the
        // English default is not used for a key the file names.
        assert_eq!(labels, ["places-home", "Escritorio", "Descargas", "Documentos", "Vídeos", "Trash"]);
        assert_eq!(places[2].path, home.join("Descargas"));
        assert!(places[5].trash && !places[0].trash);
        assert_eq!(places[5].path, trash);
    }

    #[test]
    fn without_user_dirs_the_english_defaults_are_used_when_they_exist() {
        let home = crate::fs::ops::tests::fresh_fixture("places-default");
        std::fs::create_dir(home.join("Downloads")).unwrap();
        let labels: Vec<String> = places(&home, None, None).into_iter().map(|p| p.label).collect();
        assert_eq!(labels, ["places-default", "Downloads"]);
    }

    #[test]
    fn the_trash_lives_under_the_data_home() {
        let home = Path::new("/home/u");
        assert_eq!(trash_dir(home, None), PathBuf::from("/home/u/.local/share/Trash/files"));
        assert_eq!(trash_dir(home, Some(Path::new("/data"))), PathBuf::from("/data/Trash/files"));
        assert!(in_trash(Path::new("/home/u/.local/share/Trash/files/x.txt"), &trash_dir(home, None)));
        assert!(!in_trash(Path::new("/home/u/Descargas"), &trash_dir(home, None)));
    }

    #[test]
    fn mounts_keep_real_disks_once_and_decode_spaces() {
        let text = "\
sysfs /sys sysfs rw 0 0
/dev/sda2 / ext4 rw,relatime 0 0
/dev/sda1 /boot/efi vfat rw 0 0
/dev/loop3 /snap/core/123 squashfs ro 0 0
/dev/sdb1 /media/u/My\\040USB vfat rw 0 0
/dev/sda2 /var/snap/bind ext4 rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/nvme0n1p3 /home ext4 rw 0 0
";
        let mounts = parse_mounts(text);
        let got: Vec<(&str, &Path)> = mounts.iter().map(|m| (m.label.as_str(), m.path.as_path())).collect();
        assert_eq!(
            got,
            [
                ("File System", Path::new("/")),
                ("My USB", Path::new("/media/u/My USB")),
                ("home", Path::new("/home")),
            ]
        );
    }

    #[test]
    fn mount_escapes_decode_and_odd_ones_stay_as_written() {
        assert_eq!(unescape_mount("/a\\040b\\011c\\012d\\134e"), "/a b\tc\nd\\e");
        assert_eq!(unescape_mount("/a\\04"), "/a\\04", "too short to be an escape");
        assert_eq!(unescape_mount("/x\\777"), "/x\\777", "above one byte: not an escape");
    }

    #[cfg(unix)]
    #[test]
    fn usage_of_the_root_disk_is_known() {
        let usage = usage(Path::new("/")).expect("statvfs on /");
        assert!(usage.total > 0 && usage.free <= usage.total, "{usage:?}");
        assert_eq!(super::usage(Path::new("/no/such/folder")), None);
    }
}
