//! The Recycle Bin (Windows), read and emptied as File Explorer does. Each
//! drive keeps `X:\$Recycle.Bin\<user's SID>\` with, per deleted entry, a pair
//! `$I<id><.ext>` (the record: original path, size, time of deletion) and
//! `$R<id><.ext>` (the entry itself, a file or a folder). Nothing here is
//! Windows-only but the SID lookup, so it is tested on Linux with fake bins.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What a `$I` record holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub size: u64,
    /// `None` when the record has no usable time.
    pub deleted: Option<SystemTime>,
    /// The full path the entry had, with the separators Windows wrote.
    pub original: String,
}

/// A recycled entry: its record, its content and what the record says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recycled {
    /// The `$I` file.
    pub info: PathBuf,
    /// The `$R` file or folder.
    pub data: PathBuf,
    pub original: PathBuf,
    /// The name it had.
    pub name: String,
    /// The folder it was in.
    pub location: String,
    pub size: u64,
    pub deleted: Option<SystemTime>,
    pub is_dir: bool,
}

/// What a restore or a permanent delete of several entries did: how many went
/// through, and the first thing that failed (the others are still tried).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub count: usize,
    pub error: Option<String>,
}

/// A record is a 24 byte header (version, size, deletion time), then the path:
/// version 1 (Vista, 7) pads it to 260 UTF-16 characters, version 2 (Windows 10
/// and later) gives its length in characters first, NUL included.
pub fn parse_info(bytes: &[u8]) -> Option<Info> {
    /// Version 1 keeps the path in 260 UTF-16 characters.
    const V1_PATH: usize = 520;
    let word = |at: usize| bytes.get(at..at + 8).and_then(|b| Some(i64::from_le_bytes(b.try_into().ok()?)));
    let (version, size, filetime) = (word(0)?, word(8)?, word(16)?);
    let size = u64::try_from(size).ok()?;
    let path = match version {
        1 => bytes.get(HEADER..).map(|tail| &tail[..tail.len().min(V1_PATH)])?,
        2 => {
            let chars = u32::from_le_bytes(bytes.get(HEADER..HEADER + 4)?.try_into().ok()?) as usize;
            bytes.get(HEADER + 4..HEADER + 4 + chars.checked_mul(2)?)?
        }
        _ => return None,
    };
    let units: Vec<u16> = path.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&unit| unit != 0).collect();
    let original = String::from_utf16_lossy(&units);
    (!original.is_empty()).then(|| Info { size, deleted: filetime_to_time(filetime), original })
}

/// Version, size and time of deletion, 8 bytes each.
const HEADER: usize = 24;

/// 100 ns ticks between 1601 (FILETIME's start) and 1970.
const FILETIME_UNIX: i64 = 116_444_736_000_000_000;

/// A FILETIME as a time; `None` before 1970, which is what a blank one is.
fn filetime_to_time(filetime: i64) -> Option<SystemTime> {
    let ticks = u64::try_from(filetime.checked_sub(FILETIME_UNIX)?).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(ticks / 10_000_000, (ticks % 10_000_000) as u32 * 100))
}

/// The folder and the name of a path as Windows wrote it (either separator).
pub fn split_original(original: &str) -> (&str, &str) {
    match original.rfind(['\\', '/']) {
        Some(at) => (&original[..at], &original[at + 1..]),
        None => ("", original),
    }
}

/// The `$R` name that goes with a `$I` name: `$Iabc.txt` is `$Rabc.txt`.
pub fn data_name(info: &str) -> Option<String> {
    let rest = info.strip_prefix("$I").filter(|rest| !rest.is_empty())?;
    Some(format!("$R{rest}"))
}

/// The longest record read: a path of 32 767 characters takes 64 KiB.
const MAX_RECORD: u64 = 70_000;

/// The recycled entries of one `$Recycle.Bin\<SID>` folder, by record name.
/// A record without its content, or that cannot be read, is left out.
pub fn read_folder(dir: &Path) -> Vec<Recycled> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = read.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
    names.sort();
    let mut items = Vec::new();
    for name in names {
        let Some(data_name) = data_name(&name) else { continue };
        let (info, data) = (dir.join(&name), dir.join(data_name));
        let Ok(content) = std::fs::symlink_metadata(&data) else { continue };
        if std::fs::metadata(&info).is_ok_and(|m| m.len() > MAX_RECORD) {
            continue;
        }
        let Some(record) = std::fs::read(&info).ok().and_then(|bytes| parse_info(&bytes)) else { continue };
        let (location, original_name) = split_original(&record.original);
        if original_name.is_empty() {
            continue;
        }
        items.push(Recycled {
            info,
            data,
            original: PathBuf::from(&record.original),
            name: original_name.to_owned(),
            location: location.to_owned(),
            size: record.size,
            deleted: record.deleted,
            is_dir: content.is_dir(),
        });
    }
    items
}

