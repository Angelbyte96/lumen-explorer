//! The bar above the list: back, forward and up buttons, the bookmark star,
//! and the current path as clickable crumbs ("angelbyte › Descargas").
//! Ctrl+L or a click on the empty part turns the crumbs into a text field.

use std::path::{Path, PathBuf};

use eframe::egui;


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
            (UP, nav.current.parent().is_some(), "Up to the parent folder (Alt+Up)", ToolbarEvent::Up),
        ];
        for (glyph, enabled, hint, on_click) in buttons {
            let button = egui::Button::new(egui::RichText::new(glyph).size(16.0)).frame(false);
            let button = ui.add_enabled(enabled, button);
            crate::ui::focus_ring(ui, &button);
            if button.on_hover_text(hint).clicked() {
                event = Some(on_click);
            }
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
    fn a_click_on_the_empty_crumb_bar_opens_the_path_box() {
        use crate::ui::testing::{click, frame};
        let ctx = egui::Context::default();
        let nav = Nav {
            current: Path::new("/home/u/Descargas"),
            home: Some(Path::new("/home/u")),
            can_back: false,
            can_forward: false,
            bookmarked: false,
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
            Pressed::Bar(ToolbarEvent::ToggleBookmark),
            Pressed::Bar(ToolbarEvent::EditPath),
        ];
        for (n, stop) in stops.into_iter().enumerate() {
            let (pressed, shapes) = tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
            assert_eq!(pressed, None, "Tab only moves the focus");
            assert_eq!(rings(&ctx, &shapes), 1, "one ring on stop {n}");
            let key = if n % 2 == 0 { egui::Key::Enter } else { egui::Key::Space };
            assert_eq!(tap(&ctx, &nav, key, egui::Modifiers::NONE).0, Some(stop), "{key:?} on stop {n}");
        }
        // Past the address bar it starts again; Shift+Tab goes back.
        tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::NONE);
        assert_eq!(tap(&ctx, &nav, egui::Key::Enter, egui::Modifiers::NONE).0, Some(Pressed::Tab(TabEvent::Select(0))));
        tap(&ctx, &nav, egui::Key::Tab, egui::Modifiers::SHIFT);
        window(&ctx, &nav, vec![]);
        assert_eq!(tap(&ctx, &nav, egui::Key::Enter, egui::Modifiers::NONE).0, Some(Pressed::Bar(ToolbarEvent::EditPath)));
    }
}
