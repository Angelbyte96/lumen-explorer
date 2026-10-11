//! Lumen's own colors, in one place, so themes can replace them later.

use eframe::egui::{Color32, Visuals};

/// The highlight: selected rows, the hovered menu line, a confirming button.
pub fn accent(visuals: &Visuals) -> Color32 {
    visuals.selection.bg_fill
}

/// Text drawn on the accent or on [`danger`].
pub fn on_accent(visuals: &Visuals) -> Color32 {
    visuals.selection.stroke.color
}

/// What cannot be undone: a permanent delete.
pub fn danger(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode { Color32::from_rgb(200, 62, 62) } else { Color32::from_rgb(196, 43, 43) }
}

/// The background of a card inside a dialog.
pub fn card(visuals: &Visuals) -> Color32 {
    visuals.faint_bg_color
}
