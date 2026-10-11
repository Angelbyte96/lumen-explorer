//! The quick-jump index: every folder and file below the roots, kept in
//! memory and built by a background crawl, like File Pilot's (no system
//! search service, nothing written to disk).
//!
//! Names live in one string and each entry is a few numbers, so a disk with
//! millions of files stays small. The crawl goes breadth first, so shallow
//! folders (Downloads, Documents) are there within moments of starting.
//! Measured on a Linux laptop: 1.3 million entries, about 66 MB, crawled in
//! 23 s from a cold disk cache and 2 s from a warm one.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

use super::entry::is_hidden_name;

/// Folders never crawled: nothing in them is found, nor the folder itself.
pub const SKIPPED_NAMES: [&str; 4] = ["node_modules", ".git", "target", ".cache"];

/// What the crawl leaves out.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// Folder names skipped wherever they are.
    pub skip_names: Vec<String>,
    /// Folders skipped by full path (Linux: /proc, /sys, ...).
    pub skip_paths: Vec<PathBuf>,
    /// Mount points the crawl may cross into; any other folder on another
    /// device than its parent (tmpfs, network shares, fuse) is skipped.
    pub mounts: Vec<PathBuf>,
}

/// No parent: a root, whose name is its whole path.
const ROOT: u32 = u32::MAX;
const DIR: u8 = 1;
const HIDDEN: u8 = 2;

#[derive(Debug, Clone, Copy)]
struct Node {
    start: u32,
    len: u32,
    parent: u32,
    /// Folders up to the root; 16 bytes a node with or without it.
    depth: u16,
    flags: u8,
}

/// The entries found so far.
#[derive(Debug, Default)]
pub struct Index {
    names: String,
    nodes: Vec<Node>,
}

impl Index {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn name(&self, id: u32) -> &str {
        let node = self.nodes[id as usize];
        &self.names[node.start as usize..(node.start + node.len) as usize]
    }

    pub fn is_dir(&self, id: u32) -> bool {
        self.nodes[id as usize].flags & DIR != 0
    }

    pub fn is_hidden(&self, id: u32) -> bool {
        self.nodes[id as usize].flags & HIDDEN != 0
    }

    pub fn parent(&self, id: u32) -> Option<u32> {
        Some(self.nodes[id as usize].parent).filter(|&p| p != ROOT)
    }

    /// The entry's full path.
    pub fn path(&self, id: u32) -> PathBuf {
        let mut chain = vec![id];
        while let Some(parent) = self.parent(*chain.last().expect("never empty")) {
            chain.push(parent);
        }
        chain.iter().rev().map(|&id| self.name(id)).collect()
    }

    /// How many folders up to its root.
    pub fn depth(&self, id: u32) -> usize {
        self.nodes[id as usize].depth as usize
    }

    /// Adds an entry below `parent` (a root without one, named by its whole
    /// path). Inside a hidden folder an entry counts as hidden too.
    pub fn add(&mut self, parent: Option<u32>, name: &str, is_dir: bool) -> u32 {
        let hidden = parent.is_some_and(|p| self.is_hidden(p) || is_hidden_name(name));
        let flags = if is_dir { DIR } else { 0 } | if hidden { HIDDEN } else { 0 };
        let depth = parent.map_or(0, |p| self.nodes[p as usize].depth.saturating_add(1));
        let id = self.nodes.len() as u32;
        self.nodes.push(Node { start: self.names.len() as u32, len: name.len() as u32, parent: parent.unwrap_or(ROOT), depth, flags });
        self.names.push_str(name);
        id
    }
}

