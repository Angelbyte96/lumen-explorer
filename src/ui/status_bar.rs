use std::time::Duration;

use eframe::egui;

use crate::format::format_size;
use crate::fs::count::Totals;
use crate::fs::transfer::{Mode, Progress};
use crate::i18n::tr;
use crate::ui::view_size::ViewSize;

pub struct Status<'a> {
    /// Text typed in the filter bar.
    pub filter: &'a str,
    /// Typing goes to the filter now (no path box or rename open): show its cursor.
    pub caret: bool,
    /// Ctrl+F filter mode: the cursor shows before anything is typed.
    pub filter_mode: bool,
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
    /// Ctrl+E walk in progress, already formatted.
    pub walking: Option<String>,
    /// Folders among the rows shown.
    pub folder_count: usize,
    /// Selected row, as an index into the rows shown.
    pub selected: Option<usize>,
    /// Rows picked with Ctrl+A, Ctrl+click or Shift.
    pub picked: usize,
    pub picked_size: PickedSize,
    /// The view slider's position, shown at the bar's right end.
    pub view: ViewSize,
}

/// Room the islands take below the list: one island and its air.
pub fn island_room(ctx: &egui::Context) -> f32 {
    let text = ctx.global_style().text_styles[&egui::TextStyle::Body].size;
    text.max(14.0) + 2.0 * (ISLAND_MARGIN as f32 + EDGE) + 6.0
}

/// Runs `add` in the room left above `room` points at the bottom: the views
/// (and their scroll bars) end there, so the islands never cover them. A plain
/// maximum height is not enough, as a scroll area still takes the whole height.
pub fn above_islands<R>(ui: &mut egui::Ui, room: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut rect = ui.available_rect_before_wrap();
    rect.max.y = (rect.max.y - room).max(rect.min.y);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), add).inner
}

/// Air between an island and the list's edges.
const EDGE: f32 = 6.0;
const ISLAND_MARGIN: i8 = 4;

/// An island's frame: rounded, raised over the list like File Pilot's.
fn island(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    egui::Frame::new()
        .fill(style.visuals.window_fill)
        .stroke(style.visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(8, ISLAND_MARGIN))
}

/// Floating over the bottom of `area` (the list's panel), like File Pilot:
/// the filter at the left; the counters, the view button and its percent at
/// the right; progress, notices and errors above that, only while there is
/// one. Returns the view picked with its button this frame.
pub fn show(ctx: &egui::Context, area: egui::Rect, status: &Status<'_>) -> Option<ViewSize> {
    let corner = |id: &str, pivot: egui::Align2, at: egui::Pos2| egui::Area::new(egui::Id::new(id)).order(egui::Order::Middle).fade_in(false).pivot(pivot).fixed_pos(at);
    corner("lumen_island_filter", egui::Align2::LEFT_BOTTOM, area.left_bottom() + egui::vec2(EDGE, -EDGE)).show(ctx, |ui| {
        island(ctx).show(ui, |ui| {
            ui.horizontal(|ui| {
                filter_bar(ui, status.filter, status.item_count, status.caret && (status.filter_mode || !status.filter.is_empty()));
                if !status.filter.is_empty() {
                    ui.weak(count_text(status.item_count, true));
                }
                if status.hidden_count > 0 && !status.show_hidden {
                    ui.weak(tr!("({0} hidden, Ctrl+H to show)", status.hidden_count));
                }
            });
        });
    });
    let mut picked = None;
    let counters = corner("lumen_island_view", egui::Align2::RIGHT_BOTTOM, area.right_bottom() + egui::vec2(-EDGE, -EDGE)).show(ctx, |ui| {
        island(ctx).show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(picked) = picked_text(status.picked, &status.picked_size) {
                    ui.strong(picked);
                    ui.separator();
                }
                crate::ui::icons::folder(ui);
                ui.label(status.folder_count.to_string()).on_hover_text(tr!("Folders shown"));
                ui.add_space(6.0);
                crate::ui::icons::file(ui);
                let listed = status.last_elapsed.map(|e| format!("\n{}", tr!("Listed in {0} ms", format!("{:.2}", e.as_secs_f64() * 1000.0)))).unwrap_or_default();
                ui.label(position_text(status.selected, status.item_count))
                    .on_hover_text(format!("{}{listed}", tr!("Selected row / rows shown ({0})", count_text(status.item_count, false))));
                ui.separator();
                picked = crate::ui::view_picker::show(ui, status.view);
            });
        });
    });
    note(ctx, area, counters.response.rect.top(), status);
    picked
}

