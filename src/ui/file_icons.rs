//! Windows' own icons for the list, like File Explorer and File Pilot: the
//! type's (its program's) for ordinary files, the file's own for programs,
//! shortcuts, .ico files and folders. Asked from the shell on a thread of
//! its own; the painted icon shows until one arrives, or when there is none.
//! Off Windows every row keeps the painted icons.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use crate::fs::Entry;

/// Types whose icon lives in the file itself, not in its type.
const OWN_ICON: &[&str] = &["exe", "lnk", "ico"];

/// The icon's side in pixels: the list's biggest zoom draws it at 64 points.
pub const SIDE: i32 = 64;

/// Which icon an entry shows: one per type, or its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Ordinary files: one icon per extension, lowercase ("" for none).
    Type(String),
    /// Programs, shortcuts and .ico files (asked again once modified), and
    /// folders (special ones have their own).
    Path(PathBuf, Option<SystemTime>),
}

/// The icon the entry shows.
pub fn key_of(entry: &Entry) -> Key {
    if entry.is_dir {
        return Key::Path(entry.path.clone(), None);
    }
    let ext = entry.path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if OWN_ICON.contains(&ext.as_str()) {
        Key::Path(entry.path.clone(), entry.modified)
    } else {
        Key::Type(ext)
    }
}

/// Icons asked for, loading and loaded. `None` once loaded: the shell had no
/// icon for it, and the painted one stays.
#[derive(Default)]
pub struct FileIcons {
    loaded: HashMap<Key, Option<egui::TextureHandle>>,
    /// Asked for while drawing (which only reads), with a path to ask the
    /// shell about; sent on the next poll.
    wanted: RefCell<HashMap<Key, PathBuf>>,
    pending: HashSet<Key>,
    #[cfg(windows)]
    worker: Option<native::Worker>,
}

impl FileIcons {
    /// The shell's icon for the entry once it is loaded; asks for it
    /// otherwise (on Windows only).
    pub fn texture(&self, entry: &Entry) -> Option<egui::TextureId> {
        self.lookup(key_of(entry), &entry.path)
    }

    /// The shell's icon for the folder or disk at `path` (a side panel row).
    pub fn folder_texture(&self, path: &std::path::Path) -> Option<egui::TextureId> {
        self.lookup(Key::Path(path.to_path_buf(), None), path)
    }

    fn lookup(&self, key: Key, path: &std::path::Path) -> Option<egui::TextureId> {
        if let Some(texture) = self.loaded.get(&key) {
            return texture.as_ref().map(egui::TextureHandle::id);
        }
        if cfg!(windows) && !self.pending.contains(&key) {
            self.wanted.borrow_mut().entry(key).or_insert_with(|| path.to_path_buf());
        }
        None
    }

    /// As if the shell had answered with `texture` for the folder or disk at `path`.
    #[cfg(test)]
    pub fn insert_folder(&mut self, path: &std::path::Path, texture: egui::TextureHandle) {
        self.loaded.insert(Key::Path(path.to_path_buf(), None), Some(texture));
    }

    /// As if the shell had answered with `texture` for this entry.
    #[cfg(test)]
    pub fn insert(&mut self, entry: &Entry, texture: egui::TextureHandle) {
        self.loaded.insert(key_of(entry), Some(texture));
    }

    /// Sends what was asked for to the shell's thread and uploads what came
    /// back. Call once a frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let wanted: Vec<(Key, PathBuf)> = self.wanted.borrow_mut().drain().collect();
            if !wanted.is_empty() {
                let worker = self.worker.get_or_insert_with(|| native::Worker::start(ctx.clone()));
                for (key, path) in wanted {
                    worker.ask(key.clone(), path);
                    self.pending.insert(key);
                }
            }
            let Some(worker) = &self.worker else { return };
            let answers = worker.answers();
            if !answers.is_empty() {
                // Drawn already this frame: the next one shows them.
                ctx.request_repaint();
            }
            for (key, icon) in answers {
                self.pending.remove(&key);
                let texture = icon.map(|(size, rgba)| {
                    let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
                    ctx.load_texture(format!("lumen_file_icon_{key:?}"), image, egui::TextureOptions::LINEAR)
                });
                self.loaded.insert(key, texture);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            // Nothing is asked for here: everything keeps the painted ones.
            self.wanted.borrow_mut().clear();
        }
    }
}

