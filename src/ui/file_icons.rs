//! The file's own icon (an .exe's, a shortcut's, an .ico's), asked from the
//! shell on a thread of its own and kept per path and modification time.
//! Every other type keeps Lumen's painted icons; off Windows, all do.

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

/// Whether the entry's icon comes from the file itself.
pub fn has_own_icon(entry: &Entry) -> bool {
    !entry.is_dir
        && entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| OWN_ICON.contains(&ext.to_ascii_lowercase().as_str()))
}

type Key = (PathBuf, Option<SystemTime>);

/// Icons asked for, loading and loaded. `None` once loaded: the shell had no
/// icon for it, and the painted one stays.
#[derive(Default)]
pub struct FileIcons {
    loaded: HashMap<Key, Option<egui::TextureHandle>>,
    /// Asked for while drawing (which only reads), sent on the next poll.
    wanted: RefCell<HashSet<Key>>,
    pending: HashSet<Key>,
    #[cfg(windows)]
    worker: Option<native::Worker>,
}

impl FileIcons {
    /// The entry's own icon once it is loaded; asks for it otherwise.
    pub fn texture(&self, entry: &Entry) -> Option<egui::TextureId> {
        if !has_own_icon(entry) {
            return None;
        }
        let key = (entry.path.clone(), entry.modified);
        match self.loaded.get(&key) {
            Some(texture) => texture.as_ref().map(egui::TextureHandle::id),
            None => {
                if !self.pending.contains(&key) {
                    self.wanted.borrow_mut().insert(key);
                }
                None
            }
        }
    }

    /// As if the shell had answered with `texture` for this entry.
    #[cfg(test)]
    pub fn insert(&mut self, entry: &Entry, texture: egui::TextureHandle) {
        self.loaded.insert((entry.path.clone(), entry.modified), Some(texture));
    }

    /// Sends what was asked for to the shell's thread and uploads what came
    /// back. Call once a frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let wanted: Vec<Key> = self.wanted.borrow_mut().drain().collect();
            if !wanted.is_empty() {
                let worker = self.worker.get_or_insert_with(|| native::Worker::start(ctx.clone()));
                for key in wanted {
                    worker.ask(key.clone());
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
                    ctx.load_texture(format!("lumen_file_icon_{}", key.0.display()), image, egui::TextureOptions::LINEAR)
                });
                self.loaded.insert(key, texture);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            // Nothing has its own icon here: everything keeps the painted ones.
            self.wanted.borrow_mut().clear();
        }
    }
}

#[cfg(windows)]
pub mod native {
    use std::path::Path;
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
        asks: Sender<Key>,
        answers: Receiver<(Key, Icon)>,
    }

    impl Worker {
        pub fn start(ctx: egui::Context) -> Self {
            let (asks, requests) = channel::<Key>();
            let (send, answers) = channel();
            let spawned = std::thread::Builder::new().name("lumen-file-icons".into()).spawn(move || {
                // SAFETY: COM for this thread, undone when it ends.
                let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                for key in requests {
                    let icon = icon_of(&key.0, super::SIDE);
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

        pub fn ask(&self, key: Key) {
            let _ = self.asks.send(key);
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
    fn exe_lnk_and_ico_files_have_their_own_icon_and_nothing_else_does() {
        for name in ["setup.exe", "App.LNK", "logo.Ico"] {
            assert!(has_own_icon(&entry(name)), "{name}");
        }
        for name in ["notes.txt", "photo.png", "exe", "archive.exe.zip"] {
            assert!(!has_own_icon(&entry(name)), "{name}");
        }
        let folder = Entry { is_dir: true, ..entry("tools.exe") };
        assert!(!has_own_icon(&folder), "a folder named like a program");
    }

    #[test]
    fn a_painted_type_never_asks_the_shell() {
        let icons = FileIcons::default();
        assert_eq!(icons.texture(&entry("notes.txt")), None);
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
    fn a_file_the_shell_cannot_read_keeps_the_painted_icon() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let gone = Entry { path: PathBuf::from(r"C:\no\such\folder\ghost.exe"), ..entry("ghost.exe") };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        icons.texture(&gone);
        while !icons.loaded.contains_key(&(gone.path.clone(), None)) && std::time::Instant::now() < deadline {
            icons.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(icons.loaded.get(&(gone.path.clone(), None)).map(Option::is_none), Some(true), "answered: no icon");
        assert_eq!(icons.texture(&gone), None, "and not asked again");
        assert!(icons.wanted.borrow().is_empty());
    }
}
