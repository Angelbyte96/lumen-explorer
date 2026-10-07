//! All keyboard shortcuts live here so they are easy to find and change.

use eframe::egui::{Context, Event, Key, Modifiers};

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
    /// A typed character for type-to-search.
    TypeChar(char),
}

/// Rows jumped by PageUp/PageDown.
const PAGE: isize = 20;

/// Reads this frame's shortcuts. List navigation keys are ignored while a text
/// field (the path bar) has keyboard focus, so typing never moves the selection.
/// Only text fields count: a row focused with Tab must not swallow the arrows.
pub fn read_actions(ctx: &Context) -> Vec<Action> {
    let typing = ctx.text_edit_focused();
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
        // egui ignores extra Shift/Alt when matching, so Shift+Arrow would act
        // like a plain arrow. Require no modifiers; those combos are reserved.
        if i.modifiers.is_none() {
            for (key, action) in plain {
                // Counts key-repeat events too (holding an arrow).
                let presses = i.count_and_consume_key(Modifiers::NONE, key);
                actions.extend(std::iter::repeat_n(action, presses));
            }
        }
        let m = i.modifiers;
        if !(m.ctrl || m.command || m.alt) {
            for event in &i.events {
                if let Event::Text(text) = event {
                    actions.extend(text.chars().map(Action::TypeChar));
                }
            }
        }
        actions
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::RawInput;

    /// Runs one frame with these events and returns what `read_actions` saw.
    fn actions_for(events: Vec<Event>) -> Vec<Action> {
        let ctx = Context::default();
        let mut actions = Vec::new();
        ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
            actions = read_actions(ui.ctx());
        })
        .drop_without_applying_deltas();
        actions
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn typed_text_becomes_search_chars() {
        let actions = actions_for(vec![Event::Text("do".into()), Event::Text("c".into())]);
        assert_eq!(
            actions,
            [Action::TypeChar('d'), Action::TypeChar('o'), Action::TypeChar('c')]
        );
    }

    #[test]
    fn text_with_ctrl_or_alt_held_is_not_a_search() {
        for held in [Modifiers::CTRL, Modifiers::ALT] {
            let actions = actions_for(vec![
                Event::ModifiersChanged(held),
                Event::Text("d".into()),
            ]);
            assert_eq!(actions, [], "modifiers: {held:?}");
        }
    }

    #[test]
    fn plain_keys_still_navigate() {
        let actions = actions_for(vec![
            key(Key::ArrowDown, Modifiers::NONE),
            key(Key::Backspace, Modifiers::NONE),
        ]);
        assert_eq!(actions, [Action::Move(1), Action::GoParent]);
    }
}
