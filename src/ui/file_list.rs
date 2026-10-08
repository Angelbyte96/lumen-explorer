//! The file table. Only visible rows are laid out, so huge folders stay fast.

use std::path::PathBuf;

use eframe::egui::{self, text::CCursorRange};
use egui_extras::{Column, TableBuilder};

use crate::app::{Scroll, Sort, SortColumn, View};
use crate::format::{format_size, format_time};
use crate::fs::Entry;
use crate::fs::count::Totals;
use crate::ui::icons;

pub enum ListEvent {
    Select(usize),
    Activate(usize),
    Sort(SortColumn),
    /// The in-place rename ended: `true` with Enter, `false` with Escape or a click elsewhere.
    RenameEnded(bool),
}

/// An entry whose name is being edited in its row.
pub struct RenameEdit {
    pub path: PathBuf,
    pub is_dir: bool,
    pub text: String,
    /// Focus the field (and select the name) on the next frame.
    pub focus: bool,
}

impl RenameEdit {
    pub fn new(path: PathBuf, name: &str, is_dir: bool) -> Self {
        Self { path, is_dir, text: name.to_owned(), focus: true }
    }
}

/// Draws `view`'s rows; `scroll` brings the selection into view.
pub fn show(
    ui: &mut egui::Ui,
    view: &View,
    scroll: Option<Scroll>,
    mut edit: Option<&mut RenameEdit>,
) -> Option<ListEvent> {
    let View { entries, rows, totals, selected, sort, .. } = view;
    let (selected, sort) = (*selected, *sort);
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let mut event = None;

    let mut table = TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .column(Column::remainder().at_least(200.0).clip(true))
        .column(Column::exact(110.0).clip(true))
        .column(Column::exact(130.0))
        .column(Column::exact(60.0))
        .column(Column::exact(60.0))
        .column(Column::exact(80.0));
    if let Some(scroll) = scroll
        && let Some(sel) = selected
    {
        table = table
            .scroll_to_row(sel, None)
            .animate_scrolling(scroll == Scroll::Smooth);
    }

    table
        .header(row_height, |mut header| {
            for (label, column) in COLUMNS {
                header.col(|ui| {
                    if sort_header(ui, label, column, sort) {
                        event = Some(ListEvent::Sort(column));
                    }
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let i = row.index();
                let entry = &entries[rows[i]];
                row.set_selected(selected == Some(i));
                row.col(|ui| {
                    // Cells stack top-down: keep the icon and the name on one line,
                    // or the name lands below the row and gets clipped out of view.
                    ui.horizontal(|ui| {
                        icons::show(ui, entry);
                        if let Some(edit) = edit.as_deref_mut().filter(|e| e.path == entry.path) {
                            if let Some(submit) = rename_field(ui, edit) {
                                event = Some(ListEvent::RenameEnded(submit));
                            }
                            return;
                        }
                        name_label(ui, entry);
                    });
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(entry.kind()).selectable(false).truncate());
                });
                row.col(|ui| {
                    let date = entry.modified.map(format_time).unwrap_or_default();
                    ui.add(egui::Label::new(date).selectable(false));
                });
                count_cols(&mut row, &count_cells(entry, totals.get(&entry.name)));
                let response = row.response();
                if response.double_clicked() {
                    event = Some(ListEvent::Activate(i));
                } else if response.clicked() {
                    event = Some(ListEvent::Select(i));
                }
            });
        });
    event
}

/// The entry's name, dimmed when hidden, cut with "…" when too long.
pub fn name_label(ui: &mut egui::Ui, entry: &Entry) {
    let text = egui::RichText::new(&entry.name);
    let text = if entry.hidden { text.weak() } else { text };
    ui.add(egui::Label::new(text).selectable(false).truncate());
}

/// Header labels and the column each one sorts, in display order.
pub const COLUMNS: [(&str, SortColumn); 6] = [
    ("Name", SortColumn::Name),
    ("Type", SortColumn::Type),
    ("Date Modified", SortColumn::Modified),
    ("Folders", SortColumn::Folders),
    ("Files", SortColumn::Files),
    ("Size", SortColumn::Size),
];

/// Marks the sorted column's direction.
pub const SORT_ASCENDING: &str = "⏶";
pub const SORT_DESCENDING: &str = "⏷";

/// Shown where a value is not known (yet): folders until Alt+R counts them.
pub const UNKNOWN: &str = "--";

/// Folders, Files and Size cells of a row. Files have no counts; folders
/// show `--` until `totals` arrive.
pub fn count_cells(entry: &Entry, totals: Option<&Totals>) -> [String; 3] {
    let unknown = || UNKNOWN.to_owned();
    match (entry.is_dir, totals) {
        (true, Some(t)) => [t.folders.to_string(), t.files.to_string(), format_size(t.bytes)],
        (true, None) => [unknown(), unknown(), unknown()],
        (false, _) => [unknown(), unknown(), entry.size.map(format_size).unwrap_or_else(unknown)],
    }
}

