//! Lumen's own caption bar, like File Pilot's: the system title bar is off
//! (on Windows and Linux alike) and the tab row takes its place. Its empty
//! space moves the window and a double click there maximizes or restores
//! it; the window's edges and corners resize it.

use eframe::egui::{self, viewport::ResizeDirection, CursorIcon, ViewportCommand};

/// How far in from the window's border its edges resize it.
pub const EDGE: f32 = 5.0;

/// The id of the caption bar's empty space.
pub fn bar_id() -> egui::Id {
    egui::Id::new("lumen_caption_bar")
}

/// Whether the window fills the screen's work area now.
pub fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Draws `contents` in the caption bar, whose empty space moves the window
/// (a press and a short drag) and maximizes or restores it on a double click.
pub fn show<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // Sensed before the contents, so every control drawn on top keeps its own clicks;
    // not focusable, so the bar is no Tab stop.
    let bar = ui.interact(ui.max_rect(), bar_id(), egui::Sense::CLICK | egui::Sense::DRAG);
    let inner = contents(ui);
    let ctx = ui.ctx();
    if bar.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
    } else if bar.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    inner
}

/// Which way the window's border at `pos` resizes it, if `pos` is on it.
pub fn resize_direction(window: egui::Rect, pos: egui::Pos2) -> Option<ResizeDirection> {
    if !window.contains(pos) {
        return None;
    }
    let west = pos.x < window.left() + EDGE;
    let east = pos.x >= window.right() - EDGE;
    let north = pos.y < window.top() + EDGE;
    let south = pos.y >= window.bottom() - EDGE;
    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (.., true, _) => ResizeDirection::West,
        (.., true) => ResizeDirection::East,
        _ => return None,
    })
}

