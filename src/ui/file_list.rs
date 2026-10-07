//! The file table. Only visible rows are laid out, so huge folders stay fast.

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::app::{Scroll, Sort, SortColumn};
use crate::format::{format_size, format_time};
use crate::fs::Entry;

pub enum ListEvent {
    Select(usize),
    Activate(usize),
    Sort(SortColumn),
}

/// `rows` holds indices into `entries` in display order; `selected` indexes `rows`.
pub fn show(
    ui: &mut egui::Ui,
    entries: &[Entry],
    rows: &[usize],
    selected: Option<usize>,
    scroll: Option<Scroll>,
    sort: Sort,
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
