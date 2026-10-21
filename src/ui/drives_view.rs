//! The This PC view (Windows): one row per drive, with its type, file system,
//! free space, size and how full it is. Drawn instead of the file list.

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::format::format_size;
use crate::places::{DriveInfo, Mount, Usage};
use crate::ui::sidebar::{DISK, used_fraction};

/// A drive as its row shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveRow {
    pub path: PathBuf,
    /// "Windows (C:)", as the side panel's Storage names it.
    pub name: String,
    /// "Local Disk", "Removable Disk"...
    pub kind: &'static str,
    /// "NTFS", "FAT32"; empty when unknown.
    pub fs: String,
    /// Empty (with `size` and `percent`) when the space could not be read.
    pub free: String,
    pub size: String,
    /// Share in use, 0.0 to 1.0, for the bar.
    pub used: Option<f32>,
    pub percent: String,
}

/// "54.3%"; a whole number loses its ".0" ("0%").
pub fn percent_text(usage: &Usage) -> String {
    let used = if usage.total == 0 { 0.0 } else { usage.total.saturating_sub(usage.free) as f64 / usage.total as f64 * 100.0 };
    let text = format!("{used:.1}");
    format!("{}%", text.strip_suffix(".0").unwrap_or(&text))
}

/// The rows for the side panel's disks; `info` says each one's kind and file system.
pub fn rows(disks: &[(Mount, Option<Usage>)], info: impl Fn(&Path) -> DriveInfo) -> Vec<DriveRow> {
    disks
        .iter()
        .map(|(mount, usage)| {
            let info = info(&mount.path);
            DriveRow {
                path: mount.path.clone(),
                name: mount.label.clone(),
                kind: info.kind.name(),
                fs: info.fs,
                free: usage.map(|u| format_size(u.free)).unwrap_or_default(),
                size: usage.map(|u| format_size(u.total)).unwrap_or_default(),
                used: usage.as_ref().map(used_fraction),
                percent: usage.as_ref().map(percent_text).unwrap_or_default(),
            }
        })
        .collect()
}

/// What the view asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrivesEvent {
    Select(usize),
    /// Double click: open the drive.
    Open(usize),
    /// Right click: the drive's menu.
    Menu(usize),
}

const COLUMNS: [&str; 6] = ["Name", "Type", "File System", "Free Space", "Size", "Percent Full"];

