//! Editable path field. Hidden behind the toolbar's crumbs until Ctrl+L (or a
//! click on the crumb bar) focuses it with the cursor after the path; Enter navigates,
//! Escape or a click elsewhere goes back to the crumbs.

use std::path::Path;

use eframe::egui::{self, text::{CCursor, CCursorRange}};

#[derive(Default)]
pub struct PathBar {
    text: String,
    focus_requested: bool,
}

impl PathBar {
    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    /// Whether the field is shown instead of the crumbs.
    pub fn is_editing(&self, ctx: &egui::Context) -> bool {
        self.focus_requested || ctx.memory(|m| m.has_focus(Self::id()))
    }

    fn id() -> egui::Id {
        egui::Id::new("lumen_path_bar")
    }

    /// Draws the bar. Returns the typed text when the user presses Enter.
    pub fn show(&mut self, ui: &mut egui::Ui, current: &Path) -> Option<String> {
        let id = Self::id();
        let has_focus = ui.memory(|m| m.has_focus(id));
        if !has_focus {
            // Starting an edit, or not editing: the folder actually shown.
            self.text = current.display().to_string();
        }

        let mut output = egui::TextEdit::singleline(&mut self.text)
            .id(id)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .show(ui);
        let response = &output.response.response;

        if std::mem::take(&mut self.focus_requested) {
            response.request_focus();
            // The path stays: typing goes after it, nothing is selected.
            let end = CCursor::new(self.text.chars().count());
            output.state.cursor.set_char_range(Some(CCursorRange::one(end)));
            output.state.store(ui.ctx(), id);
        }

        if response.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                return Some(self.text.clone());
            }
            // Escape or clicking elsewhere cancels the edit.
            self.text = current.display().to_string();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing::frame;
    use eframe::egui::{Event, Key, Modifiers};

    fn key(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    #[test]
    fn editing_keeps_the_path_and_types_at_its_end() {
        let ctx = egui::Context::default();
        let current = Path::new("/home/u/Descargas");
        let mut bar = PathBar::default();
        bar.request_focus();
        frame(&ctx, vec![], |ui| bar.show(ui, current));
        assert_eq!(frame(&ctx, vec![Event::Text("/fotos".into())], |ui| bar.show(ui, current)), None);
        let typed = frame(&ctx, vec![key(Key::Enter)], |ui| bar.show(ui, current));
        assert_eq!(typed.as_deref(), Some("/home/u/Descargas/fotos"));

        // Lumen went there; the next edit starts from it, not from the old text.
        let current = Path::new("/srv");
        bar.request_focus();
        frame(&ctx, vec![], |ui| bar.show(ui, current));
        let typed = frame(&ctx, vec![key(Key::Enter)], |ui| bar.show(ui, current));
        assert_eq!(typed.as_deref(), Some("/srv"));
    }

    #[test]
    fn escape_still_drops_the_edit() {
        let ctx = egui::Context::default();
        let current = Path::new("/home/u");
        let mut bar = PathBar::default();
        bar.request_focus();
        frame(&ctx, vec![], |ui| bar.show(ui, current));
        frame(&ctx, vec![Event::Text("x".into())], |ui| bar.show(ui, current));
        assert_eq!(frame(&ctx, vec![key(Key::Escape)], |ui| bar.show(ui, current)), None);
        assert!(!bar.is_editing(&ctx));
        assert_eq!(bar.text, "/home/u");
    }
}
