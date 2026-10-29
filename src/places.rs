//! Folders the user keeps coming back to, for the side panel: bookmarks,
//! the usual places (home, Downloads...), mounted disks and recent folders.
//! No UI here, so it is unit-testable.

use std::path::{Path, PathBuf};

use crate::i18n::tr;

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

/// A visit's weight halves every two weeks.
pub const VISIT_HALF_LIFE: u64 = 14 * 24 * 60 * 60;
/// How many visited folders the quick jump remembers.
pub const VISIT_LIMIT: usize = 200;

/// A visited folder: its score when last visited, and when (Unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    pub path: PathBuf,
    pub score: f64,
    pub at: u64,
}

/// Folders visited, scored like File Pilot's: each visit adds one, and the
/// score fades with time, so the quick jump puts the folders used often and
/// lately first.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Visits {
    list: Vec<Visit>,
}

impl Visits {
    /// From the saved list; a repeated path keeps its latest visit.
    pub fn from_list(list: Vec<Visit>) -> Self {
        let mut visits = Self::default();
        for visit in list {
            match visits.list.iter_mut().find(|v| v.path == visit.path) {
                Some(kept) if kept.at < visit.at => *kept = visit,
                Some(_) => {}
                None => visits.list.push(visit),
            }
        }
        visits
    }

    pub fn list(&self) -> &[Visit] {
        &self.list
    }

    /// Counts a visit to `path` at `now`, dropping the weakest beyond the limit.
    pub fn visit(&mut self, path: &Path, now: u64) {
        let before = match self.list.iter().position(|v| v.path == path) {
            Some(at) => faded(&self.list.remove(at), now),
            None => 0.0,
        };
        self.list.insert(0, Visit { path: path.to_path_buf(), score: before + 1.0, at: now });
        if self.list.len() > VISIT_LIMIT {
            let weakest = (0..self.list.len()).min_by(|&a, &b| faded(&self.list[a], now).total_cmp(&faded(&self.list[b], now)));
            self.list.remove(weakest.expect("not empty"));
        }
    }

    /// Every folder's score at `now`.
    pub fn scores(&self, now: u64) -> Vec<(PathBuf, f64)> {
        self.list.iter().map(|v| (v.path.clone(), faded(v, now))).collect()
    }
}

/// `visit`'s score at `now`, halved for each half-life gone by.
pub fn faded(visit: &Visit, now: u64) -> f64 {
    visit.score * 0.5_f64.powf(now.saturating_sub(visit.at) as f64 / VISIT_HALF_LIFE as f64)
}

/// A folder in the Places section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub trash: bool,
    /// Its freedesktop icon name ("folder-download"), for the icon theme.
    pub icon: &'static str,
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

/// The folder user-dirs.dirs names for `key` (`DESKTOP`): `None` when it
/// does not name one, `Some(None)` when it is set to the home folder.
#[cfg(not(windows))]
pub fn user_dir(text: &str, home: &Path, key: &str) -> Option<Option<PathBuf>> {
    named_user_dirs(text, home).into_iter().rev().find(|(k, _)| k == key).map(|(_, path)| path)
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
                None if Path::new(value).is_absolute() => PathBuf::from(value),
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
    let mut places = vec![Place { label: label_of(home), path: home.to_path_buf(), trash: false, icon: "user-home" }];
    for ((key, english), icon) in USER_DIRS.iter().zip(ENGLISH_DIRS).zip(USER_DIR_ICONS) {
        let path = match named.iter().find(|(k, _)| k == key) {
            Some((_, path)) => path.clone(),
            None => Some(home.join(english)),
        };
        if let Some(path) = path.filter(|p| p.is_dir()) {
            places.push(Place { label: label_of(&path), path, trash: false, icon });
        }
    }
    if let Some(trash) = trash {
        places.push(Place { label: tr!("Trash").into(), path: trash.to_path_buf(), trash: true, icon: "user-trash" });
    }
    places
}

/// The folder whose files New ▸ offers: XDG_TEMPLATES_DIR, else ~/Templates;
/// `None` when user-dirs.dirs sets it to the home folder (meaning "none").
pub fn templates_dir(home: &Path, user_dirs: Option<&str>) -> Option<PathBuf> {
    let named = user_dirs.map(|text| named_user_dirs(text, home)).unwrap_or_default();
    match named.into_iter().find(|(key, _)| key == "TEMPLATES") {
        Some((_, path)) => path,
        None => Some(home.join("Templates")),
    }
}

