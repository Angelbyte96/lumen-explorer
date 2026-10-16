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
use crate::fs::thumbs::{Done, Job, Pool};

/// Textures kept at most: 256 pixels a side, about 64 MiB at worst.
pub const MAX_PICTURES: usize = 256;

type Key = (PathBuf, Option<SystemTime>);

struct Loaded {
    texture: egui::TextureHandle,
    /// In pixels.
    size: egui::Vec2,
    /// The poll count when it was last drawn.
    drawn: Cell<u64>,
}

pub struct Thumbnails {
    /// `None` once read: the file has no picture, its type icon stays.
    pictures: HashMap<Key, Option<Loaded>>,
    /// Asked for while drawing (which only reads), in drawing order; sent to
    /// the pool on the next poll.
    wanted: RefCell<Vec<Job>>,
    polls: u64,
    /// The pool holds a list: an empty frame must clear it.
    sent: bool,
    cap: usize,
    pool: Option<Pool>,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self { pictures: HashMap::new(), wanted: RefCell::default(), polls: 0, sent: false, cap: MAX_PICTURES, pool: None }
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
            if let (Job::Picture(path, modified), Done::Picture(picture)) = (job, done) {
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
        }
        self.evict();
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
}
