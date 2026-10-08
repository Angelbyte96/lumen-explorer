pub mod confirm;
pub mod file_list;
pub mod icons;
pub mod path_bar;
pub mod preview;
pub mod sidebar;
pub mod status_bar;
pub mod toolbar;

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
            super::file_list::SORT_ASCENDING,
            super::file_list::SORT_DESCENDING,
            // Punctuation in messages: “name”, Calculating…, Copying 3/10 — 45%, 1 · 2, 640 × 360.
            "“”…—·×",
        ];
        let font = FontId::proportional(14.0);
        for symbol in symbols {
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, symbol)), "{symbol:?} has no glyph");
        }
    }
}