/// The `$Recycle.Bin\<SID>` folders to read on these drive `roots`: the user's
/// own when the SID is known, else every SID folder that can be opened.
pub fn bin_folders(roots: &[PathBuf], sid: Option<&str>) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    for root in roots {
        let top = root.join("$Recycle.Bin");
        if let Some(sid) = sid {
            let own = top.join(sid);
            if own.is_dir() {
                folders.push(own);
            }
            continue;
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(&top)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().to_ascii_uppercase().starts_with("S-1-"))
            .map(|entry| entry.path())
            .filter(|path| std::fs::read_dir(path).is_ok())
            .collect();
        found.sort();
        folders.extend(found);
    }
    folders
}

/// Every recycled entry of the drives `roots`.
pub fn read_all(roots: &[PathBuf], sid: Option<&str>) -> Vec<Recycled> {
    bin_folders(roots, sid).iter().flat_map(|folder| read_folder(folder)).collect()
}

/// Where `original` goes back to: itself when free, else the same name with
/// " (restored)", then " (restored 2)"... before the extension. `taken` says
/// whether a path is in use.
pub fn free_name(original: &Path, is_dir: bool, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let Some(name) = original.file_name().map(|n| n.to_string_lossy().into_owned()) else { return original.to_path_buf() };
    if !taken(original) {
        return original.to_path_buf();
    }
    // A folder has no extension, and a leading dot is not one.
    let at = if is_dir { None } else { name.rfind('.').filter(|&at| at > 0) };
    let (stem, ext) = at.map_or((name.as_str(), ""), |at| name.split_at(at));
    (1u32..)
        .map(|n| original.with_file_name(if n == 1 { format!("{stem} (restored){ext}") } else { format!("{stem} (restored {n}){ext}") }))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| original.to_path_buf())
}

/// Puts the entry back where it was, in its folder (made if it is gone) and
/// under a free name, then drops its record. Returns where it landed.
pub fn restore(item: &Recycled) -> Result<PathBuf, String> {
    let target = free_name(&item.original, item.is_dir, |path| std::fs::symlink_metadata(path).is_ok());
    let fail = |err: std::io::Error| format!("Cannot restore {}: {err}", item.name);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    std::fs::rename(&item.data, &target).map_err(fail)?;
    // Without its content the record is skipped anyway: nothing to report.
    let _ = std::fs::remove_file(&item.info);
    Ok(target)
}

/// Deletes the entry and its record for good.
pub fn purge(item: &Recycled) -> Result<(), String> {
    if std::fs::symlink_metadata(&item.data).is_ok() {
        crate::fs::ops::delete_forever(&item.data)?;
    }
    match std::fs::remove_file(&item.info) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(format!("Cannot delete {}: {err}", item.info.display())),
        _ => Ok(()),
    }
}

/// Restores `items` in turn.
pub fn restore_all(items: &[&Recycled]) -> Done {
    each(items, |item| restore(item).map(drop))
}

/// Deletes `items` for good in turn.
pub fn purge_all(items: &[&Recycled]) -> Done {
    each(items, purge)
}

fn each(items: &[&Recycled], mut run: impl FnMut(&Recycled) -> Result<(), String>) -> Done {
    let mut done = Done::default();
    for item in items {
        match run(item) {
            Ok(()) => done.count += 1,
            Err(err) => done.error = done.error.or(Some(err)),
        }
    }
    done
}

/// "1 item restored", "3 items restored".
pub fn restored_notice(count: usize) -> String {
    format!("{count} {} restored", if count == 1 { "item" } else { "items" })
}

/// Reads the bins on a thread of their own: a bin with thousands of entries
/// is thousands of small files, which would stall the window.
#[derive(Default)]
pub struct Reader {
    receiver: Option<std::sync::mpsc::Receiver<Vec<Recycled>>>,
}

