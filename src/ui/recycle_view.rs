//! The Recycle Bin view (Windows): one row per recycled entry, with the name
//! it had, where it came from, when it was deleted and its size. Drawn instead
//! of the file list, with the same headers: click to sort, borders to resize,
//! a menu to fit.

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::format::{format_size, format_time};
use crate::recycle_bin::Recycled;
use crate::ui::{file_list, icons};

/// A column of the view, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecycleColumn {
    Name,
    Location,
    Deleted,
    Size,
}

/// Header labels and the column each one sorts, in display order.
pub const COLUMNS: [(&str, RecycleColumn); 4] = [
    ("Name", RecycleColumn::Name),
    ("Original Location", RecycleColumn::Location),
    ("Date Deleted", RecycleColumn::Deleted),
    ("Size", RecycleColumn::Size),
];

/// Each column's width in points. The shared helpers (borders, Fit All Columns)
/// work on the list's six columns: these four are its first four, which keeps
/// the list's priorities (Name, then the two after it, then the small one), and
/// the last two are unused.
pub type Widths = file_list::Widths;

/// Each column's role, in the list's six: Original Location is the long text that
/// gives first, Date Deleted keeps priority, Size is a number.
pub const ROLES: file_list::Roles = {
    use file_list::Role::*;
    [Name, Text, Short, Numeric, Short, Short]
};

/// A fingerprint of what the rows show, to tell when the columns need fitting again.
pub fn signature(rows: &[Recycled]) -> u64 {
    file_list::signature(rows.iter().map(cells))
}

/// The columns the view shows, in the list's six.
pub const SHOWN: file_list::Shown = [true, true, true, true, false, false];

/// The widths the view starts with.
pub const DEFAULT_WIDTHS: Widths = [260.0, 260.0, 150.0, 90.0, 0.0, 0.0];

/// How the entries are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecycleSort {
    pub column: RecycleColumn,
    pub descending: bool,
}

impl RecycleSort {
    /// What the view starts with: the latest deleted first.
    pub const NEWEST: Self = Self { column: RecycleColumn::Deleted, descending: true };

    /// A header click: the sorted column flips direction, another starts ascending.
    pub fn clicked(current: Option<Self>, column: RecycleColumn) -> Self {
        Self { column, descending: current.is_some_and(|sort| sort.column == column && !sort.descending) }
    }
}

/// Orders `rows` by `sort`; ties keep the order of their names in the bin.
/// An entry with no known date sorts before the others.
pub fn sort(rows: &mut [Recycled], sort: Option<RecycleSort>) {
    let Some(RecycleSort { column, descending }) = sort else { return };
    rows.sort_by(|a, b| {
        let by_column = match column {
            RecycleColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            RecycleColumn::Location => a.location.to_lowercase().cmp(&b.location.to_lowercase()),
            RecycleColumn::Deleted => a.deleted.cmp(&b.deleted),
            RecycleColumn::Size => a.size.cmp(&b.size),
        };
        let by_column = if descending { by_column.reverse() } else { by_column };
        by_column.then_with(|| a.data.cmp(&b.data))
    });
}

/// The text of each cell of a row. A folder's size is blank when the record has none.
pub fn cells(item: &Recycled) -> [String; 4] {
    let size = if item.is_dir && item.size == 0 { String::new() } else { format_size(item.size) };
    [item.name.clone(), item.location.clone(), item.deleted.map(format_time).unwrap_or_default(), size]
}

/// The entries picked in the view, found by where their content lies in the bin.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Picks {
    picked: Vec<PathBuf>,
    cursor: Option<PathBuf>,
    anchor: Option<PathBuf>,
}

impl Picks {
    pub fn is_picked(&self, item: &Recycled) -> bool {
        self.picked.contains(&item.data)
    }

    pub fn count(&self) -> usize {
        self.picked.len()
    }

