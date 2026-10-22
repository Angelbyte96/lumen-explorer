//! The This PC view (Windows): one row per drive, with its type, file system,
//! free space, size and how full it is. Drawn instead of the file list, with
//! the same headers: click to sort, borders to resize, a menu to fit.

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::format::format_size;
use crate::places::{DriveInfo, Mount, Usage};
use crate::ui::file_list;
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
    /// The numbers behind `free` and `size`, for sorting.
    pub usage: Option<Usage>,
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
                usage: *usage,
            }
        })
        .collect()
}

/// A column of the view, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveColumn {
    Name,
    Type,
    FileSystem,
    FreeSpace,
    Size,
    PercentFull,
}

/// Header labels and the column each one sorts, in display order.
pub const COLUMNS: [(&str, DriveColumn); 6] = [
    ("Name", DriveColumn::Name),
    ("Type", DriveColumn::Type),
    ("File System", DriveColumn::FileSystem),
    ("Free Space", DriveColumn::FreeSpace),
    ("Size", DriveColumn::Size),
    ("Percent Full", DriveColumn::PercentFull),
];

// Fit All Columns shares the list's rule (`file_list::fit_all`), which works on
// its six columns and says by role who gives room when they do not fit.
const _: () = assert!(COLUMNS.len() == file_list::COLUMNS.len());

/// Each column's role: Free Space and Size are numbers, the rest keep priority.
pub const ROLES: file_list::Roles = {
    use file_list::Role::*;
    [Name, Short, Short, Numeric, Numeric, Short]
};

/// A fingerprint of what the rows show, to tell when the columns need fitting again.
pub fn signature(rows: &[DriveRow]) -> u64 {
    file_list::signature(rows.iter().map(|r| (&r.name, r.kind, &r.fs, &r.free, &r.size, &r.percent)))
}

/// Each column's width in points, like the list's.
pub type Widths = file_list::Widths;

/// The widths the view starts with.
pub const DEFAULT_WIDTHS: Widths = [260.0, 130.0, 100.0, 110.0, 110.0, 140.0];

/// How the drives are ordered when a header was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveSort {
    pub column: DriveColumn,
    pub descending: bool,
}

impl DriveSort {
    /// A header click: the sorted column flips direction, another starts ascending.
    pub fn clicked(current: Option<Self>, column: DriveColumn) -> Self {
        Self { column, descending: current.is_some_and(|sort| sort.column == column && !sort.descending) }
    }
}

/// Orders `rows` by `sort`; ties keep the drives' letter order. Unsorted
/// (`None`) leaves them as they are. A drive whose space is unknown sorts
/// before the others.
pub fn sort(rows: &mut [DriveRow], sort: Option<DriveSort>) {
    let Some(DriveSort { column, descending }) = sort else { return };
    rows.sort_by(|a, b| {
        let by_column = match column {
            DriveColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            DriveColumn::Type => a.kind.cmp(b.kind),
            DriveColumn::FileSystem => a.fs.to_lowercase().cmp(&b.fs.to_lowercase()),
            DriveColumn::FreeSpace => a.usage.map(|u| u.free).cmp(&b.usage.map(|u| u.free)),
            DriveColumn::Size => a.usage.map(|u| u.total).cmp(&b.usage.map(|u| u.total)),
            DriveColumn::PercentFull => match (a.used, b.used) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (x, y) => x.is_some().cmp(&y.is_some()),
            },
        };
        let by_column = if descending { by_column.reverse() } else { by_column };
        by_column.then_with(|| a.path.cmp(&b.path))
    });
}

/// Air around the percent in its bar.
const PERCENT_AIR: f32 = 16.0;

