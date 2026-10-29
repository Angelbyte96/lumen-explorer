//! Setup on Linux, all in the home folder (no sudo):
//!   `~/.local/share/lumen/lumen`                    the installed binary (by default)
//!   `~/.local/share/applications/lumen.desktop`     its app menu entry, and the icon
//!   desktop shortcut   `lumen.desktop` in the desktop folder, marked trusted
//!   set as default     `inode/directory=lumen.desktop` in `~/.config/mimeapps.list`
//!   right-click        the Nautilus script "Open in Lumen"
//!   add to PATH        the link `~/.local/bin/lumen`
//! From the `.deb` (/usr/bin/lumen) apt owns the binary and its menu entry:
//! only the switches change, and Add to PATH is always on.

use std::path::{Path, PathBuf};

use crate::i18n::tr;

use super::{Origin, Record, Switches, System};

/// The binary's name in the install folder and on the PATH.
const BINARY: &str = "lumen";
/// The menu entry's name: GNOME matches the window to it by app id.
const ENTRY: &str = "lumen.desktop";
/// The name Nautilus shows under Scripts in its right-click menu.
const SCRIPT: &str = "Open in Lumen";
/// The key of a folder's default app in mimeapps.list.
const FOLDER_MIME: &str = "inode/directory";

pub struct Linux {
    /// The running binary.
    pub exe: PathBuf,
    pub home: PathBuf,
    /// `$XDG_DATA_HOME`, by default `~/.local/share`.
    pub data_home: PathBuf,
    /// `$XDG_CONFIG_HOME`, by default `~/.config`.
    pub config_home: PathBuf,
    /// Where Install remembers the folder; `None` when there are no settings.
    pub record_file: Option<PathBuf>,
    /// Runs gio and update-desktop-database. Off under test: they would touch
    /// the real desktop.
    pub run_tools: bool,
}

