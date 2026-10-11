//! The left panel: Bookmarks, Places, Storage and Recents, each collapsible.
//! A click on an item opens it in the list.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::format::format_size;
use crate::places::{Mount, Place, Usage};
use crate::ui::file_icons::FileIcons;
use crate::ui::icons;

pub const TOGGLE: &str = "☰";
pub const DISK: &str = "🖴";
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
    RemoveBookmark(PathBuf),
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

pub fn show(ui: &mut egui::Ui, sidebar: &Sidebar<'_>) -> Option<SidebarEvent> {
    let mut event = None;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        section(ui, "Bookmarks", true, |ui| {
            if sidebar.bookmarks.is_empty() {
                ui.weak("Ctrl+D or ☆ adds the folder you are in");
            }
            for path in sidebar.bookmarks {
                let response = item(ui, ItemIcon::Folder, &label_of(path), &path.display().to_string(), path, sidebar.current, sidebar.icons);
                if response.clicked() {
                    event = Some(SidebarEvent::Open(path.clone()));
                }
                response.context_menu(|ui| {
                    if ui.button("Remove bookmark").clicked() {
                        event = Some(SidebarEvent::RemoveBookmark(path.clone()));
                        ui.close();
                    }
                });
            }
        });
        section(ui, "Places", true, |ui| {
            for place in sidebar.places {
                let icon = if place.trash { ItemIcon::Glyph(TRASH) } else { ItemIcon::Folder };
                if item(ui, icon, &place.label, &place.path.display().to_string(), &place.path, sidebar.current, sidebar.icons).clicked() {
                    event = Some(SidebarEvent::Open(place.path.clone()));
                }
            }
        });
        section(ui, "Storage", true, |ui| {
            for (mount, usage) in sidebar.disks {
                // The bar is only 4 px tall: the whole row tells the free space too.
                let hint = disk_hint(&mount.path, usage.as_ref());
                let response = item(ui, ItemIcon::Glyph(DISK), &mount.label, &hint, &mount.path, sidebar.current, sidebar.icons);
                if response.clicked() {
                    event = Some(SidebarEvent::Open(mount.path.clone()));
                }
                if let Some(usage) = usage {
                    let bar = egui::ProgressBar::new(used_fraction(usage)).desired_height(4.0);
                    ui.add(bar).on_hover_text(hint);
                }
            }
        });
        section(ui, "Recents", false, |ui| {
            for path in sidebar.recents {
                if item(ui, ItemIcon::Folder, &label_of(path), &path.display().to_string(), path, sidebar.current, sidebar.icons).clicked() {
                    event = Some(SidebarEvent::Open(path.clone()));
                }
            }
        });
    });
    event
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .default_open(open)
        .show(ui, body);
}

enum ItemIcon {
    Folder,
    Glyph(&'static str),
}

/// One clickable row, the panel's full width; the folder on screen is
/// highlighted. `hint` shows on hover anywhere on it, icon and empty end included.
fn item(ui: &mut egui::Ui, icon: ItemIcon, label: &str, hint: &str, path: &Path, current: &Path, icons: &FileIcons) -> egui::Response {
    // Windows' icon for folders and disks once loaded (Downloads' arrow, the
    // drive's); the painted folder or the glyph meanwhile. The trash has no
    // folder of its own to ask about.
    let own = match icon {
        ItemIcon::Glyph(TRASH) => None,
        _ => icons.folder_texture(path),
    };
    let selected = path == current;
    // Senses clicks below the name button, so the button keeps its own.
    let row = egui::UiBuilder::new().sense(egui::Sense::click());
    let inner = ui.scope_builder(row, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            match (own, icon) {
                (Some(texture), _) => {
                    icons::image(ui, texture);
                }
                (None, ItemIcon::Folder) => {
                    icons::folder(ui);
                }
                (None, ItemIcon::Glyph(glyph)) => {
                    // Not selectable, or it would take the row's click.
                    ui.add(egui::Label::new(glyph).selectable(false));
                }
            }
            ui.add(egui::Button::selectable(selected, label).frame_when_inactive(false))
        })
        .inner
    });
    inner.response.union(inner.inner).on_hover_text(hint)
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
        let draw = |ui: &mut egui::Ui| item(ui, ItemIcon::Glyph(DISK), "File System", "/\nfree", Path::new("/"), Path::new("/home"), &icons);
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

    #[test]
    fn places_and_disks_show_the_shells_icon_once_loaded_and_the_painted_one_meanwhile() {
        let ctx = egui::Context::default();
        let mut icons = FileIcons::default();
        let red = |name: &str| ctx.load_texture(name, egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]), egui::TextureOptions::LINEAR);
        let (downloads, disk) = (red("downloads"), red("disk"));
        icons.insert_folder(Path::new("/home/me/Downloads"), downloads.clone());
        icons.insert_folder(Path::new("/"), disk.clone());
        let places = [Place { label: "Downloads".into(), path: "/home/me/Downloads".into(), trash: false }];
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

    #[test]
    fn a_disk_row_tells_its_free_space_not_only_its_thin_bar() {
        let gb = 1024u64.pow(3);
        let usage = Usage { total: 465 * gb, free: 120 * gb };
        assert_eq!(disk_hint(Path::new("/"), Some(&usage)), "/\n120.0 GB free of 465.0 GB");
        assert_eq!(disk_hint(Path::new("/mnt/data"), None), "/mnt/data", "usage not read yet");
    }
}
