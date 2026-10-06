//! The file table. Only visible rows are laid out, so huge folders stay fast.

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::format::{format_size, format_time};
use crate::fs::Entry;

pub enum ListEvent {
    Select(usize),
    Activate(usize),
}

/// `rows` holds indices into `entries` in display order; `selected` indexes `rows`.
pub fn show(
    ui: &mut egui::Ui,
    entries: &[Entry],
    rows: &[usize],
    selected: Option<usize>,
    scroll_to_selected: bool,
) -> Option<ListEvent> {
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let mut event = None;

    let mut table = TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .column(Column::remainder().at_least(200.0).clip(true))
        .column(Column::exact(90.0))
        .column(Column::exact(140.0));
    if scroll_to_selected && let Some(sel) = selected {
        table = table.scroll_to_row(sel, None);
    }

    table
        .header(row_height, |mut header| {
            header.col(|ui| {
                ui.strong("Name");
            });
            header.col(|ui| {
                ui.strong("Size");
            });
            header.col(|ui| {
                ui.strong("Modified");
            });
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