/// The width that shows every cell of `column` and its header whole.
pub fn fit_width(ctx: &egui::Context, rows: &[DriveRow], column: usize) -> f32 {
    let measure = file_list::body_measure(ctx);
    let gap = ctx.global_style().spacing.item_spacing.x;
    let widest = rows
        .iter()
        .map(|row| match column {
            0 => measure(DISK.to_string()) + gap + measure(row.name.clone()),
            1 => measure(row.kind.to_string()),
            2 => measure(row.fs.clone()),
            3 => measure(row.free.clone()),
            4 => measure(row.size.clone()),
            _ => measure(row.percent.clone()) + PERCENT_AIR,
        })
        .fold(file_list::header_width(ctx, COLUMNS[column].0), f32::max);
    file_list::fitted(widest)
}

/// What the view asks of the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrivesEvent {
    Select(usize),
    /// Double click: open the drive.
    Open(usize),
    /// Right click: the drive's menu.
    Menu(usize),
    /// A click on a header: sort by the column.
    Sort(DriveColumn),
    /// Right click on a header: the column's menu.
    HeaderMenu(usize),
    /// A column border was dragged and let go: the new widths.
    Resized(Widths),
    /// A double click on a column's right border: fit the column to its content.
    Fit(usize),
}

/// Draws the drives as `sort` orders them (the rows come sorted) at `widths`.
pub fn show(ui: &mut egui::Ui, rows: &[DriveRow], selected: Option<&Path>, sort: Option<DriveSort>, widths: &Widths) -> Option<DrivesEvent> {
    let mut event = None;
    if rows.is_empty() {
        ui.weak("No drives");
        return None;
    }
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
    file_list::remember_width(ui.ctx(), ui.available_rect_before_wrap().width());
    let widths = file_list::live_widths(ui, widths);
    let mut cells = [egui::Rect::NOTHING; COLUMNS.len()];
    egui::ScrollArea::horizontal().id_salt("lumen_drives_columns").auto_shrink([false, true]).show(ui, |ui| {
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .auto_shrink([false, true])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::CLICK);
        // Every column has its own width, like the list's; the last stretches over the room left.
        for &width in &widths[..COLUMNS.len() - 1] {
            table = table.column(Column::exact(width).clip(true));
        }
        table
            .column(Column::remainder().at_least(widths[COLUMNS.len() - 1]).clip(true))
            .header(row_height, |mut header| {
                for (i, (label, column)) in COLUMNS.into_iter().enumerate() {
                    header.col(|ui| {
                        cells[i] = ui.max_rect();
                        let direction = sort.filter(|sort| sort.column == column).map(|sort| sort.descending);
                        match file_list::header_cell(ui, i, label, direction) {
                            Some(file_list::HeaderClick::Sort) => event = Some(DrivesEvent::Sort(column)),
                            Some(file_list::HeaderClick::Menu) => event = Some(DrivesEvent::HeaderMenu(i)),
                            // The columns are fixed: there is none to close.
                            _ => {}
                        }
                    });
                }
            })
            .body(|mut body| {
                file_list::light_hovered_rows(body.ui_mut());
                body.rows(row_height, rows.len(), |mut row| {
                    let i = row.index();
                    let drive = &rows[i];
                    row.set_selected(selected == Some(drive.path.as_path()));
                    row.col(|ui| {
                        // Not selectable, or the text would take the row's click.
                        ui.add(egui::Label::new(DISK).selectable(false));
                        ui.add(egui::Label::new(&drive.name).selectable(false).truncate());
                    });
                    // Only Size is right-aligned: Free Space beside it would read as one column.
                    for (text, right) in [(drive.kind, false), (&drive.fs, false), (&drive.free, false), (&drive.size, true)] {
                        row.col(|ui| {
                            if right {
                                file_list::number_cell(ui, text);
                            } else {
                                ui.add(egui::Label::new(text).selectable(false).truncate());
                            }
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
        match file_list::column_borders(ui, &cells, &widths) {
            Some(file_list::BorderEvent::Resized(resized)) => event = Some(DrivesEvent::Resized(resized)),
            Some(file_list::BorderEvent::Fit(column)) => event = Some(DrivesEvent::Fit(column)),
            None => {}
        }
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

    type Frame = (Option<DrivesEvent>, Vec<egui::epaint::ClippedShape>);

    /// One frame of the view with the drives selected as `selected`, sorted as `sort` and as wide as `widths`.
    fn draw_as(ctx: &egui::Context, rows: &[DriveRow], selected: Option<&Path>, look: (Option<DriveSort>, Widths), events: Vec<egui::Event>, time: Option<f64>) -> Frame {
        let mut event = None;
        let input = egui::RawInput { events, time, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| event = show(ui, rows, selected, look.0, &look.1));
        });
        output.textures_delta.clear();
        (event, output.shapes)
    }

    fn draw(ctx: &egui::Context, rows: &[DriveRow], selected: Option<&Path>, events: Vec<egui::Event>, time: Option<f64>) -> Frame {
        draw_as(ctx, rows, selected, (None, DEFAULT_WIDTHS), events, time)
    }

    fn settled(ctx: &egui::Context, rows: &[DriveRow], selected: Option<&Path>) -> Vec<egui::epaint::ClippedShape> {
        draw(ctx, rows, selected, vec![], None);
        draw(ctx, rows, selected, vec![], None).1
    }

    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Rect {
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                // A header ends in a space (or its sort arrow) once it can show one.
                egui::Shape::Text(t) if t.galley.text().trim_end() == text => Some(t.galley.rect.translate(t.pos.to_vec2())),
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
        for text in COLUMNS.map(|(label, _)| label).into_iter().chain(["Windows (C:)", "Local Disk", "NTFS", "210.6 GB", "461.2 GB", "54.3%", "Unidad de USB (D:)", "Removable Disk", "FAT", "1.8 GB", "0%"]) {
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

    /// C: 10 of 50 GB, D: (USB) 30 of 100, E: 20 of 40: every column orders them differently.
    fn three_drives() -> Vec<DriveRow> {
        let disks = [
            (mount("Windows (C:)", "C:\\"), Some(usage(10.0, 50.0))),
            (mount("Unidad de USB (D:)", "D:\\"), Some(usage(30.0, 100.0))),
            (mount("Datos (E:)", "E:\\"), Some(usage(20.0, 40.0))),
        ];
        let info = |root: &Path| match root.to_str() {
            Some("D:\\") => DriveInfo { kind: DriveKind::Removable, fs: "FAT".into() },
            Some("E:\\") => DriveInfo { kind: DriveKind::Local, fs: "exFAT".into() },
            _ => DriveInfo { kind: DriveKind::Local, fs: "NTFS".into() },
        };
        rows(&disks, info)
    }

    /// The drive letters in the order the rows are in.
    fn letters(rows: &[DriveRow]) -> String {
        rows.iter().filter_map(|r| r.path.to_str()?.chars().next()).collect()
    }

    fn sorted(rows: &[DriveRow], column: DriveColumn, descending: bool) -> String {
        let mut rows = rows.to_vec();
        sort(&mut rows, Some(DriveSort { column, descending }));
        letters(&rows)
    }

    #[test]
    fn drives_sort_by_each_column_and_descending_reverses_it_but_not_the_ties() {
        let rows = three_drives();
        assert_eq!(letters(&rows), "CDE");
        use DriveColumn::*;
        // Ascending, descending. Ties (C: and E: are both Local Disks) stay in letter order.
        for (column, asc, desc) in [
            (Name, "EDC", "CDE"),
            (Type, "CED", "DCE"),
            (FileSystem, "EDC", "CDE"),
            (FreeSpace, "CED", "DEC"),
            (Size, "ECD", "DCE"),
            (PercentFull, "EDC", "CDE"),
        ] {
            assert_eq!(sorted(&rows, column, false), asc, "{column:?} ascending");
            assert_eq!(sorted(&rows, column, true), desc, "{column:?} descending");
        }
    }

    #[test]
    fn unsorted_drives_stay_in_letter_order_and_names_sort_ignoring_case() {
        let mut rows = three_drives();
        rows.reverse();
        sort(&mut rows, None);
        assert_eq!(letters(&rows), "EDC", "left as they are");
        let mut named = rows_named(&["beta (F:)", "Alpha (G:)", "alpha (H:)"]);
        sort(&mut named, Some(DriveSort { column: DriveColumn::Name, descending: false }));
        assert_eq!(letters(&named), "GHF", "Alpha, alpha (tie: letter order), beta");
    }

    fn rows_named(names: &[&str]) -> Vec<DriveRow> {
        let disks: Vec<_> = names.iter().map(|n| (mount(n, &format!("{}:\\", &n[n.len() - 3..n.len() - 2])), Some(usage(1.0, 2.0)))).collect();
        rows(&disks, info)
    }

    #[test]
    fn a_drive_whose_space_is_unknown_sorts_before_the_known_ones_and_last_when_descending() {
        let mut rows = three_drives();
        let disks = [(mount("Sin datos (F:)", "F:\\"), None)];
        rows.extend(self::rows(&disks, info));
        for column in [DriveColumn::FreeSpace, DriveColumn::Size, DriveColumn::PercentFull] {
            assert!(sorted(&rows, column, false).starts_with('F'), "{column:?}");
            assert!(sorted(&rows, column, true).ends_with('F'), "{column:?}");
        }
    }

    #[test]
    fn a_header_click_starts_ascending_and_the_same_column_again_reverses_it() {
        use DriveColumn::*;
        let asc = |column| DriveSort { column, descending: false };
        assert_eq!(DriveSort::clicked(None, Size), asc(Size), "first click");
        assert_eq!(DriveSort::clicked(Some(asc(Size)), Size), DriveSort { column: Size, descending: true }, "again: reversed");
        assert_eq!(DriveSort::clicked(Some(DriveSort { column: Size, descending: true }), Size), asc(Size), "and back");
        assert_eq!(DriveSort::clicked(Some(DriveSort { column: Size, descending: true }), Name), asc(Name), "another column starts ascending");
    }

    #[test]
    fn a_column_fits_its_widest_cell_or_its_header_whole_points_and_never_under_the_minimum() {
        let ctx = egui::Context::default();
        let mut rows = two_drives();
        draw(&ctx, &rows, None, vec![], None);
        let measure = file_list::body_measure(&ctx);
        let fit = |rows: &[DriveRow], column| fit_width(&ctx, rows, column);
        for (column, (label, _)) in COLUMNS.into_iter().enumerate() {
            let width = fit(&rows, column);
            assert!(width >= file_list::header_width(&ctx, label), "{column}: the header fits");
            assert!(width >= file_list::MIN_WIDTH && width.fract() == 0.0, "{column}: {width}");
        }
        // The cells: every row's text is inside the column it was fitted to.
        for row in &rows {
            assert!(fit(&rows, 1) >= measure(row.kind.to_string()) && fit(&rows, 2) >= measure(row.fs.clone()));
            assert!(fit(&rows, 3) >= measure(row.free.clone()) && fit(&rows, 4) >= measure(row.size.clone()));
            assert!(fit(&rows, 0) >= measure(row.name.clone()), "and the name with its icon");
        }
        let before = fit(&rows, 0);
        rows[1].name = "A drive with a really quite long name indeed (D:)".into();
        assert!(fit(&rows, 0) > before + 100.0, "a longer name needs a wider column");
        assert_eq!(fit(&rows, 1), fit(&two_drives(), 1), "the other columns do not care");
    }

    #[test]
    fn a_click_on_a_header_asks_to_sort_by_its_column_even_beside_its_label() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let free = text_rect(&shapes, "Free Space");
        assert_eq!(draw(&ctx, &rows, None, click(free.center(), egui::PointerButton::Primary), None).0, Some(DrivesEvent::Sort(DriveColumn::FreeSpace)));
        settled(&ctx, &rows, None);
        let beside = egui::pos2(free.right() + 15.0, free.center().y);
        assert_eq!(draw(&ctx, &rows, None, click(beside, egui::PointerButton::Primary), None).0, Some(DrivesEvent::Sort(DriveColumn::FreeSpace)), "the whole cell");
        settled(&ctx, &rows, None);
        let percent = text_rect(&shapes, "Percent Full");
        assert_eq!(draw(&ctx, &rows, None, click(percent.center(), egui::PointerButton::Primary), None).0, Some(DrivesEvent::Sort(DriveColumn::PercentFull)));
    }

    #[test]
    fn a_right_click_on_a_header_asks_for_its_menu_and_does_not_sort() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let at = text_rect(&shapes, "File System").center();
        assert_eq!(draw(&ctx, &rows, None, click(at, egui::PointerButton::Secondary), None).0, Some(DrivesEvent::HeaderMenu(2)));
    }

    #[test]
    fn the_sorted_header_shows_the_lists_arrow_and_the_others_none() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let arrows = |sort: Option<DriveSort>| {
            draw_as(&ctx, &rows, None, (sort, DEFAULT_WIDTHS), vec![], None);
            let shapes = draw_as(&ctx, &rows, None, (sort, DEFAULT_WIDTHS), vec![], None).1;
            shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text().contains(['⏶', '⏷']) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(arrows(None).is_empty(), "unsorted: no arrow");
        let size = |descending| Some(DriveSort { column: DriveColumn::Size, descending });
        assert_eq!(arrows(size(false)), [format!("Size {}", file_list::SORT_ASCENDING)]);
        assert_eq!(arrows(size(true)), [format!("Size {}", file_list::SORT_DESCENDING)]);
    }

    #[test]
    fn the_columns_are_as_wide_as_they_are_told() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let left_of = |widths: Widths, header: &str| {
            let look = (None, widths);
            draw_as(&ctx, &rows, None, look, vec![], None);
            draw_as(&ctx, &rows, None, look, vec![], None);
            text_rect(&draw_as(&ctx, &rows, None, look, vec![], None).1, header).min.x
        };
        let (name, normal) = (left_of(DEFAULT_WIDTHS, "Name"), left_of(DEFAULT_WIDTHS, "Type"));
        let wide = left_of([400.0, 130.0, 100.0, 110.0, 110.0, 140.0], "Type");
        assert!((wide - normal - 140.0).abs() < 1.5, "Name is 140 wider: {normal} {wide} (Name at {name})");
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn dragging_a_header_border_resizes_its_column_and_reports_the_widths_once_let_go() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let ty = text_rect(&shapes, "Type");
        // The border between Name and Type: just left of the Type header's text.
        let border = egui::pos2(ty.min.x - 8.0, ty.center().y);
        draw(&ctx, &rows, None, vec![egui::Event::PointerMoved(border)], None);
        draw(&ctx, &rows, None, vec![button(border, true)], None);
        for step in 1..=5 {
            let moved = egui::Event::PointerMoved(border + egui::vec2(10.0 * step as f32, 0.0));
            assert_eq!(draw(&ctx, &rows, None, vec![moved], None).0, None, "nothing reported while dragging");
        }
        let released = draw(&ctx, &rows, None, vec![button(border + egui::vec2(50.0, 0.0), false)], None).0;
        let Some(DrivesEvent::Resized(new)) = released else { panic!("the widths are reported when let go: {released:?}") };
        assert!((new[0] - 310.0).abs() < 1.5, "Name is 50 wider: {new:?}");
        assert_eq!(new[1..], DEFAULT_WIDTHS[1..], "only the dragged column changes");
    }

    #[test]
    fn the_drive_under_the_pointer_is_lit_in_a_translucent_cyan() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let at = text_rect(&shapes, "Local Disk").center();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = draw(&ctx, &rows, None, vec![egui::Event::PointerMoved(at)], None).1;
        }
        assert!(file_list::tests::hover_lit(&ctx, &shapes, at));
    }

    #[test]
    fn faint_separators_run_between_the_six_columns() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let lines = file_list::tests::separators(&ctx, &shapes);
        assert_eq!(lines.len(), 5, "{lines:?}");
        let ty = text_rect(&shapes, "Type");
        assert!(lines[0].0 < ty.min.x && lines[0].0 > text_rect(&shapes, "Name").max.x, "the first one between Name and Type");
    }

    #[test]
    fn a_double_click_on_a_header_border_fits_the_column_on_its_left() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let ty = text_rect(&shapes, "Type");
        let border = egui::pos2(ty.min.x - 8.0, ty.center().y);
        assert_eq!(draw(&ctx, &rows, None, click(border, egui::PointerButton::Primary), Some(10.0)).0, None, "one click neither sorts nor fits");
        let second = draw(&ctx, &rows, None, click(border, egui::PointerButton::Primary), Some(10.1)).0;
        assert_eq!(second, Some(DrivesEvent::Fit(0)), "Name, the column on the border's left");
    }

    /// File Pilot's This PC at ~1080 px (L18): Name ~270, the others ~150 each.
    #[test]
    fn fitting_this_pc_spreads_the_room_over_every_column_like_file_pilot() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        draw(&ctx, &rows, None, vec![], None);
        let content: Widths = std::array::from_fn(|c| fit_width(&ctx, &rows, c));
        let room = file_list::room(&ctx, 1080.0, &file_list::ALL_SHOWN);
        let fitted = file_list::fit_all(&content, &file_list::ALL_SHOWN, &ROLES, room);
        assert!((fitted.iter().sum::<f32>() - room).abs() < 0.5, "the columns add up to the room: {fitted:?} in {room}");
        assert!(fitted[0] < room / 2.0, "Name does not take the room: {fitted:?}");
        assert!(fitted[0] > 200.0 && fitted[0] < 340.0, "Name about 270: {fitted:?}");
        for c in 1..COLUMNS.len() {
            assert!(fitted[c] >= content[c], "{c} is not cut: {fitted:?}");
            assert!(fitted[c] > 90.0 && fitted[c] < 230.0, "column {c} spread, about 150: {fitted:?}");
        }
    }

    #[test]
    fn this_pc_in_a_narrow_window_squeezes_the_numbers_first_and_keeps_the_rest_whole() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        draw(&ctx, &rows, None, vec![], None);
        let content: Widths = std::array::from_fn(|c| fit_width(&ctx, &rows, c));
        let mut roomy = content;
        roomy[0] = 700.0;
        let room = roomy.iter().sum::<f32>() - 60.0;
        let fitted = file_list::fit_all(&roomy, &file_list::ALL_SHOWN, &ROLES, room);
        assert!((fitted.iter().sum::<f32>() - room).abs() < 0.5, "{fitted:?}");
        assert_eq!((fitted[1], fitted[2], fitted[5]), (content[1], content[2], content[5]), "Type, File System and Percent Full whole: {fitted:?}");
        assert!(fitted[3] < content[3] && fitted[4] < content[4], "Free Space and Size give: {fitted:?}");
        assert!(fitted[0] >= 700.0, "and Name is not cut while they can: {fitted:?}");
    }

    /// The x of the left edge of `text`'s column is the header's; its right edge is the next header's less the gap.
    #[test]
    fn only_size_is_right_aligned_and_free_space_starts_at_its_header_like_the_text_columns() {
        let ctx = egui::Context::default();
        let rows = two_drives();
        let shapes = settled(&ctx, &rows, None);
        let gap = ctx.global_style().spacing.item_spacing.x;
        let (free, percent) = (text_rect(&shapes, "Free Space"), text_rect(&shapes, "Percent Full"));
        assert!((text_rect(&shapes, "210.6 GB").min.x - free.min.x).abs() < 6.0, "Free Space starts at its header, not by Size");
        assert!((percent.min.x - text_rect(&shapes, "461.2 GB").max.x - gap).abs() < 6.0, "Size ends at the Percent Full column");
        let (ty, fs) = (text_rect(&shapes, "Type"), text_rect(&shapes, "File System"));
        assert!((text_rect(&shapes, "Local Disk").min.x - ty.min.x).abs() < 6.0, "Type starts at its header");
        assert!((text_rect(&shapes, "NTFS").min.x - fs.min.x).abs() < 6.0, "File System starts at its header");
    }
}
