//! Editable path bar. Shows the current folder; Ctrl+L focuses it with the text
//! selected, Enter navigates, Escape restores the current path.

use std::path::Path;

use eframe::egui::{self, text::CCursorRange};

#[derive(Default)]
pub struct PathBar {
    text: String,
    focus_requested: bool,
}

impl PathBar {
    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    /// Draws the bar. Returns the typed text when the user presses Enter.
    pub fn show(&mut self, ui: &mut egui::Ui, current: &Path) -> Option<String> {
        let id = egui::Id::new("rumbo_path_bar");
        let has_focus = ui.memory(|m| m.has_focus(id));
        if !has_focus && !self.focus_requested {
            // Not being edited: always mirror the folder actually shown.
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
            output
                .state
                .cursor
                .set_char_range(Some(CCursorRange::select_all(&output.galley)));
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
