//! Dragging entries out of the list: what a drag carries, the label by the
//! pointer, the folders that light up and take the drop, and the list
//! scrolling at its edges. egui holds the payload; it drops it when the button
//! is let go and on Escape.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, DragAndDrop, Rect};

use crate::app::View;
use crate::drag::{Drag, Keys};
use crate::fs::transfer::Mode;

/// A drop the list took this frame, for the app to run.
#[derive(Clone)]
pub struct Dropped {
    pub drag: Drag,
    pub target: PathBuf,
    pub keys: Keys,
}

/// The folder a drop would go to this frame and what it would do there, for
/// the label.
#[derive(Clone)]
struct Aim {
    target: PathBuf,
    mode: Mode,
}

fn aim_id() -> egui::Id {
    egui::Id::new("lumen_drag_aim")
}

fn dropped_id() -> egui::Id {
    egui::Id::new("lumen_dropped")
}

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

/// Whether a carried drag is over a row, card or cell, and whether that one
/// is a folder that takes the drop.
#[derive(Clone, Copy, Default)]
pub struct Over {
    pub inside: bool,
    pub lit: bool,
}

/// The row, card or cell at `rect` (visible inside `clip`) under a carried
/// drag. When `folder` is given and takes the drag it is `lit`, and it takes
/// the drop when the button is let go.
pub fn over(ctx: &egui::Context, clip: Rect, rect: Rect, folder: Option<&Path>) -> Over {
    let Some(drag) = DragAndDrop::payload::<Drag>(ctx) else { return Over::default() };
    if !ctx.input(|i| i.pointer.latest_pos()).is_some_and(|at| rect.contains(at) && clip.contains(at)) {
        return Over::default();
    }
    let mut over = Over { inside: true, lit: false };
    if let Some(folder) = folder {
        let keys = ctx.input(|i| Keys { ctrl: i.modifiers.ctrl, shift: i.modifiers.shift });
        if let Some(mode) = drag.mode_for(folder, keys) {
            over.lit = true;
            aim(ctx, &drag, folder, mode, keys);
        }
    }
    over
}

/// Remembers where the drag would drop for the label, and takes the drop when
/// the button is let go.
fn aim(ctx: &egui::Context, drag: &Arc<Drag>, target: &Path, mode: Mode, keys: Keys) {
    ctx.data_mut(|d| d.insert_temp(aim_id(), Aim { target: target.to_path_buf(), mode }));
    if ctx.input(|i| i.pointer.primary_released()) {
        let dropped = Dropped { drag: Drag::clone(drag), target: target.to_path_buf(), keys };
        ctx.data_mut(|d| d.insert_temp(dropped_id(), dropped));
    }
}

/// A folder that takes the drop: filled and ringed.
pub fn lit_shape(ctx: &egui::Context, rect: Rect) -> egui::Shape {
    let visuals = ctx.global_style().visuals.clone();
    egui::Shape::Rect(egui::epaint::RectShape::new(rect, 4.0, lit_fill(&visuals), visuals.selection.stroke, egui::StrokeKind::Inside))
}

/// The fill of a folder that takes the drop.
pub fn lit_fill(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.selection.bg_fill.gamma_multiply(0.55)
}

/// The empty space of the list at `rect`, when the pointer is over no row,
/// card or cell: a ring round it, and it drops into `current`, the folder on
/// screen.
pub fn background(ui: &egui::Ui, rect: Rect, current: &Path) {
    let rect = rect.intersect(ui.clip_rect());
    if over(ui.ctx(), ui.clip_rect(), rect, Some(current)).lit {
        ui.painter().rect_stroke(rect.shrink(1.0), 2, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
    }
}

/// While a drag is carried, held near the list's edge (`sideways`: left and
/// right, else top and bottom) scrolls `ui`'s scroll area. Only over the list
/// itself: the band goes on past its edge, a drag on its way to a panel does not.
pub fn edge_scroll(ui: &egui::Ui, sideways: bool) {
    if !DragAndDrop::has_payload_of_type::<Drag>(ui.ctx()) {
        return;
    }
    let (pointer, dt) = ui.input(|i| (i.pointer.latest_pos(), i.stable_dt));
    let clip = ui.clip_rect();
    let Some(pointer) = pointer.filter(|p| clip.contains(*p)) else { return };
    let (speed, along) = if sideways {
        (crate::ui::file_list::band_speed(pointer.x, clip.x_range()), egui::vec2(1.0, 0.0))
    } else {
        (crate::ui::file_list::band_speed(pointer.y, clip.y_range()), egui::vec2(0.0, 1.0))
    };
    if speed != 0.0 {
        ui.scroll_with_delta_animation(along * speed * dt, egui::style::ScrollAnimation::none());
        // Keep scrolling while the pointer is held still.
        ui.ctx().request_repaint();
    }
}

/// The drop released on a target this frame, if any.
pub fn take_drop(ctx: &egui::Context) -> Option<Dropped> {
    ctx.data_mut(|d| {
        let dropped = d.get_temp::<Dropped>(dropped_id());
        d.remove::<Dropped>(dropped_id());
        dropped
    })
}

/// Last thing of the frame, after every target: the label by the pointer
/// while a drag is carried (not on the frame it is let go), naming the action
/// and the folder when the pointer is over one that takes the drop.
pub fn finish(ctx: &egui::Context) {
    let aim = ctx.data_mut(|d| {
        let aim = d.get_temp::<Aim>(aim_id());
        d.remove::<Aim>(aim_id());
        aim
    });
    let Some(drag) = DragAndDrop::payload::<Drag>(ctx) else { return };
    let Some(at) = ctx.input(|i| i.pointer.latest_pos().filter(|_| !i.pointer.any_released())) else { return };
    let items = match drag.paths.len() {
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    let text = match aim {
        Some(Aim { target, mode }) => {
            let name = target.file_name().map_or_else(|| target.display().to_string(), |n| n.to_string_lossy().into_owned());
            let verb = match mode {
                Mode::Move => "Move",
                Mode::Copy => "Copy",
            };
            format!("{verb} {items} to {name}")
        }
        None => items,
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
