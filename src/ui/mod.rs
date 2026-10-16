pub mod confirm;
pub mod context_menu;
pub mod dnd;
pub mod file_icons;
pub mod file_list;
pub mod grid;
pub mod icon_grid;
pub mod icons;
pub mod list_view;
pub mod path_popup;
pub mod preview;
pub mod sidebar;
pub mod status_bar;
pub mod tabs;
pub mod theme;
pub mod thumbnails;
pub mod toolbar;
pub mod tree;
pub mod view_picker;
pub mod view_size;

use eframe::egui;

/// The panels whose border drags to resize them: egui makes that border a
/// Tab stop although it does nothing with keys.
pub const RESIZABLE_PANELS: [&str; 2] = ["sidebar", "preview"];

/// Passes the keyboard focus over a panel's resize border, the way Tab or
/// Shift+Tab was going. Call each frame before the panels are drawn.
pub fn skip_panel_borders(ctx: &egui::Context) {
    let back_id = egui::Id::new("lumen_tab_back");
    let tab = ctx.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } => Some(modifiers.shift),
            _ => None,
        })
    });
    if let Some(back) = tab {
        ctx.data_mut(|d| d.insert_temp(back_id, back));
    }
    let Some(focused) = ctx.memory(|m| m.focused()) else { return };
    if RESIZABLE_PANELS.iter().any(|&panel| egui::Id::new(panel).with("__resize") == focused) {
        let back = ctx.data(|d| d.get_temp::<bool>(back_id)).unwrap_or(false);
        let way = if back { egui::FocusDirection::Previous } else { egui::FocusDirection::Next };
        ctx.memory_mut(|m| m.move_focus(way));
    }
}

/// A ring round `response` while it has the keyboard focus, so it shows
/// where Tab landed and what Enter or Space will press.
pub fn focus_ring(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        let stroke = ui.visuals().selection.stroke;
        ui.painter().rect_stroke(response.rect, 3, stroke, egui::StrokeKind::Inside);
    }
}

/// Drives egui frame by frame, with real pointer events, for UI tests.
#[cfg(test)]
pub mod testing {
    use eframe::egui::{self, Event, PointerButton, Pos2, RawInput};

    pub const SCREEN: egui::Rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

    /// Runs one frame with these events; returns what `draw` returned.
    pub fn frame<R>(ctx: &egui::Context, events: Vec<Event>, mut draw: impl FnMut(&mut egui::Ui) -> R) -> R {
        let mut out = None;
        let input = RawInput { events, screen_rect: Some(SCREEN), ..RawInput::default() };
        ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| out = Some(draw(ui)));
        })
        .drop_without_applying_deltas();
        out.expect("the frame ran")
    }

    /// Lays out, then presses and releases the primary button at `pos`;
    /// returns what `ui` returned on the release frame.
    pub fn click<R>(ctx: &egui::Context, pos: Pos2, mut ui: impl FnMut(&mut egui::Ui) -> R) -> R {
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(ctx, vec![Event::PointerMoved(pos)], &mut ui);
        frame(ctx, vec![button(true)], &mut ui);
        frame(ctx, vec![button(false)], &mut ui)
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, FontId};

    /// egui's default fonts lack many symbols (arrows, most emoji); a missing
    /// one is drawn as an empty box. Every symbol the UI writes is listed here.
    #[test]
    fn every_symbol_the_ui_writes_is_in_the_default_fonts() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let symbols = [
            super::toolbar::BACK,
            super::toolbar::FORWARD,
            super::toolbar::UP,
            super::toolbar::BOOKMARKED,
            super::toolbar::NOT_BOOKMARKED,
            super::toolbar::SEPARATOR,
            super::sidebar::TOGGLE,
            super::sidebar::DISK,
            super::sidebar::TRASH,
            super::tabs::NEW,
            super::tabs::CLOSE,
            super::file_list::SORT_ASCENDING,
            super::file_list::SORT_DESCENDING,
            super::context_menu::CLOSED,
            super::context_menu::OPEN,
            crate::menu::SEARCH_SEPARATOR,
            // Punctuation in messages: “name”, Calculating…, Copying 3/10 — 45%, 1 · 2, 640 × 360.
            "“”…—·×",
        ];
        let font = FontId::proportional(14.0);
        for symbol in symbols {
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, symbol)), "{symbol:?} has no glyph");
        }
    }

    /// Catches a symbol added anywhere in UI code without updating the list
    /// above: every non-ASCII character outside tests and comments must draw.
    #[test]
    fn no_source_file_writes_a_character_the_fonts_lack() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let sources = [
            ("app.rs", include_str!("../app.rs")),
            ("config.rs", include_str!("../config.rs")),
            ("places.rs", include_str!("../places.rs")),
            ("fs/count.rs", include_str!("../fs/count.rs")),
            ("fs/ops.rs", include_str!("../fs/ops.rs")),
            ("fs/preview.rs", include_str!("../fs/preview.rs")),
            ("fs/transfer.rs", include_str!("../fs/transfer.rs")),
            ("fs/lister.rs", include_str!("../fs/lister.rs")),
            ("menu.rs", include_str!("../menu.rs")),
            ("ui/context_menu.rs", include_str!("context_menu.rs")),
            ("ui/confirm.rs", include_str!("confirm.rs")),
            ("ui/theme.rs", include_str!("theme.rs")),
            ("ui/file_list.rs", include_str!("file_list.rs")),
            ("ui/path_popup.rs", include_str!("path_popup.rs")),
            ("ui/preview.rs", include_str!("preview.rs")),
            ("ui/sidebar.rs", include_str!("sidebar.rs")),
            ("ui/status_bar.rs", include_str!("status_bar.rs")),
            ("ui/tabs.rs", include_str!("tabs.rs")),
            ("ui/toolbar.rs", include_str!("toolbar.rs")),
        ];
        let font = FontId::proportional(14.0);
        for (name, source) in sources {
            let code = source.split("#[cfg(test)]").next().unwrap_or(source);
            for line in code.lines() {
                let line = line.split("//").next().unwrap_or(line);
                for c in line.chars().filter(|c| !c.is_ascii()) {
                    assert!(ctx.fonts_mut(|f| f.has_glyph(&font, c)), "{name}: {c:?} in {line:?} has no glyph");
                }
            }
        }
    }
}
