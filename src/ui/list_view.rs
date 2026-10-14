//! The List view: icon and name only, in columns filled top to bottom that
//! scroll sideways. Folders fill the first columns and the files start a
//! column of their own, like File Pilot. Only the columns on screen are drawn.

use std::path::PathBuf;

use eframe::egui::{self, Rect, vec2};

use crate::app::{Scroll, View};
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
        let width = shape.lines() as f32 * step + GAP;
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
        for column in first..last {
            for i in shape.line(column) {
                let entry = view.entry(view.rows[i]);
                let rect = place(i);
                let response = ui.interact(rect, egui::Id::new(("lumen_list_cell", i)), egui::Sense::click());
                paint_cell(ui, view, i, rect, &response);
                let inner = rect.shrink2(vec2(6.0, 0.0));
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
                    }
                });
                if let Some(clicked) = cell_event(&response, i) {
                    event = Some(clicked);
                }
            }
        }
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

    fn draw(ctx: &egui::Context, view: &View, events: Vec<egui::Event>) -> (GridOut, Vec<(String, Rect)>) {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 300.0));
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut out = None;
        let mut output = ctx.run_ui(input, |ui| out = Some(show(ui, view, &[], None, None, &FileIcons::default())));
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect();
        (out.unwrap(), texts)
    }

    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        texts.iter().find(|(t, _)| t == text).unwrap_or_else(|| panic!("{text:?} in {texts:?}")).1
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
        assert!(matches!(last, Some(ListEvent::Cells { ref cells, cursor: Some(0) }) if cells == &[0, 1]), "a and b, cursor on a");
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
}
