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
| Ctrl+L | Edit the path bar (Enter to go, Esc to cancel; `~` is your home) |
| Ctrl+H | Show / hide hidden files |
| Alt+← / Alt+→ | Back / forward through visited folders |
| Type text | Filter the list to names containing it (ignores case; shown at the bottom left) |
| Space | Open / close the preview panel (with text in the filter, Space is just a space) |
| Delete | Move the selected entry to the trash (asks first; Enter confirms, Esc cancels) |
| F2 | Rename the selected entry in place (Enter to apply, Esc to cancel) |
| Ctrl+Shift+N | Create "New Folder" here and rename it |
| Ctrl+C / Ctrl+X | Copy / cut the selected entry |
| Ctrl+V | Paste into the current folder |
| Esc | Clear the filter; with an empty filter, cancel the copy or move in progress (holding it never does both) |

Click the Name, Size or Modified header to sort by that column; click it again to reverse. Folders always stay on top.

Lumen never deletes permanently: Delete moves entries to the system trash, and renaming or pasting onto an existing name asks first and sends the old entry to the trash.

Copy and paste use Lumen's own clipboard (not shared with other apps). Pasting into the same folder creates "name (copy)", "name (copy 2)"... Cut + paste moves: a rename on the same disk, or a copy followed by moving the original to the trash across disks. Copies and moves run in the background with progress in the status bar; cancelling keeps what was already copied and removes the half-written file.

The preview panel follows the selection: a folder shows its contents, text and code files show their first 256 KB as plain text, PNG and JPEG images are shown scaled to fit (files up to 50 MB, and pictures that would need more than 192 MB of RAM are skipped), and anything else shows its size, kind and date. Previews load in the background, and special files such as pipes are never opened.

The open folder reloads by itself when something changes on disk, keeping the selection and the sort order.

The status bar shows the item count and how long the last listing took.
Listing always runs on a background thread, so the window never freezes.
