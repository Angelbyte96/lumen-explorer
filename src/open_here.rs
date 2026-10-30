//! The folder menu's Open Console and Open File Explorer: the system's own
//! terminal and file manager, started in the folder on screen. Neither may be
//! Lumen itself, which Lumen Setup can make the folders' default.

use std::path::Path;

use crate::i18n::tr;

/// Opens a terminal in `folder`: as "Open Terminal Here".
#[cfg(not(windows))]
pub fn open_console(folder: &Path) -> Result<(), String> {
    crate::apps::open_terminal(folder)
}

/// The menu entry Lumen's install and the `.deb` add: never the system's file manager.
#[cfg(not(windows))]
const LUMEN_ENTRY: &str = "lumen.desktop";

/// The first app registered for folders that is not Lumen: Files on Ubuntu.
#[cfg(not(windows))]
pub fn file_manager(dirs: &crate::apps::Dirs) -> Option<crate::apps::App> {
    crate::apps::apps_for(dirs, "", true).into_iter().find(|app| app.desktop.file_name().is_none_or(|name| name != LUMEN_ENTRY))
}

/// Opens `folder` in the system's file manager.
#[cfg(not(windows))]
pub fn open_file_manager(folder: &Path) -> Result<(), String> {
    let app = file_manager(&crate::apps::Dirs::from_env()).ok_or(tr!("No file manager found besides Lumen"))?;
    crate::apps::open_with(&app, folder)
}

/// Windows' consoles to try, in order, with their arguments and creation
/// flags: Windows Terminal in the folder it starts in (`-d .`: its own
/// argument parsing would split a folder holding `;`), then PowerShell in a
/// console of its own (Lumen has none to share).
#[cfg(windows)]
fn consoles() -> [(&'static str, &'static [&'static str], u32); 2] {
    use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;
    [("wt.exe", &["-d", "."], 0), ("powershell.exe", &[], CREATE_NEW_CONSOLE)]
}

#[cfg(windows)]
pub fn open_console(folder: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    for (program, args, flags) in consoles() {
        match std::process::Command::new(program).args(args).current_dir(folder).creation_flags(flags).spawn() {
            Ok(mut child) => {
                // Reaped in the background so no handle is left behind.
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(tr!("Cannot open {0}: {1}", program, e)),
        }
    }
    Err(tr!("No console found").into())
}

/// Opens `folder` in File Explorer through the shell's `explore` verb, which
/// is File Explorer's own: Lumen's Set as default changes only the folders'
/// default verb, so `explorer.exe <folder>` could open Lumen again.
#[cfg(windows)]
pub fn open_file_manager(folder: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
    const SW_SHOWNORMAL: i32 = 1;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("explore".as_ref()), wide(folder.as_os_str()));
    // SAFETY: an all-zero SHELLEXECUTEINFOW is valid (null pointers and handles).
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    // SAFETY: `info` is sized and filled above; `verb` and `file` outlive the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(tr!("Cannot open {0} in File Explorer: {1}", folder.display(), std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An XDG tree whose folders open with `default` first, Nautilus listed too.
    #[cfg(not(windows))]
    fn dirs(name: &str, default: &str, apps: &[(&str, &str)]) -> crate::apps::Dirs {
        let root = crate::fs::ops::tests::fresh_fixture(name);
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let listed: Vec<&str> = apps.iter().map(|(id, _)| *id).collect();
        write("system/applications/mimeinfo.cache", &format!("[MIME Cache]\ninode/directory={};\n", listed.join(";")));
        for (id, name) in apps {
            write(&format!("system/applications/{id}"), &format!("[Desktop Entry]\nType=Application\nName={name}\n"));
        }
        if !default.is_empty() {
            write("config/mimeapps.list", &format!("[Default Applications]\ninode/directory={default};\n"));
        }
        crate::apps::Dirs { config: vec![root.join("config")], data: vec![root.join("user"), root.join("system")], lang: "en_US.UTF-8".into() }
    }

    #[cfg(not(windows))]
    #[test]
    fn the_file_manager_is_the_folders_app_that_is_not_lumen() {
        let both = [("lumen.desktop", "Lumen"), ("org.gnome.Nautilus.desktop", "Files")];
        let found = file_manager(&dirs("open-here-default-lumen", "lumen.desktop", &both)).map(|app| app.name);
        assert_eq!(found.as_deref(), Some("Files"), "with Lumen as the default, the one after it");
        let found = file_manager(&dirs("open-here-default-files", "", &both)).map(|app| app.name);
        assert_eq!(found.as_deref(), Some("Files"), "without a default, the first listed but Lumen");
        let only = [("lumen.desktop", "Lumen")];
        assert_eq!(file_manager(&dirs("open-here-only-lumen", "lumen.desktop", &only)), None, "Lumen alone is no system file manager");
        assert_eq!(file_manager(&dirs("open-here-none", "", &[])), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_first_then_powershell_in_a_console_of_its_own() {
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        assert_eq!(consoles(), [("wt.exe", &["-d", "."][..], 0), ("powershell.exe", &[][..], CREATE_NEW_CONSOLE)]);
    }
}
