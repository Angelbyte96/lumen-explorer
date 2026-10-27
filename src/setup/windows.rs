//! Setup on Windows, all under HKCU and the user's folders (no admin rights):
//!   `%LOCALAPPDATA%\Lumen\lumen.exe`                       the installed binary (by default)
//!   `%APPDATA%\...\Start Menu\Programs\Lumen.lnk`          its Start menu entry
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lumen`   its "Apps & features" entry
//!   desktop shortcut   `Lumen.lnk` in the Desktop folder
//!   set as default     `HKCU\Software\Classes\Directory\shell` and `Drive\shell` (default) = lumen,
//!                      and Win+E through the CLSID File Explorer's shortcut key opens
//!   right-click        the verb "OpenInLumen" on folders, folder backgrounds and drives
//!   add to PATH        the install folder in `HKCU\Environment` `Path`

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use super::{Origin, Record, Switches, System};

/// The binary's name in the install folder.
const BINARY: &str = "lumen.exe";
/// The shortcuts' name, in the Start menu and on the Desktop.
const LINK: &str = "Lumen.lnk";
/// The folder verb that opens Lumen when a folder is double-clicked. Hidden
/// from the right-click menu: that one is `RIGHT_CLICK`'s.
const DEFAULT_VERB: &str = "lumen";
/// The verb of the right-click entry.
const RIGHT_CLICK_VERB: &str = "OpenInLumen";
/// The text of the right-click entry.
const RIGHT_CLICK_LABEL: &str = "Open in Lumen";
/// The class File Explorer's shortcut key (Win+E) opens a new window through.
const EXPLORER_CLSID: &str = "{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";
/// What the Apps & features entry shows as publisher.
const PUBLISHER: &str = "Lumen";
/// The `--setup` flag opens the Setup screen: what the Apps & features
/// "Uninstall" button runs.
const SETUP_FLAG: &str = "--setup";
/// How long each window may take to answer the PATH change broadcast.
const BROADCAST_TIMEOUT_MS: u32 = 1000;

pub struct Windows {
    /// The running binary.
    pub exe: PathBuf,
    /// Where Install remembers the folder; `None` when there are no settings.
    pub record_file: Option<PathBuf>,
    /// The HKCU subkey that holds `Classes` and `Microsoft\Windows\...`:
    /// `Software`. Tests use their own so they never touch the user's.
    pub software: String,
    /// The HKCU subkey with the user's `Path`: `Environment`.
    pub environment: String,
    /// The Start menu's Programs folder.
    pub start_menu: PathBuf,
    /// The Desktop folder; `None` when Windows has none.
    pub desktop: Option<PathBuf>,
    /// Where Install goes by default: `%LOCALAPPDATA%\Lumen`.
    pub default_dir: PathBuf,
    /// Tells Explorer and the other windows about the changes. Off under
    /// test: it would reach the real desktop.
    pub run_tools: bool,
}

impl Windows {
    pub fn from_env(record_file: Option<PathBuf>) -> Option<Self> {
        let env_dir = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
        Some(Self {
            exe: std::env::current_exe().ok()?,
            record_file,
            software: "Software".into(),
            environment: "Environment".into(),
            start_menu: env_dir("APPDATA")?.join(r"Microsoft\Windows\Start Menu\Programs"),
            desktop: known_desktop(),
            default_dir: env_dir("LOCALAPPDATA")?.join("Lumen"),
            run_tools: true,
        })
    }

    fn record(&self) -> Option<Record> {
        Record::load(self.record_file.as_deref()?)
    }

    /// The folder Lumen was installed to; `None` when it is not installed.
    fn installed_dir(&self) -> Option<PathBuf> {
        match self.origin() {
            Origin::Installed { record, .. } => Some(record.dir),
            _ => None,
        }
    }

    fn start_menu_link(&self) -> PathBuf {
        self.start_menu.join(LINK)
    }

    fn desktop_link(&self) -> Option<PathBuf> {
        Some(self.desktop.as_ref()?.join(LINK))
    }

    /// `HKCU\<software>\Classes\<rest>`, without the `HKCU\`.
    fn classes(&self, rest: &str) -> String {
        format!(r"{}\Classes\{rest}", self.software)
    }

    fn uninstall_key(&self) -> String {
        format!(r"{}\Microsoft\Windows\CurrentVersion\Uninstall\Lumen", self.software)
    }

    /// Tells Explorer that file types and their verbs changed, so it does not
    /// wait for a restart to show them.
    fn refresh_associations(&self) {
        if self.run_tools {
            use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
            // SAFETY: the event takes no items.
            unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
        }
    }

    /// Tells the running programs that the environment changed: Explorer
    /// starts the apps from its menus with it.
    fn refresh_environment(&self) {
        if self.run_tools {
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
            let area = wide("Environment");
            // SAFETY: `area` outlives the call, which is synchronous.
            unsafe {
                SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(area.as_ptr() as isize), SMTO_ABORTIFHUNG, BROADCAST_TIMEOUT_MS, None);
            }
        }
    }

    fn set_desktop_shortcut(&self, on: bool, target: &Path) -> Result<(), String> {
        let Some(file) = self.desktop_link() else {
            return if on { Err("This PC has no Desktop folder".into()) } else { Ok(()) };
        };
        if !on {
            return super::remove_file(&file).map_err(|err| format!("Cannot remove {}: {err}", file.display()));
        }
        write_shortcut(&file, target)
    }

    fn set_default(&self, on: bool, target: &Path) -> Result<(), String> {
        if !on {
            let mut errors = Vec::new();
            for kind in ["Directory", "Drive"] {
                let shell = self.classes(&format!(r"{kind}\shell"));
                // Only a default that is Lumen's: another app's stays.
                if matches!(read_value(&shell, ""), Ok(Some((verb, _))) if verb == DEFAULT_VERB) {
                    errors.extend(delete_value(&shell, "").err());
                }
                errors.extend(delete_tree(&format!(r"{shell}\{DEFAULT_VERB}")).err());
            }
            errors.extend(delete_tree(&self.classes(&format!(r"CLSID\{EXPLORER_CLSID}"))).err());
            return if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) };
        }
        let exe = quotable(target)?;
        for kind in ["Directory", "Drive"] {
            let shell = self.classes(&format!(r"{kind}\shell"));
            write_value(&shell, "", REG_SZ, DEFAULT_VERB)?;
            // Without the menu entry: the right-click switch has its own.
            write_value(&format!(r"{shell}\{DEFAULT_VERB}"), "ProgrammaticAccessOnly", REG_SZ, "")?;
            write_value(&format!(r"{shell}\{DEFAULT_VERB}\command"), "", REG_SZ, &format!("\"{exe}\" \"%1\""))?;
        }
        // Win+E opens a new window through this class's verb.
        let verb = self.classes(&format!(r"CLSID\{EXPLORER_CLSID}\shell\opennewwindow\command"));
        write_value(&verb, "", REG_SZ, &format!("\"{exe}\""))?;
        write_value(&verb, "DelegateExecute", REG_SZ, "")
    }

    /// The keys holding the right-click verb, with what each passes Lumen.
    fn right_click_keys(&self) -> [(String, &'static str); 3] {
        let key = |at: &str| self.classes(&format!(r"{at}\shell\{RIGHT_CLICK_VERB}"));
        [(key("Directory"), "%1"), (key(r"Directory\Background"), "%V"), (key("Drive"), "%1")]
    }

    fn set_right_click(&self, on: bool, target: &Path) -> Result<(), String> {
        if !on {
            let errors: Vec<String> = self.right_click_keys().iter().filter_map(|(key, _)| delete_tree(key).err()).collect();
            return if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) };
        }
        let exe = quotable(target)?;
        for (key, argument) in self.right_click_keys() {
            write_value(&key, "MUIVerb", REG_SZ, RIGHT_CLICK_LABEL)?;
            write_value(&key, "Icon", REG_SZ, &exe)?;
            write_value(&format!(r"{key}\command"), "", REG_SZ, &format!("\"{exe}\" \"{argument}\""))?;
        }
        Ok(())
    }

    fn set_path(&self, on: bool, dir: &Path) -> Result<(), String> {
        let dir = text(dir)?;
        if dir.contains(';') {
            return Err(format!("A folder with a ; in its name cannot be on the PATH: {dir}"));
        }
        // The value keeps its type: %USERPROFILE% and the like stay unexpanded.
        let (before, kind) = read_value(&self.environment, "Path")?.unwrap_or((String::new(), REG_EXPAND_SZ));
        let after = path_with(&before, &dir, on);
        if after == before {
            return Ok(());
        }
        write_value(&self.environment, "Path", kind, &after)?;
        self.refresh_environment();
        Ok(())
    }

    /// Hands the install folder to a hidden PowerShell that outlives Lumen:
    /// Windows cannot delete the exe that is running, so it waits for this
    /// process to exit, however long the user takes to close Lumen.
    fn remove_after_exit(&self, exe: &Path, dir: &Path) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW alone: with DETACHED_PROCESS Windows ignores it.
        // The child outlives Lumen either way.
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        let script = removal_script(std::process::id(), &text(exe)?, &text(dir)?);
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|err| format!("Cannot remove {} after Lumen closes: {err}", exe.display()))
    }
}

/// Waits for process `pid` to exit, then removes `exe`, then `dir` when it
/// is empty. Single-quoted: PowerShell expands nothing inside, not even %VAR%.
fn removal_script(pid: u32, exe: &str, dir: &str) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let (exe, dir) = (quote(exe), quote(dir));
    format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
         Remove-Item -LiteralPath {exe} -Force -ErrorAction SilentlyContinue; \
         if (-not (Get-ChildItem -LiteralPath {dir} -Force -ErrorAction SilentlyContinue)) {{ Remove-Item -LiteralPath {dir} -ErrorAction SilentlyContinue }}"
    )
}

impl System for Windows {
    fn origin(&self) -> Origin {
        // Windows has no package manager Lumen comes from.
        super::origin(&self.exe, false, self.record(), BINARY, self.default_dir.clone())
    }

    fn read(&self) -> Switches {
        let Some(dir) = self.installed_dir() else { return Switches::default() };
        let default = read_value(&self.classes(r"Directory\shell"), "");
        let path = read_value(&self.environment, "Path");
        Switches {
            desktop_shortcut: self.desktop_link().is_some_and(|file| file.is_file()),
            default: matches!(default, Ok(Some((verb, _))) if verb == DEFAULT_VERB),
            right_click: key_exists(&self.right_click_keys()[0].0),
            path: matches!(path, Ok(Some((value, _))) if path_has(&value, &dir.to_string_lossy())),
        }
    }

    fn apply(&self, want: Switches) -> Result<(), String> {
        let dir = self.installed_dir().ok_or("Lumen is not installed")?;
        let target = dir.join(BINARY);
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
        let mut associations = false;
        if want.default != now.default {
            step(self.set_default(want.default, &target));
            associations = true;
        }
        if want.right_click != now.right_click {
            step(self.set_right_click(want.right_click, &target));
            associations = true;
        }
        if want.path != now.path {
            step(self.set_path(want.path, &dir));
        }
        if associations {
            self.refresh_associations();
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
    }

    fn install(&self, dir: &Path, want: Switches) -> Result<(), String> {
        if !dir.is_absolute() {
            return Err(format!("Choose a full path, not {}", dir.display()));
        }
        let record_file = self.record_file.as_deref().ok_or("Lumen has no settings folder to remember the install in")?;
        // Before anything is copied: the paths go quoted into the registry.
        let shown = quotable(&dir.join(BINARY))?;
        let binary = super::copy_binary(&self.exe, dir, BINARY).map_err(|err| format!("Cannot copy Lumen to {}: {err}", dir.display()))?;
        write_shortcut(&self.start_menu_link(), &binary)?;
        self.register_uninstall(dir, &shown, &binary)?;
        Record { dir: dir.to_path_buf(), version: env!("CARGO_PKG_VERSION").into() }
            .save(record_file)
            .map_err(|err| format!("Cannot write {}: {err}", record_file.display()))?;
        self.apply(want)
    }

    fn uninstall(&self) -> Result<(), String> {
        let Origin::Installed { record, running_it } = self.origin() else {
            return Err("Lumen is not installed".into());
        };
        self.apply(Switches::default())?;
        let link = self.start_menu_link();
        super::remove_file(&link).map_err(|err| format!("Cannot remove {}: {err}", link.display()))?;
        delete_tree(&self.uninstall_key())?;
        if let Some(file) = &self.record_file {
            super::remove_file(file).map_err(|err| format!("Cannot remove {}: {err}", file.display()))?;
        }
        let exe = record.dir.join(BINARY);
        if running_it {
            return self.remove_after_exit(&exe, &record.dir);
        }
        super::remove_file(&exe).map_err(|err| format!("Cannot remove {}: {err}", exe.display()))?;
        // Only when empty: the folder may hold the user's own files.
        let _ = std::fs::remove_dir(&record.dir);
        Ok(())
    }
}

impl Windows {
    /// The Apps & features entry. Its Uninstall button runs Lumen's Setup.
    fn register_uninstall(&self, dir: &Path, exe: &str, binary: &Path) -> Result<(), String> {
        let key = self.uninstall_key();
        let kb = std::fs::metadata(binary).map(|meta| meta.len().div_ceil(1024)).unwrap_or(0);
        write_value(&key, "DisplayName", REG_SZ, "Lumen")?;
        write_value(&key, "DisplayVersion", REG_SZ, env!("CARGO_PKG_VERSION"))?;
        write_value(&key, "Publisher", REG_SZ, PUBLISHER)?;
        write_value(&key, "DisplayIcon", REG_SZ, exe)?;
        write_value(&key, "InstallLocation", REG_SZ, &text(dir)?)?;
        write_value(&key, "UninstallString", REG_SZ, &format!("\"{exe}\" {SETUP_FLAG}"))?;
        write_dword(&key, "NoModify", 1)?;
        write_dword(&key, "NoRepair", 1)?;
        write_dword(&key, "EstimatedSize", u32::try_from(kb).unwrap_or(u32::MAX))
    }
}

/// `path` as text for a registry value or a command line; refused when it
/// cannot be one, or holds a `"` (it could not be quoted in a command).
fn quotable(path: &Path) -> Result<String, String> {
    let shown = text(path)?;
    if shown.contains('"') {
        return Err(format!("Lumen cannot start from a path holding \": {shown}"));
    }
    Ok(shown)
}

