//! Lumen Setup, like File Pilot's: Lumen installs itself for the current user
//! (no admin rights, no sudo) and four switches tie it into the desktop.
//! Each platform's [`System`] reads what the machine has and changes it; the
//! screen (`ui::setup_view`) only shows that and asks for changes.

#[cfg(not(windows))]
pub mod linux;
#[cfg(windows)]
pub mod windows;

use std::path::{Path, PathBuf};

/// The switches, as the machine has them or as the user wants them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Switches {
    /// A shortcut on the desktop.
    pub desktop_shortcut: bool,
    /// Folders open in Lumen.
    pub default: bool,
    /// "Open in Lumen" in the system file manager's right-click menu.
    pub right_click: bool,
    /// `lumen` runs from a terminal.
    pub path: bool,
}

impl Switches {
    /// What Install starts with, as File Pilot's: everything but the desktop shortcut.
    pub const INSTALL: Self = Self { desktop_shortcut: false, default: true, right_click: true, path: true };
}

/// Where the running Lumen came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// From the system's package manager (the `.deb`'s /usr/bin/lumen): it
    /// owns the files, so Setup neither installs nor uninstalls.
    Package { exe: PathBuf },
    /// Lumen is installed (by Setup) in `record.dir`; `running_it` when this
    /// is that copy, not another one started from elsewhere.
    Installed { record: Record, running_it: bool },
    /// Not installed: Setup offers to install it, by default to `default_dir`.
    NotInstalled { default_dir: PathBuf },
}

/// Windows holds Install back until its releases are code-signed: Defender's
/// machine learning flags an unsigned exe that copies itself into AppData
/// and adds shortcuts (Trojan:Win32/Bearfoos.A!ml), and quarantines it. Setup
/// still opens: it copies a script for the user to run instead, and Uninstall
/// works. Set to false once signed.
pub const INSTALL_HELD: bool = cfg!(windows);

/// What Install remembers, in the file `install` next to the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The folder Lumen was copied to.
    pub dir: PathBuf,
    /// The version copied there.
    pub version: String,
}

impl Record {
    pub fn parse(text: &str) -> Option<Self> {
        let mut dir = None;
        let mut version = None;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("dir", value)) if !value.is_empty() => dir = Some(PathBuf::from(value)),
                Some(("version", value)) => version = Some(value.to_owned()),
                _ => {}
            }
        }
        Some(Self { dir: dir?, version: version.unwrap_or_default() })
    }

    pub fn to_text(&self) -> String {
        format!("dir={}\nversion={}\n", self.dir.display(), self.version)
    }

    pub fn load(path: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(path).ok()?)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, self.to_text().as_bytes())
    }
}

/// The record's file: `install` in the settings folder.
pub fn record_path(config_file: &Path) -> Option<PathBuf> {
    Some(config_file.parent()?.join("install"))
}

