//! The tab bar under the system title bar: one tab per open folder, a × to
//! close each (when there is more than one) and a + for a new one.

use std::path::{Path, PathBuf};

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
    // Not a Tab stop: Ctrl+W closes the tab with the keyboard.
    let response = ui.interact(rect, close_id(index), egui::Sense::CLICK);
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
                    crate::ui::focus_ring(ui, &tab.response);
                    if close_clicked {
                        event = Some(TabEvent::Close(i));
                    } else if tab.response.on_hover_text(label).clicked() {
                        event = Some(TabEvent::Select(i));
                    }
                }
                let new = egui::Button::new(egui::RichText::new(NEW).size(16.0)).frame(false);
                let new = ui.add(new);
                crate::ui::focus_ring(ui, &new);
                if new.on_hover_text("New tab (Ctrl+T)").clicked() {
                    event = Some(TabEvent::New);
                }
            });
        });
    event
}

/// While entries are dragged, the tab whose folder takes the drop lights up.
/// Call it right after `show`, with each tab's folder in order.
pub fn drop_targets(ui: &egui::Ui, folders: &[PathBuf]) {
    for (i, folder) in folders.iter().enumerate() {
        let Some(tab) = ui.ctx().read_response(tab_id(i)) else { continue };
        if crate::ui::dnd::over_dir(ui.ctx(), ui.clip_rect(), tab.rect, folder) {
            ui.painter().add(crate::ui::dnd::lit_shape(ui.ctx(), tab.rect));
        }
    }
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

    use crate::drag::Keys;
    use crate::fs::ops::tests::fresh_fixture;
    use crate::ui::testing::{label, let_go, lift, lit, run};

    const NONE: egui::Modifiers = egui::Modifiers::NONE;
    const CTRL: egui::Modifiers = egui::Modifiers { ctrl: true, command: true, ..egui::Modifiers::NONE };

    /// Two tabs, a and b, in a fresh folder with a file `f.txt` beside them
    /// and another inside b.
    fn two_tabs(name: &str) -> (PathBuf, Vec<PathBuf>) {
        let dir = fresh_fixture(name);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("f.txt"), "f").unwrap();
        std::fs::write(dir.join("b/g.txt"), "g").unwrap();
        let folders = vec![dir.join("a"), dir.join("b")];
        (dir, folders)
    }

    /// The tab bar with its drop targets, as the app draws it.
    fn bar(folders: &[PathBuf]) -> impl FnMut(&mut egui::Ui) -> Option<TabEvent> + '_ {
        let labels: Vec<String> = folders.iter().map(|f| tab_label(f)).collect();
        move |ui| {
            let event = show(ui, &labels, 0);
            drop_targets(ui, folders);
            crate::ui::dnd::finish(ui.ctx());
            event
        }
    }

    /// What a drop took, as the app would read it.
    fn taken(ctx: &egui::Context) -> Option<(Vec<PathBuf>, PathBuf, Keys)> {
        crate::ui::dnd::take_drop(ctx).map(|d| (d.drag.paths, d.target, d.keys))
    }

    #[test]
    fn a_tab_under_dragged_entries_lights_up_names_the_action_and_takes_the_drop() {
        let (dir, folders) = two_tabs("dnd-tabs-drop");
        let ctx = egui::Context::default();
        run(&ctx, vec![], bar(&folders));
        let (a, b) = (tab_rect(&ctx, 0), tab_rect(&ctx, 1));
        let file = dir.join("f.txt");
        let shapes = lift(&ctx, std::slice::from_ref(&file), b.center(), NONE, bar(&folders));
        let lit_rects = lit(&ctx, &shapes);
        assert!(lit_rects.iter().any(|r| r.contains(b.center())), "the tab under the pointer lights up: {lit_rects:?}");
        assert!(!lit_rects.iter().any(|r| r.contains(a.center())), "and only it");
        assert_eq!(label(&shapes).as_deref(), Some("Move 1 item to b"), "same disk: a move");
        let (event, _) = let_go(&ctx, b.center(), NONE, bar(&folders));
        assert_eq!(event, None, "letting go on a tab does not select it");
        assert_eq!(taken(&ctx), Some((vec![file.clone()], dir.join("b"), Keys::default())));
        assert!(taken(&ctx).is_none(), "taken once");

        // Ctrl copies; the tab's × is part of the tab, not a close.
        let cross = ctx.read_response(close_id(0)).expect("the × was drawn").rect.center();
        let shapes = lift(&ctx, std::slice::from_ref(&file), cross, CTRL, bar(&folders));
        assert_eq!(label(&shapes).as_deref(), Some("Copy 1 item to a"));
        let (event, _) = let_go(&ctx, cross, CTRL, bar(&folders));
        assert_eq!(event, None, "the × does not close the tab on a drop");
        assert_eq!(taken(&ctx), Some((vec![file], dir.join("a"), Keys { ctrl: true, shift: false })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tab_that_would_refuse_the_drop_stays_dark_and_takes_nothing() {
        let (dir, mut folders) = two_tabs("dnd-tabs-refused");
        folders.push(dir.join("gone"));
        let ctx = egui::Context::default();
        run(&ctx, vec![], bar(&folders));
        let (a, b, gone) = (tab_rect(&ctx, 0), tab_rect(&ctx, 1), tab_rect(&ctx, 2));
        // The folder itself, and a folder that is no longer there.
        for (paths, at) in [(vec![dir.join("a")], a.center()), (vec![dir.join("f.txt")], gone.center())] {
            let shapes = lift(&ctx, &paths, at, NONE, bar(&folders));
            assert!(lit(&ctx, &shapes).is_empty(), "nothing lights up over {at:?}");
            assert_eq!(label(&shapes).as_deref(), Some("1 item"), "no folder named");
            let (event, _) = let_go(&ctx, at, NONE, bar(&folders));
            assert_eq!((event, taken(&ctx)), (None, None), "nothing dropped over {at:?}");
        }
        // A move into the folder the entry is in does nothing; Ctrl copies there.
        let inside = vec![dir.join("b/g.txt")];
        let shapes = lift(&ctx, &inside, b.center(), NONE, bar(&folders));
        assert!(lit(&ctx, &shapes).is_empty());
        let_go(&ctx, b.center(), NONE, bar(&folders));
        assert_eq!(taken(&ctx), None);
        let shapes = lift(&ctx, &inside, b.center(), CTRL, bar(&folders));
        assert_eq!(label(&shapes).as_deref(), Some("Copy 1 item to b"));
        let_go(&ctx, b.center(), CTRL, bar(&folders));
        assert_eq!(taken(&ctx), Some((inside, dir.join("b"), Keys { ctrl: true, shift: false })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_drag_a_tab_still_selects_on_a_click_and_closes_on_its_cross_and_never_lights() {
        let (dir, folders) = two_tabs("dnd-tabs-plain");
        let ctx = egui::Context::default();
        run(&ctx, vec![], bar(&folders));
        let name = tab_rect(&ctx, 1).left_center() + egui::vec2(12.0, 0.0);
        assert_eq!(click(&ctx, name, bar(&folders)), Some(TabEvent::Select(1)));
        let cross = ctx.read_response(close_id(1)).expect("the × was drawn").rect.center();
        assert_eq!(click(&ctx, cross, bar(&folders)), Some(TabEvent::Close(1)));
        let (_, shapes) = run(&ctx, vec![egui::Event::PointerMoved(name)], bar(&folders));
        assert!(lit(&ctx, &shapes).is_empty(), "hovering a tab lights nothing");
        assert_eq!(taken(&ctx), None, "and nothing was dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