/// The visible files in `dir`, by name; none when it cannot be read.
/// Windows' own New ▸ (the shell's) lists its templates instead.
#[cfg_attr(windows, allow(dead_code))]
pub fn templates(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn the_templates_folder_comes_from_user_dirs_or_defaults_to_templates() {
        let home = Path::new("/home/u");
        let es = "XDG_TEMPLATES_DIR=\"$HOME/Plantillas\"\n";
        assert_eq!(templates_dir(home, Some(es)), Some(home.join("Plantillas")));
        assert_eq!(templates_dir(home, None), Some(home.join("Templates")));
        assert_eq!(templates_dir(home, Some("XDG_TEMPLATES_DIR=\"$HOME/\"\n")), None, "set to home: none");
    }

    #[test]
    fn templates_are_the_visible_files_of_the_folder_by_name() {
        let dir = crate::fs::ops::tests::fresh_fixture("templates-list");
        for name in ["Spreadsheet.ods", "Note.txt", ".hidden"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        std::fs::create_dir(dir.join("folder")).unwrap();
        assert_eq!(templates(&dir), [dir.join("Note.txt"), dir.join("Spreadsheet.ods")]);
        assert_eq!(templates(&dir.join("missing")), Vec::<PathBuf>::new());
    }
}

/// The freedesktop icon of each of USER_DIRS.
const USER_DIR_ICONS: [&str; 6] = ["user-desktop", "folder-download", "folder-documents", "folder-music", "folder-pictures", "folder-videos"];

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

/// Windows: whether `path` is a drive's `$Recycle.Bin` or inside it.
pub fn in_recycle_bin(path: &Path) -> bool {
    use std::path::Component;
    let mut parts = path.components();
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(Component::Prefix(_)), Some(Component::RootDir), Some(Component::Normal(name)))
            if name.eq_ignore_ascii_case("$Recycle.Bin")
    )
}

/// What a tab shows for "This PC" (Windows): the drives. Not a path on disk,
/// only a marker the tab holds as its location, so nothing may treat it as a
/// folder (a relative path would resolve against the working directory).
pub const THIS_PC: &str = "This PC";

pub fn this_pc() -> PathBuf {
    PathBuf::from(THIS_PC)
}

/// Whether `path` is the This PC marker.
pub fn is_this_pc(path: &Path) -> bool {
    path == Path::new(THIS_PC)
}

/// Whether typed text names This PC (on Windows, where it exists).
pub fn names_this_pc(text: &str) -> bool {
    cfg!(windows) && text.trim().eq_ignore_ascii_case(THIS_PC)
}

/// What a tab shows for the Recycle Bin (Windows): the entries of every
/// drive's `$Recycle.Bin`. A marker like `THIS_PC`, not a folder on disk.
pub const RECYCLE_BIN: &str = "Recycle Bin";

pub fn recycle_bin() -> PathBuf {
    PathBuf::from(RECYCLE_BIN)
}

/// Whether `path` is the Recycle Bin marker.
pub fn is_recycle_bin(path: &Path) -> bool {
    path == Path::new(RECYCLE_BIN)
}

/// Whether `path` is a marker (This PC, the Recycle Bin) and not a folder:
/// nothing may be made, pasted, dropped, watched or listed there.
pub fn is_marker(path: &Path) -> bool {
    is_this_pc(path) || is_recycle_bin(path)
}

/// The title a marker shows in a tab, the path bar and the crumbs.
pub fn marker_label(path: &Path) -> Option<&'static str> {
    if is_this_pc(path) {
        Some(THIS_PC)
    } else {
        is_recycle_bin(path).then_some(RECYCLE_BIN)
    }
}

/// The marker typed text names, on Windows (where they exist), whatever its case.
pub fn named_marker(text: &str) -> Option<PathBuf> {
    if names_this_pc(text) {
        Some(this_pc())
    } else {
        (cfg!(windows) && text.trim().eq_ignore_ascii_case(RECYCLE_BIN)).then(recycle_bin)
    }
}

/// `places` with the Recycle Bin last when `windows`: after the user folders.
pub fn with_recycle_bin(mut places: Vec<Place>, windows: bool) -> Vec<Place> {
    if windows {
        places.push(Place { label: tr!("Recycle Bin").into(), path: recycle_bin(), trash: false, icon: "user-trash" });
    }
    places
}

/// `places` with This PC first when `windows`: the only system that has it.
pub fn with_this_pc(mut places: Vec<Place>, windows: bool) -> Vec<Place> {
    if windows {
        places.insert(0, Place { label: tr!("This PC").into(), path: this_pc(), trash: false, icon: "computer" });
    }
    places
}

/// What a drive is, as File Explorer names it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DriveKind {
    #[default]
    Local,
    Removable,
    Network,
    Cd,
    Other,
}

impl DriveKind {
    /// From `GetDriveTypeW`'s answer.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_win(raw: u32) -> Self {
        match raw {
            3 => Self::Local,
            2 => Self::Removable,
            4 => Self::Network,
            5 => Self::Cd,
            _ => Self::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Local => tr!("Local Disk"),
            Self::Removable => tr!("Removable Disk"),
            Self::Network => tr!("Network Drive"),
            Self::Cd => tr!("CD Drive"),
            Self::Other => tr!("Drive"),
        }
    }
}

/// A drive's kind and file system ("NTFS", "FAT32"; empty when unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriveInfo {
    pub kind: DriveKind,
    pub fs: String,
}

/// What reading a drive's volume gave: its name (maybe empty) and file system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    pub fs: String,
}