fn text(path: &Path) -> Result<String, String> {
    path.to_str().map(str::to_owned).ok_or_else(|| format!("Lumen cannot use a path that is not Unicode: {}", path.display()))
}

/// Whether the `;`-separated PATH `value` already names `dir`: the case and a
/// trailing backslash do not tell folders apart on Windows.
fn path_has(value: &str, dir: &str) -> bool {
    value.split(';').any(|entry| same_entry(entry, dir))
}

fn same_entry(a: &str, b: &str) -> bool {
    let plain = |entry: &str| entry.trim().trim_end_matches(['\\', '/']).to_lowercase();
    plain(a) == plain(b)
}

/// The PATH `value` with `dir` added at the end, or removed, if it is not
/// already so. Every other entry stays exactly as it was.
fn path_with(value: &str, dir: &str, on: bool) -> String {
    let mut entries: Vec<&str> = value.split(';').collect();
    let has = entries.iter().any(|entry| same_entry(entry, dir));
    match (on, has) {
        (true, false) => {
            // A value that ends with ; has no entry after it.
            if entries.last() == Some(&"") {
                entries.pop();
            }
            entries.push(dir);
        }
        (false, true) => entries.retain(|entry| !same_entry(entry, dir)),
        _ => return value.to_owned(),
    }
    entries.join(";")
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

/// The Desktop folder, wherever Windows has it (OneDrive moves it).
fn known_desktop() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the string is the shell's, copied before it is freed.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let folder = PathBuf::from(OsString::from_wide(path.as_wide()));
        CoTaskMemFree(Some(path.0 as *const _));
        Some(folder)
    }
}

