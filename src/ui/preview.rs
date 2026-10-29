//! The side panel opened with Space: the selected entry's contents.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use egui_extras::{Column, TableBuilder};

use crate::app::{Sort, visible_rows};
use crate::format::{format_size, format_time};
use crate::fs::Entry;
use crate::fs::count::Totals;
use crate::i18n::tr;
use crate::ui::file_list::{COLUMNS, column_label, count_cells, count_cols, name_label};
use crate::ui::icons;
use crate::fs::preview::Content;

/// The preview of one entry, as shown in the panel.
pub struct PreviewPane {
    pub path: PathBuf,
    /// Size and date when requested; if they change on disk, it is loaded again.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// `None` while the worker is still loading it.
    pub content: Option<Content>,
    /// The image uploaded to the GPU, made on first draw.
    pub texture: Option<egui::TextureHandle>,
}

impl PreviewPane {
    pub fn new(entry: &Entry) -> Self {
        Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
            content: None,
            texture: None,
        }
    }

    /// Whether this pane still shows `entry` as it is now.
    pub fn shows(&self, entry: &Entry) -> bool {
        self.path == entry.path && self.size == entry.size && self.modified == entry.modified
    }
}

/// Draws the panel for the selected `entry`.
/// `totals` are the entry's Alt+R counts, when computed.
pub fn show(
    ui: &mut egui::Ui,
    entry: &Entry,
    totals: Option<&Totals>,
    pane: &mut PreviewPane,
    show_hidden: bool,
    sort: Sort,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        icons::show(ui, entry, ui.text_style_height(&egui::TextStyle::Body), None);
        ui.add(egui::Label::new(egui::RichText::new(&entry.name).strong()).truncate());
    });
    ui.weak(details(entry, totals, pane.content.as_ref()));
    ui.separator();

    match &pane.content {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr!("Loading…"));
            });
        }
        Some(Content::Folder(Ok(entries))) => folder(ui, entries, show_hidden, sort),
        Some(Content::Folder(Err(err))) => {
            ui.colored_label(ui.visuals().error_fg_color, err);
        }
        Some(Content::Text { text, truncated }) => {
            if text.is_empty() {
                ui.weak(tr!("Empty file"));
            }
            if *truncated {
                ui.weak(tr!("Showing the first 256 KB"));
            }
            egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).monospace()).extend());
            });
        }
        Some(Content::Image(picture)) => {
            let texture = pane.texture.get_or_insert_with(|| {
                let image = egui::ColorImage::from_rgba_unmultiplied(picture.size, &picture.rgba);
                ui.ctx().load_texture("lumen_preview", image, egui::TextureOptions::LINEAR)
            });
            ui.add(egui::Image::new(&*texture).shrink_to_fit());
        }
        Some(Content::Info(reason)) => {
            ui.weak(reason);
        }
    }
}

/// "Folder", "Folder · 3 folders · 9 files · 2.2 MB" once counted, or
/// "12.0 KB · TS file · 2026-10-07 18:07 · 1920 × 1080".
fn details(entry: &Entry, totals: Option<&Totals>, content: Option<&Content>) -> String {
    let mut parts = Vec::new();
    if let Some(size) = entry.size {
        parts.push(format_size(size));
    }
    parts.push(entry.kind());
    if let Some(t) = totals.filter(|_| entry.is_dir) {
        parts.push(plural(t.folders, "folder"));
        parts.push(plural(t.files, "file"));
        parts.push(format_size(t.bytes));
    }
    if let Some(modified) = entry.modified {
        parts.push(format_time(modified));
    }
    if let Some(Content::Image(picture)) = content {
        parts.push(format!("{} × {} px", picture.original[0], picture.original[1]));
    }
    parts.join(" · ")
}

/// "1 file", "9 files".
pub fn plural(n: u64, noun: &str) -> String {
    match (noun, n) {
        ("file", 1) => tr!("1 file").to_owned(),
        ("file", _) => tr!("{0} files", n),
        ("folder", 1) => tr!("1 folder").to_owned(),
        ("folder", _) => tr!("{0} folders", n),
        ("item", 1) => tr!("1 item").to_owned(),
        ("item", _) => tr!("{0} items", n),
        (_, 1) => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// The folder's entries as a read-only table, sorted and filtered for hidden
/// files like the main list. Only visible rows are laid out, so a huge folder
/// previews as fast as a small one.
fn folder(ui: &mut egui::Ui, entries: &[Entry], show_hidden: bool, sort: Sort) {
    let rows = visible_rows(entries, &HashMap::new(), show_hidden, "", sort);
    if rows.is_empty() {
        ui.weak(tr!("Empty folder"));
        return;
    }
    ui.weak(tr!("{0} items", rows.len()));
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    TableBuilder::new(ui)
        // Its own id: the main list's scroll and column state must not leak in.
        .id_salt("lumen_preview_table")
        .striped(true)
        .column(Column::remainder().at_least(100.0).clip(true))
        .column(Column::auto().at_least(50.0).clip(true))
        .column(Column::exact(120.0))
        .column(Column::exact(50.0))
        .column(Column::exact(50.0))
        .column(Column::exact(70.0))
        .header(row_height, |mut header| {
            for (label, _) in COLUMNS {
                header.col(|ui| {
                    ui.strong(column_label(label));
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let entry = &entries[rows[row.index()]];
                row.col(|ui| {
                    ui.horizontal(|ui| {
                        icons::show(ui, entry, ui.text_style_height(&egui::TextStyle::Body), None);
                        name_label(ui, entry, "");
                    });
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(entry.kind()).selectable(false).truncate());
                });
                row.col(|ui| {
                    let date = entry.modified.map(format_time).unwrap_or_default();
                    ui.add(egui::Label::new(date).selectable(false));
                });
                count_cols(&mut row, &count_cells(entry, None));
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: Option<u64>) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            is_dir: false,
            size,
            modified: None,
            hidden: false,
        }
    }

    #[test]
    fn details_name_the_size_kind_and_image_dimensions() {
        assert_eq!(details(&file("main.ts", Some(2048)), None, None), "2.0 KB · TS file");
        assert_eq!(details(&file("Makefile", Some(5)), None, None), "5 B · File");
        let dir = Entry { is_dir: true, size: None, ..file("src", None) };
        assert_eq!(details(&dir, None, None), "Folder");
        let picture = Content::Image(crate::fs::preview::Picture {
            size: [2, 1],
            original: [4000, 2000],
            rgba: vec![0; 8],
        });
        assert_eq!(details(&file("a.png", Some(10)), None, Some(&picture)), "10 B · PNG file · 4000 × 2000 px");
    }

    #[test]
    fn details_of_a_counted_folder_add_its_totals() {
        let dir = Entry { is_dir: true, size: None, ..file("src", None) };
        let totals = Totals { folders: 3, files: 9, bytes: 2306867, unreadable: 0 };
        assert_eq!(details(&dir, Some(&totals), None), "Folder · 3 folders · 9 files · 2.2 MB");
        let one = Totals { folders: 1, files: 1, bytes: 0, unreadable: 0 };
        assert_eq!(details(&dir, Some(&one), None), "Folder · 1 folder · 1 file · 0 B");
    }
}