impl Reader {
    /// Whether a read is under way.
    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    /// Runs `read` on a thread and calls `wake` when it is done. Does nothing
    /// while a read is under way.
    pub fn start(&mut self, wake: impl FnOnce() + Send + 'static, read: impl FnOnce() -> Vec<Recycled> + Send + 'static) {
        if self.receiver.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(read());
            wake();
        });
    }

    /// What the read found, once, when it is done.
    pub fn poll(&mut self) -> Option<Vec<Recycled>> {
        match self.receiver.as_ref()?.try_recv() {
            Ok(rows) => {
                self.receiver = None;
                Some(rows)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            // The thread is gone without an answer: read again next time.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                None
            }
        }
    }
}

/// The SID of the user Lumen runs as ("S-1-5-21-..."), which names its folder
/// in every `$Recycle.Bin`.
#[cfg(windows)]
pub fn current_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo handle of this process needs no closing; `token` is a valid out-parameter.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let sid = (|| {
        let mut needed = 0u32;
        // SAFETY: asks only for the size; no buffer is passed.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        // Whole u64s, so the buffer is aligned for TOKEN_USER.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buffer` is at least `needed` bytes long.
        if needed == 0 || unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
            return None;
        }
        // SAFETY: the call above filled `buffer` with a TOKEN_USER, whose SID points inside it.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: `sid` is valid while `buffer` lives; `text` is a valid out-parameter.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return None;
        }
        // SAFETY: `text` is a NUL-terminated string allocated for us, freed once read.
        let sid = unsafe {
            let len = (0..).take_while(|&at| *text.add(at) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            sid
        };
        Some(sid)
    })();
    // SAFETY: `token` was opened above and is not used again.
    unsafe { CloseHandle(token) };
    sid
}

