pub mod confirm;
pub mod app_menu;
pub mod caption;
pub mod context_menu;
pub mod dnd;
pub mod drives_view;
pub mod file_icons;
pub mod file_list;
pub mod grid;
pub mod icon_grid;
pub mod icons;
pub mod list_view;
pub mod options;
pub mod path_popup;
pub mod preview;
pub mod recycle_view;
pub mod setup_view;
pub mod sidebar;
pub mod status_bar;
pub mod tab_search;
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

/// Shift+Tab while nothing has the focus (after a click on a row): starts
/// from the list, so it goes to the side panel, as Tab goes to the first tab.
/// Call each frame before any widget is drawn.
pub fn shift_tab_from_list(ctx: &egui::Context) {
    let back = ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if modifiers.shift_only()))
    });
    // egui's own Shift+Tab from nothing lands on the last widget it saw.
    if back && ctx.memory(|m| m.focused()).is_none() {
        ctx.memory_mut(|m| m.request_focus(file_list::focus_id()));
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
        frame(ctx, vec![Event::PointerMoved(pos)], &mut ui);
        frame(ctx, vec![button(pos, true, egui::Modifiers::NONE)], &mut ui);
        frame(ctx, vec![button(pos, false, egui::Modifiers::NONE)], &mut ui)
    }

    /// Like `frame`, with the shapes it drew too.
    pub fn run<R>(ctx: &egui::Context, events: Vec<Event>, mut draw: impl FnMut(&mut egui::Ui) -> R) -> (R, Vec<egui::epaint::ClippedShape>) {
        let mut out = None;
        let input = RawInput { events, screen_rect: Some(SCREEN), ..RawInput::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::CentralPanel::default().show(root, |ui| out = Some(draw(ui)));
        });
        output.textures_delta.clear();
        (out.expect("the frame ran"), output.shapes)
    }

    /// The primary button pressed or let go at `at` with `modifiers` down.
    fn button(at: Pos2, pressed: bool, modifiers: egui::Modifiers) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers }
    }

    /// Carries `paths` as a drag from the corner of the screen (a list's empty
    /// space) and holds it at `to` with `modifiers` down, long enough for the
    /// label by the pointer to show; the shapes of the last frame.
    pub fn lift<R>(ctx: &egui::Context, paths: &[std::path::PathBuf], to: Pos2, modifiers: egui::Modifiers, mut draw: impl FnMut(&mut egui::Ui) -> R) -> Vec<egui::epaint::ClippedShape> {
        let from = egui::pos2(790.0, 590.0);
        run(ctx, vec![Event::ModifiersChanged(modifiers), Event::PointerMoved(from), button(from, true, modifiers)], &mut draw);
        egui::DragAndDrop::set_payload(ctx, crate::drag::Drag { paths: paths.to_vec() });
        run(ctx, vec![Event::PointerMoved(to)], &mut draw);
        // The label is laid out the frame it first shows.
        run(ctx, vec![], &mut draw).1
    }

    /// Lets the button go at `at`; what `draw` returned that frame and the shapes.
    pub fn let_go<R>(ctx: &egui::Context, at: Pos2, modifiers: egui::Modifiers, draw: impl FnMut(&mut egui::Ui) -> R) -> (R, Vec<egui::epaint::ClippedShape>) {
        run(ctx, vec![Event::ModifiersChanged(modifiers), Event::PointerMoved(at), button(at, false, modifiers)], draw)
    }

    /// The rects filled the colour a folder that takes the drop is.
    pub fn lit(ctx: &egui::Context, shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
        let fill = crate::ui::dnd::lit_fill(&ctx.global_style().visuals);
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == fill => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    /// The text of the label by the pointer, if one is drawn.
    pub fn label(shapes: &[egui::epaint::ClippedShape]) -> Option<String> {
        shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text().contains(" item") => Some(t.galley.text().to_owned()),
            _ => None,
        })
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
            super::app_menu::MENU,
            super::sidebar::DISK,
            super::sidebar::COMPUTER,
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

    /// The Spanish catalog reaches what the views draw, by the same code paths English takes.
    #[test]
    fn the_views_speak_spanish_once_the_language_is_set() {
        use std::time::Duration;

        use super::status_bar::{PickedSize, count_text, filter_placeholder, picked_text, progress_text};
        use crate::fs::transfer::{Mode, Progress};
        use crate::i18n::{Lang, set};
        use crate::places::Usage;

        let ctx = egui::Context::default();
        let progress = Progress { files_done: 3, files_total: 10, bytes_done: 45, bytes_total: 100 };
        set(Lang::Es);
        let (_, shapes) = super::testing::run(&ctx, vec![], |ui| {
            super::drives_view::show(ui, &[], None, None, &super::drives_view::DEFAULT_WIDTHS);
        });
        let drawn: Vec<String> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        let said = [
            count_text(12, false),
            filter_placeholder(10),
            picked_text(5, &PickedSize { bytes: 0, uncounted: 2 }).unwrap_or_default(),
            progress_text(Mode::Copy, &progress, Some((2, 5))),
            super::tab_search::ago(Some(Duration::from_secs(300))),
            super::sidebar::usage_text(&Usage { free: 512, total: 2048 }),
            super::file_list::column_label("Date Modified").to_owned(),
            super::view_size::shown("XL Icons").to_owned(),
            super::caption::Button::Minimize.hint().0.to_owned(),
        ];
        set(Lang::En);
        assert!(drawn.iter().any(|t| t == "No hay unidades"), "{drawn:?}");
        assert_eq!(
            said,
            [
                "12 elementos",
                "Filtrar 10 archivos…",
                "5 seleccionados · 2 carpetas",
                "Copiando elemento 2 de 5: 3/10 — 45%",
                "hace 5 min",
                "512 B disponibles de 2.0 KB",
                "Fecha de modificación",
                "Iconos XL",
                "Minimizar ventana",
            ]
        );
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
            ("recycle_bin.rs", include_str!("../recycle_bin.rs")),
            ("fs/count.rs", include_str!("../fs/count.rs")),
            ("fs/ops.rs", include_str!("../fs/ops.rs")),
            ("fs/preview.rs", include_str!("../fs/preview.rs")),
            ("fs/transfer.rs", include_str!("../fs/transfer.rs")),
            ("fs/lister.rs", include_str!("../fs/lister.rs")),
            ("menu.rs", include_str!("../menu.rs")),
            ("ui/app_menu.rs", include_str!("app_menu.rs")),
            ("ui/context_menu.rs", include_str!("context_menu.rs")),
            ("ui/confirm.rs", include_str!("confirm.rs")),
            ("ui/drives_view.rs", include_str!("drives_view.rs")),
            ("ui/theme.rs", include_str!("theme.rs")),
            ("ui/file_list.rs", include_str!("file_list.rs")),
            ("ui/options.rs", include_str!("options.rs")),
            ("ui/path_popup.rs", include_str!("path_popup.rs")),
            ("ui/preview.rs", include_str!("preview.rs")),
            ("ui/recycle_view.rs", include_str!("recycle_view.rs")),
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
