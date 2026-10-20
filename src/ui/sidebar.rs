//! The left panel: Bookmarks, Places, Storage and Recents, each collapsible.
//! A click on an item opens it in the list; a right click asks for its menu.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::format::format_size;
use crate::places::{self, Mount, Place, Usage};
use crate::ui::file_icons::FileIcons;
use crate::ui::icons;

pub const TOGGLE: &str = "☰";
pub const DISK: &str = "🖴";
pub const COMPUTER: &str = "🖥";
pub const TRASH: &str = "🗑";

pub struct Sidebar<'a> {
    pub current: &'a Path,
    pub bookmarks: &'a [PathBuf],
    pub places: &'a [Place],
    pub disks: &'a [(Mount, Option<Usage>)],
    pub recents: &'a [PathBuf],
    /// Windows' icons for the folders and disks, once loaded.
    pub icons: &'a FileIcons,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEvent {
    Open(PathBuf),
    /// Right click on a row: the menu for that path.
    Menu(PathBuf),
}

/// "120.0 GB free of 465.0 GB".
pub fn usage_text(usage: &Usage) -> String {
    format!("{} free of {}", format_size(usage.free), format_size(usage.total))
}

/// A disk row's tooltip: where it is mounted and, when known, its free space.
pub fn disk_hint(path: &Path, usage: Option<&Usage>) -> String {
    match usage {
        Some(usage) => format!("{}\n{}", path.display(), usage_text(usage)),
        None => path.display().to_string(),
    }
}

/// Share of the disk in use, 0.0 to 1.0, for the bar under its name.
pub fn used_fraction(usage: &Usage) -> f32 {
    if usage.total == 0 {
        return 0.0;
    }
    (usage.total.saturating_sub(usage.free) as f64 / usage.total as f64) as f32
}

/// The last part of `path`, or the whole path for the root.
fn label_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Id of the side panel's one Tab stop.
pub fn focus_id() -> egui::Id {
    egui::Id::new("lumen_sidebar_focus")
}

/// The item the arrows reached while the panel has the focus.
fn cursor_id() -> egui::Id {
    egui::Id::new("lumen_sidebar_cursor")
}

pub fn show(ui: &mut egui::Ui, sidebar: &Sidebar<'_>) -> Option<SidebarEvent> {
    let mut event = None;
    // One Tab stop for the whole panel; the arrows walk its items. Escape is
    // left to egui, which drops the focus and so gives the list its keys back.
    let stop = ui.interact(ui.max_rect(), focus_id(), egui::Sense::FOCUSABLE);
    if stop.has_focus() {
        let keep = egui::EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false };
        ui.memory_mut(|m| m.set_focus_lock_filter(focus_id(), keep));
    }
    // Every title and every item of an open section, in order: where the arrows go.
    let mut rows: Vec<Row> = Vec::new();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        let at = rows.len();
        let title = section(ui, "Bookmarks", true, |ui| {
            if sidebar.bookmarks.is_empty() {
                ui.weak("Ctrl+D or ☆ adds the folder you are in");
            }
            for path in sidebar.bookmarks {
                let response = drop_target(ui, Some(path), |ui| item(ui, ItemIcon::Folder, "folder", &label_of(path), &path.display().to_string(), path, sidebar.current, sidebar.icons));
                rows.push(Row { rect: response.rect, kind: RowKind::Item(path.clone()) });
                if response.clicked() {
                    event = Some(SidebarEvent::Open(path.clone()));
                }
                if response.secondary_clicked() {
                    event = Some(SidebarEvent::Menu(path.clone()));
                }
            }
        });
        rows.insert(at, title);
        let at = rows.len();
        let title = section(ui, "Places", true, |ui| {
            for place in sidebar.places {
                let this_pc = places::is_this_pc(&place.path);
                let icon = match (place.trash, this_pc) {
                    (true, _) => ItemIcon::Glyph(TRASH),
                    (_, true) => ItemIcon::Glyph(COMPUTER),
                    _ => ItemIcon::Folder,
                };
                // Not the trash: a drop there would skip its bookkeeping (what it held, and where from).
                // Nor This PC, which is no folder to drop into.
                let folder = (!place.trash && !this_pc).then_some(&place.path);
                let response = drop_target(ui, folder, |ui| item(ui, icon, place.icon, &place.label, &place.path.display().to_string(), &place.path, sidebar.current, sidebar.icons));
                rows.push(Row { rect: response.rect, kind: RowKind::Item(place.path.clone()) });
                if response.clicked() {
                    event = Some(SidebarEvent::Open(place.path.clone()));
                }
                // This PC has no menu: it is not a path the shell can be asked about.
                if response.secondary_clicked() && !this_pc {
                    event = Some(SidebarEvent::Menu(place.path.clone()));
                }
            }
        });
        rows.insert(at, title);
        let at = rows.len();
        let title = section(ui, "Storage", true, |ui| {
            for (mount, usage) in sidebar.disks {
                // The bar is only 4 px tall: the whole row tells the free space too.
                let hint = disk_hint(&mount.path, usage.as_ref());
                let response = drop_target(ui, Some(&mount.path), |ui| item(ui, ItemIcon::Glyph(DISK), "drive-harddisk", &mount.label, &hint, &mount.path, sidebar.current, sidebar.icons));
                rows.push(Row { rect: response.rect, kind: RowKind::Item(mount.path.clone()) });
                if response.clicked() {
                    event = Some(SidebarEvent::Open(mount.path.clone()));
                }
                if response.secondary_clicked() {
                    event = Some(SidebarEvent::Menu(mount.path.clone()));
                }
                if let Some(usage) = usage {
                    let bar = egui::ProgressBar::new(used_fraction(usage)).desired_height(4.0);
                    ui.add(bar).on_hover_text(hint);
                }
            }
        });
        rows.insert(at, title);
        let at = rows.len();
        let title = section(ui, "Recents", false, |ui| {
            for path in sidebar.recents {
                let response = item(ui, ItemIcon::Folder, "folder", &label_of(path), &path.display().to_string(), path, sidebar.current, sidebar.icons);
                rows.push(Row { rect: response.rect, kind: RowKind::Item(path.clone()) });
                if response.clicked() {
                    event = Some(SidebarEvent::Open(path.clone()));
                }
                if response.secondary_clicked() {
                    event = Some(SidebarEvent::Menu(path.clone()));
                }
            }
        });
        rows.insert(at, title);
        if stop.has_focus()
            && let Some(open) = walk(ui, &stop, &rows, sidebar.current)
        {
            event = Some(SidebarEvent::Open(open));
        }
    });
    event
}

