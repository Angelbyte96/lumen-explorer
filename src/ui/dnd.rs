//! Dragging entries out of the list: what a drag carries and the label by the
//! pointer. egui holds the payload; it drops it when the button is let go and
//! on Escape.

use eframe::egui::{self, DragAndDrop};

use crate::app::View;
use crate::drag::Drag;

/// What dragging from row `i` of `view` carries: the selection when the row
/// is picked, else the entry alone.
pub fn carry(view: &View, i: usize) -> Drag {
    let paths = if view.is_highlighted(i) {
        view.selection().into_iter().map(|entry| entry.path.clone()).collect()
    } else {
        vec![view.entry(view.rows[i]).path.clone()]
    };
    Drag { paths }
}

/// Sets the payload when a drag with the primary button starts on `response`.
pub fn start(response: &egui::Response, drag: impl FnOnce() -> Drag) {
    if response.drag_started_by(egui::PointerButton::Primary) {
        DragAndDrop::set_payload(&response.ctx, drag());
    }
}

/// Last thing of the frame: the label by the pointer while a drag is carried
/// (not on the frame it is let go).
pub fn finish(ctx: &egui::Context) {
    let Some(drag) = DragAndDrop::payload::<Drag>(ctx) else { return };
    let Some(at) = ctx.input(|i| i.pointer.latest_pos().filter(|_| !i.pointer.any_released())) else { return };
    let text = match drag.paths.len() {
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    egui::Area::new(egui::Id::new("lumen_drag_label"))
        .order(egui::Order::Tooltip)
        .fade_in(false)
        .interactable(false)
        .fixed_pos(at + egui::vec2(16.0, 16.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| ui.add(egui::Label::new(text).selectable(false)));
        });
}
