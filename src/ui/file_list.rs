//! The file table. Only visible rows are laid out, so huge folders stay fast.

use std::path::PathBuf;

use eframe::egui::{self, text::CCursorRange};
use egui_extras::{Column, TableBuilder};

use crate::app::{Scroll, Sort, SortColumn, View};
use crate::format::{format_size, format_time};
use crate::fs::Entry;
use crate::fs::count::Totals;
use crate::ui::file_icons::FileIcons;
use crate::ui::icons;
use crate::ui::path_popup::{marked_name, word_ranges};

pub enum ListEvent {
    Select(usize),
    /// Ctrl+click: add or remove the row.
    Toggle(usize),
    /// Shift+click: select the range up to the row.
    Extend(usize),
    /// Right click on the row: its menu.
    Menu(usize),
    /// A band dragged from a row's empty part or below the rows, in row units
    /// (row `i` spans `[i, i + 1)`): sent on every frame the pointer drags it.
    Band { start: f32, end: f32 },
    /// A click below the rows; `menu` for a right click.
    Empty { menu: bool },
    Activate(usize),
    Sort(SortColumn),
    /// The in-place rename ended: `true` with Enter, `false` with Escape or a click elsewhere.
    RenameEnded(bool),
    /// A column border was dragged and let go: the new widths.
    Resized(Widths),
    /// Right click on a column's header: its menu.
    HeaderMenu(usize),
    /// Middle click on a column's header: hide it (never Name).
    CloseColumn(usize),
}

/// Each column's width in points, in `COLUMNS` order.
pub type Widths = [f32; COLUMNS.len()];

/// The widths Lumen starts with and goes back to.
pub const DEFAULT_WIDTHS: Widths = [320.0, 110.0, 130.0, 60.0, 60.0, 80.0];

/// Which columns are shown, in `COLUMNS` order; Name always is.
pub type Shown = [bool; COLUMNS.len()];

pub const ALL_SHOWN: Shown = [true; COLUMNS.len()];

/// The columns from left to right, as indices into `COLUMNS`.
pub type Order = [usize; COLUMNS.len()];

pub const DEFAULT_ORDER: Order = [0, 1, 2, 3, 4, 5];

/// `order` with `column` swapped with the nearest shown column on its left
/// (or right); unchanged at that end.
pub fn shift(order: &Order, shown: &Shown, column: usize, left: bool) -> Order {
    let mut order = *order;
    let Some(at) = order.iter().position(|&c| c == column) else { return order };
    // Name is always shown.
    let visible = |i: &usize| order[*i] == 0 || shown[order[*i]];
    let next = if left { (0..at).rev().find(visible) } else { (at + 1..order.len()).find(visible) };
    if let Some(next) = next {
        order.swap(at, next);
    }
    order
}

/// No column is dragged narrower than this.
pub const MIN_WIDTH: f32 = 40.0;

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

/// How opaque a cut entry's icon and name are until it is pasted.
const CUT_OPACITY: f32 = 0.5;

/// Highest list zoom level (Ctrl+wheel, Ctrl+Plus); 0 is the normal size.
pub const MAX_ZOOM: u8 = 4;

/// Icon side at each zoom level, in text heights: only icons and rows grow,
/// the text keeps its size (like Dolphin's details view).
const ZOOM_FACTORS: [f32; MAX_ZOOM as usize + 1] = [1.0, 1.5, 2.0, 3.0, 4.0];

/// The icon side for `zoom` when a line of text is `text_height` tall.
pub fn icon_side(zoom: u8, text_height: f32) -> f32 {
    text_height * ZOOM_FACTORS[usize::from(zoom.min(MAX_ZOOM))]
}

/// A row is its icon plus a little air.
pub fn row_height(zoom: u8, text_height: f32) -> f32 {
    icon_side(zoom, text_height) + 4.0
}

/// How big the list is drawn.
#[derive(Clone, Copy)]
pub struct Sizes {
    /// Icon and row size, 0 (normal) to `MAX_ZOOM`.
    pub zoom: u8,
    pub widths: Widths,
    pub shown: Shown,
    pub order: Order,
}

impl Default for Sizes {
    fn default() -> Self {
        Self { zoom: 0, widths: DEFAULT_WIDTHS, shown: ALL_SHOWN, order: DEFAULT_ORDER }
    }
}

