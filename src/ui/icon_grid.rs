//! The Icons view: a grid of cards (icon, name, size) whose icons grow from
//! M to XL with the view slider. Folders take their own rows first. Only the
//! rows on screen are drawn, so huge folders stay fast.

use std::path::PathBuf;

use eframe::egui::{self, Rect, vec2};

use crate::app::{Scroll, View};
use crate::format::format_size;
use crate::ui::file_icons::FileIcons;
use crate::ui::file_list::{self, ListEvent, RenameEdit};
use crate::ui::grid::GridShape;
use crate::ui::icons;

/// Icon side at M Icons and at XL Icons, in points.
pub const M_ICON: f32 = 48.0;
pub const XL_ICON: f32 = 224.0;
/// Air around a card's content and between cards.
const PAD: f32 = 10.0;
const GAP: f32 = 8.0;

/// The icon side for `growth` (0.0 at M Icons, 1.0 at XL Icons).
pub fn icon_side(growth: f32) -> f32 {
    M_ICON + (XL_ICON - M_ICON) * growth.clamp(0.0, 1.0)
}

/// A card's size: the icon with air around it, then the name and the size.
pub fn card_size(icon: f32, text_height: f32) -> egui::Vec2 {
    vec2(icon + 2.0 * PAD, PAD + icon + 6.0 + 2.0 * text_height + PAD)
}

/// How many cards fit across `width`.
pub fn per_row(width: f32, card: f32) -> usize {
    (((width + GAP) / (card + GAP)).floor() as usize).max(1)
}

/// What the grid did this frame and its shape, for the arrow keys.
pub struct GridOut {
    pub event: Option<ListEvent>,
    pub shape: GridShape,
}