/// COM on this thread for as long as it lives.
struct Com(bool);

impl Com {
    fn start() -> Self {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: balanced by the drop when it worked. A thread that already
        // has another model (winit's) answers an error and keeps it.
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: matches the CoInitializeEx that worked.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// A shortcut at `link` that starts `target`, with its icon.
fn write_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("Cannot create {}: {err}", dir.display()))?;
    }
    let _com = Com::start();
    let (target_w, folder_w, link_w) = (wide(target), wide(target.parent().unwrap_or(target)), wide(link));
    // SAFETY: the wide strings outlive the calls, which are synchronous.
    let made = unsafe {
        (|| -> windows::core::Result<()> {
            let shortcut: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shortcut.SetPath(PCWSTR(target_w.as_ptr()))?;
            shortcut.SetWorkingDirectory(PCWSTR(folder_w.as_ptr()))?;
            shortcut.SetIconLocation(PCWSTR(target_w.as_ptr()), 0)?;
            shortcut.SetDescription(PCWSTR(wide("Lumen").as_ptr()))?;
            shortcut.cast::<IPersistFile>()?.Save(PCWSTR(link_w.as_ptr()), true)
        })()
    };
    made.map_err(|err| format!("Cannot write {}: {err}", link.display()))
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open or create.
        unsafe { RegCloseKey(self.0) };
    }
}

