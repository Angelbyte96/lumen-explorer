//! All keyboard shortcuts live here so they are easy to find and change.

use eframe::egui::{Context, Event, Id, Key, Modifiers, MouseWheelUnit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move the selection by this many rows (negative = up).
    Move(isize),
    First,
    Last,
    /// Shift+arrows, PageUp/PageDown, Home/End: move this many rows selecting
    /// the range (`isize::MIN` / `isize::MAX` reach the first / last row).
    Extend(isize),
    /// Ctrl+A: every row shown.
    SelectAll,
    /// Shift+F10: the right-click menu for the selection.
    Menu,
    /// Enter the selected folder or open the selected file.
    Activate,
    /// Backspace: erases a filter character, or goes up when the filter is empty.
    GoParent,
    /// Backspace held down (key repeat).
    GoParentHeld,
    /// Alt+Up: the parent folder, whatever the filter holds.
    Up,
    FocusPathBar,
    ToggleHidden,
    /// Previous / next folder in the history (Alt+Left / Alt+Right).
    Back,
    Forward,
    /// A typed character for the filter bar; a leading Space toggles the preview.
    TypeChar(char),
    /// Move the selected entry to the trash (asks first).
    Trash,
    /// Shift+Delete: delete the selection for good, without the trash (asks first).
    DeleteForever,
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
    /// Bigger / smaller icons and rows in the list (Ctrl+Plus / Ctrl+Minus,
    /// Ctrl+wheel); Ctrl+0 goes back to the normal size.
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// Every list column's width to its content (Ctrl+Shift+F).
    FitColumns,
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

/// Reads this frame's shortcuts. The list's keys (arrows, Enter, Delete, typed
/// letters, the clipboard...) reach it only while it holds the keys: with
/// another part of the window focused they stay there, so moving through the
/// side panel never moves the selection. Window shortcuts work anywhere but in
/// a text field, which keeps all but Ctrl+L.
pub fn read_actions(ctx: &Context) -> Vec<Action> {
    let typing = ctx.text_edit_focused();
    let list = list_has_keys(ctx);
    let mut actions = read_keys(ctx, typing, list);
    // Outside `input_mut`: egui's context lock is not reentrant.
    let events = ctx.input(|i| i.events.clone());
    if list {
        actions.extend(clipboard_actions(ctx, &events));
    }
    if !typing {
        actions.extend(wheel_zoom(ctx, &events));
    }
    actions
}

/// The list holds the keys while it has the keyboard focus or nothing has.
/// egui drops the focus on Escape before the app reads the keys: that Escape
/// only gave the keys back, so it must not also clear the filter.
fn list_has_keys(ctx: &Context) -> bool {
    let last_id = Id::new("lumen_focus_last_frame");
    let now = ctx.memory(|m| m.focused());
    let before = ctx.data(|d| d.get_temp::<Option<Id>>(last_id)).flatten();
    ctx.data_mut(|d| d.insert_temp(last_id, now));
    let list = crate::ui::file_list::focus_id();
    if now.is_none() && before.is_some_and(|id| id != list) {
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { key: Key::Escape, .. })));
    }
    now.is_none_or(|id| id == list)
}

/// Touchpad scrolling, in points, needed for one zoom level.
const ZOOM_SWIPE: f32 = 50.0;

/// Ctrl+wheel: a mouse wheel zooms one level per notch; a touchpad sends many
/// small scrolls, so they add up to `ZOOM_SWIPE` per level.
fn wheel_zoom(ctx: &Context, events: &[Event]) -> Vec<Action> {
    let swiped_id = Id::new("lumen_zoom_swipe");
    let mut swiped = ctx.data(|d| d.get_temp::<f32>(swiped_id)).unwrap_or(0.0);
    let mut actions = Vec::new();
    for event in events {
        let Event::MouseWheel { unit, delta, modifiers, .. } = event else { continue };
        if !modifiers.command || delta.y == 0.0 {
            continue;
        }
        let step = |up: bool| if up { Action::ZoomIn } else { Action::ZoomOut };
        if *unit == MouseWheelUnit::Point {
            swiped += delta.y;
            while swiped.abs() >= ZOOM_SWIPE {
                actions.push(step(swiped > 0.0));
                swiped -= ZOOM_SWIPE.copysign(swiped);
            }
        } else {
            actions.push(step(delta.y > 0.0));
        }
    }
    ctx.data_mut(|d| d.insert_temp(swiped_id, swiped));
    actions
}

