//! The flattened listing (Ctrl+E, like File Pilot's "Toggle Subfolders"):
//! everything inside some folders, all the way down, read on a background
//! thread because a big tree can take seconds to walk.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use super::entry::Entry;

/// What a [`walk`] found.
pub struct Walk {
    /// Everything inside the roots, all the way down.
    pub found: Vec<Entry>,
    /// The folders read: the roots and the folders inside them, the ones a
    /// change in shows in the listing.
    pub folders: Vec<PathBuf>,
}

/// Everything inside `roots`, all the way down (not the roots themselves).
/// Folders that cannot be read are skipped, and a link to a folder is listed
/// but not entered, so a link back up cannot loop. Returns `None` when
/// `cancel` is set.
pub fn walk(roots: &[PathBuf], cancel: &AtomicBool) -> Option<Walk> {
    let mut found = Vec::new();
    let mut folders = Vec::new();
    // A stack, not recursion: a very deep tree cannot overflow it.
    let mut pending: Vec<PathBuf> = roots.iter().rev().cloned().collect();
    while let Some(dir) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(mut read) = super::lister::read_folder(&dir) else { continue };
        read.sort_by_cached_key(|f| (!f.entry.is_dir, f.entry.name.to_lowercase()));
        pending.extend(read.iter().filter(|f| f.entry.is_dir && !f.link).map(|f| f.entry.path.clone()).rev());
        found.extend(read.into_iter().map(|f| f.entry));
        folders.push(dir);
    }
    (!cancel.load(Ordering::Relaxed)).then_some(Walk { found, folders })
}

/// A walk running on its own thread. Dropping it cancels the walk.
pub struct Flattener {
    roots: Vec<PathBuf>,
    rx: Receiver<Walk>,
    cancel: Arc<AtomicBool>,
}

impl Flattener {
    /// Starts [`walk`] on a new thread; `wake` runs there once it is done.
    pub fn start(roots: Vec<PathBuf>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let walked = roots.clone();
        thread::spawn(move || {
            if let Some(found) = walk(&walked, &flag) {
                // The receiver only disappears when the walk is no longer wanted.
                let _ = tx.send(found);
                wake();
            }
        });
        Self { roots, rx, cancel }
    }

    /// The roots walked and what they hold, once the walk is done.
    pub fn poll(&self) -> Option<(Vec<PathBuf>, Walk)> {
        self.rx.try_recv().ok().map(|found| (self.roots.clone(), found))
    }
}

impl Drop for Flattener {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::fs::ops::tests::fresh_fixture;
    use std::fs;

    fn names(entries: &[Entry], root: &Path) -> Vec<String> {
        let mut names: Vec<String> = entries
            .iter()
            .map(|e| e.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        names.sort();
        names
    }

    fn tree(name: &str) -> PathBuf {
        let root = fresh_fixture(name);
        fs::create_dir_all(root.join("deps/inner")).unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("top.txt"), "1").unwrap();
        fs::write(root.join("deps/lib.d"), "22").unwrap();
        fs::write(root.join("deps/inner/deep.rs"), "333").unwrap();
        root
    }

    #[test]
    fn walk_lists_every_file_and_folder_under_the_root() {
        let root = tree("flat-walk-all");
        let found = walk(std::slice::from_ref(&root), &AtomicBool::new(false)).unwrap().found;
        assert_eq!(names(&found, &root), ["build", "deps", "deps/inner", "deps/inner/deep.rs", "deps/lib.d", "top.txt"]);
        let deep = found.iter().find(|e| e.name == "deep.rs").unwrap();
        assert_eq!(deep.size, Some(3));
        assert!(!deep.is_dir);
        assert!(found.iter().find(|e| e.name == "inner").unwrap().is_dir);
    }

    #[test]
    fn walk_of_several_roots_lists_only_what_they_hold() {
        let root = tree("flat-walk-roots");
        let found = walk(&[root.join("deps")], &AtomicBool::new(false)).unwrap().found;
        assert_eq!(names(&found, &root), ["deps/inner", "deps/inner/deep.rs", "deps/lib.d"]);
    }

    #[test]
    fn walk_tells_the_folders_it_read() {
        let root = tree("flat-walk-folders");
        let walked = walk(&[root.join("deps"), root.join("nope")], &AtomicBool::new(false)).unwrap();
        assert_eq!(walked.folders, [root.join("deps"), root.join("deps/inner")], "the root too; not one that cannot be read");
    }

    #[cfg(unix)]
    #[test]
    fn walk_does_not_tell_a_link_to_a_folder_as_read() {
        let root = tree("flat-walk-folders-link");
        std::os::unix::fs::symlink(root.join("build"), root.join("deps/link")).unwrap();
        let walked = walk(&[root.join("deps")], &AtomicBool::new(false)).unwrap();
        assert_eq!(walked.folders, [root.join("deps"), root.join("deps/inner")]);
        assert!(walked.found.iter().any(|e| e.name == "link"), "listed all the same");
    }

    #[test]
    fn walk_stops_when_cancelled() {
        let root = tree("flat-walk-cancel");
        assert!(walk(&[root], &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn an_unreadable_root_lists_nothing() {
        let root = tree("flat-walk-missing");
        let walked = walk(&[root.join("nope")], &AtomicBool::new(false)).unwrap();
        assert!(walked.found.is_empty());
        assert!(walked.folders.is_empty());
    }

    #[test]
    fn a_flattener_delivers_its_walk() {
        let root = tree("flat-walk-thread");
        let (tx, rx) = mpsc::channel();
        let flattener = Flattener::start(vec![root.join("deps")], move || {
            let _ = tx.send(());
        });
        rx.recv().unwrap();
        let (roots, walked) = flattener.poll().expect("finished");
        assert_eq!(roots, [root.join("deps")]);
        assert_eq!(walked.found.len(), 3);
    }
}
