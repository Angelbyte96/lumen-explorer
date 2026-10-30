//! `org.freedesktop.FileManager1` on the session bus: what browsers and
//! editors call to show a downloaded file "in its folder" (`ShowItems`),
//! instead of opening `inode/directory`'s default app. Lumen Setup's "Set as
//! default" installs a D-Bus service file that starts `lumen --file-manager1`
//! when the name is asked for: it takes the name, starts one Lumen window per
//! call and leaves after a few idle seconds, when D-Bus starts it again.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

/// The flag the D-Bus service file starts Lumen with.
pub const FLAG: &str = "--file-manager1";
/// The bus name and the object path the interface is served at.
pub const NAME: &str = "org.freedesktop.FileManager1";
#[cfg(target_os = "linux")]
const PATH: &str = "/org/freedesktop/FileManager1";
/// How long the service waits for another call before it leaves.
#[cfg(target_os = "linux")]
const IDLE: Duration = Duration::from_secs(10);

/// One Lumen window to start: `folder`, with the entry `select` selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub folder: PathBuf,
    pub select: Option<String>,
}

impl Launch {
    /// The command line after the program name.
    pub fn args(&self) -> Vec<std::ffi::OsString> {
        let mut args = vec![self.folder.clone().into_os_string()];
        if let Some(name) = &self.select {
            args.push(format!("{}{name}", crate::app::SELECT).into());
        }
        args
    }
}

/// The path of a local `file://` URI (`file:///p` or `file://localhost/p`),
/// percent-decoded; `None` for anything else.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
            out.push(u8::from_str_radix(hex?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(OsStr::from_bytes(&out)))
}

/// The windows `ShowFolders` starts: one per folder, in order, without repeats.
pub fn show_folders(uris: &[String]) -> Vec<Launch> {
    let mut launches: Vec<Launch> = Vec::new();
    for folder in uris.iter().filter_map(|uri| uri_to_path(uri)) {
        if !launches.iter().any(|l| l.folder == folder) {
            launches.push(Launch { folder, select: None });
        }
    }
    launches
}

/// The windows `ShowItems` starts: one per folder the items are in, with the
/// first item there selected. The root has no folder: it opens itself.
pub fn show_items(uris: &[String]) -> Vec<Launch> {
    let mut launches: Vec<Launch> = Vec::new();
    for item in uris.iter().filter_map(|uri| uri_to_path(uri)) {
        let launch = match (item.parent(), item.file_name().and_then(OsStr::to_str)) {
            (Some(folder), Some(name)) => Launch { folder: folder.to_path_buf(), select: Some(name.to_owned()) },
            _ => Launch { folder: item.clone(), select: None },
        };
        if !launches.iter().any(|l| l.folder == launch.folder) {
            launches.push(launch);
        }
    }
    launches
}

#[cfg(target_os = "linux")]
struct FileManager1 {
    exe: PathBuf,
    last_call: Arc<Mutex<Instant>>,
}

#[cfg(target_os = "linux")]
impl FileManager1 {
    /// Starts every window of `launches`; `startup_id` lets the desktop give
    /// the new window focus (X11 startup notification and Wayland activation).
    fn start(&self, launches: Vec<Launch>, startup_id: &str) {
        *self.last_call.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        for launch in launches {
            let mut command = std::process::Command::new(&self.exe);
            command.args(launch.args());
            if !startup_id.is_empty() {
                command.env("DESKTOP_STARTUP_ID", startup_id).env("XDG_ACTIVATION_TOKEN", startup_id);
            }
            if let Err(err) = command.spawn() {
                eprintln!("[lumen] cannot start {}: {err}", self.exe.display());
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager1 {
    fn show_folders(&self, uris: Vec<String>, startup_id: &str) {
        self.start(show_folders(&uris), startup_id);
    }

    fn show_items(&self, uris: Vec<String>, startup_id: &str) {
        self.start(show_items(&uris), startup_id);
    }

    /// Lumen has no properties dialog: the items are shown instead.
    fn show_item_properties(&self, uris: Vec<String>, startup_id: &str) {
        self.start(show_items(&uris), startup_id);
    }
}

/// Serves the interface until it has been idle for a while. Fails when the
/// session bus is not there or another file manager holds the name.
#[cfg(target_os = "linux")]
pub fn serve(exe: &Path) -> Result<(), String> {
    let last_call = Arc::new(Mutex::new(Instant::now()));
    let service = FileManager1 { exe: exe.to_path_buf(), last_call: last_call.clone() };
    // zbus asks to replace the owner by default: a file manager already
    // running keeps the name (a second Lumen too, which serves the same calls).
    let _connection = zbus::blocking::connection::Builder::session()
        .map(|b| b.replace_existing_names(false))
        .and_then(|b| b.name(NAME))
        .and_then(|b| b.serve_at(PATH, service))
        .and_then(|b| b.build())
        .map_err(|err| format!("cannot serve {NAME}: {err}"))?;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if last_call.lock().unwrap_or_else(|e| e.into_inner()).elapsed() >= IDLE {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_file_uri_becomes_its_decoded_path_and_other_uris_none() {
        assert_eq!(uri_to_path("file:///home/u/Downloads/a%20b%C3%B1.pdf"), Some(PathBuf::from("/home/u/Downloads/a bñ.pdf")));
        assert_eq!(uri_to_path("file://localhost/tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(uri_to_path("file:///tmp/%ff"), Some(PathBuf::from(OsStr::from_bytes(b"/tmp/\xff"))), "not UTF-8 is still a path");
        assert_eq!(uri_to_path("https://example.com/a"), None);
        assert_eq!(uri_to_path("file://otherhost/tmp/x"), None, "another machine's file");
        assert_eq!(uri_to_path("file:///tmp/%zz"), None, "a broken escape");
        assert_eq!(uri_to_path("file:///tmp/%4"), None, "a cut escape");
    }

    #[test]
    fn show_items_opens_each_items_folder_with_the_first_item_there_selected() {
        let launches = show_items(&uris(&[
            "file:///home/u/Downloads/report.pdf",
            "file:///home/u/Downloads/other.pdf",
            "file:///tmp/x%20y",
            "https://example.com/skipped",
            "file:///",
        ]));
        assert_eq!(
            launches,
            [
                Launch { folder: "/home/u/Downloads".into(), select: Some("report.pdf".into()) },
                Launch { folder: "/tmp".into(), select: Some("x y".into()) },
                Launch { folder: "/".into(), select: None },
            ]
        );
    }

    #[test]
    fn show_folders_opens_each_folder_once_with_nothing_selected() {
        let launches = show_folders(&uris(&["file:///home/u", "file:///tmp", "file:///home/u", "nope"]));
        assert_eq!(launches, [Launch { folder: "/home/u".into(), select: None }, Launch { folder: "/tmp".into(), select: None }]);
    }

    #[test]
    fn a_launch_passes_the_folder_then_the_entry_to_select() {
        let launch = Launch { folder: "/home/u/My Files".into(), select: Some("a b.pdf".into()) };
        assert_eq!(launch.args(), ["/home/u/My Files", "--select=a b.pdf"]);
        assert_eq!(Launch { folder: "/tmp".into(), select: None }.args(), ["/tmp"]);
    }
}