/// A drive as one read of the disks found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub mount: Mount,
    /// `None` when the space could not be read (or was not asked).
    pub usage: Option<Usage>,
    pub info: DriveInfo,
}

/// "Ubuntu (\\wsl$) (Z:)" for a drive mapped to `\\wsl$\Ubuntu`, as File
/// Explorer says it: the volume's name when it has one ("Data (Z:)"), else
/// the share and its server from the remote name, else "Network Drive (Z:)".
#[cfg_attr(not(windows), allow(dead_code))]
pub fn network_label(letter: char, volume: &str, remote: Option<&str>) -> String {
    let volume = volume.trim();
    if !volume.is_empty() {
        return format!("{volume} ({letter}:)");
    }
    match remote.and_then(share_parts) {
        Some((share, server)) => format!("{share} ({server}) ({letter}:)"),
        None => tr!("Network Drive ({0}:)", letter),
    }
}

/// ("Ubuntu", "\\\\wsl$") from "\\\\wsl$\\Ubuntu": the share, and where it is. `None`
/// for a name that is no UNC path or names only a server.
#[cfg_attr(not(windows), allow(dead_code))]
fn share_parts(remote: &str) -> Option<(&str, String)> {
    let segments: Vec<&str> = remote.trim().strip_prefix("\\\\")?.split('\\').filter(|s| !s.is_empty()).collect();
    let (share, parents) = segments.split_last()?;
    (!parents.is_empty()).then(|| (*share, format!("\\\\{}", parents.join("\\"))))
}

/// The disk to list for the drive `letter`, or `None` when it has nothing to
/// open (an empty card reader, a CD drive). `volume` is what reading the
/// volume gave; `remote` (the share a network drive is mapped to) and `usage`
/// are only asked when they are needed, so a down share is not asked twice.
/// A network drive whose volume cannot be read is still listed, without numbers.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn disk_of(
    letter: char,
    kind: DriveKind,
    volume: Option<Volume>,
    remote: impl FnOnce() -> Option<String>,
    usage: impl FnOnce() -> Option<Usage>,
) -> Option<Disk> {
    let path = PathBuf::from(format!("{letter}:\\"));
    let label = match kind {
        DriveKind::Local | DriveKind::Removable => {
            // No volume in an empty card reader slot: nothing to open there.
            let volume = volume.as_ref()?;
            drive_label(letter, &volume.name, kind == DriveKind::Removable)
        }
        DriveKind::Network => {
            let named = volume.as_ref().is_some_and(|volume| !volume.name.trim().is_empty());
            let share = if named { None } else { remote() };
            network_label(letter, volume.as_ref().map_or("", |volume| &volume.name), share.as_deref())
        }
        DriveKind::Cd | DriveKind::Other => return None,
    };
    let usage = if volume.is_some() { usage() } else { None };
    let fs = volume.map(|volume| volume.fs).unwrap_or_default();
    Some(Disk { mount: Mount { label, path }, usage, info: DriveInfo { kind, fs } })
}

/// Reads the disks on a thread of its own, one read at a time: a share that
/// is down or slow would stall the window if the UI thread asked it.
#[derive(Default)]
pub struct DiskReader {
    receiver: Option<std::sync::mpsc::Receiver<Vec<Disk>>>,
}

impl DiskReader {
    /// Whether a read is under way.
    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    /// Runs `read` on a thread and calls `wake` when it is done. Does nothing
    /// while a read is under way.
    pub fn start(&mut self, wake: impl FnOnce() + Send + 'static, read: impl FnOnce() -> Vec<Disk> + Send + 'static) {
        if self.receiver.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(read());
            wake();
        });
    }

    /// What the read found, once, when it is done.
    pub fn poll(&mut self) -> Option<Vec<Disk>> {
        match self.receiver.as_ref()?.try_recv() {
            Ok(disks) => {
                self.receiver = None;
                Some(disks)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            // The thread is gone without an answer: read again next time.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                None
            }
        }
    }
}

/// A mounted disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub label: String,
    pub path: PathBuf,
}

/// Disks worth listing from `/proc/self/mounts`: real devices (`/dev/...`),
/// not snap loop images, `/boot` or a second mount of the same device.
#[cfg_attr(windows, allow(dead_code))]
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
            None => tr!("File System").into(),
        };
        mounts.push(Mount { label, path });
    }
    mounts
}

/// Network file systems in the mount table.
const NETWORK_TYPES: [&str; 11] =
    ["nfs", "nfs4", "cifs", "smb3", "smbfs", "fuse.sshfs", "fuse.rclone", "davfs", "fuse.s3fs", "ceph", "glusterfs"];

/// Network shares mounted by the kernel (NFS, SMB, SSHFS...), and where GNOME
/// mounts its own (gvfs), if it does.
#[cfg_attr(windows, allow(dead_code))]
pub fn parse_network_mounts(text: &str) -> (Vec<PathBuf>, Option<PathBuf>) {
    let mut shares = Vec::new();
    let mut gvfs = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace().skip(1);
        let (Some(point), Some(kind)) = (fields.next(), fields.next()) else { continue };
        let path = PathBuf::from(unescape_mount(point));
        if NETWORK_TYPES.contains(&kind) {
            shares.push(path);
        } else if kind == "fuse.gvfsd-fuse" {
            gvfs = Some(path);
        }
    }
    (shares, gvfs)
}

