//! The Windows shell's own right-click items (what File Explorer and File
//! Pilot show: Open with, Send to, 7-Zip, PowerRename...), read into Lumen's
//! menu so they can be searched. The pure part is here and tested anywhere;
//! `native` talks to the shell on Windows.

// Built everywhere so its tests run on Linux; only Windows uses it.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::app::Sort;
use crate::fs::Entry;
use crate::menu::{self, Command, Item, Kind};

/// A shell label without its accelerator marks ("&Open" -> "Open", "&&" ->
/// "&") and without a shortcut after a tab.
pub fn clean_label(raw: &str) -> String {
    let text = raw.split('\t').next().unwrap_or(raw);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            // "&&" is a literal ampersand; a single one marks the access key.
            if chars.peek() == Some(&'&') {
                out.push('&');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

/// What a shell item does in Lumen: its canonical verb picks Lumen's own
/// command when Lumen has one (its clipboard, trash and in-place rename);
/// anything else is run by the shell, by `offset` from the first id.
pub fn command_for(verb: Option<&str>, offset: u32) -> Command {
    match verb.map(str::to_ascii_lowercase).as_deref() {
        Some("open") => Command::Open,
        Some("cut") => Command::Cut,
        Some("copy") => Command::Copy,
        Some("paste") => Command::Paste,
        Some("delete") => Command::Trash,
        Some("rename") => Command::Rename,
        Some("refresh") => Command::Refresh,
        Some("newfolder") => Command::NewFolder,
        _ => Command::Shell(offset),
    }
}

/// Lumen's shortcut for a command it runs itself.
pub fn shortcut_for(command: &Command) -> Option<&'static str> {
    match command {
        Command::Cut => Some("Ctrl+X"),
        Command::Copy => Some("Ctrl+C"),
        Command::Paste => Some("Ctrl+V"),
        Command::Trash => Some("Delete"),
        Command::Rename => Some("F2"),
        Command::NewFolder => Some("Ctrl+Shift+N"),
        _ => None,
    }
}

/// No separator first, last or twice in a row, and no empty submenu, at any depth.
pub fn tidy(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for mut item in items {
        if let Kind::Sub(sub) = item.kind {
            let sub = tidy(sub);
            if sub.is_empty() {
                continue;
            }
            item.kind = Kind::Sub(sub);
        }
        let separator = item.kind == Kind::Separator;
        if separator && out.last().is_none_or(|last| last.kind == Kind::Separator) {
            continue;
        }
        out.push(item);
    }
    if out.last().is_some_and(|last| last.kind == Kind::Separator) {
        out.pop();
    }
    out
}

/// Lumen's shortcuts on the items it runs itself, and `adjust` on each, at any depth.
fn with_lumen_rules(items: Vec<Item>, adjust: &impl Fn(&mut Item, &Command)) -> Vec<Item> {
    items
        .into_iter()
        .map(|mut item| {
            match &mut item.kind {
                Kind::Sub(sub) => *sub = with_lumen_rules(std::mem::take(sub), adjust),
                Kind::Run(command) => {
                    let command = command.clone();
                    item.shortcut = item.shortcut.or(shortcut_for(&command));
                    adjust(&mut item, &command);
                }
                Kind::Separator => {}
            }
            item
        })
        .collect()
}

/// The menu for the selection on Windows: the shell's items (`shell`, empty
/// when the shell could not be asked), then what only Lumen offers.
/// `can_paste`: Lumen's clipboard holds something, for a folder's own Paste.
pub fn entry_menu(shell: Vec<Item>, selection: &[&Entry], bookmarked: bool, can_paste: bool) -> Vec<Item> {
    if shell.is_empty() {
        return menu::entry_menu(selection, bookmarked, &[]);
    }
    let single = selection.len() == 1;
    let folder = single && selection[0].is_dir;
    let mut items = tidy(with_lumen_rules(shell, &|item, command| match command {
        Command::Rename | Command::Open => item.enabled &= single,
        // A folder's Paste puts things in that folder, not in the one on screen.
        Command::Paste if folder => {
            item.kind = Kind::Run(Command::PasteInto);
            item.enabled = can_paste;
        }
        Command::Paste => item.enabled = false,
        _ => {}
    }));
    // Without CMF_CANRENAME's answer (or with a shell that leaves it out) Lumen's own.
    if !contains(&items, &Command::Rename) {
        items.push(Item::separator());
        items.push(Item::run("Rename", Some("F2"), single, Command::Rename));
    }
    items.push(Item::separator());
    if folder {
        items.push(Item::run("Open in New Tab", None, true, Command::OpenInNewTab));
    }
    items.push(Item::run("Copy Path", None, true, Command::CopyPath));
    items.push(Item::run("Delete Permanently", Some("Shift+Delete"), true, Command::DeleteForever));
    if folder {
        let bookmark = if bookmarked { "Remove from Bookmarks" } else { "Add to Bookmarks" };
        items.push(Item::run(bookmark, None, true, Command::ToggleBookmark));
        items.push(Item::run("Calculate Size", Some("Alt+R"), true, Command::Count));
    }
    items
}

/// The menu for the folder's empty space on Windows: the shell's items, then
/// Lumen's Sort by and Refresh.
pub fn background_menu(shell: Vec<Item>, can_paste: bool, sort: Sort) -> Vec<Item> {
    if shell.is_empty() {
        return menu::background_menu(can_paste, sort, &[]);
    }
    // Paste is Lumen's: what matters is Lumen's clipboard, not the system's.
    let mut items = tidy(with_lumen_rules(shell, &|item, command| {
        if *command == Command::Paste {
            item.enabled = can_paste;
        }
    }));
    // The shell leaves Paste to File Explorer, which adds its own.
    if !contains(&items, &Command::Paste) {
        items.insert(0, Item::run("Paste", Some("Ctrl+V"), can_paste, Command::Paste));
    }
    let has_refresh = contains(&items, &Command::Refresh);
    items.extend([Item::separator(), menu::sort_menu(sort)]);
    if !has_refresh {
        items.push(Item::run("Refresh", None, true, Command::Refresh));
    }
    items
}

/// Whether `command` is anywhere in `items`.
fn contains(items: &[Item], command: &Command) -> bool {
    items.iter().any(|item| match &item.kind {
        Kind::Run(c) => c == command,
        Kind::Sub(sub) => contains(sub, command),
        Kind::Separator => false,
    })
}

/// A shell bitmap's BGRA pixels as RGBA. A bitmap with no alpha at all
/// (an old-style one) is opaque.
pub fn to_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    let (chunks, _) = pixels.as_chunks_mut::<4>();
    let opaque = chunks.iter().all(|p| p[3] == 0);
    for p in chunks {
        p.swap(0, 2);
        if opaque {
            p[3] = 255;
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SortColumn;
    use crate::menu::MenuIcon;
    use crate::menu::tests::{entry, outline};

    #[test]
    fn labels_lose_their_accelerators_and_tab_shortcuts() {
        assert_eq!(clean_label("&Open"), "Open");
        assert_eq!(clean_label("Save && Close"), "Save & Close");
        assert_eq!(clean_label("Cu&t\tCtrl+X"), "Cut");
        assert_eq!(clean_label("  Open with Code  "), "Open with Code");
        assert_eq!(clean_label("&"), "");
    }

    #[test]
    fn known_verbs_run_lumens_own_commands_and_the_rest_the_shell() {
        assert_eq!(command_for(Some("cut"), 7), Command::Cut);
        assert_eq!(command_for(Some("Copy"), 7), Command::Copy);
        assert_eq!(command_for(Some("paste"), 7), Command::Paste);
        assert_eq!(command_for(Some("delete"), 7), Command::Trash);
        assert_eq!(command_for(Some("rename"), 7), Command::Rename);
        assert_eq!(command_for(Some("refresh"), 7), Command::Refresh);
        assert_eq!(command_for(Some("NewFolder"), 7), Command::NewFolder);
        assert_eq!(command_for(Some("properties"), 7), Command::Shell(7));
        assert_eq!(command_for(Some("copyaspath"), 3), Command::Shell(3));
        assert_eq!(command_for(None, 9), Command::Shell(9));
        assert_eq!(shortcut_for(&Command::Cut), Some("Ctrl+X"));
        assert_eq!(shortcut_for(&Command::Trash), Some("Delete"));
        assert_eq!(shortcut_for(&Command::Shell(1)), None);
    }

    fn shell(label: &str, offset: u32) -> Item {
        Item::run(label, None, true, Command::Shell(offset))
    }

    #[test]
    fn shell_bitmaps_turn_into_rgba_and_alpha_less_ones_are_opaque() {
        assert_eq!(to_rgba(vec![1, 2, 3, 0, 4, 5, 6, 0]), [3, 2, 1, 255, 6, 5, 4, 255], "no alpha anywhere");
        assert_eq!(to_rgba(vec![1, 2, 3, 0, 4, 5, 6, 128]), [3, 2, 1, 0, 6, 5, 4, 128], "real alpha is kept");
        assert_eq!(to_rgba(Vec::new()), Vec::<u8>::new());
    }

    #[test]
    fn tidy_drops_stray_separators_and_empty_submenus() {
        let items = vec![
            Item::separator(),
            shell("Open", 0),
            Item::separator(),
            Item::separator(),
            Item::sub_plain("Send to", vec![Item::separator()]),
            Item::sub_plain("7-Zip", vec![Item::separator(), shell("Add to archive", 1), Item::separator()]),
            Item::separator(),
        ];
        assert_eq!(outline(&tidy(items)), ["Open", "-", "7-Zip", "  Add to archive"]);
    }

    #[test]
    fn the_windows_entry_menu_is_the_shells_then_lumens_own() {
        let dir = entry("src", true);
        let items = vec![
            shell("Open", 0),
            Item::run("Cut", None, true, Command::Cut),
            Item::run("Rename", None, true, Command::Rename),
            Item::separator(),
            shell("Properties", 4),
        ];
        assert_eq!(
            outline(&entry_menu(items.clone(), &[&dir], false, false)),
            [
                "Open",
                "Cut [Ctrl+X]",
                "Rename [F2]",
                "-",
                "Properties",
                "-",
                "Open in New Tab",
                "Copy Path",
                "Delete Permanently [Shift+Delete]",
                "Add to Bookmarks",
                "Calculate Size [Alt+R]",
            ]
        );
        let file = entry("a.txt", false);
        let several = outline(&entry_menu(items, &[&dir, &file], false, false));
        assert!(several.contains(&"Rename [F2] (off)".to_owned()), "{several:?}");
        assert_eq!(several.last().map(String::as_str), Some("Delete Permanently [Shift+Delete]"));
    }

    #[test]
    fn without_the_shell_the_windows_menu_is_lumens() {
        let file = entry("a.txt", false);
        assert_eq!(entry_menu(Vec::new(), &[&file], false, false), menu::entry_menu(&[&file], false, &[]));
        let sort = Sort::default();
        assert_eq!(background_menu(Vec::new(), true, sort), menu::background_menu(true, sort, &[]));
    }

    #[test]
    fn the_windows_background_menu_adds_sort_and_a_refresh_once() {
        let sort = Sort { column: SortColumn::Name, descending: false };
        let items = vec![
            Item::run("Paste", None, true, Command::Paste),
            Item::sub_plain("New", vec![Item::run("Folder", None, true, Command::NewFolder), shell("Text Document", 2)]),
            Item::separator(),
            shell("Properties", 5),
        ];
        let out = outline(&background_menu(items.clone(), false, sort));
        assert_eq!(out[..6], ["Paste [Ctrl+V] (off)", "New", "  Folder [Ctrl+Shift+N]", "  Text Document", "-", "Properties"]);
        assert_eq!(out[6..8], ["-", "Sort by"]);
        assert_eq!(out.last().map(String::as_str), Some("Refresh"));
        let mut with_refresh = items;
        with_refresh.push(Item::run("Refresh", None, true, Command::Refresh));
        let out = outline(&background_menu(with_refresh, true, sort));
        assert_eq!(out.iter().filter(|l| l.as_str() == "Refresh").count(), 1, "{out:?}");
    }

    fn runs(items: &[Item], command: &Command) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for item in items {
            match &item.kind {
                Kind::Run(c) if c == command => out.push((item.label.clone(), item.enabled)),
                Kind::Sub(sub) => out.extend(runs(sub, command)),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn the_windows_entry_menu_has_lumens_copy_path() {
        let file = entry("a.txt", false);
        let items = entry_menu(vec![shell("Open", 0), shell("Properties", 4)], &[&file], false, false);
        assert_eq!(runs(&items, &Command::CopyPath).len(), 1, "{:?}", outline(&items));
    }

    #[test]
    fn the_windows_entry_menu_keeps_a_rename_when_the_shell_gives_none() {
        let file = entry("a.txt", false);
        let items = entry_menu(vec![shell("Open", 0), Item::run("Delete", None, true, Command::Trash)], &[&file], false, false);
        assert_eq!(runs(&items, &Command::Rename), [("Rename".to_owned(), true)], "{:?}", outline(&items));
        let two = [&file, &file];
        assert_eq!(runs(&entry_menu(vec![shell("Open", 0)], &two, false, false), &Command::Rename), [("Rename".to_owned(), false)]);
    }

    #[test]
    fn paste_in_a_folders_menu_pastes_into_that_folder() {
        let dir = entry("src", true);
        let shell_items = vec![shell("Open", 0), Item::run("Paste", None, true, Command::Paste)];
        let items = entry_menu(shell_items.clone(), &[&dir], false, true);
        assert_eq!(runs(&items, &Command::Paste), []);
        assert_eq!(runs(&items, &Command::PasteInto), [("Paste".to_owned(), true)], "{:?}", outline(&items));
        let empty = entry_menu(shell_items.clone(), &[&dir], false, false);
        assert_eq!(runs(&empty, &Command::PasteInto), [("Paste".to_owned(), false)], "Lumen's clipboard decides");
        let file = entry("a.txt", false);
        let on_file = entry_menu(shell_items, &[&file], false, true);
        assert!(runs(&on_file, &Command::Paste).iter().chain(&runs(&on_file, &Command::PasteInto)).all(|(_, on)| !on));
    }

    #[test]
    fn the_shells_open_runs_lumens_open() {
        assert_eq!(command_for(Some("open"), 0), Command::Open);
        assert_eq!(command_for(Some("Open"), 0), Command::Open);
    }

    /// Windows CI showed the shell's background menu without Paste (File
    /// Explorer adds its own): Lumen's then comes first.
    #[test]
    fn the_windows_background_menu_always_has_lumens_paste() {
        let sort = Sort::default();
        let shell_items = vec![shell("Open in Terminal", 0), Item::sub_plain("New", vec![shell("Text Document", 1)]), shell("Properties", 2)];
        let items = background_menu(shell_items, true, sort);
        assert_eq!(runs(&items, &Command::Paste), [("Paste".to_owned(), true)]);
        assert_eq!(items[0].kind, Kind::Run(Command::Paste), "{:?}", outline(&items));
        assert_eq!(items[0].shortcut, Some("Ctrl+V"));
        let with_shells = background_menu(vec![Item::run("Pegar", None, true, Command::Paste)], false, sort);
        assert_eq!(runs(&with_shells, &Command::Paste), [("Pegar".to_owned(), false)], "not twice");
    }

    #[test]
    fn shell_items_keep_the_shells_icons_and_lumens_get_its_own() {
        let picture = std::sync::Arc::new(menu::Bitmap { size: [1, 1], rgba: vec![255, 0, 0, 255] });
        let open = shell("Open", 0).with_picture(Some(picture.clone()));
        let items = entry_menu(vec![open], &[&entry("a.txt", false)], false, false);
        assert_eq!(items[0].picture.as_ref(), Some(&picture));
        assert_eq!(items[0].icon, None, "the shell's picture, not a line icon");
        let delete = items.iter().find(|i| i.label == "Delete Permanently").unwrap();
        assert_eq!(delete.icon, Some(MenuIcon::Delete));
        assert!(matches!(items[0].kind, Kind::Run(Command::Shell(0))));
    }
}

/// The shell side, Windows only: builds the shell's menu, reads it into
/// `Item`s and runs the item picked.
#[cfg(windows)]
pub mod native {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
        ReleaseDC,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_SFObject, CMF_CANRENAME, CMF_NORMAL, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu, IContextMenu2,
        IContextMenu3, ILFindLastID, ILFree, IShellFolder, IShellItem, SHBindToParent, SHCreateItemFromParsingName,
        SHParseDisplayName,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HBMMENU_CALLBACK, HMENU, MENUITEMINFOW,
        MFS_DISABLED, MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU,
        SW_SHOWNORMAL, WM_INITMENUPOPUP,
    };
    use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR, PWSTR};

    use super::{clean_label, command_for, shortcut_for, tidy, to_rgba};
    use crate::menu::{Bitmap, Item};

    /// Ids the shell may give its items; Lumen's command is the offset from FIRST.
    const FIRST: u32 = 1;
    const LAST: u32 = 0x7FFF;
    /// Not in the windows crate; the same value as SEE_MASK_UNICODE.
    const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
    /// Submenus read inside submenus; extensions rarely go deeper.
    const MAX_DEPTH: usize = 3;
    /// Pictures above this side are not item icons; they are skipped.
    const MAX_PICTURE: i32 = 64;

    /// The shell's menu while Lumen shows it: kept alive until an item runs
    /// or the menu closes, as handlers expect.
    pub struct ShellMenu {
        menu: IContextMenu,
        hmenu: HMENU,
        /// Where the items act, for handlers that need a working folder.
        folder: PathBuf,
    }

    impl Drop for ShellMenu {
        fn drop(&mut self) {
            // SAFETY: created by CreatePopupMenu in `build` and destroyed only here.
            let _ = unsafe { DestroyMenu(self.hmenu) };
        }
    }

    /// Item ids parsed from paths, freed when dropped.
    struct Pidls(Vec<*mut ITEMIDLIST>);

    impl Drop for Pidls {
        fn drop(&mut self) {
            for &pidl in &self.0 {
                // SAFETY: each came from SHParseDisplayName and is freed once.
                unsafe { ILFree(Some(pidl)) };
            }
        }
    }

    fn hwnd(window: Option<isize>) -> HWND {
        HWND(window.unwrap_or(0) as *mut core::ffi::c_void)
    }

    fn fail(what: &str, e: windows::core::Error) -> String {
        format!("The system menu is not available ({what}: {e})")
    }

    /// The shell's menu for `paths`, which share one folder. The caller's
    /// thread has COM (the loader's has OLE).
    pub fn for_entries(paths: &[PathBuf], folder: &Path, window: Option<isize>) -> Result<(ShellMenu, Vec<Item>), String> {
        // SAFETY: every pointer handed to the shell outlives the call; ids are
        // freed by `Pidls`, after GetUIObjectOf copied what it needs.
        unsafe {
            let mut pidls = Pidls(Vec::with_capacity(paths.len()));
            for path in paths {
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None)
                    .map_err(|e| fail("parse", e))?;
                pidls.0.push(pidl);
            }
            let first = *pidls.0.first().ok_or("Nothing selected")?;
            let parent: IShellFolder = SHBindToParent(first, None).map_err(|e| fail("folder", e))?;
            let children: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|&p| ILFindLastID(p) as *const _).collect();
            let menu: IContextMenu = parent.GetUIObjectOf(hwnd(window), &children, None).map_err(|e| fail("menu", e))?;
            build(menu, folder)
        }
    }

    /// The shell's menu for the empty space of `folder`; COM as above.
    pub fn for_folder(folder: &Path, window: Option<isize>) -> Result<(ShellMenu, Vec<Item>), String> {
        // SAFETY: plain COM calls on interfaces the shell returned.
        unsafe {
            let item: IShellItem =
                SHCreateItemFromParsingName(&HSTRING::from(folder.as_os_str()), None).map_err(|e| fail("folder", e))?;
            let shell_folder: IShellFolder = item.BindToHandler(None, &BHID_SFObject).map_err(|e| fail("folder", e))?;
            let menu: IContextMenu = shell_folder.CreateViewObject(hwnd(window)).map_err(|e| fail("menu", e))?;
            build(menu, folder)
        }
    }

    /// Asks `menu` to fill a popup menu and reads it.
    unsafe fn build(menu: IContextMenu, folder: &Path) -> Result<(ShellMenu, Vec<Item>), String> {
        // SAFETY: the popup menu is owned by the returned ShellMenu.
        unsafe {
            let hmenu = CreatePopupMenu().map_err(|e| fail("popup", e))?;
            let shell = ShellMenu { menu, hmenu, folder: folder.to_path_buf() };
            // CMF_CANRENAME: Lumen can rename, so the shell lists Rename (Lumen's, by its verb).
            shell.menu.QueryContextMenu(hmenu, 0, FIRST, LAST, CMF_NORMAL | CMF_CANRENAME).ok().map_err(|e| fail("items", e))?;
            let items = tidy(read(&shell.menu, hmenu, 0));
            Ok((shell, items))
        }
    }

    /// The items of `hmenu`, submenus included (filled first, as on opening).
    unsafe fn read(menu: &IContextMenu, hmenu: HMENU, depth: usize) -> Vec<Item> {
        let mut items = Vec::new();
        // SAFETY: `hmenu` is alive (owned by the ShellMenu being built).
        let count = unsafe { GetMenuItemCount(Some(hmenu)) };
        for position in 0..count.max(0) as u32 {
            let mut text = [0u16; 260];
            let mut info = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING | MIIM_BITMAP,
                dwTypeData: PWSTR(text.as_mut_ptr()),
                cch: text.len() as u32 - 1,
                ..Default::default()
            };
            // SAFETY: `info` points at `text`, whose size it states.
            if unsafe { GetMenuItemInfoW(hmenu, position, true, &mut info) }.is_err() {
                continue;
            }
            if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                items.push(Item::separator());
                continue;
            }
            let label = clean_label(&String::from_utf16_lossy(&text[..(info.cch as usize).min(text.len())]));
            // Owner-drawn items without text cannot be shown.
            if label.is_empty() {
                continue;
            }
            let enabled = info.fState.0 & MFS_DISABLED.0 == 0;
            // SAFETY: the bitmap belongs to the live menu; only read.
            let picture = unsafe { picture(info.hbmpItem) };
            let item = if !info.hSubMenu.0.is_null() {
                if depth >= MAX_DEPTH {
                    continue;
                }
                // SAFETY: as when Windows opens the submenu: lets lazy ones (Open with, Send to) fill it.
                unsafe { init_submenu(menu, info.hSubMenu, position) };
                // SAFETY: the submenu belongs to `hmenu`.
                let sub = unsafe { read(menu, info.hSubMenu, depth + 1) };
                Item::sub_plain(label, sub)
            } else {
                if !(FIRST..=LAST).contains(&info.wID) {
                    continue;
                }
                let offset = info.wID - FIRST;
                // SAFETY: an id the shell gave its own item.
                let verb = unsafe { verb(menu, offset) };
                let command = command_for(verb.as_deref(), offset);
                Item::run(label, shortcut_for(&command), enabled, command)
            };
            items.push(Item { enabled: item.enabled && enabled, ..item.with_picture(picture) });
        }
        items
    }

    unsafe fn init_submenu(menu: &IContextMenu, submenu: HMENU, position: u32) {
        let (wparam, lparam) = (WPARAM(submenu.0 as usize), LPARAM(position as isize));
        // SAFETY: the message Windows itself sends before showing a submenu.
        unsafe {
            if let Ok(menu3) = menu.cast::<IContextMenu3>() {
                let _ = menu3.HandleMenuMsg2(WM_INITMENUPOPUP, wparam, lparam, None);
            } else if let Ok(menu2) = menu.cast::<IContextMenu2>() {
                let _ = menu2.HandleMenuMsg(WM_INITMENUPOPUP, wparam, lparam);
            }
        }
    }

    /// The item's canonical verb ("cut", "properties"), if it has one.
    unsafe fn verb(menu: &IContextMenu, offset: u32) -> Option<String> {
        let mut buf = [0u16; 64];
        // SAFETY: GCS_VERBW writes UTF-16 into `buf`, at most its length in characters.
        unsafe { menu.GetCommandString(offset as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr().cast()), buf.len() as u32) }
            .ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (end > 0).then(|| String::from_utf16_lossy(&buf[..end]))
    }

    /// The item's own bitmap as premultiplied RGBA, when it has a real one.
    unsafe fn picture(bitmap: HBITMAP) -> Option<Arc<Bitmap>> {
        // Null, HBMMENU_CALLBACK and the HBMMENU_* system values (1-11) are not pictures.
        let raw = bitmap.0 as isize;
        if bitmap.0.is_null() || bitmap == HBMMENU_CALLBACK || (1..=11).contains(&raw) {
            return None;
        }
        // SAFETY: a bitmap of the live menu.
        let (size, rgba) = unsafe { pixels(bitmap, MAX_PICTURE) }?;
        Some(Arc::new(Bitmap { size, rgba }))
    }

    /// A bitmap's pixels as RGBA (BGRA turned around; no alpha at all is
    /// opaque), when neither side is over `max_side`.
    ///
    /// # Safety
    /// `bitmap` must be a valid HBITMAP for the whole call.
    pub(crate) unsafe fn pixels(bitmap: HBITMAP, max_side: i32) -> Option<([usize; 2], Vec<u8>)> {
        let mut info = BITMAP::default();
        // SAFETY: `info` is a BITMAP of the size stated.
        let read = unsafe { GetObjectW(HGDIOBJ(bitmap.0), size_of::<BITMAP>() as i32, Some((&mut info as *mut BITMAP).cast())) };
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        if read == 0 || !(1..=max_side).contains(&w) || !(1..=max_side).contains(&h) {
            return None;
        }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: rows top to bottom.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        // SAFETY: `bgra` holds w*h 32-bit pixels, as `header` asks for.
        let lines = unsafe {
            let dc = GetDC(None);
            let lines = GetDIBits(dc, bitmap, 0, h as u32, Some(bgra.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS);
            ReleaseDC(None, dc);
            lines
        };
        if lines != h {
            return None;
        }
        Some(([w as usize, h as usize], to_rgba(bgra)))
    }

    impl ShellMenu {
        /// Runs the item at `offset` from the first id, as a click would.
        pub fn invoke(&self, offset: u32, window: Option<isize>) -> Result<(), String> {
            let folder: Vec<u16> = self.folder.as_os_str().encode_wide().chain([0]).collect();
            let info = CMINVOKECOMMANDINFOEX {
                cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: CMIC_MASK_UNICODE,
                hwnd: hwnd(window),
                // An id, not a verb: MAKEINTRESOURCE(offset).
                lpVerb: PCSTR(offset as usize as *const u8),
                lpVerbW: PCWSTR(offset as usize as *const u16),
                lpDirectoryW: PCWSTR(folder.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            // SAFETY: `info` and `folder` live through the call; the EX struct
            // starts like the plain one, as the shell expects with cbSize.
            match unsafe { self.menu.InvokeCommand((&info as *const CMINVOKECOMMANDINFOEX).cast::<CMINVOKECOMMANDINFO>()) } {
                Ok(()) => Ok(()),
                // A prompt the user declined is not an error.
                Err(e) if e.code() == windows::Win32::Foundation::ERROR_CANCELLED.to_hresult() => Ok(()),
                Err(e) => Err(format!("The command did not run: {e}")),
            }
        }
    }

    /// Messages the shell's handlers expect on their thread while their menu
    /// is alive, and after one of its items ran.
    const PUMP: std::time::Duration = std::time::Duration::from_millis(15);
    /// How long the thread keeps pumping after an item ran, for handlers that
    /// finish their work through its messages.
    const AFTER_INVOKE: std::time::Duration = std::time::Duration::from_secs(5);

    /// A request to run the item at an offset, and where its outcome goes.
    type Run = (u32, Option<isize>, std::sync::mpsc::Sender<Result<(), String>>);

    enum Target {
        Entries(Vec<PathBuf>, PathBuf),
        Folder(PathBuf),
    }

    /// The shell's menu, asked for on a thread of its own: installed
    /// extensions take up to seconds to answer, and Lumen's menu shows at
    /// once meanwhile. The menu lives on that thread, where its items run,
    /// until one runs or this is dropped.
    pub struct Loader {
        items: std::sync::mpsc::Receiver<Result<Vec<Item>, String>>,
        invoke: std::sync::mpsc::Sender<Run>,
    }

    impl Loader {
        /// The menu for `paths`, which share `folder`. `wake` runs on the
        /// loader's thread once the items are ready (or failed).
        pub fn entries(paths: Vec<PathBuf>, folder: PathBuf, window: Option<isize>, wake: impl Fn() + Send + Sync + 'static) -> Self {
            Self::start(Target::Entries(paths, folder), window, wake)
        }

        /// The menu for the empty space of `folder`.
        pub fn folder(folder: PathBuf, window: Option<isize>, wake: impl Fn() + Send + Sync + 'static) -> Self {
            Self::start(Target::Folder(folder), window, wake)
        }

        fn start(target: Target, window: Option<isize>, wake: impl Fn() + Send + Sync + 'static) -> Self {
            let (items_tx, items) = std::sync::mpsc::channel();
            let (invoke, requests) = std::sync::mpsc::channel::<Run>();
            let wake = std::sync::Arc::new(wake);
            let woken = wake.clone();
            let spawned = std::thread::Builder::new().name("lumen-shell-menu".into()).spawn(move || {
                // SAFETY: OLE for this thread (handlers may use the clipboard or
                // drag and drop); undone below, after the menu is gone.
                let _ = unsafe { windows::Win32::System::Ole::OleInitialize(None) };
                let built = match &target {
                    Target::Entries(paths, folder) => for_entries(paths, folder, window),
                    Target::Folder(folder) => for_folder(folder, window),
                };
                let shell = match built {
                    Ok((shell, list)) => {
                        let _ = items_tx.send(Ok(list));
                        Some(shell)
                    }
                    Err(e) => {
                        let _ = items_tx.send(Err(e));
                        None
                    }
                };
                (*wake)();
                if let Some(shell) = shell {
                    loop {
                        pump();
                        match requests.recv_timeout(PUMP) {
                            Ok((offset, window, done)) => {
                                let _ = done.send(shell.invoke(offset, window));
                                // Its error, if any, shows without waiting for input.
                                (*wake)();
                                drop(shell);
                                // A window the item left open on this thread (not a
                                // modal dialog: those return first) lives as long as it.
                                let until = std::time::Instant::now() + AFTER_INVOKE;
                                while std::time::Instant::now() < until || thread_has_windows() {
                                    pump();
                                    std::thread::sleep(PUMP);
                                }
                                break;
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            // The menu closed without running a shell item.
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                }
                // SAFETY: what this thread put on the clipboard outlives it;
                // then OLE ends, nothing of the shell's is left on this thread.
                unsafe {
                    let _ = windows::Win32::System::Ole::OleFlushClipboard();
                    windows::Win32::System::Ole::OleUninitialize();
                }
            });
            if let Err(e) = spawned {
                let (tx, rx) = std::sync::mpsc::channel();
                let _ = tx.send(Err(format!("The system menu is not available (thread: {e})")));
                (*woken)();
                return Self { items: rx, invoke: std::sync::mpsc::channel().0 };
            }
            Self { items, invoke }
        }

        /// The shell's items once they are ready; `None` while loading.
        pub fn items(&self) -> Option<Result<Vec<Item>, String>> {
            match self.items.try_recv() {
                Ok(items) => Some(items),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("The system menu is not available".into())),
            }
        }

        /// Runs the item at `offset` on the menu's thread. The answer comes
        /// once it ran (a dialog it opens may take a while).
        pub fn invoke(self, offset: u32, window: Option<isize>) -> std::sync::mpsc::Receiver<Result<(), String>> {
            let (done, answer) = std::sync::mpsc::channel();
            if let Err(std::sync::mpsc::SendError((_, _, done))) = self.invoke.send((offset, window, done)) {
                let _ = done.send(Err("The system menu is no longer available".into()));
            }
            answer
        }
    }

    /// Whether this thread shows a window (OLE's own hidden ones do not count).
    fn thread_has_windows() -> bool {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible};
        unsafe extern "system" fn found(window: HWND, any: LPARAM) -> windows::core::BOOL {
            // SAFETY: `window` comes from the enumeration; `any` points at the
            // bool below, alive during it.
            unsafe {
                if IsWindowVisible(window).as_bool() {
                    *(any.0 as *mut bool) = true;
                    return windows::core::BOOL(0);
                }
            }
            windows::core::BOOL(1)
        }
        let mut any = false;
        // SAFETY: the callback writes only through the pointer given here.
        let _ = unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(found), LPARAM(&mut any as *mut bool as isize)) };
        any
    }

    #[test]
    fn a_thread_with_ole_and_a_built_menu_shows_no_window_so_it_can_end() {
        let dir = crate::fs::ops::tests::fresh_fixture("shell-menu-thread");
        std::thread::spawn(move || {
            // SAFETY: as the loader's thread does.
            let _ = unsafe { windows::Win32::System::Ole::OleInitialize(None) };
            let menu = for_folder(&dir, None).expect("the shell answers");
            pump();
            assert!(!thread_has_windows(), "OLE's and the menu's hidden windows do not keep it alive");
            drop(menu);
            // SAFETY: undoes the call above.
            unsafe { windows::Win32::System::Ole::OleUninitialize() };
        })
        .join()
        .unwrap();
    }

    /// Hands this thread's waiting messages to their windows.
    fn pump() {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};
        let mut msg = MSG::default();
        // SAFETY: the plain message loop, for this thread's own windows.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
