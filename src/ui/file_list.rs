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
    /// A click below the rows; `menu` for a right click.
    Empty { menu: bool },
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

/// Draws `view`'s rows, icons and rows sized by `zoom`; `scroll` brings the
/// selection into view.
pub fn show(
    ui: &mut egui::Ui,
    view: &View,
    cut: &[PathBuf],
    zoom: u8,
    scroll: Option<Scroll>,
    mut edit: Option<&mut RenameEdit>,
    file_icons: &FileIcons,
) -> Option<ListEvent> {
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

    let mut table = TableBuilder::new(ui)
        .striped(true)
        // Text sits in the middle of rows made taller by the zoom.
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
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
        .header(header_height, |mut header| {
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
                row.set_selected(if multi { view.is_picked(i) } else { selected == Some(i) });
                row.col(|ui| {
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
                            return;
                        }
                        name_label(ui, entry, view.filter());
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
    // Whatever space the rows leave below them.
    let rest = ui.available_rect_before_wrap();
    if event.is_none() && rest.height() > 0.0 {
        let response = ui.interact(rest, egui::Id::new("lumen_list_empty"), egui::Sense::click());
        if response.clicked() {
            event = Some(ListEvent::Empty { menu: false });
        } else if response.secondary_clicked() {
            event = Some(ListEvent::Empty { menu: true });
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
    let visuals = ui.visuals();
    let normal = if entry.hidden { visuals.weak_text_color() } else { visuals.text_color() };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let job = marked_name(&entry.name, &ranges, normal, visuals.hyperlink_color, font);
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
                show(ui, &view, &[], zoom, None, None, &FileIcons::default());
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
                show(ui, &view, &[], 0, None, None, &icons);
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
                show(ui, &view, &[], 0, None, None, &FileIcons::default());
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
        let mut draw = |ui: &mut egui::Ui| show(ui, &view, &[], 0, None, None, &FileIcons::default());
        frame(&ctx, vec![egui::Event::PointerMoved(below)], &mut draw);
        let left = frame(&ctx, press(below, egui::PointerButton::Primary), &mut draw);
        assert!(matches!(left, Some(ListEvent::Empty { menu: false })), "left click");
        let right = frame(&ctx, press(below, egui::PointerButton::Secondary), &mut draw);
        assert!(matches!(right, Some(ListEvent::Empty { menu: true })), "right click");
    }

    /// How opaque the first letter of `name` was painted, with `cut` waiting to be pasted.
    fn name_opacity(name: &str, cut: &[PathBuf]) -> f32 {
        let ctx = egui::Context::default();
        let mut view = View::default();
        (view.entries, view.rows) = (vec![entry("a.txt"), entry("b.txt")], vec![0, 1]);
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &view, cut, 0, None, None, &FileIcons::default());
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
        frame(&ctx, vec![], |ui| show(ui, &view, &[], 0, None, None, &FileIcons::default()));
        let second = cursor_row_rect(&ctx).expect("drawn");
        view.selected = Some(0);
        frame(&ctx, vec![], |ui| show(ui, &view, &[], 0, None, None, &FileIcons::default()));
        let first = cursor_row_rect(&ctx).expect("drawn");
        assert!(first.bottom() <= second.top() + 1.0, "{first:?} above {second:?}");
        // A cursor row scrolled out of view was not drawn: no stale place.
        view.entries = (0..300).map(|i| entry(&format!("f{i}"))).collect();
        view.rows = (0..300).collect();
        view.selected = Some(250);
        frame(&ctx, vec![], |ui| show(ui, &view, &[], 0, None, None, &FileIcons::default()));
        assert_eq!(cursor_row_rect(&ctx), None);
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
                show(ui, &view, &[], 0, None, Some(&mut edit), &FileIcons::default());
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
