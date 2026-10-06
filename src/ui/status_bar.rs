use std::time::Duration;

use eframe::egui;

pub struct Status<'a> {
    pub item_count: usize,
    pub hidden_count: usize,
    pub show_hidden: bool,
    pub loading: bool,
    pub last_elapsed: Option<Duration>,
    pub error: Option<&'a str>,
}

pub fn show(ui: &mut egui::Ui, status: &Status<'_>) {
    ui.horizontal(|ui| {
        ui.label(format!("{} items", status.item_count));
        if status.hidden_count > 0 && !status.show_hidden {
            ui.weak(format!("({} hidden, Ctrl+H to show)", status.hidden_count));
        }
        if let Some(elapsed) = status.last_elapsed {
            ui.separator();
            ui.label(format!("listed in {:.2} ms", elapsed.as_secs_f64() * 1000.0));
        }
        if status.loading {
            ui.separator();
            ui.spinner();
            ui.label("Loading…");
        }
        if let Some(error) = status.error {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    });
}