fn read_keys(ctx: &Context, typing: bool, list: bool) -> Vec<Action> {
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
        if i.consume_key(Modifiers::ALT, Key::ArrowUp) {
            actions.push(Action::Up);
        }
        if i.consume_key(Modifiers::ALT, Key::R) {
            actions.push(Action::Count);
        }
        // Ctrl++ needs Shift on some layouts (Ctrl+Shift+=); matching ignores it.
        if i.consume_key(Modifiers::COMMAND, Key::Plus) || i.consume_key(Modifiers::COMMAND, Key::Equals) {
            actions.push(Action::ZoomIn);
        }
        if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::F) {
            actions.push(Action::FitColumns);
        }
        if i.consume_key(Modifiers::COMMAND, Key::Minus) {
            actions.push(Action::ZoomOut);
        }
        if i.consume_key(Modifiers::COMMAND, Key::Num0) {
            actions.push(Action::ZoomReset);
        }
        if !list {
            return actions;
        }
        if i.consume_key(Modifiers::COMMAND, Key::A) {
            actions.push(Action::SelectAll);
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
        if i.modifiers == Modifiers::SHIFT && i.consume_key(Modifiers::SHIFT, Key::Delete) {
            actions.push(Action::DeleteForever);
        }
        if i.consume_key(Modifiers::SHIFT, Key::F10) {
            actions.push(Action::Menu);
        }
        if i.modifiers == Modifiers::SHIFT {
            let extend = [
                (Key::ArrowDown, 1),
                (Key::ArrowUp, -1),
                (Key::PageDown, PAGE),
                (Key::PageUp, -PAGE),
                (Key::Home, isize::MIN),
                (Key::End, isize::MAX),
            ];
            for (key, rows) in extend {
                let presses = i.count_and_consume_key(Modifiers::SHIFT, key);
                actions.extend(std::iter::repeat_n(Action::Extend(rows), presses));
            }
        }
        // egui ignores extra Shift/Alt when matching, so Shift+Arrow would act
        // like a plain arrow. Require no modifiers; other combos are reserved.
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
            // On Windows egui-winit turns Shift+Delete into a cut (its old
            // shortcut) and sends no Delete press: here it deletes, as in Explorer.
            Event::Cut if ctx.input(|i| i.modifiers.shift && !i.modifiers.command) => actions.push(Action::DeleteForever),
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
    use eframe::egui::{self, RawInput};

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
    fn alt_up_goes_to_the_parent_and_a_plain_up_still_moves() {
        let actions = actions_for(vec![Event::ModifiersChanged(Modifiers::ALT), key(Key::ArrowUp, Modifiers::ALT)]);
        assert_eq!(actions, [Action::Up]);
        assert_eq!(actions_for(vec![key(Key::ArrowUp, Modifiers::NONE)]), [Action::Move(-1)]);
    }

    #[test]
    fn delete_asks_to_trash_but_not_with_modifiers() {
        assert_eq!(actions_for(vec![key(Key::Delete, Modifiers::NONE)]), [Action::Trash]);
        // Real input reports held modifiers both ways.
        let shift_delete =
            vec![Event::ModifiersChanged(Modifiers::SHIFT), key(Key::Delete, Modifiers::SHIFT)];
        assert_eq!(actions_for(shift_delete), [Action::DeleteForever], "Shift+Delete skips the trash");
        let ctrl_delete = vec![Event::ModifiersChanged(Modifiers::CTRL), key(Key::Delete, Modifiers::CTRL)];
        assert_eq!(actions_for(ctrl_delete), [], "other combos stay reserved");
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
    fn ctrl_a_selects_every_row_and_types_nothing() {
        let events = vec![
            Event::ModifiersChanged(Modifiers::COMMAND),
            key(Key::A, Modifiers::COMMAND),
        ];
        assert_eq!(actions_for(events), [Action::SelectAll]);
        let plain = vec![key(Key::A, Modifiers::NONE), Event::Text("a".into())];
        assert_eq!(actions_for(plain), [Action::TypeChar('a')], "a plain a is a filter letter");
    }

    #[test]
    fn shift_with_the_move_keys_extends_the_selection() {
        let shift = |k| vec![Event::ModifiersChanged(Modifiers::SHIFT), key(k, Modifiers::SHIFT)];
        for (k, action) in [
            (Key::ArrowDown, Action::Extend(1)),
            (Key::ArrowUp, Action::Extend(-1)),
            (Key::PageDown, Action::Extend(PAGE)),
            (Key::PageUp, Action::Extend(-PAGE)),
            (Key::Home, Action::Extend(isize::MIN)),
            (Key::End, Action::Extend(isize::MAX)),
        ] {
            assert_eq!(actions_for(shift(k)), [action], "Shift+{k:?}");
        }
        assert_eq!(actions_for(vec![key(Key::ArrowDown, Modifiers::NONE)]), [Action::Move(1)]);
    }

    // egui-winit sends Shift+Delete on Windows as Event::Cut, with no key press.
    #[test]
    fn shift_delete_sent_as_a_cut_event_still_asks_to_delete() {
        let events = vec![
            Event::ModifiersChanged(Modifiers::SHIFT),
            Event::Cut,
            Event::Key { key: Key::Delete, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::SHIFT },
        ];
        assert_eq!(actions_for(events), [Action::DeleteForever]);
        let ctrl_x = vec![Event::ModifiersChanged(Modifiers::COMMAND), Event::Cut];
        assert_eq!(actions_for(ctrl_x), [Action::Cut], "Ctrl+X still cuts");
        let ctrl_shift_x = vec![Event::ModifiersChanged(Modifiers::COMMAND | Modifiers::SHIFT), Event::Cut];
        assert_eq!(actions_for(ctrl_shift_x), [Action::Cut]);
    }

    #[test]
    fn shift_f10_opens_the_menu_and_a_plain_f10_does_not() {
        let events = vec![Event::ModifiersChanged(Modifiers::SHIFT), key(Key::F10, Modifiers::SHIFT)];
        assert_eq!(actions_for(events), [Action::Menu]);
        assert_eq!(actions_for(vec![key(Key::F10, Modifiers::NONE)]), []);
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

    fn wheel(unit: MouseWheelUnit, y: f32, modifiers: Modifiers) -> Event {
        Event::MouseWheel { unit, delta: egui::vec2(0.0, y), modifiers, phase: egui::TouchPhase::Move }
    }

    #[test]
    fn ctrl_shift_f_sizes_every_column_to_fit() {
        assert_eq!(actions_for(vec![key(Key::F, Modifiers::COMMAND | Modifiers::SHIFT)]), [Action::FitColumns]);
        assert_eq!(actions_for(vec![key(Key::F, Modifiers::COMMAND)]), [], "Ctrl+F alone does nothing");
    }

    #[test]
    fn ctrl_plus_minus_and_zero_zoom_the_list() {
        let with = |key| actions_for(vec![self::key(key, Modifiers::COMMAND)]);
        assert_eq!(with(Key::Plus), [Action::ZoomIn]);
        assert_eq!(with(Key::Equals), [Action::ZoomIn], "Ctrl+= where + needs Shift");
        assert_eq!(with(Key::Minus), [Action::ZoomOut]);
        assert_eq!(with(Key::Num0), [Action::ZoomReset]);
        let shifted = actions_for(vec![self::key(Key::Equals, Modifiers::COMMAND | Modifiers::SHIFT)]);
        assert_eq!(shifted, [Action::ZoomIn], "Ctrl+Shift+= is Ctrl++ on a US keyboard");
        assert_eq!(actions_for(vec![self::key(Key::Minus, Modifiers::NONE)]), []);
    }

    #[test]
    fn ctrl_wheel_zooms_one_level_per_notch_and_a_plain_wheel_does_not() {
        let ctrl = Modifiers::COMMAND;
        assert_eq!(actions_for(vec![wheel(MouseWheelUnit::Line, 1.0, ctrl)]), [Action::ZoomIn]);
        assert_eq!(actions_for(vec![wheel(MouseWheelUnit::Line, -1.0, ctrl)]), [Action::ZoomOut]);
        assert_eq!(actions_for(vec![wheel(MouseWheelUnit::Line, 1.0, Modifiers::NONE)]), []);
    }

    #[test]
    fn ctrl_on_a_touchpad_zooms_once_per_swipe_distance_not_per_event() {
        let ctx = Context::default();
        let swipe = |y| vec![wheel(MouseWheelUnit::Point, y, Modifiers::COMMAND)];
        let mut zooms = Vec::new();
        for _ in 0..10 {
            zooms.extend(frames(&ctx, swipe(10.0))); // 100 points in small steps
        }
        assert_eq!(zooms, [Action::ZoomIn, Action::ZoomIn]);
        assert_eq!(frames(&ctx, swipe(-60.0)), [Action::ZoomOut]);
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

    /// A frame like the app's: the keys are read before a button is drawn;
    /// `focus` puts the keyboard focus on that button.
    fn with_button(ctx: &Context, events: Vec<Event>, focus: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
            actions = read_actions(ui.ctx());
            let button = ui.button("other part");
            if focus {
                button.request_focus();
            }
        })
        .drop_without_applying_deltas();
        actions
    }

    /// A context whose keyboard focus sits on a button, not on the list.
    fn focused_elsewhere() -> Context {
        let ctx = Context::default();
        with_button(&ctx, Vec::new(), true);
        with_button(&ctx, Vec::new(), false);
        assert!(ctx.memory(|m| m.focused()).is_some());
        ctx
    }

    #[test]
    fn the_lists_keys_stay_with_another_focused_part() {
        let ctx = focused_elsewhere();
        let keys = vec![
            key(Key::ArrowDown, Modifiers::NONE),
            key(Key::End, Modifiers::NONE),
            key(Key::ArrowUp, Modifiers::SHIFT),
            key(Key::Enter, Modifiers::NONE),
            key(Key::Delete, Modifiers::NONE),
            key(Key::F2, Modifiers::NONE),
            key(Key::F10, Modifiers::SHIFT),
            key(Key::Backspace, Modifiers::NONE),
            key(Key::A, Modifiers::COMMAND),
            Event::Text("a".into()),
            Event::Copy,
            Event::Paste("t".into()),
        ];
        assert_eq!(with_button(&ctx, keys, false), []);
    }

    #[test]
    fn window_shortcuts_work_wherever_the_focus_is() {
        let ctx = focused_elsewhere();
        let keys = vec![
            key(Key::L, Modifiers::COMMAND),
            key(Key::T, Modifiers::COMMAND),
            key(Key::D, Modifiers::COMMAND),
            key(Key::ArrowLeft, Modifiers::ALT),
            key(Key::Tab, Modifiers::COMMAND),
        ];
        let actions = with_button(&ctx, keys, false);
        assert_eq!(
            actions,
            [Action::FocusPathBar, Action::ToggleBookmark, Action::NewTab, Action::NextTab, Action::Back]
        );
    }

    #[test]
    fn escape_on_another_part_gives_the_list_its_keys_back_without_cancelling() {
        let ctx = focused_elsewhere();
        let escape = vec![key(Key::Escape, Modifiers::NONE), repeat(Key::Escape)];
        assert_eq!(with_button(&ctx, escape, false), []);
        assert!(ctx.memory(|m| m.focused()).is_none());
        let down = vec![release(Key::Escape, Modifiers::NONE), key(Key::ArrowDown, Modifiers::NONE)];
        assert_eq!(with_button(&ctx, down, false), [Action::Move(1)]);
        // The next Escape is the list's again.
        let escape = vec![key(Key::Escape, Modifiers::NONE)];
        assert_eq!(with_button(&ctx, escape, false), [Action::Cancel]);
    }

    #[test]
    fn the_focused_list_keeps_its_keys_and_its_escape() {
        let ctx = Context::default();
        let stop = |ctx: &Context, events| {
            let mut actions = Vec::new();
            ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
                actions = read_actions(ui.ctx());
                crate::ui::file_list::focus_stop(ui, ui.max_rect());
            })
            .drop_without_applying_deltas();
            actions
        };
        stop(&ctx, Vec::new());
        ctx.memory_mut(|m| m.request_focus(crate::ui::file_list::focus_id()));
        stop(&ctx, Vec::new());
        assert_eq!(ctx.memory(|m| m.focused()), Some(crate::ui::file_list::focus_id()));
        let keys = vec![key(Key::ArrowDown, Modifiers::NONE), key(Key::Enter, Modifiers::NONE), Event::Text("a".into())];
        assert_eq!(stop(&ctx, keys), [Action::Move(1), Action::Activate, Action::TypeChar('a')]);
        assert_eq!(stop(&ctx, vec![key(Key::Escape, Modifiers::NONE)]), [Action::Cancel]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(crate::ui::file_list::focus_id()));
    }
}
