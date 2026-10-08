//! Copy and move (cut + paste). Runs on a worker thread: reports progress
//! through a callback and stops when the cancel flag is set.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::ops::{exists, same_entry, trash};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Copy,
    Move,
}

/// Where a paste would land, decided before anything is touched.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Free target: run the transfer to this path.
    Ready(PathBuf),
    /// Cut + paste into the folder it already lives in.
    Nothing,
    /// The name is taken in the destination; ask before replacing it.
    TargetExists(PathBuf),
}

/// What a finished transfer did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Stopped by the cancel flag. Finished files stay; the half-written one is removed.
    Cancelled,
}

/// Files and bytes copied so far out of the totals measured before starting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

impl Progress {
    /// Share of bytes copied, 0-100. Empty files and folders count as done.
    pub fn percent(&self) -> u8 {
        if self.bytes_total == 0 {
            return 100;
        }
        (self.bytes_done.min(self.bytes_total) * 100 / self.bytes_total) as u8
    }
}

/// Name for the `n`-th copy in the same folder: "notes (copy).txt",
/// "notes (copy 2).txt"; folders and dotfiles get the suffix at the end.
pub fn copy_name(name: &OsStr, n: u32, is_dir: bool) -> OsString {
    let suffix = if n == 1 { " (copy)".to_owned() } else { format!(" (copy {n})") };
    let bytes = name.as_encoded_bytes();
    // Index 0 is a dotfile's leading dot, not an extension.
    let dot = bytes.iter().rposition(|&b| b == b'.').filter(|&i| i > 0 && !is_dir);
    let Some(dot) = dot else {
        let mut out = name.to_os_string();
        out.push(suffix);
        return out;
    };
    let (stem, ext) = bytes.split_at(dot);
    let mut out = Vec::with_capacity(bytes.len() + suffix.len());
    out.extend_from_slice(stem);
    out.extend_from_slice(suffix.as_bytes());
    out.extend_from_slice(ext);
    // SAFETY: split at an ASCII '.' of a valid encoded name and joined with
    // UTF-8, which `from_encoded_bytes_unchecked` allows.
    unsafe { OsString::from_encoded_bytes_unchecked(out) }
}

fn missing(path: &Path) -> String {
    format!("No longer exists: {}", path.display())
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|e| format!("Cannot open {}: {e}", path.display()))
}

/// Resolves the folder holding `path` but not `path` itself, so a symlink
/// stays the link (what gets copied or trashed) instead of its target.
fn canonical_entry(path: &Path) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or_else(|| format!("Cannot use {}", path.display()))?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Ok(canonical(parent)?.join(name))
}

/// Decides where pasting `source` into `dest_dir` goes, without changing anything.
pub fn plan(source: &Path, dest_dir: &Path, mode: Mode) -> Result<Plan, String> {
    let meta = std::fs::symlink_metadata(source).map_err(|_| missing(source))?;
    let entry = canonical_entry(source)?;
    let name = entry.file_name().expect("canonical_entry keeps the name").to_os_string();
    let dest = canonical(dest_dir)?;
    if meta.is_dir() && dest.starts_with(&entry) {
        return Err(format!("Cannot paste “{}” into itself", name.to_string_lossy()));
    }
    // Same inode also catches the folder reached through a bind mount.
    let parent = entry.parent().expect("canonical_entry keeps the parent");
    if parent == dest || same_entry(parent, &dest) {
        return Ok(match mode {
            Mode::Move => Plan::Nothing,
            Mode::Copy => {
                let free = (1..)
                    .map(|n| dest_dir.join(copy_name(&name, n, meta.is_dir())))
                    .find(|path| !exists(path))
                    .expect("the copy counter is unbounded");
                Plan::Ready(free)
            }
        });
    }
    let target = dest_dir.join(&name);
    Ok(if exists(&target) { Plan::TargetExists(target) } else { Plan::Ready(target) })
}

