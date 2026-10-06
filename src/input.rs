//! All keyboard shortcuts live here so they are easy to find and change.

use eframe::egui::{Context, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move the selection by this many rows (negative = up).
    Move(isize),
    First,
    Last,
    /// Enter the selected folder or open the selected file.
    Activate,
    GoParent,
    FocusPathBar,
    ToggleHidden,
}

/// Rows jumped by PageUp/PageDown.
const PAGE: isize = 20;

/// Reads this frame's shortcuts. List navigation keys are ignored while a text
/// field (the path bar) has keyboard focus, so typing never moves the selection.
pub fn read_actions(ctx: &Context) -> Vec<Action> {
    let typing = ctx.egui_wants_keyboard_input();
    ctx.input_mut(|i| {
        let mut actions = Vec::new();
        if i.consume_key(Modifiers::COMMAND, Key::L) {
            actions.push(Action::FocusPathBar);
        }
        if typing {
            return actions;
        }
        if i.consume_key(Modifiers::COMMAND, Key::H) {
            actions.push(Action::ToggleHidden);
        }
        let plain = [
            (Key::ArrowDown, Action::Move(1)),
            (Key::ArrowUp, Action::Move(-1)),
            (Key::PageDown, Action::Move(PAGE)),
            (Key::PageUp, Action::Move(-PAGE)),
            (Key::Home, Action::First),
            (Key::End, Action::Last),
            (Key::Enter, Action::Activate),
            (Key::Backspace, Action::GoParent),
        ];
        for (key, action) in plain {
            // Loop so key repeat (holding an arrow) moves once per repeat event.
            while i.consume_key(Modifiers::NONE, key) {
                actions.push(action);
            }
        }
        actions
    })
}
