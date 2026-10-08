//! The side panel opened with Space: the selected entry's contents.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use egui_extras::{Column, TableBuilder};

use crate::app::{Sort, visible_rows};
use crate::format::{format_size, format_time};
use crate::fs::Entry;
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
pub fn show(ui: &mut egui::Ui, entry: &Entry, pane: &mut PreviewPane, show_hidden: bool, sort: Sort) {
    ui.add_space(6.0);
    let icon = if entry.is_dir { "🗀" } else { "🗋" };
    ui.add(egui::Label::new(egui::RichText::new(format!("{icon} {}", entry.name)).strong()).truncate());
    ui.weak(details(entry, pane.content.as_ref()));
    ui.separator();

    match &pane.content {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading…");
            });
        }
        Some(Content::Folder(Ok(entries))) => folder(ui, entries, show_hidden, sort),
        Some(Content::Folder(Err(err))) => {
            ui.colored_label(ui.visuals().error_fg_color, err);
        }
        Some(Content::Text { text, truncated }) => {
            if text.is_empty() {
                ui.weak("Empty file");
            }
            if *truncated {
                ui.weak("Showing the first 256 KB");
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

/// "Folder", or "12.0 KB · TS file · 2026-10-07 18:07 · 1920 × 1080".
fn details(entry: &Entry, content: Option<&Content>) -> String {
    let mut parts = Vec::new();
    if let Some(size) = entry.size {
        parts.push(format_size(size));
    }
    parts.push(kind(entry));
    if let Some(modified) = entry.modified {
        parts.push(format_time(modified));
    }
    if let Some(Content::Image(picture)) = content {
        parts.push(format!("{} × {} px", picture.original[0], picture.original[1]));
    }
    parts.join(" · ")
}

fn kind(entry: &Entry) -> String {
    if entry.is_dir {
        return "Folder".into();
    }
    match entry.path.extension().map(|e| e.to_string_lossy()) {
        Some(ext) if !ext.is_empty() => format!("{} file", ext.to_uppercase()),
        _ => "File".into(),
    }
}

/// The folder's entries as a read-only table, sorted and filtered for hidden
/// files like the main list. Only visible rows are laid out, so a huge folder
/// previews as fast as a small one.
fn folder(ui: &mut egui::Ui, entries: &[Entry], show_hidden: bool, sort: Sort) {
    let rows = visible_rows(entries, show_hidden, "", sort);
    if rows.is_empty() {
        ui.weak("Empty folder");
        return;
    }
    ui.weak(format!("{} items", rows.len()));
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    TableBuilder::new(ui)
        // Its own id: the main list's scroll and column state must not leak in.
        .id_salt("lumen_preview_table")
        .striped(true)
        .column(Column::remainder().at_least(100.0).clip(true))
        .column(Column::auto().at_least(50.0).clip(true))
        .column(Column::exact(70.0))
        .column(Column::exact(120.0))
        .header(row_height, |mut header| {
            for label in ["Name", "Type", "Size", "Modified"] {
                header.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let entry = &entries[rows[row.index()]];
                row.col(|ui| {
                    let icon = if entry.is_dir { "🗀" } else { "🗋" };
                    let text = egui::RichText::new(format!("{icon} {}", entry.name));
                    let text = if entry.hidden { text.weak() } else { text };
                    ui.add(egui::Label::new(text).selectable(false).truncate());
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(kind(entry)).selectable(false).truncate());
                });
                row.col(|ui| {
                    let size = entry.size.map(format_size).unwrap_or_default();
                    ui.add(egui::Label::new(size).selectable(false));
                });
                row.col(|ui| {
                    let date = entry.modified.map(format_time).unwrap_or_default();
                    ui.add(egui::Label::new(date).selectable(false));
                });
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
        assert_eq!(details(&file("main.ts", Some(2048)), None), "2.0 KB · TS file");
        assert_eq!(details(&file("Makefile", Some(5)), None), "5 B · File");
        let dir = Entry { is_dir: true, size: None, ..file("src", None) };
        assert_eq!(details(&dir, None), "Folder");
        let picture = Content::Image(crate::fs::preview::Picture {
            size: [2, 1],
            original: [4000, 2000],
            rgba: vec![0; 8],
        });
        assert_eq!(details(&file("a.png", Some(10)), Some(&picture)), "10 B · PNG file · 4000 × 2000 px");
    }
}