    /// The picked entries, in the order of `rows`.
    pub fn of<'a>(&self, rows: &'a [Recycled]) -> Vec<&'a Recycled> {
        rows.iter().filter(|row| self.is_picked(row)).collect()
    }

    /// The row the cursor is on, if it is still listed.
    pub fn cursor_row(&self, rows: &[Recycled]) -> Option<usize> {
        position(rows, self.cursor.as_deref())
    }

    /// A click: only this row.
    pub fn select(&mut self, rows: &[Recycled], row: usize) {
        let Some(id) = rows.get(row).map(|r| r.data.clone()) else { return };
        self.picked = vec![id.clone()];
        self.cursor = Some(id.clone());
        self.anchor = Some(id);
    }

    /// Ctrl+click: this row joins the picks, or leaves them.
    pub fn toggle(&mut self, rows: &[Recycled], row: usize) {
        let Some(id) = rows.get(row).map(|r| r.data.clone()) else { return };
        match self.picked.iter().position(|p| *p == id) {
            Some(at) => {
                self.picked.remove(at);
            }
            None => self.picked.push(id.clone()),
        }
        self.cursor = Some(id.clone());
        self.anchor = Some(id);
    }

    /// Shift+click: the rows from the anchor to this one.
    pub fn extend(&mut self, rows: &[Recycled], row: usize) {
        let Some(id) = rows.get(row).map(|r| r.data.clone()) else { return };
        let from = position(rows, self.anchor.as_deref()).or_else(|| self.cursor_row(rows)).unwrap_or(row);
        if self.anchor.is_none() {
            self.anchor = Some(rows[from].data.clone());
        }
        self.picked = rows[from.min(row)..=from.max(row)].iter().map(|r| r.data.clone()).collect();
        self.cursor = Some(id);
    }

    /// Ctrl+A.
    pub fn select_all(&mut self, rows: &[Recycled]) {
        self.picked = rows.iter().map(|r| r.data.clone()).collect();
        if self.cursor_row(rows).is_none() {
            self.cursor = rows.first().map(|r| r.data.clone());
            self.anchor = self.cursor.clone();
        }
    }

    /// An arrow: the cursor moves `delta` rows (the first or last row with none yet),
    /// alone or, with `extend`, taking the rows between it and the anchor.
    pub fn step(&mut self, rows: &[Recycled], delta: isize, extend: bool) {
        let Some(last) = rows.len().checked_sub(1) else { return };
        let to = match self.cursor_row(rows) {
            Some(at) => at.saturating_add_signed(delta).min(last),
            None if delta < 0 => last,
            None => 0,
        };
        if extend {
            self.extend(rows, to);
        } else {
            self.select(rows, to);
        }
    }

    /// Home or End: the first or the last row.
    pub fn edge(&mut self, rows: &[Recycled], last: bool) {
        self.select(rows, if last { rows.len().saturating_sub(1) } else { 0 });
    }

    /// Drops what is no longer in `rows` (after the bin was read again).
    pub fn keep(&mut self, rows: &[Recycled]) {
        self.picked.retain(|id| position(rows, Some(id)).is_some());
        for id in [&mut self.cursor, &mut self.anchor] {
            if position(rows, id.as_deref()).is_none() {
                *id = None;
            }
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The row of `rows` whose content is `id`.
fn position(rows: &[Recycled], id: Option<&Path>) -> Option<usize> {
    let id = id?;
    rows.iter().position(|row| row.data == id)
}

/// The width that shows every cell of `column` and its header whole.
pub fn fit_width(ctx: &egui::Context, rows: &[Recycled], column: usize) -> f32 {
    let measure = file_list::body_measure(ctx);
    let style = ctx.global_style();
    let gap = style.spacing.item_spacing.x;
    let icon = ctx.fonts_mut(|fonts| fonts.row_height(&egui::TextStyle::Body.resolve(&style)));
    let widest = rows
        .iter()
        .map(|row| {
            let text = measure(cells(row)[column].clone());
            if column == 0 { icon + gap + text } else { text }
        })
        .fold(file_list::header_width(ctx, COLUMNS[column].0), f32::max);
    file_list::fitted(widest)
}

/// What the view asks of the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecycleEvent {
    /// A click: only this row.
    Select(usize),
    /// Ctrl+click.
    Toggle(usize),
    /// Shift+click.
    Extend(usize),
    /// Right click: the row's menu.
    Menu(usize),
    /// A click on a header: sort by the column.
    Sort(RecycleColumn),
    /// Right click on a header: the column's menu.
    HeaderMenu(usize),
    /// A column border was dragged and let go: the new widths.
    Resized(Widths),
    /// A double click on a column's right border: fit the column to its content.
    Fit(usize),
}

/// What the view draws besides the rows.
pub struct Look<'a> {
    pub picks: &'a Picks,
    pub sort: Option<RecycleSort>,
    pub widths: &'a Widths,
    /// The bin is being read: nothing to show yet.
    pub loading: bool,
    /// A row to bring into view.
    pub scroll_to: Option<usize>,
}

