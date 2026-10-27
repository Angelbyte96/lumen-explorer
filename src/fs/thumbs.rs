//! What the Icons views draw inside a card, read on background threads: an
//! image file's own picture, and what a folder holds (its peek).
//!
//! The UI tells the pool each frame what is on screen and still missing, in
//! the order it wants it. That list replaces the queue, so scrolling past a
//! folder of photos never decodes the ones left behind.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::SystemTime;

use super::entry::is_hidden_name;
use super::preview::{self, IMAGE_LIMIT, Picture};
use super::video;

/// A picture's longest side in pixels: the XL card's icon (224 points) with
/// some room for a scaled screen.
pub const THUMB_SIDE: u32 = 256;

/// What a folder shows from inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peek {
    /// Nothing inside (or nothing readable).
    Empty,
    /// Only folders inside.
    OnlyFolders,
    /// The first file inside, by name, and when it was modified (the key its
    /// picture is filed under).
    File(PathBuf, Option<SystemTime>),
}

/// Whether the file at `path` has a picture of its own: a PNG or JPEG, or a
/// video (one of its frames).
pub fn has_picture(path: &Path) -> bool {
    preview::is_image(path) || video::is_video(path)
}

/// An image file's picture, or a video's frame, scaled to fit `THUMB_SIDE`;
/// `None` for any other file, an image too big, or one that cannot be read.
pub fn thumbnail(path: &Path) -> Option<Picture> {
    if video::is_video(path) {
        return video::frame(path, THUMB_SIDE).ok();
    }
    if !preview::is_image(path) {
        return None;
    }
    // Follows links; FIFOs and devices are not read.
    std::fs::metadata(path).ok().filter(|m| m.is_file() && m.len() <= IMAGE_LIMIT)?;
    preview::decode_fit(path, THUMB_SIDE).ok()
}

/// What `folder` holds: its first file by name (ignoring case) and when it
/// was modified, else whether it holds folders. Hidden entries are left out.
pub fn peek(folder: &Path) -> Peek {
    let Ok(items) = std::fs::read_dir(folder) else { return Peek::Empty };
    let mut first: Option<(String, PathBuf)> = None;
    let mut folders = false;
    for item in items.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if is_hidden_name(&name) {
            continue;
        }
        let path = item.path();
        // The entry's type needs no extra read unless it is a link, which
        // counts as what it points to.
        let is_dir = match item.file_type() {
            Ok(kind) if kind.is_symlink() => std::fs::metadata(&path).is_ok_and(|m| m.is_dir()),
            Ok(kind) => kind.is_dir(),
            Err(_) => continue,
        };
        if is_dir {
            folders = true;
            continue;
        }
        let key = name.to_lowercase();
        if first.as_ref().is_none_or(|(best, _)| key < *best) {
            first = Some((key, path));
        }
    }
    match first {
        Some((_, path)) => {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            Peek::File(path, modified)
        }
        None if folders => Peek::OnlyFolders,
        None => Peek::Empty,
    }
}

/// One thing to read for the Icons views, with the modified time it was
/// asked for, so its result is filed under it even if the file changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Job {
    Picture(PathBuf, Option<SystemTime>),
    Peek(PathBuf, Option<SystemTime>),
}

pub enum Done {
    Picture(Option<Picture>),
    Peek(Peek),
}

/// The jobs waiting, most wanted first, and the ones being read or read and
/// not yet taken by the UI (which keeps asking for them until then).
#[derive(Default)]
struct Queue {
    waiting: Vec<Job>,
    running: HashSet<Job>,
    closed: bool,
}

impl Queue {
    /// Replaces what waits with `jobs`, leaving out the ones already being read.
    fn want(&mut self, mut jobs: Vec<Job>) {
        jobs.retain(|job| !self.running.contains(job));
        self.waiting = jobs;
    }

    /// The most wanted job, now counted as running.
    fn take(&mut self) -> Option<Job> {
        if self.waiting.is_empty() {
            return None;
        }
        let job = self.waiting.remove(0);
        self.running.insert(job.clone());
        Some(job)
    }

