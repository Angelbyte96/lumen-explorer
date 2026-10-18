//! The document portal's file transfer, for the files of a drag out.
//!
//! An app in a sandbox (Flatpak) cannot read the host paths of a
//! `text/uri-list`. The way GTK and Nautilus hand it files is the portal
//! `org.freedesktop.portal.Documents`: the dragging app opens a transfer, adds
//! one descriptor per file and gives the receiver only the transfer's key, as
//! `application/vnd.portal.filetransfer` (and `application/vnd.portal.files`,
//! the name GTK 4.6 used by mistake and old Flatpaks still ask for). The
//! receiver turns the key into files it can read.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use zbus::blocking::Connection;
use zbus::zvariant::{Fd, Value};

/// The kind of data that carries a transfer's key.
pub const FILE_TRANSFER: &str = "application/vnd.portal.filetransfer";
/// The same, under the name GTK 4.6 gave it by mistake; Firefox asks for it first.
pub const FILES: &str = "application/vnd.portal.files";

/// Whether `mime` is one of the kinds that carry a transfer's key.
pub fn is_portal(mime: &str) -> bool {
    mime == FILE_TRANSFER || mime == FILES
}

/// What is written to the receiver for `key`: the key and a terminating NUL,
/// as GTK writes it and reads it back (`strlen (key) + 1`).
pub fn framed(key: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(key.len() + 1);
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out
}

type Start = Box<dyn Fn(&[PathBuf]) -> Result<String, String> + Send + Sync>;

/// The portal transfers of one drag out: one for each request of a portal
/// kind, as GTK opens them. A transfer stops once its files are retrieved
/// (autostop), and an app asks again when the drag comes back to it.
pub struct Transfer {
    start: Start,
    /// The portal failed once: not asked again for this drag.
    failed: AtomicBool,
}

impl Transfer {
    /// A transfer that opens itself through the session bus.
    pub fn new() -> Self {
        Self::with(Box::new(start))
    }

    pub fn with(start: Start) -> Self {
        Transfer { start, failed: AtomicBool::new(false) }
    }

    /// The key of a new transfer of `paths`. A failure is told once on stderr
    /// and not tried again for this drag.
    pub fn key(&self, paths: &[PathBuf]) -> Option<String> {
        if self.failed.load(Ordering::Relaxed) {
            return None;
        }
        match (self.start)(paths) {
            Ok(key) => Some(key),
            Err(err) => {
                if !self.failed.swap(true, Ordering::Relaxed) {
                    eprintln!("[lumen] the document portal did not take the dragged files: {err}");
                }
                None
            }
        }
    }
}

const DESTINATION: &str = "org.freedesktop.portal.Documents";
const PATH: &str = "/org/freedesktop/portal/documents";
const INTERFACE: &str = "org.freedesktop.portal.FileTransfer";
/// GTK adds files in chunks of 16 "to avoid running into the per-message fd
/// limit of the bus" (gdk/filetransferportal.c).
const FDS_PER_CALL: usize = 16;

/// The connection to the session bus that opens the transfers, kept for the
/// life of the process once it is opened. The portal ties a transfer to the
/// bus connection that started it and drops it when that connection goes, and
/// the receiver may come for the files long after the frame that began the
/// drag, so the connection must outlive the drag; one shared by every drag
/// costs one socket and zbus's own thread, only for a process that has dragged
/// files out. Dropped when a call fails, so the next drag opens a new one.
static BUS: Mutex<Option<Connection>> = Mutex::new(None);

fn bus() -> Result<Connection, String> {
    let mut bus = BUS.lock().map_err(|_| "poisoned")?;
    if bus.is_none() {
        *bus = Some(Connection::session().map_err(|err| format!("no session bus: {err}"))?);
    }
    Ok(bus.clone().expect("just set"))
}

/// Opens a transfer of `paths` in the document portal and returns its key.
/// Blocks on the bus: only ever called from a thread of the drag out's own.
fn start(paths: &[PathBuf]) -> Result<String, String> {
    let conn = bus()?;
    let result = add_files(&conn, paths);
    if result.is_err() {
        *BUS.lock().map_err(|_| "poisoned")? = None;
    }
    result
}