/// Draws the Folders, Files and Size cells. Size is right-aligned so the
/// units line up, like File Pilot; the counts stay under their headers.
pub fn count_cols(row: &mut egui_extras::TableRow<'_, '_>, cells: &[String; 3]) {
    let [folders, files, size] = cells;
    for cell in [folders, files] {
        row.col(|ui| {
            ui.add(egui::Label::new(cell.as_str()).selectable(false));
        });
    }
    row.col(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(size.as_str()).selectable(false));
        });
    });
}

/// Id of the rename field, so the app can tell when it lost focus silently.
pub fn rename_id() -> egui::Id {
    egui::Id::new("lumen_rename")
}

/// The text field drawn in place of the name while renaming.
fn rename_field(ui: &mut egui::Ui, edit: &mut RenameEdit) -> Option<bool> {
    let id = rename_id();
    let mut output = egui::TextEdit::singleline(&mut edit.text)
        .id(id)
        .desired_width(f32::INFINITY)
        .show(ui);
    let response = &output.response.response;
    if std::mem::take(&mut edit.focus) {
        response.request_focus();
        let stem = stem_chars(&edit.text, edit.is_dir);
        let range = CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(stem),
        );
        output.state.cursor.set_char_range(Some(range));
        output.state.store(ui.ctx(), id);
        return None;
    }
    response
        .lost_focus()
        .then(|| ui.input(|i| i.key_pressed(egui::Key::Enter)))
}

/// A header label that sorts on click; the active column shows its direction.
fn sort_header(ui: &mut egui::Ui, label: &str, column: SortColumn, sort: Sort) -> bool {
    let arrow = match (sort.column == column, sort.descending) {
        (false, _) => "",
        (true, false) => SORT_ASCENDING,
        (true, true) => SORT_DESCENDING,
    };
    let text = egui::RichText::new(format!("{label} {arrow}")).strong();
    ui.add(egui::Button::new(text).frame(false)).clicked()
}

/// Characters of `name` selected when renaming starts: the part before the
/// extension, like Dolphin, so typing replaces the name and keeps ".txt".
pub fn stem_chars(name: &str, is_dir: bool) -> usize {
    let total = name.chars().count();
    if is_dir {
        return total;
    }
    match name.rfind('.') {
        // A leading dot marks a hidden file, not an extension.
        Some(dot) if dot > 0 => name[..dot].chars().count(),
        _ => total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_selects_the_name_without_its_extension() {
        assert_eq!(stem_chars("notes.txt", false), 5);
        assert_eq!(stem_chars("archive.tar.gz", false), 11);
        assert_eq!(stem_chars("ñandú.md", false), 5);
        // No extension to keep: select everything.
        assert_eq!(stem_chars(".bashrc", false), 7);
        assert_eq!(stem_chars("Makefile", false), 8);
        assert_eq!(stem_chars("v1.2", true), 4, "folders have no extension");
    }

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            is_dir: false,
            size: Some(1),
            modified: None,
            hidden: false,
        }
    }

    #[test]
    fn count_cells_show_dashes_until_a_folder_is_counted() {
        assert_eq!(count_cells(&entry("a.txt"), None), ["--", "--", "1 B"]);
        let unknown = Entry { size: None, ..entry("b.txt") };
        assert_eq!(count_cells(&unknown, None), ["--", "--", "--"]);
        let dir = Entry { is_dir: true, size: None, ..entry("src") };
        assert_eq!(count_cells(&dir, None), ["--", "--", "--"]);
        let totals = Totals { folders: 2, files: 9, bytes: 2306867, unreadable: 0 };
        assert_eq!(count_cells(&dir, Some(&totals)), ["2", "9", "2.2 MB"]);
    }

    #[test]
    fn rename_field_sits_inside_its_row() {
        let ctx = egui::Context::default();
        let entries = vec![entry("angel.jpg.jpg"), entry("other.txt")];
        let mut edit = RenameEdit::new(entries[0].path.clone(), &entries[0].name, false);
        let mut view = View::default();
        (view.entries, view.rows, view.selected) = (entries, vec![0, 1], Some(0));
        let mut row_height = 0.0;
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                show(ui, &view, None, Some(&mut edit));
            })
            .drop_without_applying_deltas();
        }
        let field = ctx.read_response(rename_id()).expect("field drawn").rect;
        // Header, then the first row: the field must be on that row, not below it
        // (where the cell clips it away and typing seems to show nothing).
        let spacing = ctx.global_style().spacing.item_spacing.y;
        let first_row_bottom = 2.0 * row_height + spacing;
        assert!(
            field.center().y < first_row_bottom,
            "field centre {} should be above {first_row_bottom}",
            field.center().y
        );
    }
}
