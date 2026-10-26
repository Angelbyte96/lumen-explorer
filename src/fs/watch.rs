//! Watches the open folder so the list can reload itself when something changes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;

use notify::event::{AccessKind, AccessMode};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode};

/// The most folders a flattened view watches: each costs the system one
/// watch (inotify allows 65,536 per user, for every program).
pub const WATCH_LIMIT: usize = 5_000;

/// Watches one folder at a time, and, when the view lists what its
/// subfolders hold (Ctrl+E), each of the folders listed. The system is told
/// on a thread of its own: thousands of folders take a while to watch, and
/// as long again to unwatch.
pub struct Watcher {
    orders: Option<Sender<Order>>,
    watched: Option<PathBuf>,
    /// The folders watched besides `watched`, as asked in [`Watcher::watch`].
    folders: Vec<PathBuf>,
    /// Counts the orders, so the watch thread can tell the newest.
    generation: u64,
    /// The same, for notify's thread: unwatching is not immediate, and news
    /// from a folder left behind must not count.
    scope: Arc<Mutex<Scope>>,
    /// Set by notify's thread, cleared by [`Watcher::take_changed`].
    changed: Arc<AtomicBool>,
    #[cfg(test)]
    probe: Arc<Probe>,
}

/// What the watch thread is asked to watch: `path` and `folders`, nothing
/// else; `path` `None` watches nothing.
struct Order {
    generation: u64,
    path: Option<PathBuf>,
    plan: Plan,
}

/// How an order's folders are handed to the system.
#[derive(Clone, Default, Debug, PartialEq)]
struct Plan {
    /// Watched each on its own.
    alone: Vec<PathBuf>,
    /// Watched all the way down, the order's folder maybe among them.
    trees: Vec<PathBuf>,
}

impl Plan {
    /// inotify (Linux) has no watch all the way down: each folder costs one
    /// watch. On Windows a handle on a folder keeps every folder above it
    /// from being renamed or trashed, while one watch all the way down costs
    /// a single handle: there the topmost folders are watched that way.
    /// `folders` holds `path` when the whole tree is listed.
    fn new(path: &Path, folders: &[PathBuf], per_folder: bool) -> Self {
        if per_folder {
            return Self { alone: folders.iter().filter(|f| *f != path).cloned().collect(), trees: Vec::new() };
        }
        if folders.iter().any(|f| f == path) {
            return Self { alone: Vec::new(), trees: vec![path.to_path_buf()] };
        }
        let all: HashSet<&Path> = folders.iter().map(PathBuf::as_path).collect();
        let tops = folders.iter().filter(|f| !f.parent().is_some_and(|up| all.contains(up))).cloned().collect();
        Self { alone: Vec::new(), trees: tops }
    }
}

/// Whose news count, for the newest order.
#[derive(Default)]
struct Scope {
    generation: u64,
    path: Option<PathBuf>,
    folders: HashSet<PathBuf>,
    trees: Vec<PathBuf>,
    /// Whether that order gave up on its folders, until taken.
    refused: bool,
}

impl Scope {
    fn new(generation: u64, path: Option<&Path>, plan: &Plan) -> Self {
        let (folders, trees) = (plan.alone.iter().cloned().collect(), plan.trees.clone());
        Self { generation, path: path.map(Path::to_path_buf), folders, trees, refused: false }
    }

    /// Whether a change at `path` is in a folder watched (or one of them).
    fn covers(&self, path: &Path) -> bool {
        let Some(watched) = self.path.as_deref() else { return false };
        let ours = |folder: &Path| folder == watched || self.folders.contains(folder);
        ours(path) || path.parent().is_some_and(ours) || self.trees.iter().any(|tree| path.starts_with(tree))
    }
}

/// What the watch thread has done, for the tests to wait on or hold it.
#[derive(Default)]
struct Probe {
    applied: Mutex<u64>,
    held: Mutex<bool>,
    moved: Condvar,
}

