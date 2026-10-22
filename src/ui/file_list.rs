//! The file table. Only visible rows are laid out, so huge folders stay fast.

use std::path::PathBuf;

use eframe::egui::{self, text::CCursorRange};
use egui_extras::{Column, TableBuilder};

use crate::app::{Scroll, SortColumn, View};
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
    /// A click on the folder's chevron (Tree): unfold it, or fold it.
    Unfold(usize),
    /// A band dragged from a row's empty part or below the rows, in row units
    /// (row `i` spans `[i, i + 1)`): sent on every frame the pointer drags it.
    /// `add`: Shift was held at the press, the rows join the selection.
    Band { start: f32, end: f32, add: bool },
    /// A band dragged from the space around the cards (List, Icons): the cells
    /// it touches and the one under the pointer; sent on every frame it moves.
    Cells { cells: Vec<usize>, cursor: Option<usize>, add: bool },
    /// A click below the rows; `menu` for a right click.
    Empty { menu: bool },
    Activate(usize),
    Sort(SortColumn),
    /// The in-place rename ended: `true` with Enter, `false` with Escape or a click elsewhere.
    RenameEnded(bool),
    /// A column border was dragged and let go: the new widths.
    Resized(Widths),
    /// A double click on a column's right border: fit the column to its content.
    Fit(usize),
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
pub(crate) const CUT_OPACITY: f32 = 0.5;

/// The icon's side in Details: one line of text.
pub fn icon_side(text_height: f32) -> f32 {
    text_height
}

/// A row is its icon plus a little air.
pub fn row_height(text_height: f32) -> f32 {
    icon_side(text_height) + 4.0
}

/// How big the list is drawn.
#[derive(Clone, Copy)]
pub struct Sizes {
    pub widths: Widths,
    pub shown: Shown,
    pub order: Order,
    /// The Tree view: a chevron before each folder, children indented under it.
    pub tree: bool,
}

