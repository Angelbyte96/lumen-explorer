//! Per-type icons drawn with the painter: no image files, nothing added to
//! the binary but a few shapes.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, pos2, vec2};

use crate::fs::Entry;
use crate::ui::thumbnails::Held;

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

/// Draws the entry's icon in a square `side` points wide: the file's own
/// (`own`, an .exe's) when it is loaded, else the painted one for its type.
pub fn show(ui: &mut egui::Ui, entry: &Entry, side: f32, own: Option<egui::TextureId>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        match own {
            Some(texture) => {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                ui.painter().image(texture, rect, uv, Color32::WHITE);
            }
            None => paint(ui.painter(), rect, kind_of(entry)),
        }
    }
    response
}

/// A picture `size` pixels big in a square `side` points wide: fitted with
/// its shape, never bigger than its own pixels.
pub fn picture(ui: &mut egui::Ui, side: f32, texture: egui::TextureId, size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    if ui.is_rect_visible(rect) && size.x > 0.0 && size.y > 0.0 {
        paint_picture(ui.painter(), fitted(rect, size / ui.ctx().pixels_per_point()), texture);
    }
    response
}

/// `own` (a picture's size in points) scaled to fit `area`, centred, never
/// bigger than itself.
fn fitted(area: Rect, own: egui::Vec2) -> Rect {
    let scale = (area.width() / own.x).min(area.height() / own.y).min(1.0);
    Rect::from_center_size(area.center(), own * scale)
}

fn paint_picture(painter: &egui::Painter, rect: Rect, texture: egui::TextureId) {
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.image(texture, rect, uv, Color32::WHITE);
}

/// A folder drawn by Lumen in a square `side` points wide, for the Icons
/// views: its back, what it holds sticking out of it, then its front. `own`
/// is the theme's icon for the file it holds, if loaded.
pub fn folder_with(ui: &mut egui::Ui, side: f32, held: &Held, own: Option<egui::TextureId>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_held_folder(ui.painter(), rect.shrink(side * 0.08), held, own, ui.ctx().pixels_per_point());
    }
    response
}

fn paint_held_folder(painter: &egui::Painter, rect: Rect, held: &Held, own: Option<egui::TextureId>, pixels_per_point: f32) {
    let (w, h) = (rect.width(), rect.height());
    let at = |x: f32, y: f32| pos2(rect.left() + w * x, rect.top() + h * y);
    let back_top = 0.22;
    painter.rect_filled(Rect::from_min_max(at(0.0, back_top - 0.07), at(0.4, back_top + 0.05)), 1.5, FOLDER_TAB);
    painter.rect_filled(Rect::from_min_max(at(0.0, back_top), at(1.0, 1.0)), 1.5, FOLDER_TAB);
    let sheet = Rect::from_min_max(at(0.05, 0.0), at(0.95, 0.74));
    match held {
        Held::Nothing => {}
        Held::Folders => {
            // Three blank sheets, the back ones lower and to the sides.
            for (dx, dy) in [(-0.08, 0.06), (0.08, 0.03), (0.0, 0.0)] {
                page(painter, sheet.translate(vec2(w * dx, h * dy)).shrink2(vec2(0.0, h * 0.06)));
            }
        }
        Held::Picture(texture, size) => {
            // Standing in the folder: its bottom edge rests where the front starts.
            let area = Rect::from_min_max(at(0.1, 0.0), at(0.9, 0.66));
            let shown = fitted(area, *size / pixels_per_point);
            paint_picture(painter, shown.translate(vec2(0.0, area.bottom() - shown.bottom())), *texture);
        }
        Held::File(file) => {
            page(painter, sheet);
            match own {
                Some(texture) => {
                    let side = sheet.width() * 0.78 * 0.6;
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    let icon = Rect::from_center_size(pos2(sheet.center().x, sheet.top() + sheet.height() * 0.5), vec2(side, side));
                    painter.image(texture, icon, uv, Color32::WHITE);
                }
                None => marker(painter, sheet, kind_of(file)),
            }
        }
    }
    painter.rect_filled(Rect::from_min_max(at(0.0, 0.62), at(1.0, 1.0)), 2.0, FOLDER);
}