fn registry_error(action: &str, path: &str, code: u32) -> String {
    format!("Cannot {action} HKCU\\{path}: {}", std::io::Error::from_raw_os_error(code as i32))
}

/// `HKCU\<path>`; `None` when it does not exist.
fn open(path: &str, access: u32) -> Result<Option<Key>, String> {
    let mut key: HKEY = null_mut();
    // SAFETY: the path is nul-terminated and `key` receives the handle.
    let code = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, access, &mut key) };
    match code {
        ERROR_SUCCESS => Ok(Some(Key(key))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        _ => Err(registry_error("read", path, code)),
    }
}

fn key_exists(path: &str) -> bool {
    matches!(open(path, KEY_READ), Ok(Some(_)))
}

/// The text value `name` ("" is the key's default) of `HKCU\<path>` and its
/// type (`REG_SZ` or `REG_EXPAND_SZ`); `None` when the key or value is missing.
fn read_value(path: &str, name: &str) -> Result<Option<(String, u32)>, String> {
    let Some(key) = open(path, KEY_READ)? else { return Ok(None) };
    let name_w = wide(name);
    let (mut kind, mut size) = (0, 0);
    // SAFETY: the first call only asks for the size; the second fills `buf`,
    // which holds `size` bytes.
    let code = unsafe { RegQueryValueExW(key.0, name_w.as_ptr(), null(), &mut kind, null_mut(), &mut size) };
    match code {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        _ => return Err(registry_error("read", path, code)),
    }
    // A value that is not text is another program's: never to be overwritten.
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Err(format!("HKCU\\{path} {name} is not text"));
    }
    let mut buf = vec![0u16; size as usize / 2 + 1];
    size = (buf.len() * 2) as u32;
    let code = unsafe { RegQueryValueExW(key.0, name_w.as_ptr(), null(), &mut kind, buf.as_mut_ptr().cast(), &mut size) };
    if code != ERROR_SUCCESS {
        return Err(registry_error("read", path, code));
    }
    let value = String::from_utf16_lossy(&buf[..(size as usize / 2).min(buf.len())]);
    Ok(Some((value.trim_end_matches('\0').to_owned(), kind)))
}