impl Default for Sizes {
    fn default() -> Self {
        Self { widths: DEFAULT_WIDTHS, shown: ALL_SHOWN, order: DEFAULT_ORDER, tree: false }
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
    let View { rows, totals, selected, sort, .. } = view;
    let (selected, sort) = (*selected, *sort);
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let header_height = row_height(text_height);
    let (icon, row_height) = (icon_side(text_height), header_height);
    let mut event = None;
    // Set again below if the cursor row is drawn this frame.
    ui.ctx().data_mut(|d| d.remove::<egui::Rect>(cursor_row_id()));

    let widths = live_widths(ui, &sizes.widths);
    let mut cells = [egui::Rect::NOTHING; COLUMNS.len()];
    // From the rows: one drawn row's index and top, to turn a height on
    // screen into row units; and whether the button went down here on a
    // row's empty part (not its icon or name), where a band may start.
    let pitch = row_height + ui.spacing().item_spacing.y;
    let mut first_drawn: Option<(usize, f32)> = None;
    let mut band_may_start = false;
    // Whether a carried drag is over a folder row, which takes it (or refuses it) itself.
    let mut over_folder = false;
    let pressed = ui.input(|i| i.pointer.primary_pressed());
    let whole = ui.available_rect_before_wrap();
    remember_width(ui.ctx(), whole.width());
    focus_stop(ui, whole);

    egui::ScrollArea::horizontal().id_salt("lumen_list_columns").auto_shrink(false).show(ui, |ui| {
        let mut table = TableBuilder::new(ui)
            .striped(true)
            // The vertical scroll bar at the list's right edge, not after the last column.
            .auto_shrink([false, true])
            // Text sits in the middle of its row, beside the taller icon.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            // Not focusable: the list is one Tab stop.
            .sense(egui::Sense::CLICK | egui::Sense::DRAG);
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
                        match header_cell(ui, i, label, (sort.column == column).then_some(sort.descending)) {
                            Some(HeaderClick::Sort) => event = Some(ListEvent::Sort(column)),
                            Some(HeaderClick::Menu) => event = Some(ListEvent::HeaderMenu(i)),
                            Some(HeaderClick::Close) if i != 0 => event = Some(ListEvent::CloseColumn(i)),
                            _ => {}
                        }
                    });
                }
            })
            .body(|mut body| {
                band_scroll(body.ui_mut());
                let clip = body.ui_mut().clip_rect();
                body.rows(row_height, rows.len(), |mut row| {
                    let i = row.index();
                    let entry = view.entry(rows[i]);
                    let mut on_name = false;
                    // Where the folder's chevron is drawn (Tree).
                    let mut chevron = None;
                    row.set_selected(view.is_highlighted(i));
                    let mut row_rect = egui::Rect::NOTHING;
                    // Behind the row's content: where a folder taking a drop is lit.
                    let mut lit_slot = None;
                    let counts = count_cells(entry, totals.get(&entry.path));
                    for &column in &shown {
                        let (cell, _) = row.col(|ui| {
                            lit_slot.get_or_insert_with(|| ui.painter().add(egui::Shape::Noop));
                            match column {
                                0 => {
                                    // Icon and name on one line as tall as the row, centred in it:
                                    // a plain `horizontal` is one text line tall, so a taller icon
                                    // would hang below the row and be clipped (and the name with it).
                                    let line = egui::vec2(ui.available_width(), row_height);
                                    ui.allocate_ui_with_layout(line, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        // Cut and not pasted yet: faded, like other explorers.
                                        if cut.contains(&entry.path) {
                                            ui.set_opacity(CUT_OPACITY);
                                        }
                                        if sizes.tree {
                                            // A step per level; files keep the chevron's room so names line up.
                                            ui.add_space(view.depth(i) as f32 * (icon + ui.spacing().item_spacing.x));
                                            if entry.is_dir {
                                                let response = icons::chevron(ui, icon, view.is_open(i));
                                                chevron = Some(response.rect);
                                            } else {
                                                ui.allocate_space(egui::vec2(icon, icon));
                                            }
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
                            }
                        });
                        // Where the row is this frame: `row.response()` holds the last frame's place.
                        first_drawn.get_or_insert((i, cell.top()));
                        row_rect = row_rect.union(cell);
                    }
                    let response = row.response();
                    let over = crate::ui::dnd::over(&response.ctx, clip, row_rect, entry.is_dir.then_some(entry.path.as_path()));
                    over_folder |= over.inside && entry.is_dir;
                    if let (true, Some(slot)) = (over.lit, lit_slot) {
                        let painter = egui::Painter::new(response.ctx.clone(), response.layer_id, row_rect);
                        painter.set(slot, crate::ui::dnd::lit_shape(&response.ctx, row_rect));
                    }
                    // A selected row drags from anywhere on it; another one only
                    // from its icon and name, its empty part starts the band.
                    let grab = on_name || view.is_highlighted(i);
                    if pressed && response.is_pointer_button_down_on() && !grab {
                        band_may_start = true;
                    }
                    if grab && !edit.as_deref().is_some_and(|e| e.path == entry.path) {
                        crate::ui::dnd::start(&response, || crate::ui::dnd::carry(view, i));
                    }
                    if selected == Some(i) {
                        response.ctx.data_mut(|d| d.insert_temp(cursor_row_id(), response.rect));
                    }
                    if view.is_outlined(i) && row_rect.is_positive() {
                        let painter = egui::Painter::new(response.ctx.clone(), response.layer_id, row_rect.expand(1.0));
                        crate::ui::icon_grid::paint_dashed(&painter, row_rect.shrink(1.0), painter.ctx().global_style().visuals.weak_text_color());
                    }
                    // The chevron's column over the whole row's height.
                    let on_chevron = chevron.is_some_and(|c| {
                        let at = response.ctx.input(|input| input.pointer.interact_pos());
                        at.is_some_and(|at| egui::Rect::from_x_y_ranges(c.x_range(), response.rect.y_range()).contains(at))
                    });
                    if response.secondary_clicked() {
                        event = Some(ListEvent::Menu(i));
                    } else if on_chevron && (response.clicked() || response.double_clicked()) {
                        event = Some(ListEvent::Unfold(i));
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
        match column_borders(ui, &cells, &widths) {
            Some(BorderEvent::Resized(resized)) => event = Some(ListEvent::Resized(resized)),
            Some(BorderEvent::Fit(column)) => event = Some(ListEvent::Fit(column)),
            None => {}
        }
        // Everything under the header that is not a folder row (a file row, the
        // gaps between rows, the space below them) drops into the folder on screen.
        if !over_folder {
            let below_header = egui::Rect::from_min_max(egui::pos2(whole.min.x, cells[0].max.y), whole.max);
            crate::ui::dnd::background(ui, below_header, &view.current);
        }
        // Whatever space the rows leave below them.
        let rest = ui.available_rect_before_wrap();
        if event.is_none() && rest.height() > 0.0 {
            let response = ui.interact(rest, egui::Id::new("lumen_list_empty"), egui::Sense::CLICK);
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
    // On top of the rows: a ring tells the keys go to the list.
    if ui.memory(|m| m.has_focus(focus_id())) {
        ui.painter().rect_stroke(whole.shrink(1.0), 2, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
    }
    event
}

/// Where a band dragged over the rows started: in row units, so a scroll
/// does not move it, and the x on screen.
#[derive(Clone, Copy)]
struct BandStart {
    at: f32,
    x: f32,
    /// Shift was held at the press: the band adds to the selection.
    add: bool,
}

fn band_id() -> egui::Id {
    egui::Id::new("lumen_list_band")
}

/// A band held this close to the list's top or bottom edge, inside it,
/// scrolls the list: no need to leave the list, or the window, as the bottom
/// edge often sits by the window's.
const BAND_STRIP: f32 = 40.0;
/// Points per second at the edge itself and past it: the same at both ends.
const BAND_TOP_SPEED: f32 = 800.0;

/// How fast a band at height `y` scrolls a list showing `view` on screen, in
/// points per second: positive brings the rows above down. From nothing at
/// the inner side of each edge's strip to the top speed at the edge.
pub(crate) fn band_speed(y: f32, view: egui::Rangef) -> f32 {
    // The nearer edge decides, for lists too short for two strips.
    let (depth, sign) = if y < view.center() { (view.min + BAND_STRIP - y, 1.0) } else { (y - (view.max - BAND_STRIP), -1.0) };
    sign * BAND_TOP_SPEED * (depth / BAND_STRIP).clamp(0.0, 1.0)
}

/// While a band is dragged near or past the top or bottom of the rows on
/// view, scrolls the table's `body` towards the pointer.
fn band_scroll(body: &mut egui::Ui) {
    crate::ui::dnd::edge_scroll(body, false);
    let (dragging, pointer, dt) = body.input(|i| (i.pointer.is_decidedly_dragging(), i.pointer.latest_pos(), i.stable_dt));
    let banding = body.ctx().data(|d| d.get_temp::<BandStart>(band_id())).is_some();
    let Some(pointer) = pointer.filter(|_| dragging && banding) else { return };
    let speed = band_speed(pointer.y, body.clip_rect().y_range());
    if speed == 0.0 {
        return;
    }
    // At once: an animated scroll would carry on after the pointer is back.
    body.scroll_with_delta_animation(egui::vec2(0.0, speed * dt), egui::style::ScrollAnimation::none());
    // Keep scrolling while the pointer is held still.
    body.ctx().request_repaint();
}

/// The band dragged with the primary button: remembered from the press when
/// `may_start`, drawn and reported while the pointer drags it, forgotten once
/// the button is up. `unit` turns a height on screen into row units, `y` back.
fn band(ui: &egui::Ui, pressed: bool, may_start: bool, unit: impl Fn(f32) -> f32, y: impl Fn(f32) -> f32) -> Option<ListEvent> {
    let (origin, down, dragging, pointer, shift) = ui.input(|i| {
        let p = &i.pointer;
        (p.press_origin(), p.primary_down(), p.is_decidedly_dragging(), p.latest_pos(), i.modifiers.shift)
    });
    if pressed {
        let start = origin.filter(|_| may_start).map(|p| BandStart { at: unit(p.y), x: p.x, add: shift });
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
    Some(ListEvent::Band { start: start.at, end: unit(pointer.y), add: start.add })
}

/// The width that shows every cell of `column` and its header whole, for
/// `view`'s rows.
pub fn fit_width(ctx: &egui::Context, view: &View, column: usize) -> f32 {
    let style = ctx.global_style();
    let font = egui::TextStyle::Body.resolve(&style);
    let measure = body_measure(ctx);
    let header = header_width(ctx, COLUMNS[column].0);
    let icon = icon_side(ctx.fonts_mut(|f| f.row_height(&font))) + style.spacing.item_spacing.x;
    let widest = view
        .rows
        .iter()
        .map(|&row| {
            let entry = view.entry(row);
            match column {
                0 => icon + measure(entry.name.clone()),
                1 => measure(entry.kind().to_string()),
                2 => measure(entry.modified.map(format_time).unwrap_or_default()),
                _ => {
                    let cells = count_cells(entry, view.totals.get(&entry.path));
                    measure(cells[column - 3].clone())
                }
            }
        })
        .fold(header, f32::max);
    fitted(widest)
}

/// How wide `text` is in the body font.
pub fn body_measure(ctx: &egui::Context) -> impl Fn(String) -> f32 + '_ {
    let font = egui::TextStyle::Body.resolve(&ctx.global_style());
    move |text| ctx.fonts_mut(|f| f.layout_no_wrap(text, font.clone(), egui::Color32::WHITE).size().x)
}

/// The width a header `label` needs, with room for the sort arrow, which may show up later.
pub fn header_width(ctx: &egui::Context, label: &str) -> f32 {
    body_measure(ctx)(format!("{label} {SORT_DESCENDING}")) + 2.0 * ctx.global_style().spacing.button_padding.x
}

/// The column width for a `widest` cell: some air, whole points (the config keeps them so).
pub fn fitted(widest: f32) -> f32 {
    (widest + FIT_MARGIN).max(MIN_WIDTH).ceil()
}

/// Air left after the widest cell of a column sized to fit.
const FIT_MARGIN: f32 = 6.0;

/// Fit All Columns never takes Name below this (unless it is narrower to begin with).
pub const NAME_MIN: f32 = 120.0;

/// The width a long text column (Original Location) shrinks toward first.
pub const TEXT_MIN: f32 = 120.0;

/// The width Folders, Files and Size shrink toward when the columns do not fit:
/// about "Fol…" and some padding.
pub const SMALL_MIN: f32 = 44.0;

/// How wide the list was drawn, for Fit All Columns.
fn list_width_id() -> egui::Id {
    egui::Id::new("lumen_list_width")
}

/// Remembers the width the list has to draw its columns in.
pub fn remember_width(ctx: &egui::Context, width: f32) {
    ctx.data_mut(|d| d.insert_temp(list_width_id(), width));
}

/// The width the shown columns can share without scrolling sideways: the list's
/// last drawn width less the gaps between columns. `None` before the list was drawn.
pub fn visible_width(ctx: &egui::Context, shown: &Shown) -> Option<f32> {
    let width = ctx.data(|d| d.get_temp::<f32>(list_width_id()))?;
    Some(room(ctx, width, shown))
}

/// What of a table `width` wide the shown columns can share: less the gaps between them.
pub fn room(ctx: &egui::Context, width: f32, shown: &Shown) -> f32 {
    let gaps = shown.iter().enumerate().filter(|&(c, &on)| on || c == 0).count().saturating_sub(1);
    (width - gaps as f32 * ctx.global_style().spacing.item_spacing.x).floor().max(0.0)
}

/// What a column holds, which decides how it gives room when the columns do not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The first column: it takes what the others leave.
    Name,
    /// Long free text other than the name (Original Location): the first to be cut.
    Text,
    /// A short value that keeps priority (Type, Date Modified, File System, Percent Full):
    /// the last to be cut.
    Short,
    /// A small number (Folders, Files, Size, Free Space): squeezed before Name. This PC and
    /// the Recycle Bin right-align them; the list right-aligns its Size only.
    /// Short values like Size stay whole while the text columns can give.
    Numeric,
}

/// Each column's role, in `COLUMNS` order.
pub type Roles = [Role; COLUMNS.len()];

/// The list's columns: Name, Type, Date Modified, Folders, Files, Size.
pub const LIST_ROLES: Roles = [Role::Name, Role::Short, Role::Short, Role::Numeric, Role::Numeric, Role::Numeric];

/// The widths Fit All Columns gives: `content` is each column's fit width, and the
/// shown columns end up adding up to `available`. Hidden ones keep their `content`.
/// `roles` says who gives room, not the columns' places.
///
/// When every shown column fits, the spare room is shared over all of them in
/// proportion to their content (like File Pilot), so no column takes it all.
/// Otherwise the columns give in this order, each in proportion to what it can
/// give: long text (`TEXT_MIN`), numbers (`SMALL_MIN`), then Name (`NAME_MIN`), and
/// only then the text and the short columns, which keep priority, down to
/// `MIN_WIDTH`. Short values (Date, Size...) stay whole while the others can give.
pub fn fit_all(content: &Widths, shown: &Shown, roles: &Roles, available: f32) -> Widths {
    let name = roles.iter().position(|&r| r == Role::Name).unwrap_or(0);
    // Name is always shown.
    let columns: Vec<usize> = (0..COLUMNS.len()).filter(|&c| c == name || shown[c]).collect();
    let with = |wanted: &[Role]| -> Vec<usize> { columns.iter().copied().filter(|&c| c != name && wanted.contains(&roles[c])).collect() };
    let mut widths = *content;
    let total: f32 = columns.iter().map(|&c| content[c]).sum();
    if total <= available {
        let grown = available / total;
        for &c in &columns {
            widths[c] = content[c] * grown;
        }
    } else {
        let mut over = total - available;
        over = squeeze(&mut widths, &with(&[Role::Text]), TEXT_MIN, over);
        over = squeeze(&mut widths, &with(&[Role::Numeric]), SMALL_MIN, over);
        over = squeeze(&mut widths, &[name], NAME_MIN, over);
        squeeze(&mut widths, &with(&[Role::Text, Role::Short]), MIN_WIDTH, over);
    }
    // Whole points: the config keeps them so. Name takes what the others leave.
    let others: Vec<usize> = columns.iter().copied().filter(|&c| c != name).collect();
    for &c in &others {
        widths[c] = widths[c].floor();
    }
    widths[name] = (available.floor() - others.iter().map(|&c| widths[c]).sum::<f32>()).max(MIN_WIDTH);
    widths
}

/// Takes `over` points off `columns` of `widths`, each giving in proportion to
/// how far it is above `floor`; returns what they could not give.
fn squeeze(widths: &mut Widths, columns: &[usize], floor: f32, over: f32) -> f32 {
    let gives = |w: f32| w - w.min(floor);
    let can: f32 = columns.iter().map(|&c| gives(widths[c])).sum();
    let take = over.min(can);
    if take > 0.0 {
        for &c in columns {
            widths[c] -= gives(widths[c]) * take / can;
        }
    }
    over - take
}

/// A fingerprint of the text a view shows, to tell when its content changed.
pub fn signature(rows: impl IntoIterator<Item = impl std::hash::Hash>) -> u64 {
    use std::hash::{DefaultHasher, Hasher};
    let mut hasher = DefaultHasher::new();
    for row in rows {
        row.hash(&mut hasher);
    }
    hasher.finish()
}

/// A view with fixed columns (This PC, the Recycle Bin) that fits them by itself
/// while nobody dragged a border or fitted a single column by hand: when it is
/// first shown, when its content changes, and when the room changes.
#[derive(Default)]
pub struct AutoFit {
    by_hand: bool,
    /// What the columns were last fitted to.
    seen: Option<Seen>,
}

struct Seen {
    signature: u64,
    content: Widths,
    room: f32,
}

impl AutoFit {
    /// The widths to take now, if the columns are due for a fit. `content` measures
    /// each column's fit width; it only runs when `signature` (the content) changed.
    pub fn fit(&mut self, signature: u64, room: f32, shown: &Shown, roles: &Roles, content: impl FnOnce() -> Widths) -> Option<Widths> {
        if self.by_hand {
            return None;
        }
        let content = match self.seen.as_ref().filter(|seen| seen.signature == signature) {
            Some(seen) if seen.room == room => return None,
            Some(seen) => seen.content,
            None => content(),
        };
        self.seen = Some(Seen { signature, content, room });
        Some(fit_all(&content, shown, roles, room))
    }

    /// A border was dragged or a column fitted on its own: the widths are the user's now.
    pub fn by_hand(&mut self) {
        self.by_hand = true;
    }

    /// Fit All Columns: the view looks after its widths again.
    pub fn auto(&mut self) {
        self.by_hand = false;
        self.seen = None;
    }
}

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

/// The widths to draw: while a border is dragged, the ones the pointer gives
/// now; `saved` otherwise.
pub fn live_widths(ui: &egui::Ui, saved: &Widths) -> Widths {
    let pointer = ui.input(|i| i.pointer.latest_pos());
    match (ui.ctx().data(|d| d.get_temp::<Drag>(drag_id())), pointer) {
        (Some(drag), Some(pointer)) => drag.at(pointer.x),
        _ => *saved,
    }
}

/// What a column border was used for.
#[derive(Debug, Clone, PartialEq)]
pub enum BorderEvent {
    /// Dragged and let go: the new widths.
    Resized(Widths),
    /// Double clicked: fit the column on its left to its content (like Excel).
    Fit(usize),
}

/// The draggable border on the right of each header `cells` holds, drawn
/// over the header. Says when a border is let go after a drag, or double clicked.
pub fn column_borders(ui: &mut egui::Ui, cells: &[egui::Rect; COLUMNS.len()], widths: &Widths) -> Option<BorderEvent> {
    let gap = ui.spacing().item_spacing.x;
    let mut event = None;
    for (column, cell) in cells.iter().enumerate().filter(|(_, cell)| cell.is_positive()) {
        let x = cell.right() + gap / 2.0;
        let rect = egui::Rect::from_x_y_ranges(x - BORDER_GRAB..=x + BORDER_GRAB, cell.y_range());
        let response = ui.interact(rect, egui::Id::new(("lumen_column_border", column)), egui::Sense::CLICK | egui::Sense::DRAG);
        if response.double_clicked() {
            event = Some(BorderEvent::Fit(column));
        }
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
            event = Some(BorderEvent::Resized(drag.at(x)));
        }
    }
    event
}

pub(crate) fn cursor_row_id() -> egui::Id {
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

/// `name_label` without its own tooltip when cut: for a cell whose whole
/// area already shows the name on hover, so it never shows twice.
pub fn cell_name_label(ui: &mut egui::Ui, entry: &Entry, typed: &str) {
    let ranges = word_ranges(&entry.name, typed).unwrap_or_default();
    ui.add(name_widget(ui, entry, &ranges).show_tooltip_when_elided(false));
}

fn name_widget(ui: &egui::Ui, entry: &Entry, ranges: &[std::ops::Range<usize>]) -> egui::Label {
    let visuals = ui.visuals();
    let normal = if entry.hidden { visuals.weak_text_color() } else { visuals.text_color() };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let job = marked_name(&entry.name, ranges, normal, visuals.hyperlink_color, font);
    egui::Label::new(job).selectable(false).truncate()
}

/// [`name_label`] with the letters to mark given, for names found by the
/// quick jump, whose letters need not be whole words.
pub fn ranged_name_label(ui: &mut egui::Ui, entry: &Entry, ranges: &[std::ops::Range<usize>]) {
    ui.add(name_widget(ui, entry, ranges));
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

/// A cell of a number column in the views with fixed columns (This PC, the
/// Recycle Bin): right-aligned so the units line up, like the list's Size.
pub fn number_cell(ui: &mut egui::Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add(egui::Label::new(text).selectable(false));
    });
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

/// Id of the list's one Tab stop.
pub fn focus_id() -> egui::Id {
    egui::Id::new("lumen_list_focus")
}

/// The list's one Tab stop, over the whole list: Tab lands here, not on each
/// row or header. While it has the focus the arrows and Escape stay with the
/// list instead of moving egui's focus to a nearby widget. Its ring only shows
/// where Tab landed: the first key (not Tab or a modifier alone) or click on the list lets the focus
/// go, which leaves the keys with the list all the same.
pub fn focus_stop(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Response {
    let response = ui.interact(rect, focus_id(), egui::Sense::FOCUSABLE);
    if response.has_focus() {
        let used = ui.input(|i| {
            i.raw.events.iter().any(|e| match e {
                egui::Event::Key { key, pressed: true, .. } => !is_tab_or_modifier(*key),
                egui::Event::Text(_) => true,
                egui::Event::PointerButton { pos, pressed: true, .. } => rect.contains(*pos),
                _ => false,
            })
        });
        if used {
            ui.memory_mut(|m| m.surrender_focus(focus_id()));
        } else {
            let keep = egui::EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: true };
            ui.memory_mut(|m| m.set_focus_lock_filter(focus_id(), keep));
        }
    }
    response
}

/// Keys that do not count as using the list: Tab moves the focus, and a
/// modifier pressed alone (Shift before Shift+Tab) does nothing yet.
fn is_tab_or_modifier(key: egui::Key) -> bool {
    use egui::Key::*;
    matches!(key, Tab | ShiftLeft | ShiftRight | ControlLeft | ControlRight | AltLeft | AltRight | SuperLeft | SuperRight)
}

/// Id of the rename field, so the app can tell when it lost focus silently.
pub fn rename_id() -> egui::Id {
    egui::Id::new("lumen_rename")
}

/// The text field drawn in place of the name while renaming.
pub(crate) fn rename_field(ui: &mut egui::Ui, edit: &mut RenameEdit) -> Option<bool> {
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

/// A click on a column's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderClick {
    /// Left click: sort by the column.
    Sort,
    /// Right click: the column's menu.
    Menu,
    /// Middle click: close the column.
    Close,
}

/// The header cell `ui` is, for column `i`: the whole cell takes the click, not
/// only its label. `direction` is `Some(descending)` when the table is sorted by it.
pub fn header_cell(ui: &mut egui::Ui, i: usize, label: &str, direction: Option<bool>) -> Option<HeaderClick> {
    let cell = ui.interact(ui.max_rect(), egui::Id::new(("lumen_header", i)), egui::Sense::CLICK);
    let label = sort_header(ui, label, direction);
    if label.clicked() || cell.clicked() {
        Some(HeaderClick::Sort)
    } else if label.secondary_clicked() || cell.secondary_clicked() {
        Some(HeaderClick::Menu)
    } else if label.middle_clicked() || cell.middle_clicked() {
        Some(HeaderClick::Close)
    } else {
        None
    }
}

/// A header label that sorts on click; the sorted column shows its direction.
fn sort_header(ui: &mut egui::Ui, label: &str, direction: Option<bool>) -> egui::Response {
    let arrow = match direction {
        None => "",
        Some(false) => SORT_ASCENDING,
        Some(true) => SORT_DESCENDING,
    };
    let text = egui::RichText::new(format!("{label} {arrow}")).strong();
    ui.add(egui::Button::new(text).frame(false).sense(egui::Sense::CLICK))
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

    /// Where each text and the icon of the first row were painted.
    fn painted() -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows) = (vec![entry("a.txt")], vec![0]);
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default());
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
    fn names_stay_level_with_the_other_columns_and_icons_are_whole() {
        let shapes = painted();
        let y = |text: &str| {
            shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos.y + t.galley.rect.center().y),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{text:?} painted"))
        };
        assert!((y("a.txt") - y("TXT file")).abs() < 1.5, "name off its row");
        // The icon: small shapes in the Name column, none cut by the row's clip.
        for s in shapes.iter().filter(|s| !matches!(s.shape, egui::Shape::Text(_)) && s.clip_rect.min.x < 20.0) {
            let b = s.shape.visual_bounding_rect();
            if b.is_finite() && b.width() < 80.0 {
                assert!(b.max.y <= s.clip_rect.max.y + 0.5, "icon clipped {b:?} by {:?}", s.clip_rect);
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
        drag_holding(ctx, view, from, to, egui::Modifiers::NONE)
    }

    fn drag_holding(
        ctx: &egui::Context,
        view: &View,
        from: egui::Pos2,
        to: egui::Pos2,
        modifiers: egui::Modifiers,
    ) -> (Vec<Option<ListEvent>>, Vec<egui::epaint::ClippedShape>) {
        let steps = vec![
            vec![egui::Event::ModifiersChanged(modifiers), egui::Event::PointerMoved(from), button(from, true)],
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
            Some(ListEvent::Band { start, end, .. }) => Some((start.floor(), end.floor())),
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
    fn a_band_started_with_shift_held_adds_to_the_selection() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt"]);
        let (first, third) = (row_rect(&ctx, &mut view, 0), row_rect(&ctx, &mut view, 2));
        let (from, to) = (egui::pos2(500.0, first.center().y), egui::pos2(520.0, third.center().y));
        let (events, _) = drag_holding(&ctx, &view, from, to, egui::Modifiers::SHIFT);
        assert!(matches!(events[2], Some(ListEvent::Band { add: true, .. })), "Shift held at the press");
        let (events, _) = drag(&ctx, &view, from, to);
        assert!(matches!(events[2], Some(ListEvent::Band { add: false, .. })), "without Shift it replaces");
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

    fn carried(ctx: &egui::Context) -> Option<Vec<PathBuf>> {
        egui::DragAndDrop::payload::<crate::drag::Drag>(ctx).map(|drag| drag.paths.clone())
    }

    /// Presses at `from` and drags to `to`, holding the button; the list's
    /// event and what was painted on the last frame.
    fn lift(ctx: &egui::Context, view: &View, from: egui::Pos2, to: egui::Pos2) -> (Option<ListEvent>, Vec<egui::epaint::ClippedShape>) {
        let steps = [
            vec![egui::Event::PointerMoved(from), button(from, true)],
            vec![egui::Event::PointerMoved(from + egui::vec2(0.0, 10.0 * (to.y - from.y).signum()))],
            vec![egui::Event::PointerMoved(to)],
            // The label is laid out the frame it first shows.
            vec![],
        ];
        let mut last = (None, Vec::new());
        for events in steps {
            let mut event = None;
            let mut output = ctx.run_ui(egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() }, |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    event = show(ui, view, &[], Sizes::default(), None, None, &FileIcons::default());
                });
                crate::ui::dnd::finish(root.ctx());
            });
            output.textures_delta.clear();
            last = (event, output.shapes);
        }
        last
    }

    fn let_go(ctx: &egui::Context, view: &View, at: egui::Pos2) -> Vec<egui::epaint::ClippedShape> {
        let mut output = ctx.run_ui(egui::RawInput { events: vec![button(at, false)], screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() }, |root| {
            egui::CentralPanel::default().show(root, |ui| {
                show(ui, view, &[], Sizes::default(), None, None, &FileIcons::default());
            });
            crate::ui::dnd::finish(root.ctx());
        });
        output.textures_delta.clear();
        output.shapes
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from("/x").join(n)).collect()
    }

    #[test]
    fn a_drag_from_a_rows_icon_or_name_carries_the_entry_or_the_selection_it_is_in() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        let rows: Vec<egui::Rect> = (0..4).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let on_name = |row: usize| egui::pos2(rows[row].left() + 30.0, rows[row].center().y);
        let below = egui::pos2(300.0, 500.0);

        let (event, _) = lift(&ctx, &view, on_name(1), below);
        assert_eq!(carried(&ctx), Some(paths(&["b.txt"])), "nothing selected: the row under the pointer");
        assert!(band_rows(&event).is_none(), "not a band");
        let shapes = let_go(&ctx, &view, below);
        assert_eq!(carried(&ctx), None, "let go: nothing is carried any more");
        assert!(!texts_of(&shapes).iter().any(|t| t.contains("item")), "and no label");

        view.select_row(0);
        view.toggle_pick(2);
        lift(&ctx, &view, on_name(2), below);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt", "c.txt"])), "picked: the selection in display order");
        let_go(&ctx, &view, below);

        lift(&ctx, &view, on_name(3), below);
        assert_eq!(carried(&ctx), Some(paths(&["d.txt"])), "not in the selection: alone");
    }

    fn texts_of(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_drag_from_a_rows_empty_part_carries_nothing() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt"]);
        let (first, third) = (row_rect(&ctx, &mut view, 0), row_rect(&ctx, &mut view, 2));
        let (event, _) = lift(&ctx, &view, egui::pos2(500.0, first.center().y), egui::pos2(520.0, third.center().y));
        assert!(band_rows(&event).is_some(), "it is a band");
        assert_eq!(carried(&ctx), None);
    }

    #[test]
    fn a_drag_from_anywhere_on_a_selected_row_carries_the_selection_and_elsewhere_bands() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        let rows: Vec<egui::Rect> = (0..4).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let empty_part = |row: usize| egui::pos2(500.0, rows[row].center().y);
        let below = egui::pos2(300.0, 500.0);
        view.select_row(0);
        view.toggle_pick(1);
        // The Date column of a picked row: the selection goes.
        let (event, _) = lift(&ctx, &view, empty_part(1), below);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt", "b.txt"])));
        assert!(band_rows(&event).is_none(), "not a band");
        let_go(&ctx, &view, below);
        // The same column of a row that is not selected: a band, as before.
        let (event, _) = lift(&ctx, &view, empty_part(2), egui::pos2(520.0, rows[3].center().y));
        assert!(band_rows(&event).is_some(), "it is a band");
        assert_eq!(carried(&ctx), None);
    }

    #[test]
    fn a_label_by_the_pointer_counts_what_is_carried_and_a_click_on_a_name_still_selects() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt", "c.txt"]);
        view.current = PathBuf::from("/x");
        let rows: Vec<egui::Rect> = (0..3).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let name = egui::pos2(rows[0].left() + 30.0, rows[0].center().y);
        // Over a file row the folder on screen takes the drop: the label counts and names it.
        let to = egui::pos2(400.0, rows[2].center().y);
        let (_, shapes) = lift(&ctx, &view, name, to);
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to x"), "{:?}", texts_of(&shapes));
        let label = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Copy 1 item to x" => Some(t.pos),
                _ => None,
            })
            .unwrap();
        assert!(label.x > to.x && label.x < to.x + 40.0 && label.y > to.y && label.y < to.y + 40.0, "by the pointer: {label:?}");
        let_go(&ctx, &view, to);
        view.select_row(0);
        view.extend_to(2);
        let (_, shapes) = lift(&ctx, &view, name, to);
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 3 items to x"), "{:?}", texts_of(&shapes));
        let_go(&ctx, &view, to);

        // A click on the name is a click, not a drag.
        let at = egui::pos2(rows[1].left() + 30.0, rows[1].center().y);
        crate::ui::testing::frame(&ctx, vec![egui::Event::PointerMoved(at)], |ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default()));
        let clicked = crate::ui::testing::frame(&ctx, press(at, egui::PointerButton::Primary), |ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default()));
        assert!(matches!(clicked, Some(ListEvent::Select(1))));
        assert_eq!(carried(&ctx), None);
    }

    const SHIFT: egui::Modifiers = egui::Modifiers { shift: true, ..egui::Modifiers::NONE };

    /// One frame of the list at `sizes`, holding `modifiers`, with the label drawn.
    fn hold(ctx: &egui::Context, view: &View, sizes: Sizes, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Vec<egui::epaint::ClippedShape> {
        // The keys held stay held until a frame says otherwise.
        let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
        let mut output = ctx.run_ui(egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() }, |root| {
            egui::CentralPanel::default().show(root, |ui| {
                show(ui, view, &[], sizes, None, None, &FileIcons::default());
            });
            crate::ui::dnd::finish(root.ctx());
        });
        output.textures_delta.clear();
        output.shapes
    }

    /// Presses at `from` and moves a little, holding the button.
    fn pick_up(ctx: &egui::Context, view: &View, sizes: Sizes, from: egui::Pos2) {
        hold(ctx, view, sizes, vec![egui::Event::PointerMoved(from), button(from, true)], Default::default());
        hold(ctx, view, sizes, vec![egui::Event::PointerMoved(from + egui::vec2(0.0, 10.0))], Default::default());
    }

    fn lit(ctx: &egui::Context, shapes: &[egui::epaint::ClippedShape], at: egui::Pos2) -> bool {
        let fill = crate::ui::dnd::lit_fill(&ctx.global_style().visuals);
        shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == fill && r.rect.contains(at)))
    }

    /// Lets go at `at` holding `modifiers`; the drop the list took.
    fn release(ctx: &egui::Context, view: &View, sizes: Sizes, at: egui::Pos2, modifiers: egui::Modifiers) -> Option<(Vec<PathBuf>, PathBuf)> {
        hold(ctx, view, sizes, vec![button(at, false)], modifiers);
        crate::ui::dnd::take_drop(ctx).map(|d| (d.drag.paths, d.target))
    }

    fn folder_entry(name: &str) -> Entry {
        Entry { is_dir: true, size: None, ..entry(name) }
    }

    #[test]
    fn a_dragged_row_over_a_folder_row_lights_it_names_the_action_and_drops_on_it() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.entries = vec![folder_entry("dir"), entry("a.txt"), entry("b.txt")];
        view.rows = vec![0, 1, 2];
        view.current = PathBuf::from("/x");
        let rows: Vec<egui::Rect> = (0..3).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let (name_a, dir, b) = (egui::pos2(rows[1].left() + 30.0, rows[1].center().y), egui::pos2(500.0, rows[0].center().y), egui::pos2(500.0, rows[2].center().y));
        pick_up(&ctx, &view, Sizes::default(), name_a);
        let shapes = hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(dir)], SHIFT);
        assert!(lit(&ctx, &shapes, dir), "the folder row lights up");
        assert!(!lit(&ctx, &shapes, name_a) && !lit(&ctx, &shapes, b), "and only it");
        assert!(texts_of(&shapes).iter().any(|t| t == "Move 1 item to dir"), "{:?}", texts_of(&shapes));
        let shapes = hold(&ctx, &view, Sizes::default(), vec![], Default::default());
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to dir"), "{:?}", texts_of(&shapes));
        assert_eq!(release(&ctx, &view, Sizes::default(), dir, SHIFT), Some((paths(&["a.txt"]), PathBuf::from("/x/dir"))));
        assert!(crate::ui::dnd::take_drop(&ctx).is_none(), "taken once");
        assert!(!lit(&ctx, &hold(&ctx, &view, Sizes::default(), vec![], Default::default()), dir), "dark again");

        // A file row is no folder: the folder on screen takes the drop, and no row lights up.
        pick_up(&ctx, &view, Sizes::default(), name_a);
        let shapes = hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(b)], Default::default());
        assert!(!lit(&ctx, &shapes, b));
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to x"), "{:?}", texts_of(&shapes));
        assert_eq!(release(&ctx, &view, Sizes::default(), b, Default::default()), Some((paths(&["a.txt"]), PathBuf::from("/x"))));
    }

    #[test]
    fn a_file_row_and_the_gap_between_rows_drop_into_the_folder_on_screen() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.entries = vec![folder_entry("dir"), entry("a.txt"), entry("b.txt")];
        view.rows = vec![0, 1, 2];
        view.current = PathBuf::from("/x");
        let rows: Vec<egui::Rect> = (0..3).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let name_a = egui::pos2(rows[1].left() + 30.0, rows[1].center().y);
        // Between two rows: no row, not the space below the rows either.
        let gap = egui::pos2(500.0, (rows[1].bottom() + rows[2].top()) / 2.0);
        assert!(rows[2].top() > rows[1].bottom(), "a gap between the rows: {:?} {:?}", rows[1], rows[2]);
        let ring = ctx.global_style().visuals.selection.stroke;
        for over in [egui::pos2(500.0, rows[2].center().y), gap] {
            pick_up(&ctx, &view, Sizes::default(), name_a);
            let shapes = hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(over)], Default::default());
            assert!(!lit(&ctx, &shapes, over), "no row lights up");
            assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to x"), "{:?}", texts_of(&shapes));
            let ringed = shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke == ring && r.fill == egui::Color32::TRANSPARENT && r.rect.contains(over)));
            assert!(ringed, "the list is ringed");
            assert_eq!(release(&ctx, &view, Sizes::default(), over, Default::default()), Some((paths(&["a.txt"]), PathBuf::from("/x"))));
        }
    }

    #[test]
    fn a_folder_row_that_refuses_the_drag_stays_refused_and_the_folder_on_screen_does_not_take_it() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.entries = vec![folder_entry("dir"), entry("a.txt")];
        view.rows = vec![0, 1];
        view.current = PathBuf::from("/x");
        let dir = row_rect(&ctx, &mut view, 0);
        let name = egui::pos2(dir.left() + 30.0, dir.center().y);
        let over = egui::pos2(500.0, dir.center().y);
        // `dir` onto itself.
        pick_up(&ctx, &view, Sizes::default(), name);
        let shapes = hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(over)], Default::default());
        assert!(!lit(&ctx, &shapes, over));
        assert!(texts_of(&shapes).iter().any(|t| t == "1 item"), "{:?}", texts_of(&shapes));
        assert_eq!(release(&ctx, &view, Sizes::default(), over, Default::default()), None);
    }

    #[test]
    fn a_folder_row_cannot_take_itself_and_a_folder_unfolded_in_the_tree_takes_a_drop() {
        let ctx = egui::Context::default();
        let view = tree_listed();
        let tree = Sizes { tree: true, ..Sizes::default() };
        settled_tree(&ctx, &view, true);
        let shapes = settled_tree(&ctx, &view, true);
        let (src, deep, a) = (text_rect(&shapes, "src").center(), text_rect(&shapes, "deep").center(), text_rect(&shapes, "a.txt").center());
        // `src` onto `deep`, which is inside it: refused.
        pick_up(&ctx, &view, tree, src);
        let shapes = hold(&ctx, &view, tree, vec![egui::Event::PointerMoved(deep)], Default::default());
        assert!(!lit(&ctx, &shapes, deep), "a folder is not dropped below itself");
        assert!(texts_of(&shapes).iter().any(|t| t == "1 item"));
        assert_eq!(release(&ctx, &view, tree, deep, Default::default()), None);
        // `a.txt` onto `deep`.
        pick_up(&ctx, &view, tree, a);
        let shapes = hold(&ctx, &view, tree, vec![egui::Event::PointerMoved(deep)], Default::default());
        assert!(lit(&ctx, &shapes, deep), "the unfolded folder lights up");
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to deep"), "{:?}", texts_of(&shapes));
        assert_eq!(release(&ctx, &view, tree, deep, Default::default()), Some((paths(&["a.txt"]), PathBuf::from("/x/src/deep"))));
    }

    #[test]
    fn letting_go_below_the_rows_drops_into_the_folder_on_screen() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt"]);
        view.current = PathBuf::from("/x");
        let first = row_rect(&ctx, &mut view, 0);
        let name = egui::pos2(first.left() + 30.0, first.center().y);
        let below = egui::pos2(300.0, 500.0);
        pick_up(&ctx, &view, Sizes::default(), name);
        let shapes = hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(below)], Default::default());
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to x"), "{:?}", texts_of(&shapes));
        let ring = ctx.global_style().visuals.selection.stroke;
        let ringed = shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke == ring && r.fill == egui::Color32::TRANSPARENT && r.rect.contains(below)));
        assert!(ringed, "the space below the rows is ringed");
        assert_eq!(release(&ctx, &view, Sizes::default(), below, Default::default()), Some((paths(&["a.txt"]), PathBuf::from("/x"))));
        // Over a file row the folder on screen takes it as well (a.txt is already in it).
        pick_up(&ctx, &view, Sizes::default(), name);
        let over_b = egui::pos2(500.0, row_rect(&ctx, &mut view, 1).center().y);
        let shapes = hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(over_b)], Default::default());
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to x"), "{:?}", texts_of(&shapes));
        assert_eq!(release(&ctx, &view, Sizes::default(), over_b, Default::default()), Some((paths(&["a.txt"]), PathBuf::from("/x"))));
    }

    #[test]
    fn held_near_the_top_or_bottom_edge_a_carried_drag_scrolls_the_rows() {
        use crate::ui::testing::SCREEN;
        let ctx = egui::Context::default();
        let names: Vec<String> = (0..300).map(|i| format!("f{i}")).collect();
        let mut view = listed(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let first = row_rect(&ctx, &mut view, 0);
        let top_of = |shapes: &[egui::epaint::ClippedShape]| text_rect(shapes, "f10").top();
        let sizes = Sizes::default();
        let before = top_of(&hold(&ctx, &view, sizes, vec![], Default::default()));
        pick_up(&ctx, &view, sizes, egui::pos2(first.left() + 30.0, first.center().y));
        let middle = egui::pos2(500.0, SCREEN.center().y);
        hold(&ctx, &view, sizes, vec![egui::Event::PointerMoved(middle)], Default::default());
        for _ in 0..5 {
            hold(&ctx, &view, sizes, vec![], Default::default());
        }
        assert_eq!(top_of(&hold(&ctx, &view, sizes, vec![], Default::default())), before, "held in the middle: no scroll");

        let near = egui::pos2(500.0, SCREEN.bottom() - 12.0);
        hold(&ctx, &view, sizes, vec![egui::Event::PointerMoved(near)], Default::default());
        let tops: Vec<f32> = (0..5).map(|_| top_of(&hold(&ctx, &view, sizes, vec![], Default::default()))).collect();
        assert!(tops.windows(2).all(|w| w[1] < w[0]) && tops[4] < before - 20.0, "the rows come up while held: {tops:?}");
        let_go(&ctx, &view, near);

        // Without a drag the pointer there does nothing (once the last scrolls have landed).
        for _ in 0..3 {
            hold(&ctx, &view, sizes, vec![], Default::default());
        }
        let settled_top = top_of(&hold(&ctx, &view, sizes, vec![], Default::default()));
        for _ in 0..5 {
            hold(&ctx, &view, sizes, vec![egui::Event::PointerMoved(near)], Default::default());
        }
        assert_eq!(top_of(&hold(&ctx, &view, sizes, vec![], Default::default())), settled_top, "no drag, no scroll");
    }

    #[test]
    fn a_band_dragged_past_the_bottom_or_the_top_scrolls_the_list() {
        use crate::ui::testing::frame;
        let ctx = egui::Context::default();
        let names: Vec<String> = (0..300).map(|i| format!("f{i}")).collect();
        let mut view = listed(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let first = row_rect(&ctx, &mut view, 0);
        let from = egui::pos2(500.0, first.center().y);
        let step = |events| match frame(&ctx, events, |ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default())) {
            Some(ListEvent::Band { end, .. }) => end,
            _ => panic!("no band"),
        };
        frame(&ctx, vec![egui::Event::PointerMoved(from), button(from, true)], |ui| {
            show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default())
        });
        let below = egui::pos2(500.0, 700.0);
        let mut ends = vec![step(vec![egui::Event::PointerMoved(below)])];
        for _ in 0..10 {
            ends.push(step(vec![]));
        }
        // A scroll shows on the next frame: the end may stay put for one.
        assert!(ends.iter().all(|e| e.is_finite()), "{ends:?}");
        assert!(ends.windows(2).all(|w| w[1] >= w[0]), "held below, the rows keep coming up: {ends:?}");
        assert!(ends[10] - ends[0] >= 1.0, "at least a row more: {ends:?}");
        let above = egui::pos2(500.0, 0.0);
        let mut ends = vec![step(vec![egui::Event::PointerMoved(above)])];
        for _ in 0..10 {
            ends.push(step(vec![]));
        }
        assert!(ends.iter().all(|e| e.is_finite()), "{ends:?}");
        // The scroll asked for on the last frame below lands on the first one above.
        assert!(ends[1..].windows(2).all(|w| w[1] <= w[0]), "held above, they go back down: {ends:?}");
        assert!(ends[0] - ends[10] >= 1.0, "at least a row less: {ends:?}");
    }

    #[test]
    fn the_band_scrolls_from_a_strip_inside_each_edge_as_fast_at_both_ends() {
        let view = egui::Rangef::new(100.0, 500.0);
        assert_eq!(band_speed(300.0, view), 0.0, "the middle does not scroll");
        assert_eq!(band_speed(100.0 + BAND_STRIP + 1.0, view), 0.0, "nor just past the strip");
        let (top, bottom) = (band_speed(110.0, view), band_speed(490.0, view));
        assert!(top > 0.0, "inside the top strip the rows above come down: {top}");
        assert_eq!(bottom, -top, "the same depth into the bottom strip, the same speed up");
        assert!(band_speed(105.0, view) > top, "faster closer to the edge");
        assert_eq!(band_speed(100.0, view), BAND_TOP_SPEED, "top speed at the edge itself");
        assert_eq!(band_speed(500.0, view), -BAND_TOP_SPEED);
        assert_eq!(band_speed(0.0, view), BAND_TOP_SPEED, "and no faster past it");
        assert_eq!(band_speed(900.0, view), -BAND_TOP_SPEED);
        // A list too short for two strips: the nearer edge decides.
        let short = egui::Rangef::new(100.0, 140.0);
        assert!(band_speed(110.0, short) > 0.0 && band_speed(130.0, short) < 0.0);
    }

    #[test]
    fn a_band_held_near_the_bottom_inside_the_window_scrolls_the_list() {
        use crate::ui::testing::{SCREEN, frame};
        let ctx = egui::Context::default();
        let names: Vec<String> = (0..300).map(|i| format!("f{i}")).collect();
        let mut view = listed(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let first = row_rect(&ctx, &mut view, 0);
        let from = egui::pos2(500.0, first.center().y);
        let draw = |ui: &mut egui::Ui| show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default());
        frame(&ctx, vec![egui::Event::PointerMoved(from), button(from, true)], draw);
        // Inside the window and over the last rows on view.
        let near = egui::pos2(500.0, SCREEN.bottom() - 12.0);
        let mut ends = Vec::new();
        for events in [vec![egui::Event::PointerMoved(near)], vec![], vec![], vec![], vec![]] {
            match frame(&ctx, events, draw) {
                Some(ListEvent::Band { end, .. }) => ends.push(end),
                _ => panic!("no band"),
            }
        }
        assert!(ends[4] > ends[0], "the rows keep coming up: {ends:?}");
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
    fn an_unselected_cursor_row_has_a_dashed_outline() {
        let ctx = egui::Context::default();
        let mut view = listed(&["a.txt", "b.txt"]);
        view.select_row(1);
        view.unselect();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() }, |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    show(ui, &view, &[], Sizes::default(), None, None, &FileIcons::default());
                });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let y = |text: &str| shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos.y + t.galley.size().y / 2.0),
            _ => None,
        }).unwrap();
        let (a, b) = (y("a.txt"), y("b.txt"));
        let pitch = b - a;
        let band = |mid: f32| egui::Rect::from_min_max(egui::pos2(0.0, mid - pitch / 2.0), egui::pos2(crate::ui::testing::SCREEN.max.x, mid + pitch / 2.0));
        assert!(crate::ui::icon_grid::tests::dashes_in(&shapes, band(b)) >= 8, "dashes around b.txt's row");
        assert_eq!(crate::ui::icon_grid::tests::dashes_in(&shapes, band(a)), 0);
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
        draw_list_at(ctx, view, widths, events, None)
    }

    /// `draw_list` at `time`, for double clicks.
    fn draw_list_at(ctx: &egui::Context, view: &View, widths: &Widths, events: Vec<egui::Event>, time: Option<f64>) -> Drawn {
        let mut event = None;
        let mut output = ctx.run_ui(
            egui::RawInput { events, time, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
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
    fn a_double_click_on_a_header_border_fits_the_column_on_its_left_and_does_not_sort() {
        let ctx = egui::Context::default();
        let view = listed(&["a.txt", "b.txt"]);
        let lefts = settled(&ctx, &view, &DEFAULT_WIDTHS);
        // The border between Type and Date Modified.
        let border = egui::pos2(lefts[2] - 8.0, 10.0);
        let click = || vec![egui::Event::PointerMoved(border), button(border, true), button(border, false)];
        let first = draw_list_at(&ctx, &view, &DEFAULT_WIDTHS, click(), Some(10.0));
        assert!(first.event.is_none(), "one click on a border neither sorts nor fits");
        let second = draw_list_at(&ctx, &view, &DEFAULT_WIDTHS, click(), Some(10.1));
        assert!(matches!(second.event, Some(ListEvent::Fit(1))), "Type, the column on the border's left");
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
        let name = fit_width(&ctx, &view, 0);
        assert_eq!(painted_name(&view, name, long), long, "whole at its fit width ({name})");
        assert_ne!(painted_name(&view, name - 20.0, long), long, "a little narrower cuts it");
        // Folders holds only "--": the header sets its width.
        let font = egui::TextStyle::Body.resolve(&ctx.global_style());
        let header = ctx.fonts_mut(|f| f.layout_no_wrap("Folders".into(), font, egui::Color32::WHITE).size().x);
        let folders = fit_width(&ctx, &view, 3);
        assert!(folders >= header && folders < header + 40.0, "{folders} for a {header} header");
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

    fn tab() -> egui::Event {
        egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// A button, the list and another button, like the side panel and the
    /// list in the window. Returns the buttons' ids and the list's rect.
    fn between_buttons(ctx: &egui::Context, view: &View, events: Vec<egui::Event>) -> (egui::Id, egui::Id, egui::Rect, Vec<egui::epaint::ClippedShape>) {
        let (mut before, mut after, mut rect) = (egui::Id::NULL, egui::Id::NULL, egui::Rect::NOTHING);
        let mut output = ctx.run_ui(
            egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() },
            |root| {
                crate::ui::shift_tab_from_list(root.ctx());
                egui::Panel::top("before").show(root, |ui| before = ui.button("before").id);
                egui::CentralPanel::default().show(root, |ui| {
                    rect = ui.max_rect();
                    show(ui, view, &[], Sizes::default(), None, None, &FileIcons::default());
                });
                // Tab follows the drawing order: this one comes after the list.
                egui::Area::new(egui::Id::new("after")).fixed_pos(egui::pos2(700.0, 560.0)).show(root.ctx(), |ui| after = ui.button("after").id);
            },
        );
        output.textures_delta.clear();
        (before, after, rect, output.shapes)
    }

    #[test]
    fn the_list_is_one_tab_stop_not_one_per_row_or_header() {
        let ctx = egui::Context::default();
        let view = listed(&["a", "b", "c", "d"]);
        let (before, after, _, _) = between_buttons(&ctx, &view, vec![]);
        ctx.memory_mut(|m| m.request_focus(before));
        between_buttons(&ctx, &view, vec![]);
        between_buttons(&ctx, &view, vec![tab()]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "Tab from the button lands on the list");
        between_buttons(&ctx, &view, vec![tab()]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(after), "the next Tab leaves the list");
        let back = egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::SHIFT };
        between_buttons(&ctx, &view, vec![back]);
        between_buttons(&ctx, &view, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "Shift+Tab comes back to the list");
    }

    #[test]
    fn shift_tab_with_nothing_focused_starts_from_the_list() {
        let ctx = egui::Context::default();
        let view = listed(&["a", "b", "c", "d"]);
        let (before, _, _, _) = between_buttons(&ctx, &view, vec![]);
        between_buttons(&ctx, &view, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), None, "a click on a row leaves nothing focused");
        let back = egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::SHIFT };
        between_buttons(&ctx, &view, vec![back]);
        between_buttons(&ctx, &view, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(before), "the first Shift+Tab goes to what comes before the list");
    }

    #[test]
    fn a_key_or_a_click_on_the_focused_list_drops_its_ring_but_not_its_keys() {
        let view = listed(&["a", "b", "c", "d"]);
        let ring = egui::Context::default().global_style().visuals.selection.stroke.color;
        let rings = |shapes: &[egui::epaint::ClippedShape], list: egui::Rect| -> usize {
            shapes
                .iter()
                .filter(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color == ring && r.rect.width() > list.width() - 20.0))
                .count()
        };
        let row = |list: egui::Rect| list.center_top() + egui::vec2(0.0, 40.0);
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let list = between_buttons(&egui::Context::default(), &view, vec![]).2;
        let inputs: Vec<(&str, Vec<Vec<egui::Event>>)> = vec![
            ("Down", vec![vec![key(egui::Key::ArrowDown)]]),
            ("Escape", vec![vec![key(egui::Key::Escape)]]),
            ("a letter", vec![vec![egui::Event::Text("a".into())]]),
            ("a click", vec![vec![egui::Event::PointerMoved(row(list))], vec![press(row(list), true)], vec![press(row(list), false)]]),
        ];
        for (what, frames) in inputs {
            let ctx = egui::Context::default();
            between_buttons(&ctx, &view, vec![]);
            ctx.memory_mut(|m| m.request_focus(focus_id()));
            between_buttons(&ctx, &view, vec![]);
            let (_, _, list, shapes) = between_buttons(&ctx, &view, vec![]);
            assert_eq!(rings(&shapes, list), 1, "the ring before {what}");
            for events in frames {
                between_buttons(&ctx, &view, events);
            }
            let (_, _, list, shapes) = between_buttons(&ctx, &view, vec![]);
            assert_eq!(rings(&shapes, list), 0, "{what} drops the ring");
            let focused = ctx.memory(|m| m.focused());
            assert!(focused.is_none_or(|id| id == focus_id()), "{what} leaves the keys with the list, not {focused:?}");
        }
    }

    #[test]
    fn a_ring_shows_round_the_list_only_while_it_has_the_focus() {
        let ctx = egui::Context::default();
        let view = listed(&["a", "b"]);
        let ring = ctx.global_style().visuals.selection.stroke.color;
        let rings = |shapes: &[egui::epaint::ClippedShape], list: egui::Rect| -> usize {
            shapes
                .iter()
                .filter(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color == ring && r.rect.width() > list.width() - 20.0 && r.rect.height() > list.height() - 20.0))
                .count()
        };
        between_buttons(&ctx, &view, vec![]);
        let (_, _, list, shapes) = between_buttons(&ctx, &view, vec![]);
        assert_eq!(rings(&shapes, list), 0, "no ring without the focus");
        ctx.memory_mut(|m| m.request_focus(focus_id()));
        between_buttons(&ctx, &view, vec![]);
        let (_, _, list, shapes) = between_buttons(&ctx, &view, vec![]);
        assert_eq!(rings(&shapes, list), 1, "a ring round the focused list");
    }

    #[test]
    fn a_click_on_the_list_takes_the_focus_from_another_part() {
        let ctx = egui::Context::default();
        let view = listed(&["a", "b", "c", "d"]);
        let (before, _, list, _) = between_buttons(&ctx, &view, vec![]);
        ctx.memory_mut(|m| m.request_focus(before));
        between_buttons(&ctx, &view, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(before));
        let on_row = list.center_top() + egui::vec2(0.0, 40.0);
        let press = |pressed| egui::Event::PointerButton { pos: on_row, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        between_buttons(&ctx, &view, vec![egui::Event::PointerMoved(on_row)]);
        between_buttons(&ctx, &view, vec![press(true)]);
        between_buttons(&ctx, &view, vec![press(false)]);
        let focused = ctx.memory(|m| m.focused());
        assert!(focused.is_none_or(|id| id == focus_id()), "the list holds the keys again, not {focused:?}");
    }

    #[test]
    fn shift_tab_from_the_focused_list_goes_back_in_one_press() {
        let ctx = egui::Context::default();
        let view = listed(&["a", "b"]);
        let (before, _, _, _) = between_buttons(&ctx, &view, vec![]);
        ctx.memory_mut(|m| m.request_focus(focus_id()));
        between_buttons(&ctx, &view, vec![]);
        let key = |key, pressed, modifiers| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        // Shift goes down first, as a key of its own.
        let shift = egui::Modifiers::SHIFT;
        between_buttons(&ctx, &view, vec![egui::Event::ModifiersChanged(shift), key(egui::Key::ShiftLeft, true, shift)]);
        between_buttons(&ctx, &view, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "Shift alone keeps the ring");
        for modifier in [egui::Key::ControlLeft, egui::Key::AltRight, egui::Key::SuperLeft] {
            between_buttons(&ctx, &view, vec![key(modifier, true, egui::Modifiers::NONE), key(modifier, false, egui::Modifiers::NONE)]);
            assert_eq!(ctx.memory(|m| m.focused()), Some(focus_id()), "{modifier:?} alone keeps the ring");
        }
        between_buttons(&ctx, &view, vec![key(egui::Key::Tab, true, shift)]);
        between_buttons(&ctx, &view, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(before), "one Shift+Tab goes back");
    }

    /// `/x` in the Tree view: src (unfolded: deep/, main.rs) and a.txt.
    fn tree_listed() -> View {
        let folder = |path: &str| Entry { is_dir: true, size: None, ..entry(path) };
        let mut view = View::default();
        let top = vec![Entry { path: PathBuf::from("/x/src"), ..folder("src") }, entry("a.txt")];
        view.apply_listing(crate::fs::Listing { id: 1, path: PathBuf::from("/x"), result: Ok(top), elapsed: std::time::Duration::ZERO }, None);
        view.toggle_open(0);
        let kids = vec![
            Entry { path: PathBuf::from("/x/src/deep"), ..folder("deep") },
            Entry { path: PathBuf::from("/x/src/main.rs"), ..entry("main.rs") },
        ];
        view.apply_kids(crate::fs::Listing { id: 2, path: PathBuf::from("/x/src"), result: Ok(kids), elapsed: std::time::Duration::ZERO });
        view
    }

    /// One frame of the list, at `time`, with these events.
    fn draw_tree(ctx: &egui::Context, view: &View, tree: bool, events: Vec<egui::Event>, time: Option<f64>) -> (Option<ListEvent>, Vec<egui::epaint::ClippedShape>) {
        let mut event = None;
        let input = egui::RawInput { events, time, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| {
                event = show(ui, view, &[], Sizes { tree, ..Sizes::default() }, None, None, &FileIcons::default());
            });
        });
        output.textures_delta.clear();
        (event, output.shapes)
    }

    fn settled_tree(ctx: &egui::Context, view: &View, tree: bool) -> Vec<egui::epaint::ClippedShape> {
        draw_tree(ctx, view, tree, vec![], None);
        draw_tree(ctx, view, tree, vec![], None);
        draw_tree(ctx, view, tree, vec![], None).1
    }

    /// The chevrons painted: the corners of each one's open line, in drawing order.
    fn chevrons(shapes: &[egui::epaint::ClippedShape]) -> Vec<Vec<egui::Pos2>> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Path(path) if !path.closed && path.points.len() == 3 => Some(path.points.clone()),
                _ => None,
            })
            .collect()
    }

    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Rect {
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{text:?} painted"))
    }

    fn bounds(points: &[egui::Pos2]) -> egui::Rect {
        egui::Rect::from_points(points)
    }

    #[test]
    fn the_tree_view_paints_a_chevron_before_each_folders_icon_and_details_paints_none() {
        let ctx = egui::Context::default();
        let view = tree_listed();
        let shapes = settled_tree(&ctx, &view, true);
        let chevrons = chevrons(&shapes);
        assert_eq!(chevrons.len(), 2, "src and deep are folders; main.rs and a.txt are not: {chevrons:?}");
        let (src, deep) = (text_rect(&shapes, "src"), text_rect(&shapes, "deep"));
        for (chevron, name) in [(&chevrons[0], src), (&chevrons[1], deep)] {
            let at = bounds(chevron);
            let icon = icon_side(14.0);
            assert!(at.right() < name.left() - icon + 1.0, "before the icon: {at:?} {name:?}");
            assert!((at.center().y - name.center().y).abs() < 2.0, "on the folder's row");
        }
        let plain = settled_tree(&ctx, &view, false);
        assert!(self::chevrons(&plain).is_empty(), "the Details table has none");
    }

    #[test]
    fn the_chevron_points_right_when_folded_and_down_when_unfolded() {
        let ctx = egui::Context::default();
        let chevrons = chevrons(&settled_tree(&ctx, &tree_listed(), true));
        let (open, folded) = (&chevrons[0], &chevrons[1]);
        assert!(open[1].y > open[0].y + 1.0 && (open[0].y - open[2].y).abs() < 0.5, "v: {open:?}");
        assert!(open[0].x < open[1].x && open[1].x < open[2].x, "v: {open:?}");
        assert!(folded[1].x > folded[0].x + 1.0 && (folded[0].x - folded[2].x).abs() < 0.5, ">: {folded:?}");
        assert!(folded[0].y < folded[1].y && folded[1].y < folded[2].y, ">: {folded:?}");
    }

    #[test]
    fn children_are_indented_one_step_under_their_folder_and_files_line_up_with_folders() {
        let ctx = egui::Context::default();
        let shapes = settled_tree(&ctx, &tree_listed(), true);
        let x = |name: &str| text_rect(&shapes, name).left();
        let step = x("deep") - x("src");
        assert!(step >= icon_side(14.0) - 1.0, "a step is at least an icon wide: {step}");
        assert!((x("main.rs") - x("deep")).abs() < 0.5, "a file under src lines up with a folder under src");
        assert!((x("a.txt") - x("src")).abs() < 0.5, "and at the top too");
    }

    #[test]
    fn a_click_on_the_chevron_unfolds_or_folds_and_a_click_elsewhere_selects() {
        let ctx = egui::Context::default();
        let view = tree_listed();
        let shapes = settled_tree(&ctx, &view, true);
        let chevron = |i: usize| bounds(&chevrons(&shapes)[i]).center();
        let click = |at: egui::Pos2| {
            draw_tree(&ctx, &view, true, vec![egui::Event::PointerMoved(at)], None);
            let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            let event = draw_tree(&ctx, &view, true, vec![press(true), press(false)], None).0;
            settled_tree(&ctx, &view, true);
            event
        };
        assert!(matches!(click(chevron(0)), Some(ListEvent::Unfold(0))), "src's chevron");
        assert!(matches!(click(chevron(1)), Some(ListEvent::Unfold(1))), "deep's chevron");
        assert!(matches!(click(text_rect(&shapes, "src").center()), Some(ListEvent::Select(0))), "the name selects");
        assert!(matches!(click(text_rect(&shapes, "main.rs").center()), Some(ListEvent::Select(2))));
        let blank = egui::pos2(chevron(1).x, text_rect(&shapes, "main.rs").center().y);
        assert!(matches!(click(blank), Some(ListEvent::Select(2))), "where a file has no chevron, it is the row");
        let date = egui::pos2(500.0, text_rect(&shapes, "src").center().y);
        assert!(matches!(click(date), Some(ListEvent::Select(0))), "the rest of the row selects");
    }

    #[test]
    fn a_double_click_on_a_folders_name_still_opens_it() {
        let ctx = egui::Context::default();
        let view = tree_listed();
        let shapes = settled_tree(&ctx, &view, true);
        let at = text_rect(&shapes, "src").center();
        let events = || {
            let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            vec![egui::Event::PointerMoved(at), press(true), press(false)]
        };
        draw_tree(&ctx, &view, true, events(), Some(10.0));
        let second = draw_tree(&ctx, &view, true, events(), Some(10.1)).0;
        assert!(matches!(second, Some(ListEvent::Activate(0))), "opens, not unfolds");
    }

    #[test]
    fn files_dragged_in_from_another_app_light_the_folder_row_under_them_and_drop_on_it() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.entries = vec![folder_entry("dir"), entry("a.txt")];
        view.rows = vec![0, 1];
        let rows: Vec<egui::Rect> = (0..2).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let (dir, file) = (egui::pos2(500.0, rows[0].center().y), egui::pos2(500.0, rows[1].center().y));
        let outside = |at, released| {
            let drag = crate::drag::Drag { paths: paths(&["a.txt"]) };
            crate::ui::dnd::set_outside(&ctx, Some(crate::ui::dnd::Outside { drag, at, keys: crate::drag::Keys { ctrl: true, shift: false }, released }));
        };
        // egui's pointer is over the file row; the drag is over the folder row.
        hold(&ctx, &view, Sizes::default(), vec![egui::Event::PointerMoved(file)], Default::default());
        outside(dir, false);
        hold(&ctx, &view, Sizes::default(), vec![], Default::default());
        let shapes = hold(&ctx, &view, Sizes::default(), vec![], Default::default());
        assert!(lit(&ctx, &shapes, dir) && !lit(&ctx, &shapes, file), "only the folder row lights up");
        assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to dir"), "{:?}", texts_of(&shapes));
        outside(dir, true);
        hold(&ctx, &view, Sizes::default(), vec![], Default::default());
        assert_eq!(crate::ui::dnd::take_drop(&ctx).map(|d| (d.drag.paths, d.target)), Some((paths(&["a.txt"]), PathBuf::from("/x/dir"))));
        crate::ui::dnd::set_outside(&ctx, None);
    }

    #[test]
    fn files_dragged_in_from_another_app_over_a_file_row_or_a_gap_drop_into_the_folder_on_screen() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.entries = vec![folder_entry("dir"), entry("a.txt"), entry("b.txt")];
        view.rows = vec![0, 1, 2];
        view.current = PathBuf::from("/x");
        let rows: Vec<egui::Rect> = (0..3).map(|row| row_rect(&ctx, &mut view, row)).collect();
        let gap = egui::pos2(500.0, (rows[1].bottom() + rows[2].top()) / 2.0);
        let outside = |at, released| {
            let drag = crate::drag::Drag { paths: paths(&["a.txt"]) };
            crate::ui::dnd::set_outside(&ctx, Some(crate::ui::dnd::Outside { drag, at, keys: Default::default(), released }));
        };
        for over in [egui::pos2(500.0, rows[2].center().y), gap] {
            outside(over, false);
            hold(&ctx, &view, Sizes::default(), vec![], Default::default());
            let shapes = hold(&ctx, &view, Sizes::default(), vec![], Default::default());
            assert!(!lit(&ctx, &shapes, over), "no row lights up");
            assert!(texts_of(&shapes).iter().any(|t| t == "Copy 1 item to x"), "{:?}", texts_of(&shapes));
            outside(over, true);
            hold(&ctx, &view, Sizes::default(), vec![], Default::default());
            assert_eq!(crate::ui::dnd::take_drop(&ctx).map(|d| (d.drag.paths, d.target)), Some((paths(&["a.txt"]), PathBuf::from("/x"))));
        }
        crate::ui::dnd::set_outside(&ctx, None);
    }

    /// The widths of the shown columns added up.
    fn total(widths: &Widths, shown: &Shown) -> f32 {
        widths.iter().zip(shown).filter(|&(_, &on)| on).map(|(w, _)| w).sum()
    }

    #[test]
    fn fit_all_spreads_the_spare_room_over_every_shown_column_in_proportion_to_its_content() {
        let content = [200.0, 100.0, 140.0, 50.0, 50.0, 60.0];
        let fitted = fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 900.0);
        assert_eq!(fitted, [300.0, 150.0, 210.0, 75.0, 75.0, 90.0], "600 points of content over 900: everyone 1.5 times");
        assert_eq!(fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 600.0), content, "exactly the room: nothing to share");
    }

    #[test]
    fn fit_all_follows_the_roles_not_the_column_positions_when_they_do_not_fit() {
        // The long text column gives before the numbers, wherever it sits.
        let roles = [Role::Name, Role::Short, Role::Numeric, Role::Numeric, Role::Short, Role::Text];
        let content = [300.0, 110.0, 70.0, 70.0, 150.0, 600.0];
        let fitted = fit_all(&content, &ALL_SHOWN, &roles, 1000.0);
        assert!((total(&fitted, &ALL_SHOWN) - 1000.0).abs() < 0.5, "{fitted:?}");
        assert_eq!(fitted, [300.0, 110.0, 70.0, 70.0, 150.0, 300.0], "only the text column gives");
        // The same widths with the list's roles (no text column): the numbers give, Type and Date stay whole.
        let fitted = fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 1000.0);
        assert_eq!(fitted[1..3], [110.0, 70.0], "{fitted:?}");
        assert!((3..6).all(|c| fitted[c] >= SMALL_MIN && fitted[c] < content[c]), "{fitted:?}");
        assert!(fitted[0] >= 300.0, "{fitted:?}");
    }

    #[test]
    fn auto_fit_fits_when_first_shown_and_again_only_when_the_room_or_the_content_change() {
        let content = [200.0, 100.0, 140.0, 50.0, 50.0, 60.0];
        let measured = std::cell::Cell::new(0);
        let measure = |widths: Widths| {
            let measured = &measured;
            move || {
                measured.set(measured.get() + 1);
                widths
            }
        };
        let mut auto = AutoFit::default();
        let first = auto.fit(1, 900.0, &ALL_SHOWN, &LIST_ROLES, measure(content));
        assert_eq!(first, Some(fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 900.0)), "first shown");
        assert_eq!(auto.fit(1, 900.0, &ALL_SHOWN, &LIST_ROLES, measure(content)), None, "nothing changed");
        assert_eq!(measured.get(), 1);
        let narrower = auto.fit(1, 700.0, &ALL_SHOWN, &LIST_ROLES, measure(content));
        assert_eq!(narrower, Some(fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 700.0)), "the window was resized");
        assert_eq!(measured.get(), 1, "a resize does not measure the content again");
        let more = [300.0; COLUMNS.len()];
        let changed = auto.fit(2, 700.0, &ALL_SHOWN, &LIST_ROLES, measure(more));
        assert_eq!(changed, Some(fit_all(&more, &ALL_SHOWN, &LIST_ROLES, 700.0)), "the rows changed");
        assert_eq!(measured.get(), 2);
    }

    #[test]
    fn auto_fit_leaves_widths_set_by_hand_alone_until_fit_all_columns() {
        let content = [200.0, 100.0, 140.0, 50.0, 50.0, 60.0];
        let mut auto = AutoFit::default();
        assert!(auto.fit(1, 900.0, &ALL_SHOWN, &LIST_ROLES, || content).is_some());
        auto.by_hand();
        assert_eq!(auto.fit(1, 700.0, &ALL_SHOWN, &LIST_ROLES, || content), None, "resized after a drag");
        assert_eq!(auto.fit(2, 700.0, &ALL_SHOWN, &LIST_ROLES, || content), None, "and new rows");
        auto.auto();
        assert_eq!(auto.fit(2, 700.0, &ALL_SHOWN, &LIST_ROLES, || content), Some(fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 700.0)));
    }

    #[test]
    fn the_room_of_a_table_is_its_width_less_the_gaps_between_shown_columns() {
        let ctx = egui::Context::default();
        let gap = ctx.global_style().spacing.item_spacing.x;
        assert_eq!(room(&ctx, 1080.0, &ALL_SHOWN), (1080.0 - 5.0 * gap).floor());
        assert_eq!(room(&ctx, 1080.0, &[true, false, false, false, true, false]), (1080.0 - gap).floor());
        assert_eq!(room(&ctx, 10.0, &ALL_SHOWN), 0.0, "never negative");
    }

    #[test]
    fn fit_all_with_a_very_long_name_keeps_type_and_date_whole_and_squeezes_the_small_columns() {
        let content = [1400.0, 110.0, 150.0, 60.0, 60.0, 70.0];
        let fitted = fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 1000.0);
        assert!((total(&fitted, &ALL_SHOWN) - 1000.0).abs() < 0.5, "adds up to the room: {fitted:?}");
        assert_eq!((fitted[1], fitted[2]), (110.0, 150.0), "Type and Date Modified whole: {fitted:?}");
        for c in 3..6 {
            assert!(fitted[c] >= SMALL_MIN && fitted[c] < content[c], "column {c} squeezed but readable: {fitted:?}");
        }
        assert!(fitted[0] > 600.0 && fitted[0] < 1400.0, "Name takes the rest: {fitted:?}");
    }

    #[test]
    fn fit_all_takes_from_the_small_columns_in_proportion_and_keeps_name_whole_for_a_little_overflow() {
        let content = [500.0, 110.0, 150.0, 60.0, 60.0, 70.0];
        let fitted = fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 900.0);
        assert!((total(&fitted, &ALL_SHOWN) - 900.0).abs() < 0.5, "{fitted:?}");
        assert!(fitted[0] >= 500.0, "Name is not cut while the small columns can give: {fitted:?}");
        assert_eq!((fitted[1], fitted[2]), (110.0, 150.0));
        assert!(fitted[3..].iter().all(|&w| w >= SMALL_MIN), "{fitted:?}");
        assert!(fitted[5] > fitted[3], "the wider column stays wider: {fitted:?}");
    }

    #[test]
    fn fit_all_in_a_very_narrow_list_takes_name_to_its_minimum_then_squeezes_type_and_date() {
        let content = [800.0, 150.0, 160.0, 60.0, 60.0, 60.0];
        let fitted = fit_all(&content, &ALL_SHOWN, &LIST_ROLES, 400.0);
        assert!((total(&fitted, &ALL_SHOWN) - 400.0).abs() < 0.5, "adds up to the room: {fitted:?}");
        assert!(fitted[0] >= NAME_MIN && fitted[0] < NAME_MIN + 3.0, "Name at its minimum: {fitted:?}");
        assert!(fitted[1] < 150.0 && fitted[2] < 160.0, "Type and Date give too: {fitted:?}");
        assert!(fitted.iter().all(|&w| w >= MIN_WIDTH), "{fitted:?}");
    }

    #[test]
    fn fit_all_ignores_hidden_columns() {
        let content = [1400.0, 110.0, 150.0, 900.0, 60.0, 900.0];
        let shown = [true, true, true, false, true, false];
        let fitted = fit_all(&content, &shown, &LIST_ROLES, 700.0);
        assert!((total(&fitted, &shown) - 700.0).abs() < 0.5, "{fitted:?}");
        assert_eq!((fitted[3], fitted[5]), (900.0, 900.0), "hidden ones keep their fit width");
        assert_eq!((fitted[1], fitted[2]), (110.0, 150.0));
        assert!(fitted[0] > 380.0, "{fitted:?}");
        // With room to spare the shown ones share it in proportion; the hidden ones still keep theirs.
        let roomy = fit_all(&content, &shown, &LIST_ROLES, 3000.0);
        assert_eq!((roomy[3], roomy[5]), (900.0, 900.0));
        assert!((total(&roomy, &shown) - 3000.0).abs() < 0.5, "{roomy:?}");
        for c in [0, 1, 2, 4] {
            assert!((roomy[c] / content[c] - 3000.0 / 1720.0).abs() < 0.02, "column {c} grows with the rest: {roomy:?}");
        }
    }

    #[test]
    fn the_visible_width_is_the_lists_less_the_gaps_between_shown_columns() {
        let ctx = egui::Context::default();
        assert_eq!(visible_width(&ctx, &ALL_SHOWN), None, "not drawn yet");
        remember_width(&ctx, 1000.0);
        let gap = ctx.global_style().spacing.item_spacing.x;
        assert_eq!(visible_width(&ctx, &ALL_SHOWN), Some((1000.0 - 5.0 * gap).floor()));
        let two = [true, false, false, false, true, false];
        assert_eq!(visible_width(&ctx, &two), Some((1000.0 - gap).floor()));
    }

    #[test]
    fn the_list_remembers_its_width_and_columns_fitted_to_it_leave_nothing_off_screen() {
        let ctx = egui::Context::default();
        let long = format!("{}.txt", "A folder with a name that goes on and on ".repeat(5));
        let view = listed(&["a.txt", &long]);
        draw_list(&ctx, &view, &DEFAULT_WIDTHS, vec![]);
        let room = visible_width(&ctx, &ALL_SHOWN).expect("drawn");
        assert!(room > 600.0 && room < crate::ui::testing::SCREEN.width(), "{room}");
        let mut content = [0.0; COLUMNS.len()];
        for (c, width) in content.iter_mut().enumerate() {
            *width = fit_width(&ctx, &view, c);
        }
        assert!(content.iter().sum::<f32>() > room, "the columns together overflow: {content:?}");
        let fitted = fit_all(&content, &ALL_SHOWN, &LIST_ROLES, room);
        let mut lefts = Vec::new();
        for _ in 0..3 {
            lefts = draw_list(&ctx, &view, &fitted, vec![]).lefts;
        }
        assert!(lefts.iter().all(Option::is_some), "every header in view: {lefts:?}");
        let (screen, last) = (crate::ui::testing::SCREEN.right(), lefts[5].unwrap());
        assert!(last < screen, "Size starts inside the window: {last} < {screen}");
        // No sideways scroll: the columns plus the gaps are no wider than the list.
        let list = ctx.data(|d| d.get_temp::<f32>(list_width_id())).unwrap();
        let gaps = 5.0 * ctx.global_style().spacing.item_spacing.x;
        assert!(fitted.iter().sum::<f32>() + gaps <= list + 0.5, "{fitted:?} in {list}");
    }

    /// Files last: the last column stretches to the list's edge, so Size sits before it to be cut.
    const SIZE_BEFORE_FILES: Order = [0, 1, 2, 3, 5, 4];

    /// The characters painted for the first text starting like `start`, with the columns `widths` wide.
    fn painted_like(view: &View, widths: Widths, order: Order, start: &str) -> String {
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() }, |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    show(ui, view, &[], Sizes { widths, order, ..Sizes::default() }, None, None, &FileIcons::default());
                });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().starts_with(start) => {
                    Some(t.galley.rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.chr)).collect())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{start} painted"))
    }

    #[test]
    fn headers_and_cells_that_do_not_fit_their_column_end_in_an_ellipsis() {
        let mut view = listed(&["a.txt"]);
        view.entries[0].modified = Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000));
        view.entries[0].size = Some(1_234_567_890);
        let narrow = [200.0, 110.0, 50.0, 44.0, 44.0, 30.0];
        assert!(painted_like(&view, narrow, SIZE_BEFORE_FILES, "Folders").ends_with('…'), "header cut");
        assert!(painted_like(&view, narrow, SIZE_BEFORE_FILES, "2023").ends_with('…'), "date cut");
        assert!(painted_like(&view, narrow, SIZE_BEFORE_FILES, "1.1").ends_with('…'), "size cut");
        let roomy = [200.0, 110.0, 150.0, 90.0, 90.0, 90.0];
        assert_eq!(painted_like(&view, roomy, SIZE_BEFORE_FILES, "Folders").trim_end(), "Folders");
        assert_eq!(painted_like(&view, roomy, SIZE_BEFORE_FILES, "2023").len(), "2023-11-14 22:13".len(), "date whole");
    }
}
