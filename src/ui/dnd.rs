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
    /// What it would do with no key held.
    preferred: Mode,
}

/// Files dragged over the window from another app (Wayland). While one is
/// over the window egui gets no pointer events, so where it is, which keys
/// act on it and whether it was let go come from the compositor; the app sets
/// them every frame and the targets read them in place of egui's pointer.
#[derive(Clone)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct Outside {
    pub drag: Drag,
    /// In egui points.
    pub at: egui::Pos2,
    /// The action agreed with the dragging app, as the keys that force it.
    pub keys: Keys,
    /// Let go over the window this frame.
    pub released: bool,
}

fn outside_id() -> egui::Id {
    egui::Id::new("lumen_drag_outside")
}

/// Sets (or, with `None`, clears) the drag from another app for this frame.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn set_outside(ctx: &egui::Context, outside: Option<Outside>) {
    ctx.data_mut(|d| match outside {
        Some(outside) => {
            d.insert_temp(outside_id(), outside);
        }
        None => d.remove::<Outside>(outside_id()),
    });
}

fn outside(ctx: &egui::Context) -> Option<Outside> {
    ctx.data(|d| d.get_temp::<Outside>(outside_id()))
}

/// The drag being carried: the one from another app, else egui's.
fn carried(ctx: &egui::Context) -> Option<Arc<Drag>> {
    match outside(ctx) {
        Some(outside) => Some(Arc::new(outside.drag)),
        None => DragAndDrop::payload::<Drag>(ctx),
    }
}

/// Where the carried drag is: egui's pointer, or the position the compositor gave.
fn pointer(ctx: &egui::Context) -> Option<egui::Pos2> {
    match outside(ctx) {
        Some(outside) => Some(outside.at),
        None => ctx.input(|i| i.pointer.latest_pos()),
    }
}

/// Whether the carried drag was let go this frame.
fn let_go(ctx: &egui::Context) -> bool {
    match outside(ctx) {
        Some(outside) => outside.released,
        None => ctx.input(|i| i.pointer.primary_released()),
    }
}

/// The keys that act on the carried drag: those held, or the action agreed
/// with the app it comes from.
fn keys(ctx: &egui::Context) -> Keys {
    match outside(ctx) {
        Some(outside) => outside.keys,
        None => ctx.input(|i| Keys { ctrl: i.modifiers.ctrl, shift: i.modifiers.shift }),
    }
}

/// Whether the pointer at `pos` (`None`: it is gone) is outside `screen`, the
/// window's own surface: the right and bottom edges are outside, as a pixel
/// is.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn outside_window(pos: Option<egui::Pos2>, screen: Rect) -> bool {
    let Some(pos) = pos else { return true };
    pos.x < screen.min.x || pos.y < screen.min.y || pos.x >= screen.max.x || pos.y >= screen.max.y
}

/// Whether the pointer is outside the window. During the implicit grab of a
/// held button the compositor keeps sending motion past the edge, which is
/// how a drag in progress is seen leaving.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn left_window(ctx: &egui::Context) -> bool {
    outside_window(ctx.input(|i| i.pointer.latest_pos()), ctx.content_rect())
}

/// The drag carried inside Lumen (egui's payload), not one from another app.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn carried_inside(ctx: &egui::Context) -> Option<Drag> {
    if outside(ctx).is_some() {
        return None;
    }
    DragAndDrop::payload::<Drag>(ctx).map(|drag| Drag::clone(&drag))
}

/// What the pointer was doing when the drag left to the compositor.
#[derive(Clone, Copy)]
struct HandedOff {
    at: egui::Pos2,
    modifiers: egui::Modifiers,
}

fn handed_off_id() -> egui::Id {
    egui::Id::new("lumen_drag_handed_off")
}

/// The compositor has the drag now (a Wayland drag out): the pointer is its
/// and egui will never see the button let go. Drops the payload, so nothing is
/// carried, and has the next frame see the release (`end_hand_off`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn hand_off(ctx: &egui::Context) {
    DragAndDrop::clear_payload(ctx);
    let (at, modifiers) = ctx.input(|i| (i.pointer.latest_pos().or(i.pointer.interact_pos()), i.modifiers));
    ctx.data_mut(|d| d.insert_temp(handed_off_id(), HandedOff { at: at.unwrap_or_default(), modifiers }));
    ctx.request_repaint();
}