/// A gvfs mount that is a network share (Files > Other Locations), not a
/// phone, camera or cloud account: those are slow to read and not shares.
#[cfg_attr(windows, allow(dead_code))]
pub fn is_gvfs_share(name: &str) -> bool {
    ["smb-share:", "sftp:", "ftp:", "dav:", "davs:", "nfs:", "afp-volume:"].iter().any(|kind| name.starts_with(kind))
}

/// Network shares to search: the kernel's and gvfs's on Linux.
#[cfg(not(windows))]
pub fn network_mounts() -> Vec<PathBuf> {
    let (mut shares, gvfs) = parse_network_mounts(&std::fs::read_to_string("/proc/self/mounts").unwrap_or_default());
    if let Some(Ok(items)) = gvfs.map(std::fs::read_dir) {
        shares.extend(items.flatten().filter(|i| is_gvfs_share(&i.file_name().to_string_lossy())).map(|i| i.path()));
    }
    shares
}

/// Network drives (mapped letters) on Windows. Their volume is not read:
/// a share that is down would stall.
#[cfg(windows)]
pub fn network_mounts() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_REMOTE: u32 = 4;
    // SAFETY: no arguments; returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    drive_letters(mask)
        .into_iter()
        .map(|letter| format!("{letter}:\\"))
        // SAFETY: the wide string is NUL-terminated and outlives the call.
        .filter(|root| unsafe { GetDriveTypeW(wide(root).as_ptr()) } == DRIVE_REMOTE)
        .map(PathBuf::from)
        .collect()
}

/// Disks for the side panel: mounted devices on Linux, drive letters on Windows.
#[cfg(not(windows))]
pub fn disks() -> Vec<Mount> {
    parse_mounts(&std::fs::read_to_string("/proc/self/mounts").unwrap_or_default())
}

/// Local and removable drives with a volume in them, for the quick jump's
/// crawl. Network drives are `network_mounts`: reading them can stall.
#[cfg(windows)]
pub fn disks() -> Vec<Mount> {
    windows_disks(false, false).into_iter().map(|disk| disk.mount).collect()
}

/// Every disk for the side panel and This PC, with its space and file system.
/// On Windows the network drives are in it, and a share that is down makes this
/// wait: call it on a thread of its own (`DiskReader`), never on the UI's.
#[cfg(windows)]
pub fn read_disks() -> Vec<Disk> {
    windows_disks(true, true)
}

#[cfg(not(windows))]
pub fn read_disks() -> Vec<Disk> {
    disks()
        .into_iter()
        .map(|mount| {
            let usage = usage(&mount.path);
            Disk { mount, usage, info: DriveInfo::default() }
        })
        .collect()
}

/// The drive letters in use, read as `disk_of` wants. Reading a network
/// drive can take as long as its share does; `network` and `space` say
/// whether to ask them at all and whether to read the free space.
#[cfg(windows)]
fn windows_disks(network: bool, space: bool) -> Vec<Disk> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetThreadErrorMode};
    // An empty card reader would otherwise pop up a "no disk" system dialog.
    let mut previous = 0;
    // SAFETY: `previous` is a valid out-parameter; restored below.
    unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut previous) };
    // SAFETY: no arguments; returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    let mut disks = Vec::new();
    for letter in drive_letters(mask) {
        let root = format!("{letter}:\\");
        let wide = wide(&root);
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        let kind = DriveKind::from_win(unsafe { GetDriveTypeW(wide.as_ptr()) });
        if !matches!(kind, DriveKind::Local | DriveKind::Removable | DriveKind::Network) || (kind == DriveKind::Network && !network) {
            continue;
        }
        let volume = read_volume(&wide);
        let usage = || if space { usage(Path::new(&root)) } else { None };
        disks.extend(disk_of(letter, kind, volume, || remote_name(letter), usage));
    }
    // SAFETY: restores the mode read above; the old value is not needed.
    unsafe { SetThreadErrorMode(previous, std::ptr::null_mut()) };
    disks
}

/// The volume name and file system of the drive at `root` (wide, "C:\"), or
/// `None` when it cannot be read: an empty card reader slot, a share that is down.
#[cfg(windows)]
fn read_volume(root: &[u16]) -> Option<Volume> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let (mut name, mut fs) = ([0u16; 261], [0u16; 64]);
    // SAFETY: `root` is NUL-terminated; `name` and `fs` are writable buffers of
    // the lengths passed; the other out-parameters are optional and null.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            name.as_mut_ptr(),
            name.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    };
    let text = |buffer: &[u16]| String::from_utf16_lossy(&buffer[..buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len())]);
    (ok != 0).then(|| Volume { name: text(&name), fs: text(&fs) })
}