/// The Tree view's chevron in a square `side` points wide: `>` for a folded
/// folder, `v` for an unfolded one. Drawn, so it needs no glyph.
pub fn chevron(ui: &mut egui::Ui, side: f32, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    let (c, r) = (rect.center(), side * 0.2);
    let corners = if open {
        vec![c + vec2(-r, -r * 0.5), c + vec2(0.0, r * 0.5), c + vec2(r, -r * 0.5)]
    } else {
        vec![c + vec2(-r * 0.5, -r), c + vec2(r * 0.5, 0.0), c + vec2(-r * 0.5, r)]
    };
    ui.painter().add(Shape::line(corners, Stroke::new((side * 0.12).max(1.0), ui.visuals().text_color())));
    response
}

/// Corner rounding of the lock's body; tests find the lock by it.
pub const LOCK_ROUNDING: u8 = 2;

/// A padlock filling `rect`: a body below and a shackle arching over it.
/// Drawn, so it needs no glyph.
pub fn paint_lock(painter: &egui::Painter, rect: Rect, color: Color32) {
    let side = rect.width().min(rect.height());
    let c = rect.center();
    let body = Rect::from_center_size(c + vec2(0.0, side * 0.15), vec2(side * 0.62, side * 0.45));
    painter.rect_filled(body, LOCK_ROUNDING, color);
    let (r, top) = (side * 0.2, body.top());
    let arch: Vec<Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 12.0;
            pos2(c.x - r * a.cos(), top - side * 0.08 - r * a.sin())
        })
        .collect();
    let mut shackle = vec![pos2(c.x - r, top)];
    shackle.extend(arch);
    shackle.push(pos2(c.x + r, top));
    painter.add(Shape::line(shackle, Stroke::new((side * 0.1).max(1.0), color)));
}

/// The side panel button: a window outline with its left strip split off.
pub fn paint_side_panel(painter: &egui::Painter, rect: Rect, color: Color32) {
    let side = rect.width().min(rect.height());
    let frame = Rect::from_center_size(rect.center(), vec2(side * 0.56, side * 0.46));
    let stroke = Stroke::new((side * 0.06).max(1.0), color);
    painter.rect_stroke(frame, side * 0.06, stroke, egui::StrokeKind::Inside);
    let x = frame.left() + frame.width() * 0.36;
    painter.line_segment([pos2(x, frame.top()), pos2(x, frame.bottom())], stroke);
}

/// The glyph box of a window button's icon: a square of 40% of `rect`'s side.
fn window_glyph(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), vec2(1.0, 1.0) * rect.width().min(rect.height()) * 0.4)
}

/// A thin line, whatever the scale: the stroke of the window buttons' icons.
fn window_stroke(color: Color32) -> Stroke {
    Stroke::new(1.0, color)
}

/// The minimize button: a line across the glyph box.
pub fn paint_minimize(painter: &egui::Painter, rect: Rect, color: Color32) {
    let g = window_glyph(rect);
    let y = g.center().y.round() + 0.5;
    painter.line_segment([pos2(g.left(), y), pos2(g.right(), y)], window_stroke(color));
}

/// The Search Tabs button: a chevron pointing down.
pub fn paint_search_tabs(painter: &egui::Painter, rect: Rect, color: Color32) {
    let g = window_glyph(rect);
    let half = g.width() * 0.5;
    let (x, y) = (g.center().x, g.center().y.round() + 0.5);
    painter.add(Shape::line(vec![pos2(x - half, y - half * 0.5), pos2(x, y + half * 0.5), pos2(x + half, y - half * 0.5)], window_stroke(color)));
}

