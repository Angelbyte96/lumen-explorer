//! The Icons view: a grid of cards (icon, name, size) whose icons grow from
//! M to XL with the view slider. Folders take their own rows first. Only the
//! rows on screen are drawn, so huge folders stay fast.

use std::path::PathBuf;

use eframe::egui::{self, Rect, vec2};

use crate::app::{Scroll, View};
use crate::format::format_size;
use crate::ui::dnd;
use crate::ui::file_icons::FileIcons;
use crate::ui::file_list::{self, ListEvent, RenameEdit};
use crate::ui::grid::GridShape;
use crate::ui::icons;
use crate::ui::thumbnails::{Held, Thumbnails};

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

/// What a card can draw for its icon: the type's or file's own icon, or an
/// image file's picture.
#[derive(Clone, Copy)]
pub struct Art<'a> {
    pub file_icons: &'a FileIcons,
    pub thumbs: &'a Thumbnails,
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
    art: Art,
) -> GridOut {
    let Art { file_icons, thumbs } = art;
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
        let (clip, mut covered) = (ui.clip_rect(), false);
        for line in first..last {
            for i in shape.line(line) {
                let entry = view.entry(view.rows[i]);
                let rect = place(i);
                let over = dnd::over(ui.ctx(), clip, rect, entry.is_dir.then_some(entry.path.as_path()));
                covered |= over.inside;
                let response = ui.interact(rect, egui::Id::new(("lumen_card", i)), egui::Sense::click_and_drag());
                if !edit.as_deref().is_some_and(|e| e.path == entry.path) {
                    dnd::start(&response, || dnd::carry(view, i));
                }
                paint_cell(ui, view, i, rect, &response);
                if over.lit {
                    ui.painter().add(dnd::lit_shape(ui.ctx(), rect));
                }
                let inner = rect.shrink2(vec2(PAD / 2.0, PAD));
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
                    if cut.contains(&entry.path) {
                        ui.set_opacity(file_list::CUT_OPACITY);
                    }
                    if entry.is_dir {
                        // Drawn by Lumen, never with the theme's folder icon.
                        let held = thumbs.held(entry);
                        let own = match &held {
                            Held::File(file) => file_icons.texture(file),
                            _ => None,
                        };
                        icons::folder_with(ui, icon, &held, own);
                    } else {
                        match thumbs.picture(entry) {
                            Some((texture, size)) => icons::picture(ui, icon, texture, size),
                            None => icons::show(ui, entry, icon, file_icons.texture(entry)),
                        };
                    }
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
        if !covered {
            dnd::background(ui, content, &view.current);
        }
        dnd::edge_scroll(ui, false);
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
    let visuals = ui.visuals();
    if view.is_highlighted(i) {
        ui.painter().rect_filled(rect, 4.0, visuals.selection.bg_fill.gamma_multiply(0.35));
        ui.painter().rect_stroke(rect, 4.0, visuals.selection.stroke, egui::StrokeKind::Inside);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    if view.is_outlined(i) {
        paint_dashed(ui.painter(), rect.shrink(1.0), visuals.weak_text_color());
    }
    if view.selected == Some(i) {
        ui.ctx().data_mut(|d| d.insert_temp(file_list::cursor_row_id(), rect));
    }
}

/// A thin dashed outline around `rect`: the cursor when nothing is selected,
/// as in other file managers.
pub(crate) fn paint_dashed(painter: &egui::Painter, rect: Rect, color: egui::Color32) {
    let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    painter.extend(egui::Shape::dashed_line(&corners, egui::Stroke::new(1.0, color), 3.0, 3.0));
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
pub(crate) mod tests {
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
        /// Filled rectangles, and the stroked ones with no fill.
        fills: Vec<(Rect, egui::Color32)>,
        rings: Vec<(Rect, egui::Stroke)>,
    }

    fn draw(ctx: &egui::Context, view: &View, growth: f32, events: Vec<egui::Event>) -> Drawn {
        draw_at(ctx, view, growth, events, None)
    }

    fn draw_at(ctx: &egui::Context, view: &View, growth: f32, events: Vec<egui::Event>, time: Option<f64>) -> Drawn {
        draw_keys(ctx, view, growth, events, time, Default::default())
    }

    fn draw_keys(ctx: &egui::Context, view: &View, growth: f32, events: Vec<egui::Event>, time: Option<f64>, modifiers: egui::Modifiers) -> Drawn {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 480.0));
        // The keys held stay held until a frame says otherwise.
        let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
        let input = egui::RawInput { screen_rect: Some(screen), events, time, ..Default::default() };
        let mut out = None;
        let mut output = ctx.run_ui(input, |ui| {
            out = Some(show(ui, view, &[], growth, None, None, Art { file_icons: &FileIcons::default(), thumbs: &Thumbnails::default() }));
            crate::ui::dnd::finish(ui.ctx());
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()), t.galley.elided)),
                _ => None,
            })
            .collect();
        let rects = output.shapes.iter().filter_map(|s| match &s.shape {
            egui::Shape::Rect(r) => Some(r.clone()),
            _ => None,
        });
        let fills = rects.clone().filter(|r| r.fill != egui::Color32::TRANSPARENT).map(|r| (r.rect, r.fill)).collect();
        let rings = rects.filter(|r| r.fill == egui::Color32::TRANSPARENT && !r.stroke.is_empty()).map(|r| (r.rect, r.stroke)).collect();
        Drawn { out: out.unwrap(), texts, fills, rings }
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

    /// Short line segments painted inside `rect`: a dashed outline.
    pub(crate) fn dashes_in(shapes: &[egui::epaint::ClippedShape], rect: Rect) -> usize {
        let weak = egui::Context::default().global_style().visuals.weak_text_color();
        shapes
            .iter()
            .filter(|s| matches!(s.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == weak && stroke.width == 1.0))
            .filter(|s| rect.expand(2.0).contains_rect(s.shape.visual_bounding_rect()))
            .count()
    }

    #[test]
    fn an_unselected_cursor_card_has_a_dashed_outline_and_no_highlight() {
        let ctx = egui::Context::default();
        let mut view = listed(&[], &["a.txt", "b.txt"]);
        view.select_row(0);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 480.0));
        let paint = |view: &View| {
            let mut shapes = Vec::new();
            for _ in 0..2 {
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                    show(ui, view, &[], 0.0, None, None, Art { file_icons: &FileIcons::default(), thumbs: &Thumbnails::default() });
                });
                output.textures_delta.clear();
                shapes = output.shapes;
            }
            shapes
        };
        let card = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(GAP + card_size(M_ICON, 20.0).x + 2.0, 200.0));
        let fill = ctx.global_style().visuals.selection.bg_fill.gamma_multiply(0.35);
        let filled = |shapes: &[egui::epaint::ClippedShape]| shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == fill));
        let selected = paint(&view);
        assert!(filled(&selected) && dashes_in(&selected, card) == 0, "selected: filled, no dashes");
        view.unselect();
        let unselected = paint(&view);
        assert!(!filled(&unselected), "no highlight");
        assert!(dashes_in(&unselected, card) >= 8, "a dashed outline around the cursor card");
    }

    #[test]
    fn an_image_card_shows_its_own_picture_fitted_with_its_shape() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["photo.png", "notes.txt"]);
        let photo = std::path::Path::new("/x/photo.png");
        let thumbs = Thumbnails::default().with_picture(&ctx, photo, [400, 200]);
        let (texture, _) = thumbs.picture(view.entry(0)).unwrap();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 480.0));
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                show(ui, &view, &[], 1.0, None, None, Art { file_icons: &FileIcons::default(), thumbs: &thumbs });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let pictures: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(s.shape.visual_bounding_rect()),
                _ => None,
            })
            .collect();
        assert_eq!(pictures.len(), 1, "one card shows a picture");
        let picture = pictures[0];
        assert!((picture.width() - XL_ICON).abs() < 0.5 && (picture.height() - XL_ICON / 2.0).abs() < 0.5, "fitted, 2:1: {picture:?}");
    }

    #[test]
    fn a_small_picture_is_not_blown_up_past_its_own_pixels() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["icon.png"]);
        let thumbs = Thumbnails::default().with_picture(&ctx, std::path::Path::new("/x/icon.png"), [16, 32]);
        let (texture, _) = thumbs.picture(view.entry(0)).unwrap();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 480.0));
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                show(ui, &view, &[], 1.0, None, None, Art { file_icons: &FileIcons::default(), thumbs: &thumbs });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let picture = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(s.shape.visual_bounding_rect()),
                _ => None,
            })
            .expect("drawn");
        assert!((picture.width() - 16.0).abs() < 0.5 && (picture.height() - 32.0).abs() < 0.5, "its own size: {picture:?}");
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

    /// What the drag in progress carries, as paths.
    fn carried(ctx: &egui::Context) -> Option<Vec<PathBuf>> {
        egui::DragAndDrop::payload::<crate::drag::Drag>(ctx).map(|drag| drag.paths.clone())
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    /// Presses at `from` and drags to `to`, holding the button; what the last frame drew.
    fn lift(ctx: &egui::Context, view: &View, from: egui::Pos2, to: egui::Pos2) -> Drawn {
        draw(ctx, view, 0.0, vec![egui::Event::PointerMoved(from), button(from, true)]);
        draw(ctx, view, 0.0, vec![egui::Event::PointerMoved(from + (to - from) * 0.1)]);
        draw(ctx, view, 0.0, vec![egui::Event::PointerMoved(to)]);
        // The label is laid out the frame it first shows.
        draw(ctx, view, 0.0, vec![])
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from("/x").join(n)).collect()
    }

    #[test]
    fn a_drag_from_a_card_carries_that_entry_when_it_is_not_selected() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt", "b.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let a = at(&drawn, "a.txt").1.center();
        let away = egui::pos2(630.0, 470.0);
        lift(&ctx, &view, a, away);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt"])), "nothing selected: the card under the pointer");
        draw(&ctx, &view, 0.0, vec![button(away, false)]);
        assert_eq!(carried(&ctx), None, "let go: nothing is carried any more");

        // Another entry is the selection: the dragged card is not part of it, so it goes alone.
        view.select_row(2);
        lift(&ctx, &view, a, away);
        assert_eq!(carried(&ctx), Some(paths(&["a.txt"])), "the selection stays where it was");
    }

    #[test]
    fn a_drag_from_a_picked_card_carries_the_whole_selection_in_display_order() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt", "b.txt", "c.txt"]);
        view.select_row(3);
        view.toggle_pick(1);
        let drawn = settled(&ctx, &view, 0.0);
        let c = at(&drawn, "c.txt").1.center();
        lift(&ctx, &view, c, egui::pos2(630.0, 470.0));
        assert_eq!(carried(&ctx), Some(paths(&["a.txt", "c.txt"])), "both picked entries, not the folder or b");
    }

    #[test]
    fn a_label_by_the_pointer_counts_what_is_carried_and_goes_with_it() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt", "b.txt", "c.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let a = at(&drawn, "a.txt").1.center();
        // Over file cards: they take no drop, so the label only counts.
        let first = at(&drawn, "b.txt").1.center();
        let one = lift(&ctx, &view, a, first);
        let label = at(&one, "1 item").1;
        assert!(one.texts.iter().all(|(t, ..)| t != "1 items"), "singular: {:?}", one.texts);
        assert!(label.min.x > first.x && label.min.y > first.y && label.min.x < first.x + 40.0 && label.min.y < first.y + 40.0, "by the pointer: {label:?}");
        let second = at(&drawn, "c.txt").1.center();
        let moved = draw(&ctx, &view, 0.0, vec![egui::Event::PointerMoved(second)]);
        let label = at(&moved, "1 item").1;
        assert!(label.min.x > second.x && label.min.x < second.x + 40.0 && label.min.y > second.y && label.min.y < second.y + 40.0, "follows it: {label:?}");

        draw(&ctx, &view, 0.0, vec![button(second, false)]);
        assert!(!draw(&ctx, &view, 0.0, vec![]).texts.iter().any(|(t, ..)| t.contains("item")), "gone once let go");

        view.select_row(1);
        view.extend_to(3);
        let three = lift(&ctx, &view, at(&settled(&ctx, &view, 0.0), "b.txt").1.center(), second);
        at(&three, "3 items");
    }

    #[test]
    fn escape_cancels_the_drag_and_its_label() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["a.txt", "b.txt"]);
        let a = at(&settled(&ctx, &view, 0.0), "a.txt").1.center();
        let away = egui::pos2(500.0, 400.0);
        lift(&ctx, &view, a, away);
        assert!(carried(&ctx).is_some());
        let key = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let after = draw(&ctx, &view, 0.0, vec![key]);
        assert_eq!(carried(&ctx), None, "nothing is carried");
        assert!(!after.texts.iter().any(|(t, ..)| t.contains("item")), "no label: {:?}", after.texts);
        let released = draw(&ctx, &view, 0.0, vec![button(away, false)]);
        assert!(released.out.event.is_none(), "letting go afterwards is not a click");
    }

    #[test]
    fn a_click_or_a_band_from_the_empty_space_carries_nothing_and_still_works() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt", "b.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (a, b) = (at(&drawn, "a.txt").1, at(&drawn, "b.txt").1);
        let selected = draw(&ctx, &view, 0.0, click(a.center(), egui::PointerButton::Primary)).out.event;
        assert!(matches!(selected, Some(ListEvent::Select(1))), "a click on a card selects it");
        assert_eq!(carried(&ctx), None);
        settled(&ctx, &view, 0.0);
        let band = lift(&ctx, &view, egui::pos2(a.left() - 30.0, a.top() - 70.0).max(egui::pos2(2.0, 2.0)), b.center());
        assert_eq!(banded(&band.out.event), Some(vec![1, 2]), "a band from the space around the cards still picks");
        assert_eq!(carried(&ctx), None, "and carries nothing");
    }

    const SHIFT: egui::Modifiers = egui::Modifiers { shift: true, ..egui::Modifiers::NONE };
    const CTRL: egui::Modifiers = egui::Modifiers { ctrl: true, ..egui::Modifiers::NONE };

    /// Whether a drop-target fill was painted over `at`.
    fn lit(ctx: &egui::Context, drawn: &Drawn, at: egui::Pos2) -> bool {
        let fill = crate::ui::dnd::lit_fill(&ctx.global_style().visuals);
        drawn.fills.iter().any(|(rect, f)| *f == fill && rect.contains(at))
    }

    /// Lets go at `at` holding `modifiers`; the drop the grid took.
    fn let_go(ctx: &egui::Context, view: &View, at: egui::Pos2, modifiers: egui::Modifiers) -> Option<(Vec<PathBuf>, PathBuf, crate::drag::Keys)> {
        draw_keys(ctx, view, 0.0, vec![button(at, false)], None, modifiers);
        crate::ui::dnd::take_drop(ctx).map(|d| (d.drag.paths, d.target, d.keys))
    }

    #[test]
    fn a_dragged_card_over_a_folder_card_lights_it_names_the_action_and_drops_on_it() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt", "b.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (folder, a) = (at(&drawn, "test").1.center(), at(&drawn, "a.txt").1.center());
        let held = lift(&ctx, &view, a, folder);
        assert!(lit(&ctx, &held, folder), "the folder lights up: {:?}", held.fills);
        assert!(!lit(&ctx, &held, a), "and only it");
        // /x is not a disk to compare with: a copy, until Shift forces a move.
        at(&held, "Copy 1 item to test");
        let shift = draw_keys(&ctx, &view, 0.0, vec![], None, SHIFT);
        at(&shift, "Move 1 item to test");
        let ctrl = draw_keys(&ctx, &view, 0.0, vec![], None, CTRL);
        at(&ctrl, "Copy 1 item to test");
        let dropped = let_go(&ctx, &view, folder, SHIFT);
        assert_eq!(dropped, Some((paths(&["a.txt"]), PathBuf::from("/x/test"), crate::drag::Keys { ctrl: false, shift: true })));
        assert!(crate::ui::dnd::take_drop(&ctx).is_none(), "taken once");
        assert_eq!(carried(&ctx), None);
        let after = draw(&ctx, &view, 0.0, vec![]);
        assert!(!lit(&ctx, &after, folder) && !after.texts.iter().any(|(t, ..)| t.contains("item")), "nothing lit, no label");
    }

    #[test]
    fn a_folder_card_is_only_lit_when_a_drag_is_carried() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt"]);
        let folder = at(&settled(&ctx, &view, 0.0), "test").1.center();
        let hovered = draw(&ctx, &view, 0.0, vec![egui::Event::PointerMoved(folder)]);
        assert!(!lit(&ctx, &hovered, folder), "hover alone is not a drop target");
    }

    #[test]
    fn a_folder_that_cannot_take_the_drop_stays_dark_and_takes_nothing() {
        let ctx = egui::Context::default();
        let view = listed(&["one", "two"], &["a.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (one, two) = (at(&drawn, "one").1.center(), at(&drawn, "two").1.center());
        // One folder onto itself.
        let held = lift(&ctx, &view, one, one + vec2(0.0, -25.0));
        assert!(!lit(&ctx, &held, one), "not lit: {:?}", held.fills);
        at(&held, "1 item");
        assert_eq!(let_go(&ctx, &view, one + vec2(0.0, -25.0), Default::default()), None, "no drop");
        // And onto the other one.
        let held = lift(&ctx, &view, one, two);
        assert!(lit(&ctx, &held, two));
        at(&held, "Copy 1 item to two");
        assert!(let_go(&ctx, &view, two, Default::default()).is_some());
    }

    #[test]
    fn a_file_card_takes_no_drop() {
        let ctx = egui::Context::default();
        let view = listed(&[], &["a.txt", "b.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (a, b) = (at(&drawn, "a.txt").1.center(), at(&drawn, "b.txt").1.center());
        let held = lift(&ctx, &view, a, b);
        assert!(!lit(&ctx, &held, b));
        at(&held, "1 item");
        assert_eq!(let_go(&ctx, &view, b, Default::default()), None);
    }

    #[test]
    fn letting_go_on_the_empty_space_drops_into_the_folder_on_screen() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt"]);
        view.current = PathBuf::from("/x");
        let a = at(&settled(&ctx, &view, 0.0), "a.txt").1.center();
        let space = egui::pos2(500.0, 400.0);
        let held = lift(&ctx, &view, a, space);
        at(&held, "Copy 1 item to x");
        let ring = ctx.global_style().visuals.selection.stroke;
        assert!(held.rings.iter().any(|(rect, stroke)| *stroke == ring && rect.contains(space) && rect.width() > 400.0), "the list is ringed: {:?}", held.rings);
        assert!(!lit(&ctx, &held, a), "no card lights up");
        let dropped = let_go(&ctx, &view, space, CTRL);
        assert_eq!(dropped, Some((paths(&["a.txt"]), PathBuf::from("/x"), crate::drag::Keys { ctrl: true, shift: false })));
    }

    #[test]
    fn the_empty_space_refuses_a_move_into_the_folder_the_entries_are_in_but_not_a_copy() {
        use crate::fs::ops::tests::fresh_fixture;
        let dir = fresh_fixture("dnd-grid-in-place");
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let mut view = View::default();
        view.entries = vec![Entry { name: "a.txt".into(), path: dir.join("a.txt"), is_dir: false, size: Some(1), modified: None, hidden: false }];
        view.rows = vec![0];
        view.current = dir.clone();
        let ctx = egui::Context::default();
        let a = at(&settled(&ctx, &view, 0.0), "a.txt").1.center();
        let space = egui::pos2(500.0, 400.0);
        let held = lift(&ctx, &view, a, space);
        at(&held, "1 item");
        assert!(held.rings.is_empty(), "nothing ringed: {:?}", held.rings);
        assert_eq!(let_go(&ctx, &view, space, Default::default()), None, "a move there does nothing");

        lift(&ctx, &view, a, space);
        let ctrl = draw_keys(&ctx, &view, 0.0, vec![], None, CTRL);
        at(&ctrl, &format!("Copy 1 item to {}", dir.file_name().unwrap().to_string_lossy()));
        let dropped = let_go(&ctx, &view, space, CTRL);
        assert_eq!(dropped.map(|d| d.1), Some(dir.clone()), "Ctrl copies there, as Paste makes duplicates");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escape_then_letting_go_over_a_folder_drops_nothing() {
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["a.txt"]);
        let drawn = settled(&ctx, &view, 0.0);
        let (folder, a) = (at(&drawn, "test").1.center(), at(&drawn, "a.txt").1.center());
        let held = lift(&ctx, &view, a, folder);
        assert!(lit(&ctx, &held, folder));
        let key = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let after = draw(&ctx, &view, 0.0, vec![key]);
        assert!(!lit(&ctx, &after, folder), "no longer lit");
        assert_eq!(let_go(&ctx, &view, folder, Default::default()), None);
    }

    #[test]
    fn held_near_the_top_or_bottom_edge_a_carried_drag_scrolls_the_grid() {
        let ctx = egui::Context::default();
        let files: Vec<String> = (0..200).map(|n| format!("f{n:03}.txt")).collect();
        let view = listed(&[], &files.iter().map(String::as_str).collect::<Vec<_>>());
        let drawn = settled(&ctx, &view, 0.0);
        let row_two = at(&drawn, "f010.txt").1.top();
        let from = at(&drawn, "f000.txt").1.center();
        let middle = egui::pos2(300.0, 240.0);
        lift(&ctx, &view, from, middle);
        for _ in 0..5 {
            draw(&ctx, &view, 0.0, vec![]);
        }
        assert_eq!(at(&draw(&ctx, &view, 0.0, vec![]), "f010.txt").1.top(), row_two, "held in the middle: no scroll");

        let bottom = egui::pos2(300.0, 470.0);
        draw(&ctx, &view, 0.0, vec![egui::Event::PointerMoved(bottom)]);
        let mut tops = Vec::new();
        for _ in 0..5 {
            tops.push(at(&draw(&ctx, &view, 0.0, vec![]), "f010.txt").1.top());
        }
        assert!(tops.windows(2).all(|w| w[1] < w[0]) && tops[4] < row_two - 20.0, "the rows come up while held: {tops:?}");
        draw(&ctx, &view, 0.0, vec![button(bottom, false)]);

        // Without a drag the pointer there does nothing (once the last scrolls have landed).
        for _ in 0..3 {
            draw(&ctx, &view, 0.0, vec![]);
        }
        let settled_top = at(&draw(&ctx, &view, 0.0, vec![]), "f010.txt").1.top();
        for _ in 0..5 {
            draw(&ctx, &view, 0.0, vec![egui::Event::PointerMoved(bottom)]);
        }
        assert_eq!(at(&draw(&ctx, &view, 0.0, vec![]), "f010.txt").1.top(), settled_top, "no drag, no scroll");
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

    /// Draws the grid twice (the second shows what the first asked for).
    fn shapes_of(ctx: &egui::Context, view: &View, growth: f32, file_icons: &FileIcons, thumbs: &Thumbnails) -> Vec<egui::epaint::ClippedShape> {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 480.0));
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                show(ui, view, &[], growth, None, None, Art { file_icons, thumbs });
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        shapes
    }

    fn meshes_of(shapes: &[egui::epaint::ClippedShape], texture: egui::TextureId) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(s.shape.visual_bounding_rect()),
                _ => None,
            })
            .collect()
    }

    /// The sheets (page-coloured polygons) drawn.
    fn sheets_of(shapes: &[egui::epaint::ClippedShape]) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Path(path) if path.fill == icons::PAGE => Some(s.shape.visual_bounding_rect()),
                _ => None,
            })
            .collect()
    }

    fn texture(ctx: &egui::Context, name: &str) -> egui::TextureHandle {
        ctx.load_texture(name, egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]), egui::TextureOptions::LINEAR)
    }

    /// The first card's rect at `growth`, as the grid places it.
    fn first_card(growth: f32) -> Rect {
        Rect::from_min_size(pos2(GAP, GAP), card_size(icon_side(growth), 20.0)).expand2(vec2(0.0, 20.0))
    }

    /// A view with the folder `/x/test` and thumbnails where it holds `peek`.
    fn peeking(peek: crate::fs::thumbs::Peek) -> (View, Thumbnails) {
        (listed(&["test"], &[]), Thumbnails::default().with_peek(std::path::Path::new("/x/test"), peek))
    }

    #[test]
    fn a_folder_whose_peek_is_a_picture_draws_it_inside_its_card() {
        use crate::fs::thumbs::Peek;
        let ctx = egui::Context::default();
        let view = listed(&["test"], &[]);
        let thumbs = Thumbnails::default()
            .with_peek(std::path::Path::new("/x/test"), Peek::File(PathBuf::from("/x/test/photo.png"), None))
            .with_picture(&ctx, std::path::Path::new("/x/test/photo.png"), [400, 200]);
        let id = thumbs.picture_of(std::path::Path::new("/x/test/photo.png")).expect("loaded");
        let shapes = shapes_of(&ctx, &view, 1.0, &FileIcons::default(), &thumbs);
        let pictures = meshes_of(&shapes, id);
        assert_eq!(pictures.len(), 1, "the picture is drawn once");
        let card = first_card(1.0);
        assert!(card.contains_rect(pictures[0]), "inside the card: {:?} in {card:?}", pictures[0]);
        assert!((pictures[0].width() / pictures[0].height() - 2.0).abs() < 0.05, "keeps its shape: {:?}", pictures[0]);
        assert!(pictures[0].width() <= XL_ICON, "fits the icon: {:?}", pictures[0]);
        assert!(sheets_of(&shapes).is_empty(), "a picture, not a sheet");
    }

    #[test]
    fn a_folder_holding_only_folders_draws_blank_sheets_and_an_empty_one_none() {
        use crate::fs::thumbs::Peek;
        let ctx = egui::Context::default();
        let (view, thumbs) = peeking(Peek::OnlyFolders);
        let sheets = sheets_of(&shapes_of(&ctx, &view, 1.0, &FileIcons::default(), &thumbs));
        assert!(sheets.len() >= 2 && sheets.len() <= 3, "two or three blank sheets: {sheets:?}");
        assert!(sheets.iter().all(|s| first_card(1.0).contains_rect(*s)));
        assert!(sheets.windows(2).all(|w| w[0] != w[1]), "slightly offset: {sheets:?}");

        let (view, thumbs) = peeking(Peek::Empty);
        assert!(sheets_of(&shapes_of(&ctx, &view, 1.0, &FileIcons::default(), &thumbs)).is_empty(), "nothing sticks out of an empty folder");
        let view = listed(&["test"], &[]);
        let unread = Thumbnails::default();
        assert!(sheets_of(&shapes_of(&ctx, &view, 1.0, &FileIcons::default(), &unread)).is_empty(), "nothing while the peek is not read");
        assert_eq!(unread.asked(), vec![crate::fs::thumbs::Job::Peek(PathBuf::from("/x/test"), None); 2], "and it is asked for");
    }

    #[test]
    fn a_folder_peeking_a_file_draws_one_sheet_with_its_type_icon() {
        use crate::fs::thumbs::Peek;
        let ctx = egui::Context::default();
        let (view, thumbs) = peeking(Peek::File(PathBuf::from("/x/test/data.json"), None));
        let painted = shapes_of(&ctx, &view, 1.0, &FileIcons::default(), &thumbs);
        assert_eq!(sheets_of(&painted).len(), 1, "one sheet");

        let mut file_icons = FileIcons::default();
        let json = texture(&ctx, "json");
        file_icons.insert(&entry("data.json", false), json.clone());
        let shown = shapes_of(&ctx, &view, 1.0, &file_icons, &thumbs);
        assert_eq!(sheets_of(&shown).len(), 1);
        let marks = meshes_of(&shown, json.id());
        assert_eq!(marks.len(), 1, "the file's type icon from the theme sits on the sheet");
        assert!(first_card(1.0).contains_rect(marks[0]));
        assert!(sheets_of(&shown)[0].contains_rect(marks[0].shrink(0.5)), "on the sheet: {marks:?} on {:?}", sheets_of(&shown));
    }

    #[test]
    fn a_picture_that_cannot_be_decoded_falls_back_to_the_sheet() {
        use crate::fs::thumbs::Peek;
        let ctx = egui::Context::default();
        let (view, mut thumbs) = peeking(Peek::File(PathBuf::from("/x/test/broken.png"), None));
        thumbs = thumbs.without_picture(std::path::Path::new("/x/test/broken.png"));
        assert_eq!(sheets_of(&shapes_of(&ctx, &view, 1.0, &FileIcons::default(), &thumbs)).len(), 1);
        assert!(thumbs.asked().is_empty(), "not asked again");
    }

    #[test]
    fn in_the_icons_grid_a_folder_never_draws_the_themes_folder_icon_but_files_keep_theirs() {
        use crate::fs::thumbs::Peek;
        let ctx = egui::Context::default();
        let view = listed(&["test"], &["notes.txt"]);
        let mut file_icons = FileIcons::default();
        let (folder_icon, text_icon) = (texture(&ctx, "folder"), texture(&ctx, "text"));
        file_icons.insert(view.entry(0), folder_icon.clone());
        file_icons.insert(view.entry(1), text_icon.clone());
        for peek in [Peek::Empty, Peek::OnlyFolders, Peek::File(PathBuf::from("/x/test/a.md"), None)] {
            let thumbs = Thumbnails::default().with_peek(std::path::Path::new("/x/test"), peek.clone());
            let shapes = shapes_of(&ctx, &view, 0.0, &file_icons, &thumbs);
            assert!(meshes_of(&shapes, folder_icon.id()).is_empty(), "{peek:?}: drawn by Lumen");
            assert_eq!(meshes_of(&shapes, text_icon.id()).len(), 1, "{peek:?}: the file keeps its icon");
        }
        let unread = shapes_of(&ctx, &view, 0.0, &file_icons, &Thumbnails::default());
        assert!(meshes_of(&unread, folder_icon.id()).is_empty(), "also while the peek is not read");
    }

    /// Holds a drag from another app at `to` with the keys agreed with it.
    fn hold_outside(ctx: &egui::Context, view: &View, to: egui::Pos2, keys: crate::drag::Keys, released: bool) -> Drawn {
        let drag = crate::drag::Drag { paths: paths(&["a.txt"]) };
        crate::ui::dnd::set_outside(ctx, Some(crate::ui::dnd::Outside { drag, at: to, keys, released }));
        let drawn = draw(ctx, view, 0.0, vec![]);
        if released { drawn } else { draw(ctx, view, 0.0, vec![]) }
    }

    #[test]
    fn files_dragged_in_from_another_app_light_the_folder_card_under_them_and_drop_on_it() {
        let ctx = egui::Context::default();
        let mut view = listed(&["test"], &["a.txt", "b.txt"]);
        view.current = PathBuf::from("/x");
        let drawn = settled(&ctx, &view, 0.0);
        let (folder, b) = (at(&drawn, "test").1.center(), at(&drawn, "b.txt").1.center());
        let none = crate::drag::Keys::default();
        // egui's pointer is elsewhere (it gets no events during such a drag).
        draw(&ctx, &view, 0.0, vec![egui::Event::PointerMoved(b)]);
        let held = hold_outside(&ctx, &view, folder, none, false);
        assert!(lit(&ctx, &held, folder), "the folder card lights up: {:?}", held.fills);
        assert!(!lit(&ctx, &held, b), "and only it");
        at(&held, "Copy 1 item to test");
        let held = hold_outside(&ctx, &view, folder, crate::drag::Keys { ctrl: false, shift: true }, false);
        at(&held, "Move 1 item to test");
        hold_outside(&ctx, &view, folder, crate::drag::Keys { ctrl: false, shift: true }, true);
        let dropped = crate::ui::dnd::take_drop(&ctx).map(|d| (d.drag.paths, d.target, d.keys));
        assert_eq!(dropped, Some((paths(&["a.txt"]), PathBuf::from("/x/test"), crate::drag::Keys { ctrl: false, shift: true })));

        // The empty space drops into the folder on screen; a file card takes nothing.
        let space = egui::pos2(500.0, 400.0);
        let held = hold_outside(&ctx, &view, space, none, false);
        at(&held, "Copy 1 item to x");
        hold_outside(&ctx, &view, space, none, true);
        assert_eq!(crate::ui::dnd::take_drop(&ctx).map(|d| d.target), Some(PathBuf::from("/x")));
        let held = hold_outside(&ctx, &view, b, none, false);
        assert!(!lit(&ctx, &held, b));
        at(&held, "1 item");
        hold_outside(&ctx, &view, b, none, true);
        assert!(crate::ui::dnd::take_drop(&ctx).is_none());
        crate::ui::dnd::set_outside(&ctx, None);
    }
}
