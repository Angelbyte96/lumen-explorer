//! The pictures the Icons views draw in their cards: read by the pool in
//! `fs::thumbs` and kept as textures, by path and modified time so a changed
//! file is read again. At most `MAX_PICTURES` are kept; the ones drawn
//! longest ago go first.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;

use crate::fs::Entry;
use crate::fs::thumbs::{Done, Job, Peek, Pool};

/// Textures kept at most: 256 pixels a side, about 64 MiB at worst.
pub const MAX_PICTURES: usize = 256;
/// Folder peeks kept at most (a few bytes each).
pub const MAX_PEEKS: usize = 4096;

type Key = (PathBuf, Option<SystemTime>);

struct Loaded {
    texture: egui::TextureHandle,
    /// In pixels.
    size: egui::Vec2,
    /// The poll count when it was last drawn.
    drawn: Cell<u64>,
}

/// What a folder shows sticking out of it.
pub enum Held {
    /// Empty, or not read yet.
    Nothing,
    /// Only folders inside: blank sheets.
    Folders,
    /// The first file's picture and its size in pixels.
    Picture(egui::TextureId, egui::Vec2),
    /// The first file, without a picture (yet): a sheet with its type icon.
    File(Entry),
}

pub struct Thumbnails {
    /// `None` once read: the file has no picture, its type icon stays.
    pictures: HashMap<Key, Option<Loaded>>,
    /// What each folder holds once read, with the poll count when it was last
    /// drawn.
    peeks: HashMap<Key, (Peek, Cell<u64>)>,
    /// Asked for while drawing (which only reads), in drawing order; sent to
    /// the pool on the next poll.
    wanted: RefCell<Vec<Job>>,
    polls: u64,
    /// The pool holds a list: an empty frame must clear it.
    sent: bool,
    cap: usize,
    peek_cap: usize,
    pool: Option<Pool>,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self {
            pictures: HashMap::new(),
            peeks: HashMap::new(),
            wanted: RefCell::default(),
            polls: 0,
            sent: false,
            cap: MAX_PICTURES,
            peek_cap: MAX_PEEKS,
            pool: None,
        }
    }
}

impl Thumbnails {
    /// An image file's picture and its size in pixels once read; asks for it
    /// otherwise. `None` for anything else.
    pub fn picture(&self, entry: &Entry) -> Option<(egui::TextureId, egui::Vec2)> {
        if entry.is_dir || !crate::fs::preview::is_image(&entry.path) {
            return None;
        }
        self.picture_at(&entry.path, entry.modified)
    }

    /// What `folder` shows sticking out of it; asks for its peek (and its
    /// first file's picture) while they are missing.
    pub fn held(&self, folder: &Entry) -> Held {
        let key = (folder.path.clone(), folder.modified);
        let Some((peek, drawn)) = self.peeks.get(&key) else {
            self.wanted.borrow_mut().push(Job::Peek(key.0, key.1));
            return Held::Nothing;
        };
        drawn.set(self.polls);
        match peek {
            Peek::Empty => Held::Nothing,
            Peek::OnlyFolders => Held::Folders,
            Peek::File(path, modified) => {
                if crate::fs::preview::is_image(path)
                    && let Some((texture, size)) = self.picture_at(path, *modified)
                {
                    return Held::Picture(texture, size);
                }
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Held::File(Entry { name, path: path.clone(), is_dir: false, size: None, modified: *modified, hidden: false })
            }
        }
    }

    fn picture_at(&self, path: &Path, modified: Option<SystemTime>) -> Option<(egui::TextureId, egui::Vec2)> {
        match self.pictures.get(&(path.to_path_buf(), modified)) {
            Some(Some(loaded)) => {
                loaded.drawn.set(self.polls);
                Some((loaded.texture.id(), loaded.size))
            }
            Some(None) => None,
            None => {
                self.wanted.borrow_mut().push(Job::Picture(path.to_path_buf(), modified));
                None
            }
        }
    }