/// The share a network drive letter is mapped to ("\\wsl$\Ubuntu"), if Windows says.
#[cfg(windows)]
fn remote_name(letter: char) -> Option<String> {
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    let local = wide(&format!("{letter}:"));
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: `local` is NUL-terminated; `buffer` is writable for `len` units.
    let error = unsafe { WNetGetConnectionW(local.as_ptr(), buffer.as_mut_ptr(), &mut len) };
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    (error == 0 && end > 0).then(|| String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Drive letters set in a `GetLogicalDrives` mask, A: being bit 0.
pub fn drive_letters(mask: u32) -> Vec<char> {
    ('A'..='Z').zip(0..26).filter(|&(_, bit)| mask & (1 << bit) != 0).map(|(letter, _)| letter).collect()
}

/// Whether the drive letters differ between two `GetLogicalDrives` masks:
/// a drive was plugged in or ejected. Bits past Z mean nothing.
pub fn drives_changed(old: u32, new: u32) -> bool {
    // Bits 0 to 25 are A: to Z:.
    (old ^ new) & 0x03FF_FFFF != 0
}

/// The drive letters in use now, as a `GetLogicalDrives` mask: a call as cheap
/// as can be, unlike reading the disks. `None` where there are no drive letters.
#[cfg(windows)]
pub fn drive_mask() -> Option<u32> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
    // SAFETY: no arguments; returns a bitmask.
    Some(unsafe { GetLogicalDrives() })
}

#[cfg(not(windows))]
pub fn drive_mask() -> Option<u32> {
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
/// "Windows (C:)"; without a volume name, "Local Disk (D:)" or "USB Drive (F:)"
/// as File Explorer says.
pub fn drive_label(letter: char, volume: &str, removable: bool) -> String {
    let unnamed = if removable { tr!("USB Drive") } else { tr!("Local Disk") };
    let name = if volume.trim().is_empty() { unnamed } else { volume.trim() };
    format!("{name} ({letter}:)")
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

#[cfg(windows)]
pub fn usage(path: &Path) -> Option<Usage> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let root = wide(&path.to_string_lossy());
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `root` is a NUL-terminated UTF-16 string; the out-parameters are
    // valid u64s, and the unused one is null.
    let ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some(Usage { total, free })
}

#[cfg(not(any(unix, windows)))]
pub fn usage(_path: &Path) -> Option<Usage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60;

    fn score(visits: &Visits, path: &str, now: u64) -> f64 {
        visits.scores(now).into_iter().find(|(p, _)| p == Path::new(path)).map_or(0.0, |(_, s)| s)
    }

    #[test]
    fn visits_add_up_and_halve_every_two_weeks() {
        let mut visits = Visits::default();
        visits.visit(Path::new("/d"), 1000);
        visits.visit(Path::new("/d"), 1000);
        visits.visit(Path::new("/e"), 1000);
        assert_eq!(score(&visits, "/d", 1000), 2.0);
        assert_eq!(score(&visits, "/e", 1000), 1.0);
        assert!((score(&visits, "/d", 1000 + 14 * DAY) - 1.0).abs() < 1e-9);
        assert!((score(&visits, "/d", 1000 + 28 * DAY) - 0.5).abs() < 1e-9);
        assert_eq!(score(&visits, "/nope", 1000), 0.0);
    }

    #[test]
    fn a_new_visit_adds_to_the_faded_score_and_is_remembered_once() {
        let mut visits = Visits::default();
        visits.visit(Path::new("/d"), 0);
        visits.visit(Path::new("/d"), 0);
        visits.visit(Path::new("/d"), 14 * DAY);
        assert!((score(&visits, "/d", 14 * DAY) - 2.0).abs() < 1e-9, "2 halved, plus 1");
        assert_eq!(visits.list().len(), 1);
        assert_eq!(visits.list()[0].at, 14 * DAY);
    }

    #[test]
    fn beyond_the_limit_the_weakest_folder_is_forgotten() {
        let mut visits = Visits::default();
        visits.visit(Path::new("/old"), 0);
        for i in 0..VISIT_LIMIT {
            visits.visit(&PathBuf::from(format!("/f{i}")), 30 * DAY);
        }
        assert_eq!(visits.list().len(), VISIT_LIMIT);
        assert_eq!(score(&visits, "/old", 30 * DAY), 0.0, "faded the most");
        assert_eq!(score(&visits, "/f0", 30 * DAY), 1.0);
    }

    #[test]
    fn a_saved_list_keeps_the_latest_visit_of_a_repeated_folder() {
        let visit = |path: &str, score, at| Visit { path: PathBuf::from(path), score, at };
        let visits = Visits::from_list(vec![visit("/d", 5.0, 10), visit("/e", 1.0, 10), visit("/d", 2.0, 20)]);
        assert_eq!(visits.list(), [visit("/d", 2.0, 20), visit("/e", 1.0, 10)]);
    }

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
        // An XDG file only exists on Linux, where /media/fotos is absolute.
        #[cfg(unix)]
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
        let icons: Vec<&str> = places.iter().map(|p| p.icon).collect();
        assert_eq!(icons, ["user-home", "user-desktop", "folder-download", "folder-documents", "folder-videos", "user-trash"]);
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

    #[cfg(windows)]
    #[test]
    fn the_recycle_bin_of_every_drive_counts_as_the_trash() {
        for path in [r"C:\$Recycle.Bin", r"C:\$Recycle.Bin\S-1-5-21\$R1.png", r"d:\$RECYCLE.BIN"] {
            assert!(in_recycle_bin(Path::new(path)), "{path}");
        }
        for path in [r"C:\", r"C:\Users\$Recycle.Bin", r"C:\Recycle.Bin", r"C:\Users\u\Downloads"] {
            assert!(!in_recycle_bin(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn network_shares_are_found_in_the_mount_table() {
        let text = "/dev/sda2 / ext4 rw 0 0\nserver:/export /mnt/nas nfs4 rw 0 0\n//pc/share /mnt/win\\040share cifs rw 0 0\n\
                    u@h:/ /home/u/remote fuse.sshfs rw 0 0\ntmpfs /tmp tmpfs rw 0 0\ngvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0\n";
        let (shares, gvfs) = parse_network_mounts(text);
        assert_eq!(shares, ["/mnt/nas", "/mnt/win share", "/home/u/remote"].map(PathBuf::from));
        assert_eq!(gvfs, Some(PathBuf::from("/run/user/1000/gvfs")));
        assert_eq!(parse_network_mounts("/dev/sda2 / ext4 rw 0 0\n"), (Vec::new(), None));
    }

    #[test]
    fn only_gvfs_network_shares_are_searched() {
        for name in ["smb-share:server=pc,share=docs", "sftp:host=h", "ftp:host=h", "dav:host=h", "davs:host=h", "nfs:host=h", "afp-volume:host=h"] {
            assert!(is_gvfs_share(name), "{name}");
        }
        for name in ["mtp:host=phone", "google-drive:host=gmail.com", "gphoto2:host=cam", "onedrive:host=x"] {
            assert!(!is_gvfs_share(name), "{name}");
        }
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
    fn drive_letters_follow_the_logical_drives_mask() {
        assert_eq!(drive_letters(0), Vec::<char>::new());
        // Bit 0 is A:, bit 2 is C:, bit 3 is D:, bit 25 is Z:.
        assert_eq!(drive_letters(0b1101), ['A', 'C', 'D']);
        assert_eq!(drive_letters(1 << 25), ['Z']);
        assert_eq!(drive_letters(u32::MAX), ('A'..='Z').collect::<Vec<_>>(), "bits past Z are ignored");
    }

    #[test]
    fn a_drive_letter_coming_or_going_is_a_change() {
        assert!(!drives_changed(0b1100, 0b1100));
        assert!(drives_changed(0b0100, 0b1100), "plugged in");
        assert!(drives_changed(0b1100, 0b0100), "ejected");
        assert!(drives_changed(0b0110, 0b1010), "one gone while another came");
        assert!(!drives_changed(0b1100, 0b1100 | 1 << 31), "bits past Z are not drives");
    }

    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_among_the_drive_letters_and_elsewhere_there_are_none() {
        let mask = drive_mask().expect("Windows has drive letters");
        let system = std::env::var("SystemDrive").ok().and_then(|d| d.chars().next()).unwrap_or('C');
        assert!(drive_letters(mask).contains(&system.to_ascii_uppercase()), "{mask:b}");
    }

    #[cfg(not(windows))]
    #[test]
    fn elsewhere_there_are_no_drive_letters_to_look_up() {
        assert_eq!(drive_mask(), None);
    }

    #[test]
    fn drives_are_labelled_like_file_explorer() {
        assert_eq!(drive_label('C', "Windows", false), "Windows (C:)");
        assert_eq!(drive_label('D', "", false), "Local Disk (D:)");
        assert_eq!(drive_label('E', "  ", false), "Local Disk (E:)", "a blank volume name counts as none");
        assert_eq!(drive_label('F', "", true), "USB Drive (F:)");
        assert_eq!(drive_label('F', "FOTOS", true), "FOTOS (F:)");
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

#[cfg(test)]
mod this_pc_tests {
    use super::*;

    fn place(label: &str) -> Place {
        Place { label: label.into(), path: PathBuf::from(format!("/home/u/{label}")), trash: false, icon: "folder" }
    }

    #[test]
    fn on_windows_this_pc_is_the_first_place_with_the_computer_icon() {
        let listed = with_this_pc(vec![place("u"), place("Downloads")], true);
        let labels: Vec<&str> = listed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["This PC", "u", "Downloads"]);
        assert_eq!((listed[0].icon, listed[0].trash), ("computer", false));
        assert!(is_this_pc(&listed[0].path));
    }

    #[test]
    fn elsewhere_the_places_are_left_as_they_are() {
        let listed = with_this_pc(vec![place("u")], false);
        assert_eq!(listed, [place("u")]);
    }

    #[test]
    fn only_the_marker_is_this_pc() {
        assert!(is_this_pc(&this_pc()));
        assert!(!is_this_pc(Path::new("/this pc")));
        assert!(!is_this_pc(Path::new("C:\\")));
        assert!(!is_this_pc(Path::new("")));
    }

    #[test]
    fn typed_text_names_this_pc_on_windows_only_whatever_its_case() {
        for text in ["This PC", "this pc", "  THIS PC "] {
            assert_eq!(names_this_pc(text), cfg!(windows), "{text:?}");
        }
        for text in ["This", "This PC\\", "C:\\"] {
            assert!(!names_this_pc(text), "{text:?}");
        }
    }

    #[test]
    fn only_the_marker_is_the_recycle_bin_and_it_is_not_this_pc() {
        assert!(is_recycle_bin(&recycle_bin()));
        assert!(!is_recycle_bin(Path::new("/recycle bin")));
        assert!(!is_recycle_bin(Path::new("C:\\$Recycle.Bin")), "that is a folder on disk");
        assert!(!is_recycle_bin(&this_pc()) && !is_this_pc(&recycle_bin()));
        assert!(!is_recycle_bin(Path::new("")));
    }

    #[test]
    fn both_markers_are_no_folder_and_nothing_else_is() {
        assert!(is_marker(&this_pc()) && is_marker(&recycle_bin()));
        for path in ["C:\\", "/", "", "Recycle", "C:\\$Recycle.Bin", "Recycle Bin\\x"] {
            assert!(!is_marker(Path::new(path)), "{path:?}");
        }
    }

    #[test]
    fn a_marker_is_titled_by_its_name() {
        assert_eq!(marker_label(&this_pc()), Some("This PC"));
        assert_eq!(marker_label(&recycle_bin()), Some("Recycle Bin"));
        assert_eq!(marker_label(Path::new("/home")), None);
    }

    #[test]
    fn typed_text_names_a_marker_on_windows_only_whatever_its_case() {
        for (text, marker) in [("This PC", this_pc()), ("  this pc ", this_pc()), ("Recycle Bin", recycle_bin()), ("recycle BIN", recycle_bin()), (" RECYCLE BIN  ", recycle_bin())] {
            assert_eq!(named_marker(text), cfg!(windows).then_some(marker), "{text:?}");
        }
        for text in ["Recycle", "Recycle Bin\\", "C:\\$Recycle.Bin", "", "Bin"] {
            assert_eq!(named_marker(text), None, "{text:?}");
        }
    }

    #[test]
    fn on_windows_the_recycle_bin_is_the_last_place_with_a_trash_icon_but_not_the_trash() {
        let listed = with_recycle_bin(with_this_pc(vec![place("u"), place("Downloads")], true), true);
        let labels: Vec<&str> = listed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["This PC", "u", "Downloads", "Recycle Bin"]);
        let bin = listed.last().unwrap();
        assert_eq!((bin.icon, bin.trash), ("user-trash", false));
        assert!(is_recycle_bin(&bin.path));
    }

    #[test]
    fn elsewhere_the_places_get_no_recycle_bin() {
        assert_eq!(with_recycle_bin(vec![place("u")], false), [place("u")]);
    }

    #[test]
    fn drive_types_are_named_as_file_explorer_does() {
        let named: Vec<&str> = [3, 2, 4, 5].into_iter().map(|raw| DriveKind::from_win(raw).name()).collect();
        assert_eq!(named, ["Local Disk", "Removable Disk", "Network Drive", "CD Drive"]);
        assert_eq!(DriveKind::from_win(0), DriveKind::Other, "unknown");
        assert_eq!(DriveKind::from_win(6), DriveKind::Other, "a RAM disk");
    }

    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_a_local_disk_with_a_file_system() {
        let root = std::env::var("SystemDrive").map_or_else(|_| PathBuf::from("C:\\"), |d| PathBuf::from(format!("{d}\\")));
        let disk = read_disks().into_iter().find(|disk| disk.mount.path == root).expect("the system drive is listed");
        assert_eq!(disk.info.kind, DriveKind::Local);
        assert!(!disk.info.fs.is_empty(), "{disk:?}");
        assert!(disk.usage.is_some(), "{disk:?}");
    }

    #[test]
    fn a_network_drive_is_named_after_its_volume_else_its_share_else_just_network_drive() {
        let share = Some(r"\\wsl$\Ubuntu");
        assert_eq!(network_label('Z', "", share), r"Ubuntu (\\wsl$) (Z:)", "as File Explorer says it");
        assert_eq!(network_label('S', "  ", Some(r"\\nas\media\")), r"media (\\nas) (S:)");
        assert_eq!(network_label('Z', "Data", share), "Data (Z:)", "the volume's own name first");
        assert_eq!(network_label('Z', "", None), "Network Drive (Z:)");
        assert_eq!(network_label('Z', "", Some("")), "Network Drive (Z:)");
        assert_eq!(network_label('Z', "", Some(r"\\server")), "Network Drive (Z:)", "a server alone names no share");
        assert_eq!(network_label('Z', "", Some("https://dav.example/x")), "Network Drive (Z:)", "not a UNC name");
    }

    fn volume(name: &str, fs: &str) -> Option<Volume> {
        Some(Volume { name: name.into(), fs: fs.into() })
    }

    const SPACE: Usage = Usage { total: 1006, free: 993 };

    #[test]
    fn a_readable_network_drive_lists_with_its_file_system_and_space() {
        let disk = disk_of('Z', DriveKind::Network, volume("", "9P"), || Some(r"\\wsl$\Ubuntu".into()), || Some(SPACE)).unwrap();
        assert_eq!(disk.mount, Mount { label: r"Ubuntu (\\wsl$) (Z:)".into(), path: PathBuf::from("Z:\\") });
        assert_eq!(disk.info, DriveInfo { kind: DriveKind::Network, fs: "9P".into() });
        assert_eq!(disk.info.kind.name(), "Network Drive");
        assert_eq!(disk.usage, Some(SPACE));
    }

    #[test]
    fn a_network_drive_with_a_volume_name_does_not_ask_for_the_share() {
        let disk = disk_of('Z', DriveKind::Network, volume("Backup", "NTFS"), || panic!("no need"), || Some(SPACE)).unwrap();
        assert_eq!(disk.mount.label, "Backup (Z:)");
    }

    #[test]
    fn a_network_drive_that_cannot_be_read_is_listed_without_numbers() {
        let disk = disk_of('Z', DriveKind::Network, None, || Some(r"\\nas\media".into()), || panic!("a share that is down is not asked again")).unwrap();
        assert_eq!(disk.mount, Mount { label: r"media (\\nas) (Z:)".into(), path: PathBuf::from("Z:\\") });
        assert_eq!(disk.info, DriveInfo { kind: DriveKind::Network, fs: String::new() });
        assert_eq!(disk.usage, None);
        let unnamed = disk_of('Y', DriveKind::Network, None, || None, || None).unwrap();
        assert_eq!(unnamed.mount.label, "Network Drive (Y:)");
    }

    #[test]
    fn local_and_removable_drives_keep_their_labels_and_an_empty_slot_is_left_out() {
        let local = disk_of('C', DriveKind::Local, volume("Windows", "NTFS"), || panic!("not a share"), || Some(SPACE)).unwrap();
        assert_eq!(local.mount, Mount { label: "Windows (C:)".into(), path: PathBuf::from("C:\\") });
        assert_eq!(local.info, DriveInfo { kind: DriveKind::Local, fs: "NTFS".into() });
        let usb = disk_of('F', DriveKind::Removable, volume("", "FAT32"), || None, || None).unwrap();
        assert_eq!(usb.mount.label, "USB Drive (F:)");
        assert_eq!(usb.usage, None);
        assert_eq!(disk_of('G', DriveKind::Removable, None, || None, || Some(SPACE)), None, "an empty card reader");
        assert_eq!(disk_of('D', DriveKind::Local, None, || None, || None), None);
        assert_eq!(disk_of('E', DriveKind::Cd, volume("Disc", "CDFS"), || None, || None), None);
    }

    fn disk(label: &str) -> Disk {
        Disk { mount: Mount { label: label.into(), path: PathBuf::from("/") }, usage: None, info: DriveInfo::default() }
    }

    /// Polls until the reader answers; the read ends within a moment in these tests.
    fn answer(reader: &mut DiskReader) -> Vec<Disk> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(disks) = reader.poll() {
                return disks;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the read did not answer");
    }

    #[test]
    fn the_reader_answers_once_and_wakes_the_window() {
        let (woke, woken) = std::sync::mpsc::channel();
        let mut reader = DiskReader::default();
        assert!(!reader.is_running() && reader.poll().is_none());
        reader.start(move || woke.send(()).unwrap(), || vec![disk("A")]);
        assert!(reader.is_running());
        woken.recv_timeout(std::time::Duration::from_secs(10)).expect("woken when done");
        assert_eq!(answer(&mut reader), [disk("A")]);
        assert!(!reader.is_running(), "free for the next read");
        assert!(reader.poll().is_none(), "answered once");
    }

    #[test]
    fn a_slow_read_is_polled_without_waiting_and_only_one_runs_at_a_time() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let mut reader = DiskReader::default();
        let started = std::time::Instant::now();
        reader.start(|| {}, move || {
            let _ = blocked.recv_timeout(std::time::Duration::from_secs(10));
            vec![disk("first")]
        });
        assert!(reader.is_running() && reader.poll().is_none());
        reader.start(|| {}, || vec![disk("second")]);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "neither start nor poll waited for the read");
        release.send(()).unwrap();
        assert_eq!(answer(&mut reader), [disk("first")], "the second start was ignored");
    }

    #[test]
    fn a_read_that_dies_without_an_answer_frees_the_reader() {
        let mut reader = DiskReader::default();
        reader.start(|| {}, || panic!("the thread dies"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while reader.is_running() && std::time::Instant::now() < deadline {
            assert!(reader.poll().is_none());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!reader.is_running(), "read again next time");
    }
}