#[cfg(windows)]
pub mod native {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, Sender, channel};

    use eframe::egui;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY};
    use windows::core::HSTRING;

    use super::Key;

    type Icon = Option<([usize; 2], Vec<u8>)>;

    /// The shell's icon for `path`, `side` pixels square, as premultiplied
    /// RGBA. Needs COM on the calling thread.
    pub fn icon_of(path: &Path, side: i32) -> Icon {
        // SAFETY: COM calls on this thread's apartment; the bitmap is ours
        // and freed once its pixels are read.
        unsafe {
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
            let bitmap = factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY).ok()?;
            let icon = crate::shell_menu::native::pixels(bitmap, side);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            icon
        }
    }

    /// One thread with COM that answers icon requests in order, so the list
    /// never waits on the shell.
    pub struct Worker {
        asks: Sender<(Key, PathBuf)>,
        answers: Receiver<(Key, Icon)>,
    }

    impl Worker {
        pub fn start(ctx: egui::Context) -> Self {
            let (asks, requests) = channel::<(Key, PathBuf)>();
            let (send, answers) = channel();
            let spawned = std::thread::Builder::new().name("lumen-file-icons".into()).spawn(move || {
                // SAFETY: COM for this thread, undone when it ends.
                let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                for (key, path) in requests {
                    let icon = icon_of(&path, super::SIDE);
                    if send.send((key, icon)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
                // SAFETY: undoes the call above; nothing of COM's is left here.
                unsafe { CoUninitialize() };
            });
            if spawned.is_err() {
                // No thread: every icon stays painted.
                return Self { asks: channel().0, answers: channel().1 };
            }
            Self { asks, answers }
        }

        pub fn ask(&self, key: Key, path: PathBuf) {
            let _ = self.asks.send((key, path));
        }

        pub fn answers(&self) -> Vec<(Key, Icon)> {
            self.answers.try_iter().collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry { name: name.into(), path: PathBuf::from(name), is_dir: false, size: Some(1), modified: None, hidden: false }
    }

    #[test]
    fn ordinary_files_share_their_types_icon_and_programs_shortcuts_icos_and_folders_have_their_own() {
        assert_eq!(key_of(&entry("a.pdf")), Key::Type("pdf".into()));
        assert_eq!(key_of(&entry("B.PDF")), key_of(&entry("a.pdf")), "one icon per type, any case");
        assert_eq!(key_of(&entry("README")), Key::Type(String::new()));
        assert_eq!(key_of(&entry("archive.exe.zip")), Key::Type("zip".into()));
        let modified = Some(SystemTime::UNIX_EPOCH);
        for name in ["setup.exe", "App.LNK", "logo.Ico"] {
            let own = Entry { modified, ..entry(name) };
            assert_eq!(key_of(&own), Key::Path(PathBuf::from(name), modified), "{name}");
        }
        let folder = Entry { is_dir: true, modified, ..entry("Downloads") };
        assert_eq!(key_of(&folder), Key::Path(PathBuf::from("Downloads"), None), "a folder's own, whatever its date");
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_nothing_is_asked_for() {
        let icons = FileIcons::default();
        assert_eq!(icons.texture(&entry("setup.exe")), None);
        assert!(icons.wanted.borrow().is_empty());
    }

    #[cfg(windows)]
    fn explorer() -> Entry {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let path = PathBuf::from(root).join("explorer.exe");
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Entry { name: "explorer.exe".into(), path, is_dir: false, size: Some(1), modified, hidden: false }
    }

    #[cfg(windows)]
    #[test]
    fn the_shell_gives_a_programs_own_icon_as_rgba() {
        let explorer = explorer();
        let (size, rgba) = native::icon_of(&explorer.path, SIDE).expect("explorer.exe has an icon");
        assert_eq!(size, [SIDE as usize, SIDE as usize]);
        assert_eq!(rgba.len(), size[0] * size[1] * 4);
        assert!(rgba.chunks(4).any(|px| px[3] > 0), "not a blank square");
    }

    /// Polls until the entry's icon is in, or 20 s pass.
    #[cfg(windows)]
    fn wait(icons: &mut FileIcons, ctx: &egui::Context, entry: &Entry) -> Option<egui::TextureId> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(id) = icons.texture(entry) {
                return Some(id);
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            icons.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_icon_loads_off_the_ui_thread_once_and_again_when_the_file_changes() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let explorer = explorer();
        assert_eq!(icons.texture(&explorer), None, "not loaded yet: the painted icon meanwhile");
        let first = wait(&mut icons, &ctx, &explorer).expect("the icon arrived");
        assert_eq!(icons.texture(&explorer), Some(first), "kept, not asked again");
        assert!(icons.wanted.borrow().is_empty() && icons.pending.is_empty());
        let changed = Entry { modified: explorer.modified.map(|t| t + std::time::Duration::from_secs(1)), ..explorer.clone() };
        assert_eq!(icons.texture(&changed), None, "a modified file is asked for again");
        assert!(wait(&mut icons, &ctx, &changed).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn files_of_one_type_are_asked_for_once_and_folders_get_the_shells_icon() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let dir = crate::fs::ops::tests::fresh_fixture("file-icons-types");
        std::fs::write(dir.join("a.pdf"), "").unwrap();
        std::fs::write(dir.join("B.PDF"), "").unwrap();
        let file = |name: &str| Entry { path: dir.join(name), ..entry(name) };
        let (a, b) = (file("a.pdf"), file("B.PDF"));
        icons.texture(&a);
        icons.texture(&b);
        assert_eq!(icons.wanted.borrow().len(), 1, "one request for the type");
        let shown = wait(&mut icons, &ctx, &a).expect("the type's icon");
        assert_eq!(icons.texture(&b), Some(shown), "the other file shows the same one");
        let folder = Entry { is_dir: true, ..file("") };
        assert!(wait(&mut icons, &ctx, &folder).is_some(), "the folder's icon");
    }

    #[cfg(windows)]
    #[test]
    fn a_file_the_shell_cannot_read_keeps_the_painted_icon() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let gone = Entry { path: PathBuf::from(r"C:\no\such\folder\ghost.exe"), ..entry("ghost.exe") };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        icons.texture(&gone);
        while !icons.loaded.contains_key(&key_of(&gone)) && std::time::Instant::now() < deadline {
            icons.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(icons.loaded.get(&key_of(&gone)).map(Option::is_none), Some(true), "answered: no icon");
        assert_eq!(icons.texture(&gone), None, "and not asked again");
        assert!(icons.wanted.borrow().is_empty());
    }
}