/// First thing of a frame: if the drag was handed off, the release and the
/// pointer gone that egui missed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn end_hand_off(ctx: &egui::Context, raw_input: &mut egui::RawInput) {
    let Some(HandedOff { at, modifiers }) = ctx.data_mut(|d| {
        let handed = d.get_temp::<HandedOff>(handed_off_id());
        d.remove::<HandedOff>(handed_off_id());
        handed
    }) else {
        return;
    };
    raw_input.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers });
    raw_input.events.push(egui::Event::PointerGone);
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

fn trash_id() -> egui::Id {
    egui::Id::new("lumen_drag_trash")
}

/// The trash's folder: no folder in it takes a drop, which would skip its
/// bookkeeping (what it held, and where from). Set by the app every frame.
pub fn set_trash(ctx: &egui::Context, trash: Option<&Path>) {
    ctx.data_mut(|d| match trash {
        Some(trash) => {
            d.insert_temp(trash_id(), Arc::new(trash.to_path_buf()));
        }
        None => d.remove::<Arc<PathBuf>>(trash_id()),
    });
}

fn in_trash(ctx: &egui::Context, folder: &Path) -> bool {
    ctx.data(|d| d.get_temp::<Arc<PathBuf>>(trash_id())).is_some_and(|trash| crate::places::in_trash(folder, &trash))
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
    let Some(drag) = carried(ctx) else { return Over::default() };
    if !pointer(ctx).is_some_and(|at| rect.contains(at) && clip.contains(at)) {
        return Over::default();
    }
    let mut over = Over { inside: true, lit: false };
    if let Some(folder) = folder.filter(|folder| !in_trash(ctx, folder)) {
        let keys = keys(ctx);
        // Another app is told what to prefer with no key held (Ctrl and Shift
        // are its own to choose), so what it refuses there is refused for good.
        let preferred = drag.mode_for(folder, Keys::default());
        let refused = outside(ctx).is_some() && preferred.is_none();
        if let Some(mode) = drag.mode_for(folder, keys).filter(|_| !refused) {
            over.lit = true;
            aim(ctx, &drag, folder, mode, preferred.unwrap_or(mode), keys);
        }
    }
    over
}

/// The row at `rect` for the folder `path` in a panel, under a carried drag:
/// whether it is lit and takes the drop. A `path` that is no folder (a disk
/// not mounted, a folder gone) takes nothing; it is only looked at when the
/// pointer is over `rect`.
pub fn over_dir(ctx: &egui::Context, clip: Rect, rect: Rect, path: &Path) -> bool {
    over(ctx, clip, rect, None).inside && path.is_dir() && over(ctx, clip, rect, Some(path)).lit
}

