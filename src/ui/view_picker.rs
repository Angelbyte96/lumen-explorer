//! The view button at the right of the status bar, like File Pilot's: the
//! view's icon, name and percent; a click opens a panel above it with the
//! six views beside a vertical slider.

use eframe::egui::{self, Color32, Rect, Stroke, pos2, vec2};

use crate::ui::view_size::{Layout, STOPS, ViewSize};

/// The slider's rail, bottom (0%) to top (100%), in points.
const RAIL: f32 = 230.0;
/// Room for the names left of the slider.
const NAMES: f32 = 150.0;
/// Room for the knob right of the names.
const KNOB_ZONE: f32 = 32.0;
const KNOB_RADIUS: f32 = 9.0;

fn popup_id() -> egui::Id {
    egui::Id::new("lumen_view_picker")
}

/// Draws the button; with the panel open, the slider too. Returns the
/// position picked this frame.
pub fn show(ui: &mut egui::Ui, size: ViewSize) -> Option<ViewSize> {
    let mut picked = None;
    // Right to left: the percent box at the edge, then icon and name.
    let percent = percent_box(ui, size);
    let name = ui.add(egui::Button::new(size.name()).frame(false)).on_hover_text("View (Ctrl+wheel)");
    let icon_side = ui.text_style_height(&egui::TextStyle::Body);
    let (icon_rect, icon) = ui.allocate_exact_size(vec2(icon_side, icon_side), egui::Sense::click());
    paint_icon(ui.painter(), icon_rect, size.layout(), ui.visuals().text_color());
    let anchor = name.rect.union(icon.rect).union(percent.rect);
    if name.clicked() || icon.clicked() || percent.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup_id());
    }
    egui::Popup::new(popup_id(), ui.ctx().clone(), anchor, ui.layer_id())
        .open_memory(None)
        .align(egui::RectAlign::TOP_END)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| picked = panel(ui, size));
    picked
}

/// "0%" in a small box whose blue part grows with the slider.
fn percent_box(ui: &mut egui::Ui, size: ViewSize) -> egui::Response {
    let text = format!("{}%", size.percent());
    let galley = ui.painter().layout_no_wrap(text, egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color());
    let box_size = vec2(galley.size().x.max(30.0) + 12.0, galley.size().y + 4.0);
    let (rect, response) = ui.allocate_exact_size(box_size, egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * f32::from(size.percent()) / 100.0, rect.height()));
    painter.rect_filled(filled, 3.0, ui.visuals().selection.bg_fill);
    painter.rect_stroke(rect, 3.0, ui.visuals().widgets.inactive.bg_stroke, egui::StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::PLACEHOLDER);
    response.on_hover_text("View size")
}

/// The open panel: each view's name at its height on the slider, the slider
/// right of them. A click on a name jumps there; the knob drags.
fn panel(ui: &mut egui::Ui, size: ViewSize) -> Option<ViewSize> {
    let row = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(NAMES + KNOB_ZONE, RAIL + row), egui::Sense::hover());
    // The rail spans the rows' centres: half a row of air above and below.
    let rail = egui::Rangef::new(rect.top() + row / 2.0, rect.bottom() - row / 2.0);
    let mut picked = None;
    for (percent, name, layout) in STOPS {
        let y = height_of(percent, rail);
        let row_rect = Rect::from_center_size(pos2(rect.left() + NAMES / 2.0, y), vec2(NAMES, row - 2.0));
        let response = ui.interact(row_rect, popup_id().with(name), egui::Sense::click());
        let current = size.name() == name;
        let visuals = ui.visuals();
        let (fill, text) = if current {
            (visuals.selection.bg_fill, visuals.selection.stroke.color)
        } else if response.hovered() {
            (visuals.widgets.hovered.weak_bg_fill, visuals.strong_text_color())
        } else {
            (Color32::TRANSPARENT, visuals.text_color())
        };
        let painter = ui.painter();
        painter.rect_filled(row_rect, 3.0, fill);
        let side = row - 8.0;
        let icon = Rect::from_center_size(pos2(row_rect.left() + 6.0 + side / 2.0, y), vec2(side, side));
        paint_icon(painter, icon, layout, text);
        let font = egui::TextStyle::Body.resolve(ui.style());
        painter.text(pos2(icon.right() + 8.0, y), egui::Align2::LEFT_CENTER, name, font, text);
        if response.clicked() {
            picked = Some(ViewSize::new(percent));
        }
    }
    let x = rect.right() - KNOB_ZONE / 2.0;
    let track = Rect::from_x_y_ranges(egui::Rangef::new(x - KNOB_ZONE / 2.0, x + KNOB_ZONE / 2.0), rail.expand(KNOB_RADIUS));
    let drag = ui.interact(track, popup_id().with("knob"), egui::Sense::click_and_drag());
    if (drag.dragged() || drag.clicked())
        && let Some(pointer) = drag.interact_pointer_pos()
    {
        let moved = ViewSize::new(percent_at(pointer.y, rail));
        if moved != size {
            picked = Some(moved);
        }
    }
    let shown = picked.unwrap_or(size);
    let painter = ui.painter();
    let knob = height_of(shown.percent(), rail);
    let width = 6.0;
    let rail_rect = Rect::from_x_y_ranges(egui::Rangef::new(x - width / 2.0, x + width / 2.0), rail);
    painter.rect_filled(rail_rect, width / 2.0, ui.visuals().widgets.inactive.bg_fill);
    let fill = Rect::from_x_y_ranges(rail_rect.x_range(), egui::Rangef::new(knob, rail.max));
    painter.rect_filled(fill, width / 2.0, ui.visuals().selection.bg_fill);
    painter.circle_filled(pos2(x, knob), KNOB_RADIUS, ui.visuals().selection.bg_fill);
    picked
}