pub fn show(
    ui: &mut egui::Ui,
    view: &View,
    cut: &[PathBuf],
    growth: f32,
    scroll: Option<Scroll>,
    mut edit: Option<&mut RenameEdit>,
    file_icons: &FileIcons,
) -> GridOut {
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let icon = icon_side(growth);
    let card = card_size(icon, text_height);
    let whole = ui.available_rect_before_wrap();
    file_list::focus_stop(ui, whole);
    let shape = GridShape::new(per_row(whole.width() - 2.0 * GAP, card.x), view.folder_count(), view.rows.len());
    let pitch = card.y + GAP;
    let mut event = None;
    ui.ctx().data_mut(|d| d.remove::<Rect>(file_list::cursor_row_id()));

    egui::ScrollArea::vertical().id_salt("lumen_icon_grid").auto_shrink(false).show_viewport(ui, |ui, viewport| {
        let height = shape.lines() as f32 * pitch + GAP;
        let (content, background) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(viewport.height())), egui::Sense::click());
        let origin = content.min + vec2(GAP, GAP);
        let place = |i: usize| {
            let (line, at) = shape.cell(i);
            Rect::from_min_size(origin + vec2(at as f32 * (card.x + GAP), line as f32 * pitch), card)
        };
        if let Some(_scroll) = scroll
            && let Some(sel) = view.selected.filter(|s| *s < shape.total)
        {
            ui.scroll_to_rect(place(sel).expand(GAP / 2.0), None);
        }
        let first = ((viewport.min.y - GAP) / pitch).floor().max(0.0) as usize;
        let last = ((viewport.max.y / pitch).ceil() as usize).min(shape.lines());
        for line in first..last {
            for i in shape.line(line) {
                let entry = view.entry(view.rows[i]);
                let rect = place(i);
                let response = ui.interact(rect, egui::Id::new(("lumen_card", i)), egui::Sense::click());
                paint_cell(ui, view, i, rect, &response);
                let inner = rect.shrink2(vec2(PAD / 2.0, PAD));
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
                    if cut.contains(&entry.path) {
                        ui.set_opacity(file_list::CUT_OPACITY);
                    }
                    icons::show(ui, entry, icon, file_icons.texture(entry));
                    ui.add_space(6.0);
                    if let Some(edit) = edit.as_deref_mut().filter(|e| e.path == entry.path) {
                        if let Some(submit) = file_list::rename_field(ui, edit) {
                            event = Some(ListEvent::RenameEnded(submit));
                        }
                    } else {
                        file_list::cell_name_label(ui, entry, view.filter());
                    }
                    let size = if entry.is_dir { view.totals.get(&entry.path).map(|t| t.bytes) } else { entry.size };
                    ui.add(egui::Label::new(egui::RichText::new(size.map(format_size).unwrap_or_default()).weak()).selectable(false));
                });
                if let Some(clicked) = cell_event(&response.on_hover_text(&entry.name), i) {
                    event = Some(clicked);
                }
            }
        }
        let may_start = ui.input(|i| i.pointer.primary_pressed()) && background.is_pointer_button_down_on();
        if let Some(band) = cell_band(ui, content, may_start, &shape, place, false) {
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

/// Where a band over the cells started, from the content's top left, so a
/// scroll does not move it.
#[derive(Clone, Copy)]
struct CellBand(egui::Vec2);

fn cell_band_id() -> egui::Id {
    egui::Id::new("lumen_cell_band")
}

/// The rubber band of the views laid out in lines: a drag that starts on the
/// space around the cells (`may_start`) draws a band and picks every cell of
/// `shape` it touches (`place` gives each one's rect on screen). Held near an
/// edge it scrolls: down/up in Icons, sideways in List (`sideways`).
pub(crate) fn cell_band(ui: &egui::Ui, content: Rect, may_start: bool, shape: &GridShape, place: impl Fn(usize) -> Rect, sideways: bool) -> Option<ListEvent> {
    let (pressed, origin, down, dragging, pointer, dt) = ui.input(|i| {
        (i.pointer.primary_pressed(), i.pointer.press_origin(), i.pointer.primary_down(), i.pointer.is_decidedly_dragging(), i.pointer.latest_pos(), i.stable_dt)
    });
    if pressed {
        let start = origin.filter(|_| may_start).map(|p| CellBand(p - content.min));
        ui.ctx().data_mut(|d| match start {
            Some(start) => {
                d.insert_temp(cell_band_id(), start);
            }
            None => d.remove::<CellBand>(cell_band_id()),
        });
    }
    let start = ui.ctx().data(|d| d.get_temp::<CellBand>(cell_band_id()))?;
    if !down {
        ui.ctx().data_mut(|d| d.remove::<CellBand>(cell_band_id()));
        return None;
    }
    let pointer = pointer.filter(|_| dragging)?;
    let clip = ui.clip_rect();
    let speed = if sideways { file_list::band_speed(pointer.x, clip.x_range()) } else { file_list::band_speed(pointer.y, clip.y_range()) };
    if speed != 0.0 {
        let delta = if sideways { vec2(speed * dt, 0.0) } else { vec2(0.0, speed * dt) };
        ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
        ui.ctx().request_repaint();
    }
    let band = Rect::from_two_pos(content.min + start.0, pointer);
    let selection = ui.visuals().selection;
    ui.painter().rect(band, 0.0, selection.bg_fill.gamma_multiply(0.3), selection.stroke, egui::StrokeKind::Inside);
    let cells: Vec<usize> = (0..shape.total).filter(|&i| place(i).intersects(band)).collect();
    let cursor = cells.iter().copied().find(|&i| place(i).contains(pointer)).or(cells.last().copied());
    Some(ListEvent::Cells { cells, cursor })
}

/// A cell's highlight: picked or the cursor, else hovered. The cursor's
/// place is kept for Shift+F10.
pub(crate) fn paint_cell(ui: &egui::Ui, view: &View, i: usize, rect: Rect, response: &egui::Response) {
    let selected = if view.picked_count() > 0 { view.is_picked(i) } else { view.selected == Some(i) };
    let visuals = ui.visuals();
    if selected {
        ui.painter().rect_filled(rect, 4.0, visuals.selection.bg_fill.gamma_multiply(0.35));
        ui.painter().rect_stroke(rect, 4.0, visuals.selection.stroke, egui::StrokeKind::Inside);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    if view.selected == Some(i) {
        ui.ctx().data_mut(|d| d.insert_temp(file_list::cursor_row_id(), rect));
    }
}

/// What a click on cell `i` means, like a click on a Details row.
pub(crate) fn cell_event(response: &egui::Response, i: usize) -> Option<ListEvent> {
    if response.secondary_clicked() {
        Some(ListEvent::Menu(i))
    } else if response.double_clicked() {
        Some(ListEvent::Activate(i))
    } else if response.clicked() {
        let m = response.ctx.input(|input| input.modifiers);
        Some(if m.command {
            ListEvent::Toggle(i)
        } else if m.shift {
            ListEvent::Extend(i)
        } else {
            ListEvent::Select(i)
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Entry;
    use eframe::egui::pos2;

    #[test]
    fn icons_grow_from_m_to_xl_and_cards_fit_their_row() {
        assert_eq!(icon_side(0.0), M_ICON);
        assert_eq!(icon_side(1.0), XL_ICON);
        assert_eq!(icon_side(0.5), (M_ICON + XL_ICON) / 2.0);
        assert_eq!(icon_side(3.0), XL_ICON, "never past XL");
        assert_eq!(per_row(500.0, 92.0), 5);
        assert_eq!(per_row(50.0, 92.0), 1, "a narrow window still shows one card a row");
    }

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry { name: name.into(), path: PathBuf::from("/x").join(name), is_dir, size: (!is_dir).then_some(2048), modified: None, hidden: false }
    }

    fn listed(dirs: &[&str], files: &[&str]) -> View {
        let mut view = View::default();
        view.entries = dirs.iter().map(|n| entry(n, true)).chain(files.iter().map(|n| entry(n, false))).collect();
        view.rows = (0..view.entries.len()).collect();
        view
    }

    struct Drawn {
        out: GridOut,
        texts: Vec<(String, Rect, bool)>,
    }

    fn draw(ctx: &egui::Context, view: &View, growth: f32, events: Vec<egui::Event>) -> Drawn {
        draw_at(ctx, view, growth, events, None)
    }

    fn draw_at(ctx: &egui::Context, view: &View, growth: f32, events: Vec<egui::Event>, time: Option<f64>) -> Drawn {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 480.0));
        let input = egui::RawInput { screen_rect: Some(screen), events, time, ..Default::default() };
        let mut out = None;
        let mut output = ctx.run_ui(input, |ui| out = Some(show(ui, view, &[], growth, None, None, &FileIcons::default())));
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()), t.galley.elided)),
                _ => None,
            })
            .collect();
        Drawn { out: out.unwrap(), texts }
    }

    fn settled(ctx: &egui::Context, view: &View, growth: f32) -> Drawn {
        draw(ctx, view, growth, vec![]);
        draw(ctx, view, growth, vec![])
    }

    fn at<'a>(drawn: &'a Drawn, text: &str) -> &'a (String, Rect, bool) {
        drawn.texts.iter().find(|(t, ..)| t.starts_with(text)).unwrap_or_else(|| panic!("{text:?} in {:?}", drawn.texts))
    }

    #[test]
    fn folders_take_their_own_row_and_each_card_shows_its_name_and_size() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["01 - Portada.png", "02 - Contenido.png"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (folder, file1, file2) = (at(&drawn, "test").1, at(&drawn, "01 - ").1, at(&drawn, "02 - ").1);
        assert!(file1.top() > folder.bottom(), "files start below the folder's row");
        assert!((file1.top() - file2.top()).abs() < 1.0 && file2.left() > file1.right(), "files side by side");
        assert!((file1.left() - folder.left()).abs() < 30.0, "both rows start at the left");
        assert!(drawn.texts.iter().filter(|(t, ..)| t == "2.0 KB").count() == 2, "file sizes: {:?}", drawn.texts);
        assert_eq!(drawn.out.shape, GridShape::new(drawn.out.shape.per_line, 1, 3));
        assert!(drawn.out.shape.per_line >= 3, "M cards are small: {}", drawn.out.shape.per_line);
    }

    #[test]
    fn at_m_a_long_name_is_cut_and_at_xl_it_is_whole_with_fewer_cards_a_row() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["07 - Contenido.png"]);
        assert!(settled(&ctx, &view, 0.0).texts.iter().any(|(t, _, elided)| t.starts_with("07 - ") && *elided), "cut at M");
        let xl = settled(&ctx, &view, 1.0);
        assert!(!at(&xl, "07 - ").2, "whole at XL");
        assert!(xl.out.shape.per_line < settled(&ctx, &view, 0.0).out.shape.per_line);
    }

    #[test]
    fn hovering_a_cut_name_shows_the_whole_name_once() {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|s| s.interaction.tooltip_delay = 0.0);
        let name = "verify-f4.J1fQ-a-rather-long-name.png";
        let view = listed(&[], &[name]);
        let cut = at(&settled(&ctx, &view, 0.0), "verify").1;
        draw_at(&ctx, &view, 0.0, vec![egui::Event::PointerMoved(cut.center())], Some(1.0));
        draw_at(&ctx, &view, 0.0, vec![], Some(2.0));
        let drawn = draw_at(&ctx, &view, 0.0, vec![], Some(3.0));
        let whole = drawn.texts.iter().filter(|(t, _, elided)| t == name && !elided).count();
        assert_eq!(whole, 1, "one tooltip: {:?}", drawn.texts);
    }

    fn click(pos: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
        let press = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(pos), press(true), press(false)]
    }

    /// Presses at `from`, drags to `to` and lets go; the events of each frame.
    fn drag(ctx: &egui::Context, view: &View, from: egui::Pos2, to: egui::Pos2) -> Vec<Option<ListEvent>> {
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let steps = vec![
            vec![egui::Event::PointerMoved(from), button(from, true)],
            vec![egui::Event::PointerMoved(from + (to - from) * 0.1)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ];
        steps.into_iter().map(|events| draw(ctx, view, 0.0, events).out.event).collect()
    }

    fn banded(event: &Option<ListEvent>) -> Option<Vec<usize>> {
        match event {
            Some(ListEvent::Cells { cells, .. }) => Some(cells.clone()),
            _ => None,
        }
    }

    #[test]
    fn a_drag_from_the_space_around_the_cards_picks_every_card_it_touches() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt", "b.txt", "c.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (a, b) = (at(&drawn, "a.txt").1, at(&drawn, "b.txt").1);
        // From the gap left of the first file card, down-right over a.txt and b.txt.
        let from = egui::pos2(a.left() - 30.0, a.top() - 70.0);
        let events = drag(&ctx, &view, from.max(egui::pos2(2.0, 2.0)), egui::pos2(b.center().x, b.center().y));
        assert_eq!(banded(&events[2]), Some(vec![1, 2]), "the folder row above is not reached: {:?}", events.iter().map(banded).collect::<Vec<_>>());
        assert!(matches!(&events[2], Some(ListEvent::Cells { cursor: Some(2), .. })), "the cursor goes to the card under the pointer");
        let events = drag(&ctx, &view, egui::pos2(630.0, 470.0), egui::pos2(2.0, 2.0));
        assert_eq!(banded(&events[2]), Some(vec![0, 1, 2, 3]), "a band over everything picks everything");
    }

    #[test]
    fn a_drag_that_starts_on_a_card_is_not_a_band() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["a.txt", "b.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let events = drag(&ctx, &view, at(&drawn, "a.txt").1.center(), egui::pos2(630.0, 470.0));
        assert!(events.iter().all(|e| banded(e).is_none()), "kept for dragging files later");
    }

    #[test]
    fn a_card_answers_clicks_like_a_row_and_the_space_around_them_is_empty() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let file = at(&drawn, "a.txt").1.center();
        let event = |events| draw(&ctx, &view, 0.0, events).out.event;
        assert!(matches!(event(click(file, egui::PointerButton::Primary)), Some(ListEvent::Select(1))));
        settled(&ctx, &view, 0.0);
        assert!(matches!(event(click(file, egui::PointerButton::Secondary)), Some(ListEvent::Menu(1))));
        settled(&ctx, &view, 0.0);
        assert!(matches!(event(click(pos2(600.0, 400.0), egui::PointerButton::Primary)), Some(ListEvent::Empty { menu: false })));
        draw_at(&ctx, &view, 0.0, click(file, egui::PointerButton::Primary), Some(10.0));
        let second = draw_at(&ctx, &view, 0.0, click(file, egui::PointerButton::Primary), Some(10.1)).out.event;
        assert!(matches!(second, Some(ListEvent::Activate(1))), "a double click opens it");
    }
}