fn write_raw(path: &str, name: &str, kind: u32, data: &[u8]) -> Result<(), String> {
    let mut key: HKEY = null_mut();
    // SAFETY: the path is nul-terminated and `key` receives the handle.
    let code = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, null(), &mut key, null_mut()) };
    if code != ERROR_SUCCESS {
        return Err(registry_error("write", path, code));
    }
    let key = Key(key);
    // SAFETY: `data` is `data.len()` bytes.
    let code = unsafe { RegSetValueExW(key.0, wide(name).as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
    if code != ERROR_SUCCESS {
        return Err(registry_error("write", path, code));
    }
    Ok(())
}

/// Sets the text value `name` ("" is the key's default) of `HKCU\<path>`,
/// creating the key.
fn write_value(path: &str, name: &str, kind: u32, value: &str) -> Result<(), String> {
    let data: Vec<u8> = wide(value).iter().flat_map(|unit| unit.to_le_bytes()).collect();
    write_raw(path, name, kind, &data)
}

fn write_dword(path: &str, name: &str, value: u32) -> Result<(), String> {
    write_raw(path, name, REG_DWORD, &value.to_le_bytes())
}

/// Removes the value `name` ("" is the key's default); already gone is no error.
fn delete_value(path: &str, name: &str) -> Result<(), String> {
    let Some(key) = open(path, KEY_SET_VALUE)? else { return Ok(()) };
    // SAFETY: the name is nul-terminated.
    match unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        code => Err(registry_error("remove", path, code)),
    }
}

