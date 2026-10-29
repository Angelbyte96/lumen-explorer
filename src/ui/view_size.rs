//! The view slider (like File Pilot's): one position from 0 to 100 picks the
//! view. Details, Tree, List and M Icons are steps; from M Icons up to XL Icons
//! it is one icon grid whose icons grow smoothly with the position.

use crate::i18n::tr;

/// The views, smallest to largest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Details,
    Tree,
    List,
    Icons,
}

/// The slider's named positions, bottom to top, with the view each one is.
pub const STOPS: [(u8, &str, Layout); 6] = [
    (0, "Details", Layout::Details),
    (10, "Tree", Layout::Tree),
    (20, "List", Layout::List),
    (30, "M Icons", Layout::Icons),
    (65, "L Icons", Layout::Icons),
    (100, "XL Icons", Layout::Icons),
];

/// A stop's name in the interface's language. `STOPS` and `ViewSize::name`
/// hold English, which the views are told apart by.
pub fn shown(name: &str) -> &str {
    match name {
        "Details" => tr!("Details"),
        "Tree" => tr!("Tree"),
        "List" => tr!("List"),
        "M Icons" => tr!("M Icons"),
        "L Icons" => tr!("L Icons"),
        "XL Icons" => tr!("XL Icons"),
        other => other,
    }
}

/// Where the icon grid starts: below it only the steps exist.
pub const ICONS: u8 = 30;
/// How far Ctrl+wheel, Ctrl+Plus and Ctrl+Minus move the slider inside the grid.
pub const STEP: u8 = 5;

/// The slider's position, 0 (Details) to 100 (XL Icons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewSize(u8);

impl ViewSize {
    /// `percent` capped at 100; below the grid it lands on the nearest step,
    /// since nothing lies between two steps.
    pub fn new(percent: u8) -> Self {
        let percent = percent.min(100);
        if percent >= ICONS {
            return Self(percent);
        }
        let nearest = STOPS.iter().map(|s| s.0).filter(|s| *s <= ICONS).min_by_key(|s| s.abs_diff(percent));
        Self(nearest.unwrap_or(0))
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    pub fn layout(self) -> Layout {
        self.stop().2
    }

    /// The name shown for this position: the highest stop it has reached.
    pub fn name(self) -> &'static str {
        self.stop().1
    }

    fn stop(self) -> (u8, &'static str, Layout) {
        *STOPS.iter().rev().find(|s| s.0 <= self.0).unwrap_or(&STOPS[0])
    }

    /// How far the icons have grown from M Icons (0.0) to XL Icons (1.0);
    /// `None` below the grid.
    pub fn growth(self) -> Option<f32> {
        (self.0 >= ICONS).then(|| f32::from(self.0 - ICONS) / f32::from(100 - ICONS))
    }

    /// One Ctrl+wheel notch or Ctrl+Plus: the next step below the grid,
    /// `STEP` inside it, never past XL Icons.
    pub fn bigger(self) -> Self {
        if self.0 < ICONS {
            let next = STOPS.iter().map(|s| s.0).find(|s| *s > self.0);
            return Self(next.unwrap_or(ICONS));
        }
        Self(self.0.saturating_add(STEP).min(100))
    }

    /// One Ctrl+wheel notch down or Ctrl+Minus: `STEP` inside the grid, but
    /// never below M Icons; from M Icons down, the step below.
    pub fn smaller(self) -> Self {
        if self.0 > ICONS {
            return Self(self.0.saturating_sub(STEP).max(ICONS));
        }
        let previous = STOPS.iter().rev().map(|s| s.0).find(|s| *s < self.0);
        Self(previous.unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stop_names_its_view() {
        let named: Vec<(u8, &str, Layout)> = STOPS.iter().map(|s| (s.0, ViewSize::new(s.0).name(), ViewSize::new(s.0).layout())).collect();
        assert_eq!(named, STOPS.to_vec());
        assert_eq!(ViewSize::default(), ViewSize::new(0), "Details unless chosen");
    }

    #[test]
    fn between_two_icon_stops_the_lower_name_holds_and_the_icons_grow_smoothly() {
        assert_eq!(ViewSize::new(64).name(), "M Icons");
        assert_eq!(ViewSize::new(99).name(), "L Icons");
        assert_eq!(ViewSize::new(50).layout(), Layout::Icons);
        assert_eq!(ViewSize::new(30).growth(), Some(0.0));
        assert_eq!(ViewSize::new(65).growth(), Some(0.5));
        assert_eq!(ViewSize::new(100).growth(), Some(1.0));
        assert_eq!(ViewSize::new(20).growth(), None, "List has no icon growth");
    }

    #[test]
    fn below_the_grid_a_position_lands_on_the_nearest_step_and_above_100_is_capped() {
        let landed: Vec<u8> = [0, 4, 6, 14, 16, 24, 26, 29].map(|p| ViewSize::new(p).percent()).to_vec();
        assert_eq!(landed, [0, 0, 10, 10, 20, 20, 30, 30]);
        assert_eq!(ViewSize::new(31).percent(), 31, "the grid keeps every position");
        assert_eq!(ViewSize::new(255).percent(), 100);
    }

    #[test]
    fn bigger_walks_the_steps_then_grows_the_icons_up_to_xl() {
        let mut size = ViewSize::new(0);
        let mut walked = Vec::new();
        while size.percent() < 100 {
            size = size.bigger();
            walked.push(size.percent());
        }
        assert_eq!(walked, [10, 20, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 100]);
        assert_eq!(size.bigger().percent(), 100, "stops at XL Icons");
        assert_eq!(ViewSize::new(98).bigger().percent(), 100);
    }

    #[test]
    fn smaller_shrinks_the_icons_down_to_m_then_walks_the_steps() {
        let mut size = ViewSize::new(100);
        let mut walked = Vec::new();
        while size.percent() > 0 {
            size = size.smaller();
            walked.push(size.percent());
        }
        assert_eq!(walked, [95, 90, 85, 80, 75, 70, 65, 60, 55, 50, 45, 40, 35, 30, 20, 10, 0]);
        assert_eq!(size.smaller().percent(), 0, "stops at Details");
        assert_eq!(ViewSize::new(32).smaller().percent(), 30, "M Icons before List");
    }
}
