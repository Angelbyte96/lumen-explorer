//! The List view: icon and name only, in columns filled top to bottom that
//! scroll sideways. Folders fill the first columns and the files start a
//! column of their own, like File Pilot. Only the columns on screen are drawn.

use std::path::PathBuf;

use eframe::egui::{self, Rect, vec2};

use crate::app::{Scroll, View};
use crate::ui::dnd;
use crate::ui::file_icons::FileIcons;
use crate::ui::file_list::{self, ListEvent, RenameEdit};
use crate::ui::grid::GridShape;
use crate::ui::icon_grid::{GridOut, cell_band, cell_event, paint_cell};
use crate::ui::icons;

/// A column's width, in points.
pub const COLUMN: f32 = 280.0;
const GAP: f32 = 8.0;

/// How many cells fit down `height`, one `pitch` each.
pub fn per_column(height: f32, pitch: f32) -> usize {
    ((height / pitch).floor() as usize).max(1)
}

pub fn show(
    ui: &mut egui::Ui,
    view: &View,
    cut: &[PathBuf],
    scroll: Option<Scroll>,
    mut edit: Option<&mut RenameEdit>,
    file_icons: &FileIcons,
    overscroll: bool,
) -> GridOut {
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let cell = vec2(COLUMN, text_height + 8.0);
    let pitch = cell.y + 2.0;
    let whole = ui.available_rect_before_wrap();
    file_list::focus_stop(ui, whole);
    // Room left for the sideways scroll bar.
    let room = whole.height() - 2.0 * GAP - ui.spacing().scroll.bar_width - ui.spacing().scroll.bar_outer_margin;
    let shape = GridShape::new(per_column(room, pitch), view.folder_count(), view.rows.len());
    let step = COLUMN + 2.0 * GAP;
    let mut event = None;
    ui.ctx().data_mut(|d| d.remove::<Rect>(file_list::cursor_row_id()));

    egui::ScrollArea::horizontal().id_salt("lumen_list_view").auto_shrink(false).show_viewport(ui, |ui, viewport| {
        // Scroll Beyond Last Item: room to bring the last column to the left edge.
        let beyond = if overscroll && shape.total > 0 { (whole.width() - step).max(0.0) } else { 0.0 };
        let width = shape.lines() as f32 * step + GAP + beyond;
        let (content, background) = ui.allocate_exact_size(vec2(width.max(viewport.width()), ui.available_height()), egui::Sense::click());
        let origin = content.min + vec2(GAP, GAP);
        let place = |i: usize| {
            let (column, at) = shape.cell(i);
            Rect::from_min_size(origin + vec2(column as f32 * step, at as f32 * pitch), cell)
        };
        if scroll.is_some()
            && let Some(sel) = view.selected.filter(|s| *s < shape.total)
        {
            ui.scroll_to_rect(place(sel).expand(GAP), None);
        }
        let first = ((viewport.min.x - GAP) / step).floor().max(0.0) as usize;
        let last = ((viewport.max.x / step).ceil() as usize).min(shape.lines());
        let (clip, mut covered) = (ui.clip_rect(), false);
        for column in first..last {
            for i in shape.line(column) {
                let entry = view.entry(view.rows[i]);
                let rect = place(i);
                let over = dnd::over(ui.ctx(), clip, rect, entry.is_dir.then_some(entry.path.as_path()));
                covered |= over.inside && entry.is_dir;
                let response = ui.interact(rect, egui::Id::new(("lumen_list_cell", i)), egui::Sense::click_and_drag());
                if !edit.as_deref().is_some_and(|e| e.path == entry.path) {
                    dnd::start(&response, || dnd::carry(view, i));
                }
                paint_cell(ui, view, i, rect, &response);
                if over.lit {
                    ui.painter().add(dnd::lit_shape(ui.ctx(), rect));
                }
                let inner = rect.shrink2(vec2(6.0, 0.0));
                let mut grey_folder = None;
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    if cut.contains(&entry.path) {
                        ui.set_opacity(file_list::CUT_OPACITY);
                    }
                    icons::show(ui, entry, text_height, file_icons.texture(entry));
                    if let Some(edit) = edit.as_deref_mut().filter(|e| e.path == entry.path) {
                        if let Some(submit) = file_list::rename_field(ui, edit) {
                            event = Some(ListEvent::RenameEnded(submit));
                        }
                    } else {
                        file_list::name_label(ui, entry, view.filter());
                        grey_folder = file_list::folder_label(ui, view.relative_parent(entry));
                    }
                });
                if file_list::opens_folder(&response, grey_folder) {
                    event = Some(ListEvent::OpenFolder(i));
                } else if let Some(clicked) = cell_event(&response, i) {
                    event = Some(clicked);
                }
            }
        }
        if !covered {
            dnd::background(ui, content, &view.current);
        }
        dnd::edge_scroll(ui, true);
        let may_start = ui.input(|i| i.pointer.primary_pressed()) && background.is_pointer_button_down_on();
        if let Some(band) = cell_band(ui, content, may_start, &shape, place, true) {
            event = Some(band);
        }
        if event.is_none() {
            if background.clicked() {
                event = Some(ListEvent::Empty { menu: false });
            } else if background.secondary_clicked() {
                event = Some(ListEvent::Empty { menu: true });
            }
        }
    });
    if ui.memory(|m| m.has_focus(file_list::focus_id())) {
        ui.painter().rect_stroke(whole.shrink(1.0), 2, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
    }
    GridOut { event, shape }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Entry;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry { name: name.into(), path: PathBuf::from("/x").join(name), is_dir, size: Some(1), modified: None, hidden: false }
    }

    fn listed(dirs: &[&str], files: &[String]) -> View {
        let mut view = View::default();
        view.entries = dirs.iter().map(|n| entry(n, true)).chain(files.iter().map(|n| entry(n, false))).collect();
        view.rows = (0..view.entries.len()).collect();
        view
    }

    /// The filled rectangles painted.
    type Fills = Vec<(Rect, egui::Color32)>;

    fn draw(ctx: &egui::Context, view: &View, events: Vec<egui::Event>) -> (GridOut, Vec<(String, Rect)>) {
        let (out, texts, _) = draw_shapes(ctx, view, events, Default::default());
        (out, texts)
    }

    /// `draw`, holding `modifiers`, with the filled rectangles painted too.
    fn draw_shapes(ctx: &egui::Context, view: &View, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> (GridOut, Vec<(String, Rect)>, Fills) {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 300.0));
        // The keys held stay held until a frame says otherwise.
        let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut out = None;
        let mut output = ctx.run_ui(input, |ui| {
            out = Some(show(ui, view, &[], None, None, &FileIcons::default(), false));
            crate::ui::dnd::finish(ui.ctx());
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect();
        let fills = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill != egui::Color32::TRANSPARENT => Some((r.rect, r.fill)),
                _ => None,
            })
            .collect();
        (out.unwrap(), texts, fills)
    }

    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        texts.iter().find(|(t, _)| t == text).unwrap_or_else(|| panic!("{text:?} in {texts:?}")).1
    }

    #[test]
    fn the_sideways_scroll_bar_stays_above_the_room_kept_for_the_islands() {
        let ctx = egui::Context::default();
        let files: Vec<String> = (0..200).map(|n| format!("file-{n:03}.txt")).collect();
        let view = listed(&[], &files);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 300.0));
        let room = 40.0;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut output = ctx.run_ui(input, |root| {
                egui::CentralPanel::default_margins().show(root, |ui| {
                    crate::ui::status_bar::above_islands(ui, room, |ui| {
                        show(ui, &view, &[], None, None, &FileIcons::default(), false);
                    });
                });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let low: Vec<Rect> = shapes.iter().map(|s| s.shape.visual_bounding_rect()).filter(|r| r.is_finite() && *r != screen && r.max.y > screen.max.y - room + 0.5).collect();
        assert!(low.is_empty(), "nothing drawn in the islands' strip: {low:?}");
    }

    #[test]
    fn a_column_fits_the_height_and_never_holds_less_than_one() {
        assert_eq!(per_column(240.0, 24.0), 10);
        assert_eq!(per_column(5.0, 24.0), 1);
    }

    #[test]
    fn folders_fill_their_own_column_and_files_go_down_then_over() {
        let ctx = egui::Context::default();
        let files: Vec<String> = (1..=20).map(|n| format!("{n:02}.png")).collect();
        let view = listed(&["test"], &files);
        draw(&ctx, &view, vec![]);
        let (out, texts) = draw(&ctx, &view, vec![]);
        let per = out.shape.per_line;
        assert!((3..20).contains(&per), "a 300-point window holds a few cells a column: {per}");
        let (folder, first, second) = (find(&texts, "test"), find(&texts, "01.png"), find(&texts, "02.png"));
        assert!(first.left() > folder.right(), "files start a column right of the folder's");
        assert!((first.top() - folder.top()).abs() < 1.0, "both columns start at the top");
        assert!(second.top() > first.bottom() && (second.left() - first.left()).abs() < 1.0, "down the column");
        let next = find(&texts, &format!("{:02}.png", per + 1));
        assert!(next.left() > first.right() && (next.top() - first.top()).abs() < 1.0, "the next column starts at the top");
    }

    #[test]
    fn a_drag_from_beside_the_cells_picks_the_cells_it_touches() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["a.txt".into(), "b.txt".into(), "c.txt".into()]);
        draw(&ctx, &view, vec![]);
        let (_, texts) = draw(&ctx, &view, vec![]);
        let (a, b) = (find(&texts, "a.txt"), find(&texts, "b.txt"));
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        // From the empty column on the right, left and up over b.txt and a.txt.
        let (from, to) = (egui::pos2(700.0, b.center().y), a.center());
        let mut last = None;
        for events in [vec![egui::Event::PointerMoved(from), button(from, true)], vec![egui::Event::PointerMoved(from + (to - from) * 0.1)], vec![egui::Event::PointerMoved(to)]] {
            last = draw(&ctx, &view, events).0.event;
        }
        draw(&ctx, &view, vec![button(to, false)]);
        assert!(matches!(last, Some(ListEvent::Cells { ref cells, cursor: Some(0), .. }) if cells == &[0, 1]), "a and b, cursor on a");
    }

    fn carried(ctx: &egui::Context) -> Option<Vec<PathBuf>> {
        egui::DragAndDrop::payload::<crate::drag::Drag>(ctx).map(|drag| drag.paths.clone())
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    /// Presses at `from` and drags to `to`, holding the button; what the last frame drew.
    fn lift(ctx: &egui::Context, view: &View, from: egui::Pos2, to: egui::Pos2) -> (GridOut, Vec<(String, Rect)>) {
        draw(ctx, view, vec![egui::Event::PointerMoved(from), button(from, true)]);
        draw(ctx, view, vec![egui::Event::PointerMoved(from + (to - from) * 0.1)]);
        draw(ctx, view, vec![egui::Event::PointerMoved(to)]);
        // The label is laid out the frame it first shows.
        draw(ctx, view, vec![])
    }

    fn settled_texts(ctx: &egui::Context, view: &View) -> Vec<(String, Rect)> {
        draw(ctx, view, vec![]);
        draw(ctx, view, vec![]).1
    }

    #[test]
    fn a_drag_from_a_cell_carries_the_entry_or_the_selection_it_is_in() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt".into(), "b.txt".into(), "c.txt".into()]);
        let texts = settled_texts(&ctx, &view);
        let (a, c) = (find(&texts, "a.txt").center(), find(&texts, "c.txt").center());
        let away = egui::pos2(700.0, 200.0);
        let paths = |names: &[&str]| names.iter().map(|n| PathBuf::from("/x").join(n)).collect::<Vec<_>>();

        lift(&ctx, &view, a, away);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt"])), "nothing selected: the cell under the pointer");
        let (_, texts) = draw(&ctx, &view, vec![button(away, false)]);
        assert_eq!(carried(&ctx), None, "let go: nothing is carried any more");
        assert!(!texts.iter().any(|(t, _)| t.contains("item")), "and no label: {texts:?}");

        view.select_row(3);
        view.toggle_pick(1);
        lift(&ctx, &view, c, away);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt", "c.txt"])), "picked: the selection in display order");
        draw(&ctx, &view, vec![button(away, false)]);

        view.select_row(2);
        lift(&ctx, &view, a, away);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt"])), "not in the selection: alone");
    }

    #[test]
    fn a_label_by_the_pointer_counts_what_is_carried() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt".into(), "b.txt".into()]);
        view.current = PathBuf::from("/x");
        let texts = settled_texts(&ctx, &view);
        // Over a file cell the folder on screen takes the drop: the label counts and names it.
        let to = find(&texts, "b.txt").center();
        let (_, one) = lift(&ctx, &view, find(&texts, "a.txt").center(), to);
        let label = find(&one, "Copy 1 item to x");
        assert!(label.min.x > to.x && label.min.x < to.x + 40.0 && label.min.y > to.y && label.min.y < to.y + 40.0, "by the pointer: {label:?}");
        draw(&ctx, &view, vec![button(to, false)]);
        view.select_row(1);
        view.extend_to(2);
        let (_, two) = lift(&ctx, &view, find(&texts, "a.txt").center(), to);
        find(&two, "Copy 2 items to x");
    }

    #[test]
    fn a_click_on_a_nested_cells_grey_folder_opens_that_folder() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let listing = |entries| crate::fs::Listing { id: 1, path: PathBuf::from("/x"), result: Ok(entries), elapsed: std::time::Duration::ZERO };
        view.apply_listing(listing(vec![entry("src", true), entry("a.txt", false)]), None);
        view.set_flat(vec![PathBuf::from("/x")]);
        let main = Entry { path: PathBuf::from("/x/src/main.rs"), ..entry("main.rs", false) };
        view.apply_flat(&[PathBuf::from("/x")], vec![main]);
        let row = view.rows.iter().position(|&i| view.entry(i).name == "main.rs").unwrap();
        let texts = settled_texts(&ctx, &view);
        let click = |pos| vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)];
        let (out, _) = draw(&ctx, &view, click(find(&texts, "> src").center()));
        assert!(matches!(out.event, Some(ListEvent::OpenFolder(r)) if r == row), "the grey folder opens it: {:?}", out.event);
        draw(&ctx, &view, vec![]);
        let (out, _) = draw(&ctx, &view, click(find(&texts, "main.rs").center()));
        assert!(matches!(out.event, Some(ListEvent::Select(r)) if r == row), "the name still selects: {:?}", out.event);
        let grey = find(&texts, "> src").center();
        let with = |modifiers| {
            draw_shapes(&ctx, &view, vec![], modifiers);
            let event = draw_shapes(&ctx, &view, click(grey), modifiers).0.event;
            draw(&ctx, &view, vec![]);
            event
        };
        let toggled = with(egui::Modifiers::COMMAND);
        assert!(matches!(toggled, Some(ListEvent::Toggle(r)) if r == row), "Ctrl+click picks the cell: {toggled:?}");
        let extended = with(egui::Modifiers::SHIFT);
        assert!(matches!(extended, Some(ListEvent::Extend(r)) if r == row), "Shift+click extends: {extended:?}");
        let right = |pressed| egui::Event::PointerButton { pos: grey, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() };
        let (out, _) = draw(&ctx, &view, vec![egui::Event::PointerMoved(grey), right(true), right(false)]);
        assert!(matches!(out.event, Some(ListEvent::Menu(r)) if r == row), "a right click opens the cell's menu: {:?}", out.event);
    }

    #[test]
    fn a_click_or_a_band_from_the_empty_column_carries_nothing_and_still_works() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["a.txt".into(), "b.txt".into(), "c.txt".into()]);
        let texts = settled_texts(&ctx, &view);
        let (a, b) = (find(&texts, "a.txt"), find(&texts, "b.txt"));
        let click = |pos| vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)];
        let (out, _) = draw(&ctx, &view, click(a.center()));
        assert!(matches!(out.event, Some(ListEvent::Select(0))), "a click on a cell selects it");
        assert_eq!(carried(&ctx), None);
        draw(&ctx, &view, vec![]);
        let (out, _) = lift(&ctx, &view, egui::pos2(700.0, b.center().y), a.center());
        assert!(matches!(out.event, Some(ListEvent::Cells { ref cells, .. }) if cells == &[0, 1]), "a band from the empty column still picks");
        assert_eq!(carried(&ctx), None, "and carries nothing");
    }

    const SHIFT: egui::Modifiers = egui::Modifiers { shift: true, ..egui::Modifiers::NONE };

    fn lit(ctx: &egui::Context, fills: &Fills, at: egui::Pos2) -> bool {
        let fill = crate::ui::dnd::lit_fill(&ctx.global_style().visuals);
        fills.iter().any(|(rect, f)| *f == fill && rect.contains(at))
    }

    fn let_go(ctx: &egui::Context, view: &View, at: egui::Pos2, modifiers: egui::Modifiers) -> Option<(Vec<PathBuf>, PathBuf)> {
        draw_shapes(ctx, view, vec![button(at, false)], modifiers);
        crate::ui::dnd::take_drop(ctx).map(|d| (d.drag.paths, d.target))
    }

    #[test]
    fn a_dragged_cell_over_a_folder_cell_lights_it_names_the_action_and_drops_on_it() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt".into(), "b.txt".into()]);
        view.current = PathBuf::from("/x");
        let texts = settled_texts(&ctx, &view);
        let (folder, a, b) = (find(&texts, "test").center(), find(&texts, "a.txt").center(), find(&texts, "b.txt").center());
        draw(&ctx, &view, vec![egui::Event::PointerMoved(a), button(a, true)]);
        draw(&ctx, &view, vec![egui::Event::PointerMoved(a + vec2(5.0, 5.0))]);
        let (_, texts, fills) = draw_shapes(&ctx, &view, vec![egui::Event::PointerMoved(folder)], SHIFT);
        assert!(lit(&ctx, &fills, folder), "the folder lights up: {fills:?}");
        assert!(!lit(&ctx, &fills, a), "and only it");
        find(&texts, "Move 1 item to test");
        let (_, texts, _) = draw_shapes(&ctx, &view, vec![], Default::default());
        find(&texts, "Copy 1 item to test");
        assert_eq!(let_go(&ctx, &view, folder, SHIFT), Some((vec![PathBuf::from("/x/a.txt")], PathBuf::from("/x/test"))));

        // A file cell is no folder: the folder on screen takes the drop, and no cell lights up.
        draw(&ctx, &view, vec![egui::Event::PointerMoved(a), button(a, true)]);
        draw(&ctx, &view, vec![egui::Event::PointerMoved(a + vec2(5.0, 5.0))]);
        let (_, texts, fills) = draw_shapes(&ctx, &view, vec![egui::Event::PointerMoved(b)], Default::default());
        assert!(!lit(&ctx, &fills, b));
        find(&texts, "Copy 1 item to x");
        assert_eq!(let_go(&ctx, &view, b, Default::default()), Some((vec![PathBuf::from("/x/a.txt")], PathBuf::from("/x"))));
    }

    #[test]
    fn a_file_cell_and_the_gap_between_cells_drop_into_the_folder_on_screen() {
        let ctx = egui::Context::default();
        let mut view = listed(&[], &["a.txt".into(), "b.txt".into(), "c.txt".into()]);
        view.current = PathBuf::from("/x");
        let texts = settled_texts(&ctx, &view);
        let (a, b) = (find(&texts, "a.txt").center(), find(&texts, "b.txt").center());
        // Between two cells of a column: no cell, not the background either.
        let gap = egui::pos2(a.x, (a.y + b.y) / 2.0);
        for over in [b, gap] {
            draw(&ctx, &view, vec![egui::Event::PointerMoved(a), button(a, true)]);
            draw(&ctx, &view, vec![egui::Event::PointerMoved(a + vec2(5.0, 5.0))]);
            let (_, texts, fills) = draw_shapes(&ctx, &view, vec![egui::Event::PointerMoved(over)], Default::default());
            assert!(!lit(&ctx, &fills, over), "no cell lights up");
            find(&texts, "Copy 1 item to x");
            assert_eq!(let_go(&ctx, &view, over, Default::default()), Some((vec![PathBuf::from("/x/a.txt")], PathBuf::from("/x"))));
        }
    }

    #[test]
    fn a_drag_from_another_app_over_a_file_cell_or_a_gap_names_the_folder_on_screen_and_drops_there() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt".into(), "b.txt".into()]);
        view.current = PathBuf::from("/x");
        let texts = settled_texts(&ctx, &view);
        let (folder, a, b) = (find(&texts, "test").center(), find(&texts, "a.txt").center(), find(&texts, "b.txt").center());
        let gap = egui::pos2(a.x, (a.y + b.y) / 2.0);
        let hold = |at, released| {
            let drag = crate::drag::Drag { paths: vec![PathBuf::from("/x/a.txt")] };
            crate::ui::dnd::set_outside(&ctx, Some(crate::ui::dnd::Outside { drag, at, keys: Default::default(), released }));
            draw_shapes(&ctx, &view, vec![], Default::default());
            draw_shapes(&ctx, &view, vec![], Default::default())
        };
        // A folder cell takes it itself; a file cell and a gap fall to the folder on screen.
        let (_, texts, fills) = hold(folder, false);
        find(&texts, "Copy 1 item to test");
        assert!(lit(&ctx, &fills, folder));
        for over in [b, gap] {
            let (_, texts, fills) = hold(over, false);
            assert!(!lit(&ctx, &fills, over));
            find(&texts, "Copy 1 item to x");
            hold(over, true);
            assert_eq!(crate::ui::dnd::take_drop(&ctx).map(|d| d.target), Some(PathBuf::from("/x")));
        }
        crate::ui::dnd::set_outside(&ctx, None);
    }

    #[test]
    fn letting_go_on_the_empty_space_drops_into_the_folder_on_screen() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt".into()]);
        view.current = PathBuf::from("/x");
        let texts = settled_texts(&ctx, &view);
        let a = find(&texts, "a.txt").center();
        let space = egui::pos2(700.0, 200.0);
        draw(&ctx, &view, vec![egui::Event::PointerMoved(a), button(a, true)]);
        draw(&ctx, &view, vec![egui::Event::PointerMoved(a + vec2(5.0, 5.0))]);
        let (_, texts, _) = draw_shapes(&ctx, &view, vec![egui::Event::PointerMoved(space)], Default::default());
        find(&texts, "Copy 1 item to x");
        assert_eq!(let_go(&ctx, &view, space, Default::default()), Some((vec![PathBuf::from("/x/a.txt")], PathBuf::from("/x"))));
    }

    #[test]
    fn held_near_the_left_or_right_edge_a_carried_drag_scrolls_the_columns() {
        let ctx = egui::Context::default();
        let files: Vec<String> = (0..200).map(|n| format!("f{n:03}.txt")).collect();
        let view = listed(&[], &files);
        let texts = settled_texts(&ctx, &view);
        let from = find(&texts, "f000.txt").center();
        let second_column = find(&texts, "f012.txt").min.x;
        draw(&ctx, &view, vec![egui::Event::PointerMoved(from), button(from, true)]);
        draw(&ctx, &view, vec![egui::Event::PointerMoved(from + vec2(5.0, 5.0))]);
        let middle = egui::pos2(400.0, 150.0);
        draw(&ctx, &view, vec![egui::Event::PointerMoved(middle)]);
        for _ in 0..5 {
            draw(&ctx, &view, vec![]);
        }
        assert_eq!(find(&draw(&ctx, &view, vec![]).1, "f012.txt").min.x, second_column, "held in the middle: no scroll");

        draw(&ctx, &view, vec![egui::Event::PointerMoved(egui::pos2(790.0, 150.0))]);
        let mut lefts = Vec::new();
        for _ in 0..5 {
            lefts.push(find(&draw(&ctx, &view, vec![]).1, "f012.txt").min.x);
        }
        assert!(lefts.windows(2).all(|w| w[1] < w[0]) && lefts[4] < second_column - 20.0, "the columns come in from the right while held: {lefts:?}");
    }

    #[test]
    fn a_click_on_a_cell_selects_it_and_a_click_beside_the_cells_is_empty() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt".into()]);
        draw(&ctx, &view, vec![]);
        let (_, texts) = draw(&ctx, &view, vec![]);
        let click = |pos| {
            let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            vec![egui::Event::PointerMoved(pos), press(true), press(false)]
        };
        let (out, _) = draw(&ctx, &view, click(find(&texts, "a.txt").center()));
        assert!(matches!(out.event, Some(ListEvent::Select(1))));
        draw(&ctx, &view, vec![]);
        let (out, _) = draw(&ctx, &view, click(egui::pos2(780.0, 200.0)));
        assert!(matches!(out.event, Some(ListEvent::Empty { menu: false })));
    }

    /// Where the last cell's left ends up after scrolling all the way across a long folder.
    fn last_cell_left_after_scrolling_across(overscroll: bool) -> f32 {
        let ctx = egui::Context::default();
        let files: Vec<String> = (0..200).map(|n| format!("file-{n:03}.txt")).collect();
        let view = listed(&[], &files);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 300.0));
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(-100_000.0, 0.0), modifiers: Default::default(), phase: egui::TouchPhase::Move };
        let mut left = f32::NAN;
        for (n, events) in std::iter::once(vec![egui::Event::PointerMoved(screen.center())]).chain(std::iter::once(vec![wheel])).chain((0..40).map(|_| vec![])).enumerate() {
            let input = egui::RawInput { screen_rect: Some(screen), events, time: Some(n as f64 / 60.0), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                show(ui, &view, &[], None, None, &FileIcons::default(), overscroll);
            });
            output.textures_delta.clear();
            left = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "file-199.txt" => Some(t.pos.x),
                    _ => None,
                })
                .unwrap_or(f32::NAN);
        }
        left
    }

    #[test]
    fn the_list_stops_at_the_last_column_unless_scroll_beyond_last_item_is_on() {
        let off = last_cell_left_after_scrolling_across(false);
        assert!(off > 800.0 - COLUMN - 2.0 * GAP - 40.0, "the last column ends at the right edge: {off}");
        let on = last_cell_left_after_scrolling_across(true);
        assert!(on < 100.0, "the last column can be scrolled to the left edge: {on}");
    }
}
