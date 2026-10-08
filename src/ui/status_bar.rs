use std::time::Duration;

use eframe::egui;

use crate::fs::count::Totals;
use crate::fs::transfer::{Mode, Progress};

pub struct Status<'a> {
    /// Text typed in the filter bar.
    pub filter: &'a str,
    pub item_count: usize,
    pub hidden_count: usize,
    pub show_hidden: bool,
    pub loading: bool,
    pub last_elapsed: Option<Duration>,
    pub error: Option<&'a str>,
    /// Progress of the running copy or move, already formatted.
    pub job: Option<String>,
    pub notice: Option<&'a str>,
    /// Alt+R count in progress, already formatted.
    pub counting: Option<String>,
    /// Folders among the rows shown.
    pub folder_count: usize,
    /// Selected row, as an index into the rows shown.
    pub selected: Option<usize>,
}

pub fn show(ui: &mut egui::Ui, status: &Status<'_>) {
    ui.horizontal(|ui| {
        filter_bar(ui, status.filter, status.item_count);
        ui.separator();
        ui.label(count_text(status.item_count, !status.filter.is_empty()));
        if status.hidden_count > 0 && !status.show_hidden {
            ui.weak(format!("({} hidden, Ctrl+H to show)", status.hidden_count));
        }
        if let Some(elapsed) = status.last_elapsed {
            ui.separator();
            ui.label(format!("listed in {:.2} ms", elapsed.as_secs_f64() * 1000.0));
        }
        if status.loading {
            ui.separator();
            ui.spinner();
            ui.label("Loading…");
        }
        if let Some(job) = &status.job {
            ui.separator();
            ui.spinner();
            ui.label(job);
            ui.weak("Esc to cancel");
        }
        if let Some(counting) = &status.counting {
            ui.separator();
            ui.spinner();
            ui.label(counting);
            ui.weak("Esc to cancel");
        }
        if let Some(notice) = status.notice {
            ui.separator();
            ui.label(notice);
        }
        if let Some(error) = status.error {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(position_text(status.selected, status.item_count))
                .on_hover_text("Selected row / rows shown");
            crate::ui::icons::file(ui);
            ui.add_space(8.0);
            ui.label(status.folder_count.to_string()).on_hover_text("Folders shown");
            crate::ui::icons::folder(ui);
        });
    });
}

/// Always-visible filter at the bottom left. It is drawn, not a text field:
/// typing anywhere in the list feeds it, so it never has to take focus.
fn filter_bar(ui: &mut egui::Ui, filter: &str, items: usize) {
    let text = if filter.is_empty() {
        egui::RichText::new(filter_placeholder(items)).weak()
    } else {
        egui::RichText::new(filter).strong()
    };
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.inactive.bg_stroke)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.set_min_width(180.0);
            crate::ui::icons::magnifier(ui);
            ui.add(egui::Label::new(text).selectable(false));
        });
}

/// What the empty filter bar says: "Filter 10 files…".
pub fn filter_placeholder(items: usize) -> String {
    let noun = if items == 1 { "file" } else { "files" };
    format!("Filter {items} {noun}…")
}

/// "9/10": the selected row, counted from 1, of the rows shown.
pub fn position_text(selected: Option<usize>, rows: usize) -> String {
    format!("{}/{rows}", selected.map_or(0, |i| i + 1))
}

/// "12 items", or how many rows the filter left.
pub fn count_text(items: usize, filtering: bool) -> String {
    match (filtering, items) {
        (false, n) => format!("{n} items"),
        (true, 0) => "No matches".into(),
        (true, 1) => "1 match".into(),
        (true, n) => format!("{n} matches"),
    }
}

/// "Calculating “sub”… 3 folders · 120 files" while Alt+R runs.
pub fn counting_text(name: &str, totals: &Totals) -> String {
    use crate::ui::preview::plural;
    format!("Calculating “{name}”… {} · {}", plural(totals.folders, "folder"), plural(totals.files, "file"))
}

/// "Copying 3/10 — 45%": files done of total, then share of bytes.
pub fn progress_text(mode: Mode, progress: &Progress) -> String {
    let verb = match mode {
        Mode::Copy => "Copying",
        Mode::Move => "Moving",
    };
    format!("{verb} {}/{} — {}%", progress.files_done, progress.files_total, progress.percent())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_reads_matches_while_filtering() {
        assert_eq!(count_text(12, false), "12 items");
        assert_eq!(count_text(0, false), "0 items");
        assert_eq!(count_text(3, true), "3 matches");
        assert_eq!(count_text(1, true), "1 match");
        assert_eq!(count_text(0, true), "No matches");
    }

    #[test]
    fn counting_reads_the_folder_and_running_totals() {
        let t = Totals { folders: 3, files: 1, bytes: 0, unreadable: 0 };
        assert_eq!(counting_text("sub", &t), "Calculating “sub”… 3 folders · 1 file");
    }

    #[test]
    fn the_empty_filter_says_how_many_files_it_filters() {
        assert_eq!(filter_placeholder(10), "Filter 10 files…");
        assert_eq!(filter_placeholder(1), "Filter 1 file…");
        assert_eq!(filter_placeholder(0), "Filter 0 files…");
    }

    #[test]
    fn position_counts_the_selected_row_from_one() {
        assert_eq!(position_text(Some(8), 10), "9/10");
        assert_eq!(position_text(Some(0), 1), "1/1");
        assert_eq!(position_text(None, 0), "0/0");
    }

    #[test]
    fn progress_reads_files_then_percent() {
        let p = Progress { files_done: 3, files_total: 10, bytes_done: 45, bytes_total: 100 };
        assert_eq!(progress_text(Mode::Copy, &p), "Copying 3/10 — 45%");
        assert_eq!(progress_text(Mode::Move, &Progress::default()), "Moving 0/0 — 100%");
    }
}
