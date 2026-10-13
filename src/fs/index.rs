//! The quick-jump index: every folder and file below the roots, kept in
//! memory and built by a background crawl, like File Pilot's (no system
//! search service, nothing written to disk).
//!
//! Names live in one string and each entry is a few numbers, so a disk with
//! millions of files stays small. The crawl goes breadth first, so shallow
//! folders (Downloads, Documents) are there within moments of starting.
//! Measured on a Linux laptop: the whole of `/` was 1.3 million entries,
//! about 66 MB, 23 s from a cold disk cache and 2 s from a warm one; home
//! and the disks alone (the scope since fase-9 T8) are 0.7 million, 1 s warm.

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
    /// A network share's crawl: gvfs shows a link to a folder as a folder,
    /// so links are followed there and could loop. It stops at
    /// [`SHARE_DEPTH`], and a folder holding a whole Linux system gives only
    /// its home, like this machine's.
    pub share: bool,
}

/// How deep a share is crawled: past any real folder, short of a loop.
pub const SHARE_DEPTH: usize = 16;

/// Folders that together mark the top of a Linux system.
const LINUX_ROOT: [&str; 3] = ["proc", "etc", "usr"];

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

/// What the crawl covers: local roots crawled together, network roots each
/// on its own thread (a share that hangs never stops the rest), and what
/// is left out.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub roots: Vec<PathBuf>,
    pub network: Vec<PathBuf>,
    pub rules: Rules,
}

/// Where the quick jump looks, like File Pilot: Linux, the home folder, the
/// side panel's other disks and the network shares; Windows, every drive and
/// network drive, without the system's and the installed programs' folders.
pub fn scope(windows: bool, home: Option<&Path>, disks: Vec<PathBuf>, network: Vec<PathBuf>) -> Scope {
    let mut skip_names: Vec<String> = SKIPPED_NAMES.map(String::from).to_vec();
    if windows {
        // Each user's program settings and caches.
        skip_names.push("AppData".into());
        let skip_paths = disks.iter().chain(&network).flat_map(|drive| WINDOWS_SYSTEM.map(|name| drive.join(name))).collect();
        return Scope { roots: disks, network, rules: Rules { skip_names, skip_paths, mounts: Vec::new(), share: false } };
    }
    // `/` and a disk holding home (a /home partition) would bring in the
    // system or other users; a disk inside home is crossed into from it.
    let others = disks.iter().filter(|d| home.is_none_or(|h| !h.starts_with(d) && !d.starts_with(h)) && d.parent().is_some());
    let roots = home.map(Path::to_path_buf).into_iter().chain(others.cloned()).collect();
    let rules = Rules { skip_names, skip_paths: network.clone(), mounts: disks, share: false };
    Scope { roots, network, rules }
}

/// Folders at the top of a Windows drive that hold the system or installed
/// programs, not the user's files.
const WINDOWS_SYSTEM: [&str; 8] =
    ["Windows", "Program Files", "Program Files (x86)", "ProgramData", "$Recycle.Bin", "System Volume Information", "Recovery", "PerfLogs"];

/// Whether the crawl may go into a folder on `dev` from a parent on
/// `parent_dev`: always on the same device, else only a listed mount.
pub fn may_enter(path: &Path, dev: Option<u64>, parent_dev: Option<u64>, same_filesystem: impl FnOnce() -> bool, rules: &Rules) -> bool {
    dev == parent_dev || rules.mounts.iter().any(|m| m == path) || same_filesystem()
}

/// Whether two folders are on the same kind of filesystem: a btrfs
/// subvolume has its own device number but is part of the same disk.
#[cfg(target_os = "linux")]
fn same_filesystem(a: &Path, b: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let kind = |path: &Path| {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `stat` is a writable statfs.
        (unsafe { libc::statfs(path.as_ptr(), &mut stat) } == 0).then_some(stat.f_type)
    };
    kind(a).is_some_and(|k| Some(k) == kind(b))
}

#[cfg(not(target_os = "linux"))]
fn same_filesystem(_a: &Path, _b: &Path) -> bool {
    false
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
            if may_enter(&child, dev, parent_dev, || same_filesystem(path, &child), rules) {
                next = Some((child, dev));
            }
        } else if kind.is_symlink() && std::fs::metadata(&child).is_ok_and(|m| m.is_dir()) {
            // Found as the folder it points to, never followed: no loops.
            is_dir = true;
        }
        found.push((name, is_dir, next));
    }
    let is_linux_root = LINUX_ROOT.iter().all(|top| found.iter().any(|(name, is_dir, _)| *is_dir && name == top));
    if rules.share && is_linux_root {
        found.retain(|(name, ..)| name == "home");
    }
    found
}

