//! Watches the open folder so the list can reload itself when something changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// Watches one folder (not its subfolders) at a time.
pub struct Watcher {
    inner: Option<RecommendedWatcher>,
    watched: Option<PathBuf>,
    /// Set by notify's thread, cleared by [`Watcher::take_changed`].
    changed: Arc<AtomicBool>,
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

impl Watcher {
    /// `wake` runs on notify's thread on the first change after each
    /// [`Watcher::take_changed`], so a burst wakes the UI once.
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let flag = changed.clone();
        let inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            // An error (e.g. the kernel queue overflowed) may hide changes: reload.
            if event.map_or(true, |e| is_change(&e.kind)) && !flag.swap(true, Ordering::SeqCst) {
                wake();
            }
        })
        .ok();
        Self { inner, watched: None, changed }
    }

    /// Starts watching `path` instead of the previous folder. A folder that
    /// cannot be watched (no permission, too many watches) just does not refresh.
    pub fn watch(&mut self, path: &Path) {
        if self.watched.as_deref() == Some(path) {
            return;
        }
        // This PC is no folder, and as a relative path it could name one.
        if crate::places::is_this_pc(path) {
            self.forget();
            return;
        }
        let Some(inner) = &mut self.inner else { return };
        if let Some(old) = self.watched.take() {
            let _ = inner.unwatch(&old);
        }
        // News from the old folder no longer matter; the new one was just listed.
        self.changed.store(false, Ordering::SeqCst);
        if inner.watch(path, RecursiveMode::NonRecursive).is_ok() {
            self.watched = Some(path.to_path_buf());
        }
    }

    /// Stops watching, e.g. because the folder was deleted; the next
    /// [`Watcher::watch`] starts afresh even for the same path.
    pub fn forget(&mut self) {
        if let (Some(inner), Some(old)) = (&mut self.inner, self.watched.take()) {
            let _ = inner.unwatch(&old);
        }
    }

    /// Whether something changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;
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
        w.watch(&dir);
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
        w.watch(&dir);
        crate::fs::lister::read_entries(&dir).unwrap();
        std::fs::read(dir.join("a.txt")).unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed(), "otherwise every listing would trigger another");
    }

    #[test]
    fn this_pc_is_watched_as_nothing_and_stops_the_old_folder() {
        let dir = fresh_fixture("watch-this-pc");
        let (mut w, _woke) = watcher();
        w.watch(&dir);
        assert_eq!(w.watched.as_deref(), Some(dir.as_path()));
        w.watch(&crate::places::this_pc());
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
        w.watch(&dir.join("a"));
        w.watch(&dir.join("b"));
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
        w.watch(&dir);
        std::fs::write(dir.join("sub/deep.txt"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(!w.take_changed());
    }

    #[test]
    fn a_forgotten_folder_is_watched_again_when_recreated() {
        let dir = fresh_fixture("watch-recreate");
        let open = dir.join("open");
        std::fs::create_dir(&open).unwrap();
        let (mut w, _woke) = watcher();
        w.watch(&open);
        std::fs::remove_dir(&open).unwrap();
        std::thread::sleep(SETTLE);
        w.take_changed();
        w.forget();
        std::fs::create_dir(&open).unwrap();
        w.watch(&open);
        std::fs::write(open.join("x"), "").unwrap();
        std::thread::sleep(SETTLE);
        assert!(w.take_changed());
    }

    #[test]
    fn a_missing_folder_is_ignored() {
        let dir = fresh_fixture("watch-missing");
        let (mut w, _woke) = watcher();
        w.watch(&dir.join("gone"));
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