/// The maximize button: a square.
pub fn paint_maximize(painter: &egui::Painter, rect: Rect, color: Color32) {
    let g = window_glyph(rect);
    painter.rect_stroke(g, 0, window_stroke(color), egui::StrokeKind::Inside);
}

/// The restore button, for a maximized window: two overlapping squares, the
/// one behind showing only its top and right sides.
pub fn paint_restore(painter: &egui::Painter, rect: Rect, color: Color32) {
    let g = window_glyph(rect);
    let side = g.width() * 0.75;
    let front = Rect::from_min_size(pos2(g.left(), g.bottom() - side), vec2(side, side));
    let back = Rect::from_min_size(pos2(g.right() - side, g.top()), vec2(side, side));
    let stroke = window_stroke(color);
    painter.rect_stroke(front, 0, stroke, egui::StrokeKind::Inside);
    painter.add(Shape::line(
        vec![pos2(back.left(), front.top()), back.left_top(), back.right_top(), back.right_bottom(), pos2(front.right(), back.bottom())],
        stroke,
    ));
}

/// The close button: an ×.
pub fn paint_close(painter: &egui::Painter, rect: Rect, color: Color32) {
    let g = window_glyph(rect);
    let stroke = window_stroke(color);
    painter.line_segment([g.left_top(), g.right_bottom()], stroke);
    painter.line_segment([g.right_top(), g.left_bottom()], stroke);
}

/// A folder icon, for rows that are not entries (the side panel).
pub fn folder(ui: &mut egui::Ui) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    paint(ui.painter(), rect, IconKind::Folder);
    response
}

/// A loaded picture (Windows' icon for a side panel row), as big as the
/// painted icons beside it.
pub fn image(ui: &mut egui::Ui, texture: egui::TextureId) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    ui.painter().image(texture, rect, uv, Color32::WHITE);
    response
}

/// A one-colour icon drawn in white (the theme's symbolic ones), tinted
/// with the text colour, as big as the text beside it.
pub fn symbol(ui: &mut egui::Ui, texture: egui::TextureId) -> egui::Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    ui.painter().image(texture, rect, uv, ui.visuals().text_color());
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