/// Opening or reading (Lumen's own listing does both) is not a change;
/// finishing a write is.
fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Whether `event` tells of a change in a folder watched now: news that
/// arrive late from a folder left behind, or with nothing watched, do not
/// count. An error counts while a folder is watched.
fn concerns(event: &notify::Result<notify::Event>, scope: &Scope) -> bool {
    if scope.path.is_none() {
        return false;
    }
    match event {
        Err(_) => true,
        Ok(e) => is_change(&e.kind) && (e.paths.is_empty() || e.paths.iter().any(|path| scope.covers(path))),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Watcher {
    /// `wake` runs on notify's thread on the first change after each
    /// [`Watcher::take_changed`], so a burst wakes the UI once, and on the
    /// watch thread when it gives up on an order's folders.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start::<RecommendedWatcher>(wake)
    }

    /// [`Watcher::new`] on the backend `W`.
    fn start<W: notify::Watcher + Send + 'static>(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let wake = Arc::new(wake);
        let changed = Arc::new(AtomicBool::new(false));
        let scope: Arc<Mutex<Scope>> = Arc::default();
        let probe: Arc<Probe> = Arc::default();
        let (flag, now, woken) = (changed.clone(), scope.clone(), wake.clone());
        let handler = move |event: notify::Result<notify::Event>| {
            // An error (e.g. the kernel queue overflowed) may hide changes: reload.
            let ours = concerns(&event, &lock(&now));
            if ours && !flag.swap(true, Ordering::SeqCst) {
                woken();
            }
        };
        // Like the flatten walk, a link is not entered.
        let orders = W::new(handler, Config::default().with_follow_symlinks(false)).ok().map(|inner| {
            let (tx, rx) = mpsc::channel();
            let (scope, probe) = (scope.clone(), probe.clone());
            thread::spawn(move || run(rx, inner, &scope, &probe, &*wake));
            tx
        });
        Self {
            orders,
            watched: None,
            folders: Vec::new(),
            generation: 0,
            scope,
            changed,
            #[cfg(test)]
            probe,
        }
    }

    /// Starts watching `path` instead of the previous folder, and `folders`
    /// (what a flattened view walked: `path` too when it walked the whole
    /// tree), as [`Plan::new`] says. A folder
    /// that cannot be watched (gone, no permission) just does not refresh.
    /// More than [`WATCH_LIMIT`] folders, or the system's watches running
    /// out, and nothing is watched: [`Watcher::take_refused`] says so.
    pub fn watch(&mut self, path: &Path, folders: &[PathBuf]) {
        if self.watched.as_deref() == Some(path) && folders.is_empty() && self.folders.is_empty() {
            return;
        }
        // A marker (This PC, the Recycle Bin) is no folder, and as a relative path it could name one.
        if crate::places::is_marker(path) {
            self.forget();
            // Nothing to refresh there: news from the folder left behind are dropped.
            self.changed.store(false, Ordering::SeqCst);
            return;
        }
        if self.orders.is_none() {
            return;
        }
        if self.watched.as_deref() != Some(path) {
            // News from the old folder no longer matter; the new one was just listed.
            self.changed.store(false, Ordering::SeqCst);
        }
        if folders.iter().filter(|f| *f != path).count() > WATCH_LIMIT {
            self.forget();
            // Its reload would walk them all again.
            self.changed.store(false, Ordering::SeqCst);
            lock(&self.scope).refused = true;
            return;
        }
        self.order(Some(path), folders);
        self.watched = Some(path.to_path_buf());
        self.folders = folders.to_vec();
    }

    /// Watches `path` alone, unless it is the folder watched already: then
    /// its folders stay watched too.
    pub fn watch_folder(&mut self, path: &Path) {
        if self.watched.as_deref() != Some(path) {
            self.watch(path, &[]);
        }
    }

    /// Tells notify's thread whose news count now, then the watch thread what to watch.
    fn order(&mut self, path: Option<&Path>, folders: &[PathBuf]) {
        self.generation += 1;
        let plan = path.map(|path| Plan::new(path, folders, !cfg!(windows))).unwrap_or_default();
        *lock(&self.scope) = Scope::new(self.generation, path, &plan);
        if let Some(orders) = &self.orders {
            let _ = orders.send(Order { generation: self.generation, path: path.map(Path::to_path_buf), plan });
        }
    }

    /// Stops watching, e.g. because the folder was deleted; the next
    /// [`Watcher::watch`] starts afresh even for the same path.
    pub fn forget(&mut self) {
        self.order(None, &[]);
        self.watched = None;
        self.folders.clear();
    }

    /// Whether the last [`Watcher::watch`] gave up on its folders, once.
    pub fn take_refused(&self) -> bool {
        std::mem::take(&mut lock(&self.scope).refused)
    }

    /// Whether something changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }

    /// Waits until the watch thread has carried out every order so far.
    #[cfg(test)]
    pub(crate) fn settle(&self) {
        let applied = lock(&self.probe.applied);
        let (applied, timeout) = self
            .probe
            .moved
            .wait_timeout_while(applied, Duration::from_secs(10), |applied| *applied < self.generation)
            .unwrap_or_else(|e| e.into_inner());
        drop(applied);
        assert!(!timeout.timed_out(), "the watch thread never got to the last order");
    }

    /// Keeps the watch thread from carrying out orders while the result lives.
    #[cfg(test)]
    fn hold(&self) -> Hold {
        *lock(&self.probe.held) = true;
        Hold(self.probe.clone())
    }
}