    /// Sends what the views asked for to the pool and takes its results.
    /// Runs after drawing.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.polls += 1;
        let wanted = self.wanted.take();
        if !wanted.is_empty() || self.sent {
            let mut seen = HashSet::new();
            let mut jobs = Vec::new();
            for job in wanted {
                if seen.insert(job.clone()) {
                    jobs.push(job);
                }
            }
            self.sent = !jobs.is_empty();
            let pool = self.pool.get_or_insert_with(|| {
                let ctx = ctx.clone();
                Pool::start(threads(), move || ctx.request_repaint())
            });
            pool.want(jobs);
        }
        let Some(pool) = &self.pool else { return };
        let done = pool.done();
        if !done.is_empty() {
            // Drawn already this frame: the next one shows them.
            ctx.request_repaint();
        }
        for (job, done) in done {
            match (job, done) {
                (Job::Picture(path, modified), Done::Picture(picture)) => {
                    let loaded = picture.map(|picture| {
                        let image = egui::ColorImage::from_rgba_unmultiplied(picture.size, &picture.rgba);
                        Loaded {
                            texture: ctx.load_texture(format!("lumen_thumb_{}", path.display()), image, egui::TextureOptions::LINEAR),
                            size: egui::vec2(picture.size[0] as f32, picture.size[1] as f32),
                            drawn: Cell::new(self.polls),
                        }
                    });
                    self.pictures.insert((path, modified), loaded);
                }
                (Job::Peek(path, modified), Done::Peek(peek)) => {
                    self.peeks.insert((path, modified), (peek, Cell::new(self.polls)));
                }
                _ => {}
            }
        }
        self.evict();
        self.evict_peeks();
    }

    /// Drops the pictures drawn longest ago while more than `cap` are kept.
    fn evict(&mut self) {
        let mut loaded: Vec<(u64, Key)> =
            self.pictures.iter().filter_map(|(key, loaded)| Some((loaded.as_ref()?.drawn.get(), key.clone()))).collect();
        if loaded.len() <= self.cap {
            return;
        }
        loaded.sort_unstable_by_key(|(drawn, _)| *drawn);
        let extra = loaded.len() - self.cap;
        for (_, key) in loaded.into_iter().take(extra) {
            self.pictures.remove(&key);
        }
    }

    /// Drops the peeks drawn longest ago while more than `peek_cap` are kept.
    fn evict_peeks(&mut self) {
        if self.peeks.len() <= self.peek_cap {
            return;
        }
        let mut kept: Vec<(u64, Key)> = self.peeks.iter().map(|(key, (_, drawn))| (drawn.get(), key.clone())).collect();
        kept.sort_unstable_by_key(|(drawn, _)| *drawn);
        let extra = kept.len() - self.peek_cap;
        for (_, key) in kept.into_iter().take(extra) {
            self.peeks.remove(&key);
        }
    }

    /// As if `folder` (unmodified) had been read and held `peek`.
    #[cfg(test)]
    pub fn with_peek(mut self, folder: &Path, peek: Peek) -> Self {
        self.peeks.insert((folder.to_path_buf(), None), (peek, Cell::new(0)));
        self
    }

    /// The jobs asked for since the last poll.
    #[cfg(test)]
    pub fn asked(&self) -> Vec<Job> {
        self.wanted.borrow().clone()
    }

    /// The texture of `path`'s picture (unmodified), if loaded.
    #[cfg(test)]
    pub fn picture_of(&self, path: &Path) -> Option<egui::TextureId> {
        self.picture_at(path, None).map(|(texture, _)| texture)
    }

    /// As if `path` (unmodified) had been read and has no picture.
    #[cfg(test)]
    pub fn without_picture(mut self, path: &Path) -> Self {
        self.pictures.insert((path.to_path_buf(), None), None);
        self
    }

    /// A picture `size` pixels big for `path`, as if it had been read.
    #[cfg(test)]
    pub fn with_picture(mut self, ctx: &egui::Context, path: &Path, size: [usize; 2]) -> Self {
        let image = egui::ColorImage::new(size, vec![egui::Color32::RED; size[0] * size[1]]);
        let texture = ctx.load_texture("test_thumb", image, egui::TextureOptions::LINEAR);
        let loaded = Loaded { texture, size: egui::vec2(size[0] as f32, size[1] as f32), drawn: Cell::new(0) };
        self.pictures.insert((path.to_path_buf(), None), Some(loaded));
        self
    }
}