/// Copies or moves `source` to `target`. With `replace`, an entry already at
/// `target` goes to the trash first. A move the filesystem cannot rename
/// (another disk) is a copy followed by sending the source to the trash.
pub fn run(
    source: &Path,
    target: &Path,
    mode: Mode,
    replace: bool,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&Progress),
) -> Result<Outcome, String> {
    if !exists(source) {
        return Err(missing(source));
    }
    if replace && exists(target) {
        // Trashing a folder that holds the source would take the source with it.
        if canonical_entry(source)?.starts_with(canonical_entry(target)?)
            || same_entry(source, target)
        {
            let name = target.file_name().unwrap_or_default().to_string_lossy();
            return Err(format!("Cannot replace “{name}”: it contains what is being pasted"));
        }
        trash(target)?;
    }
    match mode {
        Mode::Copy => copy_all(source, target, cancel, on_progress),
        // `rename` silently replaces a file that appeared after `plan` checked.
        Mode::Move if !replace && exists(target) => Err(format!(
            "“{}” already exists",
            target.file_name().unwrap_or_default().to_string_lossy()
        )),
        Mode::Move => match std::fs::rename(source, target) {
            Ok(()) => {
                on_progress(&Progress { files_done: 1, files_total: 1, ..Progress::default() });
                Ok(Outcome::Done)
            }
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
                move_by_copy(source, target, cancel, on_progress)
            }
            Err(e) => Err(format!("Cannot move {}: {e}", source.display())),
        },
    }
}

/// The cross-device half of a move: copy everything, then trash the source.
fn move_by_copy(
    source: &Path,
    target: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(&Progress),
) -> Result<Outcome, String> {
    let outcome = copy_all(source, target, cancel, on_progress)?;
    if outcome == Outcome::Done {
        trash(source)?;
    }
    Ok(outcome)
}

/// Counts files (symlinks included) and their bytes under `path`. Unreadable
/// parts are skipped here; the copy itself reports them.
fn measure(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return (0, 0) };
    if !meta.is_dir() {
        return (1, if meta.is_file() { meta.len() } else { 0 });
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|item| measure(&item.path()))
        .fold((0, 0), |(f, b), (f2, b2)| (f + f2, b + b2))
}

fn copy_all(
    source: &Path,
    target: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(&Progress),
) -> Result<Outcome, String> {
    let (files_total, bytes_total) = measure(source);
    let mut copier = Copier {
        progress: Progress { files_total, bytes_total, ..Progress::default() },
        cancel,
        on_progress,
        buf: vec![0; 256 * 1024],
    };
    copier.copy(source, target)
}

struct Copier<'a, F> {
    progress: Progress,
    cancel: &'a AtomicBool,
    on_progress: F,
    buf: Vec<u8>,
}

impl<F: FnMut(&Progress)> Copier<'_, F> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn file_done(&mut self) {
        self.progress.files_done += 1;
        (self.on_progress)(&self.progress);
    }

    fn copy(&mut self, from: &Path, to: &Path) -> Result<Outcome, String> {
        if self.cancelled() {
            return Ok(Outcome::Cancelled);
        }
        let fail = |e: std::io::Error| format!("Cannot copy {}: {e}", from.display());
        let meta = std::fs::symlink_metadata(from).map_err(fail)?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            copy_link(from, to).map_err(fail)?;
            self.file_done();
        } else if kind.is_dir() {
            std::fs::create_dir(to).map_err(fail)?;
            let outcome = self.copy_children(from, to);
            if !matches!(outcome, Ok(Outcome::Done)) {
                // A folder this copy created and left empty is not "already copied".
                // `remove_dir` refuses a folder with anything inside, so finished
                // files are never touched.
                let _ = std::fs::remove_dir(to);
                return outcome;
            }
            // Last, so a read-only folder still receives its contents.
            std::fs::set_permissions(to, meta.permissions()).map_err(fail)?;
        } else if kind.is_file() {
            return self.copy_file(from, to, &meta);
        } else {
            // Pipes, sockets and devices: opening one can block forever or never end.
            return Err(format!(
                "Cannot copy {}: not a regular file, folder or link",
                from.display()
            ));
        }
        Ok(Outcome::Done)
    }

    fn copy_children(&mut self, from: &Path, to: &Path) -> Result<Outcome, String> {
        let fail = |e: std::io::Error| format!("Cannot copy {}: {e}", from.display());
        // Sorted so a cancelled copy leaves a predictable prefix behind.
        let mut names: Vec<_> =
            std::fs::read_dir(from).map_err(fail)?.flatten().map(|i| i.file_name()).collect();
        names.sort();
        for name in names {
            if self.copy(&from.join(&name), &to.join(&name))? == Outcome::Cancelled {
                return Ok(Outcome::Cancelled);
            }
        }
        Ok(Outcome::Done)
    }

    /// Chunked so progress moves and cancel is noticed inside big files.
    /// A half-written file is removed on cancel or error.
    fn copy_file(&mut self, from: &Path, to: &Path, meta: &Metadata) -> Result<Outcome, String> {
        let fail = |e: std::io::Error| format!("Cannot copy {}: {e}", from.display());
        let mut src = File::open(from).map_err(fail)?;
        let mut dst = OpenOptions::new().write(true).create_new(true).open(to).map_err(fail)?;
        let discard = |dst: File| {
            drop(dst);
            let _ = std::fs::remove_file(to);
        };
        loop {
            if self.cancelled() {
                discard(dst);
                return Ok(Outcome::Cancelled);
            }
            let n = match src.read(&mut self.buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    discard(dst);
                    return Err(fail(e));
                }
            };
            if let Err(e) = dst.write_all(&self.buf[..n]) {
                discard(dst);
                return Err(fail(e));
            }
            self.progress.bytes_done += n as u64;
            (self.on_progress)(&self.progress);
        }
        // Like `cp -p`: same permissions and date; a date failure is harmless.
        if let Err(e) = dst.set_permissions(meta.permissions()) {
            discard(dst);
            return Err(fail(e));
        }
        if let Ok(modified) = meta.modified() {
            let _ = dst.set_modified(modified);
        }
        self.file_done();
        Ok(Outcome::Done)
    }
}

