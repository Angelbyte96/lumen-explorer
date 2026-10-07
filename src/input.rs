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
    /// Previous / next folder in the history (Alt+Left / Alt+Right).
    Back,
    Forward,
    /// A typed character for type-to-search.
    TypeChar(char),
    /// Move the selected entry to the trash (asks first).
    Trash,
    /// Edit the selected entry's name in place.
    Rename,
    /// Create a folder here and start renaming it (Ctrl+Shift+N).
    NewFolder,
}

/// Answer to an open confirmation dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Yes,
    No,
}

/// While a dialog is open only Enter and Escape matter; everything else is
/// left alone so the list underneath does not react.
pub fn read_confirm(ctx: &Context) -> Option<Confirm> {
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            Some(Confirm::Yes)
        } else if i.consume_key(Modifiers::NONE, Key::Escape) {
            Some(Confirm::No)
        } else {
            None
        }
    })
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
        if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::N) {
            actions.push(Action::NewFolder);
        }
        if i.consume_key(Modifiers::COMMAND, Key::H) {
            actions.push(Action::ToggleHidden);
        }
        if i.consume_key(Modifiers::ALT, Key::ArrowLeft) {
            actions.push(Action::Back);
        }
        if i.consume_key(Modifiers::ALT, Key::ArrowRight) {
            actions.push(Action::Forward);
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
            (Key::Delete, Action::Trash),
            (Key::F2, Action::Rename),
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
    fn alt_arrows_walk_history_and_plain_left_right_do_nothing() {
        let actions = actions_for(vec![
            key(Key::ArrowLeft, Modifiers::ALT),
            key(Key::ArrowRight, Modifiers::ALT),
        ]);
        assert_eq!(actions, [Action::Back, Action::Forward]);
        let actions = actions_for(vec![
            key(Key::ArrowLeft, Modifiers::NONE),
            key(Key::ArrowRight, Modifiers::NONE),
        ]);
        assert_eq!(actions, []);
    }

    #[test]
    fn delete_asks_to_trash_but_not_with_modifiers() {
        assert_eq!(actions_for(vec![key(Key::Delete, Modifiers::NONE)]), [Action::Trash]);
        // Real input reports held modifiers both ways; Shift+Delete is reserved.
        let shift_delete =
            vec![Event::ModifiersChanged(Modifiers::SHIFT), key(Key::Delete, Modifiers::SHIFT)];
        assert_eq!(actions_for(shift_delete), []);
    }

    #[test]
    fn f2_renames() {
        assert_eq!(actions_for(vec![key(Key::F2, Modifiers::NONE)]), [Action::Rename]);
    }

    #[test]
    fn ctrl_shift_n_creates_a_folder_and_types_nothing() {
        let events = vec![
            Event::ModifiersChanged(Modifiers::COMMAND | Modifiers::SHIFT),
            key(Key::N, Modifiers::COMMAND | Modifiers::SHIFT),
            Event::Text("N".into()),
        ];
        assert_eq!(actions_for(events), [Action::NewFolder]);
    }

    fn confirm_for(events: Vec<Event>) -> Option<Confirm> {
        let ctx = Context::default();
        let mut answer = None;
        ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
            answer = read_confirm(ui.ctx());
        })
        .drop_without_applying_deltas();
        answer
    }

    #[test]
    fn enter_confirms_and_escape_cancels() {
        assert_eq!(confirm_for(vec![key(Key::Enter, Modifiers::NONE)]), Some(Confirm::Yes));
        assert_eq!(confirm_for(vec![key(Key::Escape, Modifiers::NONE)]), Some(Confirm::No));
        assert_eq!(confirm_for(vec![key(Key::ArrowDown, Modifiers::NONE)]), None);
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
