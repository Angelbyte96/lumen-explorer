//! The file table. Only visible rows are laid out, so huge folders stay fast.

use std::path::PathBuf;

use eframe::egui::{self, text::CCursorRange};
use egui_extras::{Column, TableBuilder};

use crate::app::{Scroll, Sort, SortColumn};
use crate::format::{format_size, format_time};
use crate::fs::Entry;

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

/// `rows` holds indices into `entries` in display order; `selected` indexes `rows`.
pub fn show(
    ui: &mut egui::Ui,
    entries: &[Entry],
    rows: &[usize],
    selected: Option<usize>,
    scroll: Option<Scroll>,
    sort: Sort,
    mut edit: Option<&mut RenameEdit>,
) -> Option<ListEvent> {
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let mut event = None;

    let mut table = TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .column(Column::remainder().at_least(200.0).clip(true))
        .column(Column::exact(90.0))
        .column(Column::exact(140.0));
    if let Some(scroll) = scroll
        && let Some(sel) = selected
    {
        table = table
            .scroll_to_row(sel, None)
            .animate_scrolling(scroll == Scroll::Smooth);
    }

    table
        .header(row_height, |mut header| {
            for (label, column) in [
                ("Name", SortColumn::Name),
                ("Size", SortColumn::Size),
                ("Modified", SortColumn::Modified),
            ] {
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
                    let icon = if entry.is_dir { "🗀" } else { "🗋" };
                    if let Some(edit) = edit.as_deref_mut().filter(|e| e.path == entry.path) {
                        ui.label(icon);
                        if let Some(submit) = rename_field(ui, edit) {
                            event = Some(ListEvent::RenameEnded(submit));
                        }
                        return;
                    }
                    let text = egui::RichText::new(format!("{icon} {}", entry.name));
                    let text = if entry.hidden { text.weak() } else { text };
                    ui.add(egui::Label::new(text).selectable(false).truncate());
                });
                row.col(|ui| {
                    let size = entry.size.map(format_size).unwrap_or_default();
                    ui.add(egui::Label::new(size).selectable(false));
                });
                row.col(|ui| {
                    let date = entry.modified.map(format_time).unwrap_or_default();
                    ui.add(egui::Label::new(date).selectable(false));
                });
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
        (true, false) => " ⬆",
        (true, true) => " ⬇",
    };
    let text = egui::RichText::new(format!("{label}{arrow}")).strong();
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
}