    fn finish(&mut self, job: &Job) {
        self.running.remove(job);
    }
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

/// Worker threads reading pictures and peeks; they stop when it is dropped.
pub struct Pool {
    queue: Shared,
    results: Receiver<(Job, Done)>,
}

impl Pool {
    /// Starts `threads` workers; `wake` runs after each result (to repaint).
    pub fn start(threads: usize, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let queue: Shared = Arc::default();
        let (outbox, results) = mpsc::channel();
        let wake = Arc::new(wake);
        for _ in 0..threads.max(1) {
            let (queue, outbox, wake) = (queue.clone(), outbox.clone(), wake.clone());
            thread::spawn(move || work(&queue, &outbox, &*wake));
        }
        Self { queue, results }
    }

    /// What to read next, most wanted first; replaces what was asked before.
    pub fn want(&self, jobs: Vec<Job>) {
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).want(jobs);
        ready.notify_all();
    }

    /// The results read since the last call.
    pub fn done(&self) -> Vec<(Job, Done)> {
        let done: Vec<(Job, Done)> = self.results.try_iter().collect();
        if !done.is_empty() {
            let mut queue = self.queue.0.lock().unwrap_or_else(|e| e.into_inner());
            for (job, _) in &done {
                queue.finish(job);
            }
        }
        done
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        ready.notify_all();
    }
}

