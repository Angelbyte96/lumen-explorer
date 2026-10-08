//! All keyboard shortcuts live here so they are easy to find and change.

use eframe::egui::{Context, Event, Id, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move the selection by this many rows (negative = up).
    Move(isize),
    First,
    Last,
    /// Enter the selected folder or open the selected file.
    Activate,
    /// Backspace: erases a filter character, or goes up when the filter is empty.
    GoParent,
    /// Backspace held down (key repeat).
    GoParentHeld,
    FocusPathBar,
    ToggleHidden,
    /// Previous / next folder in the history (Alt+Left / Alt+Right).
    Back,
    Forward,
    /// A typed character for the filter bar; a leading Space toggles the preview.
    TypeChar(char),
    /// Move the selected entry to the trash (asks first).
    Trash,
    /// Edit the selected entry's name in place.
    Rename,
    /// Create a folder here and start renaming it (Ctrl+Shift+N).
    NewFolder,
    /// Put the selected entry in Lumen's clipboard (Ctrl+C / Ctrl+X).
    Copy,
    Cut,
    /// Paste the clipboard into the current folder (Ctrl+V).
    Paste,
    /// Alt+R: count the selected folder's size, folders and files.
    Count,
    /// Ctrl+T: open the current folder in a new tab.
    NewTab,
    /// Ctrl+W: close the tab on screen (not the last one).
    CloseTab,
    /// Ctrl+Tab / Ctrl+Shift+Tab: the next / previous tab, wrapping around.
    NextTab,
    PrevTab,
    /// Ctrl+D: bookmark the current folder, or remove its bookmark.
    ToggleBookmark,
    /// Escape: clears the filter, else stops a running count, else the copy or move.
    Cancel,
    /// Escape held down (key repeat).
    CancelHeld,
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
    let mut actions = read_keys(ctx, typing);
    if !typing {
        // Outside `input_mut`: egui's context lock is not reentrant.
        let events = ctx.input(|i| i.events.clone());
        actions.extend(clipboard_actions(ctx, &events));
    }
    actions
}