/// The pointer over the border that resizes `direction`.
pub fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// Lets the window's edges and corners resize it, above anything drawn
/// there; nothing while it is maximized. Call it every frame: an egui area is
/// not interactive on its first frame, so the edges work from the second.
pub fn resize_edges(ctx: &egui::Context) {
    if maximized(ctx) {
        return;
    }
    let window = ctx.content_rect();
    // The four sides, each with its corners; on a layer above the window's
    // contents (and its menus), so a control by the border never takes the press.
    let sides = [
        egui::Rect::from_min_max(window.min, egui::pos2(window.right(), window.top() + EDGE)),
        egui::Rect::from_min_max(egui::pos2(window.left(), window.bottom() - EDGE), window.max),
        egui::Rect::from_min_max(window.min, egui::pos2(window.left() + EDGE, window.bottom())),
        egui::Rect::from_min_max(egui::pos2(window.right() - EDGE, window.top()), window.max),
    ];
    for (index, side) in sides.into_iter().enumerate() {
        let id = egui::Id::new(("lumen_resize_edge", index));
        egui::Area::new(id).order(egui::Order::Tooltip).fixed_pos(side.min).constrain(false).show(ctx, |ui| {
            // Allocated, so the area has the side's size and is found under the pointer.
            let (_, edge) = ui.allocate_exact_size(side.size(), egui::Sense::DRAG);
            let pointer = ctx.input(|i| i.pointer.interact_pos());
            let Some(direction) = pointer.and_then(|pos| resize_direction(window, pos)) else { return };
            if edge.hovered() || edge.is_pointer_button_down_on() {
                ctx.set_cursor_icon(cursor(direction));
            }
            if edge.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing::SCREEN;
    use eframe::egui::{Event, PointerButton, Pos2, RawInput};

    /// One frame: the caption bar with a button in it, then the edges; what
    /// it asked of the window, and whether the button was clicked.
    fn window(ctx: &egui::Context, maximized: bool, events: Vec<Event>) -> (Vec<ViewportCommand>, bool) {
        let (commands, clicked, _) = window_and_cursor(ctx, maximized, events);
        (commands, clicked)
    }

    /// `window`, with the pointer's look that frame.
    fn window_and_cursor(ctx: &egui::Context, maximized: bool, events: Vec<Event>) -> (Vec<ViewportCommand>, bool, CursorIcon) {
        let mut input = RawInput { events, screen_rect: Some(SCREEN), ..RawInput::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(maximized);
        let mut clicked = false;
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("caption").show(root, |ui| {
                show(ui, |ui| {
                    ui.horizontal(|ui| clicked = ui.button("tab").clicked());
                })
            });
            resize_edges(root.ctx());
        });
        output.textures_delta.clear();
        let commands = output.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default();
        (commands, clicked, output.platform_output.cursor_icon)
    }

    fn press(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    /// An empty spot of the caption bar, right of its button.
    fn empty_bar(ctx: &egui::Context) -> Pos2 {
        window(ctx, false, vec![]);
        let bar = ctx.read_response(bar_id()).expect("the bar was drawn").rect;
        egui::pos2(bar.center().x + 100.0, bar.center().y)
    }

    fn sent(commands: &[ViewportCommand], wanted: &ViewportCommand) -> bool {
        commands.contains(wanted)
    }

    #[test]
    fn the_border_resizes_toward_its_side_and_its_corners_both_ways() {
        let w = SCREEN;
        let cases = [
            (egui::pos2(400.0, 1.0), Some(ResizeDirection::North)),
            (egui::pos2(400.0, 598.0), Some(ResizeDirection::South)),
            (egui::pos2(1.0, 300.0), Some(ResizeDirection::West)),
            (egui::pos2(798.0, 300.0), Some(ResizeDirection::East)),
            (egui::pos2(2.0, 2.0), Some(ResizeDirection::NorthWest)),
            (egui::pos2(798.0, 2.0), Some(ResizeDirection::NorthEast)),
            (egui::pos2(2.0, 598.0), Some(ResizeDirection::SouthWest)),
            (egui::pos2(798.0, 598.0), Some(ResizeDirection::SouthEast)),
            (egui::pos2(400.0, 300.0), None),
            (egui::pos2(400.0, EDGE + 1.0), None),
        ];
        for (pos, direction) in cases {
            assert_eq!(resize_direction(w, pos), direction, "at {pos:?}");
        }
    }

    #[test]
    fn dragging_the_bars_empty_space_moves_the_window() {
        let ctx = egui::Context::default();
        let at = empty_bar(&ctx);
        window(&ctx, false, vec![Event::PointerMoved(at)]);
        window(&ctx, false, vec![press(at, true)]);
        let (commands, _) = window(&ctx, false, vec![Event::PointerMoved(at + egui::vec2(20.0, 0.0))]);
        assert!(sent(&commands, &ViewportCommand::StartDrag), "{commands:?}");
    }

    #[test]
    fn a_click_on_a_control_in_the_bar_is_the_controls_and_moves_nothing() {
        let ctx = egui::Context::default();
        window(&ctx, false, vec![]);
        let button = ctx.read_response(bar_id()).expect("the bar was drawn").rect.left_center() + egui::vec2(15.0, 0.0);
        window(&ctx, false, vec![Event::PointerMoved(button)]);
        let (pressed, _) = window(&ctx, false, vec![press(button, true)]);
        let (released, clicked) = window(&ctx, false, vec![press(button, false)]);
        assert!(clicked, "the button took the click");
        assert!(![pressed, released].concat().iter().any(|c| matches!(c, ViewportCommand::StartDrag | ViewportCommand::Maximized(_))));
    }

    #[test]
    fn a_double_click_on_the_bars_empty_space_maximizes_or_restores() {
        for maximized in [false, true] {
            let ctx = egui::Context::default();
            let at = empty_bar(&ctx);
            window(&ctx, maximized, vec![Event::PointerMoved(at)]);
            let mut commands = Vec::new();
            for pressed in [true, false, true, false] {
                commands.extend(window(&ctx, maximized, vec![press(at, pressed)]).0);
            }
            assert!(sent(&commands, &ViewportCommand::Maximized(!maximized)), "maximized {maximized}: {commands:?}");
            assert!(!sent(&commands, &ViewportCommand::StartDrag), "a click is not a drag: {commands:?}");
        }
    }

    #[test]
    fn pressing_the_windows_border_resizes_it_even_over_a_control() {
        let ctx = egui::Context::default();
        // An egui area is not interactive on its first frame (its sizing pass).
        window(&ctx, false, vec![]);
        let corner = egui::pos2(1.0, 1.0);
        let top = egui::pos2(15.0, 1.0);
        for (at, direction) in [(corner, ResizeDirection::NorthWest), (top, ResizeDirection::North), (egui::pos2(798.0, 300.0), ResizeDirection::East)] {
            window(&ctx, false, vec![Event::PointerMoved(at)]);
            let (_, _, icon) = window_and_cursor(&ctx, false, vec![]);
            assert_eq!(icon, cursor(direction), "hovering {at:?}");
            let (commands, _) = window(&ctx, false, vec![press(at, true)]);
            window(&ctx, false, vec![press(at, false)]);
            assert!(sent(&commands, &ViewportCommand::BeginResize(direction)), "at {at:?}: {commands:?}");
            assert!(!sent(&commands, &ViewportCommand::StartDrag), "at {at:?}: {commands:?}");
        }
    }

    #[test]
    fn a_maximized_window_has_no_edges_to_resize() {
        let ctx = egui::Context::default();
        let at = egui::pos2(798.0, 300.0);
        // Warmed up unmaximized, so the edges would be there to press if they were drawn.
        window(&ctx, false, vec![]);
        window(&ctx, false, vec![]);
        window(&ctx, true, vec![Event::PointerMoved(at)]);
        let (commands, _) = window(&ctx, true, vec![press(at, true)]);
        assert!(!commands.iter().any(|c| matches!(c, ViewportCommand::BeginResize(_))), "{commands:?}");
    }
}
