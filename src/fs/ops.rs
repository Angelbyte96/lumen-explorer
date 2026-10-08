//! File operations started by the user. Deleting always goes to the trash.

use std::path::{Path, PathBuf};

/// Moves `path` to the trash. Never deletes permanently.
pub fn trash(path: &Path) -> Result<(), String> {
    // `trash` would report a missing file as a generic error; say it plainly.
    if !exists(path) {
        return Err(format!("No longer exists: {}", path.display()));
    }
    trash::delete(path).map_err(|e| format!("Cannot move {} to trash: {e}", path.display()))
}

/// Checks a name typed by the user for a new or renamed entry.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        Err("Name cannot be empty".into())
    } else if name.contains('/') {
        Err("Name cannot contain “/”".into())
    } else if name.contains('\0') {
        Err("Name cannot contain a NUL character".into())
    } else if name == "." || name == ".." {
        Err(format!("“{name}” is not a valid name"))
    } else {
        Ok(())
    }
}

/// On case-insensitive filesystems (vfat/exFAT USB drives) "a.txt" -> "A.txt"
/// finds the source itself at the target; trashing it would remove what we rename.
fn is_case_change_of_same_entry(from: &Path, to: &Path, old: &str, new: &str) -> bool {
    old.to_lowercase() == new.to_lowercase() && same_entry(from, to)
}

