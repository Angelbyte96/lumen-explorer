//! Directory listing on a background thread.
//!
//! The UI calls [`Lister::request`] and polls [`Lister::poll`] every frame.
//! Each request gets an increasing id; results from superseded requests are
//! dropped, so navigating fast never shows a stale folder.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::entry::{Entry, is_hidden_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    NotFound(PathBuf),
    PermissionDenied(PathBuf),
    NotADirectory(PathBuf),
    Other(PathBuf, String),
}

impl ListError {
    fn from_io(path: &Path, err: &io::Error) -> Self {
        let path = path.to_path_buf();
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path),
            io::ErrorKind::PermissionDenied => Self::PermissionDenied(path),
            io::ErrorKind::NotADirectory => Self::NotADirectory(path),
            _ => Self::Other(path, err.to_string()),
        }
    }
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "Path does not exist: {}", p.display()),
            Self::PermissionDenied(p) => write!(f, "Permission denied: {}", p.display()),
            Self::NotADirectory(p) => write!(f, "Not a directory: {}", p.display()),
            Self::Other(p, msg) => write!(f, "Cannot open {}: {msg}", p.display()),
        }
    }
}

/// Result of one listing request.
#[derive(Debug)]
pub struct Listing {
    pub id: u64,
    pub path: PathBuf,
    pub result: Result<Vec<Entry>, ListError>,
    pub elapsed: Duration,
}