fn add_files(conn: &Connection, paths: &[PathBuf]) -> Result<String, String> {
    // A path that is not absolute names no file for the portal either.
    let paths: Vec<&PathBuf> = paths.iter().filter(|path| path.as_os_str().as_bytes().first() == Some(&b'/')).collect();
    if paths.is_empty() {
        return Err("no file to hand over".into());
    }
    // O_PATH: the portal only needs to name the file, and a file that cannot
    // be read can still be handed over. Opened before the transfer starts, so
    // a vanished file leaves no transfer behind.
    let files: Vec<File> = paths
        .iter()
        .map(|path| OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_CLOEXEC).open(path).map_err(|err| format!("{}: {err}", path.display())))
        .collect::<Result<_, _>>()?;
    let options = HashMap::from([("writable", Value::from(false)), ("autostop", Value::from(true))]);
    let reply = conn
        .call_method(Some(DESTINATION), PATH, Some(INTERFACE), "StartTransfer", &(options,))
        .map_err(|err| format!("StartTransfer: {err}"))?;
    let key: String = reply.body().deserialize().map_err(|err| format!("StartTransfer: {err}"))?;
    for chunk in files.chunks(FDS_PER_CALL) {
        let fds: Vec<Fd> = chunk.iter().map(Fd::from).collect();
        let none: HashMap<&str, Value> = HashMap::new();
        conn.call_method(Some(DESTINATION), PATH, Some(INTERFACE), "AddFiles", &(key.as_str(), fds, none)).map_err(|err| format!("AddFiles: {err}"))?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Seen = Arc<Mutex<Vec<Vec<PathBuf>>>>;

    /// A transfer whose portal answers `result`, an `Ok` key numbered by the call ("key1", "key2"...).
    fn counting(result: Result<String, String>) -> (Transfer, Arc<AtomicUsize>, Seen) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let transfer = {
            let (calls, seen) = (calls.clone(), seen.clone());
            Transfer::with(Box::new(move |paths| {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                seen.lock().unwrap().push(paths.to_vec());
                result.clone().map(|key| format!("{key}{call}"))
            }))
        };
        (transfer, calls, seen)
    }

    /// Needs a session bus with the document portal running, so it is not part
    /// of a plain `cargo test`: `cargo test portal -- --ignored`. Another
    /// connection (as the receiving app's) turns the key back into files.
    #[test]
    #[ignore = "needs the session bus and the document portal"]
    fn the_real_portal_hands_the_files_to_another_connection() {
        let dir = std::env::temp_dir().join(format!("lumen-portal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..20).map(|n| dir.join(format!("file {n}.txt"))).collect();
        for file in &files {
            std::fs::write(file, b"hello").unwrap();
        }
        let key = start(&files);
        let received: Result<Vec<String>, String> = key.clone().and_then(|key| {
            let receiver = Connection::session().map_err(|err| err.to_string())?;
            let options: HashMap<&str, Value> = HashMap::new();
            let reply = receiver
                .call_method(Some(DESTINATION), PATH, Some(INTERFACE), "RetrieveFiles", &(key.as_str(), options))
                .map_err(|err| err.to_string())?;
            reply.body().deserialize::<Vec<String>>().map_err(|err| err.to_string())
        });
        std::fs::remove_dir_all(&dir).unwrap();
        let key = key.expect("StartTransfer and AddFiles");
        assert!(!key.is_empty() && !key.contains('\0'), "{key:?}");
        let received = received.expect("RetrieveFiles");
        assert_eq!(received.len(), 20, "{received:?}");
        eprintln!("portal key {key:?}, first file {:?}", received[0]);
    }

    #[test]
    fn the_key_is_written_with_its_terminating_nul() {
        assert_eq!(framed("1c7f3a"), b"1c7f3a\0");
        assert_eq!(framed(""), b"\0");
    }

    #[test]
    fn both_portal_kinds_are_recognised_and_nothing_else() {
        assert!(is_portal("application/vnd.portal.filetransfer"));
        assert!(is_portal("application/vnd.portal.files"));
        for other in ["text/uri-list", "text/plain", "application/vnd.portal.file", ""] {
            assert!(!is_portal(other), "{other}");
        }
    }

    #[test]
    fn every_request_of_a_drag_opens_a_transfer_of_its_own() {
        // A transfer stops once its files are retrieved (autostop), and an app
        // asks again when the drag comes back to it (Zen after Alt+Tab, L47).
        let paths = vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b c")];
        let (transfer, calls, seen) = counting(Ok("key".into()));
        assert_eq!(transfer.key(&paths).as_deref(), Some("key1"));
        assert_eq!(transfer.key(&paths).as_deref(), Some("key2"));
        assert_eq!(transfer.key(&paths).as_deref(), Some("key3"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(*seen.lock().unwrap(), vec![paths.clone(), paths.clone(), paths], "each with the dragged paths");
    }

    #[test]
    fn a_portal_that_fails_gives_no_key_and_is_not_asked_again() {
        let paths = vec![PathBuf::from("/tmp/a")];
        let (transfer, calls, _) = counting(Err("no session bus".into()));
        assert_eq!(transfer.key(&paths), None);
        assert_eq!(transfer.key(&paths), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn requests_at_the_same_time_get_a_transfer_each() {
        let paths = vec![PathBuf::from("/tmp/a")];
        let (transfer, calls, _) = counting(Ok("k".into()));
        let transfer = Arc::new(transfer);
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (transfer, paths) = (transfer.clone(), paths.clone());
                std::thread::spawn(move || transfer.key(&paths))
            })
            .collect();
        let mut keys: Vec<String> = threads.into_iter().map(|thread| thread.join().unwrap().expect("a key")).collect();
        keys.sort();
        assert_eq!(keys, ["k1", "k2", "k3", "k4"]);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }
}
