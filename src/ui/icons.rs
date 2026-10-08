//! Per-type icons drawn with the painter: no image files, nothing added to
//! the binary but a few shapes.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, pos2, vec2};

use crate::fs::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Folder,
    Image,
    Pdf,
    Document,
    Code,
    Archive,
    File,
}

/// Picks the icon from the extension, ignoring case.
pub fn kind_of(entry: &Entry) -> IconKind {
    if entry.is_dir {
        return IconKind::Folder;
    }
    let Some(ext) = entry.path.extension().and_then(|e| e.to_str()) else {
        return IconKind::File;
    };
    let ext = ext.to_ascii_lowercase();
    let is = |list: &[&str]| list.contains(&ext.as_str());
    if is(IMAGE) {
        IconKind::Image
    } else if ext == "pdf" {
        IconKind::Pdf
    } else if is(DOCUMENT) {
        IconKind::Document
    } else if is(CODE) {
        IconKind::Code
    } else if is(ARCHIVE) {
        IconKind::Archive
    } else {
        IconKind::File
    }
}

const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "tif", "tiff", "heic", "avif"];
const DOCUMENT: &[&str] = &[
    "txt", "md", "rtf", "doc", "docx", "odt", "xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp", "epub",
];
const CODE: &[&str] = &[
    "rs", "js", "mjs", "ts", "jsx", "tsx", "html", "htm", "css", "scss", "json", "toml", "yaml", "yml",
    "xml", "sql", "py", "sh", "bash", "zsh", "fish", "c", "h", "cpp", "hpp", "cs", "java", "kt", "go",
    "rb", "php", "lua", "vue", "svelte", "dart", "swift", "ini", "conf", "lock",
];
const ARCHIVE: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "deb", "rpm", "appimage", "iso", "jar",
];

/// Draws the entry's icon in a square `side` points wide.
pub fn show(ui: &mut egui::Ui, entry: &Entry, side: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, kind_of(entry));
    }
    response
}

/// A folder icon, for rows that are not entries (the side panel).
pub fn folder(ui: &mut egui::Ui) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    paint(ui.painter(), rect, IconKind::Folder);
    response
}

/// A plain file icon, for the status bar count.
pub fn file(ui: &mut egui::Ui) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    paint(ui.painter(), rect, IconKind::File);
    response
}

/// A magnifying glass, for the filter bar (the fonts have none).
pub fn magnifier(ui: &mut egui::Ui) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    let color = ui.visuals().weak_text_color();
    let stroke = Stroke::new(side * 0.1, color);
    let r = side * 0.27;
    let center = rect.center() - vec2(side * 0.08, side * 0.08);
    ui.painter().circle_stroke(center, r, stroke);
    let handle = vec2(1.0, 1.0).normalized();
    ui.painter().line_segment([center + handle * r, center + handle * (r + side * 0.28)], Stroke::new(side * 0.13, color));
    response
}

const FOLDER: Color32 = Color32::from_rgb(0xF2, 0xC1, 0x4E);
const FOLDER_TAB: Color32 = Color32::from_rgb(0xD9, 0xA2, 0x2E);
const PAGE: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xED);
const FOLD: Color32 = Color32::from_rgb(0xB8, 0xBC, 0xC2);

