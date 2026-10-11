//! The system's own icons for the list, like File Explorer and Nautilus. On
//! Windows the shell's: the type's (its program's) for ordinary files, the
//! file's own for programs, shortcuts, .ico files and folders. Elsewhere the
//! user's freedesktop icon theme: one per file type, special folders their
//! own. Loaded on a thread of their own; the painted icon shows until one
//! arrives, or when there is none.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use crate::fs::Entry;

/// Types whose icon lives in the file itself, not in its type (Windows).
const OWN_ICON: &[&str] = if cfg!(windows) { &["exe", "lnk", "ico"] } else { &[] };

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
    /// A side panel row's one-colour icon, by its freedesktop name
    /// ("folder-download" asks for folder-download-symbolic).
    #[cfg(not(windows))]
    Symbolic(&'static str),
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
    /// One texture per icon file (and way of drawing it), shared by every
    /// key showing it: a thousand folders upload one folder icon.
    by_file: HashMap<(PathBuf, bool), egui::TextureHandle>,
    worker: Option<native::Worker>,
    /// Where the icon theme and the MIME tables are, and the special folders.
    #[cfg(not(windows))]
    setup: Option<native::Setup>,
}

impl FileIcons {
    /// The shell's icon for the entry once it is loaded; asks for it
    /// otherwise (on Windows only).
    pub fn texture(&self, entry: &Entry) -> Option<egui::TextureId> {
        self.lookup(key_of(entry), &entry.path)
    }

    /// The shell's icon for the folder or disk at `path` (a side panel row).
    #[cfg(windows)]
    pub fn folder_texture(&self, path: &std::path::Path) -> Option<egui::TextureId> {
        self.lookup(Key::Path(path.to_path_buf(), None), path)
    }

    /// The theme's one-colour icon called `name` (+ "-symbolic"), drawn in
    /// white: tint it with the text colour.
    #[cfg(not(windows))]
    pub fn symbolic_texture(&self, name: &'static str) -> Option<egui::TextureId> {
        self.lookup(Key::Symbolic(name), std::path::Path::new(""))
    }

    /// As if the theme had answered with `texture` for the symbolic icon `name`.
    #[cfg(all(test, not(windows)))]
    pub fn insert_symbolic(&mut self, name: &'static str, texture: egui::TextureHandle) {
        self.loaded.insert(Key::Symbolic(name), Some(texture));
    }

    /// The symbolic icons asked for and not sent yet.
    #[cfg(all(test, not(windows)))]
    pub fn wanted_symbolic(&self) -> Vec<&'static str> {
        self.wanted
            .borrow()
            .keys()
            .filter_map(|key| match key {
                Key::Symbolic(name) => Some(*name),
                _ => None,
            })
            .collect()
    }

    fn lookup(&self, key: Key, path: &std::path::Path) -> Option<egui::TextureId> {
        if let Some(texture) = self.loaded.get(&key) {
            return texture.as_ref().map(egui::TextureHandle::id);
        }
        if !self.pending.contains(&key) {
            self.wanted.borrow_mut().entry(key).or_insert_with(|| path.to_path_buf());
        }
        None
    }

    /// As if the shell had answered with `texture` for the folder or disk at `path`.
    #[cfg(all(test, windows))]
    pub fn insert_folder(&mut self, path: &std::path::Path, texture: egui::TextureHandle) {
        self.loaded.insert(Key::Path(path.to_path_buf(), None), Some(texture));
    }

    /// As if the shell had answered with `texture` for this entry.
    #[cfg(test)]
    pub fn insert(&mut self, entry: &Entry, texture: egui::TextureHandle) {
        self.loaded.insert(key_of(entry), Some(texture));
    }

    /// Where to find icons, set once before the first poll asks for any;
    /// the environment's otherwise.
    #[cfg(not(windows))]
    pub fn set_setup(&mut self, setup: native::Setup) {
        self.setup = Some(setup);
    }

    /// Sends what was asked for to the icons' thread and uploads what came
    /// back. Call once a frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let wanted: Vec<(Key, PathBuf)> = self.wanted.borrow_mut().drain().collect();
        if !wanted.is_empty() {
            #[cfg(windows)]
            let worker = self.worker.get_or_insert_with(|| native::Worker::start(ctx.clone()));
            #[cfg(not(windows))]
            let worker = self.worker.get_or_insert_with(|| {
                let setup = self.setup.take().unwrap_or_else(|| native::Setup::from_env(Vec::new()));
                native::Worker::start(ctx.clone(), setup)
            });
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
        for (key, file, icon) in answers {
            self.pending.remove(&key);
            let shared = file.as_ref().and_then(|file| self.by_file.get(file)).cloned();
            let texture = shared.or_else(|| {
                let (size, rgba) = icon?;
                let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
                let texture = ctx.load_texture(format!("lumen_file_icon_{key:?}"), image, egui::TextureOptions::LINEAR);
                if let Some(file) = file {
                    self.by_file.insert(file, texture.clone());
                }
                Some(texture)
            });
            self.loaded.insert(key, texture);
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

    /// The shell's answers: one per key, none shared (see the Linux worker).
    pub type Answer = (Key, Option<(PathBuf, bool)>, Icon);

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

        pub fn answers(&self) -> Vec<Answer> {
            self.answers.try_iter().map(|(key, icon)| (key, None, icon)).collect()
        }
    }
}