/// Progress, notices and errors: floating above the right island while any is
/// there. A long error is cut to fit and shows whole on hover.
fn note(ctx: &egui::Context, area: egui::Rect, above: f32, status: &Status<'_>) {
    let busy = status.loading || status.job.is_some() || status.counting.is_some() || status.walking.is_some();
    if !busy && status.notice.is_none() && status.error.is_none() {
        return;
    }
    let at = egui::pos2(area.right() - EDGE, above - EDGE);
    egui::Area::new(egui::Id::new("lumen_island_note")).order(egui::Order::Middle).fade_in(false).pivot(egui::Align2::RIGHT_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        ui.set_max_width(area.width() - 2.0 * EDGE - 24.0);
        island(ctx).show(ui, |ui| {
            ui.vertical(|ui| {
                if status.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr!("Loading…"));
                    });
                }
                for running in [&status.job, &status.counting, &status.walking].into_iter().flatten() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(running);
                        ui.weak(tr!("Esc to cancel"));
                    });
                }
                if let Some(notice) = status.notice {
                    ui.label(notice);
                }
                if let Some(error) = status.error {
                    let text = egui::RichText::new(error).color(ui.visuals().error_fg_color);
                    ui.add(egui::Label::new(text).truncate());
                }
            });
        });
    });
}

/// Always-visible filter at the bottom left. It is drawn, not a text field:
/// typing anywhere in the list feeds it, so it never has to take focus. With
/// `caret`, a steady text cursor follows the typed text, or stands before the
/// placeholder of an empty filter (Ctrl+F); otherwise an empty filter has
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
            if caret {
                let x = if filter.is_empty() { label.left() - 1.0 } else { label.right() + 1.0 };
                let stroke = ui.visuals().text_cursor.stroke;
                ui.painter().line_segment([egui::pos2(x, label.top()), egui::pos2(x, label.bottom())], stroke);
            }
        });
}

/// What the empty filter bar says: "Filter 10 files…".
pub fn filter_placeholder(items: usize) -> String {
    if items == 1 { tr!("Filter 1 file…").to_owned() } else { tr!("Filter {0} files…", items) }
}

/// The picked rows' total size: files and folders counted with Alt+R add
/// their bytes; `uncounted` folders have no size yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PickedSize {
    pub bytes: u64,
    pub uncounted: u64,
}

/// "5 selected · 12.4 MB + 2 folders" while rows are picked: their total
/// size, and the folders not counted yet.
pub fn picked_text(picked: usize, size: &PickedSize) -> Option<String> {
    use crate::ui::preview::plural;
    if picked == 0 {
        return None;
    }
    let folders = plural(size.uncounted, "folder");
    let total = match (size.bytes, size.uncounted) {
        (0, 1..) => folders,
        (bytes, 0) => format_size(bytes),
        (bytes, _) => format!("{} + {folders}", format_size(bytes)),
    };
    Some(if picked == 1 { tr!("1 selected · {0}", total) } else { tr!("{0} selected · {1}", picked, total) })
}

/// "9/10": the selected row, counted from 1, of the rows shown.
pub fn position_text(selected: Option<usize>, rows: usize) -> String {
    format!("{}/{rows}", selected.map_or(0, |i| i + 1))
}

/// "12 items", or how many rows the filter left.
pub fn count_text(items: usize, filtering: bool) -> String {
    match (filtering, items) {
        (false, 1) => tr!("1 item").into(),
        (false, n) => tr!("{0} items", n),
        (true, 0) => tr!("No matches").into(),
        (true, 1) => tr!("1 match").into(),
        (true, n) => tr!("{0} matches", n),
    }
}

/// "Listing subfolders… 120 items" while a Ctrl+E walk runs.
pub fn walking_text(found: usize) -> String {
    use crate::ui::preview::plural;
    tr!("Listing subfolders… {0}", plural(found as u64, "item"))
}