/// Draws `view`'s rows at `sizes`; `scroll` brings the selection into view.
pub fn show(
    ui: &mut egui::Ui,
    view: &View,
    cut: &[PathBuf],
    sizes: Sizes,
    scroll: Option<Scroll>,
    mut edit: Option<&mut RenameEdit>,
    file_icons: &FileIcons,
) -> Option<ListEvent> {
    let zoom = sizes.zoom;
    let View { entries, rows, totals, selected, sort, .. } = view;
    let (selected, sort) = (*selected, *sort);
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let header_height = row_height(0, text_height);
    let (icon, row_height) = (icon_side(zoom, text_height), row_height(zoom, text_height));
    // With rows picked, they are the highlight; the cursor row alone otherwise.
    let multi = view.picked_count() > 0;
    let mut event = None;
    // Set again below if the cursor row is drawn this frame.
    ui.ctx().data_mut(|d| d.remove::<egui::Rect>(cursor_row_id()));

    // While a border is dragged, the widths the pointer gives now; the saved ones otherwise.
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let widths = match (ui.ctx().data(|d| d.get_temp::<Drag>(drag_id())), pointer) {
        (Some(drag), Some(pointer)) => drag.at(pointer.x),
        _ => sizes.widths,
    };
    let mut cells = [egui::Rect::NOTHING; COLUMNS.len()];
    // From the rows: one drawn row's index and top, to turn a height on
    // screen into row units; and whether the button went down here on a
    // row's empty part (not its icon or name), where a band may start.
    let pitch = row_height + ui.spacing().item_spacing.y;
    let mut first_drawn: Option<(usize, f32)> = None;
    let mut band_may_start = false;
    let pressed = ui.input(|i| i.pointer.primary_pressed());

    egui::ScrollArea::horizontal().id_salt("lumen_list_columns").auto_shrink(false).show(ui, |ui| {
        let mut table = TableBuilder::new(ui)
            .striped(true)
            // The vertical scroll bar at the list's right edge, not after the last column.
            .auto_shrink([false, true])
            // Text sits in the middle of rows made taller by the zoom.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::click());
        // Every column has its own width, like File Pilot: the last one stretches
        // over whatever room the others leave, and columns wider than the list
        // scroll sideways.
        // In their order; Name is always shown.
        let shown: Vec<usize> = sizes.order.into_iter().filter(|&c| c == 0 || sizes.shown[c]).collect();
        let (&last, others) = shown.split_last().expect("Name is shown");
        for &column in others {
            table = table.column(Column::exact(widths[column]).clip(true));
        }
        table = table.column(Column::remainder().at_least(widths[last]).clip(true));
        if let Some(scroll) = scroll
            && let Some(sel) = selected
        {
            table = table
                .scroll_to_row(sel, None)
                .animate_scrolling(scroll == Scroll::Smooth);
        }

        table
            .header(header_height, |mut header| {
                for &i in &shown {
                    let (label, column) = COLUMNS[i];
                    header.col(|ui| {
                        cells[i] = ui.max_rect();
                        // The whole header, not only its label: a click sorts, a right click opens its menu.
                        let cell = ui.interact(ui.max_rect(), egui::Id::new(("lumen_header", i)), egui::Sense::click());
                        let label = sort_header(ui, label, column, sort);
                        if label.clicked() || cell.clicked() {
                            event = Some(ListEvent::Sort(column));
                        } else if label.secondary_clicked() || cell.secondary_clicked() {
                            event = Some(ListEvent::HeaderMenu(i));
                        } else if (label.middle_clicked() || cell.middle_clicked()) && i != 0 {
                            event = Some(ListEvent::CloseColumn(i));
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let i = row.index();
                    let entry = &entries[rows[i]];
                    let mut on_name = false;
                    row.set_selected(if multi { view.is_picked(i) } else { selected == Some(i) });
                    let counts = count_cells(entry, totals.get(&entry.name));
                    for &column in &shown {
                        row.col(|ui| match column {
                            0 => {
                                // Icon and name on one line as tall as the row, centred in it:
                                // a plain `horizontal` is one text line tall, so a zoomed icon
                                // would hang below the row and be clipped (and the name with it).
                                let line = egui::vec2(ui.available_width(), row_height);
                                ui.allocate_ui_with_layout(line, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    // Cut and not pasted yet: faded, like other explorers.
                                    if cut.contains(&entry.path) {
                                        ui.set_opacity(CUT_OPACITY);
                                    }
                                    icons::show(ui, entry, icon, file_icons.texture(entry));
                                    if let Some(edit) = edit.as_deref_mut().filter(|e| e.path == entry.path) {
                                        if let Some(submit) = rename_field(ui, edit) {
                                            event = Some(ListEvent::RenameEnded(submit));
                                        }
                                    } else {
                                        name_label(ui, entry, view.filter());
                                    }
                                    // The icon and the name: kept for dragging the entry itself.
                                    on_name = ui.input(|i| i.pointer.press_origin()).is_some_and(|p| ui.min_rect().contains(p));
                                });
                            }
                            1 => {
                                ui.add(egui::Label::new(entry.kind()).selectable(false).truncate());
                            }
                            2 => {
                                let date = entry.modified.map(format_time).unwrap_or_default();
                                ui.add(egui::Label::new(date).selectable(false));
                            }
                            _ => count_cell(ui, column - 3, &counts[column - 3]),
                        });
                    }
                    let response = row.response();
                    first_drawn.get_or_insert((i, response.rect.top()));
                    if pressed && response.is_pointer_button_down_on() && !on_name {
                        band_may_start = true;
                    }
                    if selected == Some(i) {
                        response.ctx.data_mut(|d| d.insert_temp(cursor_row_id(), response.rect));
                    }
                    if response.secondary_clicked() {
                        event = Some(ListEvent::Menu(i));
                    } else if response.double_clicked() {
                        event = Some(ListEvent::Activate(i));
                    } else if response.clicked() {
                        let m = response.ctx.input(|input| input.modifiers);
                        event = Some(if m.command {
                            ListEvent::Toggle(i)
                        } else if m.shift {
                            ListEvent::Extend(i)
                        } else {
                            ListEvent::Select(i)
                        });
                    }
                });
            });
        if let Some(resized) = column_borders(ui, &cells, &widths) {
            event = Some(resized);
        }
        // Whatever space the rows leave below them.
        let rest = ui.available_rect_before_wrap();
        if event.is_none() && rest.height() > 0.0 {
            let response = ui.interact(rest, egui::Id::new("lumen_list_empty"), egui::Sense::click());
            if pressed && response.is_pointer_button_down_on() {
                band_may_start = true;
            }
            if response.clicked() {
                event = Some(ListEvent::Empty { menu: false });
            } else if response.secondary_clicked() {
                event = Some(ListEvent::Empty { menu: true });
            }
        }
        if let Some((row, top)) = first_drawn
            && let Some(band) = band(ui, pressed, band_may_start, |y| row as f32 + (y - top) / pitch, |u| top + (u - row as f32) * pitch)
        {
            event = Some(band);
        }
    });
    event
}

/// Where a band dragged over the rows started: in row units, so a scroll
/// does not move it, and the x on screen.
#[derive(Clone, Copy)]
struct BandStart {
    at: f32,
    x: f32,
}

fn band_id() -> egui::Id {
    egui::Id::new("lumen_list_band")
}

/// The band dragged with the primary button: remembered from the press when
/// `may_start`, drawn and reported while the pointer drags it, forgotten once
/// the button is up. `unit` turns a height on screen into row units, `y` back.
fn band(ui: &egui::Ui, pressed: bool, may_start: bool, unit: impl Fn(f32) -> f32, y: impl Fn(f32) -> f32) -> Option<ListEvent> {
    let (origin, down, dragging, pointer) =
        ui.input(|i| (i.pointer.press_origin(), i.pointer.primary_down(), i.pointer.is_decidedly_dragging(), i.pointer.latest_pos()));
    if pressed {
        let start = origin.filter(|_| may_start).map(|p| BandStart { at: unit(p.y), x: p.x });
        ui.ctx().data_mut(|d| {
            if let Some(start) = start {
                d.insert_temp(band_id(), start);
            } else {
                d.remove::<BandStart>(band_id());
            }
        });
    }
    let start = ui.ctx().data(|d| d.get_temp::<BandStart>(band_id()))?;
    if !down {
        ui.ctx().data_mut(|d| d.remove::<BandStart>(band_id()));
        return None;
    }
    let pointer = pointer.filter(|_| dragging)?;
    let rect = egui::Rect::from_two_pos(egui::pos2(start.x, y(start.at)), pointer);
    let selection = ui.visuals().selection;
    ui.painter().rect(rect, 0.0, selection.bg_fill.gamma_multiply(0.3), selection.stroke, egui::StrokeKind::Inside);
    Some(ListEvent::Band { start: start.at, end: unit(pointer.y) })
}

/// The width that shows every cell of `column` and its header whole, for
/// `view`'s rows at `zoom`.
pub fn fit_width(ctx: &egui::Context, view: &View, zoom: u8, column: usize) -> f32 {
    let style = ctx.global_style();
    let font = egui::TextStyle::Body.resolve(&style);
    let measure = |text: String| ctx.fonts_mut(|f| f.layout_no_wrap(text, font.clone(), egui::Color32::WHITE).size().x);
    // The header with room for the sort arrow, which may show up later.
    let header = measure(format!("{} {SORT_DESCENDING}", COLUMNS[column].0)) + 2.0 * style.spacing.button_padding.x;
    let icon = icon_side(zoom, ctx.fonts_mut(|f| f.row_height(&font))) + style.spacing.item_spacing.x;
    let widest = view
        .rows
        .iter()
        .map(|&row| {
            let entry = &view.entries[row];
            match column {
                0 => icon + measure(entry.name.clone()),
                1 => measure(entry.kind().to_string()),
                2 => measure(entry.modified.map(format_time).unwrap_or_default()),
                _ => {
                    let cells = count_cells(entry, view.totals.get(&entry.name));
                    measure(cells[column - 3].clone())
                }
            }
        })
        .fold(header, f32::max);
    // Whole points: the config keeps them so.
    (widest + FIT_MARGIN).max(MIN_WIDTH).ceil()
}

/// Air left after the widest cell of a column sized to fit.
const FIT_MARGIN: f32 = 6.0;

/// A column border being dragged: which column, where the pointer and the
/// column's width were when it was grabbed, and the widths before it.
#[derive(Clone)]
struct Drag {
    column: usize,
    start_x: f32,
    start_width: f32,
    widths: Widths,
}

impl Drag {
    /// The widths with the pointer at `x`.
    fn at(&self, x: f32) -> Widths {
        let mut widths = self.widths;
        widths[self.column] = (self.start_width + x - self.start_x).max(MIN_WIDTH);
        widths
    }
}

fn drag_id() -> egui::Id {
    egui::Id::new("lumen_column_drag")
}

/// How far either side of a border the pointer grabs it.
const BORDER_GRAB: f32 = 4.0;

/// The draggable border on the right of each header `cells` holds, drawn
/// over the header. Returns the new widths when a border is let go.
fn column_borders(ui: &mut egui::Ui, cells: &[egui::Rect; COLUMNS.len()], widths: &Widths) -> Option<ListEvent> {
    let gap = ui.spacing().item_spacing.x;
    let mut event = None;
    for (column, cell) in cells.iter().enumerate().filter(|(_, cell)| cell.is_positive()) {
        let x = cell.right() + gap / 2.0;
        let rect = egui::Rect::from_x_y_ranges(x - BORDER_GRAB..=x + BORDER_GRAB, cell.y_range());
        let response = ui.interact(rect, egui::Id::new(("lumen_column_border", column)), egui::Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            let stroke = if response.dragged() { ui.visuals().widgets.active.fg_stroke } else { ui.visuals().widgets.hovered.fg_stroke };
            ui.painter().vline(x, cell.y_range(), stroke);
        }
        let pointer = response.interact_pointer_pos();
        // From where the button went down: egui starts a drag only once the pointer moved.
        if response.drag_started()
            && let Some(start) = ui.input(|i| i.pointer.press_origin()).or(pointer)
        {
            // The width drawn: the last column may be stretched past its own.
            let drag = Drag { column, start_x: start.x, start_width: cell.width(), widths: *widths };
            ui.ctx().data_mut(|d| d.insert_temp(drag_id(), drag));
        }
        if response.drag_stopped()
            && let Some(drag) = ui.ctx().data(|d| d.get_temp::<Drag>(drag_id()))
        {
            ui.ctx().data_mut(|d| d.remove::<Drag>(drag_id()));
            let x = pointer.or_else(|| ui.input(|i| i.pointer.latest_pos())).map_or(drag.start_x, |p| p.x);
            event = Some(ListEvent::Resized(drag.at(x)));
        }
    }
    event
}

fn cursor_row_id() -> egui::Id {
    egui::Id::new("lumen_cursor_row")
}

/// Where the cursor row was last drawn: Shift+F10 opens its menu there.
pub fn cursor_row_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    ctx.data(|d| d.get_temp(cursor_row_id()))
}