fn read_keys(ctx: &Context, typing: bool) -> Vec<Action> {
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
        if i.consume_key(Modifiers::COMMAND, Key::D) {
            actions.push(Action::ToggleBookmark);
        }
        if i.consume_key(Modifiers::COMMAND, Key::T) {
            actions.push(Action::NewTab);
        }
        if i.consume_key(Modifiers::COMMAND, Key::W) {
            actions.push(Action::CloseTab);
        }
        // With Shift first: matching ignores an extra Shift.
        if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
            actions.push(Action::PrevTab);
        }
        if i.consume_key(Modifiers::COMMAND, Key::Tab) {
            actions.push(Action::NextTab);
        }
        if i.consume_key(Modifiers::ALT, Key::ArrowLeft) {
            actions.push(Action::Back);
        }
        if i.consume_key(Modifiers::ALT, Key::ArrowRight) {
            actions.push(Action::Forward);
        }
        if i.consume_key(Modifiers::ALT, Key::R) {
            actions.push(Action::Count);
        }
        let plain = [
            (Key::ArrowDown, Action::Move(1)),
            (Key::ArrowUp, Action::Move(-1)),
            (Key::PageDown, Action::Move(PAGE)),
            (Key::PageUp, Action::Move(-PAGE)),
            (Key::Home, Action::First),
            (Key::End, Action::Last),
            (Key::Enter, Action::Activate),
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
            // These act on the filter first; the app must know a held key from
            // a new press so holding one does not run past an emptied filter.
            for (key, press, held) in [
                (Key::Backspace, Action::GoParent, Action::GoParentHeld),
                (Key::Escape, Action::Cancel, Action::CancelHeld),
            ] {
                i.events.retain(|event| match event {
                    Event::Key { key: k, pressed: true, repeat, .. } if *k == key => {
                        actions.push(if *repeat { held } else { press });
                        false
                    }
                    _ => true,
                });
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

/// egui-winit turns Ctrl+C / Ctrl+X into `Copy` / `Cut` events, but Ctrl+V
/// becomes a `Paste` event only when the system clipboard holds text; with
/// anything else only the V release arrives. Lumen's clipboard is its own, so
/// paste on either, and not twice for the same keystroke.
fn clipboard_actions(ctx: &Context, events: &[Event]) -> Vec<Action> {
    let pasted_id = Id::new("lumen_paste_event_seen");
    let mut pasted = ctx.data(|d| d.get_temp::<bool>(pasted_id)).unwrap_or(false);
    let mut actions = Vec::new();
    for event in events {
        match event {
            Event::Copy => actions.push(Action::Copy),
            Event::Cut => actions.push(Action::Cut),
            Event::Paste(_) => {
                actions.push(Action::Paste);
                pasted = true;
            }
            Event::Key { key: Key::V, pressed: false, modifiers, .. } if modifiers.command => {
                if !pasted {
                    actions.push(Action::Paste);
                }
                pasted = false;
            }
            _ => {}
        }
    }
    ctx.data_mut(|d| d.insert_temp(pasted_id, pasted));
    actions
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
    fn alt_r_counts_and_types_nothing() {
        let actions = actions_for(vec![
            Event::ModifiersChanged(Modifiers::ALT),
            key(Key::R, Modifiers::ALT),
            Event::Text("r".into()),
        ]);
        assert_eq!(actions, [Action::Count]);
        let actions = actions_for(vec![key(Key::R, Modifiers::NONE), Event::Text("r".into())]);
        assert_eq!(actions, [Action::TypeChar('r')], "a plain r is a filter letter");
    }

    #[test]
    fn ctrl_d_toggles_the_bookmark_and_types_nothing() {
        let events = vec![
            Event::ModifiersChanged(Modifiers::COMMAND),
            key(Key::D, Modifiers::COMMAND),
            Event::Text("d".into()),
        ];
        assert_eq!(actions_for(events), [Action::ToggleBookmark]);
    }

    #[test]
    fn ctrl_t_w_and_tab_manage_tabs() {
        let with = |m: Modifiers, k: Key| actions_for(vec![Event::ModifiersChanged(m), key(k, m)]);
        assert_eq!(with(Modifiers::COMMAND, Key::T), [Action::NewTab]);
        assert_eq!(with(Modifiers::COMMAND, Key::W), [Action::CloseTab]);
        assert_eq!(with(Modifiers::COMMAND, Key::Tab), [Action::NextTab]);
        assert_eq!(with(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab), [Action::PrevTab]);
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

    fn release(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }
    }

    #[test]
    fn clipboard_events_become_copy_cut_and_paste() {
        let actions = actions_for(vec![Event::Copy, Event::Cut, Event::Paste("any".into())]);
        assert_eq!(actions, [Action::Copy, Action::Cut, Action::Paste]);
    }

    /// One context across frames, like the running app.
    fn frames(ctx: &Context, events: Vec<Event>) -> Vec<Action> {
        let mut actions = Vec::new();
        ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
            actions = read_actions(ui.ctx());
        })
        .drop_without_applying_deltas();
        actions
    }

    #[test]
    fn ctrl_v_with_no_text_in_the_system_clipboard_pastes_on_release() {
        // egui-winit sends no press event then, only the release.
        let ctx = Context::default();
        assert_eq!(frames(&ctx, vec![release(Key::V, Modifiers::COMMAND)]), [Action::Paste]);
        assert_eq!(frames(&ctx, vec![release(Key::V, Modifiers::NONE)]), []);
    }

    #[test]
    fn a_paste_event_is_not_repeated_by_its_release() {
        let ctx = Context::default();
        assert_eq!(frames(&ctx, vec![Event::Paste("t".into())]), [Action::Paste]);
        assert_eq!(frames(&ctx, vec![release(Key::V, Modifiers::COMMAND)]), []);
        // The next Ctrl+V with an empty system clipboard works again.
        assert_eq!(frames(&ctx, vec![release(Key::V, Modifiers::COMMAND)]), [Action::Paste]);
    }

    #[test]
    fn escape_cancels() {
        assert_eq!(actions_for(vec![key(Key::Escape, Modifiers::NONE)]), [Action::Cancel]);
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

    fn repeat(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: true, modifiers: Modifiers::NONE }
    }

    #[test]
    fn held_backspace_and_escape_are_told_apart_from_a_fresh_press() {
        let held = vec![key(Key::Backspace, Modifiers::NONE), repeat(Key::Backspace), repeat(Key::Backspace)];
        assert_eq!(actions_for(held), [Action::GoParent, Action::GoParentHeld, Action::GoParentHeld]);
        let held = vec![key(Key::Escape, Modifiers::NONE), repeat(Key::Escape)];
        assert_eq!(actions_for(held), [Action::Cancel, Action::CancelHeld]);
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
