# Lumen

A fast, lightweight, keyboard-first file explorer written in Rust with eframe/egui.

## Build and run

```sh
cargo run                      # debug build, opens the current directory
cargo run -- /usr/bin          # open a specific folder
cargo build --release          # optimized, size-focused binary at target/release/lumen
LUMEN_LOG=1 cargo run          # print how long each folder listing takes
cargo test                     # unit tests (fixtures live in ~/pruebas-explorador)
```

## Keyboard shortcuts

| Key | Action |
|---|---|
| ↑ / ↓ | Move selection |
| PageUp / PageDown | Move 20 rows |
| Home / End | First / last entry |
| Enter (or double click) | Enter folder / open file with the default app |
| Backspace | Erase the last filter character, or go to the parent folder when the filter is empty (holding it stops once the filter is empty) |
| Ctrl+L | Edit the path instead of the crumbs, starting from the current one (Enter to go, Esc to cancel; `~` is your home); clicking the empty part of the crumb bar does the same |
| Ctrl+T | Open the current folder in a new tab (also the + in the tab bar) |
| Ctrl+W | Close the tab (also its ×); the last tab stays open |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+D | Bookmark the current folder, or remove its bookmark (same as the ☆ button) |
| Ctrl+H | Show / hide hidden files |
| Alt+← / Alt+→ | Back / forward through visited folders |
| Type text | Filter the list to names containing it (ignores case; shown at the bottom left) |
| Space | Open / close the preview panel (with text in the filter, Space is just a space) |
| Delete | Move the selected entry to the trash (asks first; Enter confirms, Esc cancels) |
| F2 | Rename the selected entry in place (Enter to apply, Esc to cancel) |
| Ctrl+Shift+N | Create "New Folder" here and rename it |
| Ctrl+C / Ctrl+X | Copy / cut the selected entry |
| Ctrl+V | Paste into the current folder |
| Alt+R | Count the selected folder: its size and how many folders and files it holds, all the way down (runs in the background; symlinks are not followed and other disks are not entered). Folders show `--` until counted; the result stays while you are in this folder |
| Esc | Clear the filter; with an empty filter, stop the Alt+R count, then cancel the copy or move in progress (holding it stops at the first thing it undid) |

Each tab keeps its own folder, history, filter and selection; a tab is reloaded when you come back to it.

The side panel on the left (☰ shows or hides it) lists your **Bookmarks** (right-click one to remove it), **Places** (home, Desktop, Downloads, Documents, Music, Pictures and Videos as named on your system, plus the Trash), **Storage** (mounted disks with a bar showing how full they are) and **Recents** (the last 10 folders you visited in Lumen). Nothing inside the Trash can be trashed again.

The toolbar has back, forward and up buttons, the bookmark star and the current path as crumbs (`angelbyte › Descargas`): click a crumb to go there. Inside your home folder the crumbs start at it; elsewhere they start at `/`.

The list and the folder preview show Name, Type, Date Modified, Folders, Files and Size. Click a header to sort by that column; click it again to reverse. Folders always stay on top; counted folders sort by their totals.

Lumen never deletes permanently: Delete moves entries to the system trash, and renaming or pasting onto an existing name asks first and sends the old entry to the trash.

Copy and paste use Lumen's own clipboard (not shared with other apps). Pasting into the same folder creates "name (copy)", "name (copy 2)"... Cut + paste moves: a rename on the same disk, or a copy followed by moving the original to the trash across disks. Copies and moves run in the background with progress in the status bar; cancelling keeps what was already copied and removes the half-written file.

The preview panel follows the selection: a folder shows its contents, text and code files show their first 256 KB as plain text, PNG and JPEG images are shown scaled to fit (files up to 50 MB, and pictures that would need more than 192 MB of RAM are skipped), and anything else shows its size, kind and date. Previews load in the background, and special files such as pipes are never opened.

The open folder reloads by itself when something changes on disk, keeping the selection and the sort order.

Settings are kept in `~/.config/lumen/config` (or `$XDG_CONFIG_HOME/lumen/config`): bookmarks, sort order, Ctrl+H, whether the side panel and the preview are open, and the window size. They are written as soon as they change (the window size when Lumen closes). It is a plain `key = value` file; lines Lumen does not understand are ignored. If the file exists but cannot be read, Lumen says so and saves nothing that session, so the file is never overwritten. With two Lumen windows open, each one writes only what it changed, so neither loses the other's bookmarks. Open tabs are not restored yet.

The status bar shows the filter on the left ("Filter 9 files…" while empty), how long the last listing took, and on the right the folders shown and the selected row out of the rows shown ("1/9").
Listing always runs on a background thread, so the window never freezes.