/// The entry's name, dimmed when hidden, cut with "…" when too long. The
/// letters matching the `typed` words are in the link colour.
pub fn name_label(ui: &mut egui::Ui, entry: &Entry, typed: &str) {
    let ranges = word_ranges(&entry.name, typed).unwrap_or_default();
    ranged_name_label(ui, entry, &ranges);
}

/// [`name_label`] with the letters to mark given, for names found by the
/// quick jump, whose letters need not be whole words.
pub fn ranged_name_label(ui: &mut egui::Ui, entry: &Entry, ranges: &[std::ops::Range<usize>]) {
    let visuals = ui.visuals();
    let normal = if entry.hidden { visuals.weak_text_color() } else { visuals.text_color() };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let job = marked_name(&entry.name, ranges, normal, visuals.hyperlink_color, font);
    ui.add(egui::Label::new(job).selectable(false).truncate());
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
    for (i, cell) in cells.iter().enumerate() {
        row.col(|ui| count_cell(ui, i, cell));
    }
}

/// Cell `i` of the Folders, Files and Size cells.
fn count_cell(ui: &mut egui::Ui, i: usize, text: &str) {
    if i < 2 {
        ui.add(egui::Label::new(text).selectable(false));
    } else {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(text).selectable(false));
        });
    }
}