/// What the crawl covers by default, given the disks of the side panel's
/// Storage section. Linux: everything from `/` and those disks, without the
/// kernel's and snap's pseudo folders. Windows: every drive.
pub fn scope(disks: Vec<PathBuf>) -> (Vec<PathBuf>, Rules) {
    let skip_names = SKIPPED_NAMES.map(String::from).to_vec();
    if cfg!(windows) {
        return (disks, Rules { skip_names, ..Rules::default() });
    }
    let skip_paths = ["/proc", "/sys", "/dev", "/run", "/snap"].map(PathBuf::from).to_vec();
    (vec![PathBuf::from("/")], Rules { skip_names, skip_paths, mounts: disks })
}

/// Whether the crawl may go into a folder on `dev` from a parent on
/// `parent_dev`: always on the same device, else only a listed mount.
pub fn may_enter(path: &Path, dev: Option<u64>, parent_dev: Option<u64>, rules: &Rules) -> bool {
    dev == parent_dev || rules.mounts.iter().any(|m| m == path)
}

#[cfg(unix)]
fn device(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

/// Windows crawls drive by drive: no mounts inside them to check.
#[cfg(not(unix))]
fn device(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

/// A folder's entries, read before taking the index's lock: name, whether a
/// folder and,
/// for a folder to crawl next, its path and device.
type Found = Vec<(String, bool, Option<(PathBuf, Option<u64>)>)>;

fn read_folder(path: &Path, parent_dev: Option<u64>, rules: &Rules) -> Found {
    let Ok(items) = std::fs::read_dir(path) else { return Vec::new() };
    let mut found = Vec::new();
    for item in items.flatten() {
        let Ok(kind) = item.file_type() else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let mut is_dir = false;
        let mut next = None;
        let child = item.path();
        if kind.is_dir() {
            if rules.skip_names.contains(&name) || rules.skip_paths.contains(&child) {
                continue;
            }
            is_dir = true;
            // Another device's folder (tmpfs, a network share) is found but
            // not crawled, unless it is one of the disks.
            let dev = item.metadata().ok().and_then(|m| device(&m));
            if may_enter(&child, dev, parent_dev, rules) {
                next = Some((child, dev));
            }
        } else if kind.is_symlink() && std::fs::metadata(&child).is_ok_and(|m| m.is_dir()) {
            // Found as the folder it points to, never followed: no loops.
            is_dir = true;
        }
        found.push((name, is_dir, next));
    }
    found
}

/// Crawls `roots` into `index`, breadth first, until done or `cancel` is set.
/// Unreadable folders are skipped.
pub fn crawl(roots: &[PathBuf], rules: &Rules, index: &RwLock<Index>, cancel: &AtomicBool) {
    let mut queue = VecDeque::new();
    for root in roots {
        let Ok(meta) = std::fs::metadata(root) else { continue };
        if !meta.is_dir() || rules.skip_paths.contains(root) {
            continue;
        }
        let id = index.write().expect("index lock").add(None, &root.to_string_lossy(), true);
        queue.push_back((id, root.clone(), device(&meta)));
    }
    while let Some((parent, path, dev)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let found = read_folder(&path, dev, rules);
        let mut index = index.write().expect("index lock");
        for (name, is_dir, next) in found {
            let id = index.add(Some(parent), &name, is_dir);
            if let Some((path, dev)) = next {
                queue.push_back((id, path, dev));
            }
        }
    }
}

/// Lowers the calling thread's CPU and disk priority, so the crawl never
/// slows the window or other programs down.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain syscalls on the calling thread's id; failures are harmless.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 19);
        // ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_CLASS_IDLE)
        libc::syscall(libc::SYS_ioprio_set, 1, tid, 3 << 13);
    }
    #[cfg(windows)]
    // SAFETY: the pseudo handle of the calling thread; failure is harmless.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// A crawl on its own low-priority thread; dropping it stops the crawl.
pub struct Crawler {
    index: Arc<RwLock<Index>>,
    done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Crawler {
    /// Starts crawling; `on_done` runs on the crawl's thread when it ends.
    pub fn start(roots: Vec<PathBuf>, rules: Rules, on_done: impl FnOnce() + Send + 'static) -> Self {
        let index = Arc::new(RwLock::new(Index::default()));
        let done = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (shared, finished, stop) = (index.clone(), done.clone(), cancel.clone());
        thread::spawn(move || {
            lower_priority();
            crawl(&roots, &rules, &shared, &stop);
            finished.store(true, Ordering::Release);
            on_done();
        });
        Self { index, done, cancel }
    }

    /// The entries found so far; readable while the crawl goes on.
    pub fn index(&self) -> &Arc<RwLock<Index>> {
        &self.index
    }

    /// A crawler over `index` with no thread behind it, finished or still
    /// going as `done` says: tests need a crawl whose state they control.
    #[cfg(test)]
    pub fn fixed(index: Index, done: bool) -> Self {
        Self { index: Arc::new(RwLock::new(index)), done: Arc::new(AtomicBool::new(done)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    /// Ends a [`Crawler::fixed`] crawl.
    #[cfg(test)]
    pub fn finish(&self) {
        self.done.store(true, Ordering::Release);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

impl Drop for Crawler {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test fixtures live only under ~/pruebas-explorador (project safety rule).
    fn fixture(name: &str) -> PathBuf {
        let root = crate::fs::home_dir()
            .expect("HOME must be set")
            .join("pruebas-explorador")
            .join("test-fixtures")
            .join(name);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// docs/a/deep.txt, Downloads/, .hidden, a folder of each skipped name
    /// with a file inside, and a link to a folder.
    fn tree(name: &str) -> PathBuf {
        let root = fixture(name);
        std::fs::create_dir_all(root.join("docs/a")).unwrap();
        std::fs::create_dir_all(root.join("Downloads")).unwrap();
        std::fs::write(root.join("docs/a/deep.txt"), "x").unwrap();
        std::fs::write(root.join("readme.md"), "x").unwrap();
        std::fs::write(root.join(".hidden"), "x").unwrap();
        for skipped in SKIPPED_NAMES {
            std::fs::create_dir_all(root.join("docs").join(skipped)).unwrap();
            std::fs::write(root.join("docs").join(skipped).join("inside.txt"), "x").unwrap();
        }
        #[cfg(unix)]
        if !root.join("link").exists() {
            std::os::unix::fs::symlink(root.join("docs"), root.join("link")).unwrap();
        }
        root
    }

    fn rules() -> Rules {
        Rules { skip_names: SKIPPED_NAMES.map(String::from).to_vec(), ..Rules::default() }
    }

    fn crawled(roots: &[PathBuf], rules: &Rules) -> Index {
        let index = RwLock::new(Index::default());
        crawl(roots, rules, &index, &AtomicBool::new(false));
        index.into_inner().unwrap()
    }

    /// Every entry's path below `root`, sorted, folders marked with a slash.
    fn listed(index: &Index, root: &Path) -> Vec<String> {
        let mut paths: Vec<String> = (0..index.len() as u32)
            .filter_map(|id| {
                let path = index.path(id);
                let below = path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
                let slash = if index.is_dir(id) && !below.is_empty() { "/" } else { "" };
                Some(format!("{below}{slash}"))
            })
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn the_crawl_finds_every_folder_and_file_but_skipped_folders_and_inside_links() {
        let root = tree("index-crawl");
        let index = crawled(std::slice::from_ref(&root), &rules());
        let mut expected = vec!["", ".hidden", "Downloads/", "docs/", "docs/a/", "docs/a/deep.txt", "readme.md"];
        // A link is found, as the folder it points to, but not followed.
        if cfg!(unix) {
            expected.push("link/");
        }
        expected.sort();
        assert_eq!(listed(&index, &root), expected);
    }

    #[test]
    fn a_root_is_its_whole_path_and_entries_know_their_depth_and_hidden_flag() {
        let root = tree("index-paths");
        let index = crawled(std::slice::from_ref(&root), &rules());
        let find = |name: &str| (0..index.len() as u32).find(|&id| index.name(id) == name).unwrap();
        assert_eq!(index.path(0), root);
        assert_eq!(index.parent(0), None);
        assert_eq!(index.path(find("deep.txt")), root.join("docs/a/deep.txt"));
        assert_eq!(index.depth(find("deep.txt")), 3);
        assert!(index.is_hidden(find(".hidden")));
        assert!(!index.is_hidden(find("readme.md")));
        assert!(!index.is_dir(find("readme.md")));
    }

    #[test]
    fn shallow_entries_come_before_deep_ones() {
        let root = tree("index-breadth");
        let index = crawled(std::slice::from_ref(&root), &rules());
        let find = |name: &str| (0..index.len() as u32).find(|&id| index.name(id) == name).unwrap();
        assert!(find("Downloads") < find("a"));
        assert!(find("readme.md") < find("deep.txt"));
    }

    #[test]
    fn a_skipped_path_is_left_out_with_all_it_holds() {
        let root = tree("index-skip-path");
        let rules = Rules { skip_paths: vec![root.join("docs")], ..rules() };
        let index = crawled(std::slice::from_ref(&root), &rules);
        let mut expected = vec!["", ".hidden", "Downloads/", "readme.md"];
        if cfg!(unix) {
            expected.push("link/");
        }
        expected.sort();
        assert_eq!(listed(&index, &root), expected);
    }

    #[test]
    fn a_missing_root_is_ignored() {
        let root = tree("index-missing");
        let index = crawled(&[root.join("nope"), root.join("Downloads")], &rules());
        assert_eq!(listed(&index, &root), ["Downloads/"]);
    }

    #[test]
    fn the_scope_is_every_drive_on_windows_and_the_root_with_the_disks_elsewhere() {
        let disks = vec![PathBuf::from("/media/usb")];
        let (roots, rules) = scope(disks.clone());
        assert_eq!(rules.skip_names, SKIPPED_NAMES);
        if cfg!(windows) {
            assert_eq!(roots, disks);
            assert!(rules.skip_paths.is_empty());
        } else {
            assert_eq!(roots, [PathBuf::from("/")]);
            assert_eq!(rules.mounts, disks);
            for pseudo in ["/proc", "/sys", "/dev", "/run", "/snap"] {
                assert!(rules.skip_paths.contains(&PathBuf::from(pseudo)), "{pseudo}");
            }
        }
    }


    #[test]
    fn another_device_is_entered_only_at_a_listed_mount() {
        let rules = Rules { mounts: vec![PathBuf::from("/media/usb")], ..Rules::default() };
        assert!(may_enter(Path::new("/home/x"), Some(1), Some(1), &rules));
        assert!(may_enter(Path::new("/media/usb"), Some(2), Some(1), &rules));
        assert!(!may_enter(Path::new("/tmp"), Some(3), Some(1), &rules), "tmpfs, network shares, fuse");
        assert!(may_enter(Path::new("C:\\Users"), None, None, &rules), "no device ids: no check");
    }

    /// The real scope of this machine, timed: `cargo test --release crawl_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore = "crawls the whole machine"]
    fn crawl_this_machine() {
        let disks = crate::places::disks().into_iter().map(|m| m.path).collect();
        let (roots, rules) = scope(disks);
        let start = std::time::Instant::now();
        let index = crawled(&roots, &rules);
        eprintln!("{} entries, {} bytes of names, {:?}", index.len(), index.names.len(), start.elapsed());
    }

    #[test]
    fn the_background_crawl_fills_the_index_and_calls_back_when_done() {
        let root = tree("index-background");
        let (tx, rx) = std::sync::mpsc::channel();
        let crawler = Crawler::start(vec![root.clone()], rules(), move || tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the crawl ends");
        assert!(crawler.is_done());
        assert!(listed(&crawler.index().read().unwrap(), &root).contains(&"docs/a/deep.txt".to_string()));
    }
}
