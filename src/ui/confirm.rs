//! Modal yes/no dialog for destructive operations. Enter and Escape are read
//! by the app (see `input::read_confirm`); this only draws and handles clicks.

use eframe::egui;

use crate::input::Confirm;

pub fn show(ctx: &egui::Context, message: &str, yes_label: &str) -> Option<Confirm> {
    let mut answer = None;
    let response = egui::Modal::new(egui::Id::new("lumen_confirm")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.label(message);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(yes_label).clicked() {
                answer = Some(Confirm::Yes);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(Confirm::No);
            }
            ui.weak("Enter to confirm · Esc to cancel");
        });
    });
    // Clicking outside the dialog cancels, like Escape.
    if answer.is_none() && response.should_close() {
        answer = Some(Confirm::No);
    }
    answer
}