/// Id of the rename field, so the app can tell when it lost focus silently.
pub fn rename_id() -> egui::Id {
    egui::Id::new("lumen_rename")
}

/// The text field drawn in place of the name while renaming.
fn rename_field(ui: &mut egui::Ui, edit: &mut RenameEdit) -> Option<bool> {
    let id = rename_id();
    // Characters a name cannot hold are dropped before the field reads them, as
    // File Pilot does: typing one does nothing, a pasted name keeps the rest.
    if ui.memory(|m| m.has_focus(id)) {
        ui.input_mut(|i| {
            for event in &mut i.events {
                if let egui::Event::Text(text) | egui::Event::Paste(text) = event {
                    text.retain(crate::fs::ops::allowed_in_name);
                }
            }
            i.events.retain(|e| !matches!(e, egui::Event::Text(t) | egui::Event::Paste(t) if t.is_empty()));
        });
    }
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
fn sort_header(ui: &mut egui::Ui, label: &str, column: SortColumn, sort: Sort) -> egui::Response {
    let arrow = match (sort.column == column, sort.descending) {
        (false, _) => "",
        (true, false) => SORT_ASCENDING,
        (true, true) => SORT_DESCENDING,
    };
    let text = egui::RichText::new(format!("{label} {arrow}")).strong();
    ui.add(egui::Button::new(text).frame(false))
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

    /// Focuses the rename field (name selected), then sends these events to it.
    fn type_into_rename(name: &str, events: Vec<egui::Event>) -> String {
        let ctx = egui::Context::default();
        let mut edit = RenameEdit { path: PathBuf::from("/x").join(name), is_dir: false, text: name.into(), focus: true };
        for events in [Vec::new(), events] {
            let input = egui::RawInput { events, ..egui::RawInput::default() };
            ctx.run_ui(input, |ui| {
                rename_field(ui, &mut edit);
            })
            .drop_without_applying_deltas();
        }
        edit.text
    }

    #[test]
    fn forbidden_characters_are_never_typed_into_the_rename_field() {
        let typed = type_into_rename("notes.txt", vec![egui::Event::Text("a/b".into())]);
        assert_eq!(typed, "ab.txt", "the slash is dropped, the rest is typed");
        let pasted = type_into_rename("notes.txt", vec![egui::Event::Paste("x/y\0z".into())]);
        assert_eq!(pasted, "xyz.txt", "pasted text keeps its other characters");
        let windows = type_into_rename("notes.txt", vec![egui::Event::Text("a:b*c?".into())]);
        let expected = if cfg!(windows) { "abc.txt" } else { "a:b*c?.txt" };
        assert_eq!(windows, expected, "Windows' own forbidden characters");
        let only_forbidden = type_into_rename("notes.txt", vec![egui::Event::Text("/".into())]);
        assert_eq!(only_forbidden, "notes.txt", "a forbidden key leaves the selected name alone");
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
    fn zoom_grows_icons_and_rows_from_the_normal_size_up_to_a_limit() {
        let text = 16.0;
        assert_eq!(icon_side(0, text), text, "level 0 looks as before");
        assert_eq!(row_height(0, text), text + 4.0);
        let sides: Vec<f32> = (0..=MAX_ZOOM).map(|z| icon_side(z, text)).collect();
        assert!(sides.windows(2).all(|w| w[0] < w[1]), "each level is bigger: {sides:?}");
        assert_eq!(icon_side(MAX_ZOOM, text), 4.0 * text);
        assert_eq!(icon_side(MAX_ZOOM + 3, text), icon_side(MAX_ZOOM, text), "never past the limit");
    }

    /// Where each text and the icon of the first row were painted, at `zoom`.
    fn painted(zoom: u8) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows) = (vec![entry("a.txt")], vec![0]);
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &view, &[], Sizes { zoom, ..Sizes::default() }, None, None, &FileIcons::default());
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        shapes
    }

    #[test]
    fn a_file_whose_icon_is_loaded_shows_it_and_one_still_loading_keeps_the_painted_one() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows) = (vec![entry("setup.exe"), entry("notes.txt")], vec![0, 1]);
        let mut icons = FileIcons::default();
        let image = egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]);
        let texture = ctx.load_texture("test icon", image, egui::TextureOptions::LINEAR);
        icons.insert(&view.entries[0], texture.clone());
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &view, &[], Sizes::default(), None, None, &icons);
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let drawn: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => Some(s.shape.visual_bounding_rect()),
                _ => None,
            })
            .collect();
        assert_eq!(drawn.len(), 1, "the .exe's icon, once: {drawn:?}");
        let row = |text: &str| {
            shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos.y + t.galley.rect.center().y),
                _ => None,
            })
        };
        let (exe, txt) = (row("setup.exe").unwrap(), row("notes.txt").unwrap());
        assert!((drawn[0].center().y - exe).abs() < 2.0, "on the .exe's row, not on {txt}");
    }

    #[test]
    fn zoomed_names_stay_level_with_the_other_columns_and_icons_are_whole() {
        for zoom in 0..=MAX_ZOOM {
            let shapes = painted(zoom);
            let y = |text: &str| {
                shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos.y + t.galley.rect.center().y),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{text:?} painted"))
            };
            assert!((y("a.txt") - y("TXT file")).abs() < 1.5, "zoom {zoom}: name off its row");
            // The icon: small shapes in the Name column, none cut by the row's clip.
            for s in shapes.iter().filter(|s| !matches!(s.shape, egui::Shape::Text(_)) && s.clip_rect.min.x < 20.0) {
                let b = s.shape.visual_bounding_rect();
                if b.is_finite() && b.width() < 80.0 {
                    assert!(b.max.y <= s.clip_rect.max.y + 0.5, "zoom {zoom}: icon clipped {b:?} by {:?}", s.clip_rect);
                }
            }
        }
    }

    #[test]
    fn the_filter_words_are_highlighted_in_the_name() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let listing = crate::fs::Listing {
            id: 1,
            path: PathBuf::from("/x"),
            result: Ok(vec![entry("Slides de contenido"), entry("notes.txt")]),
            elapsed: std::time::Duration::ZERO,
        };
        view.apply_listing(listing, None);
        for c in "contenido sli".chars() {
            view.push_filter(c);
        }
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default());
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let marked = ctx.global_style().visuals.hyperlink_color;
        let galley = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Slides de contenido" => Some(t.galley.clone()),
                _ => None,
            })
            .expect("name painted");
        let coloured: Vec<&str> = galley
            .job
            .sections
            .iter()
            .filter(|section| section.format.color == marked)
            .map(|section| &galley.job.text[section.byte_range.start.0..section.byte_range.end.0])
            .collect();
        assert_eq!(coloured, ["Sli", "contenido"]);
    }

    fn press(pos: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
        let event = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
        vec![event(true), event(false)]
    }

    /// One entry listed, then a left and a right click well below it.
    #[test]
    fn a_click_below_the_rows_is_a_click_on_the_empty_space() {
        use crate::ui::testing::frame;
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows, view.selected) = (vec![entry("a.txt")], vec![0], Some(0));
        let below = egui::pos2(300.0, 500.0);
        let mut draw = |ui: &mut egui::Ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default());
        frame(&ctx, vec![egui::Event::PointerMoved(below)], &mut draw);
        let left = frame(&ctx, press(below, egui::PointerButton::Primary), &mut draw);
        assert!(matches!(left, Some(ListEvent::Empty { menu: false })), "left click");
        let right = frame(&ctx, press(below, egui::PointerButton::Secondary), &mut draw);
        assert!(matches!(right, Some(ListEvent::Empty { menu: true })), "right click");
    }

    /// Where the row `row` of `view` is drawn once settled.
    fn row_rect(ctx: &egui::Context, view: &mut View, row: usize) -> egui::Rect {
        use crate::ui::testing::frame;
        let was = view.selected.replace(row);
        for _ in 0..2 {
            frame(ctx, vec![], |ui| show(ui, view, &[], Sizes::default(), None, None, &FileIcons::default()));
        }
        view.selected = was;
        cursor_row_rect(ctx).expect("row drawn")
    }

    /// A drag with the primary button from `from` through `to`: the list's
    /// event on each frame from the press to the release, and what was
    /// painted while the pointer was at `to`.
    fn drag(ctx: &egui::Context, view: &View, from: egui::Pos2, to: egui::Pos2) -> (Vec<Option<ListEvent>>, Vec<egui::epaint::ClippedShape>) {
        let steps = vec![
            vec![egui::Event::PointerMoved(from), button(from, true)],
            vec![egui::Event::PointerMoved(from + egui::vec2(0.0, 10.0 * (to.y - from.y).signum()))],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ];
        let (mut events, mut shapes) = (Vec::new(), Vec::new());
        for (i, step) in steps.into_iter().enumerate() {
            let mut event = None;
            let mut output = ctx.run_ui(
                egui::RawInput { events: step, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        event = show(ui, view, &[], Sizes::default(), None, None, &FileIcons::default());
                    });
                },
            );
            output.textures_delta.clear();
            if i == 2 {
                shapes = output.shapes;
            }
            events.push(event);
        }
        (events, shapes)
    }

    /// The band's start and end rows, when `event` is a band.
    fn band_rows(event: &Option<ListEvent>) -> Option<(f32, f32)> {
        match event {
            Some(ListEvent::Band { start, end }) => Some((start.floor(), end.floor())),
            _ => None,
        }
    }

    #[test]
    fn a_drag_from_a_rows_empty_part_bands_the_rows_it_crosses_and_draws_the_band() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        let (first, third) = (row_rect(&ctx, &mut view, 0), row_rect(&ctx, &mut view, 2));
        // In the Date Modified column: no icon or name there.
        let (from, to) = (egui::pos2(500.0, first.center().y), egui::pos2(520.0, third.center().y));
        let (events, shapes) = drag(&ctx, &view, from, to);
        assert_eq!(band_rows(&events[2]), Some((0.0, 2.0)), "{:?}", events.iter().map(band_rows).collect::<Vec<_>>());
        assert!(events[3].is_none(), "letting go is not a click on the row");
        let band = shapes.iter().any(|s| match &s.shape {
            egui::Shape::Rect(r) => {
                (r.rect.top() - from.y).abs() < 1.0 && (r.rect.bottom() - to.y).abs() < 1.0 && (r.rect.left() - from.x).abs() < 1.0 && (r.rect.right() - to.x).abs() < 1.0
            }
            _ => false,
        });
        assert!(band, "the band is drawn from the press to the pointer");
        // Dragged back upwards: the start stays on the first row.
        let (events, _) = drag(&ctx, &view, to, from);
        assert_eq!(band_rows(&events[2]), Some((2.0, 0.0)));
    }

    #[test]
    fn a_drag_from_below_the_rows_bands_up_to_the_pointer() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt"]);
        let second = row_rect(&ctx, &mut view, 1);
        let (events, _) = drag(&ctx, &view, egui::pos2(300.0, 500.0), egui::pos2(300.0, second.center().y));
        let (start, end) = band_rows(&events[2]).expect("a band");
        assert!(start >= 3.0, "starts below the last row: {start}");
        assert_eq!(end, 1.0);
    }

    #[test]
    fn a_drag_on_the_icon_or_the_name_is_not_a_band() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt"]);
        let (first, third) = (row_rect(&ctx, &mut view, 0), row_rect(&ctx, &mut view, 2));
        for x in [first.left() + 4.0, first.left() + 30.0] {
            let (events, _) = drag(&ctx, &view, egui::pos2(x, first.center().y), egui::pos2(x, third.center().y));
            assert!(events.iter().all(|e| band_rows(e).is_none()), "x = {x}");
        }
    }

    #[test]
    fn a_click_on_a_rows_empty_part_still_selects_it() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt"]);
        let second = row_rect(&ctx, &mut view, 1);
        let at = egui::pos2(500.0, second.center().y);
        crate::ui::testing::frame(&ctx, vec![egui::Event::PointerMoved(at)], |ui| {
            show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default())
        });
        let clicked = crate::ui::testing::frame(&ctx, press(at, egui::PointerButton::Primary), |ui| {
            show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default())
        });
        assert!(matches!(clicked, Some(ListEvent::Select(1))));
    }

    /// How opaque the first letter of `name` was painted, with `cut` waiting to be pasted.
    fn name_opacity(name: &str, cut: &[PathBuf]) -> f32 {
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows) = (vec![entry("a.txt"), entry("b.txt")], vec![0, 1]);
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &view, cut, Sizes::default(), None, None, &FileIcons::default());
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == name => {
                    let glyph = t.galley.rows[0].visuals.mesh.vertices[0].color;
                    Some(f32::from(glyph.a()) / 255.0)
                }
                _ => None,
            })
            .expect("name painted")
    }

    #[test]
    fn cut_entries_are_greyed_until_pasted() {
        let cut = [PathBuf::from("/x/a.txt")];
        assert!(name_opacity("a.txt", &cut) < 0.7, "the cut one is faded");
        assert_eq!(name_opacity("b.txt", &cut), 1.0, "the others are not");
        assert_eq!(name_opacity("a.txt", &[]), 1.0);
    }

    #[test]
    fn the_cursor_row_is_remembered_for_shift_f10() {
        use crate::ui::testing::frame;
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows, view.selected) = (vec![entry("a.txt"), entry("b.txt")], vec![0, 1], Some(1));
        frame(&ctx, vec![], |ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default()));
        let second = cursor_row_rect(&ctx).expect("drawn");
        view.selected = Some(0);
        frame(&ctx, vec![], |ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default()));
        let first = cursor_row_rect(&ctx).expect("drawn");
        assert!(first.bottom() <= second.top() + 1.0, "{first:?} above {second:?}");
        // A cursor row scrolled out of view was not drawn: no stale place.
        view.entries = (0..300).map(|i| entry(&format!("f{i}"))).collect();
        view.rows = (0..300).collect();
        view.selected = Some(250);
        frame(&ctx, vec![], |ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default()));
        assert_eq!(cursor_row_rect(&ctx), None);
    }

    /// One frame of the list: where each header text starts (`None` when it
    /// is out of view), the list's event and the pointer's look.
    struct Drawn {
        lefts: Vec<Option<f32>>,
        event: Option<ListEvent>,
        cursor: egui::CursorIcon,
    }

    fn draw_list(ctx: &egui::Context, view: &View, widths: &Widths, events: Vec<egui::Event>) -> Drawn {
        let mut event = None;
        let mut output = ctx.run_ui(
            egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    event = show(ui, view, &[], Sizes { widths: *widths, ..Sizes::default() }, None, None, &FileIcons::default());
                });
            },
        );
        output.textures_delta.clear();
        let lefts = COLUMNS
            .iter()
            .map(|(label, _)| {
                output.shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Text(t)
                        if t.galley.text().trim_end_matches([' ', '⏶', '⏷']) == *label
                            && s.clip_rect.intersects(egui::Rect::from_min_max(t.pos, t.pos + t.galley.size())) =>
                    {
                        Some(t.pos.x)
                    }
                    _ => None,
                })
            })
            .collect();
        Drawn { lefts, event, cursor: output.platform_output.cursor_icon }
    }

    /// The header text lefts once the list has settled, all in view.
    fn settled(ctx: &egui::Context, view: &View, widths: &Widths) -> Vec<f32> {
        let mut lefts = Vec::new();
        for _ in 0..3 {
            lefts = draw_list(ctx, view, widths, vec![]).lefts;
        }
        lefts.into_iter().map(|l| l.expect("header in view")).collect()
    }

    fn listed(names: &[&str]) -> View {
        let mut view = View::default();
        view.entries = names.iter().map(|n| entry(n)).collect();
        view.rows = (0..names.len()).collect();
        view
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn every_column_keeps_its_own_width_even_past_the_window() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let normal = settled(&ctx, &view, &DEFAULT_WIDTHS);
        let wide_widths = [600.0, 110.0, 130.0, 60.0, 60.0, 80.0];
        for _ in 0..2 {
            draw_list(&ctx, &view, &wide_widths, vec![]);
        }
        let wide = draw_list(&ctx, &view, &wide_widths, vec![]).lefts;
        let (name, ty) = (wide[0].expect("Name in view"), wide[1].expect("Type in view"));
        assert!((ty - name - (normal[1] - normal[0]) - 280.0).abs() < 1.0, "Name is 280 wider: {normal:?} {wide:?}");
        assert_eq!(wide[5], None, "Size is past the 800 pt window, not squeezed into it: {wide:?}");
        // Scrolling sideways over the list brings it into view.
        let over = egui::pos2(400.0, 100.0);
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-400.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        draw_list(&ctx, &view, &wide_widths, vec![egui::Event::PointerMoved(over)]);
        draw_list(&ctx, &view, &wide_widths, vec![wheel]);
        let mut scrolled = Vec::new();
        for _ in 0..3 {
            scrolled = draw_list(&ctx, &view, &wide_widths, vec![]).lefts;
        }
        assert!(scrolled[5].is_some_and(|x| x < 800.0), "Size scrolled into view: {scrolled:?}");
    }

    #[test]
    fn the_last_column_stretches_to_the_lists_edge_when_the_columns_leave_room() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let narrow = [150.0, 60.0, 60.0, 40.0, 40.0, 50.0];
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        show(ui, &view, &[], Sizes { widths: narrow, ..Sizes::default() }, None, None, &FileIcons::default());
                    });
                },
            );
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let size_right = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "1 B" => Some(t.pos.x + t.galley.size().x),
                _ => None,
            })
            .expect("size painted");
        // The 800 pt screen less the panel's margin and the scroll bar's room.
        assert!(size_right > 750.0, "Size is right-aligned at the list's edge, not after 50 pt: {size_right}");
    }

    #[test]
    fn dragging_a_header_border_resizes_its_column_and_reports_the_widths_once_let_go() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt", "b.txt"]);
        let widths = DEFAULT_WIDTHS;
        let lefts = settled(&ctx, &view, &widths);
        // The border between Name and Type: just left of the Type header's text.
        let border = egui::pos2(lefts[1] - 8.0, 10.0);
        let hover = draw_list(&ctx, &view, &widths, vec![egui::Event::PointerMoved(border)]);
        assert_eq!(hover.cursor, egui::CursorIcon::ResizeColumn, "the pointer shows the border can be dragged");
        draw_list(&ctx, &view, &widths, vec![button(border, true)]);
        let mut during = Vec::new();
        for step in 1..=5 {
            let moved = egui::Event::PointerMoved(border + egui::vec2(10.0 * step as f32, 0.0));
            let drawn = draw_list(&ctx, &view, &widths, vec![moved]);
            assert!(drawn.event.is_none(), "nothing reported while dragging");
            during = drawn.lefts;
        }
        let (name, ty) = (during[0].unwrap(), during[1].unwrap());
        assert!((ty - lefts[1] - 50.0).abs() < 1.5, "Type moved right with the border: {lefts:?} {during:?}");
        assert!((name - lefts[0]).abs() < 0.5, "Name's left side stays");
        let released = draw_list(&ctx, &view, &widths, vec![button(border + egui::vec2(50.0, 0.0), false)]);
        let Some(ListEvent::Resized(new)) = released.event else { panic!("the widths are reported when let go") };
        assert!((new[0] - 370.0).abs() < 1.5, "Name is 50 wider: {new:?}");
        assert_eq!(new[1..], DEFAULT_WIDTHS[1..], "only the dragged column changes");
    }

    #[test]
    fn a_column_is_never_dragged_narrower_than_the_minimum() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let lefts = settled(&ctx, &view, &DEFAULT_WIDTHS);
        // The border between Type and Date Modified, dragged far to the left.
        let border = egui::pos2(lefts[2] - 8.0, 10.0);
        let far = egui::pos2(0.0, 10.0);
        for events in [vec![egui::Event::PointerMoved(border)], vec![button(border, true)], vec![egui::Event::PointerMoved(far)]] {
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, events);
        }
        let released = draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![button(far, false)]);
        let Some(ListEvent::Resized(new)) = released.event else { panic!("reported when let go") };
        assert_eq!(new[1], MIN_WIDTH, "{new:?}");
        assert_eq!(new[0], DEFAULT_WIDTHS[0]);
    }

    #[test]
    fn a_right_click_on_a_header_opens_its_menu_and_a_left_click_still_sorts() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let lefts = settled(&ctx, &view, &DEFAULT_WIDTHS);
        let click = |pos: egui::Pos2, button: egui::PointerButton| {
            let press = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![egui::Event::PointerMoved(pos)]);
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![press(true)]);
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![press(false)]).event
        };
        let on_label = egui::pos2(lefts[1] + 5.0, 10.0);
        let past_label = egui::pos2(lefts[1] + 80.0, 10.0);
        let right = egui::PointerButton::Secondary;
        assert!(matches!(click(on_label, right), Some(ListEvent::HeaderMenu(1))), "on Type's label");
        assert!(matches!(click(past_label, right), Some(ListEvent::HeaderMenu(1))), "on Type's free part");
        let left = click(on_label, egui::PointerButton::Primary);
        assert!(matches!(left, Some(ListEvent::Sort(SortColumn::Type))), "a left click sorts");
    }

    #[test]
    fn hidden_columns_are_not_drawn_and_the_next_one_takes_their_place() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let all = settled(&ctx, &view, &DEFAULT_WIDTHS);
        let shown = [true, false, true, true, true, true];
        let mut shapes = Vec::new();
        let mut lefts = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        show(ui, &view, &[], Sizes { shown, ..Sizes::default() }, None, None, &FileIcons::default());
                    });
                },
            );
            output.textures_delta.clear();
            shapes = output.shapes;
            lefts = COLUMNS
                .iter()
                .map(|(label, _)| {
                    shapes.iter().find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text().trim_end_matches([' ', '⏶', '⏷']) == *label => Some(t.pos.x),
                        _ => None,
                    })
                })
                .collect::<Vec<_>>();
        }
        let painted = |text: &str| shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == text));
        assert_eq!(lefts[1], None, "no Type header");
        assert!(!painted("TXT file"), "no Type cell");
        assert!(painted("a.txt") && painted("1 B"), "the other cells are drawn");
        assert_eq!(lefts[2], Some(all[1]), "Date Modified moves into Type's place");
    }

    #[test]
    fn a_middle_click_on_a_header_closes_its_column_but_never_name() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let lefts = settled(&ctx, &view, &DEFAULT_WIDTHS);
        let middle = |pos: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Middle, pressed, modifiers: egui::Modifiers::NONE };
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![egui::Event::PointerMoved(pos)]);
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![press(true)]);
            draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![press(false)]).event
        };
        assert!(matches!(middle(egui::pos2(lefts[2] + 80.0, 10.0)), Some(ListEvent::CloseColumn(2))));
        assert!(middle(egui::pos2(lefts[0] + 5.0, 10.0)).is_none(), "Name stays");
    }

    #[test]
    fn shifting_a_column_skips_hidden_ones_and_stops_at_the_ends() {
        let all = ALL_SHOWN;
        assert_eq!(shift(&DEFAULT_ORDER, &all, 2, true), [0, 2, 1, 3, 4, 5]);
        assert_eq!(shift(&DEFAULT_ORDER, &all, 2, false), [0, 1, 3, 2, 4, 5]);
        assert_eq!(shift(&DEFAULT_ORDER, &all, 0, true), DEFAULT_ORDER, "Name is already first");
        assert_eq!(shift(&DEFAULT_ORDER, &all, 5, false), DEFAULT_ORDER, "Size is already last");
        assert_eq!(shift(&DEFAULT_ORDER, &all, 0, false), [1, 0, 2, 3, 4, 5], "Name moves too");
        let no_type = [true, false, true, true, true, true];
        assert_eq!(shift(&DEFAULT_ORDER, &no_type, 2, true), [2, 1, 0, 3, 4, 5], "past the hidden Type, onto Name");
        let no_size = [true, true, true, true, true, false];
        assert_eq!(shift(&DEFAULT_ORDER, &no_size, 4, false), DEFAULT_ORDER, "only hidden ones on the right");
        let moved = [3, 0, 1, 2, 4, 5];
        assert_eq!(shift(&moved, &all, 3, false), [0, 3, 1, 2, 4, 5], "follows the current order");
    }

    #[test]
    fn columns_are_drawn_in_their_order() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt"]);
        let order = [2, 0, 1, 3, 4, 5];
        let mut lefts = Vec::new();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        show(ui, &view, &[], Sizes { order, ..Sizes::default() }, None, None, &FileIcons::default());
                    });
                },
            );
            output.textures_delta.clear();
            shapes = output.shapes;
            lefts = COLUMNS
                .iter()
                .map(|(label, _)| {
                    shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::Shape::Text(t) if t.galley.text().trim_end_matches([' ', '⏶', '⏷']) == *label => Some(t.pos.x),
                            _ => None,
                        })
                        .expect("header painted")
                })
                .collect::<Vec<_>>();
        }
        assert!(lefts[2] < lefts[0] && lefts[0] < lefts[1] && lefts[1] < lefts[3], "Date Modified, Name, Type, ...: {lefts:?}");
        let name = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "a.txt" => Some(t.pos.x),
                _ => None,
            })
            .expect("name painted");
        assert!(name > lefts[0] && name < lefts[1], "the name sits under its header: {name} in {lefts:?}");
    }

    /// What is painted of the row whose name starts like `name`, with Name `width` wide.
    fn painted_name(view: &View, width: f32, name: &str) -> String {
        let ctx = egui::Context::default();
        let widths = [width, 110.0, 130.0, 60.0, 60.0, 80.0];
        let wide = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2000.0, 600.0));
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(wide), ..Default::default() }, |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    show(ui, view, &[], Sizes { widths, ..Sizes::default() }, None, None, &FileIcons::default());
                });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let first = name.split(' ').next().unwrap();
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                // The characters laid out: an elided name ends in "…" there, not in its text.
                egui::Shape::Text(t) if t.galley.text().starts_with(first) => {
                    Some(t.galley.rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.chr)).collect())
                }
                _ => None,
            })
            .expect("name painted")
    }

    #[test]
    fn a_column_sized_to_fit_shows_its_longest_cell_and_its_header_whole() {
        let long = "Presentacion final del proyecto de arquitectura 2026.pptx";
        let view = listed(&["a.txt", long]);
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let name = fit_width(&ctx, &view, 0, 0);
        assert_eq!(painted_name(&view, name, long), long, "whole at its fit width ({name})");
        assert_ne!(painted_name(&view, name - 20.0, long), long, "a little narrower cuts it");
        // Folders holds only "--": the header sets its width.
        let font = egui::TextStyle::Body.resolve(&ctx.global_style());
        let header = ctx.fonts_mut(|f| f.layout_no_wrap("Folders".into(), font, egui::Color32::WHITE).size().x);
        let folders = fit_width(&ctx, &view, 0, 3);
        assert!(folders >= header && folders < header + 40.0, "{folders} for a {header} header");
        assert!(fit_width(&ctx, &view, MAX_ZOOM, 0) > name, "a bigger icon needs more room");
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
                show(ui, &view, &[], Sizes::default(), None, Some(&mut edit), &FileIcons::default());
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
