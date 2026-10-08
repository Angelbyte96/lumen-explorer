use std::time::Duration;

use eframe::egui;

use crate::fs::count::Totals;
use crate::fs::transfer::{Mode, Progress};

pub struct Status<'a> {
    /// Text typed in the filter bar.
    pub filter: &'a str,
    /// Typing goes to the filter now (no path box or rename open): show its cursor.
    pub caret: bool,
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
        filter_bar(ui, status.filter, status.item_count, status.caret);
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(position_text(status.selected, status.item_count))
                .on_hover_text("Selected row / rows shown");
            crate::ui::icons::file(ui);
            ui.add_space(8.0);
            ui.label(status.folder_count.to_string()).on_hover_text("Folders shown");
            crate::ui::icons::folder(ui);
            // Laid out after the counters so it gets only the space left between
            // them and the rest of the bar; cut short, the full text is on hover.
            if let Some(error) = status.error {
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.separator();
                    let text = egui::RichText::new(error).color(ui.visuals().error_fg_color);
                    ui.add(egui::Label::new(text).truncate()).on_hover_text(error);
                });
            }
        });
    });
}

/// Always-visible filter at the bottom left. It is drawn, not a text field:
/// typing anywhere in the list feeds it, so it never has to take focus. With
/// `caret`, a steady text cursor follows the typed text; an empty filter has
/// none, so it does not look like a field waiting for input.
fn filter_bar(ui: &mut egui::Ui, filter: &str, items: usize, caret: bool) {
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
            let label = ui.add(egui::Label::new(text).selectable(false)).rect;
            if caret && !filter.is_empty() {
                let x = label.right() + 1.0;
                let stroke = ui.visuals().text_cursor.stroke;
                ui.painter().line_segment([egui::pos2(x, label.top()), egui::pos2(x, label.bottom())], stroke);
            }
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

    /// The filter bar's text and its cursor (a line in the text cursor colour), if any.
    fn painted_bar(filter: &str, caret: bool) -> (egui::Rect, Option<egui::Rect>) {
        let ctx = egui::Context::default();
        let status = Status {
            filter,
            caret,
            item_count: 9,
            hidden_count: 0,
            show_hidden: false,
            loading: false,
            last_elapsed: None,
            error: None,
            job: None,
            notice: None,
            counting: None,
            folder_count: 0,
            selected: None,
        };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show(ui, &status));
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let shown = if filter.is_empty() { filter_placeholder(9) } else { filter.to_owned() };
        let text = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == shown => Some(t.galley.rect.translate(t.pos.to_vec2())),
                _ => None,
            })
            .expect("filter text painted");
        let colour = ctx.global_style().visuals.text_cursor.stroke.color;
        let cursor = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::LineSegment { points, stroke } if stroke.color == colour => Some(egui::Rect::from_two_pos(points[0], points[1])),
            _ => None,
        });
        (text, cursor)
    }

    #[test]
    fn the_filter_bar_shows_a_cursor_while_typing_goes_to_it() {
        let (text, cursor) = painted_bar("f6", true);
        let cursor = cursor.expect("cursor painted");
        assert!((cursor.left() - text.right()).abs() <= 2.0, "after the text: {cursor:?} vs {text:?}");
        assert!(cursor.top() <= text.top() + 1.0 && cursor.bottom() >= text.bottom() - 1.0, "as tall as the text");
        assert_eq!(painted_bar("", true).1, None, "no cursor while nothing is typed");
        assert_eq!(painted_bar("f6", false).1, None, "no cursor while the path box or a rename takes the keys");
    }

    const LONG_ERROR: &str = "Cannot rename C:\\Users\\someone\\Documents\\a very long folder name\\report.txt: \
        The filename, directory name, or volume label syntax is incorrect. (os error 123)";

    /// Text shapes painted by the bar in a narrow window, after `frames` frames
    /// with the pointer at `pointer`.
    fn painted_narrow(pointer: Option<egui::Pos2>) -> Vec<(String, egui::Rect, bool)> {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|s| s.interaction.tooltip_delay = 0.0);
        let status = Status {
            filter: "",
            caret: false,
            item_count: 9,
            hidden_count: 0,
            show_hidden: false,
            loading: false,
            last_elapsed: None,
            error: Some(LONG_ERROR),
            job: None,
            notice: None,
            counting: None,
            folder_count: 2,
            selected: None,
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 200.0));
        let mut shapes = Vec::new();
        for frame in 0..4 {
            let mut input = egui::RawInput { screen_rect: Some(screen), time: Some(frame as f64), ..Default::default() };
            if let Some(pos) = pointer {
                input.events.push(egui::Event::PointerMoved(pos));
            }
            let mut output = ctx.run_ui(input, |ui| show(ui, &status));
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => {
                    Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()), t.galley.elided))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_long_error_is_shortened_to_fit_and_shows_in_full_on_hover() {
        let texts = painted_narrow(None);
        let (_, error, elided) = texts.iter().find(|(t, ..)| t.starts_with("Cannot rename")).expect("error painted");
        let (_, position, _) = texts.iter().find(|(t, ..)| t == "0/9").expect("position painted");
        assert!(*elided, "shortened with an ellipsis");
        assert!(error.right() <= position.left(), "leaves the counters visible: {error:?} vs {position:?}");
        assert!(!texts.iter().any(|(t, _, elided)| t == LONG_ERROR && !elided), "no tooltip without hover");

        let hovered = painted_narrow(Some(error.center()));
        assert!(hovered.iter().any(|(t, _, elided)| t == LONG_ERROR && !elided), "hover shows the whole error");
    }

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