/// Which [`Origin`] the running `exe` has: a package's when `packaged`, else
/// installed when the record names a folder that still holds the binary.
pub fn origin(exe: &Path, packaged: bool, record: Option<Record>, binary_name: &str, default_dir: PathBuf) -> Origin {
    if packaged {
        return Origin::Package { exe: exe.to_path_buf() };
    }
    match record.filter(|r| r.dir.join(binary_name).is_file()) {
        Some(record) => {
            let running_it = same_file(&record.dir.join(binary_name), exe);
            Origin::Installed { record, running_it }
        }
        None => Origin::NotInstalled { default_dir },
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// What a platform does for Setup. Errors are messages for the screen.
pub trait System {
    fn origin(&self) -> Origin;
    /// The switches as the machine has them now.
    fn read(&self) -> Switches;
    /// Turns each switch to `want`, leaving the others as they are.
    fn apply(&self, want: Switches) -> Result<(), String>;
    /// Copies the running Lumen into `dir` and turns the switches to `want`.
    fn install(&self, dir: &Path, want: Switches) -> Result<(), String>;
    /// Removes the installed copy, its menu entry and every switch. The
    /// settings (bookmarks, tabs) stay.
    fn uninstall(&self) -> Result<(), String>;
    /// A script for the user to run that does what [`Self::install`] would:
    /// what Setup offers while Install is held ([`INSTALL_HELD`]).
    fn install_script(&self, _dir: &Path, _want: Switches) -> Result<String, String> {
        Err("Lumen installs itself here".into())
    }
    /// A script for the user to run that does what [`Self::apply`] would.
    fn apply_script(&self, _want: Switches) -> Result<String, String> {
        Err("Lumen changes the switches itself here".into())
    }
}

/// Writes `bytes` to `path` through a temporary file beside it, so a failed
/// write leaves the old file. A symlink at `path` is followed: its target is
/// replaced, not the link (~/.config/mimeapps.list is often one into dotfiles).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.clone().into_os_string();
    tmp.push(".lumen-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Removes `path`; a file that is already gone is no error.
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Copies the running binary to `dir/name` through a temporary file, so a
/// failed copy leaves an older install whole.
pub fn copy_binary(exe: &Path, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let to = dir.join(name);
    if same_file(exe, &to) {
        return Ok(to);
    }
    let tmp = dir.join(format!("{name}.lumen-tmp"));
    std::fs::copy(exe, &tmp)?;
    std::fs::rename(&tmp, &to).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    #[test]
    fn a_record_reads_back_what_it_wrote() {
        let record = Record { dir: PathBuf::from("/home/me/.local/share/lumen"), version: "0.7.0".into() };
        assert_eq!(Record::parse(&record.to_text()), Some(record));
    }

    #[test]
    fn a_record_without_a_folder_is_no_record() {
        assert_eq!(Record::parse("version=0.7.0\n"), None);
        assert_eq!(Record::parse("dir=\nversion=0.7.0\n"), None);
        assert_eq!(Record::parse(""), None);
    }

    #[test]
    fn the_record_sits_next_to_the_settings() {
        assert_eq!(record_path(Path::new("/home/me/.config/lumen/config")), Some(PathBuf::from("/home/me/.config/lumen/install")));
    }

    #[test]
    fn a_package_is_neither_installed_nor_offered_an_install() {
        let dir = fresh_fixture("setup-origin-package");
        let record = Record { dir: dir.clone(), version: "0.7.0".into() };
        std::fs::write(dir.join("lumen"), "bin").unwrap();
        let origin = origin(Path::new("/usr/bin/lumen"), true, Some(record), "lumen", dir.join("default"));
        assert_eq!(origin, Origin::Package { exe: PathBuf::from("/usr/bin/lumen") });
    }

    #[test]
    fn installed_from_the_copy_itself_or_from_another() {
        let dir = fresh_fixture("setup-origin-installed");
        std::fs::write(dir.join("lumen"), "bin").unwrap();
        std::fs::write(dir.join("download"), "bin").unwrap();
        let record = Record { dir: dir.clone(), version: "0.7.0".into() };
        let from_it = origin(&dir.join("lumen"), false, Some(record.clone()), "lumen", dir.join("default"));
        assert_eq!(from_it, Origin::Installed { record: record.clone(), running_it: true });
        let from_elsewhere = origin(&dir.join("download"), false, Some(record.clone()), "lumen", dir.join("default"));
        assert_eq!(from_elsewhere, Origin::Installed { record, running_it: false });
    }

    #[test]
    fn a_record_whose_binary_is_gone_means_not_installed() {
        let dir = fresh_fixture("setup-origin-gone");
        let record = Record { dir: dir.join("lumen-dir"), version: "0.7.0".into() };
        let origin = origin(&dir.join("download"), false, Some(record), "lumen", dir.join("default"));
        assert_eq!(origin, Origin::NotInstalled { default_dir: dir.join("default") });
    }

    #[cfg(unix)]
    #[test]
    fn writing_through_a_symlink_keeps_the_link() {
        let dir = fresh_fixture("setup-write-symlink");
        std::fs::write(dir.join("real"), "old").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        write_atomic(&dir.join("link"), b"new").unwrap();
        assert!(dir.join("link").symlink_metadata().unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(std::fs::read_to_string(dir.join("real")).unwrap(), "new");
    }

    #[test]
    fn copying_the_binary_replaces_an_older_one() {
        let dir = fresh_fixture("setup-copy");
        std::fs::write(dir.join("new"), "v2").unwrap();
        std::fs::create_dir_all(dir.join("to")).unwrap();
        std::fs::write(dir.join("to/lumen"), "v1").unwrap();
        let to = copy_binary(&dir.join("new"), &dir.join("to"), "lumen").unwrap();
        assert_eq!(std::fs::read_to_string(to).unwrap(), "v2");
        assert!(!dir.join("to/lumen.lumen-tmp").exists());
    }
}