/// Crawls `roots` into `index`, breadth first, until done or `cancel` is set.
/// Unreadable folders are skipped.
pub fn crawl(roots: &[PathBuf], rules: &Rules, index: &RwLock<Index>, cancel: &AtomicBool) {
    let mut queue = VecDeque::new();
    for root in roots {
        let Ok(meta) = std::fs::metadata(root) else { continue };
        if !meta.is_dir() {
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
            if rules.share && index.depth(id) >= SHARE_DEPTH {
                continue;
            }
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
    /// Starts crawling what `scope` returns; both run on the crawl's thread,
    /// so reading the mounts never blocks the caller. `on_done` runs there
    /// too when the crawl ends.
    pub fn start(scope: impl FnOnce() -> Scope + Send + 'static, on_done: impl FnOnce() + Send + 'static) -> Self {
        let index = Arc::new(RwLock::new(Index::default()));
        let done = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (shared, finished, stop) = (index.clone(), done.clone(), cancel.clone());
        thread::spawn(move || {
            lower_priority();
            let Scope { roots, network, rules } = scope();
            // Each share on its own thread, outside `done`: one that hangs only
            // loses its own entries and never holds back the rest or a re-crawl.
            for share in network {
                let (shared, stop) = (shared.clone(), stop.clone());
                let rules = Rules { share: true, ..rules.clone() };
                thread::spawn(move || {
                    lower_priority();
                    crawl(&[share], &rules, &shared, &stop);
                });
            }
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
    /// Made again on every run, so nothing an older run left is crawled.
    fn fixture(name: &str) -> PathBuf {
        let root = crate::fs::home_dir()
            .expect("HOME must be set")
            .join("pruebas-explorador")
            .join("test-fixtures")
            .join(name);
        // remove_dir_all never follows the fixture's link out of it.
        let _ = std::fs::remove_dir_all(&root);
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
        std::os::unix::fs::symlink(root.join("docs"), root.join("link")).unwrap();
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
    fn on_linux_the_scope_is_home_the_other_disks_and_the_network_shares() {
        let home = PathBuf::from("/home/u");
        let disks = ["/", "/home", "/media/usb", "/run/media/u/stick", "/home/u/data"].map(PathBuf::from).to_vec();
        let network = vec![PathBuf::from("/mnt/nas")];
        let scope = scope(false, Some(&home), disks, network.clone());
        assert_eq!(scope.roots, [home, PathBuf::from("/media/usb"), PathBuf::from("/run/media/u/stick")], "no system folders");
        assert_eq!(scope.network, network);
        assert_eq!(scope.rules.skip_names, SKIPPED_NAMES);
        assert_eq!(scope.rules.skip_paths, network, "the local crawl never touches a share");
        assert!(scope.rules.mounts.contains(&PathBuf::from("/home/u/data")), "a disk inside home is crossed into");
    }

    #[test]
    fn on_windows_every_drive_is_searched_without_system_and_installation_folders() {
        let (c, d, z) = (PathBuf::from("C:\\"), PathBuf::from("D:\\"), PathBuf::from("Z:\\"));
        let scope = scope(true, Some(Path::new("C:\\Users\\u")), vec![c.clone(), d.clone()], vec![z.clone()]);
        assert_eq!(scope.roots, [c.clone(), d.clone()]);
        assert_eq!(scope.network, std::slice::from_ref(&z));
        let system = ["Windows", "Program Files", "Program Files (x86)", "ProgramData", "$Recycle.Bin", "System Volume Information", "Recovery", "PerfLogs"];
        for drive in [&c, &d, &z] {
            for name in system {
                assert!(scope.rules.skip_paths.contains(&drive.join(name)), "{name}");
            }
        }
        assert!(scope.rules.skip_names.iter().any(|n| n == "AppData"));
        assert!(scope.rules.skip_names.iter().any(|n| n == "node_modules"));
    }

    #[test]
    fn another_device_is_entered_only_at_a_listed_mount_or_on_the_same_filesystem() {
        let rules = Rules { mounts: vec![PathBuf::from("/media/usb")], ..Rules::default() };
        assert!(may_enter(Path::new("/home/x"), Some(1), Some(1), || false, &rules));
        assert!(may_enter(Path::new("/media/usb"), Some(2), Some(1), || false, &rules));
        assert!(!may_enter(Path::new("/tmp"), Some(3), Some(1), || false, &rules), "tmpfs, network shares, fuse");
        assert!(may_enter(Path::new("/home/x/sub"), Some(4), Some(1), || true, &rules), "the same filesystem: a btrfs subvolume");
        assert!(may_enter(Path::new("C:\\Users"), None, None, || false, &rules), "no device ids: no check");
    }

    /// The real scope of this machine, timed: `cargo test --release crawl_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore = "crawls the whole machine"]
    fn crawl_this_machine() {
        let disks = crate::places::disks().into_iter().map(|m| m.path).collect();
        let scope = scope(cfg!(windows), crate::fs::home_dir().as_deref(), disks, crate::places::network_mounts());
        let start = std::time::Instant::now();
        let index = crawled(&scope.roots, &scope.rules);
        eprintln!("{} entries, {} bytes of names, {:?}", index.len(), index.names.len(), start.elapsed());
    }

    #[test]
    fn network_roots_are_crawled_on_their_own_into_the_same_index() {
        let root = tree("index-network");
        let (tx, rx) = std::sync::mpsc::channel();
        let scope = Scope { roots: vec![root.join("docs")], network: vec![root.join("Downloads")], rules: rules() };
        std::fs::write(root.join("Downloads/shared.txt"), "x").unwrap();
        let crawler = Crawler::start(move || scope, move || tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the local crawl ends");
        let found = |name: &str| {
            let index = crawler.index().read().unwrap();
            (0..index.len() as u32).any(|id| index.name(id) == name)
        };
        assert!(found("deep.txt"));
        let start = std::time::Instant::now();
        while !found("shared.txt") {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "the share is crawled too");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn the_background_crawl_fills_the_index_and_calls_back_when_done() {
        let root = tree("index-background");
        let (tx, rx) = std::sync::mpsc::channel();
        let scope = Scope { roots: vec![root.clone()], rules: rules(), ..Scope::default() };
        let crawler = Crawler::start(move || scope, move || tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the crawl ends");
        assert!(crawler.is_done());
        assert!(listed(&crawler.index().read().unwrap(), &root).contains(&"docs/a/deep.txt".to_string()));
    }

    fn share_rules() -> Rules {
        Rules { share: true, ..rules() }
    }

    #[test]
    fn on_a_share_holding_a_whole_linux_system_only_its_home_is_crawled() {
        let root = fixture("index-share-system");
        for folder in ["proc/1/root", "etc/ssl", "usr/lib", "home/u/docs"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(root.join("etc/ssl/cert.pem"), "x").unwrap();
        std::fs::write(root.join("home/u/docs/notes.txt"), "x").unwrap();
        let local = listed(&crawled(std::slice::from_ref(&root), &rules()), &root);
        assert!(local.contains(&"etc/ssl/cert.pem".to_string()), "a local root is crawled whole: {local:?}");
        let shared = listed(&crawled(std::slice::from_ref(&root), &share_rules()), &root);
        assert_eq!(shared, ["", "home/", "home/u/", "home/u/docs/", "home/u/docs/notes.txt"]);
    }

    #[test]
    fn a_share_is_crawled_only_so_deep_so_a_loop_of_links_ends() {
        let root = fixture("index-share-deep");
        let deepest = (0..SHARE_DEPTH + 3).fold(root.clone(), |path, n| path.join(format!("d{n}")));
        std::fs::create_dir_all(&deepest).unwrap();
        let depth = |index: &Index| (0..index.len() as u32).map(|id| index.depth(id)).max().unwrap_or(0);
        assert_eq!(depth(&crawled(std::slice::from_ref(&root), &rules())), SHARE_DEPTH + 3, "a local crawl has no limit");
        assert_eq!(depth(&crawled(std::slice::from_ref(&root), &share_rules())), SHARE_DEPTH);
    }

    #[test]
    fn the_scope_is_read_on_the_crawl_thread_not_the_caller() {
        let root = tree("index-scope-thread");
        let (tx, rx) = std::sync::mpsc::channel();
        let (read_on, read_rx) = std::sync::mpsc::channel();
        let scope = move || {
            read_on.send(thread::current().id()).unwrap();
            Scope { roots: vec![root.clone()], rules: rules(), ..Scope::default() }
        };
        let crawler = Crawler::start(scope, move || tx.send(()).unwrap());
        let reader = read_rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the scope is read");
        assert_ne!(reader, thread::current().id(), "mounts are never scanned on the caller's (UI) thread");
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the crawl ends");
        assert!(crawler.index().read().unwrap().len() > 0, "the scope it read is crawled");
    }
}