fn work(queue: &Shared, outbox: &Sender<(Job, Done)>, wake: &(dyn Fn() + Send + Sync)) {
    let (lock, ready) = &**queue;
    loop {
        let job = {
            let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if queue.closed {
                    return;
                }
                if let Some(job) = queue.take() {
                    break job;
                }
                queue = ready.wait(queue).unwrap_or_else(|e| e.into_inner());
            }
        };
        let done = match &job {
            Job::Picture(path, _) => Done::Picture(thumbnail(path)),
            Job::Peek(path, _) => Done::Peek(peek(path)),
        };
        if outbox.send((job, done)).is_err() {
            return;
        }
        wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;
    use std::time::{Duration, Instant};

    #[test]
    fn a_picture_is_scaled_to_fit_the_thumb_side_keeping_its_shape() {
        let dir = fresh_fixture("thumbs-picture");
        image::RgbImage::new(1024, 512).save(dir.join("wide.png")).unwrap();
        image::RgbImage::new(100, 200).save(dir.join("small.jpg")).unwrap();
        let wide = thumbnail(&dir.join("wide.png")).expect("a png has a picture");
        assert_eq!(wide.size, [THUMB_SIDE as usize, THUMB_SIDE as usize / 2]);
        assert_eq!(wide.original, [1024, 512]);
        let small = thumbnail(&dir.join("small.jpg")).expect("a jpeg has a picture");
        assert_eq!(small.size, [100, 200], "a small picture is not scaled up");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_video_has_a_frame_scaled_to_fit_the_thumb_side() {
        let dir = fresh_fixture("thumbs-video");
        crate::fs::video::tests::write_video(&dir.join("wide.mkv"), 512, 256, 2.0);
        let frame = thumbnail(&dir.join("wide.mkv")).expect("a video has a frame");
        assert_eq!((frame.size, frame.original), ([THUMB_SIDE as usize, THUMB_SIDE as usize / 2], [512, 256]));
    }

    #[test]
    fn images_and_videos_have_pictures_other_files_do_not() {
        for name in ["a.png", "b.JPG", "c.mp4", "d.webm"] {
            assert!(has_picture(Path::new(name)), "{name}");
        }
        for name in ["a.txt", "b.pdf", "c.ts"] {
            assert!(!has_picture(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn files_that_are_not_pictures_or_too_big_or_broken_have_none() {
        let dir = fresh_fixture("thumbs-none");
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        std::fs::write(dir.join("broken.png"), "not a png").unwrap();
        let huge = std::fs::File::create(dir.join("huge.jpg")).unwrap();
        huge.set_len(IMAGE_LIMIT + 1).unwrap();
        std::fs::write(dir.join("broken.mp4"), "not a video").unwrap();
        for name in ["notes.txt", "broken.png", "huge.jpg", "missing.png", "broken.mp4"] {
            assert!(thumbnail(&dir.join(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn a_folder_peeks_its_first_file_by_name_else_says_what_it_holds() {
        let dir = fresh_fixture("thumbs-peek");
        let files = dir.join("files");
        std::fs::create_dir_all(files.join("Aaa-folder")).unwrap();
        std::fs::write(files.join("c.txt"), "").unwrap();
        std::fs::write(files.join("B.png"), "").unwrap();
        std::fs::write(files.join(".a-hidden"), "").unwrap();
        std::fs::write(files.join("a.md"), "").unwrap();
        let modified = std::fs::metadata(files.join("a.md")).unwrap().modified().ok();
        assert!(modified.is_some());
        assert_eq!(peek(&files), Peek::File(files.join("a.md"), modified), "by name ignoring case, folders and hidden files left out, with its modified time");

        let folders = dir.join("folders");
        std::fs::create_dir_all(folders.join("one")).unwrap();
        std::fs::create_dir_all(folders.join("two")).unwrap();
        assert_eq!(peek(&folders), Peek::OnlyFolders);

        let hidden = dir.join("hidden");
        std::fs::create_dir_all(hidden.join(".git")).unwrap();
        std::fs::write(hidden.join(".env"), "").unwrap();
        assert_eq!(peek(&hidden), Peek::Empty, "hidden entries do not count");

        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(peek(&empty), Peek::Empty);
        assert_eq!(peek(&dir.join("missing")), Peek::Empty);
    }

    fn job(name: &str) -> Job {
        Job::Peek(PathBuf::from(name), None)
    }

    #[test]
    fn the_queue_takes_the_most_wanted_first_and_a_new_list_replaces_the_old() {
        let mut queue = Queue::default();
        queue.want(vec![job("a"), job("b"), job("c")]);
        assert_eq!(queue.take(), Some(job("a")));
        queue.want(vec![job("a"), job("d"), job("b")]);
        assert_eq!(queue.take(), Some(job("d")), "a is being read, c is no longer wanted");
        assert_eq!(queue.take(), Some(job("b")));
        assert_eq!(queue.take(), None);
        queue.finish(&job("a"));
        queue.want(vec![job("a")]);
        assert_eq!(queue.take(), Some(job("a")), "asked again once read");
    }

    #[test]
    fn the_pool_reads_what_is_wanted_and_wakes_the_ui() {
        let dir = fresh_fixture("thumbs-pool");
        image::RgbImage::new(4, 2).save(dir.join("a.png")).unwrap();
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = woken.clone();
        let pool = Pool::start(2, move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        pool.want(vec![Job::Picture(dir.join("a.png"), None), Job::Peek(dir.clone(), None)]);
        let mut done = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while done.len() < 2 && Instant::now() < deadline {
            done.extend(pool.done());
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(done.len(), 2);
        for (job, result) in done {
            match (job, result) {
                (Job::Picture(..), Done::Picture(picture)) => assert_eq!(picture.map(|p| p.size), Some([4, 2])),
                (Job::Peek(..), Done::Peek(peek)) => assert!(matches!(peek, Peek::File(path, _) if path == dir.join("a.png"))),
                _ => panic!("a result of the wrong kind"),
            }
        }
        assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn a_job_read_but_not_yet_taken_is_not_read_again() {
        let dir = fresh_fixture("thumbs-taken");
        image::RgbImage::new(4, 2).save(dir.join("a.png")).unwrap();
        let pool = Pool::start(1, || {});
        let job = Job::Picture(dir.join("a.png"), None);
        pool.want(vec![job.clone()]);
        // Read, its result waiting: the UI still asks for it.
        thread::sleep(Duration::from_millis(200));
        pool.want(vec![job.clone()]);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(pool.done().len(), 1, "read once");
        pool.want(vec![job.clone()]);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(pool.done().len(), 1, "asked again once taken, read again");
    }
}