/// Draws the entries as `look.sort` orders them (the rows come sorted).
pub fn show(ui: &mut egui::Ui, rows: &[Recycled], look: &Look) -> Option<RecycleEvent> {
    let mut event = None;
    if rows.is_empty() {
        ui.weak(if look.loading { "Reading the Recycle Bin…" } else { "The Recycle Bin is empty" });
        return None;
    }
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
    file_list::remember_width(ui.ctx(), ui.available_rect_before_wrap().width());
    let widths = file_list::live_widths(ui, look.widths);
    let mut cell_rects = [egui::Rect::NOTHING; file_list::COLUMNS.len()];
    egui::ScrollArea::horizontal().id_salt("lumen_recycle_columns").auto_shrink([false, true]).show(ui, |ui| {
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .auto_shrink([false, true])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::CLICK);
        if let Some(row) = look.scroll_to {
            table = table.scroll_to_row(row, None);
        }
        // Every column has its own width, like the list's; the last stretches over the room left.
        for &width in &widths[..COLUMNS.len() - 1] {
            table = table.column(Column::exact(width).clip(true));
        }
        table
            .column(Column::remainder().at_least(widths[COLUMNS.len() - 1]).clip(true))
            .header(row_height, |mut header| {
                for (i, (label, column)) in COLUMNS.into_iter().enumerate() {
                    header.col(|ui| {
                        cell_rects[i] = ui.max_rect();
                        let direction = look.sort.filter(|sort| sort.column == column).map(|sort| sort.descending);
                        match file_list::header_cell(ui, i, label, direction) {
                            Some(file_list::HeaderClick::Sort) => event = Some(RecycleEvent::Sort(column)),
                            Some(file_list::HeaderClick::Menu) => event = Some(RecycleEvent::HeaderMenu(i)),
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
                    let item = &rows[i];
                    let texts = cells(item);
                    row.set_selected(look.picks.is_picked(item));
                    row.col(|ui| {
                        if item.is_dir {
                            icons::folder(ui);
                        } else {
                            icons::file(ui);
                        }
                        // Not selectable, or the text would take the row's click.
                        ui.add(egui::Label::new(&texts[0]).selectable(false).truncate());
                    });
                    for (text, role) in texts[1..].iter().zip(&ROLES[1..]) {
                        row.col(|ui| match role {
                            file_list::Role::Numeric => file_list::number_cell(ui, text),
                            _ => {
                                ui.add(egui::Label::new(text).selectable(false).truncate());
                            }
                        });
                    }
                    let response = row.response();
                    if response.secondary_clicked() {
                        event = Some(RecycleEvent::Menu(i));
                    } else if response.clicked() {
                        let modifiers = response.ctx.input(|input| input.modifiers);
                        event = Some(if modifiers.command {
                            RecycleEvent::Toggle(i)
                        } else if modifiers.shift {
                            RecycleEvent::Extend(i)
                        } else {
                            RecycleEvent::Select(i)
                        });
                    }
                });
            });
        match file_list::column_borders(ui, &cell_rects, &widths) {
            Some(file_list::BorderEvent::Resized(resized)) => event = Some(RecycleEvent::Resized(resized)),
            Some(file_list::BorderEvent::Fit(column)) => event = Some(RecycleEvent::Fit(column)),
            None => {}
        }
    });
    event
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn at(unix: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(unix))
    }

    fn item(id: &str, original: &str, deleted: Option<u64>, size: u64, is_dir: bool) -> Recycled {
        let (location, name) = crate::recycle_bin::split_original(original);
        let bin = PathBuf::from("/bin");
        Recycled {
            info: bin.join(format!("$I{id}")),
            data: bin.join(format!("$R{id}")),
            original: PathBuf::from(original),
            name: name.to_owned(),
            location: location.to_owned(),
            size,
            deleted: deleted.and_then(at),
            is_dir,
        }
    }

    /// Three entries every column orders differently.
    fn three() -> Vec<Recycled> {
        vec![
            item("A", "C:\\Users\\Ana\\beta.txt", Some(1_600_000_000), 300, false),
            item("B", "D:\\Work\\Alpha.png", Some(1_700_000_000), 20, false),
            item("C", "C:\\Docs\\charlie", Some(1_650_000_000), 5000, true),
        ]
    }

    fn ids(rows: &[Recycled]) -> String {
        rows.iter().map(|r| r.data.file_name().unwrap().to_string_lossy().trim_start_matches("$R").to_owned()).collect()
    }

    fn sorted(rows: &[Recycled], column: RecycleColumn, descending: bool) -> String {
        let mut rows = rows.to_vec();
        sort(&mut rows, Some(RecycleSort { column, descending }));
        ids(&rows)
    }

    #[test]
    fn a_row_reads_with_its_original_name_place_date_and_size() {
        let rows = three();
        let date = crate::format::format_time(rows[0].deleted.unwrap());
        assert_eq!(cells(&rows[0]), ["beta.txt".to_owned(), "C:\\Users\\Ana".into(), date, "300 B".into()]);
        assert_eq!(cells(&rows[1])[3], "20 B");
    }

    #[test]
    fn a_folder_shows_its_size_only_when_the_record_has_one_and_an_unknown_date_is_blank() {
        let folder = item("F", "C:\\Docs\\empty", None, 0, true);
        assert_eq!(cells(&folder), ["empty".to_owned(), "C:\\Docs".into(), String::new(), String::new()]);
        assert_eq!(cells(&three()[2])[3], "4.9 KB");
        assert_eq!(cells(&item("Z", "C:\\Docs\\zero.txt", None, 0, false))[3], "0 B", "an empty file is 0 B");
    }

    #[test]
    fn entries_sort_by_each_column_and_descending_reverses_it() {
        let rows = three();
        use RecycleColumn::*;
        for (column, asc, desc) in [(Name, "BAC", "CAB"), (Location, "CAB", "BAC"), (Deleted, "ACB", "BCA"), (Size, "BAC", "CAB")] {
            assert_eq!(sorted(&rows, column, false), asc, "{column:?} ascending");
            assert_eq!(sorted(&rows, column, true), desc, "{column:?} descending");
        }
    }

    #[test]
    fn ties_keep_their_order_and_names_sort_ignoring_case_and_unknown_dates_come_first() {
        let mut rows = vec![
            item("1", "C:\\x\\b.txt", Some(5), 1, false),
            item("2", "C:\\x\\B.txt", Some(5), 1, false),
            item("3", "C:\\x\\a.txt", None, 1, false),
        ];
        sort(&mut rows, Some(RecycleSort { column: RecycleColumn::Name, descending: false }));
        assert_eq!(ids(&rows), "312", "a, then b and B tied: their order in the bin");
        sort(&mut rows, Some(RecycleSort { column: RecycleColumn::Deleted, descending: false }));
        assert_eq!(ids(&rows), "312", "the one with no date, then the tied ones");
        sort(&mut rows, Some(RecycleSort { column: RecycleColumn::Deleted, descending: true }));
        assert_eq!(ids(&rows), "123", "descending: the unknown date last, the tie still in bin order");
        let mut unsorted = three();
        sort(&mut unsorted, None);
        assert_eq!(ids(&unsorted), "ABC", "left as they are");
    }

    #[test]
    fn a_header_click_starts_ascending_and_the_same_column_again_reverses_it() {
        use RecycleColumn::*;
        let asc = |column| RecycleSort { column, descending: false };
        assert_eq!(RecycleSort::clicked(None, Size), asc(Size));
        assert_eq!(RecycleSort::clicked(Some(asc(Size)), Size), RecycleSort { column: Size, descending: true });
        assert_eq!(RecycleSort::clicked(Some(RecycleSort { column: Size, descending: true }), Size), asc(Size));
        assert_eq!(RecycleSort::clicked(Some(asc(Size)), Name), asc(Name), "another column starts ascending");
        assert_eq!(RecycleSort::NEWEST, RecycleSort { column: Deleted, descending: true });
    }

    fn picked(picks: &Picks, rows: &[Recycled]) -> String {
        ids(&picks.of(rows).into_iter().cloned().collect::<Vec<_>>())
    }

    fn five() -> Vec<Recycled> {
        ["1", "2", "3", "4", "5"].iter().map(|id| item(id, &format!("C:\\x\\{id}.txt"), Some(1), 1, false)).collect()
    }

    #[test]
    fn a_click_picks_one_row_ctrl_click_adds_and_removes_and_shift_click_takes_the_range() {
        let rows = five();
        let mut picks = Picks::default();
        picks.select(&rows, 1);
        assert_eq!((picked(&picks, &rows), picks.cursor_row(&rows), picks.count()), ("2".into(), Some(1), 1));
        picks.toggle(&rows, 3);
        assert_eq!(picked(&picks, &rows), "24");
        picks.toggle(&rows, 1);
        assert_eq!((picked(&picks, &rows), picks.cursor_row(&rows)), ("4".into(), Some(1)), "2 left, and the cursor is where the click was");
        picks.select(&rows, 2);
        picks.extend(&rows, 4);
        assert_eq!(picked(&picks, &rows), "345", "from the anchor to the click");
        picks.extend(&rows, 0);
        assert_eq!(picked(&picks, &rows), "123", "the anchor stays: back over it, the other way");
        picks.select(&rows, 4);
        assert_eq!(picked(&picks, &rows), "5", "a plain click starts over");
        assert!(picks.is_picked(&rows[4]) && !picks.is_picked(&rows[3]));
    }

    #[test]
    fn shift_click_with_nothing_picked_picks_just_that_row_and_out_of_range_rows_do_nothing() {
        let rows = five();
        let mut picks = Picks::default();
        picks.extend(&rows, 2);
        assert_eq!(picked(&picks, &rows), "3");
        picks.select(&rows, 99);
        picks.toggle(&rows, 99);
        picks.extend(&rows, 99);
        assert_eq!(picked(&picks, &rows), "3");
    }

    #[test]
    fn the_arrows_move_the_cursor_and_shift_arrows_extend_from_the_anchor() {
        let rows = five();
        let mut picks = Picks::default();
        picks.step(&rows, 1, false);
        assert_eq!((picked(&picks, &rows), picks.cursor_row(&rows)), ("1".into(), Some(0)), "down with none: the first");
        picks.step(&rows, 2, false);
        assert_eq!(picked(&picks, &rows), "3");
        picks.step(&rows, 99, false);
        assert_eq!(picked(&picks, &rows), "5", "stops at the last");
        picks.step(&rows, -1, true);
        picks.step(&rows, -1, true);
        assert_eq!(picked(&picks, &rows), "345", "the anchor is where the arrows started");
        picks.step(&rows, 1, true);
        assert_eq!(picked(&picks, &rows), "45");
        picks.edge(&rows, false);
        assert_eq!(picked(&picks, &rows), "1");
        picks.edge(&rows, true);
        assert_eq!(picked(&picks, &rows), "5");
        let mut fresh = Picks::default();
        fresh.step(&rows, -1, false);
        assert_eq!(picked(&fresh, &rows), "5", "up with none: the last");
        let mut none = Picks::default();
        none.step(&[], 1, false);
        none.edge(&[], true);
        assert_eq!(none.count(), 0);
    }

    #[test]
    fn select_all_picks_every_row_clear_drops_them_and_keep_forgets_what_left_the_bin() {
        let rows = five();
        let mut picks = Picks::default();
        picks.select_all(&rows);
        assert_eq!(picked(&picks, &rows), "12345");
        picks.keep(&rows[1..4]);
        assert_eq!(picked(&picks, &rows), "234");
        assert_eq!(picks.cursor_row(&rows[3..]), None, "a cursor no longer listed is no row");
        picks.clear();
        assert_eq!((picks.count(), picks.cursor_row(&rows)), (0, None));
    }

    type Frame = (Option<RecycleEvent>, Vec<egui::epaint::ClippedShape>);

    struct Scene {
        picks: Picks,
        sort: Option<RecycleSort>,
        widths: Widths,
        loading: bool,
    }

    impl Default for Scene {
        fn default() -> Self {
            Self { picks: Picks::default(), sort: None, widths: DEFAULT_WIDTHS, loading: false }
        }
    }

    fn draw(ctx: &egui::Context, rows: &[Recycled], scene: &Scene, events: Vec<egui::Event>, modifiers: egui::Modifiers, time: Option<f64>) -> Frame {
        let mut event = None;
        let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
        let input = egui::RawInput { events, time, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| {
                let look = Look { picks: &scene.picks, sort: scene.sort, widths: &scene.widths, loading: scene.loading, scroll_to: None };
                event = show(ui, rows, &look);
            });
        });
        output.textures_delta.clear();
        (event, output.shapes)
    }

    fn settled(ctx: &egui::Context, rows: &[Recycled], scene: &Scene) -> Vec<egui::epaint::ClippedShape> {
        draw(ctx, rows, scene, vec![], egui::Modifiers::NONE, None);
        draw(ctx, rows, scene, vec![], egui::Modifiers::NONE, None).1
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

    fn click(at: egui::Pos2, button: egui::PointerButton, modifiers: egui::Modifiers) -> Vec<egui::Event> {
        let press = |pressed| egui::Event::PointerButton { pos: at, button, pressed, modifiers };
        vec![egui::Event::PointerMoved(at), press(true), press(false)]
    }

    #[test]
    fn the_view_has_its_four_columns_and_shows_each_entry_by_its_original_name() {
        let ctx = egui::Context::default();
        let rows = three();
        let shapes = settled(&ctx, &rows, &Scene::default());
        for text in COLUMNS.map(|(label, _)| label) {
            text_rect(&shapes, text);
        }
        for row in &rows {
            for text in cells(row) {
                if !text.is_empty() {
                    text_rect(&shapes, &text);
                }
            }
        }
    }

    #[test]
    fn a_click_selects_ctrl_toggles_shift_extends_and_a_right_click_asks_for_the_menu() {
        let ctx = egui::Context::default();
        let rows = three();
        let scene = Scene::default();
        let shapes = settled(&ctx, &rows, &scene);
        let (a, b) = (text_rect(&shapes, "beta.txt").center(), text_rect(&shapes, "Alpha.png").center());
        let event = |at, button, modifiers| {
            let event = draw(&ctx, &rows, &scene, click(at, button, modifiers), modifiers, None).0;
            settled(&ctx, &rows, &scene);
            event
        };
        use egui::{Modifiers as M, PointerButton as B};
        assert_eq!(event(b, B::Primary, M::NONE), Some(RecycleEvent::Select(1)));
        assert_eq!(event(a, B::Primary, M::COMMAND), Some(RecycleEvent::Toggle(0)));
        assert_eq!(event(a, B::Primary, M::SHIFT), Some(RecycleEvent::Extend(0)));
        assert_eq!(event(a, B::Secondary, M::NONE), Some(RecycleEvent::Menu(0)));
    }

    #[test]
    fn a_click_on_any_cell_of_the_row_selects_it_and_a_double_click_opens_nothing() {
        let ctx = egui::Context::default();
        let rows = three();
        let scene = Scene::default();
        let shapes = settled(&ctx, &rows, &scene);
        let name = text_rect(&shapes, "beta.txt");
        for text in ["C:\\Users\\Ana", "300 B"] {
            let at = egui::pos2(text_rect(&shapes, text).center().x, name.center().y);
            let event = draw(&ctx, &rows, &scene, click(at, egui::PointerButton::Primary, egui::Modifiers::NONE), egui::Modifiers::NONE, None).0;
            assert_eq!(event, Some(RecycleEvent::Select(0)), "{text}");
            settled(&ctx, &rows, &scene);
        }
        let at = name.center();
        draw(&ctx, &rows, &scene, click(at, egui::PointerButton::Primary, egui::Modifiers::NONE), egui::Modifiers::NONE, Some(10.0));
        let second = draw(&ctx, &rows, &scene, click(at, egui::PointerButton::Primary, egui::Modifiers::NONE), egui::Modifiers::NONE, Some(10.1)).0;
        assert_eq!(second, Some(RecycleEvent::Select(0)), "just a selection");
    }

    #[test]
    fn picked_rows_are_drawn_apart_from_the_others() {
        let ctx = egui::Context::default();
        let rows = three();
        let plain = Scene::default();
        let mut scene = Scene::default();
        scene.picks.select(&rows, 1);
        let rects = |shapes: Vec<egui::epaint::ClippedShape>| shapes.into_iter().filter(|s| matches!(s.shape, egui::Shape::Rect(_))).collect::<Vec<_>>();
        assert_ne!(rects(settled(&ctx, &rows, &scene)), rects(settled(&ctx, &rows, &plain)), "the selection paints a row");
    }

    #[test]
    fn nothing_to_show_says_so_and_a_bin_being_read_says_that() {
        let ctx = egui::Context::default();
        let (event, shapes) = draw(&ctx, &[], &Scene::default(), vec![], egui::Modifiers::NONE, None);
        assert_eq!(event, None);
        text_rect(&shapes, "The Recycle Bin is empty");
        let scene = Scene { loading: true, ..Scene::default() };
        text_rect(&draw(&ctx, &[], &scene, vec![], egui::Modifiers::NONE, None).1, "Reading the Recycle Bin…");
    }

    #[test]
    fn a_column_fits_its_widest_cell_or_its_header_whole_points_and_never_under_the_minimum() {
        let ctx = egui::Context::default();
        let mut rows = three();
        draw(&ctx, &rows, &Scene::default(), vec![], egui::Modifiers::NONE, None);
        let measure = file_list::body_measure(&ctx);
        for (column, (label, _)) in COLUMNS.into_iter().enumerate() {
            let width = fit_width(&ctx, &rows, column);
            assert!(width >= file_list::header_width(&ctx, label), "{column}: the header fits");
            assert!(width >= file_list::MIN_WIDTH && width.fract() == 0.0, "{column}: {width}");
        }
        for row in &rows {
            let text = cells(row);
            for (column, cell) in text.iter().enumerate().skip(1) {
                assert!(fit_width(&ctx, &rows, column) >= measure(cell.clone()), "{column}");
            }
            assert!(fit_width(&ctx, &rows, 0) >= measure(text[0].clone()), "and the name with its icon");
        }
        let before = (fit_width(&ctx, &rows, 0), fit_width(&ctx, &rows, 1));
        rows[1].name = "A file with a really quite long name indeed.png".into();
        assert!(fit_width(&ctx, &rows, 0) > before.0 + 100.0, "a longer name needs a wider column");
        assert_eq!(fit_width(&ctx, &rows, 1), before.1, "the others do not care");
    }

    #[test]
    fn a_click_on_a_header_asks_to_sort_by_its_column_a_right_click_for_its_menu() {
        let ctx = egui::Context::default();
        let rows = three();
        let scene = Scene::default();
        let shapes = settled(&ctx, &rows, &scene);
        let deleted = text_rect(&shapes, "Date Deleted");
        let none = egui::Modifiers::NONE;
        assert_eq!(draw(&ctx, &rows, &scene, click(deleted.center(), egui::PointerButton::Primary, none), none, None).0, Some(RecycleEvent::Sort(RecycleColumn::Deleted)));
        settled(&ctx, &rows, &scene);
        let place = text_rect(&shapes, "Original Location").center();
        assert_eq!(draw(&ctx, &rows, &scene, click(place, egui::PointerButton::Secondary, none), none, None).0, Some(RecycleEvent::HeaderMenu(1)));
    }

    #[test]
    fn the_sorted_header_shows_the_lists_arrow_and_the_others_none() {
        let ctx = egui::Context::default();
        let rows = three();
        let arrows = |sort: Option<RecycleSort>| {
            let scene = Scene { sort, ..Scene::default() };
            settled(&ctx, &rows, &scene)
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text().contains(['⏶', '⏷']) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(arrows(None).is_empty());
        let size = |descending| Some(RecycleSort { column: RecycleColumn::Size, descending });
        assert_eq!(arrows(size(false)), [format!("Size {}", file_list::SORT_ASCENDING)]);
        assert_eq!(arrows(size(true)), [format!("Size {}", file_list::SORT_DESCENDING)]);
    }

    #[test]
    fn the_columns_are_as_wide_as_they_are_told() {
        let ctx = egui::Context::default();
        let rows = three();
        let left_of = |widths: Widths, header: &str| {
            let scene = Scene { widths, ..Scene::default() };
            settled(&ctx, &rows, &scene);
            text_rect(&settled(&ctx, &rows, &scene), header).min.x
        };
        let normal = left_of(DEFAULT_WIDTHS, "Original Location");
        let wide = left_of([400.0, 300.0, 150.0, 90.0, 0.0, 0.0], "Original Location");
        assert!((wide - normal - 140.0).abs() < 1.5, "Name is 140 wider: {normal} {wide}");
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn dragging_a_header_border_resizes_its_column_and_reports_the_widths_once_let_go() {
        let ctx = egui::Context::default();
        let rows = three();
        let scene = Scene::default();
        let shapes = settled(&ctx, &rows, &scene);
        let place = text_rect(&shapes, "Original Location");
        let border = egui::pos2(place.min.x - 8.0, place.center().y);
        let frame = |events| draw(&ctx, &rows, &scene, events, egui::Modifiers::NONE, None).0;
        frame(vec![egui::Event::PointerMoved(border)]);
        frame(vec![button(border, true)]);
        for step in 1..=5 {
            assert_eq!(frame(vec![egui::Event::PointerMoved(border + egui::vec2(10.0 * step as f32, 0.0))]), None, "nothing reported while dragging");
        }
        let released = frame(vec![button(border + egui::vec2(50.0, 0.0), false)]);
        let Some(RecycleEvent::Resized(new)) = released else { panic!("the widths are reported when let go: {released:?}") };
        assert!((new[0] - 310.0).abs() < 1.5, "Name is 50 wider: {new:?}");
        assert_eq!(new[1..], DEFAULT_WIDTHS[1..], "only the dragged column changes");
    }

    #[test]
    fn the_entry_under_the_pointer_is_lit_in_a_translucent_cyan() {
        let ctx = egui::Context::default();
        let rows = three();
        let scene = Scene::default();
        let shapes = settled(&ctx, &rows, &scene);
        let at = text_rect(&shapes, "300 B").center();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = draw(&ctx, &rows, &scene, vec![egui::Event::PointerMoved(at)], egui::Modifiers::NONE, None).1;
        }
        assert!(file_list::tests::hover_lit(&ctx, &shapes, at));
    }

    #[test]
    fn faint_separators_run_between_the_four_columns() {
        let ctx = egui::Context::default();
        let rows = three();
        let shapes = settled(&ctx, &rows, &Scene::default());
        let lines = file_list::tests::separators(&ctx, &shapes);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].0 < text_rect(&shapes, "Original Location").min.x, "the first one between Name and Original Location");
    }

    #[test]
    fn a_double_click_on_a_header_border_fits_the_column_on_its_left() {
        let ctx = egui::Context::default();
        let rows = three();
        let scene = Scene::default();
        let shapes = settled(&ctx, &rows, &scene);
        let place = text_rect(&shapes, "Original Location");
        let border = egui::pos2(place.min.x - 8.0, place.center().y);
        let frame = |time| draw(&ctx, &rows, &scene, click(border, egui::PointerButton::Primary, egui::Modifiers::NONE), egui::Modifiers::NONE, Some(time)).0;
        assert_eq!(frame(10.0), None, "one click neither sorts nor fits");
        assert_eq!(frame(10.1), Some(RecycleEvent::Fit(0)), "Name, the column on the border's left");
    }

    fn fitted_in(content: [f32; 4], room: f32) -> Widths {
        let content = [content[0], content[1], content[2], content[3], 0.0, 0.0];
        file_list::fit_all(&content, &SHOWN, &ROLES, room)
    }

    /// L18 (134426): Name cut, Original Location very wide, Date Deleted and Size cut.
    #[test]
    fn when_the_bin_does_not_fit_original_location_gives_first_and_date_size_and_name_stay_whole() {
        let fitted = fitted_in([520.0, 640.0, 130.0, 70.0], 1000.0);
        assert_eq!(fitted, [520.0, 280.0, 130.0, 70.0, 0.0, 0.0], "Original Location takes what is left");
    }

    #[test]
    fn the_bin_squeezes_location_to_its_floor_then_size_then_name_and_date_last() {
        let fitted = fitted_in([520.0, 640.0, 130.0, 70.0], 700.0);
        assert_eq!(fitted, [406.0, file_list::TEXT_MIN, 130.0, file_list::SMALL_MIN, 0.0, 0.0], "Location at its floor, Size at 44, Name gives next");
        let narrow = fitted_in([520.0, 640.0, 130.0, 70.0], 400.0);
        assert!((narrow[..4].iter().sum::<f32>() - 400.0).abs() < 0.5, "{narrow:?}");
        assert!(narrow[0] >= file_list::NAME_MIN && narrow[0] < file_list::NAME_MIN + 3.0, "Name at its minimum: {narrow:?}");
        assert!(narrow[1] < file_list::TEXT_MIN && narrow[2] < 130.0, "Location and Date give at the end: {narrow:?}");
        assert!(narrow[..4].iter().all(|&w| w >= file_list::MIN_WIDTH), "{narrow:?}");
        assert_eq!(narrow[4..], [0.0, 0.0], "the unused columns stay out");
    }

    #[test]
    fn when_the_bin_fits_the_spare_room_goes_to_every_column_not_all_to_location_or_name() {
        let fitted = fitted_in([300.0, 700.0, 140.0, 70.0], 1300.0);
        assert!((fitted[..4].iter().sum::<f32>() - 1300.0).abs() < 0.5, "{fitted:?}");
        assert!(fitted[1] <= 700.0 + 90.0 * 700.0 / 1210.0 + 1.0, "Location: its content plus its share: {fitted:?}");
        assert!(fitted[0] > 320.0 && fitted[2] > 145.0 && fitted[3] > 72.0, "the others grow too: {fitted:?}");
    }

    #[test]
    fn size_is_right_aligned_and_the_text_columns_left_aligned() {
        let ctx = egui::Context::default();
        let rows = three();
        let shapes = settled(&ctx, &rows, &Scene::default());
        let right = crate::ui::testing::SCREEN.right();
        assert!(right - text_rect(&shapes, "300 B").max.x < 24.0, "Size ends at the table's edge: {}", text_rect(&shapes, "300 B").max.x);
        let place = text_rect(&shapes, "Original Location");
        assert!((text_rect(&shapes, "C:\\Users\\Ana").min.x - place.min.x).abs() < 6.0, "Original Location starts at its header");
    }
}