/// The line icons of the right-click menu, in `color`, inside `rect`.
pub fn menu_icon(painter: &egui::Painter, rect: Rect, icon: crate::menu::MenuIcon, color: Color32) {
    use crate::menu::MenuIcon as I;
    let s = rect.width();
    let stroke = Stroke::new((s * 0.09).max(1.0), color);
    // Points in a 0..1 box, so each icon reads like a little drawing.
    let p = |x: f32, y: f32| rect.min + vec2(x * s, y * s);
    let line = |points: &[(f32, f32)]| {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| p(x, y)).collect();
        painter.add(Shape::line(points, stroke));
    };
    let frame = |x0: f32, y0: f32, x1: f32, y1: f32| {
        painter.rect_stroke(Rect::from_min_max(p(x0, y0), p(x1, y1)), s * 0.08, stroke, egui::StrokeKind::Inside);
    };
    match icon {
        I::Open => {
            line(&[(0.55, 0.15), (0.15, 0.15), (0.15, 0.85), (0.85, 0.85), (0.85, 0.45)]);
            line(&[(0.45, 0.55), (0.85, 0.15)]);
            line(&[(0.6, 0.15), (0.85, 0.15), (0.85, 0.4)]);
        }
        I::OpenWith => {
            frame(0.1, 0.1, 0.42, 0.42);
            frame(0.58, 0.1, 0.9, 0.42);
            frame(0.1, 0.58, 0.42, 0.9);
            frame(0.58, 0.58, 0.9, 0.9);
        }
        I::NewTab => {
            frame(0.1, 0.2, 0.9, 0.85);
            line(&[(0.1, 0.38), (0.9, 0.38)]);
            line(&[(0.5, 0.5), (0.5, 0.75)]);
            line(&[(0.38, 0.62), (0.62, 0.62)]);
        }
        I::Cut => {
            painter.circle_stroke(p(0.28, 0.75), s * 0.13, stroke);
            painter.circle_stroke(p(0.72, 0.75), s * 0.13, stroke);
            line(&[(0.36, 0.64), (0.75, 0.1)]);
            line(&[(0.64, 0.64), (0.25, 0.1)]);
        }
        I::Copy => {
            frame(0.12, 0.12, 0.62, 0.68);
            line(&[(0.75, 0.32), (0.88, 0.32), (0.88, 0.88), (0.38, 0.88), (0.38, 0.8)]);
        }
        I::CopyPath => {
            frame(0.15, 0.1, 0.85, 0.9);
            line(&[(0.62, 0.25), (0.38, 0.75)]);
        }
        I::Paste => {
            frame(0.18, 0.18, 0.82, 0.9);
            frame(0.35, 0.1, 0.65, 0.28);
            line(&[(0.32, 0.5), (0.68, 0.5)]);
            line(&[(0.32, 0.68), (0.6, 0.68)]);
        }
        I::Rename => {
            line(&[(0.15, 0.85), (0.2, 0.62), (0.7, 0.12), (0.88, 0.3), (0.38, 0.8), (0.15, 0.85)]);
            line(&[(0.6, 0.22), (0.78, 0.4)]);
        }
        I::Trash => {
            line(&[(0.12, 0.25), (0.88, 0.25)]);
            line(&[(0.38, 0.25), (0.42, 0.1), (0.58, 0.1), (0.62, 0.25)]);
            line(&[(0.22, 0.25), (0.28, 0.9), (0.72, 0.9), (0.78, 0.25)]);
            line(&[(0.42, 0.4), (0.42, 0.75)]);
            line(&[(0.58, 0.4), (0.58, 0.75)]);
        }
        I::Delete => {
            painter.circle_stroke(p(0.5, 0.5), s * 0.4, stroke);
            line(&[(0.34, 0.34), (0.66, 0.66)]);
            line(&[(0.66, 0.34), (0.34, 0.66)]);
        }
        I::Bookmark => line(&[(0.25, 0.1), (0.75, 0.1), (0.75, 0.9), (0.5, 0.68), (0.25, 0.9), (0.25, 0.1)]),
        I::Size => {
            line(&[(0.1, 0.9), (0.9, 0.9)]);
            line(&[(0.25, 0.9), (0.25, 0.55)]);
            line(&[(0.5, 0.9), (0.5, 0.3)]);
            line(&[(0.75, 0.9), (0.75, 0.12)]);
        }
        I::New => {
            line(&[(0.5, 0.12), (0.5, 0.88)]);
            line(&[(0.12, 0.5), (0.88, 0.5)]);
        }
        I::NewFolder => {
            line(&[(0.1, 0.85), (0.1, 0.15), (0.4, 0.15), (0.5, 0.28), (0.9, 0.28), (0.9, 0.85), (0.1, 0.85)]);
            line(&[(0.5, 0.42), (0.5, 0.72)]);
            line(&[(0.35, 0.57), (0.65, 0.57)]);
        }
        I::NewFile => {
            line(&[(0.2, 0.1), (0.6, 0.1), (0.8, 0.3), (0.8, 0.9), (0.2, 0.9), (0.2, 0.1)]);
            line(&[(0.5, 0.45), (0.5, 0.75)]);
            line(&[(0.35, 0.6), (0.65, 0.6)]);
        }
        I::Terminal => {
            frame(0.08, 0.15, 0.92, 0.85);
            line(&[(0.25, 0.38), (0.42, 0.52), (0.25, 0.66)]);
            line(&[(0.5, 0.68), (0.72, 0.68)]);
        }
        I::Sort => {
            line(&[(0.3, 0.85), (0.3, 0.15)]);
            line(&[(0.15, 0.3), (0.3, 0.15), (0.45, 0.3)]);
            line(&[(0.7, 0.15), (0.7, 0.85)]);
            line(&[(0.55, 0.7), (0.7, 0.85), (0.85, 0.7)]);
        }
        I::Refresh => {
            let center = p(0.5, 0.5);
            let r = s * 0.35;
            let arc: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.25 + 1.5 * i as f32 / 16.0);
                    center + vec2(a.cos(), -a.sin()) * r
                })
                .collect();
            let end = *arc.last().expect("points");
            painter.add(Shape::line(arc, stroke));
            painter.add(Shape::line(vec![end + vec2(-s * 0.18, 0.0), end, end + vec2(0.0, -s * 0.18)], stroke));
        }
        I::GoTo => {
            line(&[(0.1, 0.5), (0.72, 0.5)]);
            line(&[(0.5, 0.28), (0.72, 0.5), (0.5, 0.72)]);
            line(&[(0.9, 0.15), (0.9, 0.85)]);
        }
        I::Commands => {
            line(&[(0.15, 0.25), (0.45, 0.5), (0.15, 0.75)]);
            line(&[(0.55, 0.78), (0.88, 0.78)]);
        }
        I::LoadLayout => {
            frame(0.1, 0.15, 0.9, 0.85);
            line(&[(0.4, 0.15), (0.4, 0.85)]);
            line(&[(0.4, 0.5), (0.9, 0.5)]);
        }
        I::SaveLayout => {
            frame(0.1, 0.15, 0.9, 0.85);
            line(&[(0.1, 0.38), (0.9, 0.38)]);
            line(&[(0.4, 0.38), (0.4, 0.85)]);
        }
        I::Options => {
            let center = p(0.5, 0.5);
            painter.circle_stroke(center, s * 0.26, stroke);
            painter.circle_stroke(center, s * 0.1, stroke);
            for n in 0..8 {
                let a = std::f32::consts::TAU * n as f32 / 8.0;
                let dir = vec2(a.cos(), a.sin());
                painter.line_segment([center + dir * s * 0.26, center + dir * s * 0.4], stroke);
            }
        }
        I::Setup => {
            line(&[(0.1, 0.6), (0.1, 0.88), (0.9, 0.88), (0.9, 0.6)]);
            line(&[(0.5, 0.1), (0.5, 0.66)]);
            line(&[(0.3, 0.46), (0.5, 0.66), (0.7, 0.46)]);
        }
    }
}