/// Reads and sorts a directory synchronously. Runs on the worker thread.
pub fn read_entries(path: &Path) -> Result<Vec<Entry>, ListError> {
    // This PC is no folder: its drives are drawn from the side panel's disks.
    if crate::places::is_this_pc(path) {
        return Ok(Vec::new());
    }
    // `read_dir` on a file fails with a generic error on some platforms, so check first.
    let meta = std::fs::metadata(path).map_err(|e| ListError::from_io(path, &e))?;
    if !meta.is_dir() {
        return Err(ListError::NotADirectory(path.to_path_buf()));
    }
    let iter = std::fs::read_dir(path).map_err(|e| ListError::from_io(path, &e))?;

    let mut entries = Vec::new();
    for item in iter {
        // A single unreadable entry should not fail the whole folder.
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let path = item.path();
        // Follow symlinks so a link to a folder behaves like a folder;
        // fall back to the link itself when it is broken.
        let meta = std::fs::metadata(&path).or_else(|_| item.metadata()).ok();
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        entries.push(Entry {
            hidden: is_hidden_name(&name),
            size: meta.as_ref().filter(|_| !is_dir).map(|m| m.len()),
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            name,
            path,
            is_dir,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// Folders first, then case-insensitive by name.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
}

pub struct Lister {
    tx: Sender<Listing>,
    rx: Receiver<Listing>,
    latest_id: u64,
    #[cfg(test)]
    latest_path: Option<PathBuf>,
    /// Print each listing time to stderr (enabled with `LUMEN_LOG=1`).
    log: bool,
}

impl Lister {
    pub fn new(log: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            latest_id: 0,
            #[cfg(test)]
            latest_path: None,
            log,
        }
    }

    /// Starts listing `path` on a new thread. `on_done` runs on that thread when
    /// the result is ready (the UI uses it to request a repaint).
    pub fn request(&mut self, path: PathBuf, on_done: impl FnOnce() + Send + 'static) -> u64 {
        self.latest_id += 1;
        let id = self.latest_id;
        #[cfg(test)]
        {
            self.latest_path = Some(path.clone());
        }
        let tx = self.tx.clone();
        let log = self.log;
        thread::spawn(move || {
            let start = Instant::now();
            let result = read_entries(&path);
            let elapsed = start.elapsed();
            if log {
                match &result {
                    Ok(e) => eprintln!(
                        "[lumen] listed {} entries in {:.3} ms: {}",
                        e.len(),
                        elapsed.as_secs_f64() * 1000.0,
                        path.display()
                    ),
                    Err(err) => eprintln!("[lumen] listing failed: {err}"),
                }
            }
            // The receiver only disappears when the app is closing; nothing to do then.
            let _ = tx.send(Listing { id, path, result, elapsed });
            on_done();
        });
        id
    }

    /// Returns the result of the most recent request once it is ready.
    pub fn poll(&self) -> Option<Listing> {
        let mut newest = None;
        while let Ok(listing) = self.rx.try_recv() {
            if listing.id == self.latest_id {
                newest = Some(listing);
            }
        }
        newest
    }

    pub fn is_pending(&self, shown_id: u64) -> bool {
        shown_id != self.latest_id
    }

    /// Id of the newest request.
    #[cfg(test)]
    pub fn latest_id(&self) -> u64 {
        self.latest_id
    }

    /// The folder of the newest request.
    #[cfg(test)]
    pub fn last_requested(&self) -> Option<PathBuf> {
        self.latest_path.clone()
    }
}

/// Reads several folders at once, each on a thread of its own (the Tree
/// view's unfolded folders). Unlike [`Lister`], every result is delivered:
/// none of them supersedes another.
pub struct Readers {
    tx: Sender<Listing>,
    rx: Receiver<Listing>,
    next_id: u64,
}

impl Default for Readers {
    fn default() -> Self {
        Self::new()
    }
}

impl Readers {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, next_id: 0 }
    }

    /// Starts reading `path` on a new thread; `on_done` runs on it once the
    /// result is ready. Returns the id the result will carry: later requests
    /// have bigger ids.
    pub fn request(&mut self, path: PathBuf, on_done: impl FnOnce() + Send + 'static) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let result = read_entries(&path);
            // The receiver only disappears when the app is closing; nothing to do then.
            let _ = tx.send(Listing { id, path, result, elapsed: start.elapsed() });
            on_done();
        });
        id
    }

    /// Every result that is ready.
    pub fn poll(&self) -> Vec<Listing> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test fixtures live only under ~/pruebas-explorador (project safety rule).
    /// They are created idempotently and never deleted by the tests.
    fn fixture(name: &str) -> PathBuf {
        let root = crate::fs::home_dir()
            .expect("HOME must be set")
            .join("pruebas-explorador")
            .join("test-fixtures")
            .join(name);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Each test passes its own name: tests run in parallel and must not
    /// rewrite files another test is reading.
    fn sample_dir(name: &str) -> PathBuf {
        let dir = fixture(name);
        for d in ["beta_dir", "Alpha_dir", ".hidden_dir"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for (f, body) in [("zeta.txt", "12345"), ("Apple.txt", ""), (".hidden.txt", "x")] {
            std::fs::write(dir.join(f), body).unwrap();
        }
        dir
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_folders_first_then_case_insensitive_names() {
        let entries = read_entries(&sample_dir("sorted")).unwrap();
        assert_eq!(
            names(&entries),
            [".hidden_dir", "Alpha_dir", "beta_dir", ".hidden.txt", "Apple.txt", "zeta.txt"]
        );
    }

    #[test]
    fn entries_carry_size_date_and_hidden_flag() {
        let entries = read_entries(&sample_dir("metadata")).unwrap();
        let zeta = entries.iter().find(|e| e.name == "zeta.txt").unwrap();
        assert_eq!(zeta.size, Some(5));
        assert!(zeta.modified.is_some());
        assert!(!zeta.is_dir && !zeta.hidden);
        let dir = entries.iter().find(|e| e.name == "Alpha_dir").unwrap();
        assert!(dir.is_dir);
        assert_eq!(dir.size, None);
        assert!(entries.iter().find(|e| e.name == ".hidden.txt").unwrap().hidden);
    }

    #[test]
    fn this_pc_lists_no_entries_whatever_the_working_directory_holds() {
        assert_eq!(read_entries(&crate::places::this_pc()).unwrap().len(), 0);
    }

    #[test]
    fn missing_path_is_not_found() {
        let p = fixture("missing").join("does-not-exist");
        assert_eq!(read_entries(&p).unwrap_err(), ListError::NotFound(p));
    }

    #[test]
    fn file_path_is_not_a_directory() {
        let p = sample_dir("not-a-dir").join("zeta.txt");
        assert_eq!(read_entries(&p).unwrap_err(), ListError::NotADirectory(p));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_dir_is_permission_denied() {
        // /root is mode 700 on Ubuntu; read-only check, nothing is written there.
        let p = Path::new("/root");
        if std::fs::read_dir(p).is_ok() {
            return; // running as root: nothing to prove
        }
        assert_eq!(
            read_entries(p).unwrap_err(),
            ListError::PermissionDenied(p.to_path_buf())
        );
    }

    #[test]
    fn error_messages_are_readable() {
        let p = PathBuf::from("/nope");
        assert_eq!(ListError::NotFound(p.clone()).to_string(), "Path does not exist: /nope");
        assert_eq!(ListError::PermissionDenied(p).to_string(), "Permission denied: /nope");
    }

    #[test]
    fn background_request_delivers_latest_result_only() {
        let dir = sample_dir("background");
        let mut lister = Lister::new(false);
        let (done_tx, done_rx) = mpsc::channel();
        let t1 = done_tx.clone();
        let first = lister.request(dir.join("does-not-exist"), move || t1.send(()).unwrap());
        let second = lister.request(dir.clone(), move || done_tx.send(()).unwrap());
        assert!(second > first);
        assert!(lister.is_pending(first));
        for _ in 0..2 {
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let listing = lister.poll().expect("latest result");
        assert_eq!(listing.id, second);
        assert_eq!(listing.path, dir);
        assert_eq!(listing.result.unwrap().len(), 6);
        assert!(!lister.is_pending(listing.id));
        assert!(lister.poll().is_none(), "stale result must be discarded");
    }

    #[test]
    fn readers_deliver_every_folder_asked_for_not_only_the_newest() {
        let dir = sample_dir("readers");
        let mut readers = Readers::new();
        let (done_tx, done_rx) = mpsc::channel();
        let mut ids = Vec::new();
        for path in [dir.clone(), dir.join("Alpha_dir"), dir.join("does-not-exist")] {
            let done = done_tx.clone();
            ids.push(readers.request(path, move || done.send(()).unwrap()));
        }
        for _ in 0..3 {
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let mut listings = readers.poll();
        listings.sort_by_key(|l| l.id);
        let paths: Vec<&Path> = listings.iter().map(|l| l.path.as_path()).collect();
        assert_eq!(paths, [dir.as_path(), dir.join("Alpha_dir").as_path(), dir.join("does-not-exist").as_path()]);
        assert_eq!(listings[0].result.as_ref().unwrap().len(), 6);
        assert!(listings[1].result.as_ref().unwrap().is_empty());
        assert!(matches!(listings[2].result, Err(ListError::NotFound(_))), "an unreadable folder is an error, not a crash");
        let delivered: Vec<u64> = listings.iter().map(|l| l.id).collect();
        assert_eq!(ids, delivered, "a request returns the id its result will carry");
        assert!(readers.poll().is_empty(), "each result is delivered once");
    }
}