/// What the worker thread tells the UI.
enum JobEvent {
    Progress(Progress),
    Finished(Result<Outcome, String>),
}

/// A copy or move running on its own thread, so the window keeps responding.
pub struct Job {
    pub mode: Mode,
    /// Latest progress received; updated by [`Job::poll`].
    pub progress: Progress,
    rx: Receiver<JobEvent>,
    cancel: Arc<AtomicBool>,
}

/// Minimum time between progress messages, so a fast copy does not flood the UI.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

impl Job {
    /// Starts [`run`] on a new thread. `wake` runs on that thread whenever there
    /// is news (the UI uses it to request a repaint).
    pub fn start(
        source: PathBuf,
        target: PathBuf,
        mode: Mode,
        replace: bool,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        Self::start_with(Arc::new(AtomicBool::new(false)), source, target, mode, replace, wake)
    }

    fn start_with(
        cancel: Arc<AtomicBool>,
        source: PathBuf,
        target: PathBuf,
        mode: Mode,
        replace: bool,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let flag = cancel.clone();
        thread::spawn(move || {
            let mut latest = Progress::default();
            let mut sent: Option<Instant> = None;
            let result = run(&source, &target, mode, replace, &flag, |p| {
                latest = *p;
                if sent.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
                    sent = Some(Instant::now());
                    // The receiver only disappears when the app is closing.
                    let _ = tx.send(JobEvent::Progress(*p));
                    wake();
                }
            });
            let _ = tx.send(JobEvent::Progress(latest));
            let _ = tx.send(JobEvent::Finished(result));
            wake();
        });
        Self { mode, progress: Progress::default(), rx, cancel }
    }

    /// Asks the worker to stop; it finishes with [`Outcome::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Takes the messages that arrived. Returns the result once the job is over.
    pub fn poll(&mut self) -> Option<Result<Outcome, String>> {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                JobEvent::Progress(p) => self.progress = p,
                JobEvent::Finished(result) => return Some(result),
            }
        }
        None
    }
}

/// Copies the link itself, never what it points to.
#[cfg(unix)]
fn copy_link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
}

