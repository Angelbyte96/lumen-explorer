//! The Spanish catalog: each English string the interface shows and its
//! translation, one file per area of the sources. Neutral Spanish, as
//! Windows and GNOME word it in Spanish. A string several areas use has the
//! same translation in each.

use std::collections::HashMap;
use std::sync::OnceLock;

mod app;
mod options;
mod system;
mod ui;

pub const AREAS: [&[(&str, &str)]; 4] = [options::ENTRIES, app::ENTRIES, ui::ENTRIES, system::ENTRIES];

pub fn get(english: &str) -> Option<&'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| AREAS.iter().flat_map(|area| area.iter().copied()).collect()).get(english).copied()
}