#[cfg(not(windows))]
pub mod native {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, Sender, channel};

    use eframe::egui;

    use super::Key;
    use crate::icon_names::IconNames;
    use crate::icon_theme::{self, IconTheme};

    pub type Icon = Option<([usize; 2], Vec<u8>)>;

    /// Reads an icon file into pixels: as it is, or as a symbolic shape.
    type Draw = fn(&Path, u32) -> Icon;

    /// Where the icons come from.
    pub struct Setup {
        /// $XDG_DATA_HOME, then $XDG_DATA_DIRS: mime/ tables.
        pub data: Vec<PathBuf>,
        /// Icon theme folders, most important first.
        pub bases: Vec<PathBuf>,
        /// The theme; `None` asks the desktop (on the icons' thread: it may
        /// run gsettings).
        pub theme: Option<String>,
        /// Special folders and their icon names.
        pub folders: Vec<(PathBuf, String)>,
    }

    impl Setup {
        pub fn from_env(folders: Vec<(PathBuf, String)>) -> Self {
            let dirs = crate::apps::Dirs::from_env();
            let home = crate::fs::home_dir().unwrap_or_default();
            Self { bases: icon_theme::base_dirs(&home, &dirs.data), data: dirs.data, theme: None, folders }
        }
    }

    /// Side panel icons are drawn at text height: 32 px is sharp up to twice
    /// a 16 point line.
    const SYMBOLIC_SIDE: u32 = 32;

    /// The theme used when the desktop names none.
    const FALLBACK_THEME: &str = "Adwaita";

    /// The icon file drawn in a `side` pixels square, as premultiplied RGBA:
    /// SVG with resvg, PNG with image. `None` when it cannot be read.
    pub fn load(file: &Path, side: u32) -> Icon {
        let bytes = std::fs::read(file).ok()?;
        if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
            draw_svg(&bytes, side)
        } else {
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let mut rgba = image.into_raw();
            for px in rgba.as_chunks_mut::<4>().0 {
                let alpha = u16::from(px[3]);
                for channel in &mut px[..3] {
                    *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
                }
            }
            Some((size, rgba))
        }
    }

    /// A symbolic icon in white, keeping its shape (alpha): drawn tinted, it
    /// takes the text colour, as GTK recolours them.
    pub fn symbolic(file: &Path, side: u32) -> Icon {
        let (size, mut rgba) = load(file, side)?;
        for px in rgba.as_chunks_mut::<4>().0 {
            // Premultiplied white: every channel is the alpha.
            let alpha = px[3];
            px[..3].fill(alpha);
        }
        Some((size, rgba))
    }

    /// The SVG scaled to fit a `side` square, centred; tiny-skia's pixels
    /// are premultiplied already.
    fn draw_svg(bytes: &[u8], side: u32) -> Icon {
        use resvg::{tiny_skia, usvg};
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
        let size = tree.size();
        let scale = side as f32 / size.width().max(size.height());
        let (dx, dy) = ((side as f32 - size.width() * scale) / 2.0, (side as f32 - size.height() * scale) / 2.0);
        let mut pixmap = tiny_skia::Pixmap::new(side, side)?;
        resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
        Some(([side as usize, side as usize], pixmap.take()))
    }

    /// One thread that loads the theme once, then answers icon requests in
    /// order, so the list never waits on the disk.
    pub struct Worker {
        asks: Sender<(Key, PathBuf)>,
        answers: Receiver<Answer>,
    }

    /// A key's icon and the file it came from (drawn as a symbolic shape or
    /// not), so keys showing one file share its texture.
    pub type Answer = (Key, Option<(PathBuf, bool)>, Icon);

    /// The icons' thread stack: resvg walks an SVG's nesting recursively, and
    /// a theme's file is not ours to trust.
    const STACK: usize = 32 << 20;

    impl Worker {
        pub fn start(ctx: egui::Context, setup: Setup) -> Self {
            let (asks, requests) = channel::<(Key, PathBuf)>();
            let (send, answers) = channel();
            let spawned = std::thread::Builder::new().name("lumen-file-icons".into()).stack_size(STACK).spawn(move || {
                let config = crate::apps::Dirs::from_env().config.into_iter().next().unwrap_or_default();
                let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
                let theme = setup.theme.or_else(|| icon_theme::theme_name(&desktop, &config, icon_theme::ask_gsettings));
                let theme = IconTheme::load(theme.as_deref().unwrap_or(FALLBACK_THEME), &setup.bases);
                let names = IconNames::read(&setup.data, setup.folders);
                // Many keys share one file (text-x-generic, folder): drawn once.
                let mut drawn: HashSet<(PathBuf, bool)> = HashSet::new();
                for (key, _) in requests {
                    let (wanted, side, draw, shape): (Vec<String>, u32, Draw, bool) = match &key {
                        Key::Type(ext) => (names.for_type(ext), super::SIDE as u32, load, false),
                        Key::Path(path, _) => (names.for_folder(path), super::SIDE as u32, load, false),
                        Key::Symbolic(name) => (vec![format!("{name}-symbolic"), "folder-symbolic".into()], SYMBOLIC_SIDE, symbolic, true),
                    };
                    let wanted: Vec<&str> = wanted.iter().map(String::as_str).collect();
                    let answer = match theme.find(&wanted, side) {
                        Some(file) => {
                            let source = (file, shape);
                            // Sent once per file: the UI keeps its texture for the next keys.
                            let icon = if drawn.contains(&source) { None } else { draw(&source.0, side) };
                            drawn.insert(source.clone());
                            (key, Some(source), icon)
                        }
                        None => (key, None, None),
                    };
                    if send.send(answer).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
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

        pub fn answers(&self) -> Vec<Answer> {
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
            if cfg!(windows) {
                assert_eq!(key_of(&own), Key::Path(PathBuf::from(name), modified), "{name}");
            } else {
                let ext = name.rsplit('.').next().unwrap().to_ascii_lowercase();
                assert_eq!(key_of(&own), Key::Type(ext), "{name}: the theme has one icon per type");
            }
        }
        let folder = Entry { is_dir: true, modified, ..entry("Downloads") };
        assert_eq!(key_of(&folder), Key::Path(PathBuf::from("Downloads"), None), "a folder's own, whatever its date");
    }

    /// A theme "T" with a PNG for PDFs (64 px, one half-transparent red
    /// pixel) and an SVG folder; MIME tables naming .pdf and .txt.
    #[cfg(not(windows))]
    fn theme_fixture(name: &str) -> (PathBuf, native::Setup) {
        let dir = crate::fs::ops::tests::fresh_fixture(name);
        let theme = dir.join("icons/T");
        for sub in ["32x32@2x/mimetypes", "scalable/places"] {
            std::fs::create_dir_all(theme.join(sub)).unwrap();
        }
        std::fs::write(
            theme.join("index.theme"),
            "[Icon Theme]\nDirectories=32x32@2x/mimetypes,scalable/places\n\n[32x32@2x/mimetypes]\nSize=32\nScale=2\nType=Fixed\n\n\
             [scalable/places]\nSize=16\nMinSize=8\nMaxSize=512\nType=Scalable\n",
        )
        .unwrap();
        let mut png = image::RgbaImage::new(64, 64);
        png.put_pixel(0, 0, image::Rgba([255, 0, 0, 128]));
        png.save(theme.join("32x32@2x/mimetypes/application-pdf.png")).unwrap();
        std::fs::write(
            theme.join("scalable/places/folder.svg"),
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="4" y="4" width="8" height="8" fill="#0000ff"/></svg>"##,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("data/mime")).unwrap();
        std::fs::write(dir.join("data/mime/globs2"), "50:application/pdf:*.pdf\n50:text/plain:*.txt\n").unwrap();
        let setup = native::Setup { data: vec![dir.join("data")], bases: vec![dir.join("icons")], theme: Some("T".into()), folders: Vec::new() };
        (dir, setup)
    }

    #[cfg(not(windows))]
    #[test]
    fn a_png_icon_is_read_as_premultiplied_rgba_and_an_svg_is_drawn_to_size() {
        let (dir, _) = theme_fixture("file-icons-load");
        let (size, rgba) = native::load(&dir.join("icons/T/32x32@2x/mimetypes/application-pdf.png"), 64).expect("png");
        assert_eq!(size, [64, 64]);
        assert_eq!(&rgba[..4], &[128, 0, 0, 128], "premultiplied, as egui takes it");
        let (size, rgba) = native::load(&dir.join("icons/T/scalable/places/folder.svg"), 64).expect("svg");
        assert_eq!(size, [64, 64], "a 16 px drawing scaled to 64");
        let at = |x: usize, y: usize| &rgba[(y * 64 + x) * 4..][..4];
        assert_eq!(at(32, 32), &[0, 0, 255, 255], "the square fills the middle");
        assert_eq!(at(2, 2)[3], 0, "and leaves the corners empty");
        assert_eq!(native::load(&dir.join("missing.svg"), 64), None);
        std::fs::write(dir.join("broken.png"), "not a picture").unwrap();
        assert_eq!(native::load(&dir.join("broken.png"), 64), None);
    }

    /// Polls until every entry has an answer, or 20 s pass.
    #[cfg(not(windows))]
    fn answered(icons: &mut FileIcons, ctx: &egui::Context, entries: &[&Entry]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        for entry in entries {
            icons.texture(entry);
        }
        while entries.iter().any(|e| !icons.loaded.contains_key(&key_of(e))) && std::time::Instant::now() < deadline {
            icons.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn rows_get_the_theme_icon_of_their_type_and_folders_theirs() {
        let ctx = egui::Context::default();
        let (_, setup) = theme_fixture("file-icons-theme");
        let mut icons = FileIcons::default();
        icons.set_setup(setup);
        let (pdf, other_pdf, text) = (entry("a.pdf"), entry("B.PDF"), entry("notes.txt"));
        let folder = Entry { is_dir: true, ..entry("Projects") };
        icons.texture(&pdf);
        icons.texture(&other_pdf);
        assert_eq!(icons.wanted.borrow().len(), 1, "one request for the type");
        answered(&mut icons, &ctx, &[&pdf, &text, &folder]);
        let shown = icons.texture(&pdf).expect("the type's PNG");
        assert_eq!(icons.texture(&other_pdf), Some(shown));
        assert!(icons.texture(&folder).is_some(), "the folder's SVG");
        let other_folder = Entry { is_dir: true, ..entry("Music") };
        answered(&mut icons, &ctx, &[&other_folder]);
        assert_eq!(icons.texture(&other_folder), icons.texture(&folder), "one texture per icon file, shared by every folder");
        assert_eq!(icons.loaded.get(&key_of(&text)).map(Option::is_none), Some(true), "the theme has no text icon: the painted one");
        assert!(icons.wanted.borrow().is_empty() && icons.pending.is_empty(), "nothing asked again");
    }

    #[cfg(not(windows))]
    #[test]
    fn symbolic_icons_load_as_a_white_shape_to_tint_and_fall_back_to_the_folder() {
        let ctx = egui::Context::default();
        let (dir, setup) = theme_fixture("file-icons-symbolic");
        let svg = |colour: &str| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="{colour}"/></svg>"#);
        std::fs::write(dir.join("icons/T/scalable/places/folder-download-symbolic.svg"), svg("#2e3436")).unwrap();
        std::fs::write(dir.join("icons/T/scalable/places/folder-symbolic.svg"), svg("#bebebe")).unwrap();
        let (size, rgba) = native::symbolic(&dir.join("icons/T/scalable/places/folder-download-symbolic.svg"), 32).expect("drawn");
        assert_eq!(size, [32, 32]);
        assert!(rgba.chunks(4).all(|px| px == [255, 255, 255, 255]), "white where drawn, whatever its colour");
        let mut icons = FileIcons::default();
        icons.set_setup(setup);
        icons.symbolic_texture("folder-download");
        icons.symbolic_texture("folder-music");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let keys = [Key::Symbolic("folder-download"), Key::Symbolic("folder-music")];
        while keys.iter().any(|k| !icons.loaded.contains_key(k)) && std::time::Instant::now() < deadline {
            icons.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(icons.symbolic_texture("folder-download").is_some());
        assert!(icons.symbolic_texture("folder-music").is_some(), "no music icon: the plain folder's");
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