/// Removes `HKCU\<path>` and everything under it; already gone is no error.
fn delete_tree(path: &str) -> Result<(), String> {
    // An empty path would be HKCU itself.
    if path.is_empty() {
        return Err("Cannot remove HKCU".into());
    }
    // SAFETY: the path is nul-terminated.
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr()) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        code => Err(registry_error("remove", path, code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    /// A Windows system on a fresh fixture folder and its own registry
    /// subkey, which is deleted when the test ends (and when the next run of
    /// it starts, in case it was stopped).
    struct Fixture {
        windows: Windows,
        home: PathBuf,
    }

    impl std::ops::Deref for Fixture {
        type Target = Windows;
        fn deref(&self) -> &Windows {
            &self.windows
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = delete_tree(&self.windows.software);
        }
    }

    /// A system started from `exe` in the fixture.
    fn system(name: &str, exe: &str) -> Fixture {
        let home = fresh_fixture(name);
        let exe = home.join(exe);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "MZ").unwrap();
        let software = format!(r"Software\LumenTest\{name}");
        delete_tree(&software).unwrap();
        let windows = Windows {
            exe,
            record_file: Some(home.join(r"AppData\Roaming\lumen\install")),
            environment: format!(r"{software}\Environment"),
            software,
            start_menu: home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs"),
            desktop: Some(home.join("Desktop")),
            default_dir: home.join(r"AppData\Local\Lumen"),
            run_tools: false,
        };
        Fixture { windows, home }
    }

    fn installed(name: &str) -> Fixture {
        let fixture = system(name, r"Downloads\lumen.exe");
        let dir = fixture.home.join(r"AppData\Local\Lumen");
        fixture.install(&dir, Switches::default()).unwrap();
        fixture
    }

    fn all() -> Switches {
        Switches { desktop_shortcut: true, default: true, right_click: true, path: true }
    }

    /// The text value, or `None`.
    fn value(win: &Windows, key: &str, name: &str) -> Option<String> {
        read_value(&format!(r"{}\{key}", win.software), name).unwrap().map(|(text, _)| text)
    }

    fn exists(win: &Windows, key: &str) -> bool {
        key_exists(&format!(r"{}\{key}", win.software))
    }

    /// The text value of a key under `Classes` (the switches' own tree).
    fn class_value(win: &Windows, key: &str, name: &str) -> Option<String> {
        read_value(&win.classes(key), name).unwrap().map(|(text, _)| text)
    }

    fn class_exists(win: &Windows, key: &str) -> bool {
        key_exists(&win.classes(key))
    }

    #[test]
    fn started_from_a_download_lumen_is_not_installed() {
        let win = system("setup-windows-download", r"Downloads\lumen.exe");
        assert_eq!(win.origin(), Origin::NotInstalled { default_dir: win.home.join(r"AppData\Local\Lumen") });
        assert_eq!(win.read(), Switches::default());
        assert_eq!(win.apply(Switches::INSTALL), Err("Lumen is not installed".into()));
        assert_eq!(win.uninstall(), Err("Lumen is not installed".into()));
    }

    #[test]
    fn install_copies_lumen_and_adds_it_to_the_start_menu_and_apps_and_features() {
        let win = installed("setup-windows-install");
        let dir = win.home.join(r"AppData\Local\Lumen");
        let exe = dir.join("lumen.exe");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "MZ");
        assert!(win.start_menu.join("Lumen.lnk").is_file());
        let record = Record { dir: dir.clone(), version: env!("CARGO_PKG_VERSION").into() };
        assert_eq!(win.origin(), Origin::Installed { record, running_it: false });
        assert_eq!(win.read(), Switches::default(), "no switch asked for, none on");
        let key = r"Microsoft\Windows\CurrentVersion\Uninstall\Lumen";
        let text = |name: &str| value(&win, key, name).unwrap_or_else(|| panic!("{name} is missing"));
        assert_eq!(text("DisplayName"), "Lumen");
        assert_eq!(text("DisplayVersion"), env!("CARGO_PKG_VERSION"));
        assert_eq!(text("Publisher"), "Lumen");
        assert_eq!(text("DisplayIcon"), exe.to_str().unwrap());
        assert_eq!(text("InstallLocation"), dir.to_str().unwrap());
        assert_eq!(text("UninstallString"), format!("\"{}\" --setup", exe.display()));
        let dword = |name: &str| {
            let mut data = [0u8; 4];
            let mut size = 4u32;
            let mut kind = 0;
            let key = open(&format!(r"{}\{key}", win.software), KEY_READ).unwrap().unwrap();
            let code = unsafe { RegQueryValueExW(key.0, wide(name).as_ptr(), null(), &mut kind, data.as_mut_ptr(), &mut size) };
            assert_eq!((code, kind), (ERROR_SUCCESS, REG_DWORD), "{name} is a DWORD");
            u32::from_le_bytes(data)
        };
        assert_eq!((dword("NoModify"), dword("NoRepair")), (1, 1));
        assert_eq!(dword("EstimatedSize"), 1, "2 bytes round up to 1 KB");
    }

    #[test]
    fn install_turns_on_the_switches_asked_for() {
        let win = system("setup-windows-install-switches", r"Downloads\lumen.exe");
        let dir = win.home.join(r"Apps\Lumen");
        win.install(&dir, all()).unwrap();
        assert_eq!(win.read(), all());
        assert!(win.home.join(r"Desktop\Lumen.lnk").is_file());
        let exe = dir.join("lumen.exe");
        let command = |key: &str| class_value(&win, key, "").unwrap();
        assert_eq!(command(r"Directory\shell\OpenInLumen\command"), format!("\"{}\" \"%1\"", exe.display()));
        assert_eq!(command(r"Directory\Background\shell\OpenInLumen\command"), format!("\"{}\" \"%V\"", exe.display()));
        assert_eq!(command(r"Drive\shell\OpenInLumen\command"), format!("\"{}\" \"%1\"", exe.display()));
        assert_eq!(value(&win, r"Environment", "Path"), Some(dir.to_str().unwrap().to_owned()));
    }

    #[test]
    fn each_switch_turns_off_alone() {
        let win = installed("setup-windows-switch-off");
        win.apply(all()).unwrap();
        for off in [
            Switches { desktop_shortcut: false, ..all() },
            Switches { default: false, ..all() },
            Switches { right_click: false, ..all() },
            Switches { path: false, ..all() },
        ] {
            win.apply(off).unwrap();
            assert_eq!(win.read(), off);
            win.apply(all()).unwrap();
            assert_eq!(win.read(), all());
        }
    }

    #[test]
    fn each_switch_turns_on_alone() {
        let win = installed("setup-windows-switch-on");
        for on in [
            Switches { desktop_shortcut: true, ..Switches::default() },
            Switches { default: true, ..Switches::default() },
            Switches { right_click: true, ..Switches::default() },
            Switches { path: true, ..Switches::default() },
        ] {
            win.apply(on).unwrap();
            assert_eq!(win.read(), on);
            win.apply(Switches::default()).unwrap();
            assert_eq!(win.read(), Switches::default());
        }
    }

    #[test]
    fn set_as_default_writes_the_folder_drive_and_win_e_keys_and_off_removes_them() {
        let win = installed("setup-windows-default");
        let exe = win.home.join(r"AppData\Local\Lumen\lumen.exe");
        win.apply(Switches { default: true, ..Switches::default() }).unwrap();
        let command = format!("\"{}\" \"%1\"", exe.display());
        for kind in ["Directory", "Drive"] {
            assert_eq!(class_value(&win, &format!(r"{kind}\shell"), "").as_deref(), Some("lumen"));
            assert_eq!(class_value(&win, &format!(r"{kind}\shell\lumen"), "ProgrammaticAccessOnly").as_deref(), Some(""));
            assert_eq!(class_value(&win, &format!(r"{kind}\shell\lumen\command"), "").as_deref(), Some(command.as_str()));
        }
        let win_e = format!(r"CLSID\{EXPLORER_CLSID}\shell\opennewwindow\command");
        assert_eq!(class_value(&win, &win_e, "").as_deref(), Some(format!("\"{}\"", exe.display()).as_str()));
        assert_eq!(class_value(&win, &win_e, "DelegateExecute").as_deref(), Some(""));
        assert!(!class_exists(&win, r"Directory\shell\OpenInLumen"), "the right-click entry is its own switch");

        win.apply(Switches::default()).unwrap();
        for kind in ["Directory", "Drive"] {
            assert_eq!(class_value(&win, &format!(r"{kind}\shell"), ""), None);
            assert!(!class_exists(&win, &format!(r"{kind}\shell\lumen")));
        }
        assert!(!class_exists(&win, &format!(r"CLSID\{EXPLORER_CLSID}")));
    }

    #[test]
    fn right_click_writes_the_verb_on_folders_backgrounds_and_drives() {
        let win = installed("setup-windows-right-click");
        let exe = win.home.join(r"AppData\Local\Lumen\lumen.exe");
        win.apply(Switches { right_click: true, ..Switches::default() }).unwrap();
        for key in [r"Directory\shell\OpenInLumen", r"Directory\Background\shell\OpenInLumen", r"Drive\shell\OpenInLumen"] {
            assert_eq!(class_value(&win, key, "MUIVerb").as_deref(), Some("Open in Lumen"));
            assert_eq!(class_value(&win, key, "Icon").as_deref(), exe.to_str());
        }
        assert!(!class_exists(&win, r"Directory\shell\lumen"), "the default is its own switch");
        win.apply(Switches::default()).unwrap();
        for key in [r"Directory\shell\OpenInLumen", r"Directory\Background\shell\OpenInLumen", r"Drive\shell\OpenInLumen"] {
            assert!(!class_exists(&win, key), "{key} is still there");
        }
    }

    #[test]
    fn path_keeps_the_other_entries_and_the_values_type() {
        let win = installed("setup-windows-path");
        let dir = win.home.join(r"AppData\Local\Lumen");
        let before = r"%USERPROFILE%\bin;;C:\Tools\";
        write_value(&win.environment, "Path", REG_EXPAND_SZ, before).unwrap();
        win.apply(Switches { path: true, ..Switches::default() }).unwrap();
        let (on, kind) = read_value(&win.environment, "Path").unwrap().unwrap();
        assert_eq!(on, format!("{before};{}", dir.display()));
        assert_eq!(kind, REG_EXPAND_SZ);
        win.apply(Switches::default()).unwrap();
        assert_eq!(read_value(&win.environment, "Path").unwrap(), Some((before.to_owned(), REG_EXPAND_SZ)));

        write_value(&win.environment, "Path", REG_SZ, r"C:\Tools").unwrap();
        win.apply(Switches { path: true, ..Switches::default() }).unwrap();
        assert_eq!(read_value(&win.environment, "Path").unwrap().unwrap().1, REG_SZ, "a plain value stays plain");
    }

    #[test]
    fn path_finds_the_folder_whatever_its_case_or_trailing_backslash() {
        let win = installed("setup-windows-path-case");
        let dir = win.home.join(r"AppData\Local\Lumen");
        let spelled = format!(r"C:\Tools;{}\", dir.to_str().unwrap().to_uppercase());
        write_value(&win.environment, "Path", REG_EXPAND_SZ, &spelled).unwrap();
        assert!(win.read().path);
        win.apply(Switches { path: true, ..Switches::default() }).unwrap();
        assert_eq!(value(&win, "Environment", "Path").as_deref(), Some(spelled.as_str()), "already there: untouched");
        win.apply(Switches::default()).unwrap();
        assert_eq!(value(&win, "Environment", "Path").as_deref(), Some(r"C:\Tools"));
    }

    #[test]
    fn default_off_leaves_another_apps_default() {
        let win = installed("setup-windows-default-other");
        write_value(&win.classes(r"Directory\shell"), "", REG_SZ, "FileMan").unwrap();
        write_value(&win.classes(r"Drive\shell"), "", REG_SZ, "FileMan").unwrap();
        assert!(!win.read().default);
        win.apply(Switches::default()).unwrap();
        assert_eq!(class_value(&win, r"Directory\shell", "").as_deref(), Some("FileMan"));
        assert_eq!(class_value(&win, r"Drive\shell", "").as_deref(), Some("FileMan"));
    }

    #[test]
    fn uninstall_removes_lumen_and_every_switch_but_keeps_the_settings() {
        let win = installed("setup-windows-uninstall");
        let home = win.home.clone();
        write_value(&win.environment, "Path", REG_EXPAND_SZ, r"C:\Tools").unwrap();
        win.apply(all()).unwrap();
        std::fs::write(home.join(r"AppData\Roaming\lumen\config"), "bookmarks").unwrap();
        win.uninstall().unwrap();
        for gone in [
            r"AppData\Local\Lumen",
            r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Lumen.lnk",
            r"Desktop\Lumen.lnk",
            r"AppData\Roaming\lumen\install",
        ] {
            assert!(home.join(gone).symlink_metadata().is_err(), "{gone} is still there");
        }
        for key in [
            r"Microsoft\Windows\CurrentVersion\Uninstall\Lumen",
            r"Directory\shell\lumen",
            r"Drive\shell\lumen",
            r"Directory\shell\OpenInLumen",
            r"Directory\Background\shell\OpenInLumen",
            r"Drive\shell\OpenInLumen",
        ] {
            assert!(!class_exists(&win, key), "{key} is still there");
        }
        assert!(!class_exists(&win, &format!(r"CLSID\{EXPLORER_CLSID}")));
        assert_eq!(value(&win, "Environment", "Path").as_deref(), Some(r"C:\Tools"));
        assert_eq!(std::fs::read_to_string(home.join(r"AppData\Roaming\lumen\config")).unwrap(), "bookmarks");
        assert_eq!(win.origin(), Origin::NotInstalled { default_dir: home.join(r"AppData\Local\Lumen") });
        assert!(home.join(r"Downloads\lumen.exe").is_file(), "the download stays");
    }

    #[test]
    fn uninstall_leaves_a_folder_that_holds_other_files() {
        let win = system("setup-windows-uninstall-shared", r"Downloads\lumen.exe");
        let dir = win.home.join("Apps");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        win.install(&dir, Switches::default()).unwrap();
        win.uninstall().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("notes.txt")).unwrap(), "mine");
        assert!(!dir.join("lumen.exe").exists());
    }

    #[test]
    fn install_refuses_a_path_a_command_cannot_quote() {
        let win = system("setup-windows-bad-path", r"Downloads\lumen.exe");
        let dir = win.home.join("say \"hi\"");
        let err = win.install(&dir, Switches::default()).unwrap_err();
        assert_eq!(err, format!("Lumen cannot start from a path holding \": {}", dir.join("lumen.exe").display()));
        assert!(!dir.exists(), "nothing copied");
        assert!(!win.start_menu.join("Lumen.lnk").exists());
        assert!(!exists(&win, r"Microsoft\Windows\CurrentVersion\Uninstall\Lumen"));
        assert!(!win.home.join(r"AppData\Roaming\lumen\install").exists(), "nothing recorded");
    }

    #[test]
    fn install_refuses_a_relative_path() {
        let win = system("setup-windows-relative", r"Downloads\lumen.exe");
        assert_eq!(win.install(Path::new(r"Apps\Lumen"), Switches::default()), Err(r"Choose a full path, not Apps\Lumen".into()));
        assert_eq!(win.origin(), Origin::NotInstalled { default_dir: win.home.join(r"AppData\Local\Lumen") });
    }

    #[test]
    fn a_folder_with_a_semicolon_is_not_added_to_the_path() {
        let win = system("setup-windows-semicolon", r"Downloads\lumen.exe");
        let dir = win.home.join("a;b");
        win.install(&dir, Switches::default()).unwrap();
        let err = win.apply(Switches { path: true, ..Switches::default() }).unwrap_err();
        assert_eq!(err, format!("A folder with a ; in its name cannot be on the PATH: {}", dir.display()));
        assert_eq!(value(&win, "Environment", "Path"), None);
    }

    #[test]
    fn a_path_entry_is_added_and_removed_without_touching_the_rest() {
        assert_eq!(path_with("", r"C:\L", true), r"C:\L");
        assert_eq!(path_with(r"C:\A;C:\B", r"C:\L", true), r"C:\A;C:\B;C:\L");
        assert_eq!(path_with(r"C:\A;", r"C:\L", true), r"C:\A;C:\L");
        assert_eq!(path_with(r"C:\A;c:\l\;C:\B", r"C:\L", true), r"C:\A;c:\l\;C:\B", "already there");
        assert_eq!(path_with(r"C:\A;c:\l\;;C:\B", r"C:\L", false), r"C:\A;;C:\B");
        assert_eq!(path_with(r"C:\A;;C:\B", r"C:\L", false), r"C:\A;;C:\B", "not there");
    }

    #[test]
    fn the_removal_waits_for_lumen_to_exit_before_deleting() {
        let script = removal_script(4242, r"C:\Users\o'neil\Lumen\lumen.exe", r"C:\Users\o'neil\Lumen");
        let wait = script.find("Wait-Process -Id 4242").expect("waits for Lumen");
        let delete = script.find(r"Remove-Item -LiteralPath 'C:\Users\o''neil\Lumen\lumen.exe' -Force").expect("deletes the exe");
        assert!(wait < delete, "{script}");
        assert!(script.contains(r"if (-not (Get-ChildItem -LiteralPath 'C:\Users\o''neil\Lumen' -Force"), "the folder only when empty: {script}");
    }
}
