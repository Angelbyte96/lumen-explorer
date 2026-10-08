//! The bar above the list: back, forward and up buttons, the bookmark star,
//! and the current path as clickable crumbs ("angelbyte › Descargas").
//! Ctrl+L or a click on the empty part turns the crumbs into a text field.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::ui::path_bar::PathBar;

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
    Back,
    Forward,
    Up,
    ToggleBookmark,
    /// A crumb was clicked.
    Go(PathBuf),
    /// Text typed in the path field, confirmed with Enter.
    Typed(String),
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

pub fn show(ui: &mut egui::Ui, nav: &Nav<'_>, path_bar: &mut PathBar) -> Option<ToolbarEvent> {
    let mut event = None;
    ui.horizontal(|ui| {
        let buttons = [
            (BACK, nav.can_back, "Back (Alt+Left)", ToolbarEvent::Back),
            (FORWARD, nav.can_forward, "Forward (Alt+Right)", ToolbarEvent::Forward),
            (UP, nav.current.parent().is_some(), "Up to the parent folder", ToolbarEvent::Up),
        ];
        for (glyph, enabled, hint, on_click) in buttons {
            let button = egui::Button::new(egui::RichText::new(glyph).size(16.0)).frame(false);
            if ui.add_enabled(enabled, button).on_hover_text(hint).clicked() {
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
        if ui.add(star).on_hover_text(hint).clicked() {
            event = Some(ToolbarEvent::ToggleBookmark);
        }
        ui.separator();
        if path_bar.is_editing(ui.ctx()) {
            if let Some(text) = path_bar.show(ui, nav.current) {
                event = Some(ToolbarEvent::Typed(text));
            }
            return;
        }
        if let Some(go) = crumb_bar(ui, nav, path_bar) {
            event = Some(go);
        }
    });
    event
}

/// The crumbs, kept scrolled to the end so the current folder is always visible.
fn crumb_bar(ui: &mut egui::Ui, nav: &Nav<'_>, path_bar: &mut PathBar) -> Option<ToolbarEvent> {
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
                    if ui.add(egui::Button::new(text).frame(false)).on_hover_text(hint).clicked() {
                        event = Some(ToolbarEvent::Go(crumb.path.clone()));
                    }
                }
                // The empty rest of the bar edits the path, like Ctrl+L.
                let rest = egui::vec2(ui.available_width().max(40.0), height);
                let response = ui.allocate_response(rest, egui::Sense::click());
                if response.on_hover_text("Edit the path (Ctrl+L)").clicked() {
                    path_bar.request_focus();
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
}
