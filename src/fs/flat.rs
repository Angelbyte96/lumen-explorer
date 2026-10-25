//! The flattened listing (Ctrl+E, like File Pilot's "Toggle Subfolders"):
//! everything inside some folders, all the way down, read on a background
//! thread because a big tree can take seconds to walk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// `seen` keeps how many entries it found so far.
pub fn walk(roots: &[PathBuf], cancel: &AtomicBool, seen: &AtomicUsize) -> Option<Walk> {
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
        seen.store(found.len(), Ordering::Relaxed);
        folders.push(dir);
    }
    (!cancel.load(Ordering::Relaxed)).then_some(Walk { found, folders })
}

/// A walk running on its own thread. Dropping it cancels the walk.
pub struct Flattener {
    roots: Vec<PathBuf>,
    rx: Receiver<Walk>,
    cancel: Arc<AtomicBool>,
    seen: Arc<AtomicUsize>,
}

impl Flattener {
    /// Starts [`walk`] on a new thread; `wake` runs there once it is done.
    pub fn start(roots: Vec<PathBuf>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let seen = Arc::new(AtomicUsize::new(0));
        let count = seen.clone();
        let walked = roots.clone();
        thread::spawn(move || {
            if let Some(found) = walk(&walked, &flag, &count) {
                // The receiver only disappears when the walk is no longer wanted.
                let _ = tx.send(found);
                wake();
            }
        });
        Self { roots, rx, cancel, seen }
    }