const FOLDER: Color32 = Color32::from_rgb(0xF2, 0xC1, 0x4E);
const FOLDER_TAB: Color32 = Color32::from_rgb(0xD9, 0xA2, 0x2E);
pub(crate) const PAGE: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xED);
const FOLD: Color32 = Color32::from_rgb(0xB8, 0xBC, 0xC2);

pub fn paint(painter: &egui::Painter, rect: Rect, kind: IconKind) {
    let rect = rect.shrink(rect.width() * 0.08);
    if kind == IconKind::Folder {
        paint_folder(painter, rect);
        return;
    }
    page(painter, rect);
    marker(painter, rect, kind);
}

/// The type's mark on a page whose rect is `rect` (nothing for a plain file).
fn marker(painter: &egui::Painter, rect: Rect, kind: IconKind) {
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
    fn every_menu_icon_draws_strokes_inside_its_box() {
        use crate::menu::MenuIcon;
        let ctx = egui::Context::default();
        let side = 16.0;
        let rect = Rect::from_min_size(pos2(40.0, 40.0), vec2(side, side));
        for icon in MenuIcon::ALL {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                menu_icon(ui.painter(), rect, icon, Color32::WHITE);
            });
            output.textures_delta.clear();
            let bounds = output.shapes.iter().map(|s| s.shape.visual_bounding_rect()).fold(Rect::NOTHING, |a, b| a.union(b));
            assert!(!output.shapes.is_empty(), "{icon:?} draws nothing");
            assert!(rect.expand(1.5).contains_rect(bounds), "{icon:?} spills out: {bounds:?}");
        }
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
