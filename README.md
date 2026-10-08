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

## Install (Linux)

```sh
scripts/install-desktop.sh             # build release, add Lumen to the app menu
scripts/install-desktop.sh --uninstall # remove it again
```

No sudo: the binary goes to `~/.local/bin/lumen`, the menu entry to
`~/.local/share/applications/lumen.desktop` and the icon to
`~/.local/share/icons/hicolor`. Lumen then shows up as "Lumen" in the GNOME
app grid, and can be pinned to the dock.

The icon is `assets/lumen.svg`; after editing it, run
`python3 scripts/render_icon.py` to regenerate `assets/lumen.png`, which the
binary uses as its window icon.

## Windows

Every push to `main` or a `feat/*` branch runs the "Windows build" workflow in
GitHub Actions: it runs the tests on Windows and keeps `lumen.exe` as a
download (Actions tab, open a run, Artifacts, `lumen-windows`). The `.exe`
needs no installer; double-click it to start Lumen. To check the code compiles
for Windows locally:

```sh
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu --all-targets
```

On Windows:

- Started without a folder, Lumen opens your user folder (Windows hands apps
  a working directory such as `C:\Windows\System32`). On Linux it opens the
  folder it was started from.
- Storage in the side panel lists local and USB drives with their free space;
  network and CD drives are left out.
- Files open as a double-click in File Explorer does: Windows asks before
  opening a downloaded file only when its type is dangerous (`.exe`, `.bat`…),
  not for a downloaded image or document.
- Settings are saved in `%USERPROFILE%\.config\lumen\config`.

## Releases

Artifacts expire and need a GitHub login; a release is a public, permanent
download. To publish one, set `version` in `Cargo.toml`, commit, then push a
matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The "Release" workflow tests on Windows, builds `lumen-v0.1.0-windows-x86_64.exe`
and creates the GitHub Release with it and notes generated from the commits.
A tag that does not match `Cargo.toml` fails without publishing anything
(`scripts/check-release-tag.sh`, tested by `scripts/test-release-tag.sh`).

## Keyboard shortcuts

| Key | Action |
|---|---|
| ↑ / ↓ | Move selection |
| PageUp / PageDown | Move 20 rows |
| Home / End | First / last entry |
| Shift+↑ / Shift+↓, Shift+PageUp / PageDown, Shift+Home / End | Select the range from where the selection started (also Shift+click) |
| Ctrl+A | Select every row shown (with text in the filter, only the rows it left) |
| Ctrl+click | Add a row to the selection, or take it out |
| Enter (or double click) | Enter folder / open file with the default app |
| Backspace | Erase the last filter character, or go to the parent folder when the filter is empty (holding it stops once the filter is empty) |
| Ctrl+L | Open the path box, centred on the window, with the current path selected (type a name to replace it): it lists what is in the folder typed so far, narrowed by the words you type, in any order ("tareas revision" finds "revision tareas.pdf"), with the matching letters highlighted, a name typed in full listed first, and the first match picked. Up/Down pick a row, Enter or a click opens it (a file in its app) or the typed path (`~` is your home), Tab completes it, Ctrl+Enter opens it in a new tab (a file: its folder, with the file selected), Escape closes. Clicking the empty part of the crumb bar does the same |
| Ctrl+T | Open the current folder in a new tab (also the + in the tab bar) |
| Ctrl+W | Close the tab (also its ×); the last tab stays open |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+D | Bookmark the current folder, or remove its bookmark (same as the ☆ button) |
| Ctrl+H | Show / hide hidden files |
| Alt+← / Alt+→ | Back / forward through visited folders |
| Alt+↑ | Go to the parent folder, selecting the folder you came from (works with text in the filter) |
| Type text | Filter the list to names containing every typed word, in any order, ignoring case ("slides contenido" finds "Slides de contenido"); a name typed in full comes first; the matched letters are highlighted and the text is shown at the bottom left |
| Space | Open / close the preview panel (with text in the filter, Space starts another word) |
| Delete | Move the selected entry to the trash (asks first; Enter confirms, Esc cancels) |
| F2 | Rename the selected entry in place (Enter to apply, Esc to cancel) |
| Ctrl+Shift+N | Create "New Folder" here and rename it |
| Ctrl+C / Ctrl+X | Copy / cut the selected entry |
| Ctrl+V | Paste into the current folder |
| Alt+R | Count the selected folder: its size and how many folders and files it holds, all the way down (runs in the background; symlinks are not followed and other disks are not entered). Folders show `--` until counted; the result stays while you are in this folder |
| Ctrl+Plus / Ctrl+Minus, Ctrl+wheel | Bigger / smaller icons and rows in the list (5 sizes); the text, side panel and toolbar keep their size |
| Ctrl+0 | Icons and rows back to the normal size |
| Esc | Clear the filter; with an empty filter, drop a multiple selection, then stop the Alt+R count, then cancel the copy or move in progress (holding it stops at the first thing it undid) |

Each tab keeps its own folder, history, filter and selection; a tab is reloaded when you come back to it.

The side panel on the left (☰ shows or hides it) lists your **Bookmarks** (right-click one to remove it), **Places** (home, Desktop, Downloads, Documents, Music, Pictures and Videos as named on your system, plus the Trash), **Storage** (mounted disks with a bar showing how full they are; hover a disk for its free space) and **Recents** (the last 10 folders you visited in Lumen). Nothing inside the Trash can be trashed again.

The toolbar has back, forward and up buttons, the bookmark star and the current path as crumbs (`angelbyte › Descargas`): click a crumb to go there. Inside your home folder the crumbs start at it; elsewhere they start at `/`.

The list and the folder preview show Name, Type, Date Modified, Folders, Files and Size. Click a header to sort by that column; click it again to reverse. Folders always stay on top; counted folders sort by their totals.

Lumen never deletes permanently: Delete moves entries to the system trash, and renaming or pasting onto an existing name asks first and sends the old entry to the trash.

Copy and paste use Lumen's own clipboard (not shared with other apps). Pasting into the same folder creates "name (copy)", "name (copy 2)"... Cut + paste moves: a rename on the same disk, or a copy followed by moving the original to the trash across disks. Copies and moves run in the background with progress in the status bar; cancelling keeps what was already copied and removes the half-written file.

The preview panel follows the selection: a folder shows its contents, text and code files show their first 256 KB as plain text, PNG and JPEG images are shown scaled to fit (files up to 50 MB, and pictures that would need more than 192 MB of RAM are skipped), and anything else shows its size, kind and date. Previews load in the background, and special files such as pipes are never opened.

The open folder reloads by itself when something changes on disk, keeping the selection and the sort order.

Settings are kept in `~/.config/lumen/config` (or `$XDG_CONFIG_HOME/lumen/config`): bookmarks, sort order, Ctrl+H, whether the side panel and the preview are open, the list zoom and the window size. They are written as soon as they change (the window size when Lumen closes). It is a plain `key = value` file; lines Lumen does not understand are ignored. If the file exists but cannot be read, Lumen says so and saves nothing that session, so the file is never overwritten. With two Lumen windows open, each one writes only what it changed, so neither loses the other's bookmarks. Open tabs are not restored yet.

The status bar shows the filter on the left ("Filter 9 files…" while empty, with a text cursor after the typed text), how long the last listing took, and on the right the folders shown and the selected row out of the rows shown ("1/9").
Listing always runs on a background thread, so the window never freezes.
