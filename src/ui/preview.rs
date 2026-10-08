//! The side panel opened with Space: the selected entry's contents.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

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
pub fn show(ui: &mut egui::Ui, entry: &Entry, pane: &mut PreviewPane, show_hidden: bool) {
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
        Some(Content::Folder(Ok(entries))) => folder(ui, entries, show_hidden),
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

/// The folder's entries, honouring Ctrl+H like the main list. Only visible
/// rows are laid out, so a huge folder previews as fast as a small one.
fn folder(ui: &mut egui::Ui, entries: &[Entry], show_hidden: bool) {
    let shown: Vec<&Entry> = entries.iter().filter(|e| show_hidden || !e.hidden).collect();
    if shown.is_empty() {
        ui.weak("Empty folder");
        return;
    }
    ui.weak(format!("{} items", shown.len()));
    let row_height = ui.text_style_height(&egui::TextStyle::Body);
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_height, shown.len(), |ui, range| {
        for entry in &shown[range] {
            let icon = if entry.is_dir { "🗀" } else { "🗋" };
            let text = egui::RichText::new(format!("{icon} {}", entry.name));
            let text = if entry.hidden { text.weak() } else { text };
            ui.add(egui::Label::new(text).selectable(false).truncate());
        }
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