/// The rail's height for `percent`: 0 at the bottom, 100 at the top.
pub fn height_of(percent: u8, rail: egui::Rangef) -> f32 {
    rail.max - rail.span() * f32::from(percent.min(100)) / 100.0
}

/// The percent under the pointer's height `y`, held to the rail's ends.
pub fn percent_at(y: f32, rail: egui::Rangef) -> u8 {
    ((rail.max - y) / rail.span() * 100.0).round().clamp(0.0, 100.0) as u8
}

/// A small mark for each view, drawn in `color`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, layout: Layout, color: Color32) {
    let stroke = Stroke::new((rect.height() * 0.1).max(1.0), color);
    let (l, r, t, h) = (rect.left(), rect.right(), rect.top(), rect.height());
    let line = |y: f32, from: f32, to: f32| painter.line_segment([pos2(from, y), pos2(to, y)], stroke);
    let rows = [0.25, 0.5, 0.75].map(|f| t + h * f);
    match layout {
        Layout::Details => {
            for y in rows {
                painter.circle_filled(pos2(l + h * 0.12, y), h * 0.07, color);
                line(y, l + h * 0.3, r - h * 0.05);
            }
        }
        Layout::Tree => {
            line(rows[0], l + h * 0.05, r - h * 0.05);
            line(rows[1], l + h * 0.4, r - h * 0.05);
            line(rows[2], l + h * 0.4, r - h * 0.05);
            painter.line_segment([pos2(l + h * 0.15, rows[0]), pos2(l + h * 0.15, rows[2])], stroke);
            line(rows[1], l + h * 0.15, l + h * 0.3);
            line(rows[2], l + h * 0.15, l + h * 0.3);
        }
        Layout::List => {
            let mid = rect.center().x;
            for y in rows {
                line(y, l + h * 0.05, mid - h * 0.08);
                line(y, mid + h * 0.08, r - h * 0.05);
            }
        }
        Layout::Icons => {
            let frame = rect.shrink(h * 0.1);
            painter.rect_stroke(frame, h * 0.1, stroke, egui::StrokeKind::Middle);
            painter.circle_filled(pos2(frame.left() + h * 0.25, frame.top() + h * 0.25), h * 0.08, color);
            painter.line_segment([pos2(frame.left(), frame.bottom() - h * 0.1), pos2(frame.center().x, frame.center().y)], stroke);
            painter.line_segment([pos2(frame.center().x, frame.center().y), pos2(frame.right(), frame.bottom() - h * 0.25)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_runs_from_0_at_the_bottom_to_100_at_the_top_and_holds_its_ends() {
        let rail = egui::Rangef::new(100.0, 300.0);
        assert_eq!(height_of(0, rail), 300.0);
        assert_eq!(height_of(100, rail), 100.0);
        assert_eq!(height_of(65, rail), 170.0);
        assert_eq!(percent_at(170.0, rail), 65);
        assert_eq!(percent_at(50.0, rail), 100, "above the rail");
        assert_eq!(percent_at(400.0, rail), 0, "below the rail");
    }

    /// Runs the bar's view button a few frames with `events` in the last one;
    /// returns what it picked and the texts painted.
    fn run(ctx: &egui::Context, size: ViewSize, events: Vec<egui::Event>) -> (Option<ViewSize>, Vec<(String, Rect)>) {
        let (picked, texts, _) = run_shapes(ctx, size, events);
        (picked, texts)
    }

    /// `run`, plus the knob's centre when the panel is open.
    fn run_shapes(ctx: &egui::Context, size: ViewSize, events: Vec<egui::Event>) -> (Option<ViewSize>, Vec<(String, Rect)>, Option<egui::Pos2>) {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
        let mut picked = None;
        let mut shapes = Vec::new();
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| picked = show(ui, size));
            });
        });
        output.textures_delta.clear();
        shapes.extend(output.shapes);
        let texts = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect();
        let knob = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Circle(c) if c.radius == KNOB_RADIUS => Some(c.center),
            _ => None,
        });
        (picked, texts, knob)
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    /// Where `text` was painted; the highest one when the button shows it too.
    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        let found = texts.iter().filter(|(t, _)| t == text).map(|(_, r)| *r);
        found.min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("{text:?} painted in {texts:?}"))
    }

    #[test]
    fn the_button_shows_the_views_name_and_percent_and_a_click_opens_the_six_views() {
        let ctx = egui::Context::default();
        let size = ViewSize::new(65);
        let (_, texts) = run(&ctx, size, vec![]);
        let name = find(&texts, "L Icons");
        find(&texts, "65%");
        assert!(!texts.iter().any(|(t, _)| t == "Tree"), "panel closed");
        let (picked, _) = run(&ctx, size, click(name.center()));
        assert_eq!(picked, None, "opening picks nothing");
        let (_, texts) = run(&ctx, size, vec![]);
        // Bottom to top as in STOPS: Details lowest, XL highest, the icon sizes far apart.
        let heights: Vec<f32> = STOPS.iter().map(|s| find(&texts, s.1).center().y).collect();
        assert!(heights.windows(2).all(|w| w[0] > w[1]), "Details at the bottom: {heights:?}");
        assert!(heights[4] - heights[5] > 2.0 * (heights[0] - heights[1]), "XL far above L: {heights:?}");
        assert!(heights.iter().all(|y| *y < name.top()), "above the button");
    }

    #[test]
    fn a_click_on_a_name_picks_that_view_and_a_drag_on_the_slider_picks_its_height() {
        let ctx = egui::Context::default();
        let size = ViewSize::new(0);
        let (_, texts) = run(&ctx, size, vec![]);
        run(&ctx, size, click(find(&texts, "Details").center()));
        let (_, texts) = run(&ctx, size, vec![]);
        let list = find(&texts, "List");
        let (picked, _) = run(&ctx, size, click(list.center()));
        assert_eq!(picked, Some(ViewSize::new(20)));

        let (_, texts, knob) = run_shapes(&ctx, size, vec![]);
        let (xl, details) = (find(&texts, "XL Icons"), find(&texts, "Details"));
        let knob = knob.expect("knob painted");
        assert!((knob.y - details.center().y).abs() < 1.0, "the knob sits at Details");
        let at = |y: f32| pos2(knob.x, y);
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&ctx, size, vec![egui::Event::PointerMoved(at(details.center().y)), press(at(details.center().y), true)]);
        let (picked, _) = run(&ctx, size, vec![egui::Event::PointerMoved(at(xl.center().y))]);
        assert_eq!(picked, Some(ViewSize::new(100)), "dragged to the top");
        let middle = (xl.center().y + details.center().y) / 2.0;
        let (picked, _) = run(&ctx, size, vec![egui::Event::PointerMoved(at(middle))]);
        assert_eq!(picked, Some(ViewSize::new(50)), "half way up");
        run(&ctx, size, vec![press(at(middle), false)]);
    }
}
