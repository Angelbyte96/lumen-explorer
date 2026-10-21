//! The bar above the list: back, forward and up buttons, the saved view
//! lock, the bookmark star, and the current path as clickable crumbs
//! ("angelbyte › Descargas").
//! Ctrl+L or a click on the empty part turns the crumbs into a text field.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::folder_views::Scope;


pub const BACK: &str = "⏴";
pub const FORWARD: &str = "⏵";
pub const UP: &str = "⏶";
pub const BOOKMARKED: &str = "★";
pub const NOT_BOOKMARKED: &str = "☆";
pub const SEPARATOR: &str = "›";

/// What the buttons can do right now.
pub struct Nav<'a> {
    pub current: &'a Path,
    pub home: Option<&'a Path>,
    pub can_back: bool,
    pub can_forward: bool,
    pub bookmarked: bool,
    pub saved_view: SavedView,
}

/// The folder's saved view settings, as the lock's menu shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedView {
    /// The folder's own rule.
    pub own: Option<Scope>,
    /// The folder above whose rule covers this one, when it has none.
    pub covered_by: Option<PathBuf>,
    /// The view and order shown are already the default.
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolbarEvent {
    /// The ☰ button: show or hide the side panel.
    ToggleSidebar,
    Back,
    Forward,
    Up,
    ToggleBookmark,
    /// A crumb was clicked.
    Go(PathBuf),
    /// Ctrl+L's box: a click on the empty part of the crumb bar.
    EditPath,
    /// The lock's menu: save the view and order shown for this folder (with
    /// its subfolders or not), or forget its rule with `None`.
    SaveView(Option<Scope>),
    /// The lock's menu: the view and order shown become the default.
    SetDefaultView,
}

/// One clickable piece of the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub path: PathBuf,
}

/// The crumbs for `path`. Inside the home folder they start at it, named after
/// it, like File Pilot; elsewhere they start at the root ("/").
pub fn crumbs(path: &Path, home: Option<&Path>) -> Vec<Crumb> {
    if crate::places::is_this_pc(path) {
        return vec![Crumb { label: crate::places::THIS_PC.into(), path: path.to_path_buf() }];
    }
    let named = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let (base, label) = match home {
        Some(home) if path.starts_with(home) && named(home).is_some() => {
            (home.to_path_buf(), named(home).unwrap_or_default())
        }
        _ => {
            // "/" on Unix, "C:\" on Windows.
            let root = path.ancestors().last().unwrap_or(path).to_path_buf();
            let label = root.display().to_string();
            let trimmed = label.trim_end_matches(['/', '\\']);
            let label = if trimmed.is_empty() { label.clone() } else { trimmed.to_owned() };
            (root, label)
        }
    };
    let mut crumbs = vec![Crumb { label, path: base.clone() }];
    let mut at = base.clone();
    for part in path.strip_prefix(&base).into_iter().flat_map(Path::components) {
        at.push(part);
        crumbs.push(Crumb { label: part.as_os_str().to_string_lossy().into_owned(), path: at.clone() });
    }
    crumbs
}

/// Whether the Up button has a folder to go to from `path`.
fn can_go_up(path: &Path) -> bool {
    path.parent().is_some() && !crate::places::is_this_pc(path)
}

