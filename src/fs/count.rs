//! Folder size and item counts, computed on demand (Alt+R) on a background
//! thread, because a big tree can take seconds to walk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// What a folder holds, counted all the way down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub folders: u64,
    /// Files, symlinks and special files.
    pub files: u64,
    /// Bytes of regular files.
    pub bytes: u64,
    /// Subfolders that could not be read; their contents are missing.
    pub unreadable: u64,
}

/// Walks `root` and counts what is inside it (not `root` itself). Symlinks
/// count as files and are never followed; folders on another disk (a mount
/// point) count as folders but are not entered, so `/` does not walk `/proc`.
/// Returns `Ok(None)` when `cancel` is set. `on_progress` sees running totals.
pub fn count(
    root: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&Totals),
) -> Result<Option<Totals>, String> {
    let device = device_of(root).map_err(|err| format!("Cannot read {}: {err}", root.display()))?;
    let mut totals = Totals::default();
    // A stack, not recursion: a very deep tree cannot overflow it.
    let mut pending = vec![root.to_path_buf()];
    let mut is_root = true;
    while let Some(dir) = pending.pop() {
        let items = match std::fs::read_dir(&dir) {
            Ok(items) => items,
            Err(err) if is_root => return Err(format!("Cannot read {}: {err}", root.display())),
            Err(_) => {
                totals.unreadable += 1;
                continue;
            }
        };
        is_root = false;
        for item in items.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let Ok(meta) = item.metadata() else {
                totals.files += 1;
                continue;
            };
            if meta.is_dir() {
                totals.folders += 1;
                if same_device(&meta, device) {
                    pending.push(item.path());
                }
            } else {
                totals.files += 1;
                if meta.is_file() {
                    totals.bytes += meta.len();
                }
            }
        }
        on_progress(&totals);
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    on_progress(&totals);
    Ok(Some(totals))
}

#[cfg(unix)]
fn device_of(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.dev())
}

#[cfg(unix)]
fn same_device(meta: &std::fs::Metadata, device: u64) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.dev() == device
}

// Windows: no cheap device id in std; drives are separate roots anyway.
#[cfg(not(unix))]
fn device_of(path: &Path) -> std::io::Result<u64> {
    std::fs::metadata(path).map(|_| 0)
}

#[cfg(not(unix))]
fn same_device(_meta: &std::fs::Metadata, _device: u64) -> bool {
    true
}

enum Event {
    Progress(Totals),
    Finished(Result<Option<Totals>, String>),
}

/// Minimum time between progress messages.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// A count running on its own thread.
pub struct Count {
    pub path: PathBuf,
    /// Running totals; updated by [`Count::poll`].
    pub progress: Totals,
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Count {
    /// Starts [`count`] on a new thread; `wake` runs there whenever there is news.
    pub fn start(path: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let root = path.clone();
        thread::spawn(move || {
            let mut sent: Option<Instant> = None;
            let result = count(&root, &flag, |totals| {
                if sent.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
                    sent = Some(Instant::now());
                    // The receiver only disappears when the app is closing.
                    let _ = tx.send(Event::Progress(*totals));
                    wake();
                }
            });
            let _ = tx.send(Event::Finished(result));
            wake();
        });
        Self { path, progress: Totals::default(), rx, cancel }
    }

    /// Asks the worker to stop; it finishes with `Ok(None)`.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Takes the messages that arrived. Returns the result once the count is over.
    pub fn poll(&mut self) -> Option<Result<Option<Totals>, String>> {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Progress(totals) => self.progress = totals,
                Event::Finished(result) => return Some(result),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    fn run(root: &Path) -> Totals {
        count(root, &AtomicBool::new(false), |_| {}).unwrap().expect("not cancelled")
    }

    /// root/a.txt (3 B), root/sub/b.txt (5 B), root/sub/deep/ (empty), root/empty/
    fn tree(name: &str) -> PathBuf {
        let dir = fresh_fixture(name);
        std::fs::write(dir.join("a.txt"), "abc").unwrap();
        std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
        std::fs::write(dir.join("sub/b.txt"), "hello").unwrap();
        std::fs::create_dir(dir.join("empty")).unwrap();
        dir
    }

    #[test]
    fn counts_folders_files_and_bytes_all_the_way_down() {
        let dir = tree("count-tree");
        assert_eq!(run(&dir), Totals { folders: 3, files: 2, bytes: 8, unreadable: 0 });
        assert_eq!(run(&dir.join("empty")), Totals::default());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_count_as_files_and_are_not_followed() {
        let dir = tree("count-links");
        let outside = fresh_fixture("count-links-target");
        std::fs::write(outside.join("big.bin"), vec![0u8; 1000]).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link-to-dir")).unwrap();
        // A loop would never end if links were followed.
        std::os::unix::fs::symlink(&dir, dir.join("sub/loop")).unwrap();
        assert_eq!(run(&dir), Totals { folders: 3, files: 4, bytes: 8, unreadable: 0 });
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_folders_are_counted_and_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tree("count-unreadable");
        let locked = dir.join("sub/deep");
        std::fs::write(locked.join("hidden.txt"), "xx").unwrap();
        /// Unlocks the folder even if the test panics, so the next run can clean up.
        struct Unlock(PathBuf);
        impl Drop for Unlock {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _unlock = Unlock(locked);
        let totals = run(&dir);
        assert_eq!(totals, Totals { folders: 3, files: 2, bytes: 8, unreadable: 1 });
    }

    #[test]
    fn an_unreadable_root_is_an_error() {
        let dir = fresh_fixture("count-missing");
        let err = count(&dir.join("gone"), &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(err.contains("gone"), "{err}");
    }

    #[test]
    fn cancel_stops_with_nothing() {
        let dir = tree("count-cancel");
        assert_eq!(count(&dir, &AtomicBool::new(true), |_| {}), Ok(None));
    }

    #[test]
    fn progress_reports_running_totals() {
        let dir = tree("count-progress");
        let mut last = None;
        let total = count(&dir, &AtomicBool::new(false), |t| last = Some(*t)).unwrap();
        assert_eq!(last, total, "the last progress is the final total");
    }

    #[test]
    fn the_background_count_delivers_its_result() {
        let dir = tree("count-job");
        let mut job = Count::start(dir.clone(), || {});
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if let Some(result) = job.poll() {
                break result;
            }
            assert!(Instant::now() < deadline, "count did not finish");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(result.unwrap().unwrap().files, 2);
        assert_eq!(job.path, dir);
    }
}
