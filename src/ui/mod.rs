pub mod confirm;
pub mod file_list;
pub mod icons;
pub mod path_bar;
pub mod preview;
pub mod sidebar;
pub mod status_bar;
pub mod tabs;
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
            super::tabs::NEW,
            super::tabs::CLOSE,
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
            ("ui/confirm.rs", include_str!("confirm.rs")),
            ("ui/file_list.rs", include_str!("file_list.rs")),
            ("ui/path_bar.rs", include_str!("path_bar.rs")),
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
