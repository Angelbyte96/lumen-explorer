//! The tab bar under the system title bar: one tab per open folder, a × to
//! close each (when there is more than one) and a + for a new one.

use std::path::Path;

use eframe::egui;

pub const NEW: &str = "+";
pub const CLOSE: &str = "×";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabEvent {
    Select(usize),
    Close(usize),
    New,
}

/// A tab's title: the folder's name, or the whole path for the root.
pub fn tab_label(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn show(ui: &mut egui::Ui, labels: &[String], active: usize) -> Option<TabEvent> {
    let mut event = None;
    egui::ScrollArea::horizontal()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, label) in labels.iter().enumerate() {
                    let selected = i == active;
                    let frame = egui::Frame::new()
                        .fill(if selected { ui.visuals().selection.bg_fill } else { ui.visuals().faint_bg_color })
                        .corner_radius(egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 })
                        .inner_margin(egui::Margin::symmetric(8, 3));
                    frame.show(ui, |ui| {
                        ui.set_min_width(110.0);
                        ui.set_max_width(180.0);
                        let text = egui::RichText::new(label);
                        let text = if selected { text.strong() } else { text };
                        let name = egui::Button::new(text).frame(false).truncate();
                        if ui.add(name).on_hover_text(label).clicked() {
                            event = Some(TabEvent::Select(i));
                        }
                        if labels.len() > 1 {
                            let close = egui::Button::new(CLOSE).frame(false);
                            if ui.add(close).on_hover_text("Close tab (Ctrl+W)").clicked() {
                                event = Some(TabEvent::Close(i));
                            }
                        }
                    });
                }
                let new = egui::Button::new(egui::RichText::new(NEW).size(16.0)).frame(false);
                if ui.add(new).on_hover_text("New tab (Ctrl+T)").clicked() {
                    event = Some(TabEvent::New);
                }
            });
        });
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_named_after_their_folder() {
        assert_eq!(tab_label(Path::new("/home/u/Descargas")), "Descargas");
        assert_eq!(tab_label(Path::new("/")), "/");
    }
}
