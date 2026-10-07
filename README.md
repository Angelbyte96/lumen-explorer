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
| Backspace | Go to parent folder |
| Ctrl+L | Edit the path bar (Enter to go, Esc to cancel; `~` is your home) |
| Ctrl+H | Show / hide hidden files |
| Alt+← / Alt+→ | Back / forward through visited folders |
| Type letters | Jump to the first entry starting with them (a 1 s pause starts over) |
| Delete | Move the selected entry to the trash (asks first; Enter confirms, Esc cancels) |
| F2 | Rename the selected entry in place (Enter to apply, Esc to cancel) |
| Ctrl+Shift+N | Create "New Folder" here and rename it |

Click the Name, Size or Modified header to sort by that column; click it again to reverse. Folders always stay on top.

Lumen never deletes permanently: Delete moves entries to the system trash, and renaming onto an existing name asks first and sends the old entry to the trash.

The status bar shows the item count and how long the last listing took.
Listing always runs on a background thread, so the window never freezes.