#[cfg(test)]
struct Hold(Arc<Probe>);

#[cfg(test)]
impl Drop for Hold {
    fn drop(&mut self) {
        *lock(&self.0.held) = false;
        self.0.moved.notify_all();
    }
}

/// Watches `path` alone and notes it in `registered`. Watched again even
/// when already: a folder deleted and made anew under the same name lost
/// its watch. Unwatched first: on Windows notify would keep the old handle
/// open, reading, for good.
fn rewatch(inner: &mut impl notify::Watcher, registered: &mut HashSet<PathBuf>, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
    if registered.remove(path) {
        let _ = inner.unwatch(path);
    }
    inner.watch(path, mode)?;
    registered.insert(path.to_path_buf());
    Ok(())
}

/// The watch thread: carries out the newest order, watching and unwatching
/// only what changed since the last one, until the [`Watcher`] is dropped.
fn run(orders: Receiver<Order>, mut inner: impl notify::Watcher, scope: &Mutex<Scope>, probe: &Probe, wake: &dyn Fn()) {
    let mut registered: HashSet<PathBuf> = HashSet::new();
    while let Ok(mut order) = orders.recv() {
        while let Ok(newer) = orders.try_recv() {
            order = newer;
        }
        drop(probe.moved.wait_while(lock(&probe.held), |held| *held).unwrap_or_else(|e| e.into_inner()));
        let plan = &order.plan;
        let wanted: HashSet<&PathBuf> = order.path.iter().chain(&plan.alone).chain(&plan.trees).collect();
        registered.retain(|old| {
            let keep = wanted.contains(old);
            if !keep {
                let _ = inner.unwatch(old);
            }
            keep
        });
        let mode = |folder: &PathBuf| if plan.trees.contains(folder) { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        if let Some(path) = &order.path
            && rewatch(&mut inner, &mut registered, path, mode(path)).is_ok()
        {
            // One gone or not readable just does not refresh; only the
            // system's watches running out gives up on them all.
            let others = plan.alone.iter().chain(&plan.trees).filter(|folder| *folder != path);
            let refused = others.into_iter().any(|folder| match rewatch(&mut inner, &mut registered, folder, mode(folder)) {
                Ok(()) => false,
                Err(e) => matches!(e.kind, notify::ErrorKind::MaxFilesWatch),
            });
            if refused {
                for old in registered.drain() {
                    let _ = inner.unwatch(&old);
                }
                let mut scope = lock(scope);
                if scope.generation == order.generation {
                    *scope = Scope { generation: order.generation, refused: true, ..Scope::default() };
                    drop(scope);
                    wake();
                }
            }
        } else {
            // The folder is gone (or this is `forget`): nothing of it stays watched.
            for old in registered.drain() {
                let _ = inner.unwatch(&old);
            }
        }
        *lock(&probe.applied) = order.generation;
        probe.moved.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    fn created(path: &str) -> notify::Result<notify::Event> {
        Ok(notify::Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(PathBuf::from(path)))
    }

    #[test]
    fn only_news_from_the_folder_watched_now_count() {
        let plan = Plan { alone: vec![PathBuf::from("/home/u/b/sub")], trees: vec![PathBuf::from("/home/u/b/deep")] };
        let now = &Scope::new(1, Some(Path::new("/home/u/b")), &plan);
        let none = &Scope::default();
        assert!(concerns(&created("/home/u/b/x"), now));
        assert!(concerns(&created("/home/u/b"), now), "the folder itself");
        assert!(concerns(&created("/home/u/b/sub/x"), now), "a folder watched besides it");
        assert!(!concerns(&created("/home/u/b/other/x"), now), "a subfolder not watched");
        assert!(concerns(&created("/home/u/b/deep/er/x"), now), "inside a folder watched all the way down");
        assert!(!concerns(&created("/home/u/a/x"), now), "late news from the folder left");
        assert!(!concerns(&created("/home/u/bc/x"), now), "a folder whose name starts the same");
        assert!(!concerns(&created("/home/u/b/x"), none), "nothing watched");
        assert!(concerns(&Err(notify::Error::generic("overflow")), now), "an error may hide changes");
        assert!(!concerns(&Err(notify::Error::generic("overflow")), none));
        let opened = Ok(notify::Event::new(EventKind::Access(AccessKind::Open(AccessMode::Read))).add_path(PathBuf::from("/home/u/b/x")));
        assert!(!concerns(&opened, now), "reading is not a change");
        let pathless = Ok(notify::Event::new(EventKind::Any));
        assert!(concerns(&pathless, now), "no path given: it may be ours");
    }
    use std::sync::mpsc;
    use std::time::Duration;

    /// Long enough for inotify to deliver what it has.
    const SETTLE: Duration = Duration::from_millis(300);

    fn watcher() -> (Watcher, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let w = Watcher::new(move || {
            let _ = tx.send(());
        });
        (w, rx)
    }

    #[test]
    fn a_change_in_the_watched_folder_wakes_once_and_is_reported() {
        let dir = fresh_fixture("watch-change");
        let (mut w, woke) = watcher();
        w.watch(&dir, &[]);
        w.settle();
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        std::fs::write(dir.join("other.txt"), "y").unwrap();
        woke.recv_timeout(Duration::from_secs(2)).expect("change woke the UI");
        std::thread::sleep(SETTLE);
        assert!(woke.try_recv().is_err(), "a burst wakes only once");
        assert!(w.take_changed());
        assert!(!w.take_changed(), "reported once");
        std::fs::remove_file(dir.join("new.txt")).unwrap();
        woke.recv_timeout(Duration::from_secs(2)).expect("wakes again after take_changed");
        assert!(w.take_changed());
    }

    #[test]
    fn listing_the_folder_or_reading_a_file_is_not_a_change() {
        let dir = fresh_fixture("watch-read");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        // A test spawning a program at that moment forks a copy of a.txt's write
        // handle, closed only when the child starts its program: under load that
        // late close came after watch() as a finished write. Let it land first.
        std::thread::sleep(SETTLE);
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[]);
        w.settle();
        crate::fs::lister::read_entries(&dir).unwrap();
        std::fs::read(dir.join("a.txt")).unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "otherwise every listing would trigger another");
    }

    #[test]
    fn this_pc_is_watched_as_nothing_and_stops_the_old_folder() {
        let dir = fresh_fixture("watch-this-pc");
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[]);
        w.settle();
        assert_eq!(w.watched.as_deref(), Some(dir.as_path()));
        // News from the folder that came in just before leaving it.
        w.changed.store(true, Ordering::SeqCst);
        w.watch(&crate::places::this_pc(), &[]);
        w.settle();
        assert_eq!(w.watched, None);
        std::fs::write(dir.join("x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "the folder left is no longer watched");
    }

    #[test]
    fn the_recycle_bin_is_watched_as_nothing_too() {
        let dir = fresh_fixture("watch-recycle-bin");
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[]);
        w.settle();
        w.changed.store(true, Ordering::SeqCst);
        w.watch(&crate::places::recycle_bin(), &[]);
        w.settle();
        assert_eq!(w.watched, None);
        std::fs::write(dir.join("x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "the folder left is no longer watched");
    }

    #[test]
    fn switching_folders_stops_watching_the_old_one() {
        let dir = fresh_fixture("watch-switch");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&dir.join("a"), &[]);
        w.settle();
        w.watch(&dir.join("b"), &[]);
        w.settle();
        std::fs::write(dir.join("a/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed());
        std::fs::write(dir.join("b/y"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed());
    }

    /// Not on Windows: there a write inside a subfolder changes the
    /// subfolder's own date, which the folder reports (sometimes, as NTFS
    /// updates it lazily), and reloading for it is right: the list shows that date.
    #[cfg(not(windows))]
    #[test]
    fn subfolders_are_not_watched() {
        let dir = fresh_fixture("watch-shallow");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[]);
        w.settle();
        std::fs::write(dir.join("sub/deep.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed());
    }

    #[test]
    fn a_flattened_folder_is_watched_all_the_way_down() {
        let dir = fresh_fixture("watch-deep");
        std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[dir.join("sub"), dir.join("sub/deeper")]);
        w.settle();
        std::fs::write(dir.join("sub/deeper/x.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "a change two levels down");
    }

    /// Not on Windows: see `subfolders_are_not_watched`.
    #[cfg(not(windows))]
    #[test]
    fn only_the_flattened_subfolders_are_watched_all_the_way_down() {
        let dir = fresh_fixture("watch-deep-partial");
        std::fs::create_dir_all(dir.join("a/inner")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[dir.join("a"), dir.join("a/inner")]);
        w.settle();
        std::fs::write(dir.join("b/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "a subfolder not flattened");
        std::fs::write(dir.join("a/inner/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "inside a flattened one");
        std::fs::write(dir.join("top.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "the folder itself");
    }

    /// Not on Windows: see `subfolders_are_not_watched`.
    #[cfg(not(windows))]
    #[test]
    fn listing_the_folder_alone_again_stops_watching_its_subfolders() {
        let dir = fresh_fixture("watch-deep-off");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[dir.join("sub")]);
        w.settle();
        w.watch(&dir, &[]);
        w.settle();
        std::fs::write(dir.join("sub/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "the subfolder is no longer watched");
        std::fs::write(dir.join("y"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "the folder still is");
    }

    /// Flattening does not enter links (`fs::flat::walk`), so what one
    /// points to is not watched either.
    #[cfg(unix)]
    #[test]
    fn a_link_inside_a_flattened_folder_is_not_followed() {
        let dir = fresh_fixture("watch-deep-link");
        std::fs::create_dir_all(dir.join("open/sub")).unwrap();
        std::fs::create_dir_all(dir.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("open/sub/link")).unwrap();
        let (mut w, _woke) = watcher();
        let open = dir.join("open");
        // What `fs::flat::walk` entered: not the link.
        w.watch(&open, &[open.join("sub")]);
        w.settle();
        std::fs::write(dir.join("elsewhere/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed());
    }

    #[test]
    fn on_windows_the_topmost_folders_are_watched_all_the_way_down() {
        let p = |s: &str| PathBuf::from(s);
        let walked = [p("/x/a"), p("/x/a/b"), p("/x/c"), p("/x/c/d/e")];
        let elsewhere = Plan::new(Path::new("/x"), &walked, true);
        assert_eq!(elsewhere, Plan { alone: walked.to_vec(), trees: vec![] }, "inotify: each folder alone");
        let picked = Plan::new(Path::new("/x"), &walked, false);
        assert_eq!(picked, Plan { alone: vec![], trees: vec![p("/x/a"), p("/x/c"), p("/x/c/d/e")] }, "no handle under another");
        let mut whole = walked.to_vec();
        whole.insert(0, p("/x"));
        assert_eq!(Plan::new(Path::new("/x"), &whole, false), Plan { alone: vec![], trees: vec![p("/x")] }, "one handle");
        assert_eq!(Plan::new(Path::new("/x"), &whole, true).alone, walked, "the folder itself is watched anyway");
    }

    #[test]
    fn more_folders_than_the_limit_are_not_watched_and_it_says_so() {
        let dir = fresh_fixture("watch-limit");
        let (mut w, _woke) = watcher();
        let folders: Vec<PathBuf> = (0..=WATCH_LIMIT).map(|i| dir.join(format!("d{i}"))).collect();
        w.watch(&dir, &folders);
        w.settle();
        assert!(w.take_refused(), "too many folders");
        assert!(!w.take_refused(), "said once");
        std::fs::write(dir.join("top.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "not even the folder itself: its reload would walk them all again");
    }

    /// Notify's own watcher, but the system's watches run out at a folder named `full`.
    struct Full(RecommendedWatcher);

    impl notify::Watcher for Full {
        fn new<F: notify::EventHandler>(handler: F, config: Config) -> notify::Result<Self> {
            RecommendedWatcher::new(handler, config).map(Full)
        }

        fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
            if path.file_name().is_some_and(|name| name == "full") {
                return Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch));
            }
            self.0.watch(path, mode)
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.0.unwatch(path)
        }

        fn kind() -> notify::WatcherKind {
            RecommendedWatcher::kind()
        }
    }

    /// Notify's own watcher, telling when a path is watched again without
    /// an unwatch first: on Windows notify then leaks the old handle.
    struct Strict(RecommendedWatcher, HashSet<PathBuf>);

    static WATCHED_TWICE: AtomicBool = AtomicBool::new(false);

    impl notify::Watcher for Strict {
        fn new<F: notify::EventHandler>(handler: F, config: Config) -> notify::Result<Self> {
            RecommendedWatcher::new(handler, config).map(|inner| Strict(inner, HashSet::new()))
        }

        fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
            if !self.1.insert(path.to_path_buf()) {
                WATCHED_TWICE.store(true, Ordering::SeqCst);
            }
            self.0.watch(path, mode)
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.1.remove(path);
            self.0.unwatch(path)
        }

        fn kind() -> notify::WatcherKind {
            RecommendedWatcher::kind()
        }
    }

    #[test]
    fn a_folder_is_never_watched_twice_without_an_unwatch() {
        let dir = fresh_fixture("watch-no-double");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let mut w = Watcher::start::<Strict>(|| {});
        w.watch(&dir, &[dir.join("a")]);
        w.settle();
        // A flattened view reloaded, then listed alone again.
        w.watch(&dir, &[dir.join("a")]);
        w.settle();
        w.watch(&dir, &[]);
        w.settle();
        assert!(!WATCHED_TWICE.load(Ordering::SeqCst));
        std::fs::write(dir.join("top.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "still watched");
    }

    #[test]
    fn when_the_systems_watches_run_out_every_watch_is_released() {
        let dir = fresh_fixture("watch-refused");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("full")).unwrap();
        let (tx, woke) = mpsc::channel();
        let mut w = Watcher::start::<Full>(move || {
            let _ = tx.send(());
        });
        w.watch(&dir, &[dir.join("a"), dir.join("full")]);
        woke.recv_timeout(Duration::from_secs(2)).expect("giving up wakes the UI");
        w.settle();
        assert!(w.take_refused());
        std::fs::write(dir.join("a/x"), "").unwrap();
        std::fs::write(dir.join("top.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "nothing stays watched");
    }

    #[test]
    fn a_folder_gone_before_it_is_watched_is_skipped() {
        let dir = fresh_fixture("watch-gone");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[dir.join("gone"), dir.join("a")]);
        w.settle();
        assert!(!w.take_refused(), "a build tool's folder may come and go");
        std::fs::write(dir.join("a/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "the others are watched");
    }

    #[test]
    fn watching_folders_does_not_wait_for_the_system() {
        let dir = fresh_fixture("watch-async");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let (mut w, _woke) = watcher();
        let hold = w.hold();
        let (done_tx, done) = mpsc::channel();
        let folders = [dir.join("a")];
        std::thread::scope(|s| {
            s.spawn(|| {
                w.watch(&dir, &folders);
                let _ = done_tx.send(());
            });
            let returned = done.recv_timeout(Duration::from_secs(2)).is_ok();
            drop(hold);
            assert!(returned, "watch() returned while the folders were still being watched");
        });
        w.settle();
        std::fs::write(dir.join("a/x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed(), "watched once the thread got to it");
    }

    #[test]
    fn flattening_the_folder_watched_keeps_a_change_not_yet_taken() {
        let dir = fresh_fixture("watch-deep-keep");
        let (mut w, _woke) = watcher();
        w.watch(&dir, &[]);
        w.settle();
        w.changed.store(true, Ordering::SeqCst);
        w.watch(&dir, &[dir.join("sub")]);
        w.settle();
        assert!(w.take_changed(), "the folder's own change still needs its reload");
    }

    #[test]
    fn a_forgotten_folder_is_watched_again_when_recreated() {
        let dir = fresh_fixture("watch-recreate");
        let open = dir.join("open");
        std::fs::create_dir(&open).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&open, &[]);
        w.settle();
        std::fs::remove_dir(&open).unwrap();
        std::thread::sleep(SETTLE);
        w.take_changed();
        w.forget();
        w.settle();
        std::fs::create_dir(&open).unwrap();
        w.watch(&open, &[]);
        w.settle();
        std::fs::write(open.join("x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed());
    }

    #[test]
    fn a_missing_folder_is_ignored() {
        let dir = fresh_fixture("watch-missing");
        let (mut w, _woke) = watcher();
        w.watch(&dir.join("gone"), &[]);
        w.settle();
        assert!(!w.take_changed());
    }

    #[test]
    fn only_real_changes_count() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        assert!(is_change(&EventKind::Create(CreateKind::File)));
        assert!(is_change(&EventKind::Remove(RemoveKind::Any)));
        assert!(is_change(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_change(&EventKind::Access(AccessKind::Close(AccessMode::Write))));
        assert!(!is_change(&EventKind::Access(AccessKind::Open(AccessMode::Any))));
        assert!(!is_change(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
    }
}