#[cfg(windows)]
fn copy_link(from: &Path, to: &Path) -> std::io::Result<()> {
    let link = std::fs::read_link(from)?;
    if std::fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(link, to)
    } else {
        std::os::windows::fs::symlink_file(link, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::{fresh_fixture, purge_trashed_from};

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn in_trash(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = trash::os_limited::list()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.original_parent == dir)
            .map(|item| item.name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn copy_names_keep_the_extension_last() {
        assert_eq!(copy_name(os("notes.txt"), 1, false), "notes (copy).txt");
        assert_eq!(copy_name(os("notes.txt"), 2, false), "notes (copy 2).txt");
        assert_eq!(copy_name(os("src"), 1, true), "src (copy)");
        assert_eq!(copy_name(os("v1.2"), 3, true), "v1.2 (copy 3)", "folders have no extension");
        assert_eq!(copy_name(os(".bashrc"), 1, false), ".bashrc (copy)");
        assert_eq!(copy_name(os("Makefile"), 1, false), "Makefile (copy)");
    }

    #[test]
    fn percent_counts_bytes_and_empty_work_is_complete() {
        let p = Progress { files_done: 3, files_total: 10, bytes_done: 45, bytes_total: 100 };
        assert_eq!(p.percent(), 45);
        assert_eq!(Progress::default().percent(), 100);
    }

    #[test]
    fn copying_into_the_same_folder_picks_a_free_copy_name() {
        let dir = fresh_fixture("xfer-plan-same");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("a (copy).txt"), "").unwrap();
        assert_eq!(
            plan(&dir.join("a.txt"), &dir, Mode::Copy),
            Ok(Plan::Ready(dir.join("a (copy 2).txt")))
        );
        assert_eq!(plan(&dir.join("a.txt"), &dir, Mode::Move), Ok(Plan::Nothing));
    }

    #[test]
    fn pasting_elsewhere_keeps_the_name_and_reports_a_taken_one() {
        let dir = fresh_fixture("xfer-plan-other");
        std::fs::create_dir(dir.join("dst")).unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        for mode in [Mode::Copy, Mode::Move] {
            assert_eq!(
                plan(&dir.join("a.txt"), &dir.join("dst"), mode),
                Ok(Plan::Ready(dir.join("dst/a.txt")))
            );
        }
        std::fs::write(dir.join("dst/a.txt"), "old").unwrap();
        assert_eq!(
            plan(&dir.join("a.txt"), &dir.join("dst"), Mode::Copy),
            Ok(Plan::TargetExists(dir.join("dst/a.txt")))
        );
        assert_eq!(read(&dir.join("dst/a.txt")), "old", "planning changes nothing");
    }

    #[test]
    fn a_folder_cannot_be_pasted_into_itself() {
        let dir = fresh_fixture("xfer-plan-self");
        std::fs::create_dir_all(dir.join("box/inner")).unwrap();
        std::os::unix::fs::symlink(dir.join("box/inner"), dir.join("alias")).unwrap();
        for dest in ["box", "box/inner", "alias"] {
            for mode in [Mode::Copy, Mode::Move] {
                let err = plan(&dir.join("box"), &dir.join(dest), mode).unwrap_err();
                assert_eq!(err, "Cannot paste “box” into itself", "dest {dest}, {mode:?}");
            }
        }
        // A sibling whose name merely starts the same is fine.
        std::fs::create_dir(dir.join("boxes")).unwrap();
        assert!(plan(&dir.join("box"), &dir.join("boxes"), Mode::Copy).is_ok());
    }

    #[test]
    fn pasting_a_missing_entry_is_an_error() {
        let dir = fresh_fixture("xfer-plan-missing");
        let err = plan(&dir.join("ghost"), &dir, Mode::Copy).unwrap_err();
        assert!(err.contains("ghost"), "error names the item: {err}");
    }

    /// src/{a.txt, sub/b.bin, sub/run.sh (executable), link -> a.txt}
    fn sample_tree(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "hello").unwrap();
        std::fs::write(src.join("sub/b.bin"), vec![7u8; 300_000]).unwrap();
        std::fs::write(src.join("sub/run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(src.join("sub/run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::os::unix::fs::symlink("a.txt", src.join("link")).unwrap();
        src
    }

    fn assert_same_tree(target: &Path) {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(read(&target.join("a.txt")), "hello");
        assert_eq!(std::fs::read(target.join("sub/b.bin")).unwrap(), vec![7u8; 300_000]);
        let mode = std::fs::metadata(target.join("sub/run.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "permissions are copied");
        assert_eq!(std::fs::read_link(target.join("link")).unwrap(), Path::new("a.txt"));
    }

    #[test]
    fn copy_duplicates_a_tree_and_reports_progress_to_the_end() {
        let dir = fresh_fixture("xfer-copy");
        let src = sample_tree(&dir);
        let mut seen = Vec::new();
        let done = run(&src, &dir.join("dst"), Mode::Copy, false, &never(), |p| seen.push(*p));
        assert_eq!(done, Ok(Outcome::Done));
        assert_same_tree(&dir.join("dst"));
        assert_same_tree(&src);
        let last = seen.last().expect("progress was reported");
        assert_eq!((last.files_done, last.files_total), (4, 4));
        assert_eq!((last.bytes_done, last.bytes_total), (300_015, 300_015));
        assert!(seen.windows(2).all(|w| w[0].bytes_done <= w[1].bytes_done));
    }

    #[test]
    fn cancelling_keeps_finished_files_and_removes_the_half_written_one() {
        let dir = fresh_fixture("xfer-cancel");
        let src = dir.join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a_small"), "done").unwrap();
        std::fs::write(src.join("b_big"), vec![1u8; 4 << 20]).unwrap();
        let cancel = never();
        let done = run(&src, &dir.join("dst"), Mode::Copy, false, &cancel, |p| {
            // Stop in the middle of the big file.
            if p.files_done == 1 && p.bytes_done > 1 << 20 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(done, Ok(Outcome::Cancelled));
        assert_eq!(read(&dir.join("dst/a_small")), "done");
        assert!(!dir.join("dst/b_big").exists(), "partial file removed");
        assert_eq!(std::fs::metadata(src.join("b_big")).unwrap().len(), 4 << 20);
    }

    #[test]
    fn cancelling_a_folder_copy_leaves_no_empty_folders_behind() {
        let dir = fresh_fixture("xfer-cancel-empty");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/big"), vec![1u8; 4 << 20]).unwrap();
        let cancel = never();
        let done = run(&src, &dir.join("dst"), Mode::Copy, false, &cancel, |p| {
            if p.bytes_done > 1 << 20 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(done, Ok(Outcome::Cancelled));
        assert!(!dir.join("dst").exists(), "nothing was finished, so nothing stays");
    }

    #[test]
    fn cancelling_keeps_folders_that_hold_finished_files() {
        let dir = fresh_fixture("xfer-cancel-keep");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("a_sub")).unwrap();
        std::fs::write(src.join("a_sub/done.txt"), "D").unwrap();
        std::fs::create_dir_all(src.join("b_empty")).unwrap();
        std::fs::write(src.join("c_big"), vec![1u8; 4 << 20]).unwrap();
        let cancel = never();
        let done = run(&src, &dir.join("dst"), Mode::Copy, false, &cancel, |p| {
            if p.bytes_done > 1 << 20 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(done, Ok(Outcome::Cancelled));
        assert_eq!(read(&dir.join("dst/a_sub/done.txt")), "D");
        assert!(dir.join("dst/b_empty").is_dir(), "an empty source folder was finished too");
        assert!(!dir.join("dst/c_big").exists());
    }

    #[test]
    fn move_within_a_disk_renames() {
        let dir = fresh_fixture("xfer-move");
        let src = sample_tree(&dir);
        std::fs::create_dir(dir.join("to")).unwrap();
        let done = run(&src, &dir.join("to/src"), Mode::Move, false, &never(), |_| {});
        assert_eq!(done, Ok(Outcome::Done));
        assert!(!src.exists());
        assert_same_tree(&dir.join("to/src"));
        assert_eq!(in_trash(&dir), Vec::<String>::new(), "a rename trashes nothing");
    }

    #[test]
    fn move_across_disks_copies_then_trashes_the_source() {
        let dir = fresh_fixture("xfer-move-copy");
        let src = sample_tree(&dir);
        let done = move_by_copy(&src, &dir.join("moved"), &never(), |_| {});
        assert_eq!(done, Ok(Outcome::Done));
        assert_same_tree(&dir.join("moved"));
        assert!(!src.exists());
        assert_eq!(in_trash(&dir), ["src"], "the original is recoverable");
        purge_trashed_from(&dir);
    }

    #[test]
    fn cancelled_cross_disk_move_keeps_the_source() {
        let dir = fresh_fixture("xfer-move-cancel");
        let src = sample_tree(&dir);
        let done = move_by_copy(&src, &dir.join("moved"), &AtomicBool::new(true), |_| {});
        assert_eq!(done, Ok(Outcome::Cancelled));
        assert_same_tree(&src);
        assert_eq!(in_trash(&dir), Vec::<String>::new());
    }

    #[test]
    fn replacing_trashes_the_existing_target_first() {
        let dir = fresh_fixture("xfer-replace");
        std::fs::create_dir(dir.join("dst")).unwrap();
        std::fs::write(dir.join("a.txt"), "new").unwrap();
        std::fs::write(dir.join("dst/a.txt"), "old").unwrap();
        let done = run(&dir.join("a.txt"), &dir.join("dst/a.txt"), Mode::Copy, true, &never(), |_| {});
        assert_eq!(done, Ok(Outcome::Done));
        assert_eq!(read(&dir.join("dst/a.txt")), "new");
        assert_eq!(read(&dir.join("a.txt")), "new", "copy keeps the source");
        assert_eq!(in_trash(&dir.join("dst")), ["a.txt"]);
        purge_trashed_from(&dir.join("dst"));
    }

    #[test]
    fn replacing_a_folder_that_holds_the_source_is_refused() {
        let dir = fresh_fixture("xfer-replace-parent");
        std::fs::create_dir(dir.join("x")).unwrap();
        std::fs::write(dir.join("x/x"), "inner").unwrap();
        // Cut x/x and paste it next to its folder: the target is the folder itself.
        assert_eq!(plan(&dir.join("x/x"), &dir, Mode::Move), Ok(Plan::TargetExists(dir.join("x"))));
        let err = run(&dir.join("x/x"), &dir.join("x"), Mode::Move, true, &never(), |_| {})
            .unwrap_err();
        assert_eq!(err, "Cannot replace “x”: it contains what is being pasted");
        assert_eq!(read(&dir.join("x/x")), "inner");
        assert_eq!(in_trash(&dir), Vec::<String>::new());
    }

    /// Polls `job` each time it wakes us until it reports a result.
    fn wait(job: &mut Job, woke: &Receiver<()>) -> Result<Outcome, String> {
        loop {
            woke.recv_timeout(Duration::from_secs(5)).expect("job woke the UI");
            if let Some(result) = job.poll() {
                return result;
            }
        }
    }

    #[test]
    fn a_job_copies_in_the_background_and_ends_with_full_progress() {
        let dir = fresh_fixture("xfer-job");
        let src = sample_tree(&dir);
        let (tx, woke) = mpsc::channel();
        let mut job = Job::start(src, dir.join("dst"), Mode::Copy, false, move || {
            let _ = tx.send(());
        });
        assert_eq!(job.mode, Mode::Copy);
        assert_eq!(wait(&mut job, &woke), Ok(Outcome::Done));
        assert_same_tree(&dir.join("dst"));
        assert_eq!((job.progress.files_done, job.progress.files_total), (4, 4));
        assert_eq!(job.progress.percent(), 100);
    }

    #[test]
    fn a_cancelled_job_reports_it() {
        let dir = fresh_fixture("xfer-job-cancel");
        let src = sample_tree(&dir);
        let (tx, woke) = mpsc::channel();
        let flag = Arc::new(AtomicBool::new(false));
        let mut job = Job::start_with(flag.clone(), src, dir.join("dst"), Mode::Copy, false, move || {
            let _ = tx.send(());
        });
        job.cancel();
        assert!(flag.load(Ordering::Relaxed), "cancel reaches the worker's flag");
        // Raced against a fast copy, so only check the job ends cleanly.
        assert!(matches!(wait(&mut job, &woke), Ok(Outcome::Cancelled | Outcome::Done)));
    }

    #[test]
    fn a_job_cancelled_before_it_starts_copies_nothing() {
        let dir = fresh_fixture("xfer-job-precancel");
        let src = sample_tree(&dir);
        let (tx, woke) = mpsc::channel();
        let flag = Arc::new(AtomicBool::new(true));
        let mut job = Job::start_with(flag, src, dir.join("dst"), Mode::Copy, false, move || {
            let _ = tx.send(());
        });
        assert_eq!(wait(&mut job, &woke), Ok(Outcome::Cancelled));
        assert!(!dir.join("dst").exists());
    }

    #[test]
    fn a_failing_job_reports_the_error() {
        let dir = fresh_fixture("xfer-job-error");
        let (tx, woke) = mpsc::channel();
        let mut job = Job::start(dir.join("ghost"), dir.join("t"), Mode::Move, false, move || {
            let _ = tx.send(());
        });
        let err = wait(&mut job, &woke).unwrap_err();
        assert!(err.contains("ghost"), "{err}");
    }

    #[test]
    fn same_folder_through_a_symlinked_path() {
        let dir = fresh_fixture("xfer-same-link");
        std::fs::create_dir(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/a.txt"), "A").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        assert_eq!(
            plan(&dir.join("real/a.txt"), &dir.join("alias"), Mode::Copy),
            Ok(Plan::Ready(dir.join("alias/a (copy).txt")))
        );
        assert_eq!(plan(&dir.join("alias/a.txt"), &dir.join("real"), Mode::Move), Ok(Plan::Nothing));
    }

    #[test]
    fn replacing_a_link_to_the_source_keeps_the_source() {
        let dir = fresh_fixture("xfer-replace-link");
        std::fs::create_dir(dir.join("q")).unwrap();
        std::fs::write(dir.join("f.txt"), "SRC").unwrap();
        std::os::unix::fs::symlink(dir.join("f.txt"), dir.join("q/f.txt")).unwrap();
        let done = run(&dir.join("f.txt"), &dir.join("q/f.txt"), Mode::Copy, true, &never(), |_| {});
        assert_eq!(done, Ok(Outcome::Done));
        assert_eq!(read(&dir.join("f.txt")), "SRC");
        assert_eq!(read(&dir.join("q/f.txt")), "SRC");
        purge_trashed_from(&dir.join("q"));
    }

    #[test]
    fn non_utf8_copy_name_keeps_the_extension() {
        use std::os::unix::ffi::OsStrExt;
        let name = copy_name(OsStr::from_bytes(b"caf\xe9.txt"), 1, false);
        assert_eq!(name.as_bytes(), b"caf\xe9 (copy).txt");
    }

    #[test]
    fn a_fifo_does_not_block_the_copy() {
        let dir = fresh_fixture("xfer-fifo");
        std::fs::create_dir(dir.join("src")).unwrap();
        let made = std::process::Command::new("mkfifo").arg(dir.join("src/pipe")).status();
        assert!(made.unwrap().success());
        let (tx, woke) = mpsc::channel();
        let mut job = Job::start(dir.join("src"), dir.join("dst"), Mode::Copy, false, move || {
            let _ = tx.send(());
        });
        job.cancel();
        // A hang panics after 5 s. Opening the pipe here to unblock a stuck
        // reader would itself block (a FIFO writer waits for a reader).
        let result = wait(&mut job, &woke);
        assert!(result.is_err() || result == Ok(Outcome::Cancelled));
    }

    #[test]
    fn special_files_are_reported_not_opened() {
        let dir = fresh_fixture("xfer-special");
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "A").unwrap();
        let made = std::process::Command::new("mkfifo").arg(dir.join("src/pipe")).status();
        assert!(made.unwrap().success());
        let err = run(&dir.join("src"), &dir.join("dst"), Mode::Copy, false, &never(), |_| {})
            .unwrap_err();
        assert!(err.ends_with("pipe: not a regular file, folder or link"), "{err}");
        assert_eq!(read(&dir.join("dst/a.txt")), "A", "what was copied stays");
        assert!(std::fs::symlink_metadata(dir.join("dst/pipe")).is_err());
    }

    #[test]
    fn running_on_a_missing_source_is_an_error_and_creates_nothing() {
        let dir = fresh_fixture("xfer-run-missing");
        let err = run(&dir.join("ghost"), &dir.join("t"), Mode::Copy, false, &never(), |_| {})
            .unwrap_err();
        assert!(err.contains("ghost"), "error names the item: {err}");
        assert!(!dir.join("t").exists());
    }
}