impl Linux {
    pub fn from_env(record_file: Option<PathBuf>) -> Option<Self> {
        let home = crate::fs::home_dir()?;
        let env_dir = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
        Some(Self {
            exe: std::env::current_exe().ok()?,
            data_home: env_dir("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
            config_home: env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            home,
            record_file,
            run_tools: true,
        })
    }

    /// apt's copy: a binary under /usr.
    fn packaged(&self) -> bool {
        self.exe.starts_with("/usr/")
    }

    fn record(&self) -> Option<Record> {
        Record::load(self.record_file.as_deref()?)
    }

    /// The binary the switches start: apt's, or the installed copy.
    fn target(&self) -> Option<PathBuf> {
        match self.origin() {
            Origin::Package { exe } => Some(exe),
            Origin::Installed { record, .. } => Some(record.dir.join(BINARY)),
            Origin::NotInstalled { .. } => None,
        }
    }

    fn menu_entry(&self) -> PathBuf {
        self.data_home.join("applications").join(ENTRY)
    }

    fn icons(&self) -> [(PathBuf, &'static [u8]); 2] {
        let icons = self.data_home.join("icons/hicolor");
        [
            (icons.join("scalable/apps/lumen.svg"), include_bytes!("../../assets/lumen.svg")),
            (icons.join("256x256/apps/lumen.png"), include_bytes!("../../assets/lumen.png")),
        ]
    }

    /// The desktop folder user-dirs.dirs names, else `~/Desktop`; `None` when
    /// it is set to the home folder (no desktop).
    fn desktop_dir(&self) -> Option<PathBuf> {
        let text = std::fs::read_to_string(self.config_home.join("user-dirs.dirs")).unwrap_or_default();
        crate::places::user_dir(&text, &self.home, "DESKTOP").unwrap_or_else(|| Some(self.home.join("Desktop")))
    }

    fn desktop_shortcut(&self) -> Option<PathBuf> {
        Some(self.desktop_dir()?.join(ENTRY))
    }

    fn script(&self) -> PathBuf {
        self.data_home.join("nautilus/scripts").join(SCRIPT)
    }

    fn mimeapps(&self) -> PathBuf {
        self.config_home.join("mimeapps.list")
    }

    fn path_link(&self) -> PathBuf {
        self.home.join(".local/bin").join(BINARY)
    }

    fn link_is_ours(&self, target: &Path) -> bool {
        std::fs::read_link(self.path_link()).is_ok_and(|to| to == target)
    }

    fn run(&self, program: &str, args: &[&std::ffi::OsStr]) {
        if self.run_tools {
            // Optional helpers: GNOME notices the files on its own, later.
            let _ = std::process::Command::new(program)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    /// Tells the app menu its entries and icons changed.
    fn refresh(&self) {
        let apps = self.data_home.join("applications");
        self.run("update-desktop-database", &[apps.as_os_str()]);
        // Only a cache that already exists: a new one would hide icons other
        // apps install later.
        let icons = self.data_home.join("icons/hicolor");
        if icons.join("icon-theme.cache").is_file() {
            self.run("gtk-update-icon-cache", &["-q".as_ref(), "-t".as_ref(), icons.as_os_str()]);
        }
    }

    fn set_desktop_shortcut(&self, on: bool, target: &Path) -> Result<(), String> {
        let Some(file) = self.desktop_shortcut() else {
            return if on { Err(tr!("This desktop has no desktop folder").into()) } else { Ok(()) };
        };
        if !on {
            return super::remove_file(&file).map_err(|err| tr!("Cannot remove {0}: {1}", file.display(), err));
        }
        write_executable(&file, &desktop_entry(target)?)?;
        // GNOME's desktop runs only shortcuts marked trusted.
        self.run("gio", &["set".as_ref(), file.as_os_str(), "metadata::trusted".as_ref(), "true".as_ref()]);
        Ok(())
    }

    fn set_default(&self, on: bool) -> Result<(), String> {
        let file = self.mimeapps();
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let ours = folder_default(&text).as_deref() == Some(ENTRY);
        let changed = match (on, ours) {
            (true, false) => with_folder_default(&text, Some(ENTRY)),
            // Off: the desktop's own default comes back (Nautilus on GNOME).
            (false, true) => with_folder_default(&text, None),
            _ => return Ok(()),
        };
        super::write_atomic(&file, changed.as_bytes()).map_err(|err| tr!("Cannot write {0}: {1}", file.display(), err))
    }

    fn set_right_click(&self, on: bool, target: &Path) -> Result<(), String> {
        let file = self.script();
        if !on {
            return super::remove_file(&file).map_err(|err| tr!("Cannot remove {0}: {1}", file.display(), err));
        }
        write_executable(&file, &nautilus_script(target))
    }

    fn set_path(&self, on: bool, target: &Path) -> Result<(), String> {
        let link = self.path_link();
        let ours = self.link_is_ours(target);
        if !on {
            // Only Lumen's own link: a lumen put there some other way stays.
            return if ours { super::remove_file(&link).map_err(|err| tr!("Cannot remove {0}: {1}", link.display(), err)) } else { Ok(()) };
        }
        if ours {
            return Ok(());
        }
        // Lumen's own link was handled above, so anything here is someone
        // else's: a copy install-desktop.sh put there, or a link of theirs.
        if link.symlink_metadata().is_ok() {
            return Err(tr!("{0} is another file (scripts/install-desktop.sh puts one there): remove it to add Lumen to the PATH", link.display()));
        }
        let dir = link.parent().expect("the link is in ~/.local/bin");
        std::fs::create_dir_all(dir).map_err(|err| tr!("Cannot create {0}: {1}", dir.display(), err))?;
        std::os::unix::fs::symlink(target, &link).map_err(|err| tr!("Cannot create {0}: {1}", link.display(), err))
    }
}

impl System for Linux {
    fn origin(&self) -> Origin {
        super::origin(&self.exe, self.packaged(), self.record(), BINARY, self.data_home.join("lumen"))
    }

    fn read(&self) -> Switches {
        let Some(target) = self.target() else { return Switches::default() };
        let mimeapps = std::fs::read_to_string(self.mimeapps()).unwrap_or_default();
        Switches {
            desktop_shortcut: self.desktop_shortcut().is_some_and(|file| file.is_file()),
            default: folder_default(&mimeapps).as_deref() == Some(ENTRY),
            right_click: self.script().is_file(),
            // /usr/bin is always on the PATH.
            path: self.packaged() || self.link_is_ours(&target),
        }
    }

    fn apply(&self, want: Switches) -> Result<(), String> {
        let target = self.target().ok_or(tr!("Lumen is not installed"))?;
        let now = self.read();
        let mut errors = Vec::new();
        let mut step = |result: Result<(), String>| {
            if let Err(err) = result {
                errors.push(err);
            }
        };
        if want.desktop_shortcut != now.desktop_shortcut {
            step(self.set_desktop_shortcut(want.desktop_shortcut, &target));
        }
        if want.default != now.default {
            step(self.set_default(want.default));
        }
        if want.right_click != now.right_click {
            step(self.set_right_click(want.right_click, &target));
        }
        if want.path != now.path && !self.packaged() {
            step(self.set_path(want.path, &target));
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
    }

    fn install(&self, dir: &Path, want: Switches) -> Result<(), String> {
        if self.packaged() {
            return Err(tr!("Installed with apt: there is nothing to install").into());
        }
        if !dir.is_absolute() {
            return Err(tr!("Choose a full path, not {0}", dir.display()));
        }
        let record_file = self.record_file.as_deref().ok_or(tr!("Lumen has no settings folder to remember the install in"))?;
        let binary = super::copy_binary(&self.exe, dir, BINARY).map_err(|err| tr!("Cannot copy Lumen to {0}: {1}", dir.display(), err))?;
        set_executable(&binary)?;
        write_file(&self.menu_entry(), &desktop_entry(&binary)?)?;
        for (path, bytes) in self.icons() {
            super::write_atomic(&path, bytes).map_err(|err| tr!("Cannot write {0}: {1}", path.display(), err))?;
        }
        Record { dir: dir.to_path_buf(), version: env!("CARGO_PKG_VERSION").into() }
            .save(record_file)
            .map_err(|err| tr!("Cannot write {0}: {1}", record_file.display(), err))?;
        self.refresh();
        self.apply(want)
    }

    fn uninstall(&self) -> Result<(), String> {
        let Origin::Installed { record, .. } = self.origin() else {
            return Err(if self.packaged() { tr!("Installed with apt: sudo apt remove lumen").into() } else { tr!("Lumen is not installed").into() });
        };
        self.apply(Switches::default())?;
        let mut files = vec![self.menu_entry(), record.dir.join(BINARY)];
        files.extend(self.icons().map(|(path, _)| path));
        files.extend(self.record_file.clone());
        for file in files {
            super::remove_file(&file).map_err(|err| tr!("Cannot remove {0}: {1}", file.display(), err))?;
        }
        // Only when empty: the folder may hold the user's own files.
        let _ = std::fs::remove_dir(&record.dir);
        self.refresh();
        Ok(())
    }
}

/// assets/lumen.desktop starting `binary` by its full path: ~/.local/bin is
/// not always on the PATH GNOME starts apps with.
fn desktop_entry(binary: &Path) -> Result<String, String> {
    let shown = binary.to_str().filter(|p| !p.contains(['"', '`', '$', '\\', '%', '\n', '\r']));
    // Inside a quoted Exec argument these would need escaping; refuse them instead.
    let shown = shown.ok_or_else(|| tr!("Lumen cannot start from a path holding \" ` $ \\ or %: {0}", binary.display()))?;
    Ok(include_str!("../../assets/lumen.desktop")
        .lines()
        .map(|line| match line.strip_prefix("Exec=lumen ") {
            Some(args) => format!("Exec=\"{shown}\" {args}\n"),
            None => format!("{line}\n"),
        })
        .collect())
}

/// Nautilus runs a script with the selected entries, or none, in the shown
/// folder: Lumen opens the first one, or that folder.
fn nautilus_script(binary: &Path) -> String {
    let quoted = binary.to_string_lossy().replace('\'', r"'\''");
    format!("#!/bin/sh\n# Added by Lumen Setup.\nexec '{quoted}' \"${{1:-$PWD}}\"\n")
}

/// The first app of `[Default Applications]` for folders.
pub fn folder_default(text: &str) -> Option<String> {
    let mut in_defaults = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_defaults = line == "[Default Applications]";
        } else if in_defaults
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == FOLDER_MIME
        {
            return value.split(';').map(str::trim).find(|app| !app.is_empty()).map(str::to_owned);
        }
    }
    None
}

/// `text` with the folders' default set to `app`, or removed for `None`. The
/// other lines stay as they were.
pub fn with_folder_default(text: &str, app: Option<&str>) -> String {
    let mut out = Vec::new();
    let mut in_defaults = false;
    let mut found_section = false;
    let mut placed = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_defaults && !placed && let Some(app) = app {
                out.push(format!("{FOLDER_MIME}={app};"));
                placed = true;
            }
            in_defaults = trimmed == "[Default Applications]";
            found_section |= in_defaults;
        } else if in_defaults && trimmed.split_once('=').is_some_and(|(key, _)| key.trim() == FOLDER_MIME) {
            let (_, value) = trimmed.split_once('=').expect("the line has an =");
            match app.filter(|_| !placed) {
                Some(app) => {
                    out.push(format!("{FOLDER_MIME}={app};"));
                    placed = true;
                }
                // Off: only Lumen leaves the list; a fallback app stays.
                None => {
                    let kept: Vec<&str> = value.split(';').map(str::trim).filter(|app| !app.is_empty() && *app != ENTRY).collect();
                    if !kept.is_empty() {
                        out.push(format!("{FOLDER_MIME}={};", kept.join(";")));
                    }
                }
            }
            continue;
        }
        out.push(line.to_owned());
    }
    if let Some(app) = app.filter(|_| !placed) {
        if !found_section {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push("[Default Applications]".into());
        }
        out.push(format!("{FOLDER_MIME}={app};"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    super::write_atomic(path, text.as_bytes()).map_err(|err| tr!("Cannot write {0}: {1}", path.display(), err))
}

fn write_executable(path: &Path, text: &str) -> Result<(), String> {
    write_file(path, text)?;
    set_executable(path)
}

fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|err| tr!("Cannot make {0} runnable: {1}", path.display(), err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    /// A Linux system whose home is a fresh fixture, started from `exe` in it.
    fn system(name: &str, exe: &str) -> (Linux, PathBuf) {
        let home = fresh_fixture(name);
        let exe = home.join(exe);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let linux = Linux {
            exe,
            data_home: home.join(".local/share"),
            config_home: home.join(".config"),
            record_file: Some(home.join(".config/lumen/install")),
            home: home.clone(),
            run_tools: false,
        };
        (linux, home)
    }

    fn installed(name: &str) -> (Linux, PathBuf) {
        let (linux, home) = system(name, "Downloads/lumen");
        linux.install(&home.join(".local/share/lumen"), Switches::default()).unwrap();
        (linux, home)
    }

    #[test]
    fn started_from_a_download_lumen_is_not_installed() {
        let (linux, home) = system("setup-linux-download", "Downloads/lumen");
        assert_eq!(linux.origin(), Origin::NotInstalled { default_dir: home.join(".local/share/lumen") });
        assert_eq!(linux.read(), Switches::default());
        assert_eq!(linux.apply(Switches::INSTALL), Err("Lumen is not installed".into()));
    }

    #[test]
    fn install_copies_lumen_and_adds_it_to_the_app_menu() {
        let (linux, home) = installed("setup-linux-install");
        let dir = home.join(".local/share/lumen");
        assert_eq!(std::fs::read_to_string(dir.join("lumen")).unwrap(), "#!/bin/sh\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("lumen")).unwrap().permissions().mode() & 0o777, 0o755);
        let entry = std::fs::read_to_string(home.join(".local/share/applications/lumen.desktop")).unwrap();
        assert!(entry.lines().any(|l| l == format!("Exec=\"{}\" --restore %f", dir.join("lumen").display())), "{entry}");
        assert!(home.join(".local/share/icons/hicolor/256x256/apps/lumen.png").is_file());
        assert!(home.join(".local/share/icons/hicolor/scalable/apps/lumen.svg").is_file());
        let record = Record { dir: dir.clone(), version: env!("CARGO_PKG_VERSION").into() };
        assert_eq!(linux.origin(), Origin::Installed { record, running_it: false });
        assert_eq!(linux.read(), Switches::default(), "no switch asked for, none on");
    }

    #[test]
    fn install_turns_on_the_switches_asked_for() {
        let (linux, home) = system("setup-linux-install-switches", "Downloads/lumen");
        let want = Switches { desktop_shortcut: true, default: true, right_click: true, path: true };
        linux.install(&home.join("Apps/Lumen"), want).unwrap();
        assert_eq!(linux.read(), want);
        let binary = home.join("Apps/Lumen/lumen");
        assert_eq!(std::fs::read_link(home.join(".local/bin/lumen")).unwrap(), binary);
        let script = std::fs::read_to_string(home.join(".local/share/nautilus/scripts/Open in Lumen")).unwrap();
        assert!(script.contains(&format!("exec '{}' \"${{1:-$PWD}}\"", binary.display())), "{script}");
        let shortcut = std::fs::read_to_string(home.join("Desktop/lumen.desktop")).unwrap();
        assert!(shortcut.contains(&format!("Exec=\"{}\"", binary.display())));
        assert_eq!(std::fs::read_to_string(home.join(".config/mimeapps.list")).unwrap(), "[Default Applications]\ninode/directory=lumen.desktop;\n");
    }

    #[test]
    fn each_switch_turns_off_alone() {
        let (linux, _) = installed("setup-linux-switch-off");
        let all = Switches { desktop_shortcut: true, default: true, right_click: true, path: true };
        linux.apply(all).unwrap();
        for off in [
            Switches { desktop_shortcut: false, ..all },
            Switches { default: false, ..all },
            Switches { right_click: false, ..all },
            Switches { path: false, ..all },
        ] {
            linux.apply(off).unwrap();
            assert_eq!(linux.read(), off);
            linux.apply(all).unwrap();
            assert_eq!(linux.read(), all);
        }
    }

    #[test]
    fn the_desktop_shortcut_goes_to_the_desktop_user_dirs_names() {
        let (linux, home) = installed("setup-linux-escritorio");
        std::fs::write(home.join(".config/user-dirs.dirs"), "XDG_DESKTOP_DIR=\"$HOME/Escritorio\"\n").unwrap();
        linux.apply(Switches { desktop_shortcut: true, ..Switches::default() }).unwrap();
        assert!(home.join("Escritorio/lumen.desktop").is_file());
        assert!(!home.join("Desktop/lumen.desktop").exists());
    }

    #[test]
    fn a_lumen_on_the_path_that_is_not_lumens_link_is_left_alone() {
        let (linux, home) = installed("setup-linux-path-other");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::write(home.join(".local/bin/lumen"), "dev build").unwrap();
        let err = linux.apply(Switches { path: true, ..Switches::default() }).unwrap_err();
        assert_eq!(err, format!("{} is another file (scripts/install-desktop.sh puts one there): remove it to add Lumen to the PATH", home.join(".local/bin/lumen").display()));
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/lumen")).unwrap(), "dev build");
        assert!(!linux.read().path);
    }

    #[test]
    fn default_off_gives_folders_back_to_the_desktop_and_keeps_other_lines() {
        let (linux, home) = installed("setup-linux-default-off");
        let before = "[Default Applications]\ntext/plain=org.gnome.TextEditor.desktop\n\n[Added Associations]\ninode/directory=org.gnome.Nautilus.desktop;\n";
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".config/mimeapps.list"), before).unwrap();
        linux.apply(Switches { default: true, ..Switches::default() }).unwrap();
        let on = std::fs::read_to_string(home.join(".config/mimeapps.list")).unwrap();
        assert_eq!(on, "[Default Applications]\ntext/plain=org.gnome.TextEditor.desktop\n\ninode/directory=lumen.desktop;\n[Added Associations]\ninode/directory=org.gnome.Nautilus.desktop;\n");
        linux.apply(Switches::default()).unwrap();
        let off = std::fs::read_to_string(home.join(".config/mimeapps.list")).unwrap();
        assert_eq!(off, before);
    }

    #[test]
    fn another_apps_default_is_not_removed_by_turning_default_off() {
        let (linux, home) = installed("setup-linux-default-other");
        let other = "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop;\n";
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".config/mimeapps.list"), other).unwrap();
        assert!(!linux.read().default);
        linux.apply(Switches::default()).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".config/mimeapps.list")).unwrap(), other);
    }

    #[test]
    fn uninstall_removes_lumen_and_every_switch_but_keeps_the_settings() {
        let (linux, home) = installed("setup-linux-uninstall");
        linux.apply(Switches { desktop_shortcut: true, default: true, right_click: true, path: true }).unwrap();
        std::fs::write(home.join(".config/lumen/config"), "bookmarks").unwrap();
        linux.uninstall().unwrap();
        for gone in [
            ".local/share/lumen",
            ".local/share/applications/lumen.desktop",
            ".local/share/icons/hicolor/256x256/apps/lumen.png",
            ".local/share/icons/hicolor/scalable/apps/lumen.svg",
            ".local/share/nautilus/scripts/Open in Lumen",
            ".local/bin/lumen",
            "Desktop/lumen.desktop",
            ".config/lumen/install",
        ] {
            assert!(home.join(gone).symlink_metadata().is_err(), "{gone} is still there");
        }
        assert_eq!(std::fs::read_to_string(home.join(".config/lumen/config")).unwrap(), "bookmarks");
        assert_eq!(linux.origin(), Origin::NotInstalled { default_dir: home.join(".local/share/lumen") });
        assert!(home.join("Downloads/lumen").is_file(), "the download stays");
    }

    #[test]
    fn uninstall_leaves_a_folder_that_holds_other_files() {
        let (linux, home) = system("setup-linux-uninstall-shared", "Downloads/lumen");
        let dir = home.join("Apps");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        linux.install(&dir, Switches::default()).unwrap();
        linux.uninstall().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("notes.txt")).unwrap(), "mine");
        assert!(!dir.join("lumen").exists());
    }

    #[test]
    fn install_refuses_a_path_an_exec_line_cannot_hold() {
        let (linux, home) = system("setup-linux-bad-path", "Downloads/lumen");
        let err = linux.install(&home.join("50%"), Switches::default()).unwrap_err();
        assert_eq!(err, format!("Lumen cannot start from a path holding \" ` $ \\ or %: {}", home.join("50%/lumen").display()));
        assert!(!home.join(".local/share/applications/lumen.desktop").exists());
        assert!(!home.join(".config/lumen/install").exists(), "nothing recorded");
    }

    #[test]
    fn from_apt_the_switches_work_and_nothing_installs() {
        let (mut linux, home) = system("setup-linux-package", "Downloads/lumen");
        linux.exe = PathBuf::from("/usr/bin/lumen");
        assert_eq!(linux.origin(), Origin::Package { exe: PathBuf::from("/usr/bin/lumen") });
        assert!(linux.read().path, "/usr/bin is on the PATH");
        assert_eq!(linux.install(&home.join(".local/share/lumen"), Switches::INSTALL), Err("Installed with apt: there is nothing to install".into()));
        assert_eq!(linux.uninstall(), Err("Installed with apt: sudo apt remove lumen".into()));
        let want = Switches { desktop_shortcut: true, default: true, right_click: true, path: true };
        linux.apply(want).unwrap();
        assert_eq!(linux.read(), want);
        let script = std::fs::read_to_string(home.join(".local/share/nautilus/scripts/Open in Lumen")).unwrap();
        assert!(script.contains("exec '/usr/bin/lumen'"));
        assert!(!home.join(".local/bin/lumen").exists(), "no link for apt's copy");
        assert!(!home.join(".local/share/applications/lumen.desktop").exists(), "apt's own entry is used");
        linux.apply(Switches { path: false, ..Switches::default() }).unwrap();
        assert!(linux.read().path, "Add to PATH stays on");
    }

    #[test]
    fn a_quote_in_the_path_is_kept_whole_in_the_script() {
        assert_eq!(nautilus_script(Path::new("/home/o'neil/lumen")), "#!/bin/sh\n# Added by Lumen Setup.\nexec '/home/o'\\''neil/lumen' \"${1:-$PWD}\"\n");
    }

    #[test]
    fn the_folder_default_is_the_first_app_listed() {
        assert_eq!(folder_default("[Default Applications]\ninode/directory=lumen.desktop;org.gnome.Nautilus.desktop;\n").as_deref(), Some("lumen.desktop"));
        assert_eq!(folder_default("[Added Associations]\ninode/directory=lumen.desktop;\n"), None);
        assert_eq!(folder_default(""), None);
    }

    #[test]
    fn setting_the_default_replaces_the_old_line_in_place() {
        let text = "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop;\ntext/plain=gedit.desktop\n";
        assert_eq!(with_folder_default(text, Some("lumen.desktop")), "[Default Applications]\ninode/directory=lumen.desktop;\ntext/plain=gedit.desktop\n");
    }
    #[test]
    fn a_users_own_symlink_on_the_path_is_left_alone() {
        let (linux, home) = installed("setup-linux-path-user-link");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::write(home.join("devbuild"), "dev").unwrap();
        std::os::unix::fs::symlink(home.join("devbuild"), home.join(".local/bin/lumen")).unwrap();
        let err = linux.apply(Switches { path: true, ..Switches::default() }).unwrap_err();
        assert!(err.contains("is another file"), "{err}");
        assert_eq!(std::fs::read_link(home.join(".local/bin/lumen")).unwrap(), home.join("devbuild"));
        assert!(!linux.read().path);
    }

    #[test]
    fn default_through_a_symlinked_mimeapps_keeps_the_link_and_other_lines() {
        let (linux, home) = installed("setup-linux-mimeapps-link");
        let real = home.join("dotfiles/mimeapps.list");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        let before = "[Default Applications]\ntext/plain=gedit.desktop\n\n[Added Associations]\nimage/png=eog.desktop;\n";
        std::fs::write(&real, before).unwrap();
        std::os::unix::fs::symlink(&real, home.join(".config/mimeapps.list")).unwrap();
        linux.apply(Switches { default: true, ..Switches::default() }).unwrap();
        assert!(home.join(".config/mimeapps.list").symlink_metadata().unwrap().file_type().is_symlink());
        linux.apply(Switches::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), before);
        assert!(!home.join("dotfiles/mimeapps.list.lumen-tmp").exists());
    }

    #[test]
    fn uninstall_leaves_a_regular_lumen_on_the_path_and_other_files() {
        let (linux, home) = installed("setup-linux-uninstall-others");
        linux.apply(Switches { desktop_shortcut: true, right_click: true, ..Switches::default() }).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::write(home.join(".local/bin/lumen"), "dev build").unwrap();
        std::fs::write(home.join(".local/share/nautilus/scripts/Other"), "x").unwrap();
        std::fs::write(home.join("Desktop/other.desktop"), "x").unwrap();
        linux.uninstall().unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/lumen")).unwrap(), "dev build");
        assert!(home.join(".local/share/nautilus/scripts/Other").is_file() && home.join("Desktop/other.desktop").is_file());
    }

    #[test]
    fn a_relative_install_path_changes_nothing() {
        let (linux, home) = system("setup-linux-relative", "Downloads/lumen");
        assert_eq!(linux.install(Path::new("rel/dir"), Switches::INSTALL), Err("Choose a full path, not rel/dir".into()));
        assert!(!home.join(".config").exists() && !home.join(".local").exists());
    }

    #[test]
    fn default_off_keeps_a_fallback_app_listed_after_lumen() {
        let (linux, home) = installed("setup-linux-default-fallback");
        let before = "[Default Applications]\ninode/directory=lumen.desktop;org.gnome.Nautilus.desktop;\n";
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".config/mimeapps.list"), before).unwrap();
        assert!(linux.read().default);
        linux.apply(Switches::default()).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".config/mimeapps.list")).unwrap(), "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop;\n");
    }
}