/// Same device and inode: one entry reached through two paths.
#[cfg(unix)]
pub(crate) fn same_entry(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Same volume and file index, read without following a link (like
/// `symlink_metadata` on Unix).
#[cfg(windows)]
pub(crate) fn same_entry(a: &Path, b: &Path) -> bool {
    match (file_id(a), file_id(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(windows)]
fn file_id(path: &Path) -> Option<(u32, u32, u32)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    };
    // No access rights: enough to read the identity, and folders open too.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    // SAFETY: the handle stays open while `file` lives; `info` is a properly
    // sized out-parameter filled on success.
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) == 0 {
            return None;
        }
        info
    };
    Some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn same_entry(_: &Path, _: &Path) -> bool {
    false
}

pub(crate) fn exists(path: &Path) -> bool {
    // Does not follow symlinks: a broken link still occupies the name.
    std::fs::symlink_metadata(path).is_ok()
}

/// What a rename did.
#[derive(Debug, PartialEq, Eq)]
pub enum Renamed {
    /// Done; the entry now lives here.
    To(PathBuf),
    /// The new name equals the old one: nothing to do.
    Unchanged,
    /// Another entry already has that name; ask before calling again with `replace`.
    TargetExists,
}

/// Renames `from` to `new_name` in the same folder. With `replace`, an entry
/// already using that name goes to the trash first; it is never deleted.
pub fn rename(from: &Path, new_name: &str, replace: bool) -> Result<Renamed, String> {
    validate_name(new_name)?;
    if !exists(from) {
        return Err(format!("No longer exists: {}", from.display()));
    }
    // The list shows names lossily, so a non-UTF-8 name submitted as shown is unchanged.
    let old = from.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    if old == new_name {
        return Ok(Renamed::Unchanged);
    }
    let to = from.with_file_name(new_name);
    if exists(&to) && !is_case_change_of_same_entry(from, &to, &old, new_name) {
        if !replace {
            return Ok(Renamed::TargetExists);
        }
        trash(&to)?;
    }
    std::fs::rename(from, &to)
        .map_err(|e| format!("Cannot rename {}: {e}", from.display()))?;
    Ok(Renamed::To(to))
}

/// Name given to a folder created with Ctrl+Shift+N before the user renames it.
pub const NEW_FOLDER: &str = "New Folder";

/// Creates "New Folder" inside `parent`, or "New Folder 2", "New Folder 3"...
/// when the name is taken. Returns the created folder.
pub fn create_folder(parent: &Path) -> Result<PathBuf, String> {
    for n in 1.. {
        let name = if n == 1 { NEW_FOLDER.to_owned() } else { format!("{NEW_FOLDER} {n}") };
        let path = parent.join(name);
        // `create_dir` fails on a taken name, so there is no check-then-create race.
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!("Cannot create a folder in {}: {e}", parent.display()));
            }
        }
    }
    unreachable!("the name counter is unbounded")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An empty fixture folder under ~/pruebas-explorador (project safety rule).
    /// Unlike the lister fixtures these are wiped first: the operations change them.
    pub(crate) fn fresh_fixture(name: &str) -> PathBuf {
        let dir = crate::fs::home_dir()
            .expect("HOME must be set")
            .join("pruebas-explorador")
            .join("test-fixtures")
            .join("ops")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Removes this fixture's items from the trash so test runs do not pile up there.
    pub(crate) fn purge_trashed_from(dir: &Path) {
        let items: Vec<_> = trash::os_limited::list()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.original_parent == dir)
            .collect();
        let _ = trash::os_limited::purge_all(items);
    }

    /// Names of this fixture's items in the trash, sorted, as `as_trashed` gives them.
    pub(crate) fn trashed_names_from(dir: &Path) -> Vec<String> {
        let items = trash::os_limited::list().unwrap_or_default();
        let names = items.iter().filter(|item| item.original_parent == dir).map(|item| item.name.to_string_lossy());
        as_trashed(&names.collect::<Vec<_>>())
    }

    /// `names` sorted and, on Windows, without extensions: the Recycle Bin
    /// reports names the way Explorer shows them, which may hide extensions.
    pub(crate) fn as_trashed<S: AsRef<str>>(names: &[S]) -> Vec<String> {
        let mut names: Vec<String> = names
            .iter()
            .map(|name| {
                let name = name.as_ref();
                if cfg!(windows) {
                    Path::new(name).file_stem().map_or(name.into(), |s| s.to_string_lossy().into_owned())
                } else {
                    name.to_owned()
                }
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn trash_moves_file_and_folder_to_the_trash() {
        let dir = fresh_fixture("trash");
        std::fs::write(dir.join("old.txt"), "x").unwrap();
        std::fs::create_dir(dir.join("old_dir")).unwrap();

        trash(&dir.join("old.txt")).unwrap();
        trash(&dir.join("old_dir")).unwrap();

        assert!(!dir.join("old.txt").exists());
        assert!(!dir.join("old_dir").exists());
        assert_eq!(trashed_names_from(&dir), as_trashed(&["old.txt", "old_dir"]), "recoverable, not deleted");
        purge_trashed_from(&dir);
    }

    #[test]
    fn trashing_a_missing_path_is_an_error() {
        let dir = fresh_fixture("trash-missing");
        let err = trash(&dir.join("ghost.txt")).unwrap_err();
        assert!(err.contains("ghost.txt"), "error names the item: {err}");
    }

    #[test]
    fn invalid_names_are_rejected_with_a_reason() {
        for bad in ["", "   ", "a/b", "/", ".", "..", "a\0b"] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be invalid");
        }
        assert_eq!(validate_name("").unwrap_err(), "Name cannot be empty");
        assert_eq!(validate_name("a/b").unwrap_err(), "Name cannot contain “/”");
        for good in ["notes.txt", ".hidden", " spaced name ", "ñandú", "a..b"] {
            assert_eq!(validate_name(good), Ok(()), "{good:?} should be valid");
        }
    }

    #[test]
    fn rename_moves_the_entry_within_its_folder() {
        let dir = fresh_fixture("rename");
        std::fs::write(dir.join("old.txt"), "body").unwrap();
        let done = rename(&dir.join("old.txt"), "new.txt", false).unwrap();
        assert_eq!(done, Renamed::To(dir.join("new.txt")));
        assert!(!dir.join("old.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("new.txt")).unwrap(), "body");
    }

    #[test]
    fn renaming_to_the_same_name_does_nothing() {
        let dir = fresh_fixture("rename-same");
        std::fs::write(dir.join("same.txt"), "body").unwrap();
        assert_eq!(rename(&dir.join("same.txt"), "same.txt", false), Ok(Renamed::Unchanged));
        assert_eq!(std::fs::read_to_string(dir.join("same.txt")).unwrap(), "body");
    }

    #[test]
    fn invalid_name_leaves_everything_untouched() {
        let dir = fresh_fixture("rename-invalid");
        std::fs::write(dir.join("keep.txt"), "body").unwrap();
        let err = rename(&dir.join("keep.txt"), "a/b", false).unwrap_err();
        assert_eq!(err, "Name cannot contain “/”");
        assert_eq!(std::fs::read_to_string(dir.join("keep.txt")).unwrap(), "body");
    }

    #[test]
    fn existing_target_needs_confirmation_and_nothing_moves() {
        let dir = fresh_fixture("rename-exists");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        assert_eq!(rename(&dir.join("a.txt"), "b.txt", false), Ok(Renamed::TargetExists));
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "B");
    }

    #[test]
    fn replacing_sends_the_old_target_to_the_trash() {
        let dir = fresh_fixture("rename-replace");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        let done = rename(&dir.join("a.txt"), "b.txt", true).unwrap();
        assert_eq!(done, Renamed::To(dir.join("b.txt")));
        assert!(!dir.join("a.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "A");
        assert_eq!(trashed_names_from(&dir), as_trashed(&["b.txt"]), "old b.txt is recoverable");
        purge_trashed_from(&dir);
    }

    #[test]
    fn same_entry_tells_one_entry_through_two_paths_from_two_entries() {
        let dir = fresh_fixture("same-entry");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        assert!(same_entry(&dir.join("a.txt"), &dir.join("sub").join("..").join("a.txt")));
        assert!(same_entry(&dir.join("sub"), &dir.join("sub")), "folders too");
        assert!(!same_entry(&dir.join("a.txt"), &dir.join("b.txt")));
        assert!(!same_entry(&dir.join("a.txt"), &dir.join("ghost.txt")), "a missing entry is never the same");
    }

    /// NTFS ignores case: "A.txt" already exists, and it is "a.txt" itself.
    #[cfg(windows)]
    #[test]
    fn a_case_only_rename_keeps_the_file_and_trashes_nothing() {
        let dir = fresh_fixture("rename-case");
        std::fs::write(dir.join("a.txt"), "body").unwrap();
        assert_eq!(rename(&dir.join("a.txt"), "A.txt", false), Ok(Renamed::To(dir.join("A.txt"))));
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["A.txt"], "the new case is on disk");
        assert_eq!(std::fs::read_to_string(dir.join("A.txt")).unwrap(), "body");
        assert!(trashed_names_from(&dir).is_empty(), "nothing went to the Recycle Bin");
    }

    #[test]
    fn renaming_a_missing_entry_is_an_error() {
        let dir = fresh_fixture("rename-missing");
        let err = rename(&dir.join("ghost.txt"), "x.txt", false).unwrap_err();
        assert!(err.contains("ghost.txt"), "error names the item: {err}");
        assert!(!dir.join("x.txt").exists());
    }

    #[test]
    fn new_folder_gets_a_free_numbered_name() {
        let dir = fresh_fixture("new-folder");
        assert_eq!(create_folder(&dir).unwrap(), dir.join("New Folder"));
        assert_eq!(create_folder(&dir).unwrap(), dir.join("New Folder 2"));
        // A file also takes the name.
        std::fs::write(dir.join("New Folder 3"), "").unwrap();
        assert_eq!(create_folder(&dir).unwrap(), dir.join("New Folder 4"));
        assert!(dir.join("New Folder").is_dir() && dir.join("New Folder 4").is_dir());
        assert!(dir.join("New Folder 3").is_file(), "existing file untouched");
    }

    #[test]
    fn new_folder_in_a_missing_parent_is_an_error() {
        let dir = fresh_fixture("new-folder-missing");
        let err = create_folder(&dir.join("gone")).unwrap_err();
        assert!(err.contains("gone"), "error names the folder: {err}");
        assert!(!dir.join("gone").exists());
    }

    #[test]
    #[cfg(unix)]
    fn trashing_a_symlink_keeps_its_target() {
        let dir = fresh_fixture("trash-symlink");
        std::fs::write(dir.join("real.txt"), "R").unwrap();
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("link")).unwrap();
        trash(&dir.join("link")).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("link")).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("real.txt")).unwrap(), "R");
        purge_trashed_from(&dir);
    }

    #[test]
    fn replacing_a_folder_trashes_it_whole() {
        let dir = fresh_fixture("rename-replace-dir");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/s"), "S").unwrap();
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        std::fs::write(dir.join("dst/k"), "K").unwrap();
        assert_eq!(rename(&dir.join("src"), "dst", false), Ok(Renamed::TargetExists));
        rename(&dir.join("src"), "dst", true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("dst/s")).unwrap(), "S");
        assert!(!dir.join("dst/k").exists());
        assert_eq!(trashed_names_from(&dir), as_trashed(&["dst"]));
        purge_trashed_from(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn keeping_a_non_utf8_name_as_shown_is_a_no_op() {
        use std::os::unix::ffi::OsStrExt;
        let dir = fresh_fixture("rename-latin1");
        let raw = dir.join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
        std::fs::write(&raw, "C").unwrap();
        // The list shows the lossy name; F2 + Enter submits exactly that.
        let shown = raw.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(rename(&raw, &shown, false), Ok(Renamed::Unchanged));
        assert_eq!(std::fs::read_to_string(&raw).unwrap(), "C");
        assert!(!dir.join(&shown).exists());
    }

    #[test]
    fn a_hard_link_is_a_different_entry_and_is_replaced_via_trash() {
        let dir = fresh_fixture("rename-hardlink");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::hard_link(dir.join("a.txt"), dir.join("b.txt")).unwrap();
        assert_eq!(rename(&dir.join("a.txt"), "b.txt", false), Ok(Renamed::TargetExists));
        assert_eq!(rename(&dir.join("a.txt"), "b.txt", true), Ok(Renamed::To(dir.join("b.txt"))));
        assert!(!dir.join("a.txt").exists(), "rename(2) between hard links would keep both");
        assert_eq!(trashed_names_from(&dir), as_trashed(&["b.txt"]));
        purge_trashed_from(&dir);
    }
}
