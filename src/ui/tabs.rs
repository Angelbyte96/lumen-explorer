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

/// Each tab's own id; its whole area is clickable.
pub fn tab_id(index: usize) -> egui::Id {
    egui::Id::new(("lumen_tab", index))
}

/// The id of a tab's ×.
pub fn close_id(index: usize) -> egui::Id {
    egui::Id::new(("lumen_tab_close", index))
}

/// The ×: a frameless button with a known id, drawn brighter on hover.
fn close_button(ui: &mut egui::Ui, index: usize) -> egui::Response {
    let galley = egui::WidgetText::from(CLOSE).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button);
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let response = ui.interact(rect, close_id(index), egui::Sense::click());
    let color = if response.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    ui.painter().galley(rect.min, galley, color);
    response
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
                    let mut close_clicked = false;
                    // The whole tab senses clicks, below its × so the × keeps its own.
                    let area = egui::UiBuilder::new().id(tab_id(i)).sense(egui::Sense::click());
                    let tab = ui.scope_builder(area, |ui| {
                        frame.show(ui, |ui| {
                            ui.set_min_width(110.0);
                            ui.set_max_width(180.0);
                            let text = egui::RichText::new(label);
                            let text = if selected { text.strong() } else { text };
                            ui.add(egui::Label::new(text).truncate().selectable(false).sense(egui::Sense::hover()));
                            if labels.len() > 1 {
                                close_clicked = close_button(ui, i).on_hover_text("Close tab (Ctrl+W)").clicked();
                            }
                        });
                    });
                    if close_clicked {
                        event = Some(TabEvent::Close(i));
                    } else if tab.response.on_hover_text(label).clicked() {
                        event = Some(TabEvent::Select(i));
                    }
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
    use crate::ui::testing::{click, frame};

    #[test]
    fn tabs_are_named_after_their_folder() {
        assert_eq!(tab_label(Path::new("/home/u/Descargas")), "Descargas");
        assert_eq!(tab_label(Path::new("/")), "/");
    }

    fn tab_rect(ctx: &egui::Context, index: usize) -> egui::Rect {
        ctx.read_response(tab_id(index)).expect("the tab was drawn").rect
    }

    #[test]
    fn a_click_anywhere_on_a_tab_selects_it() {
        let ctx = egui::Context::default();
        let labels = ["a".to_string(), "b".to_string()];
        frame(&ctx, vec![], |ui| show(ui, &labels, 0));
        let rect = tab_rect(&ctx, 1);
        // The empty space after the short name, and the margin before it.
        for pos in [rect.center() - egui::vec2(10.0, 0.0), rect.left_center() + egui::vec2(2.0, 0.0)] {
            assert_eq!(click(&ctx, pos, |ui| show(ui, &labels, 0)), Some(TabEvent::Select(1)), "at {pos:?}");
        }
        // On the name itself too.
        let name = rect.left_center() + egui::vec2(12.0, 0.0);
        assert_eq!(click(&ctx, name, |ui| show(ui, &labels, 0)), Some(TabEvent::Select(1)));
    }

    #[test]
    fn the_cross_closes_its_tab_without_selecting_it() {
        let ctx = egui::Context::default();
        let labels = ["a".to_string(), "b".to_string()];
        frame(&ctx, vec![], |ui| show(ui, &labels, 0));
        let cross = ctx.read_response(close_id(1)).expect("the × was drawn").rect.center();
        assert_eq!(click(&ctx, cross, |ui| show(ui, &labels, 0)), Some(TabEvent::Close(1)));
    }
}
