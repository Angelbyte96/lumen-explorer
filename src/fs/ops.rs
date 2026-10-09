//! File operations started by the user. Deleting goes to the trash; only
//! Shift+Delete, after its own question, deletes for good.

use std::path::{Path, PathBuf};

/// Moves `path` to the trash. Never deletes permanently.
pub fn trash(path: &Path) -> Result<(), String> {
    // `trash` would report a missing file as a generic error; say it plainly.
    if !exists(path) {
        return Err(format!("No longer exists: {}", path.display()));
    }
    trash::delete(path).map_err(|e| format!("Cannot move {} to trash: {e}", path.display()))
}

/// Deletes `path` for good, without the trash. A folder goes with everything
/// in it; a symlink is removed itself, never what it points to.
pub fn delete_forever(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| format!("No longer exists: {}", path.display()))?;
    let result = if meta.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    result.map_err(|e| format!("Cannot delete {}: {e}", path.display()))
}

/// Opens a file with its default app, without waiting for it.
#[cfg(not(windows))]
pub fn open_with_default_app(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))
}

/// Opens a file the way a double-click in File Explorer does. The `open` crate
/// goes through PowerShell, whose zone check asks about every downloaded file,
/// even a PNG; Explorer asks only about types Windows calls dangerous.
#[cfg(windows)]
pub fn open_with_default_app(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{AssocIsDangerous, SEE_MASK_NOZONECHECKS, SHELLEXECUTEINFOW, ShellExecuteExW};
    const SW_SHOWNORMAL: i32 = 1;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let dangerous = assoc_key(path).map(|ext| {
        let ext = wide(ext.as_ref());
        // SAFETY: `ext` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe { AssocIsDangerous(ext.as_ptr()) != 0 }
    });
    let file = wide(path.as_os_str());
    // SAFETY: an all-zero SHELLEXECUTEINFOW is valid (null pointers and handles).
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = if skip_zone_check(dangerous) { SEE_MASK_NOZONECHECKS } else { 0 };
    // No verb: the file's default action, as a double-click runs.
    info.lpFile = file.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    // SAFETY: `info` is sized and filled above; `file` outlives the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if let Some(message) = open_failure(code, path) {
            return Err(message);
        }
    }
    Ok(())
}

/// The message for a failed open, or `None` when the user cancelled Windows'
/// security prompt (ERROR_CANCELLED): File Explorer says nothing then either.
#[cfg_attr(not(windows), allow(dead_code))]
fn open_failure(code: i32, path: &Path) -> Option<String> {
    const ERROR_CANCELLED: i32 = 1223;
    (code != ERROR_CANCELLED)
        .then(|| format!("Cannot open {}: {}", path.display(), std::io::Error::from_raw_os_error(code)))
}

/// ".png" for "photo.png": what Windows looks up to judge a file type.
#[cfg_attr(not(windows), allow(dead_code))]
fn assoc_key(path: &Path) -> Option<String> {
    // Win32 drops a trailing dot or space ("setup.exe." runs setup.exe) and a
    // colon names a stream: judging those by their extension would be wrong.
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(['.', ' ']) || name.contains(':') {
        return None;
    }
    path.extension().map(|ext| format!(".{}", ext.to_string_lossy()))
}

/// Skip Windows' "untrusted source" prompt only when the type is known and safe,
/// as File Explorer does. A file without an extension keeps the check.
#[cfg_attr(not(windows), allow(dead_code))]
fn skip_zone_check(dangerous: Option<bool>) -> bool {
    dangerous == Some(false)
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
        (Some(a), Some(b)) => ids_match(a, b),
        _ => false,
    }
}

/// (volume serial, file index high, low). Some network and user-space
/// filesystems report index 0 for every file: that tells nothing, so it never
/// counts as the same entry (the cautious answer for rename and paste).
#[cfg_attr(not(windows), allow(dead_code))]
fn ids_match(a: (u32, u32, u32), b: (u32, u32, u32)) -> bool {
    let known = |(_, high, low): (u32, u32, u32)| high != 0 || low != 0;
    known(a) && a == b
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

/// Creates a file in `parent`: empty and called "New File", or a copy of
/// `template` under its name. A taken name gets a number ("Report 2.odt");
/// nothing existing is ever overwritten. Returns the created file.
pub fn create_file(parent: &Path, template: Option<&Path>) -> Result<PathBuf, String> {
    use std::io::Write;
    let fail = |e: std::io::Error| format!("Cannot create a file in {}: {e}", parent.display());
    let contents = match template {
        Some(t) => std::fs::read(t).map_err(|e| format!("Cannot read the template {}: {e}", t.display()))?,
        None => Vec::new(),
    };
    let name = template.and_then(|t| t.file_name()).map_or_else(|| NEW_FILE.to_owned(), |n| n.to_string_lossy().into_owned());
    let (stem, ext) = match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => name.split_at(dot),
        None => (name.as_str(), ""),
    };
    for n in 1.. {
        let path = parent.join(if n == 1 { name.clone() } else { format!("{stem} {n}{ext}") });
        // `create_new` fails on a taken name, so there is no check-then-create race.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(&contents) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(fail(e));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fail(e)),
        }
    }
    unreachable!("the name counter is unbounded")
}