pub fn show(ui: &mut egui::Ui, rows: &[DriveRow], selected: Option<&Path>) -> Option<DrivesEvent> {
    let mut event = None;
    if rows.is_empty() {
        ui.weak("No drives");
        return None;
    }
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .auto_shrink([false, true])
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(egui::Sense::CLICK)
        .column(Column::initial(260.0).at_least(120.0).clip(true))
        .column(Column::initial(130.0).at_least(60.0).clip(true))
        .column(Column::initial(100.0).at_least(60.0).clip(true))
        .column(Column::initial(110.0).at_least(60.0).clip(true))
        .column(Column::initial(110.0).at_least(60.0).clip(true))
        .column(Column::remainder().at_least(140.0).clip(true))
        .header(row_height, |mut header| {
            for label in COLUMNS {
                header.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let i = row.index();
                let drive = &rows[i];
                row.set_selected(selected == Some(drive.path.as_path()));
                row.col(|ui| {
                    // Not selectable, or the text would take the row's click.
                    ui.add(egui::Label::new(DISK).selectable(false));
                    ui.add(egui::Label::new(&drive.name).selectable(false).truncate());
                });
                for text in [drive.kind, &drive.fs, &drive.free, &drive.size] {
                    row.col(|ui| {
                        ui.add(egui::Label::new(text).selectable(false).truncate());
                    });
                }
                row.col(|ui| {
                    if let Some(used) = drive.used {
                        let width = ui.available_width();
                        ui.add(egui::ProgressBar::new(used).desired_width(width).desired_height(14.0).text(&drive.percent));
                    }
                });
                let response = row.response();
                if response.secondary_clicked() {
                    event = Some(DrivesEvent::Menu(i));
                } else if response.double_clicked() {
                    event = Some(DrivesEvent::Open(i));
                } else if response.clicked() {
                    event = Some(DrivesEvent::Select(i));
                }
            });
        });
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::DriveKind;

    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    fn usage(free: f64, total: f64) -> Usage {
        Usage { free: (free * GB) as u64, total: (total * GB) as u64 }
    }

    fn mount(label: &str, root: &str) -> Mount {
        Mount { label: label.into(), path: PathBuf::from(root) }
    }

    fn info(root: &Path) -> DriveInfo {
        match root.to_str() {
            Some("C:\\") => DriveInfo { kind: DriveKind::Local, fs: "NTFS".into() },
            _ => DriveInfo { kind: DriveKind::Removable, fs: "FAT".into() },
        }
    }

    /// File Pilot's This PC on the user's machine (L3).
    fn two_drives() -> Vec<DriveRow> {
        let disks = [
            (mount("Windows (C:)", "C:\\"), Some(usage(210.6, 461.2))),
            (mount("Unidad de USB (D:)", "D:\\"), Some(usage(1.8, 1.8))),
        ];
        rows(&disks, info)
    }

    #[test]
    fn a_drive_row_reads_like_file_pilots() {
        let rows = two_drives();
        let cells = |r: &DriveRow| [r.name.clone(), r.kind.into(), r.fs.clone(), r.free.clone(), r.size.clone(), r.percent.clone()];
        assert_eq!(cells(&rows[0]), ["Windows (C:)", "Local Disk", "NTFS", "210.6 GB", "461.2 GB", "54.3%"]);
        assert_eq!(cells(&rows[1]), ["Unidad de USB (D:)", "Removable Disk", "FAT", "1.8 GB", "1.8 GB", "0%"]);
        assert_eq!(rows[0].path, PathBuf::from("C:\\"));
        assert!((rows[0].used.unwrap() - 0.543).abs() < 0.001 && rows[1].used == Some(0.0), "{:?}", rows.iter().map(|r| r.used).collect::<Vec<_>>());
    }

    #[test]
    fn percents_show_one_decimal_unless_whole() {
        let text = |free: u64, total: u64| percent_text(&Usage { free, total });
        assert_eq!(text(50, 100), "50%");
        assert_eq!(text(100, 100), "0%");
        assert_eq!(text(0, 100), "100%");
        assert_eq!(text(9_995, 10_000), "0.1%", "rounds to the tenth");
        assert_eq!(text(10_000, 10_000), "0%");
        assert_eq!(text(0, 0), "0%", "an empty disk is not a division by zero");
    }

    #[test]
    fn a_drive_whose_space_could_not_be_read_has_no_numbers_and_no_bar() {
        let disks = [(mount("Windows (C:)", "C:\\"), None)];
        let row = &rows(&disks, info)[0];
        assert_eq!((row.name.as_str(), row.kind, row.fs.as_str()), ("Windows (C:)", "Local Disk", "NTFS"));
        assert_eq!((row.free.as_str(), row.size.as_str(), row.percent.as_str(), row.used), ("", "", "", None));
    }

    /// One frame of the view with the drives selected as `selected`.
    fn draw(ctx: &egui::Context, rows: &[DriveRow], selected: Option<&Path>, events: Vec<egui::Event>, time: Option<f64>) -> (Option<DrivesEvent>, Vec<egui::epaint::ClippedShape>) {
        let mut event = None;
        let input = egui::RawInput { events, time, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| event = show(ui, rows, selected));
        });
        output.textures_delta.clear();
        (event, output.shapes)
    }

    fn settled(ctx: &egui::Context, rows: &[DriveRow], selected: Option<&Path>) -> Vec<egui::epaint::ClippedShape> {
        draw(ctx, rows, selected, vec![], None);
        draw(ctx, rows, selected, vec![], None).1
    }

    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Rect {
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{text:?} painted"))
    }

    fn click(at: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
        let press = |pressed| egui::Event::PointerButton { pos: at, button, pressed, modifiers: egui::Modifiers::NONE };
        vec![egui::Event::PointerMoved(at), press(true), press(false)]
    }

    #[test]
    fn the_view_has_the_six_columns_and_a_row_per_drive() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        for text in COLUMNS.into_iter().chain(["Windows (C:)", "Local Disk", "NTFS", "210.6 GB", "461.2 GB", "54.3%", "Unidad de USB (D:)", "Removable Disk", "FAT", "1.8 GB", "0%"]) {
            text_rect(&shapes, text);
        }
    }

    #[test]
    fn a_click_selects_a_drive_a_double_click_opens_it_and_a_right_click_asks_for_its_menu() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let (c, d) = (text_rect(&shapes, "Windows (C:)").center(), text_rect(&shapes, "Unidad de USB (D:)").center());
        let event = |at, button| draw(&ctx, &rows, None, click(at, button), None).0;
        assert_eq!(event(d, egui::PointerButton::Primary), Some(DrivesEvent::Select(1)));
        settled(&ctx, &rows, None);
        assert_eq!(event(c, egui::PointerButton::Secondary), Some(DrivesEvent::Menu(0)));
        settled(&ctx, &rows, None);
        draw(&ctx, &rows, None, click(d, egui::PointerButton::Primary), Some(10.0));
        let second = draw(&ctx, &rows, None, click(d, egui::PointerButton::Primary), Some(10.1)).0;
        assert_eq!(second, Some(DrivesEvent::Open(1)));
    }

    #[test]
    fn a_click_on_any_cell_of_the_row_selects_it() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let name = text_rect(&shapes, "Windows (C:)");
        for text in ["Local Disk", "NTFS", "210.6 GB", "461.2 GB"] {
            let at = egui::pos2(text_rect(&shapes, text).center().x, name.center().y);
            assert_eq!(draw(&ctx, &rows, None, click(at, egui::PointerButton::Primary), None).0, Some(DrivesEvent::Select(0)), "{text}");
            settled(&ctx, &rows, None);
        }
    }

    #[test]
    fn no_drives_draws_a_line_and_asks_for_nothing() {
        let ctx = egui::Context::default();
        let (event, shapes) = draw(&ctx, &[], None, vec![], None);
        assert_eq!(event, None);
        text_rect(&shapes, "No drives");
    }
}