/// Worker threads for the pool: pictures decode in parallel, but Lumen
/// leaves most of the machine alone.
fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;
    use std::time::{Duration, Instant};

    fn file(path: &Path, modified: Option<SystemTime>) -> Entry {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        Entry { name, path: path.to_path_buf(), is_dir: false, size: Some(1), modified, hidden: false }
    }

    /// Draws (asks) and polls until `entry` has a picture.
    fn wait(thumbs: &mut Thumbnails, ctx: &egui::Context, entry: &Entry) -> Option<(egui::TextureId, egui::Vec2)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(found) = thumbs.picture(entry) {
                return Some(found);
            }
            thumbs.poll(ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn an_image_file_gets_its_picture_off_the_ui_thread_and_again_once_changed() {
        let dir = fresh_fixture("thumbnails-read");
        let path = dir.join("photo.png");
        image::RgbImage::new(40, 20).save(&path).unwrap();
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let entry = file(&path, Some(SystemTime::UNIX_EPOCH));
        assert!(thumbs.picture(&entry).is_none(), "not read yet");
        let (first, size) = wait(&mut thumbs, &ctx, &entry).expect("read");
        assert_eq!(size, egui::vec2(40.0, 20.0));

        image::RgbImage::new(10, 30).save(&path).unwrap();
        let changed = file(&path, Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5)));
        assert!(thumbs.picture(&changed).is_none(), "a new modified time is read again");
        let (again, size) = wait(&mut thumbs, &ctx, &changed).expect("read again");
        assert_eq!(size, egui::vec2(10.0, 30.0));
        assert_ne!(first, again);
    }

    #[test]
    fn folders_and_files_that_are_not_pictures_ask_for_nothing() {
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let notes = file(Path::new("/x/notes.txt"), None);
        let mut folder = file(Path::new("/x/photos.png"), None);
        folder.is_dir = true;
        assert!(thumbs.picture(&notes).is_none() && thumbs.picture(&folder).is_none());
        thumbs.poll(&ctx);
        assert!(thumbs.pool.is_none(), "no worker started");
    }

    #[test]
    fn a_broken_image_keeps_its_type_icon_and_is_asked_for_once() {
        let dir = fresh_fixture("thumbnails-broken");
        let path = dir.join("broken.jpg");
        std::fs::write(&path, "not a jpeg").unwrap();
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let entry = file(&path, None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !thumbs.pictures.contains_key(&(path.clone(), None)) && Instant::now() < deadline {
            thumbs.picture(&entry);
            thumbs.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(thumbs.pictures.get(&(path.clone(), None)), Some(None)), "read, no picture");
        assert!(thumbs.picture(&entry).is_none());
        assert!(thumbs.wanted.borrow().is_empty(), "not asked again");
    }

    #[test]
    fn past_the_cap_the_pictures_drawn_longest_ago_are_dropped() {
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails { cap: 2, ..Default::default() };
        for name in ["a.png", "b.png", "c.png"] {
            thumbs = thumbs.with_picture(&ctx, &Path::new("/x").join(name), [2, 2]);
        }
        thumbs.polls = 7;
        thumbs.picture(&file(Path::new("/x/a.png"), None));
        thumbs.polls = 8;
        thumbs.picture(&file(Path::new("/x/c.png"), None));
        thumbs.evict();
        let kept: HashSet<_> = thumbs.pictures.keys().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(kept, HashSet::from(["a.png".to_owned(), "c.png".to_owned()]));
    }

    fn folder(path: &Path, modified: Option<SystemTime>) -> Entry {
        Entry { is_dir: true, ..file(path, modified) }
    }

    /// Draws (asks) and polls until `ready` says so; false after ten seconds.
    fn until(thumbs: &mut Thumbnails, ctx: &egui::Context, folder: &Entry, mut ready: impl FnMut(&Held) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ready(&thumbs.held(folder)) {
                return true;
            }
            thumbs.poll(ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_folder_asks_for_its_peek_once_and_has_it_after_the_poll() {
        let dir = fresh_fixture("thumbnails-peek");
        let album = dir.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("b.txt"), "hello").unwrap();
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let entry = folder(&album, Some(SystemTime::UNIX_EPOCH));
        assert!(matches!(thumbs.held(&entry), Held::Nothing), "not read yet");
        assert!(matches!(thumbs.held(&entry), Held::Nothing));
        assert_eq!(*thumbs.wanted.borrow(), vec![Job::Peek(album.clone(), Some(SystemTime::UNIX_EPOCH)); 2], "asked while drawing");
        thumbs.poll(&ctx);
        assert!(thumbs.pool.is_some() && thumbs.wanted.borrow().is_empty(), "sent to the pool");
        let found = until(&mut thumbs, &ctx, &entry, |held| matches!(held, Held::File(file) if file.path == album.join("b.txt")));
        assert!(found, "the first file, a sheet with its type icon");
        thumbs.held(&entry);
        assert!(thumbs.wanted.borrow().is_empty(), "read, so not asked again: {:?}", thumbs.wanted.borrow());
        assert!(thumbs.peeks.contains_key(&(album, Some(SystemTime::UNIX_EPOCH))), "filed under the folder's modified time");
    }

    #[test]
    fn the_first_file_being_an_image_its_picture_is_asked_for_and_shown() {
        let dir = fresh_fixture("thumbnails-peek-image");
        let album = dir.join("album");
        std::fs::create_dir_all(&album).unwrap();
        image::RgbImage::new(40, 20).save(album.join("a.png")).unwrap();
        std::fs::write(album.join("b.txt"), "hello").unwrap();
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let entry = folder(&album, None);
        assert!(until(&mut thumbs, &ctx, &entry, |held| matches!(held, Held::File(_) | Held::Picture(..))), "peek read");
        let inner = std::fs::metadata(album.join("a.png")).unwrap().modified().ok();
        assert!(thumbs.wanted.borrow().iter().all(|job| matches!(job, Job::Picture(path, modified) if *path == album.join("a.png") && *modified == inner)));
        assert!(!thumbs.wanted.borrow().is_empty(), "the image's picture is asked for through the picture path");
        let mut shown = None;
        until(&mut thumbs, &ctx, &entry, |held| {
            shown = match held {
                Held::Picture(_, size) => Some(*size),
                _ => None,
            };
            shown.is_some()
        });
        assert_eq!(shown, Some(egui::vec2(40.0, 20.0)));
    }

    #[test]
    fn a_folder_with_only_folders_or_nothing_says_so_and_asks_for_no_picture() {
        let thumbs = Thumbnails::default()
            .with_peek(Path::new("/x/folders"), Peek::OnlyFolders)
            .with_peek(Path::new("/x/empty"), Peek::Empty)
            .with_peek(Path::new("/x/notes"), Peek::File(PathBuf::from("/x/notes/a.md"), None));
        assert!(matches!(thumbs.held(&folder(Path::new("/x/folders"), None)), Held::Folders));
        assert!(matches!(thumbs.held(&folder(Path::new("/x/empty"), None)), Held::Nothing));
        assert!(matches!(thumbs.held(&folder(Path::new("/x/notes"), None)), Held::File(file) if file.path == Path::new("/x/notes/a.md") && !file.is_dir));
        assert!(thumbs.wanted.borrow().is_empty(), "nothing to read");
    }

    #[test]
    fn a_first_file_whose_picture_cannot_be_decoded_keeps_its_sheet_with_the_type_icon() {
        let dir = fresh_fixture("thumbnails-peek-broken");
        let album = dir.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("a.png"), "not a png").unwrap();
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let entry = folder(&album, None);
        let key = (album.join("a.png"), std::fs::metadata(album.join("a.png")).unwrap().modified().ok());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(thumbs.pictures.get(&key), Some(None)) && Instant::now() < deadline {
            thumbs.held(&entry);
            thumbs.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(thumbs.held(&entry), Held::File(file) if file.path == album.join("a.png")));
        assert!(thumbs.wanted.borrow().is_empty(), "not asked again");
    }

    #[test]
    fn past_the_cap_the_peeks_drawn_longest_ago_are_dropped() {
        let mut thumbs = Thumbnails { peek_cap: 2, ..Default::default() };
        for name in ["a", "b", "c"] {
            thumbs = thumbs.with_peek(&Path::new("/x").join(name), Peek::Empty);
        }
        thumbs.polls = 7;
        thumbs.held(&folder(Path::new("/x/a"), None));
        thumbs.polls = 8;
        thumbs.held(&folder(Path::new("/x/c"), None));
        thumbs.evict_peeks();
        let kept: HashSet<_> = thumbs.peeks.keys().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(kept, HashSet::from(["a".to_owned(), "c".to_owned()]));
    }
}