    /// How many entries the walk found so far.
    pub fn found(&self) -> usize {
        self.seen.load(Ordering::Relaxed)
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

/// What a [`reread`] found.
pub struct Reread {
    /// Each folder read again, with what it holds now (sorted like a listing).
    pub folders: Vec<(PathBuf, Vec<Entry>)>,
    /// Folders that are gone or cannot be read: what they held leaves the
    /// listing (they themselves stay, until the folder holding them is read).
    pub gone: Vec<PathBuf>,
    /// What the folders that appeared hold, all the way down.
    pub walked: Walk,
}

impl Reread {
    /// Brings the folders a walk read up to date: what is under a gone one
    /// leaves, the folders that appeared come in.
    pub fn update_folders(&self, folders: &mut Vec<PathBuf>) {
        folders.retain(|folder| !self.gone.iter().any(|gone| folder.starts_with(gone)));
        folders.extend(self.walked.folders.iter().cloned());
    }
}

/// Reads `folders` again, each with the subfolders it held before, and walks
/// the subfolders that appeared inside `roots`. A folder that cannot be read
/// counts as gone. Returns `None` when `cancel` is set.
pub fn reread(roots: &[PathBuf], folders: &[(PathBuf, HashSet<PathBuf>)], cancel: &AtomicBool) -> Option<Reread> {
    let (mut read, mut gone, mut fresh) = (Vec::new(), Vec::new(), Vec::new());
    for (folder, before) in folders {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(found) = super::lister::read_folder(folder) else {
            gone.push(folder.clone());
            continue;
        };
        {
            // A link is listed but never entered, as in a walk.
            let now: HashSet<&Path> = found.iter().filter(|f| f.entry.is_dir && !f.link).map(|f| f.entry.path.as_path()).collect();
            gone.extend(before.iter().filter(|old| !now.contains(old.as_path())).cloned());
            fresh.extend(now.iter().filter(|new| !before.contains(**new) && roots.iter().any(|root| new.starts_with(root))).map(|new| new.to_path_buf()));
        }
        let mut entries: Vec<Entry> = found.into_iter().map(|f| f.entry).collect();
        super::lister::sort_entries(&mut entries);
        read.push((folder.clone(), entries));
    }
    fresh.sort();
    gone.sort();
    gone.dedup();
    // The walk of a folder that appeared reads it, and the ones inside.
    read.retain(|(folder, _)| !fresh.iter().any(|new| folder.starts_with(new)));
    let walked = walk(&fresh, cancel, &AtomicUsize::new(0))?;
    Some(Reread { folders: read, gone, walked })
}

/// A re-read running on its own thread. Dropping it cancels it.
pub struct Refresher {
    roots: Vec<PathBuf>,
    current: PathBuf,
    rx: Receiver<Reread>,
    cancel: Arc<AtomicBool>,
}

impl Refresher {
    /// Starts [`reread`] on a new thread; `wake` runs there once it is done.
    pub fn start(roots: Vec<PathBuf>, current: PathBuf, folders: Vec<(PathBuf, HashSet<PathBuf>)>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let walked = roots.clone();
        thread::spawn(move || {
            if let Some(found) = reread(&walked, &folders, &flag) {
                // The receiver only disappears when the result is no longer wanted.
                let _ = tx.send(found);
                wake();
            }
        });
        Self { roots, current, rx, cancel }
    }

    /// The roots and the open folder it was started for, and what it found, once it is done.
    pub fn poll(&self) -> Option<(Vec<PathBuf>, PathBuf, Reread)> {
        self.rx.try_recv().ok().map(|found| (self.roots.clone(), self.current.clone(), found))
    }
}

impl Drop for Refresher {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let found = walk(std::slice::from_ref(&root), &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap().found;
        assert_eq!(names(&found, &root), ["build", "deps", "deps/inner", "deps/inner/deep.rs", "deps/lib.d", "top.txt"]);
        let deep = found.iter().find(|e| e.name == "deep.rs").unwrap();
        assert_eq!(deep.size, Some(3));
        assert!(!deep.is_dir);
        assert!(found.iter().find(|e| e.name == "inner").unwrap().is_dir);
    }

    #[test]
    fn walk_of_several_roots_lists_only_what_they_hold() {
        let root = tree("flat-walk-roots");
        let found = walk(&[root.join("deps")], &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap().found;
        assert_eq!(names(&found, &root), ["deps/inner", "deps/inner/deep.rs", "deps/lib.d"]);
    }

    #[test]
    fn walk_tells_the_folders_it_read() {
        let root = tree("flat-walk-folders");
        let walked = walk(&[root.join("deps"), root.join("nope")], &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
        assert_eq!(walked.folders, [root.join("deps"), root.join("deps/inner")], "the root too; not one that cannot be read");
    }

    #[cfg(unix)]
    #[test]
    fn walk_does_not_tell_a_link_to_a_folder_as_read() {
        let root = tree("flat-walk-folders-link");
        std::os::unix::fs::symlink(root.join("build"), root.join("deps/link")).unwrap();
        let walked = walk(&[root.join("deps")], &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
        assert_eq!(walked.folders, [root.join("deps"), root.join("deps/inner")]);
        assert!(walked.found.iter().any(|e| e.name == "link"), "listed all the same");
    }

    #[test]
    fn walk_counts_what_it_found_as_it_goes() {
        let root = tree("flat-walk-progress");
        let seen = AtomicUsize::new(0);
        let walked = walk(std::slice::from_ref(&root), &AtomicBool::new(false), &seen).unwrap();
        assert_eq!(seen.load(Ordering::Relaxed), walked.found.len());
        assert_eq!(seen.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn walk_stops_when_cancelled() {
        let root = tree("flat-walk-cancel");
        assert!(walk(&[root], &AtomicBool::new(true), &AtomicUsize::new(0)).is_none());
    }

    #[test]
    fn an_unreadable_root_lists_nothing() {
        let root = tree("flat-walk-missing");
        let walked = walk(&[root.join("nope")], &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
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
        assert_eq!(flattener.found(), 3, "what it found so far");
        let (roots, walked) = flattener.poll().expect("finished");
        assert_eq!(roots, [root.join("deps")]);
        assert_eq!(walked.found.len(), 3);
    }

    fn held(folders: &[PathBuf]) -> HashSet<PathBuf> {
        folders.iter().cloned().collect()
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn a_reread_finds_a_new_file() {
        let root = tree("flat-reread-new");
        let deps = root.join("deps");
        fs::write(deps.join("new.rs"), "4444").unwrap();
        let read = reread(std::slice::from_ref(&root), &[(deps.clone(), held(&[deps.join("inner")]))], &never()).unwrap();
        assert_eq!(read.folders.len(), 1);
        assert_eq!(read.folders[0].0, deps);
        assert_eq!(names(&read.folders[0].1, &root), ["deps/inner", "deps/lib.d", "deps/new.rs"]);
        assert_eq!(read.folders[0].1[0].name, "inner", "sorted like a listing: folders first");
        assert_eq!(read.folders[0].1.iter().find(|e| e.name == "new.rs").unwrap().size, Some(4));
        assert!(read.gone.is_empty());
        assert!(read.walked.found.is_empty() && read.walked.folders.is_empty(), "no folder appeared: nothing walked");
    }

    #[test]
    fn a_reread_drops_a_removed_file() {
        let root = tree("flat-reread-removed");
        let deps = root.join("deps");
        fs::remove_file(deps.join("lib.d")).unwrap();
        let read = reread(std::slice::from_ref(&root), &[(deps.clone(), held(&[deps.join("inner")]))], &never()).unwrap();
        assert_eq!(names(&read.folders[0].1, &root), ["deps/inner"]);
        assert!(read.gone.is_empty());
    }

    #[test]
    fn a_reread_walks_a_new_subfolder_all_the_way_down() {
        let root = tree("flat-reread-fresh");
        let deps = root.join("deps");
        fs::create_dir_all(deps.join("fresh/more")).unwrap();
        fs::write(deps.join("fresh/x.txt"), "x").unwrap();
        fs::write(deps.join("fresh/more/y.txt"), "y").unwrap();
        let read = reread(std::slice::from_ref(&root), &[(deps.clone(), held(&[deps.join("inner")]))], &never()).unwrap();
        assert_eq!(names(&read.folders[0].1, &root), ["deps/fresh", "deps/inner", "deps/lib.d"]);
        assert_eq!(names(&read.walked.found, &root), ["deps/fresh/more", "deps/fresh/more/y.txt", "deps/fresh/x.txt"]);
        assert_eq!(read.walked.folders, [deps.join("fresh"), deps.join("fresh/more")]);
    }

    #[test]
    fn a_reread_does_not_walk_a_new_subfolder_outside_the_roots() {
        let root = tree("flat-reread-outside");
        fs::create_dir_all(root.join("other/deep")).unwrap();
        fs::write(root.join("other/deep/z.txt"), "z").unwrap();
        let before = held(&[root.join("deps"), root.join("build")]);
        let read = reread(&[root.join("deps")], &[(root.clone(), before)], &never()).unwrap();
        assert!(names(&read.folders[0].1, &root).contains(&"other".to_string()), "the folder itself is read all the same");
        assert!(read.walked.found.is_empty() && read.walked.folders.is_empty());
    }

    #[test]
    fn a_reread_reads_only_once_a_folder_that_is_new_and_also_asked_for() {
        let root = tree("flat-reread-nested");
        let deps = root.join("deps");
        fs::create_dir(deps.join("fresh")).unwrap();
        fs::write(deps.join("fresh/x.txt"), "x").unwrap();
        let folders = [(deps.clone(), held(&[deps.join("inner")])), (deps.join("fresh"), HashSet::new())];
        let read = reread(std::slice::from_ref(&root), &folders, &never()).unwrap();
        assert_eq!(read.folders.len(), 1, "the walk of the new folder already read it");
        assert_eq!(names(&read.walked.found, &root), ["deps/fresh/x.txt"]);
        assert_eq!(read.walked.folders, [deps.join("fresh")]);
    }

    #[test]
    fn a_reread_tells_a_removed_subfolder_is_gone() {
        let root = tree("flat-reread-gone");
        let deps = root.join("deps");
        fs::remove_dir_all(deps.join("inner")).unwrap();
        let read = reread(std::slice::from_ref(&root), &[(deps.clone(), held(&[deps.join("inner")]))], &never()).unwrap();
        assert_eq!(read.gone, [deps.join("inner")]);
        assert_eq!(names(&read.folders[0].1, &root), ["deps/lib.d"]);
    }

    #[test]
    fn a_reread_tells_a_folder_it_cannot_read_is_gone() {
        let root = tree("flat-reread-unreadable");
        let nope = root.join("nope");
        let read = reread(std::slice::from_ref(&root), &[(nope.clone(), held(&[nope.join("x")]))], &never()).unwrap();
        assert!(read.folders.is_empty());
        assert_eq!(read.gone, [nope]);
    }

    #[cfg(unix)]
    #[test]
    fn a_reread_lists_a_new_link_to_a_folder_but_does_not_walk_it() {
        let root = tree("flat-reread-link");
        let deps = root.join("deps");
        std::os::unix::fs::symlink(root.join("build"), deps.join("link")).unwrap();
        fs::write(root.join("build/b.txt"), "b").unwrap();
        let read = reread(std::slice::from_ref(&root), &[(deps.clone(), held(&[deps.join("inner")]))], &never()).unwrap();
        assert!(read.folders[0].1.iter().any(|e| e.name == "link"), "listed");
        assert!(read.walked.found.is_empty() && read.walked.folders.is_empty(), "not entered");
    }

    #[test]
    fn a_reread_stops_when_cancelled() {
        let root = tree("flat-reread-cancel");
        let deps = root.join("deps");
        assert!(reread(&[root], &[(deps, HashSet::new())], &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn a_reread_updates_the_folders_a_walk_read() {
        let root = tree("flat-reread-update");
        let deps = root.join("deps");
        fs::remove_dir_all(deps.join("inner")).unwrap();
        fs::create_dir(deps.join("fresh")).unwrap();
        let read = reread(std::slice::from_ref(&root), &[(deps.clone(), held(&[deps.join("inner")]))], &never()).unwrap();
        let mut folders = vec![root.clone(), deps.clone(), deps.join("inner"), deps.join("inner/deeper"), root.join("build")];
        read.update_folders(&mut folders);
        assert_eq!(folders, [root.clone(), deps.clone(), root.join("build"), deps.join("fresh")]);
    }

    #[test]
    fn a_refresher_delivers_its_reread_for_the_view_it_was_started_for() {
        let root = tree("flat-reread-thread");
        let deps = root.join("deps");
        fs::write(deps.join("new.rs"), "4").unwrap();
        let (tx, rx) = mpsc::channel();
        let refresher = Refresher::start(vec![root.clone()], root.clone(), vec![(deps.clone(), held(&[deps.join("inner")]))], move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the reread never finished");
        let (roots, current, read) = refresher.poll().expect("finished");
        assert_eq!((roots, current), (vec![root.clone()], root.clone()));
        assert_eq!(names(&read.folders[0].1, &root), ["deps/inner", "deps/lib.d", "deps/new.rs"]);
    }
}