pub fn show(ui: &mut egui::Ui, nav: &Nav<'_>) -> Option<ToolbarEvent> {
    let mut event = None;
    ui.horizontal(|ui| {
        let toggle = egui::Button::new(egui::RichText::new(crate::ui::sidebar::TOGGLE).size(16.0)).frame(false);
        let toggle = ui.add(toggle);
        crate::ui::focus_ring(ui, &toggle);
        if toggle.on_hover_text("Show or hide the side panel").clicked() {
            event = Some(ToolbarEvent::ToggleSidebar);
        }
        ui.separator();
        let buttons = [
            (BACK, nav.can_back, "Back (Alt+Left)", ToolbarEvent::Back),
            (FORWARD, nav.can_forward, "Forward (Alt+Right)", ToolbarEvent::Forward),
            (UP, can_go_up(nav.current), "Up to the parent folder (Alt+Up)", ToolbarEvent::Up),
        ];
        for (glyph, enabled, hint, on_click) in buttons {
            let button = egui::Button::new(egui::RichText::new(glyph).size(16.0)).frame(false);
            let button = ui.add_enabled(enabled, button);
            crate::ui::focus_ring(ui, &button);
            if button.on_hover_text(hint).clicked() {
                event = Some(on_click);
            }
        }
        if let Some(chosen) = lock(ui, &nav.saved_view) {
            event = Some(chosen);
        }
        ui.separator();
        let (star, hint) = if nav.bookmarked {
            (BOOKMARKED, "Remove from bookmarks (Ctrl+D)")
        } else {
            (NOT_BOOKMARKED, "Add to bookmarks (Ctrl+D)")
        };
        let star = egui::Button::new(egui::RichText::new(star).size(16.0)).frame(false);
        let star = ui.add(star);
        crate::ui::focus_ring(ui, &star);
        if star.on_hover_text(hint).clicked() {
            event = Some(ToolbarEvent::ToggleBookmark);
        }
        ui.separator();
        if let Some(go) = crumb_bar(ui, nav) {
            event = Some(go);
        }
    });
    event
}

fn lock_id() -> egui::Id {
    egui::Id::new("lumen_saved_view_lock")
}

/// The lock's menu, as `Popup::menu` names it.
fn lock_menu_id() -> egui::Id {
    lock_id().with("popup")
}

/// Whether the lock's menu is open.
pub fn lock_menu_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_id_open(ctx, lock_menu_id())
}

/// Closes the lock's menu on Escape and uses that Escape up; false when the
/// menu was closed. Runs before the app reads its keys, which may drop an
/// Escape that moved the focus.
pub fn close_lock_menu_on_escape(ctx: &egui::Context) -> bool {
    if !lock_menu_open(ctx) || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return false;
    }
    egui::Popup::close_id(ctx, lock_menu_id());
    true
}

/// The lock: bright while a saved rule covers the folder; a click opens
/// "Saved view settings".
fn lock(ui: &mut egui::Ui, saved: &SavedView) -> Option<ToolbarEvent> {
    let side = ui.spacing().interact_size.y;
    let (_, rect) = ui.allocate_space(egui::vec2(side, side));
    // A fixed id: the app closes the menu on Escape by it.
    let button = ui.interact(rect, lock_id(), egui::Sense::click());
    let saved_here = saved.own.is_some() || saved.covered_by.is_some();
    let color = if saved_here || button.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
    crate::ui::icons::paint_lock(ui.painter(), rect.shrink(side * 0.15), color);
    crate::ui::focus_ring(ui, &button);
    let button = button.on_hover_text("Saved view settings");
    let mut chosen = None;
    // Tab took the focus out of the lock and its menu: nothing else closes it.
    let ctx = ui.ctx().clone();
    let menu_layer = egui::LayerId::new(egui::Order::Foreground, lock_menu_id());
    let outside = ctx
        .memory(|m| m.focused())
        .filter(|&id| id != lock_id())
        .is_some_and(|id| ctx.read_response(id).is_none_or(|focused| focused.layer_id != menu_layer));
    if outside && lock_menu_open(&ctx) {
        egui::Popup::close_id(&ctx, lock_menu_id());
    }
    egui::Popup::menu(&button).show(|ui| {
        ui.label(egui::RichText::new("Saved view settings").strong());
        if let Some(above) = &saved.covered_by {
            let name = above.file_name().map_or_else(|| above.display().to_string(), |n| n.to_string_lossy().into_owned());
            ui.weak(format!("Saved with {name} and its subfolders"));
        }
        let choices = [(None, "None"), (Some(Scope::Folder), "This folder"), (Some(Scope::Tree), "This folder and subfolders")];
        for (scope, label) in choices {
            if ui.radio(saved.own == scope, label).clicked() {
                chosen = Some(ToolbarEvent::SaveView(scope));
            }
        }
        ui.separator();
        let set = ui.add_enabled(!saved.is_default, egui::Button::new("Set as default"));
        if set.on_disabled_hover_text("The view and order shown are the default").clicked() {
            chosen = Some(ToolbarEvent::SetDefaultView);
        }
    });
    chosen
}