/// "Calculating “sub”… 3 folders · 120 files" while Alt+R runs.
pub fn counting_text(name: &str, totals: &Totals) -> String {
    use crate::ui::preview::plural;
    tr!("Calculating “{0}”… {1} · {2}", name, plural(totals.folders, "folder"), plural(totals.files, "file"))
}

/// "Copying 3/10 — 45%": files done of total, then share of bytes.
/// `item` is which entry of a several-entry paste is running, and of how many.
pub fn progress_text(mode: Mode, progress: &Progress, item: Option<(usize, usize)>) -> String {
    let verb = match mode {
        Mode::Copy => tr!("Copying"),
        Mode::Move => tr!("Moving"),
    };
    let item = item.map(|(n, total)| tr!("item {0} of {1}: ", n, total)).unwrap_or_default();
    format!("{verb} {item}{}/{} — {}%", progress.files_done, progress.files_total, progress.percent())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The filter bar's text and its cursor (a line in the text cursor colour), if any.
    fn painted_bar(filter: &str, caret: bool, filter_mode: bool) -> (egui::Rect, Option<egui::Rect>) {
        let ctx = egui::Context::default();
        let status = Status {
            filter,
            caret,
            filter_mode,
            item_count: 9,
            hidden_count: 0,
            show_hidden: false,
            loading: false,
            last_elapsed: None,
            error: None,
            job: None,
            notice: None,
            counting: None,
            walking: None,
            folder_count: 0,
            selected: None,
            picked: 0,
            picked_size: PickedSize::default(),
            view: ViewSize::default(),
        };
        let mut shapes = Vec::new();
        // An island sizes itself on its first frame, unseen.
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| { show(ui.ctx(), ui.max_rect(), &status); });
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
    fn the_filter_and_the_counters_float_in_the_lists_bottom_corners_inside_the_room_left_for_them() {
        let ctx = egui::Context::default();
        let status = Status {
            filter: "",
            caret: false,
            filter_mode: false,
            item_count: 9,
            hidden_count: 0,
            show_hidden: false,
            loading: false,
            last_elapsed: None,
            error: None,
            job: None,
            notice: None,
            counting: None,
            walking: None,
            folder_count: 2,
            selected: None,
            picked: 0,
            picked_size: PickedSize::default(),
            view: ViewSize::default(),
        };
        let area = egui::Rect::from_min_max(egui::pos2(200.0, 50.0), egui::pos2(900.0, 650.0));
        let mut texts = Vec::new();
        for _ in 0..3 {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                show(ui.ctx(), area, &status);
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                    _ => None,
                })
                .collect();
        }
        let find = |text: &str| texts.iter().find(|(t, _)| t == text).unwrap_or_else(|| panic!("{text:?} in {texts:?}")).1;
        let room = island_room(&ctx);
        let (filter, position, view) = (find(&filter_placeholder(9)), find("0/9"), find("Details"));
        assert!(filter.left() < area.left() + 60.0, "filter at the left, after its magnifier: {filter:?}");
        assert!(view.right() > area.right() - 80.0 && position.right() < view.left(), "counters, then the view, at the right");
        for rect in [filter, position, view] {
            assert!(rect.top() > area.bottom() - room && rect.bottom() < area.bottom(), "{rect:?} inside the bottom {room} points");
        }
        assert!(!texts.iter().any(|(t, _)| t == "Loading…"), "no note while nothing runs");
    }

    #[test]
    fn a_paste_of_several_items_says_which_one_is_running() {
        let p = Progress { files_done: 1, files_total: 4, bytes_done: 50, bytes_total: 100 };
        assert_eq!(progress_text(Mode::Copy, &p, None), "Copying 1/4 — 50%");
        assert_eq!(progress_text(Mode::Move, &p, Some((2, 5))), "Moving item 2 of 5: 1/4 — 50%");
    }

    #[test]
    fn picked_rows_are_counted_only_when_there_are_some() {
        let none = PickedSize::default();
        assert_eq!(picked_text(0, &none), None);
        assert_eq!(picked_text(1, &PickedSize { bytes: 512, uncounted: 0 }).as_deref(), Some("1 selected · 512 B"));
        assert_eq!(picked_text(5, &PickedSize { bytes: 13_002_342, uncounted: 0 }).as_deref(), Some("5 selected · 12.4 MB"));
        assert_eq!(
            picked_text(5, &PickedSize { bytes: 13_002_342, uncounted: 2 }).as_deref(),
            Some("5 selected · 12.4 MB + 2 folders"),
            "folders not counted with Alt+R have no size yet"
        );
        assert_eq!(picked_text(1, &PickedSize { bytes: 0, uncounted: 1 }).as_deref(), Some("1 selected · 1 folder"), "only uncounted folders");
        assert_eq!(picked_text(2, &PickedSize { bytes: 0, uncounted: 0 }).as_deref(), Some("2 selected · 0 B"), "empty files");
    }

    #[test]
    fn the_filter_bar_shows_a_cursor_while_typing_goes_to_it() {
        let (text, cursor) = painted_bar("f6", true, false);
        let cursor = cursor.expect("cursor painted");
        assert!((cursor.left() - text.right()).abs() <= 2.0, "after the text: {cursor:?} vs {text:?}");
        assert!(cursor.top() <= text.top() + 1.0 && cursor.bottom() >= text.bottom() - 1.0, "as tall as the text");
        assert_eq!(painted_bar("", true, false).1, None, "no cursor while nothing is typed");
        assert_eq!(painted_bar("f6", false, false).1, None, "no cursor while the path box or a rename takes the keys");
    }

    #[test]
    fn ctrl_f_filter_mode_shows_the_cursor_before_anything_is_typed() {
        let (text, cursor) = painted_bar("", true, true);
        let cursor = cursor.expect("cursor painted in filter mode");
        assert!((cursor.left() - text.left()).abs() <= 2.0, "before the placeholder: {cursor:?} vs {text:?}");
        assert_eq!(painted_bar("", false, true).1, None, "not while the path box or a rename takes the keys");
        let (text, cursor) = painted_bar("f6", true, true);
        assert!((cursor.expect("cursor").left() - text.right()).abs() <= 2.0, "after typed text, as before");
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
            filter_mode: false,
            item_count: 9,
            hidden_count: 0,
            show_hidden: false,
            loading: false,
            last_elapsed: None,
            error: Some(LONG_ERROR),
            job: None,
            notice: None,
            counting: None,
            walking: None,
            folder_count: 2,
            selected: None,
            picked: 0,
            picked_size: PickedSize::default(),
            view: ViewSize::default(),
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 200.0));
        let mut shapes = Vec::new();
        for frame in 0..4 {
            let mut input = egui::RawInput { screen_rect: Some(screen), time: Some(frame as f64), ..Default::default() };
            if let Some(pos) = pointer {
                input.events.push(egui::Event::PointerMoved(pos));
            }
            let mut output = ctx.run_ui(input, |ui| { show(ui.ctx(), ui.max_rect(), &status); });
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
        assert!(error.bottom() <= position.top(), "floats above the counters, leaving them visible: {error:?} vs {position:?}");
        assert!(error.right() <= 640.0, "inside the window");
        assert!(!texts.iter().any(|(t, _, elided)| t == LONG_ERROR && !elided), "no tooltip without hover");

        let hovered = painted_narrow(Some(error.center()));
        let full = hovered.iter().filter(|(t, _, elided)| t == LONG_ERROR && !elided).count();
        assert_eq!(full, 1, "hover shows the whole error, once");
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
    fn one_item_is_counted_in_the_singular() {
        assert_eq!(count_text(1, false), "1 item");
        crate::i18n::set(crate::i18n::Lang::Es);
        let spanish = [count_text(1, false), count_text(2, false)];
        crate::i18n::set(crate::i18n::Lang::En);
        assert_eq!(spanish, ["1 elemento", "2 elementos"]);
    }

    #[test]
    fn counting_reads_the_folder_and_running_totals() {
        let t = Totals { folders: 3, files: 1, bytes: 0, unreadable: 0 };
        assert_eq!(counting_text("sub", &t), "Calculating “sub”… 3 folders · 1 file");
        assert_eq!(walking_text(1), "Listing subfolders… 1 item");
        assert_eq!(walking_text(1234), "Listing subfolders… 1234 items");
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
        assert_eq!(progress_text(Mode::Copy, &p, None), "Copying 3/10 — 45%");
        assert_eq!(progress_text(Mode::Move, &Progress::default(), None), "Moving 0/0 — 100%");
    }
}
