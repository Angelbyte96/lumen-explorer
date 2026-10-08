//! Editable path field. Hidden behind the toolbar's crumbs until Ctrl+L (or a
//! click on the crumb bar) focuses it with the text selected; Enter navigates,
//! Escape or a click elsewhere goes back to the crumbs.

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