/// Remembers where the drag would drop for the label, and takes the drop when
/// the button is let go.
fn aim(ctx: &egui::Context, drag: &Arc<Drag>, target: &Path, mode: Mode, preferred: Mode, keys: Keys) {
    ctx.data_mut(|d| d.insert_temp(aim_id(), Aim { target: target.to_path_buf(), mode, preferred }));
    if let_go(ctx) {
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
    if carried(ui.ctx()).is_none() {
        return;
    }
    let (pointer, dt) = (pointer(ui.ctx()), ui.input(|i| i.stable_dt));
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
/// and the folder when the pointer is over one that takes the drop. Gives the
/// action that folder takes with no key held, which a drag from another app is
/// told to prefer; `None` over nothing that takes the drop.
pub fn finish(ctx: &egui::Context) -> Option<Mode> {
    let aim = ctx.data_mut(|d| {
        let aim = d.get_temp::<Aim>(aim_id());
        d.remove::<Aim>(aim_id());
        aim
    });
    let drag = carried(ctx)?;
    let preferred = aim.as_ref().map(|aim| aim.preferred);
    let gone = match outside(ctx) {
        Some(outside) => outside.released,
        None => ctx.input(|i| i.pointer.any_released()),
    };
    let Some(at) = pointer(ctx).filter(|_| !gone) else { return preferred };
    let items = match drag.paths.len() {
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    let text = match aim {
        Some(Aim { target, mode, .. }) => {
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
        // From another app, right of the icon that app draws by the pointer.
        .fixed_pos(at + if outside(ctx).is_some() { egui::vec2(72.0, 8.0) } else { egui::vec2(16.0, 16.0) })
        .show(ctx, |ui| {
            // One line as wide as the text: the area keeps the width it first
            // had ("1 item"), which would wrap a folder's name.
            egui::Frame::popup(ui.style()).show(ui, |ui| ui.add(egui::Label::new(text).selectable(false).extend()));
        });
    preferred
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;
    use crate::ui::testing::{label, let_go, lift, lit, run};
    use eframe::egui::pos2;

    const NONE: Keys = Keys { ctrl: false, shift: false };
    const CTRL: Keys = Keys { ctrl: true, shift: false };
    const ROW: Rect = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 130.0));

    /// `dir` with `a.txt` and the folders `sub` and `other`.
    fn fixture(name: &str) -> PathBuf {
        let dir = fresh_fixture(name);
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::create_dir(dir.join("other")).unwrap();
        dir
    }

    fn outside(paths: &[PathBuf], at: egui::Pos2, keys: Keys, released: bool) -> Option<Outside> {
        Some(Outside { drag: Drag { paths: paths.to_vec() }, at, keys, released })
    }

    /// One folder row at `ROW`, as a panel draws it: whether it is lit, and
    /// what `finish` asks the compositor for.
    fn row(folder: &Path) -> impl FnMut(&mut egui::Ui) -> (bool, Option<Mode>) + '_ {
        move |ui| {
            let lit = over_dir(ui.ctx(), ui.clip_rect(), ROW, folder);
            (lit, finish(ui.ctx()))
        }
    }

    /// Two frames with the drag held (the label is laid out the second): what the last drew.
    fn held(ctx: &egui::Context, folder: &Path) -> ((bool, Option<Mode>), Vec<egui::epaint::ClippedShape>) {
        run(ctx, vec![], row(folder));
        run(ctx, vec![], row(folder))
    }

    #[test]
    fn a_drag_from_another_app_lights_the_row_under_it_and_names_the_action() {
        let dir = fixture("dnd-outside-lit");
        let sub = dir.join("sub");
        let ctx = egui::Context::default();
        let file = dir.join("a.txt");
        // egui's own pointer is somewhere else: it is stale during such a drag.
        run(&ctx, vec![egui::Event::PointerMoved(pos2(700.0, 500.0))], row(&sub));
        set_outside(&ctx, outside(std::slice::from_ref(&file), ROW.center(), NONE, false));
        let ((is_lit, preferred), shapes) = held(&ctx, &sub);
        assert!(is_lit, "the row under the outside position lights up");
        assert_eq!(preferred, Some(Mode::Move), "the same disk: a move is asked for");
        assert_eq!(label(&shapes).as_deref(), Some("Move 1 item to sub"));
        // The label sits by the outside position, not by egui's pointer, and
        // clear of the icon the dragging app draws there (up to 64 points).
        let near = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text().contains(" item") => Some(t.pos),
            _ => None,
        });
        let at = ROW.center();
        assert!(near.is_some_and(|p| p.x > at.x + 64.0 && p.x < at.x + 100.0 && p.y > at.y && p.y < at.y + 40.0), "by the drag, past its icon: {near:?}");

        // Moved off the row: dark, and the offer is refused.
        set_outside(&ctx, outside(std::slice::from_ref(&file), pos2(500.0, 400.0), NONE, false));
        let ((is_lit, preferred), _) = held(&ctx, &sub);
        assert!(!is_lit);
        assert_eq!(preferred, None, "over no target the offer is refused");
        // Gone: egui's pointer over the row is no drag.
        set_outside(&ctx, None);
        run(&ctx, vec![egui::Event::PointerMoved(ROW.center())], row(&sub));
        let ((is_lit, preferred), shapes) = held(&ctx, &sub);
        assert!(!is_lit && preferred.is_none() && label(&shapes).is_none() && lit(&ctx, &shapes).is_empty(), "no drag, no label");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_agreed_action_names_the_label_but_the_preferred_one_ignores_keys() {
        let dir = fixture("dnd-outside-keys");
        let sub = dir.join("sub");
        let ctx = egui::Context::default();
        set_outside(&ctx, outside(&[dir.join("a.txt")], ROW.center(), CTRL, false));
        let ((is_lit, preferred), shapes) = held(&ctx, &sub);
        assert!(is_lit);
        assert_eq!(label(&shapes).as_deref(), Some("Copy 1 item to sub"), "Ctrl in the other app: a copy");
        assert_eq!(preferred, Some(Mode::Move), "the compositor is asked to prefer the disks' action, or Ctrl would stick");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn another_disk_prefers_a_copy() {
        // /dev/shm is a tmpfs, never the disk the home folder is on.
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let dir = fixture("dnd-outside-other-disk");
        let ctx = egui::Context::default();
        set_outside(&ctx, outside(&[dir.join("a.txt")], ROW.center(), NONE, false));
        let ((is_lit, preferred), _) = held(&ctx, shm);
        assert!(is_lit);
        assert_eq!(preferred, Some(Mode::Copy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_row_that_refuses_the_drag_is_dark_and_asks_for_nothing() {
        let dir = fixture("dnd-outside-refused");
        let ctx = egui::Context::default();
        // A folder over itself, and a move into the folder the file is in.
        for (paths, target) in [(vec![dir.join("sub")], dir.join("sub")), (vec![dir.join("a.txt")], dir.clone())] {
            set_outside(&ctx, outside(&paths, ROW.center(), NONE, false));
            let ((is_lit, preferred), shapes) = held(&ctx, &target);
            assert!(!is_lit, "{target:?} stays dark");
            assert_eq!(preferred, None);
            assert_eq!(label(&shapes).as_deref(), Some("1 item"), "no folder named");
        }
        // Ctrl in the other app would copy into the folder the file is in, but
        // the offer was refused: nothing is agreed, so nothing is lit.
        set_outside(&ctx, outside(&[dir.join("a.txt")], ROW.center(), CTRL, false));
        let ((is_lit, preferred), _) = held(&ctx, &dir);
        assert!(!is_lit && preferred.is_none());
        // The trash takes nothing either.
        set_trash(&ctx, Some(&dir.join("other")));
        set_outside(&ctx, outside(&[dir.join("a.txt")], ROW.center(), NONE, false));
        let ((is_lit, preferred), _) = held(&ctx, &dir.join("other"));
        assert!(!is_lit && preferred.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn letting_go_over_a_row_takes_the_drop_there_with_the_agreed_keys() {
        let dir = fixture("dnd-outside-drop");
        let sub = dir.join("sub");
        let ctx = egui::Context::default();
        let file = dir.join("a.txt");
        set_outside(&ctx, outside(std::slice::from_ref(&file), ROW.center(), CTRL, false));
        held(&ctx, &sub);
        assert!(take_drop(&ctx).is_none(), "held is not dropped");
        set_outside(&ctx, outside(std::slice::from_ref(&file), ROW.center(), CTRL, true));
        let (_, shapes) = run(&ctx, vec![], row(&sub));
        let dropped = take_drop(&ctx).expect("let go over the row");
        assert_eq!((dropped.drag.paths, dropped.target, dropped.keys), (vec![file.clone()], sub.clone(), CTRL));
        assert!(label(&shapes).is_none(), "no label on the frame it is let go");
        assert!(take_drop(&ctx).is_none(), "taken once");
        // Let go over nothing: no drop.
        set_outside(&ctx, outside(&[file], pos2(500.0, 400.0), NONE, true));
        run(&ctx, vec![], row(&sub));
        assert!(take_drop(&ctx).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const WINDOW: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));

    #[test]
    fn the_pointer_is_out_of_the_window_past_any_edge_or_when_gone() {
        let at = |x, y| outside_window(Some(pos2(x, y)), WINDOW);
        assert!(!at(0.0, 0.0) && !at(400.0, 300.0) && !at(799.5, 599.5), "inside, corners included");
        assert!(at(-0.5, 300.0), "left");
        assert!(at(400.0, -1.0), "above");
        assert!(at(800.0, 300.0), "the right edge is the first pixel outside");
        assert!(at(400.0, 600.0), "the bottom edge too");
        assert!(at(900.0, 700.0), "far");
        assert!(outside_window(None, WINDOW), "gone");
    }

    #[test]
    fn the_window_is_left_when_egui_gets_motion_past_its_edge() {
        let ctx = egui::Context::default();
        run(&ctx, vec![egui::Event::PointerMoved(pos2(400.0, 300.0))], |_| ());
        assert!(!left_window(&ctx));
        run(&ctx, vec![egui::Event::PointerMoved(pos2(850.0, 300.0))], |_| ());
        assert!(left_window(&ctx), "motion goes on past the edge during the implicit grab");
        run(&ctx, vec![egui::Event::PointerGone], |_| ());
        assert!(left_window(&ctx));
    }

    #[test]
    fn only_a_drag_carried_inside_lumen_is_carried_inside() {
        let dir = fixture("dnd-carried-inside");
        let ctx = egui::Context::default();
        let file = dir.join("a.txt");
        assert!(carried_inside(&ctx).is_none());
        egui::DragAndDrop::set_payload(&ctx, Drag { paths: vec![file.clone()] });
        assert_eq!(carried_inside(&ctx).map(|drag| drag.paths), Some(vec![file.clone()]));
        // Files from another app are the compositor's, not Lumen's to hand back.
        set_outside(&ctx, outside(&[file], ROW.center(), NONE, false));
        assert!(carried_inside(&ctx).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A drag that began on a row at `ROW` and went out past the right edge, held.
    fn dragged_out(ctx: &egui::Context, paths: &[PathBuf]) {
        let frame = |events: Vec<egui::Event>| {
            run(ctx, events, |ui| {
                let response = ui.allocate_rect(ROW, egui::Sense::click_and_drag());
                start(&response, || Drag { paths: paths.to_vec() });
            });
        };
        let press = |pressed| egui::Event::PointerButton { pos: ROW.center(), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(vec![egui::Event::PointerMoved(ROW.center())]);
        frame(vec![press(true)]);
        frame(vec![egui::Event::PointerMoved(pos2(500.0, 300.0))]);
        frame(vec![egui::Event::PointerMoved(pos2(850.0, 300.0))]);
    }

    #[test]
    fn a_drag_handed_off_leaves_nothing_carried_and_no_button_held_the_next_frame() {
        let dir = fixture("dnd-hand-off");
        let ctx = egui::Context::default();
        let file = dir.join("a.txt");
        dragged_out(&ctx, std::slice::from_ref(&file));
        assert!(carried_inside(&ctx).is_some() && left_window(&ctx), "out of the window, still carried");
        assert!(ctx.input(|i| i.pointer.primary_down()) && ctx.dragged_id().is_some());

        hand_off(&ctx);
        assert!(carried_inside(&ctx).is_none() && !egui::DragAndDrop::has_any_payload(&ctx), "no payload is carried");
        // The next frame: the release that the compositor kept from egui.
        let mut raw = egui::RawInput::default();
        end_hand_off(&ctx, &mut raw);
        let (_, shapes) = run(&ctx, raw.events, row(&dir.join("sub")));
        assert!(!ctx.input(|i| i.pointer.any_down()), "egui does not think a button is held");
        assert!(ctx.dragged_id().is_none(), "no widget is still being dragged");
        assert!(label(&shapes).is_none() && lit(&ctx, &shapes).is_empty(), "no label, no lit row");
        // Once: the frame after that gets no event.
        let mut raw = egui::RawInput::default();
        end_hand_off(&ctx, &mut raw);
        assert!(raw.events.is_empty(), "nothing pending");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_hand_off_a_frame_adds_nothing() {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        end_hand_off(&ctx, &mut raw);
        assert!(raw.events.is_empty());
    }

    #[test]
    fn an_internal_drag_still_reads_egui_and_ignores_a_cleared_outside() {
        let dir = fixture("dnd-outside-internal");
        let sub = dir.join("sub");
        let ctx = egui::Context::default();
        set_outside(&ctx, None);
        lift(&ctx, &[dir.join("a.txt")], ROW.center(), egui::Modifiers::NONE, row(&sub));
        let ((is_lit, preferred), _) = run(&ctx, vec![], row(&sub));
        assert!(is_lit);
        assert_eq!(preferred, Some(Mode::Move));
        let_go(&ctx, ROW.center(), egui::Modifiers::NONE, row(&sub));
        assert_eq!(take_drop(&ctx).map(|d| d.target), Some(sub));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