/// A row the arrows can reach: a section's title or one of its items.
struct Row {
    rect: egui::Rect,
    kind: RowKind,
}

enum RowKind {
    /// The title's open state lives under this id.
    Title { id: egui::Id, open: bool },
    Item(PathBuf),
}

/// The panel walked like a tree. Up/Down, Home/End move a ring through
/// `rows`; Right opens a closed section or goes from its title to its first
/// item, Left goes from an item to its title or closes the section; Enter
/// opens the folder under the ring, or opens or closes its section. The ring
/// starts on the folder on screen each time the panel gets the focus.
fn walk(ui: &mut egui::Ui, stop: &egui::Response, rows: &[Row], current: &Path) -> Option<PathBuf> {
    let last = rows.len().checked_sub(1)?;
    let saved = ui.ctx().data(|d| d.get_temp::<usize>(cursor_id()));
    let start = match saved {
        Some(at) if !stop.gained_focus() => at,
        _ => rows.iter().position(|row| matches!(&row.kind, RowKind::Item(path) if path == current)).unwrap_or(0),
    };
    let keys = [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::Home, egui::Key::End, egui::Key::ArrowRight, egui::Key::ArrowLeft, egui::Key::Enter];
    let pressed: Vec<egui::Key> = ui.input_mut(|i| {
        let mut pressed = Vec::new();
        for key in keys {
            pressed.extend(std::iter::repeat_n(key, i.count_and_consume_key(egui::Modifiers::NONE, key)));
        }
        pressed
    });
    let mut at = start.min(last);
    let mut open = None;
    for key in &pressed {
        match (key, &rows[at].kind) {
            (egui::Key::ArrowDown, _) => at = (at + 1).min(last),
            (egui::Key::ArrowUp, _) => at = at.saturating_sub(1),
            (egui::Key::Home, _) => at = 0,
            (egui::Key::End, _) => at = last,
            (egui::Key::ArrowRight, RowKind::Title { open: true, .. }) => {
                if matches!(rows.get(at + 1), Some(Row { kind: RowKind::Item(_), .. })) {
                    at += 1;
                }
            }
            (egui::Key::ArrowRight, RowKind::Title { id, open: false }) => set_open(ui, *id, true),
            (egui::Key::ArrowLeft, RowKind::Title { id, open: true }) => set_open(ui, *id, false),
            (egui::Key::ArrowLeft, RowKind::Item(_)) => {
                at = rows[..at].iter().rposition(|row| matches!(row.kind, RowKind::Title { .. })).unwrap_or(at);
            }
            (egui::Key::Enter, RowKind::Title { id, open }) => set_open(ui, *id, !open),
            (egui::Key::Enter, RowKind::Item(path)) => open = Some(path.clone()),
            _ => {}
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(cursor_id(), at));
    let rect = rows[at].rect;
    if !pressed.is_empty() {
        ui.scroll_to_rect(rect, None);
    }
    ui.painter().rect_stroke(rect, 3, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
    open
}

/// Opens or closes the section whose state lives under `id`.
fn set_open(ui: &egui::Ui, id: egui::Id, open: bool) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open);
    state.set_open(open);
    state.store(ui.ctx());
}

/// A title that opens and closes its section on a click. Not a Tab stop, as
/// egui's own collapsing header would be: the panel is one.
fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) -> Row {
    let id = ui.make_persistent_id(("lumen_section", title));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open);
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let (rect, header) = ui.allocate_exact_size(size, egui::Sense::CLICK);
    if header.clicked() {
        state.toggle(ui);
    }
    let indent = ui.spacing().indent;
    let mut icon = header.clone();
    icon.rect = egui::Rect::from_center_size(egui::pos2(rect.left() + indent / 2.0, rect.center().y), egui::Vec2::splat(ui.spacing().icon_width));
    egui::collapsing_header::paint_default_icon(ui, state.openness(ui.ctx()), &icon);
    let galley = egui::WidgetText::from(egui::RichText::new(title).strong()).into_galley(ui, Some(egui::TextWrapMode::Truncate), rect.width() - indent, egui::TextStyle::Button);
    let text_pos = egui::pos2(rect.left() + indent, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_pos, galley, ui.visuals().text_color());
    let open = state.is_open();
    state.show_body_indented(&header, ui, body);
    Row { rect, kind: RowKind::Title { id, open } }
}