#[cfg(not(windows))]
pub fn current_sid() -> Option<String> {
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    /// 2020-01-01 00:00:00 UTC, as seconds since 1970 and as a FILETIME (100 ns since 1601).
    const UNIX_2020: u64 = 1_577_836_800;
    const FILETIME_2020: i64 = (UNIX_2020 * 10_000_000) as i64 + 116_444_736_000_000_000;

    fn at(unix: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(unix))
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn header(version: i64, size: i64, filetime: i64) -> Vec<u8> {
        [version, size, filetime].into_iter().flat_map(i64::to_le_bytes).collect()
    }

    /// A Vista/7 record: the path padded with NULs to 260 characters.
    fn record_v1(size: i64, filetime: i64, path: &str) -> Vec<u8> {
        let mut bytes = header(1, size, filetime);
        let mut text = utf16(path);
        text.resize(520, 0);
        bytes.extend(text);
        bytes
    }

    /// A Windows 10 record: the length in characters (NUL included), then the path and its NUL.
    fn record_v2(size: i64, filetime: i64, path: &str) -> Vec<u8> {
        let mut bytes = header(2, size, filetime);
        let mut text = utf16(path);
        bytes.extend(((text.len() / 2 + 1) as u32).to_le_bytes());
        text.extend([0, 0]);
        bytes.extend(text);
        bytes
    }

    #[test]
    fn a_version_1_record_gives_its_size_time_and_original_path() {
        let info = parse_info(&record_v1(1234, FILETIME_2020, "C:\\Users\\Ana\\Documents\\notes.txt")).unwrap();
        assert_eq!(info, Info { size: 1234, deleted: at(UNIX_2020), original: "C:\\Users\\Ana\\Documents\\notes.txt".into() });
    }

    #[test]
    fn a_version_2_record_gives_the_same_and_reads_non_ascii_names() {
        let path = "C:\\Users\\Ángel\\Imágenes\\fotó 😀.png";
        let info = parse_info(&record_v2(9_000_000_000, FILETIME_2020 + 10_000_000 * 61, path)).unwrap();
        assert_eq!(info, Info { size: 9_000_000_000, deleted: at(UNIX_2020 + 61), original: path.into() });
    }

    #[test]
    fn a_record_that_is_not_one_is_refused() {
        let good = record_v2(5, FILETIME_2020, "C:\\a.txt");
        assert!(parse_info(&good).is_some());
        assert_eq!(parse_info(&[]), None, "empty");
        assert_eq!(parse_info(&good[..20]), None, "no whole header");
        assert_eq!(parse_info(&good[..good.len() - 6]), None, "a version 2 path cut short");
        assert_eq!(parse_info(&record_v2(5, FILETIME_2020, "")), None, "no path");
        assert_eq!(parse_info(&record_v1(5, FILETIME_2020, "")), None, "no path, version 1");
        assert_eq!(parse_info(&record_v1(-1, FILETIME_2020, "C:\\a.txt")), None, "a negative size");
        let mut unknown = good.clone();
        unknown[..8].copy_from_slice(&3i64.to_le_bytes());
        assert_eq!(parse_info(&unknown), None, "version 3");
    }

    #[test]
    fn a_time_before_1970_or_zero_is_unknown_but_the_record_still_reads() {
        for filetime in [0, -5, 116_444_735_999_999_999] {
            let info = parse_info(&record_v2(7, filetime, "C:\\a.txt")).unwrap();
            assert_eq!((info.deleted, info.size, info.original.as_str()), (None, 7, "C:\\a.txt"), "{filetime}");
        }
        assert_eq!(parse_info(&record_v2(7, 116_444_736_000_000_000, "C:\\a.txt")).unwrap().deleted, at(0), "exactly 1970");
    }

    #[test]
    fn a_path_splits_into_its_folder_and_name_at_either_separator() {
        assert_eq!(split_original("C:\\Users\\Ana\\notes.txt"), ("C:\\Users\\Ana", "notes.txt"));
        assert_eq!(split_original("/tmp/fixture/notes.txt"), ("/tmp/fixture", "notes.txt"));
        assert_eq!(split_original("C:\\notes.txt"), ("C:", "notes.txt"), "straight under the drive");
        assert_eq!(split_original("notes.txt"), ("", "notes.txt"));
        assert_eq!(split_original("C:\\Users\\Ana\\"), ("C:\\Users\\Ana", ""), "nothing after the last separator");
    }

    #[test]
    fn a_record_name_points_to_the_content_name() {
        assert_eq!(data_name("$IABC123.txt").as_deref(), Some("$RABC123.txt"));
        assert_eq!(data_name("$IABC123").as_deref(), Some("$RABC123"));
        assert_eq!(data_name("$Iabc.tar.gz").as_deref(), Some("$Rabc.tar.gz"));
        assert_eq!(data_name("$RABC123.txt"), None, "that is a content name");
        assert_eq!(data_name("desktop.ini"), None);
        assert_eq!(data_name("$I"), None, "no id");
    }

    /// Records `original` in the bin `dir` as Windows 10 does: a `$I` and a `$R`
    /// (a file, or a folder with a file in it), sharing the id and the extension.
    fn recycle(dir: &Path, id: &str, original: &str, size: i64, folder: bool) {
        recycle_at(dir, id, original, size, folder, UNIX_2020);
    }

    /// `recycle`, deleted at `unix` seconds since 1970.
    pub(crate) fn recycle_at(dir: &Path, id: &str, original: &str, size: i64, folder: bool, unix: u64) {
        let ext = if folder { String::new() } else { Path::new(original).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default() };
        std::fs::create_dir_all(dir).unwrap();
        let filetime = (unix * 10_000_000) as i64 + 116_444_736_000_000_000;
        std::fs::write(dir.join(format!("$I{id}{ext}")), record_v2(size, filetime, original)).unwrap();
        let data = dir.join(format!("$R{id}{ext}"));
        if folder {
            std::fs::create_dir(&data).unwrap();
            std::fs::write(data.join("inner.txt"), "inner").unwrap();
        } else {
            std::fs::write(&data, "content").unwrap();
        }
    }

    fn bin(dir: &Path) -> PathBuf {
        dir.join("$Recycle.Bin").join("S-1-5-21-1-2-3-1001")
    }

    #[test]
    fn a_bin_folder_lists_each_record_with_its_content_by_original_name() {
        let dir = fresh_fixture("recycle-read-pairs");
        let bin = bin(&dir);
        let (notes, docs) = (dir.join("Users").join("notes.txt"), dir.join("Users").join("Docs"));
        recycle(&bin, "AAA111", &notes.display().to_string(), 7, false);
        recycle(&bin, "BBB222", &docs.display().to_string(), 5, true);
        // No content for this record, no record for this content, a record that is not one.
        std::fs::write(bin.join("$ICCC333.txt"), record_v2(1, FILETIME_2020, "C:\\gone.txt")).unwrap();
        std::fs::write(bin.join("$RDDD444.txt"), "orphan").unwrap();
        std::fs::write(bin.join("$IEEE555.txt"), "garbage").unwrap();
        std::fs::write(bin.join("$REEE555.txt"), "content").unwrap();
        std::fs::write(bin.join("desktop.ini"), "[.ShellClassInfo]").unwrap();
        let items = read_folder(&bin);
        let names: Vec<(&str, bool, u64)> = items.iter().map(|i| (i.name.as_str(), i.is_dir, i.size)).collect();
        assert_eq!(names, [("notes.txt", false, 7), ("Docs", true, 5)], "by record name, nothing else");
        assert_eq!(items[0].original, notes);
        assert_eq!(items[0].location, dir.join("Users").display().to_string());
        assert_eq!((items[0].info.clone(), items[0].data.clone()), (bin.join("$IAAA111.txt"), bin.join("$RAAA111.txt")));
        assert_eq!((items[1].info.clone(), items[1].data.clone()), (bin.join("$IBBB222"), bin.join("$RBBB222")));
        assert_eq!(items[0].deleted, at(UNIX_2020));
    }

    #[test]
    fn a_folder_that_cannot_be_read_lists_nothing() {
        let dir = fresh_fixture("recycle-read-missing");
        assert!(read_folder(&dir.join("nothing here")).is_empty());
    }

    #[test]
    fn the_user_sid_folder_is_read_on_every_drive_that_has_one() {
        let dir = fresh_fixture("recycle-bin-folders-sid");
        let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
        for root in [&c, &d] {
            std::fs::create_dir_all(bin(root)).unwrap();
            std::fs::create_dir_all(root.join("$Recycle.Bin").join("S-1-5-21-9-9-9-500")).unwrap();
        }
        std::fs::create_dir_all(&e).unwrap();
        let roots = [c.clone(), d.clone(), e];
        assert_eq!(bin_folders(&roots, Some("S-1-5-21-1-2-3-1001")), [bin(&c), bin(&d)], "someone else's folder is not ours; a drive with no bin has none");
        assert!(bin_folders(&roots, Some("S-1-5-21-0-0-0-1")).is_empty());
    }

    #[test]
    fn without_a_sid_every_sid_folder_is_scanned_and_loose_files_are_not() {
        let dir = fresh_fixture("recycle-bin-folders-scan");
        let root = dir.join("C");
        let top = root.join("$Recycle.Bin");
        std::fs::create_dir_all(top.join("S-1-5-21-1-2-3-1001")).unwrap();
        std::fs::create_dir_all(top.join("S-1-5-18")).unwrap();
        std::fs::create_dir_all(top.join("not-a-sid")).unwrap();
        std::fs::write(top.join("desktop.ini"), "").unwrap();
        assert_eq!(bin_folders(&[root], None), [top.join("S-1-5-18"), top.join("S-1-5-21-1-2-3-1001")]);
    }

    #[test]
    fn all_the_drives_entries_are_read_together() {
        let dir = fresh_fixture("recycle-read-all");
        let (c, d) = (dir.join("C"), dir.join("D"));
        recycle(&bin(&c), "AAA111", "C:\\Users\\a.txt", 1, false);
        recycle(&bin(&d), "BBB222", "D:\\Work\\b.txt", 2, false);
        let items = read_all(&[c, d], Some("S-1-5-21-1-2-3-1001"));
        let found: Vec<(&str, &str)> = items.iter().map(|i| (i.name.as_str(), i.location.as_str())).collect();
        assert_eq!(found, [("a.txt", "C:\\Users"), ("b.txt", "D:\\Work")]);
        assert!(read_all(&[], None).is_empty());
    }

    #[test]
    fn a_taken_name_gets_restored_then_a_number_before_the_extension() {
        let taken = |names: &'static [&'static str]| move |p: &Path| names.iter().any(|n| p == Path::new(n));
        let free = |path: &str, is_dir, used| free_name(Path::new(path), is_dir, used);
        assert_eq!(free("/d/a.txt", false, taken(&[])), PathBuf::from("/d/a.txt"), "free: as it was");
        assert_eq!(free("/d/a.txt", false, taken(&["/d/a.txt"])), PathBuf::from("/d/a (restored).txt"));
        assert_eq!(free("/d/a.txt", false, taken(&["/d/a.txt", "/d/a (restored).txt"])), PathBuf::from("/d/a (restored 2).txt"));
        assert_eq!(free("/d/a.txt", false, taken(&["/d/a.txt", "/d/a (restored).txt", "/d/a (restored 2).txt"])), PathBuf::from("/d/a (restored 3).txt"));
        assert_eq!(free("/d/Docs", true, taken(&["/d/Docs"])), PathBuf::from("/d/Docs (restored)"));
        assert_eq!(free("/d/archive.tar.gz", false, taken(&["/d/archive.tar.gz"])), PathBuf::from("/d/archive.tar (restored).gz"), "only the last extension");
        assert_eq!(free("/d/LICENSE", false, taken(&["/d/LICENSE"])), PathBuf::from("/d/LICENSE (restored)"));
        assert_eq!(free("/d/.gitignore", false, taken(&["/d/.gitignore"])), PathBuf::from("/d/.gitignore (restored)"), "a leading dot is no extension");
        assert_eq!(free("/d/v1.2", true, taken(&["/d/v1.2"])), PathBuf::from("/d/v1.2 (restored)"), "a folder has none");
    }

    /// A bin with one file, `notes.txt`, recycled from `Users` under the fixture; its item.
    fn one_file(name: &str) -> (PathBuf, Recycled) {
        let dir = fresh_fixture(name);
        let bin = bin(&dir);
        let original = dir.join("Users").join("notes.txt");
        recycle(&bin, "AAA111", &original.display().to_string(), 7, false);
        let item = read_folder(&bin).remove(0);
        (dir, item)
    }

    #[test]
    fn restoring_moves_the_content_back_makes_the_folder_and_drops_the_record() {
        let (dir, item) = one_file("recycle-restore-file");
        assert!(!dir.join("Users").exists(), "its folder is gone");
        assert_eq!(restore(&item), Ok(dir.join("Users").join("notes.txt")));
        assert_eq!(std::fs::read_to_string(dir.join("Users").join("notes.txt")).unwrap(), "content");
        assert!(!item.data.exists() && !item.info.exists(), "neither the content nor the record stay in the bin");
    }

    #[test]
    fn restoring_a_folder_brings_back_what_it_held() {
        let dir = fresh_fixture("recycle-restore-folder");
        let original = dir.join("Work").join("Docs");
        recycle(&bin(&dir), "BBB222", &original.display().to_string(), 5, true);
        let item = read_folder(&bin(&dir)).remove(0);
        assert_eq!(restore(&item), Ok(original.clone()));
        assert_eq!(std::fs::read_to_string(original.join("inner.txt")).unwrap(), "inner");
        assert!(!item.data.exists() && !item.info.exists());
    }

    #[test]
    fn restoring_onto_a_taken_name_keeps_both() {
        let (dir, item) = one_file("recycle-restore-taken");
        std::fs::create_dir_all(dir.join("Users")).unwrap();
        std::fs::write(dir.join("Users").join("notes.txt"), "newer").unwrap();
        assert_eq!(restore(&item), Ok(dir.join("Users").join("notes (restored).txt")));
        assert_eq!(std::fs::read_to_string(dir.join("Users").join("notes.txt")).unwrap(), "newer", "the one in the way is untouched");
        assert_eq!(std::fs::read_to_string(dir.join("Users").join("notes (restored).txt")).unwrap(), "content");
        assert!(!item.info.exists());
    }

    #[test]
    fn a_restore_that_fails_says_so_and_leaves_the_bin_whole() {
        let (dir, item) = one_file("recycle-restore-fails");
        // Its folder cannot be made: a file holds its place.
        std::fs::write(dir.join("Users"), "a file").unwrap();
        let err = restore(&item).unwrap_err();
        assert!(err.starts_with("Cannot restore notes.txt: "), "{err}");
        assert!(item.data.exists() && item.info.exists(), "still in the bin");
    }

    #[test]
    fn deleting_for_good_removes_the_content_and_the_record() {
        let (_, item) = one_file("recycle-purge-file");
        assert_eq!(purge(&item), Ok(()));
        assert!(!item.data.exists() && !item.info.exists());
        let dir = fresh_fixture("recycle-purge-folder");
        recycle(&bin(&dir), "BBB222", &dir.join("Docs").display().to_string(), 5, true);
        let folder = read_folder(&bin(&dir)).remove(0);
        assert_eq!(purge(&folder), Ok(()));
        assert!(!folder.data.exists() && !folder.info.exists(), "a folder goes with what it holds");
    }

    #[test]
    fn deleting_for_good_an_entry_whose_content_is_already_gone_still_drops_its_record() {
        let (_, item) = one_file("recycle-purge-gone");
        std::fs::remove_file(&item.data).unwrap();
        assert_eq!(purge(&item), Ok(()));
        assert!(!item.info.exists());
    }

    #[test]
    fn several_entries_are_restored_in_turn_and_a_failure_does_not_stop_the_rest() {
        let dir = fresh_fixture("recycle-restore-all");
        let bin = bin(&dir);
        for (id, name) in [("AAA111", "a.txt"), ("BBB222", "b.txt"), ("CCC333", "c.txt")] {
            recycle(&bin, id, &dir.join("Out").join(name).display().to_string(), 1, false);
        }
        let items = read_folder(&bin);
        // b.txt cannot go back: its folder is a file.
        let blocked = recycle_blocked(&dir, &items[1]);
        let done = restore_all(&[&items[0], &blocked, &items[2]]);
        assert_eq!(done.count, 2);
        assert!(done.error.as_deref().is_some_and(|e| e.starts_with("Cannot restore b.txt: ")), "{:?}", done.error);
        assert!(dir.join("Out").join("a.txt").exists() && dir.join("Out").join("c.txt").exists());
        assert!(blocked.data.exists(), "the failed one stays in the bin");
    }

    /// `item` with another place to go back to, one that cannot be made.
    fn recycle_blocked(dir: &Path, item: &Recycled) -> Recycled {
        let file = dir.join("file");
        std::fs::write(&file, "x").unwrap();
        Recycled { original: file.join("b.txt"), ..item.clone() }
    }

    #[test]
    fn several_entries_are_deleted_in_turn_and_counted() {
        let dir = fresh_fixture("recycle-purge-all");
        let bin = bin(&dir);
        recycle(&bin, "AAA111", "C:\\a.txt", 1, false);
        recycle(&bin, "BBB222", "C:\\b.txt", 1, false);
        let items = read_folder(&bin);
        assert_eq!(purge_all(&[&items[0], &items[1]]), Done { count: 2, error: None });
        assert!(read_folder(&bin).is_empty());
        assert_eq!(purge_all(&[]), Done::default());
    }

    /// Polls until the reader answers, or 10 s pass.
    fn wait_for(reader: &mut Reader) -> Option<Vec<Recycled>> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(rows) = reader.poll() {
                return Some(rows);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn the_reader_reads_on_a_thread_wakes_the_window_and_hands_the_entries_over_once() {
        let dir = fresh_fixture("recycle-reader");
        recycle(&bin(&dir), "AAA111", "C:\\Users\\a.txt", 1, false);
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut reader = Reader::default();
        assert!(!reader.is_running() && reader.poll().is_none(), "idle at first");
        let (flag, root) = (woken.clone(), dir.clone());
        reader.start(move || flag.store(true, std::sync::atomic::Ordering::SeqCst), move || read_all(&[root], Some("S-1-5-21-1-2-3-1001")));
        assert!(reader.is_running());
        let rows = wait_for(&mut reader).expect("the entries arrive");
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["a.txt"]);
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst), "the window was woken");
        assert!(!reader.is_running() && reader.poll().is_none(), "handed over once");
    }

    #[test]
    fn a_second_start_while_reading_is_ignored() {
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let mut reader = Reader::default();
        reader.start(|| {}, move || {
            let _ = hold.recv();
            Vec::new()
        });
        reader.start(|| {}, || panic!("the second read must not run"));
        assert!(reader.is_running() && reader.poll().is_none(), "still reading");
        release.send(()).unwrap();
        assert_eq!(wait_for(&mut reader), Some(Vec::new()));
    }

    #[test]
    fn the_notice_counts_what_was_restored() {
        assert_eq!(restored_notice(1), "1 item restored");
        assert_eq!(restored_notice(2), "2 items restored");
        assert_eq!(restored_notice(12), "12 items restored");
    }

    #[cfg(windows)]
    #[test]
    fn the_current_user_has_a_sid_and_a_recycle_bin_folder_for_it_on_the_system_drive() {
        let sid = current_sid().expect("a SID");
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        let root = std::env::var("SystemDrive").map_or_else(|_| PathBuf::from("C:\\"), |d| PathBuf::from(format!("{d}\\")));
        // Only the lookup is tested: the folder exists once something was recycled on the drive.
        let found = bin_folders(&[root], Some(&sid));
        assert!(found.len() <= 1, "{found:?}");
    }
}