/// The crumbs, kept scrolled to the end so the current folder is always visible.
fn crumb_bar(ui: &mut egui::Ui, nav: &Nav<'_>) -> Option<ToolbarEvent> {
    let mut event = None;
    let height = ui.spacing().interact_size.y;
    let crumbs = crumbs(nav.current, nav.home);
    egui::ScrollArea::horizontal()
        .stick_to_right(true)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let last = crumbs.len().saturating_sub(1);
                for (i, crumb) in crumbs.iter().enumerate() {
                    if i > 0 {
                        ui.weak(SEPARATOR);
                    }
                    let text = egui::RichText::new(&crumb.label);
                    let text = if i == last { text.strong() } else { text };
                    let hint = crumb.path.display().to_string();
                    // Not Tab stops: the bar is one, and Enter edits the path.
                    let button = egui::Button::new(text).frame(false).sense(egui::Sense::CLICK);
                    if ui.add(button).on_hover_text(hint).clicked() {
                        event = Some(ToolbarEvent::Go(crumb.path.clone()));
                    }
                }
                // The empty rest of the bar edits the path, like Ctrl+L.
                let rest = egui::vec2(ui.available_width().max(40.0), height);
                let response = ui.allocate_response(rest, egui::Sense::click());
                crate::ui::focus_ring(ui, &response);
                if response.on_hover_text("Edit the path (Ctrl+L)").clicked() {
                    event = Some(ToolbarEvent::EditPath);
                }
            });
        });
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(path: &str, home: Option<&str>) -> Vec<(String, PathBuf)> {
        crumbs(Path::new(path), home.map(Path::new))
            .into_iter()
            .map(|c| (c.label, c.path))
            .collect()
    }

    fn c(label: &str, path: &str) -> (String, PathBuf) {
        (label.into(), PathBuf::from(path))
    }

    #[test]
    fn this_pc_is_one_crumb_with_no_parent_to_go_up_to() {
        let pc = crate::places::this_pc();
        assert_eq!(labels("This PC", Some("/home/u")), [c("This PC", "This PC")]);
        assert_eq!(crumbs(&pc, None).len(), 1);
        assert!(!can_go_up(&pc));
        assert!(can_go_up(Path::new("/home/u")));
        assert!(!can_go_up(Path::new("/")), "the root has none");
    }

    #[test]
    fn a_click_on_the_empty_crumb_bar_opens_the_path_box() {
        use crate::ui::testing::{click, frame};
        let ctx = egui::Context::default();
        let nav = Nav {
            current: Path::new("/home/u/Descargas"),
            home: Some(Path::new("/home/u")),
            can_back: false,
            can_forward: false,
            bookmarked: false,
            saved_view: SavedView::default(),
        };
        frame(&ctx, vec![], |ui| show(ui, &nav));
        assert_eq!(click(&ctx, egui::pos2(700.0, 18.0), |ui| show(ui, &nav)), Some(ToolbarEvent::EditPath));
    }

    #[test]
    fn inside_home_the_crumbs_start_at_it() {
        assert_eq!(
            labels("/home/u/Descargas/fotos", Some("/home/u")),
            [c("u", "/home/u"), c("Descargas", "/home/u/Descargas"), c("fotos", "/home/u/Descargas/fotos")]
        );
        assert_eq!(labels("/home/u", Some("/home/u")), [c("u", "/home/u")]);
    }

    #[test]
    fn outside_home_the_crumbs_start_at_the_root() {
        assert_eq!(labels("/usr/bin", Some("/home/u")), [c("/", "/"), c("usr", "/usr"), c("bin", "/usr/bin")]);
        assert_eq!(labels("/home", Some("/home/u")), [c("/", "/"), c("home", "/home")]);
        assert_eq!(labels("/home/user2", Some("/home/u")), [c("/", "/"), c("home", "/home"), c("user2", "/home/user2")]);
        assert_eq!(labels("/", Some("/home/u")), [c("/", "/")]);
        assert_eq!(labels("/srv", None), [c("/", "/"), c("srv", "/srv")]);
    }

    /// Events and shapes of one frame of the tab bar and the toolbar, drawn
    /// in the window's order.
    #[derive(Debug, PartialEq)]
    enum Pressed {
        Tab(crate::ui::tabs::TabEvent),
        Bar(ToolbarEvent),
    }

    fn window(ctx: &egui::Context, nav: &Nav<'_>, events: Vec<egui::Event>) -> (Option<Pressed>, Vec<egui::epaint::ClippedShape>) {
        let labels = ["a".to_string(), "b".to_string()];
        let mut pressed = None;
        let input = egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("tabs").show(root, |ui| {
                if let Some(e) = crate::ui::tabs::show(ui, &labels, 0) {
                    pressed = Some(Pressed::Tab(e));
                }
            });
            egui::Panel::top("toolbar").show(root, |ui| {
                if let Some(e) = show(ui, nav) {
                    pressed = Some(Pressed::Bar(e));
                }
            });
        });
        output.textures_delta.clear();
        (pressed, output.shapes)
    }

    fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    /// Presses and releases `k`; returns what was pressed and the shapes drawn meanwhile.
    fn tap(ctx: &egui::Context, nav: &Nav<'_>, k: egui::Key, modifiers: egui::Modifiers) -> (Option<Pressed>, Vec<egui::epaint::ClippedShape>) {
        let down = window(ctx, nav, vec![key(k, true, modifiers)]);
        let up = window(ctx, nav, vec![key(k, false, modifiers)]);
        (down.0.or(up.0), up.1)
    }

    fn rings(ctx: &egui::Context, shapes: &[egui::epaint::ClippedShape]) -> usize {
        let ring = ctx.global_style().visuals.selection.stroke.color;
        shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color == ring)).count()
    }

    #[test]
    fn tab_walks_the_tabs_then_the_toolbar_and_enter_or_space_presses_each() {
        use crate::ui::tabs::TabEvent;
        let ctx = egui::Context::default();
        let nav = Nav {
            current: Path::new("/home/u/Descargas"),
            home: Some(Path::new("/home/u")),
            can_back: true,
            can_forward: true,
            bookmarked: false,
            saved_view: SavedView::default(),
        };
        let (_, shapes) = window(&ctx, &nav, vec![]);
        assert_eq!(rings(&ctx, &shapes), 0, "no ring before Tab");
        let stops = [
            Pressed::Tab(TabEvent::Select(0)),
            Pressed::Tab(TabEvent::Select(1)),
            Pressed::Tab(TabEvent::New),
            Pressed::Bar(ToolbarEvent::ToggleSidebar),
            Pressed::Bar(ToolbarEvent::Back),
            Pressed::Bar(ToolbarEvent::Forward),
            Pressed::Bar(ToolbarEvent::Up),
            // The lock, which opens a menu instead.
            Pressed::Bar(ToolbarEvent::SaveView(None)),
            Pressed::Bar(ToolbarEvent::ToggleBookmark),
            Pressed::Bar(ToolbarEvent::EditPath),
        ];
        for (n, stop) in stops.into_iter().enumerate() {
            let (pressed, shapes) = tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
            assert_eq!(pressed, None, "Tab only moves the focus");
            assert_eq!(rings(&ctx, &shapes), 1, "one ring on stop {n}");
            let key = if n % 2 == 0 { egui::Key::Enter } else { egui::Key::Space };
            if stop == Pressed::Bar(ToolbarEvent::SaveView(None)) {
                continue; // The lock opens its menu: see the next test.
            }
            assert_eq!(tap(&ctx, &nav, key, egui::Modifiers::NONE).0, Some(stop), "{key:?} on stop {n}");
        }
        // Past the address bar it starts again; Shift+Tab goes back.
        tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
        assert_eq!(tap(&ctx, &nav, egui::Key::Enter, egui::Modifiers::NONE).0, Some(Pressed::Tab(TabEvent::Select(0))));
        tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::SHIFT);
        window(&ctx, &nav, vec![]);
        assert_eq!(tap(&ctx, &nav, egui::Key::Enter, egui::Modifiers::NONE).0, Some(Pressed::Bar(ToolbarEvent::EditPath)));
    }
    #[test]
    fn enter_or_space_on_the_lock_opens_its_menu_and_escape_closes_it() {
        let ctx = egui::Context::default();
        let nav = Nav { can_back: true, can_forward: true, ..nav_with(SavedView::default()) };
        let menu_open = |shapes: &[egui::epaint::ClippedShape]| shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Saved view settings"));
        for key in [egui::Key::Enter, egui::Key::Space] {
            window(&ctx, &nav, vec![]);
            // Three tab stops, then ☰, back, forward, up: the lock is the eighth.
            for _ in 0..8 {
                tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
            }
            let (pressed, shapes) = tap(&ctx, &nav, key, egui::Modifiers::NONE);
            assert_eq!(pressed, None, "{key:?} on the lock chooses nothing");
            assert!(menu_open(&shapes), "{key:?} on the lock opens its menu");
            let (_, shapes) = tap(&ctx, &nav, egui::Key::Escape, egui::Modifiers::NONE);
            assert!(!menu_open(&shapes), "Escape closes it");
        }
    }

    #[test]
    fn tab_out_of_the_lock_menu_closes_it() {
        let ctx = egui::Context::default();
        let nav = Nav { can_back: true, can_forward: true, ..nav_with(SavedView::default()) };
        let menu_open = |shapes: &[egui::epaint::ClippedShape]| shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Saved view settings"));
        window(&ctx, &nav, vec![]);
        for _ in 0..8 {
            tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
        }
        assert!(menu_open(&tap(&ctx, &nav, egui::Key::Enter, egui::Modifiers::NONE).1));
        // The menu itself, None, This folder, This folder and subfolders, Set as default, then the star.
        let mut open_on = Vec::new();
        for _ in 0..6 {
            let (_, shapes) = tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
            open_on.push(menu_open(&shapes));
        }
        assert!(open_on[..5].iter().all(|open| *open), "moving inside the menu keeps it open: {open_on:?}");
        assert!(!open_on[5], "the focus left the menu, so it closed: {open_on:?}");
        assert_eq!(tap(&ctx, &nav, egui::Key::Enter, egui::Modifiers::NONE).0, Some(Pressed::Bar(ToolbarEvent::ToggleBookmark)), "the star has the focus");
    }

    #[test]
    fn a_click_opens_the_lock_menu_even_while_another_widget_has_the_focus() {
        let ctx = egui::Context::default();
        let nav = Nav { can_back: true, can_forward: true, ..nav_with(SavedView::default()) };
        let menu_open = |shapes: &[egui::epaint::ClippedShape]| shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Saved view settings"));
        window(&ctx, &nav, vec![]);
        // Three tab stops, ☰, back, forward, up, the lock: the star is the ninth.
        for _ in 0..9 {
            tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
        }
        let lock = ctx.read_response(lock_id()).expect("the lock was drawn").rect.center();
        let star = ctx.memory(|m| m.focused());
        assert!(star.is_some_and(|id| id != lock_id()), "another widget has the focus");
        window(&ctx, &nav, pointer_click(lock));
        for frame in 0..3 {
            assert!(menu_open(&window(&ctx, &nav, vec![]).1), "still open on frame {frame}");
        }
    }

    #[test]
    fn escape_closes_the_lock_menu_before_the_app_reads_it() {
        let ctx = egui::Context::default();
        let nav = nav_with(SavedView::default());
        let (_, texts) = bar(&ctx, &nav, vec![]);
        bar(&ctx, &nav, pointer_click(at(&texts, LOCK).center()));
        assert!(lock_menu_open(&ctx));
        let escape = |pressed| egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        let input = egui::RawInput { events: vec![escape(true)], screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut left_for_the_app = true;
        let _ = ctx.run_ui(input, |root| {
            assert!(close_lock_menu_on_escape(root.ctx()), "it was open");
            left_for_the_app = root.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        });
        assert!(!left_for_the_app, "that Escape is used up: it does not also unselect");
        assert!(!lock_menu_open(&ctx));
        let input = egui::RawInput { events: vec![escape(true)], screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let _ = ctx.run_ui(input, |root| {
            assert!(!close_lock_menu_on_escape(root.ctx()), "closed: nothing to do");
            left_for_the_app = root.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        });
        assert!(left_for_the_app, "with the menu closed Escape is the app's");
    }

    fn nav_with(saved_view: SavedView) -> Nav<'static> {
        Nav { current: Path::new("/home/u/Fotos"), home: Some(Path::new("/home/u")), can_back: false, can_forward: false, bookmarked: false, saved_view }
    }

    /// One frame of the bar with `events`; what it returned and the texts painted.
    fn bar(ctx: &egui::Context, nav: &Nav<'_>, events: Vec<egui::Event>) -> (Option<ToolbarEvent>, Vec<(String, egui::Rect)>) {
        let input = egui::RawInput { events, screen_rect: Some(crate::ui::testing::SCREEN), ..Default::default() };
        let mut event = None;
        let mut output = ctx.run_ui(input, |root| {
            egui::Panel::top("toolbar").show(root, |ui| event = show(ui, nav));
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                // The lock is drawn: its body stands for it.
                egui::Shape::Rect(r) if r.corner_radius == egui::CornerRadius::same(crate::ui::icons::LOCK_ROUNDING) && r.fill.a() > 0 => {
                    Some((LOCK.to_owned(), r.rect))
                }
                _ => None,
            })
            .collect();
        (event, texts)
    }

    /// The name tests give the drawn lock among the painted texts.
    const LOCK: &str = "<lock>";

    fn pointer_click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    fn at(texts: &[(String, egui::Rect)], text: &str) -> egui::Rect {
        texts.iter().find(|(t, _)| t == text).map(|(_, r)| *r).unwrap_or_else(|| panic!("{text:?} not painted: {texts:?}"))
    }

    /// Clicks the lock, then `choice` in its menu; returns what the bar said.
    fn choose(saved: SavedView, choice: &str) -> (Option<ToolbarEvent>, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        let nav = nav_with(saved);
        let (_, texts) = bar(&ctx, &nav, vec![]);
        let lock = at(&texts, LOCK);
        assert_eq!(bar(&ctx, &nav, pointer_click(lock.center())).0, None, "opening chooses nothing");
        let (_, menu) = bar(&ctx, &nav, vec![]);
        let item = at(&menu, choice);
        let (event, _) = bar(&ctx, &nav, pointer_click(item.center()));
        (event, menu)
    }

    #[test]
    fn the_lock_sits_after_up_and_opens_saved_view_settings() {
        let ctx = egui::Context::default();
        let nav = nav_with(SavedView::default());
        let (_, texts) = bar(&ctx, &nav, vec![]);
        assert!(at(&texts, LOCK).left() > at(&texts, UP).right(), "after the up arrow");
        assert!(at(&texts, LOCK).right() < at(&texts, NOT_BOOKMARKED).left(), "before the star");
        let (_, menu) = choose(SavedView::default(), "None");
        let items = ["Saved view settings", "None", "This folder", "This folder and subfolders", "Set as default"];
        let tops: Vec<f32> = items.iter().map(|item| at(&menu, item).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "top to bottom in this order: {tops:?}");
    }

    #[test]
    fn each_choice_of_the_lock_menu_is_an_event() {
        assert_eq!(choose(SavedView::default(), "This folder").0, Some(ToolbarEvent::SaveView(Some(Scope::Folder))));
        assert_eq!(choose(SavedView::default(), "This folder and subfolders").0, Some(ToolbarEvent::SaveView(Some(Scope::Tree))));
        let own = SavedView { own: Some(Scope::Folder), ..SavedView::default() };
        assert_eq!(choose(own, "None").0, Some(ToolbarEvent::SaveView(None)));
        assert_eq!(choose(SavedView::default(), "Set as default").0, Some(ToolbarEvent::SetDefaultView));
    }

    #[test]
    fn set_as_default_is_greyed_while_the_view_is_already_the_default() {
        let already = SavedView { is_default: true, ..SavedView::default() };
        assert_eq!(choose(already, "Set as default").0, None);
    }

    #[test]
    fn a_folder_covered_from_above_names_that_folder_in_the_menu() {
        let covered = SavedView { covered_by: Some(PathBuf::from("/home/u")), ..SavedView::default() };
        let (_, menu) = choose(covered, "None");
        at(&menu, "Saved with u and its subfolders");
    }
}