fn paint(painter: &egui::Painter, rect: Rect, kind: IconKind) {
    let rect = rect.shrink(rect.width() * 0.08);
    if kind == IconKind::Folder {
        paint_folder(painter, rect);
        return;
    }
    page(painter, rect);
    let accent = match kind {
        IconKind::Image => Color32::from_rgb(0x3D, 0x9B, 0xE9),
        IconKind::Pdf => Color32::from_rgb(0xE0, 0x3E, 0x36),
        IconKind::Document => Color32::from_rgb(0x2F, 0x6F, 0xD6),
        IconKind::Code => Color32::from_rgb(0x3F, 0xB9, 0x6B),
        IconKind::Archive => Color32::from_rgb(0xB0, 0x7A, 0x3A),
        IconKind::File | IconKind::Folder => return,
    };
    // The marker sits in the lower part of the page, under the fold.
    let w = rect.width();
    let body = Rect::from_min_max(
        pos2(rect.left() + w * 0.18, rect.top() + rect.height() * 0.38),
        pos2(rect.right() - w * 0.12, rect.bottom() - rect.height() * 0.1),
    );
    match kind {
        IconKind::Image => {
            // A mountain and a sun.
            let base = body.bottom();
            painter.add(Shape::convex_polygon(
                vec![pos2(body.left(), base), pos2(body.center().x - w * 0.05, body.top() + body.height() * 0.3), pos2(body.right(), base)],
                accent,
                Stroke::NONE,
            ));
            painter.circle_filled(pos2(body.right() - w * 0.1, body.top() + w * 0.05), w * 0.07, Color32::from_rgb(0xF2, 0xC1, 0x4E));
        }
        IconKind::Pdf => {
            painter.rect_filled(Rect::from_min_max(pos2(rect.left(), body.center().y), pos2(rect.right() - w * 0.08, body.bottom())), 1.0, accent);
        }
        IconKind::Document => {
            for i in 0..3 {
                let y = body.top() + body.height() * (0.15 + 0.35 * i as f32);
                painter.line_segment([pos2(body.left(), y), pos2(body.right(), y)], Stroke::new(w * 0.08, accent));
            }
        }
        IconKind::Code => {
            // "< >"
            let stroke = Stroke::new(w * 0.09, accent);
            let (l, r, c) = (body.left(), body.right(), body.center());
            let half = body.height() * 0.4;
            painter.line_segment([pos2(c.x - w * 0.08, c.y - half), pos2(l, c.y)], stroke);
            painter.line_segment([pos2(l, c.y), pos2(c.x - w * 0.08, c.y + half)], stroke);
            painter.line_segment([pos2(c.x + w * 0.08, c.y - half), pos2(r, c.y)], stroke);
            painter.line_segment([pos2(r, c.y), pos2(c.x + w * 0.08, c.y + half)], stroke);
        }
        IconKind::Archive => {
            // A zipper down the middle.
            let x = rect.center().x;
            let step = rect.height() * 0.12;
            let mut y = rect.top() + step;
            while y < rect.bottom() - step {
                painter.rect_filled(Rect::from_center_size(pos2(x, y), vec2(w * 0.22, step * 0.6)), 0.0, accent);
                y += step * 1.5;
            }
        }
        IconKind::File | IconKind::Folder => {}
    }
}

/// A sheet with its top-right corner folded.
fn page(painter: &egui::Painter, rect: Rect) {
    let w = rect.width() * 0.78;
    let left = rect.center().x - w / 2.0;
    let right = left + w;
    let fold = w * 0.32;
    let points: Vec<Pos2> = vec![
        pos2(left, rect.top()),
        pos2(right - fold, rect.top()),
        pos2(right, rect.top() + fold),
        pos2(right, rect.bottom()),
        pos2(left, rect.bottom()),
    ];
    painter.add(Shape::convex_polygon(points, PAGE, Stroke::NONE));
    painter.add(Shape::convex_polygon(
        vec![pos2(right - fold, rect.top()), pos2(right, rect.top() + fold), pos2(right - fold, rect.top() + fold)],
        FOLD,
        Stroke::NONE,
    ));
}

/// A folder: a tab on top of a wider body.
fn paint_folder(painter: &egui::Painter, rect: Rect) {
    let h = rect.height();
    let top = rect.top() + h * 0.15;
    let bottom = rect.bottom() - h * 0.1;
    let tab = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.left() + rect.width() * 0.45, top + h * 0.2));
    painter.rect_filled(tab, 1.5, FOLDER_TAB);
    let body = Rect::from_min_max(pos2(rect.left(), top + h * 0.12), pos2(rect.right(), bottom));
    painter.rect_filled(body, 1.5, FOLDER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            is_dir,
            size: None,
            modified: None,
            hidden: false,
        }
    }

    fn kind(name: &str) -> IconKind {
        kind_of(&entry(name, false))
    }

    #[test]
    fn folders_get_the_folder_icon_whatever_their_name() {
        assert_eq!(kind_of(&entry("photos.png", true)), IconKind::Folder);
        assert_eq!(kind_of(&entry("src", true)), IconKind::Folder);
    }

    #[test]
    fn files_get_their_icon_from_the_extension_ignoring_case() {
        for (name, expected) in [
            ("a.png", IconKind::Image),
            ("b.JPG", IconKind::Image),
            ("c.svg", IconKind::Image),
            ("Crear_BD.pdf", IconKind::Pdf),
            ("CV.docx", IconKind::Document),
            ("notes.txt", IconKind::Document),
            ("sheet.xlsx", IconKind::Document),
            ("main.rs", IconKind::Code),
            ("index.HTML", IconKind::Code),
            ("query.sql", IconKind::Code),
            ("Config_modem.xml", IconKind::Code),
            ("backup.tar.gz", IconKind::Archive),
            ("x.zip", IconKind::Archive),
            ("Makefile", IconKind::File),
            (".bashrc", IconKind::File),
            ("data.bin", IconKind::File),
        ] {
            assert_eq!(kind(name), expected, "{name}");
        }
    }
}
