# Lumen

A fast, lightweight, keyboard-first file explorer written in Rust with eframe/egui.

## Download

Lumen is free and open source (MIT). Download it from
[GitHub Releases](https://github.com/Angelbyte96/lumen-explorer/releases):

- `lumen-<version>-windows-x86_64.exe`: Windows 10/11, installs itself for your
  user (see [Lumen Setup](#lumen-setup)).
- `lumen_<version>_amd64.deb`: Ubuntu 24.04 or later, Debian 13.
- `lumen-<version>-linux-x86_64.tar.gz`: any x86_64 Linux.

Every release file is built from this repository by the public "Release"
workflow in GitHub Actions ([`.github/workflows/release.yml`](.github/workflows/release.yml)).
See the [Code signing policy](#code-signing-policy).

## Build and run

```sh
cargo run                      # debug build, opens the current directory
cargo run -- /usr/bin          # open a specific folder
cargo build --release          # optimized, size-focused binary at target/release/lumen
LUMEN_LOG=1 cargo run          # print how long each folder listing takes
cargo test                     # unit tests (fixtures live in ~/pruebas-explorador)
```

## Lumen Setup

Lumen installs itself, like File Pilot, for your user only: no admin rights on
Windows, no sudo on Linux. ☰ (F10) ▸ Lumen Setup shows the installed version
and folder and four switches; Modify applies them:

| Switch | Windows | Linux |
|---|---|---|
| Desktop shortcut | `Lumen.lnk` on the desktop | `lumen.desktop` in the desktop folder, marked trusted |
| Set as default | folders, drives and Win+E open in Lumen (HKCU) | `inode/directory=lumen.desktop` in `~/.config/mimeapps.list` |
| Right-click entries | "Open in Lumen" on folders, drives and a folder's background | the Nautilus script "Open in Lumen" (right click ▸ Scripts) |
| Add to PATH | the install folder in your user `Path` | the link `~/.local/bin/lumen` |

Uninstall removes Lumen, its menu entry and every switch; your settings
(bookmarks, tabs) stay. Turning Set as default off gives folders back to the
desktop's own file manager.

To install a release:

- Windows: double-click `lumen-<version>-windows-x86_64.exe`. The first time,
  Lumen opens Setup: Install copies it to `%LOCALAPPDATA%\Lumen` and adds it
  to the Start menu and to Settings ▸ Apps, where Uninstall opens Setup again.
- Linux: a downloaded program cannot be started with a double click, so the
  first start is from a terminal, once:

  ```sh
  tar xzf lumen-<version>-linux-x86_64.tar.gz
  ./lumen/lumen     # opens Setup; Install copies it to ~/.local/share/lumen
  ```

  From then on Lumen is in the app menu.

With the `.deb` (below) apt owns Lumen: Setup changes the switches but neither
installs nor uninstalls; it shows `sudo apt remove lumen` instead, and Add to
PATH is always on (`/usr/bin`).

## Install from source (Linux)

```sh
scripts/install-desktop.sh             # build release, add Lumen to the app menu
scripts/install-desktop.sh --uninstall # remove it again
```

It shows each step as it goes and ends with `Done: Lumen <version> (<commit>)
is installed at ...`, the build you will get from the menu; a failed step ends
with `Lumen was NOT installed`.

No sudo: the binary goes to `~/.local/bin/lumen`, the menu entry to
`~/.local/share/applications/lumen.desktop` and the icon to
`~/.local/share/icons/hicolor`. Lumen then shows up as "Lumen" in the GNOME
app grid, and can be pinned to the dock.

Or, system-wide, the `.deb` of a release (built on Ubuntu 24.04: it installs
on Ubuntu 24.04 or later and Debian 13):

```sh
sudo apt install ./lumen_<version>_amd64.deb   # sudo apt remove lumen to remove it
```

It puts the binary at `/usr/bin/lumen`, with the same menu entry and icon.
To build one yourself from a release build:

```sh
scripts/build-deb.sh target/release/lumen <out dir>  # prints Built <out dir>/lumen_<version>_<arch>.deb
scripts/test-deb.sh "$(mktemp -d)"                   # checks the package, no root needed
```

A package built locally needs your system's glibc or newer.

The icon is `assets/lumen.svg`; after editing it, run
`python3 scripts/render_icon.py` (needs Pillow) to regenerate `assets/lumen.png`,
which the binary uses as its window icon, and `assets/lumen.ico`, which
`build.rs` puts into the Windows `.exe` so File Explorer shows it for the file.

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

- Started without a folder, Lumen reopens the tabs of the last window you
  closed (a folder that is gone opens its nearest parent, else your user
  folder), never the working directory Windows hands apps (such as
  `C:\Windows\System32`). On Linux the app menu entry does the same
  (`lumen --restore`), and so does a double click on the program in a file
  manager (no terminal), while `lumen` from a terminal opens the folder it
  was started from. A folder given to Lumen always opens alone.
- Storage in the side panel lists local and USB drives with their free space;
  CD drives are left out. On Windows it also lists network drives (a mapped
  letter such as "Ubuntu (\\wsl$) (Z:)"), as Network Drive with their file
  system and space; one that cannot be read is listed without numbers. The
  drives are read in the background, so a share that is down or slow never
  stalls the window: its rows show once it answers, and the last rows stay
  meanwhile. On Windows a drive plugged in or ejected shows up (here and in
  This PC) within about a second; free space is read every 5 seconds.
- Files open as a double-click in File Explorer does: Windows asks before
  opening a downloaded file only when its type is dangerous (`.exe`, `.bat`…),
  not for a downloaded image or document.
- Settings are saved in `%APPDATA%\lumen\config`. A settings file left by
  Lumen 0.1 in `%USERPROFILE%\.config\lumen\config` is moved there the first
  time; if it cannot be moved, Lumen keeps using it where it is.

## Releases

Artifacts expire and need a GitHub login; a release is a public, permanent
download. To publish one, set `version` in `Cargo.toml`, commit, then push a
matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The "Release" workflow tests on Windows and Ubuntu, builds
`lumen-v0.1.0-windows-x86_64.exe`, `lumen_0.1.0_amd64.deb` and
`lumen-v0.1.0-linux-x86_64.tar.gz`, and creates the GitHub Release with them,
a link to the code signing policy and notes generated from the commits.
A tag that does not match `Cargo.toml` fails without publishing anything
(`scripts/check-release-tag.sh`, tested by `scripts/test-release-tag.sh`).

## Code signing policy

Windows releases will be signed through the SignPath Foundation program
(application pending; until then the `.exe` is unsigned and Windows may warn
before running it). Once approved:

Free code signing provided by [SignPath.io](https://about.signpath.io),
certificate by [SignPath Foundation](https://signpath.org).

- What is signed: the Windows `.exe` published on GitHub Releases. Only files
  built by the "Release" workflow from this repository are submitted for
  signing; nothing built on a personal machine is signed. The private key is
  held by SignPath, never by this project.
- Linux files (`.deb`, `.tar.gz`) are not signed; get them only from
  [GitHub Releases](https://github.com/Angelbyte96/lumen-explorer/releases).

Team roles (single-maintainer project):

- Committers and reviewers: [Angelbyte96](https://github.com/Angelbyte96).
  Every change reaches `main` through a pull request that must pass the
  Windows tests; pull requests from other people are reviewed by the
  maintainer before merging.
- Approvers: [Angelbyte96](https://github.com/Angelbyte96). Each signing
  request is approved by hand.

Privacy policy: this program will not transfer any information to other
networked systems unless specifically requested by the user or the person
installing or operating it. Lumen has no telemetry and no update check. Video
thumbnails use `ffmpeg`/`ffprobe` when installed on Linux, run locally on the
file shown.

## Keyboard shortcuts

| Key | Action |
|---|---|
| ↑ / ↓ | Move selection (in Icons, to the card above / below; in List, along the column and on into the next) |
| ← / → | Tree view: → unfolds a folded folder, and on an unfolded one goes to its first child; ← folds an unfolded folder, and on a child goes to its parent folder. In Icons they go to the card on the left / right; in List to the column on the left / right; in Details they do nothing |
| PageUp / PageDown | Move 20 rows |
| Home / End | First / last entry |
| Shift+↑ / Shift+↓, Shift+PageUp / PageDown, Shift+Home / End | Select the range from where the selection started (also Shift+click) |
| Ctrl+A | Select every row shown (with text in the filter, only the rows it left) |
| Ctrl+click | Add a row to the selection, or take it out |
| Click below the rows (or beside the cards), or Esc with nothing else to undo | Unselect everything: the cursor stays on its entry with a dashed outline, and Delete, F2 or Enter do nothing until a click or an arrow selects again |
| Drag from below the rows, or from a row that is not selected outside its icon and name | Select every row the band touches; held near the list's top or bottom edge (or past it), the list scrolls, faster closer to the edge. Hold Shift as you start a band to add its rows to what is already selected, for several separate groups. In Icons and List, drag from the space around the cards: every card the band touches is picked, and near an edge the view scrolls (sideways in List) |
| Drag a row's icon or name, anywhere on a selected row, a card or a cell | Drag the selection (or that entry alone when it is not selected) onto a folder: a folder row or card, anything else in the list (a file, the gaps, the empty space: the folder on screen), a bookmark, place or disk in the side panel (not the Trash, nor a drive's `$Recycle.Bin` on Windows), or a tab. On the same disk it moves, onto another disk it copies; hold Ctrl to copy or Shift to move. The label by the pointer says what will happen; Esc cancels. Held near the list's edge it scrolls. On Wayland, carried past the window's edge it becomes a drag to other apps: a file manager copies or moves the files, a browser upload page or a sandboxed (Flatpak) app gets them (through the document portal), a terminal or editor gets their paths or opens them; pressing Alt with the button held hands it over at once, so it lives through Alt+Tab to another app (inside Lumen the folders still light up and take it). On Windows, past the window's edge, or when you Alt+Tab to another app with the button held, it becomes a system drag, as from File Explorer: the files show by the pointer, a browser upload page (Google Photos, Drive) gets them, File Explorer moves them on the same disk and copies them onto another (Ctrl/Shift as there), Esc cancels |
| Drop files from another app (Wayland) | They land on the folder under the pointer as a drag inside Lumen does (the folder lights and the label names it); on a file or the empty space, in the folder on screen. The other app's Ctrl/Shift decide copy or move; without keys, the same disk moves and another disk copies |
| Right click, Shift+F10, Menu key | The menu for the selection (a row outside it becomes the selection) or, below the rows, for the folder: type to search it ("sort size"), ↑/↓ to move, → / ← to open or close a submenu, Enter or a click to run, Esc to close. On Linux, "Open With" lists the apps installed for the entry's type (as GNOME and KDE read them), launched with `gio launch`; "Open Terminal Here" starts $TERMINAL or the first terminal installed; New ▸ creates a folder, an empty file or a copy of a file from your Templates folder (~/Templates, or its name in your language), never over an existing name, and starts renaming it. On Windows the menu holds the shell's own items, as File Explorer shows them (Open with, Send to, 7-Zip, PowerRename… whatever that PC has installed), with their icons, followed by Lumen's own; Cut, Copy, Paste, Delete and Rename there use Lumen's clipboard, trash and in-place rename. Right click on a side panel row (bookmark, place, disk or recent folder) opens the menu for that folder, not for the list's selection: Open, Open in New Tab, Copy Path and Add to / Remove from Bookmarks, and on Windows the shell's own items for it too (a USB drive shows Eject, Properties…) |
| Enter (or double click) | Enter folder / open file with the default app |
| Backspace | Erase the last filter character, or go to the parent folder when the filter is empty (holding it stops once the filter is empty) |
| Ctrl+L | Open the path box, centred on the window, with the current path selected (type a name to replace it): it lists what is in the folder typed so far, narrowed by the words you type, in any order ("tareas revision" finds "revision tareas.pdf"), with the matching letters highlighted, a name typed in full listed first, and the first match picked. Up/Down pick a row, Enter or a click opens it (a file in its app) or the typed path (`~` is your home; on Windows `C:` is the drive's top, and `C:Us` lists it narrowed to `Users`), Tab completes it, Ctrl+Enter opens it in a new tab (a file: its folder, with the file selected), Escape closes. A word with no path (`down`, no `/`) searches your places instead, folders and files, letters in order ("down" finds "Windows NT"), ignoring case and accents ("videos" finds "Vídeos"), marked, each with its folder; folders you visit often come first. On Linux it searches your home folder, the other disks and the network shares (NFS, SMB, SSHFS, and those opened in Files); on Windows every drive and network drive. System and program folders are left out (`/usr`, `/etc` and the rest of `/`; `Windows`, `Program Files`, `ProgramData`, `AppData`), and so are `node_modules`, `.git`, `target` and `.cache`. It shows "Indexing…" with what it has found while the first scan runs, and when the box opens the index is scanned again in the background if it is over 10 minutes old or Lumen created, renamed, moved, pasted or deleted something since. Clicking the empty part of the crumb bar does the same |
| F10 | Open the app menu (also ☰ in the top-left corner) with Go To focused: Go To opens the path box (as Ctrl+L), Lumen Setup opens the [setup screen](#lumen-setup) (Esc or Back returns); Commands, Load Layout, Save Layout and Options are greyed until Lumen has them; the last line is the version. ↑/↓ move through the rows it has, Enter runs one, Esc or F10 again closes it. Hovering ☰ or the panel button beside it names its command and shortcut |
| Ctrl+B | Show / hide the side panel (also the panel button beside ☰) |
| Ctrl+T | Open the current folder in a new tab (also the + in the tab bar) |
| Ctrl+W | Close the tab (also its ×); the last tab stays open |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Tab / Shift+Tab | Move the keyboard focus through the window, a ring around the part that has it: ☰, the side panel button, each tab, +, ←, →, ↑, ☆ and the path bar, then the side panel, then the list, and round again (the list's ring goes away with the first key or click on it, the list keeping the keys; after a click on a row, Tab goes to the first tab and Shift+Tab to the side panel). Enter or Space presses a focused button (Enter on the path bar opens the path box). The side panel works like a tree: ↑/↓ and Home/End move through the section titles and the items of the open sections, → opens a closed section (on an open title: goes to its first item), ← goes from an item to its title or closes the section, Enter opens the folder or opens / closes the section. The list's keys (arrows, Enter, Delete, typing…) act on the list only while it has the focus; Esc or a click on the list gives it the keys back. Ctrl, Alt and F-key shortcuts work wherever the focus is |
| Ctrl+D | Bookmark the current folder, or remove its bookmark (same as the ☆ button) |
| Ctrl+H | Show / hide hidden files |
| Ctrl+E | Toggle subfolders: also list everything the subfolders hold, all the way down, sorted with the rest; each entry from a subfolder shows its folder dimmed after its name (`lumen.pdb > deps`); a click on that folder opens it with the entry selected, and the status bar counts every row. While the subfolders are read the status bar says "Listing subfolders… N items" (Esc stops it); a very big tree, such as a home folder, can take a while. With folders picked (Ctrl+click, Shift, Ctrl+A), only what those hold is added, or taken out again when they already are listed. Again with nothing picked, or opening another folder, lists the folder alone. Not on This PC or the Recycle Bin |
| F5 | Reload the open folder (subfolders listed or not), keeping the selection and the sort order; while subfolders are being listed it starts listing them again; on This PC and the Recycle Bin it reads the drives or the bins again |
| Alt+← / Alt+→ | Back / forward through visited folders |
| Alt+↑ | Go to the parent folder, selecting the folder you came from (works with text in the filter) |
| Type text | Filter the list to names containing every typed word, in any order, ignoring case ("slides contenido" finds "Slides de contenido"); a name typed in full comes first; the matched letters are highlighted and the text is shown at the bottom left |
| Space | Open / close the preview panel (with text in the filter, Space starts another word) |
| Delete | Move the selected entries to the trash (asks once; Enter confirms, Esc cancels) |
| Shift+Delete | Delete the selected entries for good, without the trash (asks once: "This cannot be undone"; a link is removed, never what it points to) |
| F2 | Rename the selected entry in place (Enter to apply, Esc to cancel); does nothing with several selected. Characters a name cannot hold are not typed (`/` on Linux; `\ / : * ? " < > |` on Windows), and a pasted name keeps the rest |
| Ctrl+Shift+N | Create "New Folder" here and rename it |
| Ctrl+C / Ctrl+X | Copy / cut the selected entries (cut ones are faded until pasted) |
| Ctrl+Shift+C | Copy the selected entries' paths as text, one per line (Copy Path) |
| Ctrl+V | Paste into the current folder, one entry after another in the background; a name already there asks to Replace (Enter, the old one goes to the trash) or Skip (Esc) |
| Alt+R | Count the selected folder: its size and how many folders and files it holds, all the way down (runs in the background; symlinks are not followed and other disks are not entered). Folders show `--` until counted; the result stays while you are in this folder |
| Ctrl+Plus / Ctrl+Minus, Ctrl+wheel | Move the view slider: Details, Tree, List, then icons that grow from M to L to XL (and back) |
| Ctrl+0 | Back to the Details view |
| Ctrl+Shift+F | Size every list column to fit the list's width, no sideways scroll (also in the header's right-click menu) |
| Esc | Clear the filter; with an empty filter, drop a multiple selection, then stop the Alt+R count, then stop listing the subfolders (Ctrl+E) and list the folder alone, then cancel the copy or move in progress with the entries still waiting, then forget a cut waiting to be pasted, then unselect as a click on the empty space does (holding it stops at the first thing it undid) |

Each tab keeps its own folder, history, filter and selection; a tab is reloaded when you come back to it.

The side panel on the left (the panel button beside ☰, or Ctrl+B, shows or hides it) lists your **Bookmarks** (right-click one for its menu, where "Remove from Bookmarks" removes it), **Places** (on Windows **This PC** first, then home, Desktop, Downloads, Documents, Music, Pictures and Videos as named on your system, and the **Recycle Bin** last; on Linux the Trash instead), **Storage** (mounted disks with a bar showing how full they are; hover a disk for its free space) and **Recents** (the last 10 folders you visited in Lumen). Right-click any row of the panel for its menu (open it, open it in a new tab, copy its path, bookmark it; on Windows also the shell's own items, such as Eject for a USB drive). Nothing inside the Trash can be trashed again.

On Windows, **This PC** opens a view of the drives the side panel's Storage lists (network drives included, as Network Drive), with their Name, Type (Local Disk, Removable Disk, Network Drive...), File System, Free Space, Size and Percent Full (a bar). Click a drive to select it, double-click it or press Enter to open it, and right-click it (or press the Menu key) for its menu, with Eject for a USB drive. Its headers work like the list's: click one to sort by it (click again to reverse; the arrow shows the order), drag a header border to resize a column (double-click it to fit the column on its left), and right-click a header, or press Ctrl+Shift+F, to size one column or all of them to fit (all of them inside the width of the view, sharing the room over the columns as the list does; Size is right-aligned, Free Space stays on the left so the two do not read as one). The columns also fit the view by themselves when it opens and when the window is resized, until you drag a border or size a single column; Size All Columns To Fit gives that back. The sort and the widths last until you quit. Back and forward come back to it, and the crumbs and the tab say "This PC".

On Windows, the **Recycle Bin** place opens a view of what your account has recycled on every drive Storage lists, under the names the entries had: Name, Original Location (the folder each came from), Type (as in the list: Folder, "PNG file"...), Date Deleted and Size. It reads each drive's `$Recycle.Bin` (your own SID's folder) in the background whenever you open it, come back to it or click its place again. Click an entry to select it, Ctrl+click to add or remove one and Shift+click (or Shift+arrows, Ctrl+A) to take a range; the headers sort, resize and fit as in This PC, including the automatic fit (when the room is short, Name shrinks first, then Original Location, cut with "…"; Type, Date Deleted and Size stay whole, like File Pilot; Size is right-aligned) (the view starts with the latest deleted first). Right-click the selection (or press the Menu key) for **Restore**, which puts the entries back where they were (the folder is made again if it is gone, and a name already taken is never overwritten: the entry comes back as "name (restored)", "name (restored 2)"...), and for **Delete Permanently**; Delete or Shift+Delete asks the same question ("This cannot be undone"). Errors show in the red line. Entering an entry opens nothing, and nothing can be dropped, pasted or created there.

The toolbar has back, forward and up buttons, the bookmark star and the current path as crumbs (`angelbyte › Descargas`): click a crumb to go there. Inside your home folder the crumbs start at it; elsewhere they start at `/`.

The list and the folder preview show Name, Type, Date Modified, Folders, Files and Size (the list with a faint line between the column headers; the row under the pointer, in the list, This PC, the Recycle Bin and the side panel, is lit in a see-through cyan). Click a header to sort by that column; click it again to reverse. Folders always stay on top; counted folders sort by their totals. Drag the border between two headers to resize a column (every column keeps its own width, the last one stretches to the list's edge, and columns wider than the window scroll sideways), or double-click it to fit the column on its left to its content, like Size Column To Fit; the widths are saved. Right-click a header for Shift Column Left / Right (past hidden columns), Size Column To Fit, Close Column (also a middle click on the header), Size All Columns To Fit (Ctrl+Shift+F: when every shown column fits, each gets the width of its content and the room left is shared over all of them in proportion to it, like File Pilot, instead of going all to Name; otherwise Type and Date Modified keep their full width, Folders, Files and Size shrink, and Name takes the rest, long names and headers cut with "…", so nothing scrolls sideways), Restore Default Columns (default widths and order, every column shown) and a checkbox per column to show or hide it; Name is always shown. Hidden columns and the order are saved too.

Lumen has six views, picked with the view button at the bottom right (hover or click it; drag its knob or click a name) or with Ctrl+wheel and Ctrl+Plus / Ctrl+Minus: **Details** (0%, the table), **Tree** (10%), **List** (20%: icon and name in columns filled top to bottom, folders in their own columns first, scrolling sideways) and **M / L / XL Icons** (30%, 65%, 100%: a grid of cards with the icon, the name and the size, folders on their own rows first). In the Icons views PNG and JPEG files show their own picture and videos a frame of theirs (see the preview panel below for where frames come from), and each folder shows what it holds sticking out of it: its first file's picture, or a sheet with that file's type icon, or blank sheets when it holds only folders (hidden files are left out); pictures and folder contents are read in the background, only for the cards on screen. From M to XL it is one view whose icons grow smoothly with the slider; the other steps are separate views and switch with a short fade. Each tab shows its own view.

Each folder opens with its saved view and sort order, or with the default. The lock button after ↑ opens "Saved view settings": **None** (the folder has no rule of its own), **This folder** (save the view and order shown for this folder alone) and **This folder and subfolders** (also for every folder below it that has no rule of its own; the menu names that folder when you are in one of them). While a rule covers the folder the lock is bright, and changing the view or the order there updates that rule. In a folder with no rule a change lasts only that visit: the next folder without a rule opens with the default again. **Set as default** makes the view and order shown the default; it is greyed when they already are.

The Tree view (10% on the view slider) is the same table with a chevron before each folder (`>` folded, `v` unfolded): click it, or press → / ←, to show what the folder holds right in the list, one step deeper, without entering it. The folder is read in the background; one that cannot be read shows nothing under it. What is unfolded follows the list's sort and Ctrl+H, is not touched by the filter (a folder the filter hides takes its children with it), is read again when the folder changes on disk, and is forgotten when you enter another folder or leave the Tree view. Click, Ctrl+click, Shift+click, the band, Delete, F2, Ctrl+C / Ctrl+X, the right-click menu, Alt+R and Space work on the children too, on their own paths; Enter or a double click on a folder still enters it.

Rows show the system's icons. On Windows, the shell's (a program's own icon, its type's program icon for other files). On Linux, your desktop's icon theme, as Nautilus and Dolphin show it: the theme GNOME names in its settings (KDE: kdeglobals), then the themes it inherits and hicolor, picking each file's icon by its type (from its extension, through the system's MIME tables) and special folders' own (Downloads, Documents…); PNG and SVG icons alike. Lumen's own drawn icons show until the theme's arrive, or when the theme has none. The side panel shows the same icons on Windows; on Linux the theme's one-colour (symbolic) icons in the text colour, as Nautilus does: home, Desktop, Downloads, Documents, Music, Pictures, Videos, the trash, disks, and a folder for bookmarks and recents.

Lumen deletes permanently only with Shift+Delete, after asking: Delete moves entries to the system trash, and renaming or pasting onto an existing name asks first and sends the old entry to the trash.

Copy and paste use Lumen's own clipboard (not shared with other apps). Pasting into the same folder creates "name (copy)", "name (copy 2)"... Cut + paste moves: a rename on the same disk, or a copy followed by moving the original to the trash across disks. Copies and moves run in the background with their progress floating at the bottom right; cancelling keeps what was already copied and removes the half-written file.

The preview panel follows the selection: a folder shows its contents, text and code files show their first 256 KB as plain text, PNG and JPEG images are shown scaled to fit (files up to 50 MB, and pictures that would need more than 192 MB of RAM are skipped), PDFs show their first page (drawn by `pdftoppm` from poppler-utils on Linux, which the panel asks you to install if it is missing; on Windows by Windows' own PDF renderer, no PDF app needed), videos (mp4, m4v, mkv, webm, mov, avi, wmv, flv, mpg, mpeg, 3gp, ogv) show a frame from a tenth of the way in, past a black or fading start (taken by `ffmpeg` on Linux, which the panel asks you to install if it is missing, with the video's size in pixels; on Windows the same thumbnail File Explorer shows, whose frame Windows picks, so a video whose format Windows cannot play has none), and anything else shows its size, kind and date. Previews load in the background, and special files such as pipes are never opened.

The open folder reloads by itself when something changes on disk, keeping the selection and the sort order; with subfolders listed (Ctrl+E), a change inside them is picked up too, for up to 5,000 folders: only the folders that changed are read again, in the background, and a new folder is listed with what it holds, so it does not list every subfolder again (F5, or a very large change at once, does). Past 5,000 folders the view is listed but does not reload by itself, and the status bar says so (press F5 to reload it).

Settings are kept in `~/.config/lumen/config` (or `$XDG_CONFIG_HOME/lumen/config`; on Windows `%APPDATA%\lumen\config`): bookmarks, the default sort order and view (`sort = name asc`, `view = 0`-`100`, the slider's percent), each folder's saved view (`folder_view = <folder|tree> <view> <sort column> <asc|desc> <path>`), Ctrl+H, whether the side panel and the preview are open, and the window size. They are written as soon as they change (the window size when Lumen closes). It is a plain `key = value` file; lines Lumen does not understand are ignored. If the file exists but cannot be read, Lumen says so and saves nothing that session, so the file is never overwritten. With two Lumen windows open, each one writes only what it changed, so neither loses the other's bookmarks or saved views. The folders of the open tabs are saved when a window closes (`tab = …` and `active_tab = …`): the last window closed is the one reopened. The folders you visit are scored for the quick jump and saved when a window closes too (`visited = <score> <time> <path>`, at most 200): each visit adds one and the score halves every two weeks.

Two islands float at the bottom of the list, like File Pilot's. The left one holds the filter ("Filter 9 files…" while empty, with a text cursor after the typed text). The right one holds how many rows are selected and their total size ("5 selected · 12.4 MB"; folders add their size once counted with Alt+R, and "+ 2 folders" tells the ones not counted yet) while you pick several, the folders shown, the selected row out of the rows shown ("1/9"; hover it for the item count and how long the last listing took), and the view button: its icon, name and percent. Hovering or clicking the view button opens the six views beside a slider (click a name or drag the knob); it closes when the pointer leaves it. Loading, copies, counts, notices and errors float above the right island while there is one; a notice ("Path copied") or an error goes away after 4 seconds.
Listing always runs on a background thread, so the window never freezes.
