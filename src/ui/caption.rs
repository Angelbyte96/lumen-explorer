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

/// The window buttons at the right end of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The ▾ left of minimize, on the main bar only.
    SearchTabs,
    Minimize,
    Maximize,
    Close,
}

impl Button {
    pub const ALL: [Button; 3] = [Button::Minimize, Button::Maximize, Button::Close];

    pub fn id(self) -> egui::Id {
        egui::Id::new(("lumen_window_button", self as u8))
    }

    /// The command's name and shortcut, as the hover hint shows them.
    pub fn hint(self) -> (&'static str, &'static str) {
        match self {
            Button::SearchTabs => ("Search Tabs", "Ctrl+Shift+A"),
            Button::Minimize => ("Minimize Window", "Alt+M"),
            Button::Maximize => ("Toggle Fullscreen", "F11"),
            Button::Close => ("Close Window", "Alt+F4"),
        }
    }
}

/// The panel frame of a caption bar: no margin, so the window buttons reach
/// the window's edges; `show` pads the contents itself.
pub fn frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(egui::Margin::ZERO)
}

/// Room around the bar's contents (the margin an ordinary top panel has).
const PADDING: egui::Margin = egui::Margin::symmetric(8, 2);

/// Width of each window button.
const BUTTON_WIDTH: f32 = 42.0;

/// The close button's fill under the pointer, and when pressed.
const CLOSE_HOVER: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
const CLOSE_PRESSED: egui::Color32 = egui::Color32::from_rgb(0x9E, 0x22, 0x16);

/// Whether the window fills the screen's work area now.
pub fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Minimizes the window (Alt+M, and the first window button).
pub fn minimize(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
}

/// Maximizes the window, or restores it when it is maximized (F11, the second
/// window button, a double click on the bar).
pub fn toggle_maximized(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
}

/// Closes the window the way the app always does: eframe then runs `on_exit`,
/// which saves the settings.
pub fn close(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Close);
}

/// One window button in `rect`: the hover fill (red for close), then its glyph
/// painted. Not a Tab stop: each has its shortcut. True when clicked.
fn window_button(ui: &mut egui::Ui, button: Button, rect: egui::Rect) -> bool {
    // Drag too, so a press that wanders off the button is not the bar's drag.
    let response = ui.interact(rect, button.id(), egui::Sense::CLICK | egui::Sense::DRAG);
    let close = button == Button::Close;
    let fill = match (response.is_pointer_button_down_on(), response.hovered()) {
        (true, _) if close => Some(CLOSE_PRESSED),
        (_, true) if close => Some(CLOSE_HOVER),
        (true, _) => Some(ui.visuals().widgets.active.weak_bg_fill),
        (_, true) => Some(ui.visuals().widgets.hovered.weak_bg_fill),
        _ => None,
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 0, fill);
    }
    let color = match (response.hovered(), close) {
        (true, true) => egui::Color32::WHITE,
        (true, false) => ui.visuals().strong_text_color(),
        _ => ui.visuals().text_color(),
    };
    let glyph = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(24.0));
    match button {
        Button::SearchTabs => crate::ui::icons::paint_search_tabs(ui.painter(), glyph, color),
        Button::Minimize => crate::ui::icons::paint_minimize(ui.painter(), glyph, color),
        Button::Maximize if maximized(ui.ctx()) => crate::ui::icons::paint_restore(ui.painter(), glyph, color),
        Button::Maximize => crate::ui::icons::paint_maximize(ui.painter(), glyph, color),
        Button::Close => crate::ui::icons::paint_close(ui.painter(), glyph, color),
    }
    let (name, shortcut) = button.hint();
    crate::ui::app_menu::command_hint(response, name, shortcut).clicked()
}

/// Draws `contents` in the caption bar, whose empty space moves the window
/// (a press and a short drag) and maximizes or restores it on a double click.
/// The window buttons end the bar; the contents never reach under them.
pub fn show<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    show_buttons(ui, false, contents).0
}

/// `show` for the main bar: a ▾ right before minimize (Search Tabs) takes one
/// more button's room. True when it was clicked.
pub fn show_with_search_tabs<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    show_buttons(ui, true, contents)
}