/// Draws one item with `draw` and, while entries are dragged, lights it up
/// behind its content when `folder` takes the drop (it is a folder that
/// exists and the drag is accepted there). `None` is an item that takes none.
fn drop_target(ui: &mut egui::Ui, folder: Option<&PathBuf>, draw: impl FnOnce(&mut egui::Ui) -> egui::Response) -> egui::Response {
    let lit = ui.painter().add(egui::Shape::Noop);
    let response = draw(ui);
    if let Some(folder) = folder
        && crate::ui::dnd::over_dir(ui.ctx(), ui.clip_rect(), response.rect, folder)
    {
        ui.painter().set(lit, crate::ui::dnd::lit_shape(ui.ctx(), response.rect));
    }
    response
}

enum ItemIcon {
    Folder,
    Glyph(&'static str),
}

/// One clickable row, the panel's full width; the folder on screen is
/// highlighted. `hint` shows on hover anywhere on it, icon and empty end
/// included. `theme` is its freedesktop icon name ("folder-download").
#[allow(clippy::too_many_arguments)]
fn item(ui: &mut egui::Ui, icon: ItemIcon, theme: &'static str, label: &str, hint: &str, path: &Path, current: &Path, icons: &FileIcons) -> egui::Response {
    // Windows' icon for folders and disks once loaded (Downloads' arrow, the
    // drive's); the painted folder or the glyph meanwhile. The trash has no
    // folder of its own to ask about. Elsewhere the icon theme's one-colour
    // icon, as Nautilus shows them, tinted like the text.
    #[cfg(windows)]
    let own = {
        let _ = theme;
        match icon {
            ItemIcon::Glyph(TRASH | COMPUTER) => None,
            _ => icons.folder_texture(path),
        }
    };
    #[cfg(not(windows))]
    let own = icons.symbolic_texture(theme);
    let selected = path == current;
    // Senses clicks below the name button, so the button keeps its own.
    // Clicks only: the panel is one Tab stop and the arrows walk its rows.
    let row = egui::UiBuilder::new().sense(egui::Sense::CLICK);
    // Behind the row, filled once its size is known: the hover highlight.
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let inner = ui.scope_builder(row, |ui| {
        // The row lights up as a whole (below): the name's button draws no box
        // of its own on hover or click; the folder on screen keeps its fill.
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [&mut widgets.hovered, &mut widgets.active] {
            state.bg_stroke = egui::Stroke::NONE;
            state.weak_bg_fill = egui::Color32::TRANSPARENT;
            state.bg_fill = egui::Color32::TRANSPARENT;
        }
        ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            match (own, icon) {
                (Some(texture), _) if cfg!(windows) => {
                    icons::image(ui, texture);
                }
                (Some(texture), _) => {
                    icons::symbol(ui, texture);
                }
                (None, ItemIcon::Folder) => {
                    icons::folder(ui);
                }
                (None, ItemIcon::Glyph(glyph)) => {
                    // Not selectable, or it would take the row's click.
                    ui.add(egui::Label::new(glyph).selectable(false));
                }
            }
            ui.add(egui::Button::selectable(selected, label).frame_when_inactive(false).sense(egui::Sense::CLICK))
        })
        .inner
    });
    let response = inner.response.union(inner.inner);
    // The whole row, as the file rows do, also where the name's button is.
    if response.contains_pointer() && !selected {
        let fill = ui.visuals().widgets.hovered.bg_fill;
        ui.painter().set(backdrop, egui::Shape::rect_filled(response.rect, egui::CornerRadius::same(3), fill));
    }
    response.on_hover_text(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_reads_free_of_total_and_the_bar_fills_with_what_is_used() {
        let gb = 1024u64.pow(3);
        let usage = Usage { total: 465 * gb, free: 120 * gb };
        assert_eq!(usage_text(&usage), "120.0 GB free of 465.0 GB");
        assert!((used_fraction(&usage) - 345.0 / 465.0).abs() < 1e-6);
        assert_eq!(used_fraction(&Usage { total: 0, free: 0 }), 0.0, "an empty disk is not NaN");
    }

    #[test]
    fn the_whole_row_carries_the_hint_and_the_click() {
        let ctx = egui::Context::default();
        let (mut row, mut avail) = (egui::Rect::NOTHING, 0.0);
        let icons = FileIcons::default();
        let draw = |ui: &mut egui::Ui| item(ui, ItemIcon::Glyph(DISK), "drive-harddisk", "File System", "/\nfree", Path::new("/"), Path::new("/home"), &icons);
        crate::ui::testing::frame(&ctx, vec![], |ui| {
            avail = ui.available_width();
            row = draw(ui).rect;
        });
        assert!(row.min.x <= 8.5, "the glyph is covered: {row:?}");
        assert!(row.width() >= avail - 1.0, "the empty part is covered: {row:?}");
        for pos in [row.left_center() + egui::vec2(2.0, 0.0), row.right_center() - egui::vec2(5.0, 0.0)] {
            assert!(crate::ui::testing::click(&ctx, pos, |ui| draw(ui).clicked()), "a click at {pos:?} opens it");
        }
    }

    #[cfg(windows)]
    #[test]
    fn places_and_disks_show_the_shells_icon_once_loaded_and_the_painted_one_meanwhile() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let red = |name: &str| ctx.load_texture(name, egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]), egui::TextureOptions::LINEAR);
        let (downloads, disk) = (red("downloads"), red("disk"));
        icons.insert_folder(Path::new("/home/me/Downloads"), downloads.clone());
        icons.insert_folder(Path::new("/"), disk.clone());
        let places = [Place { label: "Downloads".into(), path: "/home/me/Downloads".into(), trash: false, icon: "folder-download" }];
        let disks = [(Mount { label: "File System".into(), path: "/".into() }, None)];
        let bookmarks = [PathBuf::from("/home/me/work")];
        let sidebar = Sidebar { current: Path::new("/home/me"), bookmarks: &bookmarks, places: &places, disks: &disks, recents: &[], icons: &icons };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &sidebar);
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let drawn = |texture: &egui::TextureHandle| {
            shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Mesh(m) if m.texture_id == texture.id())).count()
        };
        assert_eq!(drawn(&downloads), 1, "the place's own icon");
        assert_eq!(drawn(&disk), 1, "the disk's icon, not the glyph");
        let glyphs = shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == DISK)).count();
        assert_eq!(glyphs, 0, "no glyph once the disk's icon is in");
        let meshes = shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default())).count();
        assert_eq!(meshes, 2, "the bookmark, still loading, keeps the painted folder");
    }

    #[cfg(not(windows))]
    #[test]
    fn every_row_shows_the_themes_symbolic_icon_in_the_text_colour_once_loaded() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let white = |name: &str| ctx.load_texture(name, egui::ColorImage::new([4, 4], vec![egui::Color32::WHITE; 16]), egui::TextureOptions::LINEAR);
        let (downloads, trash, disk, folder) = (white("downloads"), white("trash"), white("disk"), white("folder"));
        icons.insert_symbolic("folder-download", downloads.clone());
        icons.insert_symbolic("user-trash", trash.clone());
        icons.insert_symbolic("drive-harddisk", disk.clone());
        icons.insert_symbolic("folder", folder.clone());
        let places = [
            Place { label: "Downloads".into(), path: "/home/me/Downloads".into(), trash: false, icon: "folder-download" },
            Place { label: "Trash".into(), path: "/home/me/.local/share/Trash/files".into(), trash: true, icon: "user-trash" },
            Place { label: "Music".into(), path: "/home/me/Music".into(), trash: false, icon: "folder-music" },
        ];
        let disks = [(Mount { label: "File System".into(), path: "/".into() }, None)];
        let (bookmarks, recents) = ([PathBuf::from("/home/me/work")], [PathBuf::from("/tmp")]);
        let sidebar = Sidebar { current: Path::new("/home/me"), bookmarks: &bookmarks, places: &places, disks: &disks, recents: &recents, icons: &icons };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &sidebar);
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let text = ctx.global_style().visuals.text_color();
        let tinted = |texture: &egui::TextureHandle| {
            shapes
                .iter()
                .filter(|s| matches!(&s.shape, egui::Shape::Mesh(m) if m.texture_id == texture.id() && m.vertices.iter().all(|v| v.color == text)))
                .count()
        };
        assert_eq!(tinted(&downloads), 1, "the place's symbolic icon, in the text colour");
        assert_eq!(tinted(&trash), 1, "the trash has one too");
        assert_eq!(tinted(&disk), 1, "the disk's, not the glyph");
        assert_eq!(tinted(&folder), 1, "the bookmark's");
        let glyphs = shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Text(t) if [DISK, TRASH].contains(&t.galley.text()))).count();
        assert_eq!(glyphs, 0, "no glyphs once the icons are in");
        assert!(icons.wanted_symbolic().contains(&"folder-music"), "Music, still loading, was asked for");
    }

    #[test]
    fn a_row_under_the_pointer_is_highlighted_across_the_panel_like_the_file_rows() {
        let ctx = egui::Context::default();
        let icons = FileIcons::default();
        let draw = |ctx: &egui::Context, events| {
            let mut row = egui::Rect::NOTHING;
            let input = egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
            let mut output = ctx.run_ui(input, |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    row = item(ui, ItemIcon::Folder, "folder-download", "Downloads", "hint", Path::new("/d"), Path::new("/home"), &icons).rect;
                });
            });
            output.textures_delta.clear();
            (output.shapes, row)
        };
        let hover = ctx.global_style().visuals.widgets.hovered.bg_fill;
        let fills = |shapes: &[egui::epaint::ClippedShape]| -> Vec<egui::Rect> {
            shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.fill == hover => Some(r.rect),
                    _ => None,
                })
                .collect()
        };
        draw(&ctx, vec![]);
        let (still, row) = draw(&ctx, vec![]);
        assert!(fills(&still).iter().all(|r| r.width() < row.width() - 1.0), "no row highlight without the pointer");
        let end = row.right_center() - egui::vec2(5.0, 0.0);
        draw(&ctx, vec![egui::Event::PointerMoved(end)]);
        let (hovered, _) = draw(&ctx, vec![]);
        assert!(
            fills(&hovered).iter().any(|r| r.width() >= row.width() - 1.0 && r.contains(row.center())),
            "the whole row lights up, even past the name: {:?} for {row:?}",
            fills(&hovered)
        );
        // Over the name itself: the row's highlight only, no outline of its own.
        draw(&ctx, vec![egui::Event::PointerMoved(row.left_center() + egui::vec2(40.0, 0.0))]);
        let (over_name, _) = draw(&ctx, vec![]);
        let outlines: Vec<egui::Rect> = over_name
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color.a() > 0 => Some(r.rect),
                _ => None,
            })
            .collect();
        assert!(outlines.is_empty(), "no box around the name: {outlines:?}");
        assert!(fills(&over_name).iter().any(|r| r.width() >= row.width() - 1.0), "the row is lit");
    }

    #[test]
    fn a_disk_row_tells_its_free_space_not_only_its_thin_bar() {
        let gb = 1024u64.pow(3);
        let usage = Usage { total: 465 * gb, free: 120 * gb };
        assert_eq!(disk_hint(Path::new("/"), Some(&usage)), "/\n120.0 GB free of 465.0 GB");
        assert_eq!(disk_hint(Path::new("/mnt/data"), None), "/mnt/data", "usage not read yet");
    }

    /// The side panel's data for the focus tests: Recents starts closed.
    struct Panel {
        bookmarks: [PathBuf; 1],
        places: [Place; 2],
        disks: [(Mount, Option<Usage>); 1],
        recents: [PathBuf; 1],
        icons: FileIcons,
    }

    fn panel() -> Panel {
        let place = |label: &str, path: &str| Place { label: label.into(), path: path.into(), trash: false, icon: "folder" };
        Panel {
            bookmarks: [PathBuf::from("/b1")],
            places: [place("Home", "/home/u"), place("Downloads", "/home/u/Downloads")],
            disks: [(Mount { label: "File System".into(), path: "/".into() }, None)],
            recents: [PathBuf::from("/r1")],
            icons: FileIcons::default(),
        }
    }

    struct Drawn {
        before: egui::Id,
        after: egui::Id,
        event: Option<SidebarEvent>,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    /// A button, the side panel and another button, in the window's order.
    fn window(ctx: &egui::Context, panel: &Panel, events: Vec<egui::Event>) -> Drawn {
        let sidebar = Sidebar {
            current: Path::new("/home/u/Downloads"),
            bookmarks: &panel.bookmarks,
            places: &panel.places,
            disks: &panel.disks,
            recents: &panel.recents,
            icons: &panel.icons,
        };
        let (mut before, mut after, mut event) = (egui::Id::NULL, egui::Id::NULL, None);
        let input = egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            crate::ui::skip_panel_borders(root.ctx());
            egui::Panel::top("before").show(root, |ui| before = ui.button("before").id);
            egui::Panel::left("sidebar").show(root, |ui| event = show(ui, &sidebar));
            egui::CentralPanel::default().show(root, |ui| after = ui.button("after").id);
        });
        output.textures_delta.clear();
        Drawn { before, after, event, shapes: output.shapes }
    }

    fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    /// Presses and releases `k`; what the panel did and drew.
    fn tap(ctx: &egui::Context, panel: &Panel, k: egui::Key) -> Drawn {
        let down = window(ctx, panel, vec![key(k, true, egui::Modifiers::NONE)]);
        let mut up = window(ctx, panel, vec![key(k, false, egui::Modifiers::NONE)]);
        up.event = down.event.or(up.event);
        up
    }

    fn focused_panel(ctx: &egui::Context, panel: &Panel) {
        window(ctx, panel, vec![]);
        ctx.memory_mut(|m| m.request_focus(focus_id()));
        window(ctx, panel, vec![]);
        window(ctx, panel, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()));
    }

    fn opens(path: &str) -> Option<SidebarEvent> {
        Some(SidebarEvent::Open(PathBuf::from(path)))
    }

    #[test]
    fn the_side_panel_is_one_tab_stop_not_one_per_item_or_section() {
        let ctx = egui::Context::default();
        let panel = panel();
        let drawn = window(&ctx, &panel, vec![]);
        ctx.memory_mut(|m| m.request_focus(drawn.before));
        window(&ctx, &panel, vec![]);
        tap(&ctx, &panel, egui::Key::Tab);
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "Tab from the button lands on the panel");
        tap(&ctx, &panel, egui::Key::Tab);
        assert_eq!(ctx.memory(|m| m.focused()), Some(drawn.after), "the next Tab leaves it, past the border");
        let back = vec![key(egui::Key::Tab, true, egui::Modifiers::SHIFT), key(egui::Key::Tab, false, egui::Modifiers::SHIFT)];
        for event in back {
            window(&ctx, &panel, vec![event]);
        }
        window(&ctx, &panel, vec![]);
        window(&ctx, &panel, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "Shift+Tab comes back over the border");
    }

    /// Frames enough for a section to finish opening or closing.
    fn settle(ctx: &egui::Context, panel: &Panel) {
        for _ in 0..20 {
            window(ctx, panel, vec![]);
        }
    }

    #[test]
    fn up_and_down_walk_the_titles_and_the_open_sections_items_from_the_current_folder() {
        let ctx = egui::Context::default();
        let panel = panel();
        focused_panel(&ctx, &panel);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/home/u/Downloads"), "starts on the folder on screen");
        tap(&ctx, &panel, egui::Key::ArrowUp);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/home/u"));
        tap(&ctx, &panel, egui::Key::Home);
        tap(&ctx, &panel, egui::Key::ArrowUp);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/b1"), "Home is the Bookmarks title, the next row its item");
        tap(&ctx, &panel, egui::Key::End);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        tap(&ctx, &panel, egui::Key::ArrowUp);
        let drawn = tap(&ctx, &panel, egui::Key::Enter);
        assert_eq!(drawn.event, opens("/"), "End is the closed Recents title, the row above it the disk");
        let ring = ctx.global_style().visuals.selection.stroke.color;
        let rings = drawn.shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color == ring)).count();
        assert_eq!(rings, 1, "one ring, on the row the arrows reached");
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "the arrows kept the focus on the panel");
    }

    #[test]
    fn left_and_right_close_and_open_sections_like_a_tree() {
        let ctx = egui::Context::default();
        let panel = panel();
        focused_panel(&ctx, &panel);
        // On Downloads: Left goes to the Places title; Enter there closes and opens it.
        tap(&ctx, &panel, egui::Key::ArrowLeft);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, None, "Enter on a title opens no folder");
        settle(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::Enter);
        settle(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/home/u"), "Places open again, its first item follows");
        // On an item Left goes to its title, Left again closes the section.
        tap(&ctx, &panel, egui::Key::ArrowLeft);
        tap(&ctx, &panel, egui::Key::ArrowLeft);
        settle(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/"), "Places closed: past its title come Storage's title and the disk");
        // Recents, closed: Right opens it, Right again goes to its first item.
        tap(&ctx, &panel, egui::Key::End);
        tap(&ctx, &panel, egui::Key::ArrowRight);
        settle(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::ArrowRight);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/r1"));
        // Right on an item and Left on a closed title do nothing.
        tap(&ctx, &panel, egui::Key::ArrowRight);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/r1"));
        tap(&ctx, &panel, egui::Key::Home);
        tap(&ctx, &panel, egui::Key::ArrowLeft);
        settle(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::ArrowLeft);
        settle(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        tap(&ctx, &panel, egui::Key::ArrowDown);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/"), "Bookmarks and Places closed, Storage's disk is third");
    }

    #[test]
    fn a_section_opened_with_a_click_joins_the_walk() {
        let ctx = egui::Context::default();
        let panel = panel();
        window(&ctx, &panel, vec![]);
        let drawn = window(&ctx, &panel, vec![]);
        let header = drawn
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Recents" => Some(t.pos + t.galley.size() / 2.0),
                _ => None,
            })
            .expect("the Recents header is drawn");
        let press = |pressed| egui::Event::PointerButton { pos: header, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        window(&ctx, &panel, vec![egui::Event::PointerMoved(header)]);
        window(&ctx, &panel, vec![press(true)]);
        window(&ctx, &panel, vec![press(false)]);
        for _ in 0..10 {
            window(&ctx, &panel, vec![]);
        }
        focused_panel(&ctx, &panel);
        tap(&ctx, &panel, egui::Key::End);
        assert_eq!(tap(&ctx, &panel, egui::Key::Enter).event, opens("/r1"));
    }

    #[test]
    fn escape_leaves_the_side_panel() {
        let ctx = egui::Context::default();
        let panel = panel();
        focused_panel(&ctx, &panel);
        assert_eq!(tap(&ctx, &panel, egui::Key::Escape).event, None);
        assert_eq!(ctx.memory(|m| m.focused()), None);
    }

    use crate::drag::Keys;
    use crate::fs::ops::tests::fresh_fixture;
    use crate::ui::testing::{label, let_go, lift, lit, run};

    const NONE: egui::Modifiers = egui::Modifiers::NONE;
    const CTRL: egui::Modifiers = egui::Modifiers { ctrl: true, command: true, ..egui::Modifiers::NONE };

    /// A panel over real folders: a bookmark `bm`, the places `home` and
    /// `trash` (the trash) and `gone` (no folder), the disks `disk` and
    /// `unmounted` (no folder), and a recent `rec`; `f.txt` is beside them.
    struct Drop {
        dir: PathBuf,
        bookmarks: [PathBuf; 1],
        places: [Place; 3],
        disks: [(Mount, Option<Usage>); 2],
        recents: [PathBuf; 1],
        icons: FileIcons,
    }

    fn drop_panel(name: &str) -> Drop {
        let dir = fresh_fixture(name);
        for sub in ["bm", "home", "trash", "disk", "rec"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("f.txt"), "f").unwrap();
        let place = |label: &str, sub: &str, trash| Place { label: label.into(), path: dir.join(sub), trash, icon: "folder" };
        let mount = |label: &str, sub: &str| (Mount { label: label.into(), path: dir.join(sub) }, None);
        Drop {
            bookmarks: [dir.join("bm")],
            places: [place("Home", "home", false), place("Trash", "trash", true), place("Gone", "gone", false)],
            disks: [mount("Disk", "disk"), mount("Unmounted", "unmounted")],
            recents: [dir.join("rec")],
            icons: FileIcons::default(),
            dir,
        }
    }

    /// The panel as the app draws it, with the label by the pointer.
    fn side(panel: &Drop) -> impl FnMut(&mut egui::Ui) -> Option<SidebarEvent> + '_ {
        move |ui| {
            let sidebar = Sidebar { current: &panel.dir, bookmarks: &panel.bookmarks, places: &panel.places, disks: &panel.disks, recents: &panel.recents, icons: &panel.icons };
            let event = show(ui, &sidebar);
            crate::ui::dnd::finish(ui.ctx());
            event
        }
    }

    /// The middle of the row showing `text`, found where it was drawn.
    fn row_of(ctx: &egui::Context, panel: &Drop, text: &str) -> egui::Pos2 {
        run(ctx, vec![], side(panel));
        let (_, shapes) = run(ctx, vec![], side(panel));
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos + t.galley.size() / 2.0),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{text:?} is drawn"))
    }

    fn taken(ctx: &egui::Context) -> Option<(Vec<PathBuf>, PathBuf, Keys)> {
        crate::ui::dnd::take_drop(ctx).map(|d| (d.drag.paths, d.target, d.keys))
    }

    #[test]
    fn a_bookmark_a_place_and_a_disk_under_dragged_entries_light_up_name_the_action_and_take_the_drop() {
        let panel = drop_panel("dnd-side-drop");
        let ctx = egui::Context::default();
        let file = panel.dir.join("f.txt");
        for (text, sub) in [("bm", "bm"), ("Home", "home"), ("Disk", "disk")] {
            let at = row_of(&ctx, &panel, text);
            let shapes = lift(&ctx, std::slice::from_ref(&file), at, NONE, side(&panel));
            let lit_rects = lit(&ctx, &shapes);
            assert!(lit_rects.iter().any(|r| r.contains(at) && r.width() > 100.0), "{text}: its row lights up: {lit_rects:?}");
            assert_eq!(lit_rects.len(), 1, "{text}: and only it");
            assert_eq!(label(&shapes), Some(format!("Move 1 item to {sub}")), "{text}: same disk, a move");
            let (event, _) = let_go(&ctx, at, NONE, side(&panel));
            assert_eq!(event, None, "{text}: letting go does not open it");
            assert_eq!(taken(&ctx), Some((vec![file.clone()], panel.dir.join(sub), Keys::default())), "{text}");
            assert!(taken(&ctx).is_none(), "{text}: taken once");
        }
        // Ctrl copies.
        let at = row_of(&ctx, &panel, "Home");
        let shapes = lift(&ctx, std::slice::from_ref(&file), at, CTRL, side(&panel));
        assert_eq!(label(&shapes).as_deref(), Some("Copy 1 item to home"));
        let_go(&ctx, at, CTRL, side(&panel));
        assert_eq!(taken(&ctx), Some((vec![file], panel.dir.join("home"), Keys { ctrl: true, shift: false })));
        let _ = std::fs::remove_dir_all(&panel.dir);
    }

    #[test]
    fn the_label_by_the_pointer_grows_to_its_text_on_one_line() {
        let panel = drop_panel("dnd-side-label");
        let ctx = egui::Context::default();
        let file = panel.dir.join("f.txt");
        // First shown short ("1 item" over the trash), then naming a folder.
        let trash = row_of(&ctx, &panel, "Trash");
        let home = row_of(&ctx, &panel, "Home");
        lift(&ctx, std::slice::from_ref(&file), trash, NONE, side(&panel));
        run(&ctx, vec![egui::Event::PointerMoved(home)], side(&panel));
        let (_, shapes) = run(&ctx, vec![], side(&panel));
        let rows = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text().contains(" item") => Some((t.galley.text().to_owned(), t.galley.rows.len())),
            _ => None,
        });
        assert_eq!(rows, Some(("Move 1 item to home".to_owned(), 1)));
        let_go(&ctx, home, NONE, side(&panel));
        let _ = std::fs::remove_dir_all(&panel.dir);
    }

    #[test]
    fn rows_that_are_no_folder_that_takes_it_stay_dark_and_take_nothing() {
        let panel = drop_panel("dnd-side-refused");
        let ctx = egui::Context::default();
        let file = panel.dir.join("f.txt");
        // Recents starts closed.
        let header = row_of(&ctx, &panel, "Recents");
        crate::ui::testing::click(&ctx, header, side(&panel));
        for _ in 0..20 {
            run(&ctx, vec![], side(&panel));
        }
        // The trash (a drop there would skip its bookkeeping), a place whose
        // folder is gone, a disk that is not mounted, and Recents.
        for text in ["Trash", "Gone", "Unmounted", "rec"] {
            let at = row_of(&ctx, &panel, text);
            let shapes = lift(&ctx, std::slice::from_ref(&file), at, NONE, side(&panel));
            assert!(lit(&ctx, &shapes).is_empty(), "{text}: nothing lights up");
            assert_eq!(label(&shapes).as_deref(), Some("1 item"), "{text}: no folder named");
            let (event, _) = let_go(&ctx, at, NONE, side(&panel));
            assert_eq!((event, taken(&ctx)), (None, None), "{text}: nothing dropped");
        }
        // The entry's own row, and a move into the folder it is in (the panel's rows
        // are under the fixture folder, `f.txt` is in): refused; Ctrl copies there.
        let at = row_of(&ctx, &panel, "bm");
        let shapes = lift(&ctx, &[panel.dir.join("bm")], at, NONE, side(&panel));
        assert!(lit(&ctx, &shapes).is_empty(), "a folder is not dropped on itself");
        let_go(&ctx, at, NONE, side(&panel));
        assert_eq!(taken(&ctx), None);
        let inside = panel.dir.join("home/g.txt");
        std::fs::write(&inside, "g").unwrap();
        let at = row_of(&ctx, &panel, "Home");
        let shapes = lift(&ctx, std::slice::from_ref(&inside), at, NONE, side(&panel));
        assert!(lit(&ctx, &shapes).is_empty(), "a move where it already is");
        let_go(&ctx, at, NONE, side(&panel));
        assert_eq!(taken(&ctx), None);
        let shapes = lift(&ctx, std::slice::from_ref(&inside), at, CTRL, side(&panel));
        assert_eq!(label(&shapes).as_deref(), Some("Copy 1 item to home"));
        let_go(&ctx, at, CTRL, side(&panel));
        assert_eq!(taken(&ctx), Some((vec![inside], panel.dir.join("home"), Keys { ctrl: true, shift: false })));
        let _ = std::fs::remove_dir_all(&panel.dir);
    }

    #[test]
    fn without_a_drag_a_click_still_opens_the_item_and_nothing_lights() {
        let panel = drop_panel("dnd-side-plain");
        let ctx = egui::Context::default();
        let home = row_of(&ctx, &panel, "Home");
        assert_eq!(crate::ui::testing::click(&ctx, home, side(&panel)), Some(SidebarEvent::Open(panel.dir.join("home"))));
        let disk = row_of(&ctx, &panel, "Disk");
        assert_eq!(crate::ui::testing::click(&ctx, disk, side(&panel)), Some(SidebarEvent::Open(panel.dir.join("disk"))));
        let (_, shapes) = run(&ctx, vec![egui::Event::PointerMoved(home)], side(&panel));
        assert!(lit(&ctx, &shapes).is_empty(), "hovering a row lights nothing");
        assert_eq!(taken(&ctx), None, "and nothing was dropped");

        let _ = std::fs::remove_dir_all(&panel.dir);
    }

    #[test]
    fn a_right_click_on_a_bookmark_a_place_a_disk_or_a_recent_asks_for_the_menu_of_that_path() {
        let panel = drop_panel("side-menu");
        let ctx = egui::Context::default();
        // Recents starts closed.
        let header = row_of(&ctx, &panel, "Recents");
        crate::ui::testing::click(&ctx, header, side(&panel));
        for _ in 0..20 {
            run(&ctx, vec![], side(&panel));
        }
        let right_click = |at| {
            let secondary = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: NONE };
            run(&ctx, vec![egui::Event::PointerMoved(at)], side(&panel));
            run(&ctx, vec![secondary(true)], side(&panel));
            run(&ctx, vec![secondary(false)], side(&panel)).0
        };
        // The trash and a place whose folder is gone are rows too.
        for (text, sub) in [("bm", "bm"), ("Home", "home"), ("Trash", "trash"), ("Gone", "gone"), ("Disk", "disk"), ("Unmounted", "unmounted"), ("rec", "rec")] {
            let at = row_of(&ctx, &panel, text);
            assert_eq!(right_click(at), Some(SidebarEvent::Menu(panel.dir.join(sub))), "{text}");
        }
        let (_, shapes) = run(&ctx, vec![], side(&panel));
        assert!(!shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Remove bookmark")), "egui's own bookmark menu is gone");
        let _ = std::fs::remove_dir_all(&panel.dir);
    }

    #[test]
    fn this_pc_opens_with_a_click_shows_a_computer_and_has_no_menu_and_takes_no_drops() {
        let panel = Drop { places: [Place { label: "This PC".into(), path: places::this_pc(), trash: false, icon: "computer" }, panel_place("Trash"), panel_place("Gone")], ..drop_panel("side-this-pc") };
        let ctx = egui::Context::default();
        let at = row_of(&ctx, &panel, "This PC");
        let (_, shapes) = run(&ctx, vec![], side(&panel));
        let glyphs = |text: &str| shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == text)).count();
        assert_eq!((glyphs(COMPUTER), glyphs("This PC")), (1, 1), "a computer, not a folder");
        assert_eq!(crate::ui::testing::click(&ctx, at, side(&panel)), Some(SidebarEvent::Open(places::this_pc())));
        let secondary = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: NONE };
        run(&ctx, vec![egui::Event::PointerMoved(at)], side(&panel));
        run(&ctx, vec![secondary(true)], side(&panel));
        assert_eq!(run(&ctx, vec![secondary(false)], side(&panel)).0, None, "no menu for what is no path");
        let shapes = lift(&ctx, &[panel.dir.join("f.txt")], at, NONE, side(&panel));
        assert!(lit(&ctx, &shapes).is_empty(), "nothing is dropped into it");
        let_go(&ctx, at, NONE, side(&panel));
        assert_eq!(taken(&ctx), None);
        let _ = std::fs::remove_dir_all(&panel.dir);
    }

    fn panel_place(label: &str) -> Place {
        Place { label: label.into(), path: PathBuf::from(format!("/nowhere/{label}")), trash: false, icon: "folder" }
    }

    #[test]
    fn files_dragged_in_from_another_app_light_the_place_under_them_and_drop_on_it_but_not_the_trash() {
        let panel = drop_panel("dnd-side-outside");
        let ctx = egui::Context::default();
        let file = panel.dir.join("f.txt");
        let outside = |at, keys, released| {
            let drag = crate::drag::Drag { paths: vec![file.clone()] };
            crate::ui::dnd::set_outside(&ctx, Some(crate::ui::dnd::Outside { drag, at, keys, released }));
        };
        let (home, trash) = (row_of(&ctx, &panel, "Home"), row_of(&ctx, &panel, "Trash"));
        outside(home, Keys::default(), false);
        run(&ctx, vec![], side(&panel));
        let (_, shapes) = run(&ctx, vec![], side(&panel));
        assert!(lit(&ctx, &shapes).iter().any(|r| r.contains(home)), "Home lights up");
        assert_eq!(label(&shapes).as_deref(), Some("Move 1 item to home"));
        outside(home, Keys::default(), true);
        run(&ctx, vec![], side(&panel));
        assert_eq!(taken(&ctx), Some((vec![file.clone()], panel.dir.join("home"), Keys::default())));

        outside(trash, Keys::default(), false);
        run(&ctx, vec![], side(&panel));
        let (_, shapes) = run(&ctx, vec![], side(&panel));
        assert!(lit(&ctx, &shapes).is_empty(), "the Trash stays dark");
        assert_eq!(label(&shapes).as_deref(), Some("1 item"));
        outside(trash, Keys::default(), true);
        run(&ctx, vec![], side(&panel));
        assert_eq!(taken(&ctx), None);
        crate::ui::dnd::set_outside(&ctx, None);
        let _ = std::fs::remove_dir_all(&panel.dir);
    }
}