pub const NEW_FILE: &str = "New File";

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
    fn delete_forever_removes_a_file_and_a_whole_folder_without_the_trash() {
        let dir = fresh_fixture("delete-forever");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::create_dir_all(dir.join("tree/deep")).unwrap();
        std::fs::write(dir.join("tree/deep/x.txt"), "X").unwrap();
        delete_forever(&dir.join("a.txt")).unwrap();
        delete_forever(&dir.join("tree")).unwrap();
        assert!(!exists(&dir.join("a.txt")) && !exists(&dir.join("tree")));
        assert_eq!(trashed_names_from(&dir), Vec::<String>::new(), "nothing went to the trash");
        assert_eq!(
            delete_forever(&dir.join("a.txt")),
            Err(format!("No longer exists: {}", dir.join("a.txt").display()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn delete_forever_removes_a_link_and_never_what_it_points_to() {
        let dir = fresh_fixture("delete-forever-link");
        std::fs::create_dir(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/keep.txt"), "K").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        delete_forever(&dir.join("link")).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("link")).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("real/keep.txt")).unwrap(), "K");
    }

    #[cfg(unix)]
    #[test]
    fn delete_forever_never_follows_links_inside_a_folder() {
        let dir = fresh_fixture("delete-forever-nested");
        std::fs::create_dir_all(dir.join("outside/deep")).unwrap();
        std::fs::write(dir.join("outside/keep.txt"), "K").unwrap();
        std::fs::write(dir.join("outside/deep/keep2.txt"), "K2").unwrap();
        std::fs::create_dir_all(dir.join("tree/inner")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("tree/link_dir")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("tree/inner/link_dir2")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside/keep.txt"), dir.join("tree/link_file")).unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("tree/dangling")).unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("dangling_top")).unwrap();
        delete_forever(&dir.join("tree")).unwrap();
        delete_forever(&dir.join("dangling_top")).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("tree")).is_err());
        assert!(std::fs::symlink_metadata(dir.join("dangling_top")).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("outside/keep.txt")).unwrap(), "K");
        assert_eq!(std::fs::read_to_string(dir.join("outside/deep/keep2.txt")).unwrap(), "K2");
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

    #[test]
    fn the_type_asked_about_is_the_extension_with_its_dot() {
        assert_eq!(assoc_key(Path::new("C:/d/photo.PNG")), Some(".PNG".into()));
        assert_eq!(assoc_key(Path::new("archive.tar.gz")), Some(".gz".into()));
        assert_eq!(assoc_key(Path::new("Makefile")), None, "no extension");
        assert_eq!(assoc_key(Path::new(".bashrc")), None, "a leading dot is a name, not an extension");
    }

    #[test]
    fn a_name_windows_would_rewrite_keeps_the_zone_check() {
        assert_eq!(assoc_key(Path::new("setup.exe.")), None, "Win32 strips the dot: it runs setup.exe");
        assert_eq!(assoc_key(Path::new("setup.exe ")), None, "Win32 strips the space");
        assert_eq!(assoc_key(Path::new("file.")), None, "no extension keeps the check");
        assert_eq!(assoc_key(Path::new("file.exe:x.png")), None, "a stream path keeps the check");
        assert_eq!(assoc_key(Path::new("photo.png")), Some(".png".into()));
    }

    #[test]
    fn a_cancelled_security_prompt_is_not_an_error() {
        assert_eq!(open_failure(1223, Path::new("a.bat")), None, "ERROR_CANCELLED: the user said no");
        assert_eq!(
            open_failure(2, Path::new("a.bat")),
            Some(format!("Cannot open a.bat: {}", std::io::Error::from_raw_os_error(2))),
        );
    }

    #[test]
    fn the_zone_check_is_skipped_only_for_a_type_known_to_be_safe() {
        assert!(skip_zone_check(Some(false)), "safe type: open like File Explorer");
        assert!(!skip_zone_check(Some(true)), "dangerous type: Windows still warns");
        assert!(!skip_zone_check(None), "no extension: Windows still warns");
    }

    #[test]
    fn a_zero_file_index_is_never_the_same_entry() {
        assert!(ids_match((7, 0, 42), (7, 0, 42)));
        assert!(!ids_match((7, 0, 42), (8, 0, 42)), "another volume");
        assert!(!ids_match((7, 0, 0), (7, 0, 0)), "index 0 is unknown, not a match");
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
    fn new_file_gets_a_free_name_and_never_overwrites() {
        let dir = fresh_fixture("new-file");
        assert_eq!(create_file(&dir, None).unwrap(), dir.join("New File"));
        std::fs::write(dir.join("New File 2"), "KEEP").unwrap();
        assert_eq!(create_file(&dir, None).unwrap(), dir.join("New File 3"));
        assert_eq!(std::fs::read_to_string(dir.join("New File 2")).unwrap(), "KEEP");
        assert_eq!(std::fs::read(dir.join("New File")).unwrap(), b"", "empty");
    }

    #[test]
    fn a_new_file_from_a_template_copies_it_and_numbers_before_the_extension() {
        let dir = fresh_fixture("new-file-template");
        let templates = fresh_fixture("new-file-templates");
        std::fs::write(templates.join("Report.odt"), "TEMPLATE").unwrap();
        let template = templates.join("Report.odt");
        assert_eq!(create_file(&dir, Some(&template)).unwrap(), dir.join("Report.odt"));
        assert_eq!(create_file(&dir, Some(&template)).unwrap(), dir.join("Report 2.odt"));
        assert_eq!(std::fs::read_to_string(dir.join("Report 2.odt")).unwrap(), "TEMPLATE");
        let missing = templates.join("gone.txt");
        assert!(create_file(&dir, Some(&missing)).is_err());
        assert!(!dir.join("gone.txt").exists(), "nothing left behind");
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