fn show_buttons<R>(ui: &mut egui::Ui, search_tabs: bool, contents: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    let buttons: Vec<Button> = search_tabs.then_some(Button::SearchTabs).into_iter().chain(Button::ALL).collect();
    let bar_rect = ui.max_rect();
    // Sensed before the contents, so every control drawn on top keeps its own clicks;
    // not focusable, so the bar is no Tab stop.
    let bar = ui.interact(bar_rect, bar_id(), egui::Sense::CLICK | egui::Sense::DRAG);
    let buttons_left = bar_rect.right() - BUTTON_WIDTH * buttons.len() as f32;
    let room = egui::Rect::from_min_max(bar_rect.min, egui::pos2(buttons_left, bar_rect.bottom()));
    let inside = ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| egui::Frame::new().inner_margin(PADDING).show(ui, contents).inner);
    let mut x = buttons_left;
    let mut searched = false;
    for button in buttons {
        let rect = egui::Rect::from_min_size(egui::pos2(x, bar_rect.top()), egui::vec2(BUTTON_WIDTH, inside.response.rect.height()));
        if window_button(ui, button, rect) {
            let ctx = ui.ctx();
            match button {
                Button::SearchTabs => searched = true,
                Button::Minimize => minimize(ctx),
                Button::Maximize => toggle_maximized(ctx),
                Button::Close => close(ctx),
            }
        }
        x += BUTTON_WIDTH;
    }
    let ctx = ui.ctx();
    if bar.double_clicked() {
        toggle_maximized(ctx);
    } else if bar.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    (inside.inner, searched)
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
        let (output, clicked) = draw(ctx, input);
        let commands = output.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default();
        (commands, clicked, output.platform_output.cursor_icon)
    }

    /// Runs `input` through the caption bar and the edges, as the app does.
    fn draw(ctx: &egui::Context, input: RawInput) -> (egui::FullOutput, bool) {
        let mut clicked = false;
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("caption").frame(frame(root.style())).show(root, |ui| {
                show(ui, |ui| {
                    // Where the contents may reach, for the tests that check the buttons' room.
                    ui.ctx().data_mut(|d| d.insert_temp(content_right_id(), ui.max_rect().right()));
                    ui.horizontal(|ui| clicked = ui.button("tab").clicked());
                })
            });
            resize_edges(root.ctx());
        });
        output.textures_delta.clear();
        (output, clicked)
    }

    fn content_right_id() -> egui::Id {
        egui::Id::new("caption_test_content_right")
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

    /// The middle of `button`, once the bar was drawn.
    fn center(ctx: &egui::Context, button: Button) -> Pos2 {
        ctx.read_response(button.id()).expect("the button was drawn").rect.center()
    }

    #[test]
    fn each_window_button_sends_its_command_and_starts_no_drag() {
        let cases = [
            (Button::Minimize, false, ViewportCommand::Minimized(true)),
            (Button::Maximize, false, ViewportCommand::Maximized(true)),
            (Button::Maximize, true, ViewportCommand::Maximized(false)),
            (Button::Close, false, ViewportCommand::Close),
        ];
        for (button, maximized, wanted) in cases {
            let ctx = egui::Context::default();
            window(&ctx, maximized, vec![]);
            let at = center(&ctx, button);
            window(&ctx, maximized, vec![Event::PointerMoved(at)]);
            let mut commands = window(&ctx, maximized, vec![press(at, true)]).0;
            assert!(commands.is_empty(), "{button:?} acts on the release, not the press: {commands:?}");
            commands.extend(window(&ctx, maximized, vec![press(at, false)]).0);
            assert_eq!(commands.iter().filter(|c| **c == wanted).count(), 1, "{button:?} maximized {maximized}: {commands:?}");
            assert_eq!(commands.len(), 1, "{button:?} asks for that and nothing else: {commands:?}");
        }
    }

    #[test]
    fn dragging_from_a_window_button_moves_nothing() {
        for button in Button::ALL {
            let ctx = egui::Context::default();
            window(&ctx, false, vec![]);
            let at = center(&ctx, button);
            window(&ctx, false, vec![Event::PointerMoved(at)]);
            let mut commands = window(&ctx, false, vec![press(at, true)]).0;
            commands.extend(window(&ctx, false, vec![Event::PointerMoved(at - egui::vec2(25.0, 0.0))]).0);
            assert!(commands.is_empty(), "{button:?}: {commands:?}");
        }
    }

    #[test]
    fn the_buttons_end_the_bar_and_the_contents_stop_before_them() {
        let ctx = egui::Context::default();
        window(&ctx, false, vec![]);
        let [min, max, close] = Button::ALL.map(|b| ctx.read_response(b.id()).expect("the button was drawn").rect);
        assert_eq!(close.right(), SCREEN.right(), "the close button is flush with the window's edge");
        assert_eq!((min.right(), max.right()), (max.left(), close.left()), "side by side, minimize first");
        assert!(min.width() > 20.0 && min.height() > 20.0, "big enough to hit: {min:?}");
        assert_eq!((min.top(), max.top(), close.top()), (SCREEN.top(), SCREEN.top(), SCREEN.top()), "from the window's top");
        let content_right = ctx.data(|d| d.get_temp::<f32>(content_right_id())).expect("the contents were drawn");
        assert!(content_right <= min.left(), "the contents end at {content_right}, the buttons start at {}", min.left());
    }

    /// One frame of the main bar, with the Search Tabs button; whether it was clicked.
    fn main_bar(ctx: &egui::Context, events: Vec<Event>) -> bool {
        let input = RawInput { events, screen_rect: Some(SCREEN), ..RawInput::default() };
        let mut clicked = false;
        ctx.run_ui(input, |root| {
            egui::Panel::top("caption").frame(frame(root.style())).show(root, |ui| {
                clicked = show_with_search_tabs(ui, |ui| {
                    ui.ctx().data_mut(|d| d.insert_temp(content_right_id(), ui.max_rect().right()));
                })
                .1;
            });
        })
        .drop_without_applying_deltas();
        clicked
    }

    #[test]
    fn the_search_tabs_button_sits_left_of_minimize_and_reports_its_click() {
        let ctx = egui::Context::default();
        main_bar(&ctx, vec![]);
        let search = ctx.read_response(Button::SearchTabs.id()).expect("the button was drawn").rect;
        let min = ctx.read_response(Button::Minimize.id()).expect("minimize was drawn").rect;
        assert_eq!(search.right(), min.left(), "right before minimize");
        assert_eq!(search.size(), min.size(), "as big as the window buttons");
        let content_right = ctx.data(|d| d.get_temp::<f32>(content_right_id())).expect("the contents were drawn");
        assert!(content_right <= search.left(), "the contents stop before it");
        assert!(!ctx.read_response(Button::SearchTabs.id()).unwrap().sense.is_focusable(), "Ctrl+Shift+A instead of a Tab stop");
        let at = search.center();
        main_bar(&ctx, vec![Event::PointerMoved(at)]);
        assert!(!main_bar(&ctx, vec![press(at, true)]), "on the release");
        assert!(main_bar(&ctx, vec![press(at, false)]));
        assert_eq!(Button::SearchTabs.hint(), ("Search Tabs", "Ctrl+Shift+A"));
    }

    #[test]
    fn the_setup_strip_has_no_search_tabs_button() {
        let ctx = egui::Context::default();
        window(&ctx, false, vec![]);
        assert!(ctx.read_response(Button::SearchTabs.id()).is_none());
    }

    #[test]
    fn the_window_buttons_are_no_tab_stops() {
        let ctx = egui::Context::default();
        window(&ctx, false, vec![]);
        for button in Button::ALL {
            let sense = ctx.read_response(button.id()).expect("the button was drawn").sense;
            assert!(!sense.is_focusable(), "{button:?} has its shortcut instead of a Tab stop");
        }
    }

    #[test]
    fn hovering_a_window_button_names_its_command_and_shortcut() {
        assert_eq!(Button::Minimize.hint(), ("Minimize Window", "Alt+M"));
        assert_eq!(Button::Maximize.hint(), ("Toggle Fullscreen", "F11"));
        assert_eq!(Button::Close.hint(), ("Close Window", "Alt+F4"));
        for button in Button::ALL {
            let ctx = egui::Context::default();
            ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
            window(&ctx, false, vec![]);
            let at = center(&ctx, button);
            let mut texts = Vec::new();
            for time in 1..6 {
                let events = if time == 1 { vec![Event::PointerMoved(at)] } else { vec![] };
                let input = RawInput { events, screen_rect: Some(SCREEN), time: Some(f64::from(time)), ..RawInput::default() };
                let (output, _) = draw(&ctx, input);
                texts = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                        _ => None,
                    })
                    .collect();
            }
            let (name, shortcut) = button.hint();
            for wanted in ["Cmd:", name, shortcut] {
                assert!(texts.iter().any(|t| t == wanted), "{button:?}: {wanted:?} in {texts:?}");
            }
        }
    }
}
