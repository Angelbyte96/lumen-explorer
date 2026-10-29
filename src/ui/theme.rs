//! Lumen's own colors, in one place, so themes can replace them later.

use eframe::egui::{self, Color32, Visuals};

/// The highlight: selected rows, the hovered menu line, a confirming button.
pub fn accent(visuals: &Visuals) -> Color32 {
    visuals.selection.bg_fill
}

/// Behind the row under the pointer: the accent, see-through, like File Pilot's.
pub fn row_hover(visuals: &Visuals) -> Color32 {
    accent(visuals).gamma_multiply(ROW_HOVER_OPACITY)
}

/// How much of the accent a hovered row shows.
pub const ROW_HOVER_OPACITY: f32 = 0.35;

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

/// A color scheme, chosen in the Options window. File Pilot's built-in
/// schemes by name, with Lumen in Voidstar's place, then Dracula, Gentleman and Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Sirius,
    Pollux,
    Eridanus,
    Lumen,
    Regulus,
    Altair,
    Dracula,
    Gentleman,
    Space,
}

impl Scheme {
    /// Every scheme, in the order the Options window lists them.
    pub const ALL: [Scheme; 9] = [
        Scheme::Sirius,
        Scheme::Pollux,
        Scheme::Eridanus,
        Scheme::Lumen,
        Scheme::Regulus,
        Scheme::Altair,
        Scheme::Dracula,
        Scheme::Gentleman,
        Scheme::Space,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scheme::Sirius => "Sirius",
            Scheme::Pollux => "Pollux",
            Scheme::Eridanus => "Eridanus",
            Scheme::Lumen => "Lumen",
            Scheme::Regulus => "Regulus",
            Scheme::Altair => "Altair",
            Scheme::Dracula => "Dracula",
            Scheme::Gentleman => "Gentleman",
            Scheme::Space => "Space",
        }
    }

    /// The scheme named `name`, any case.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name))
    }

    /// What it looks like when the system uses `theme`: Lumen follows it,
    /// every other scheme is always light or always dark. Every scheme
    /// has the glass look; Lumen frames its controls in egui's link blue,
    /// brighter than its selection.
    pub fn visuals(self, theme: egui::Theme) -> Visuals {
        match self.palette() {
            None => {
                let mut v = theme.default_visuals();
                let frame = see_through(v.hyperlink_color);
                glass(&mut v, frame);
                v
            }
            Some(palette) => palette.visuals(),
        }
    }

    /// Its colors; Lumen has none of its own, it is egui's.
    ///
    /// File Pilot's built-in colors are not published: Sirius, Pollux,
    /// Eridanus, Regulus and Altair are Lumen's own take on each star until
    /// they are read from File Pilot. Dracula is draculatheme.com's palette,
    /// Gentleman is gentle-shell's Gentleman-Sexy on a dark plum instead of
    /// its black, Space is Lumen's purple glass. The buttons sit close to the
    /// panel, framed in a see-through accent.
    fn palette(self) -> Option<Palette> {
        Some(match self {
            Scheme::Lumen => return None,
            // A blue-white star: a cool, bright light scheme.
            Scheme::Sirius => Palette {
                dark: false,
                background: hex(0xF4F7FB),
                surface: hex(0xFFFFFF),
                inner: hex(0xFFFFFF),
                widget: hex(0xEBF0F7),
                frame: glass_frame(0x2F6FEB),
                border: hex(0xC5CFDD),
                text: hex(0x3A4250),
                strong: hex(0x10151C),
                accent: hex(0x2F6FEB),
                on_accent: hex(0xFFFFFF),
                link: hex(0x2F6FEB),
                warning: hex(0xB26A00),
                error: hex(0xC42B2B),
            },
            // An orange giant: warm browns and amber.
            Scheme::Pollux => Palette {
                dark: true,
                background: hex(0x1E1A16),
                surface: hex(0x26211C),
                inner: hex(0x15120F),
                widget: hex(0x27221D),
                frame: glass_frame(0xE0954A),
                border: hex(0x4A4036),
                text: hex(0xCFC4B6),
                strong: hex(0xF5ECE0),
                accent: hex(0xD9822B),
                on_accent: hex(0x1E1A16),
                link: hex(0xF0A85A),
                warning: hex(0xF2B86D),
                error: hex(0xE5604D),
            },
            // The river constellation: deep navy and teal.
            Scheme::Eridanus => Palette {
                dark: true,
                background: hex(0x0F1724),
                surface: hex(0x152033),
                inner: hex(0x0A101A),
                widget: hex(0x162133),
                frame: glass_frame(0x3BBDB5),
                border: hex(0x2E3F5C),
                text: hex(0xB4C2D6),
                strong: hex(0xEAF1FB),
                accent: hex(0x1FA7A0),
                on_accent: hex(0x0A101A),
                link: hex(0x4FD1C5),
                warning: hex(0xF2B86D),
                error: hex(0xF2647A),
            },
            // A blue-white star at night: slate and blue-violet.
            Scheme::Regulus => Palette {
                dark: true,
                background: hex(0x1A1B26),
                surface: hex(0x222433),
                inner: hex(0x12131B),
                widget: hex(0x232534),
                frame: glass_frame(0x7480EA),
                border: hex(0x3D4159),
                text: hex(0xBFC3DA),
                strong: hex(0xF0F1FA),
                accent: hex(0x5563DE),
                on_accent: hex(0xFFFFFF),
                link: hex(0x8C98FF),
                warning: hex(0xF2B86D),
                error: hex(0xF26D7D),
            },
            // A white star in a summer sky: warm paper and burnt orange.
            Scheme::Altair => Palette {
                dark: false,
                background: hex(0xFAF6EF),
                surface: hex(0xFFFDF8),
                inner: hex(0xFFFFFF),
                widget: hex(0xF3ECE1),
                frame: glass_frame(0xB8501A),
                border: hex(0xD8CCB8),
                text: hex(0x4A4036),
                strong: hex(0x1C1712),
                accent: hex(0xB8501A),
                on_accent: hex(0xFFFFFF),
                link: hex(0xB8501A),
                warning: hex(0x9A6700),
                error: hex(0xC42B2B),
            },
            Scheme::Dracula => Palette {
                dark: true,
                background: hex(0x282A36),
                surface: hex(0x21222C),
                inner: hex(0x191A21),
                widget: hex(0x2F3140),
                frame: glass_frame(0xBD93F9),
                border: hex(0x44475A),
                text: hex(0xF8F8F2),
                strong: hex(0xFFFFFF),
                accent: hex(0xBD93F9),
                on_accent: hex(0x282A36),
                link: hex(0x8BE9FD),
                warning: hex(0xFFB86C),
                error: hex(0xFF5555),
            },
            Scheme::Gentleman => Palette {
                dark: true,
                background: hex(0x221820),
                surface: hex(0x2A1E27),
                inner: hex(0x170F15),
                widget: hex(0x2C1F28),
                frame: glass_frame(0xE0306F),
                border: hex(0x563040),
                text: hex(0xD2CBD0),
                strong: hex(0xF6EFF3),
                accent: hex(0xF43888),
                on_accent: hex(0x060407),
                link: hex(0xFF4F9A),
                warning: hex(0xF2B86D),
                error: hex(0xFF718F),
            },
            // Purple glass: controls barely lighter than the panel, framed in
            // see-through lavender (no real blur behind them).
            Scheme::Space => Palette {
                dark: true,
                background: hex(0x221D25),
                surface: hex(0x29222D),
                inner: hex(0x1A161D),
                widget: hex(0x2A2330),
                frame: SPACE_BORDER,
                border: SPACE_BORDER,
                text: hex(0xC9B3DD),
                strong: hex(0xF3ECF9),
                accent: hex(0x7A4A96),
                on_accent: hex(0xF3ECF9),
                link: hex(0xB98BE0),
                warning: hex(0xF2B86D),
                error: hex(0xFF7A9A),
            },
        })
    }
}

/// How much shows through a glass frame.
const GLASS_FRAME_ALPHA: u8 = 0x99;
/// The opaque `color` as a glass frame.
const fn see_through(color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(color.r(), color.g(), color.b(), GLASS_FRAME_ALPHA)
}
/// A glass frame in `rgb`.
const fn glass_frame(rgb: u32) -> Color32 {
    see_through(hex(rgb))
}
/// Space's frames and separators: its lavender, see-through.
const SPACE_BORDER: Color32 = glass_frame(0x9060AA);
/// How round the controls are.
const GLASS_CORNERS: u8 = 6;
/// How round windows and menus are.
const GLASS_WINDOW_CORNERS: u8 = 10;

/// The glass look on top of a scheme's colors: controls at rest and
/// windows framed in `frame`, rounder corners (there is no real blur behind
/// them). Hovered, pressed and open controls keep their own frames, so the
/// see-through one marks a control at rest.
fn glass(v: &mut Visuals, frame: Color32) {
    v.window_stroke = egui::Stroke::new(1.0, frame);
    let w = &mut v.widgets;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, frame);
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = egui::CornerRadius::same(GLASS_CORNERS);
    }
    v.window_corner_radius = egui::CornerRadius::same(GLASS_WINDOW_CORNERS);
    v.menu_corner_radius = egui::CornerRadius::same(GLASS_WINDOW_CORNERS);
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// A scheme's colors, by role.
struct Palette {
    dark: bool,
    /// Panels and the window.
    background: Color32,
    /// Popups and menus.
    surface: Color32,
    /// Inside text fields.
    inner: Color32,
    /// Buttons and other controls at rest, close to the background.
    widget: Color32,
    /// Around buttons at rest and windows: the accent, see-through.
    frame: Color32,
    /// Separators, and around hovered and open controls.
    border: Color32,
    text: Color32,
    /// Hovered and pressed text, headings.
    strong: Color32,
    /// Selected rows; see [`accent`].
    accent: Color32,
    /// Text on the accent.
    on_accent: Color32,
    link: Color32,
    warning: Color32,
    error: Color32,
}

impl Palette {
    /// egui's own visuals for a light or dark scheme, in these colors.
    fn visuals(&self) -> Visuals {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        let stroke = |width, color| egui::Stroke::new(width, color);
        let hovered = self.widget.lerp_to_gamma(self.strong, 0.08);
        let pressed = self.widget.lerp_to_gamma(self.strong, 0.16);
        v.panel_fill = self.background;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.inner;
        v.faint_bg_color = self.background.lerp_to_gamma(self.strong, 0.04);
        v.code_bg_color = self.widget;
        v.hyperlink_color = self.link;
        v.warn_fg_color = self.warning;
        v.error_fg_color = self.error;
        v.selection.bg_fill = self.accent;
        v.selection.stroke = stroke(1.0, self.on_accent);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.background;
        w.noninteractive.weak_bg_fill = self.background;
        w.noninteractive.bg_stroke = stroke(1.0, self.border);
        w.noninteractive.fg_stroke = stroke(1.0, self.text);
        w.inactive.bg_fill = self.widget;
        w.inactive.weak_bg_fill = self.widget;
        w.inactive.fg_stroke = stroke(1.0, self.text.lerp_to_gamma(self.strong, 0.5));
        w.hovered.bg_fill = hovered;
        w.hovered.weak_bg_fill = hovered;
        w.hovered.bg_stroke = stroke(1.0, self.border.lerp_to_gamma(self.strong, 0.3));
        w.hovered.fg_stroke = stroke(1.5, self.strong);
        w.active.bg_fill = pressed;
        w.active.weak_bg_fill = pressed;
        w.active.bg_stroke = stroke(1.0, self.accent);
        w.active.fg_stroke = stroke(2.0, self.strong);
        w.open.bg_fill = self.background;
        w.open.weak_bg_fill = hovered;
        w.open.bg_stroke = stroke(1.0, self.border);
        w.open.fg_stroke = stroke(1.0, self.text.lerp_to_gamma(self.strong, 0.5));
        glass(&mut v, self.frame);
        v
    }
}

/// The schemes in use: one for both, or one per system theme when
/// `follow_system` is on (File Pilot's "System Light / Dark Scheme").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schemes {
    pub scheme: Scheme,
    pub follow_system: bool,
    pub light: Scheme,
    pub dark: Scheme,
}

impl Default for Schemes {
    fn default() -> Self {
        Self { scheme: Scheme::Lumen, follow_system: false, light: Scheme::Lumen, dark: Scheme::Lumen }
    }
}

impl Schemes {
    /// The scheme shown while the system uses `theme`.
    pub fn for_theme(&self, theme: egui::Theme) -> Scheme {
        match (self.follow_system, theme) {
            (false, _) => self.scheme,
            (true, egui::Theme::Light) => self.light,
            (true, egui::Theme::Dark) => self.dark,
        }
    }

    /// Gives `ctx` the visuals of both system themes; egui shows the one the system uses.
    pub fn apply(&self, ctx: &egui::Context) {
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.set_visuals_of(theme, self.for_theme(theme).visuals(theme));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Theme;

    /// WCAG contrast ratio of two opaque colors.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let channel = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn the_schemes_are_file_pilots_with_lumen_for_voidstar_then_dracula_gentleman_and_space() {
        let names: Vec<&str> = Scheme::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["Sirius", "Pollux", "Eridanus", "Lumen", "Regulus", "Altair", "Dracula", "Gentleman", "Space"]);
        for scheme in Scheme::ALL {
            assert_eq!(Scheme::from_name(scheme.name()), Some(scheme));
            assert_eq!(Scheme::from_name(&scheme.name().to_uppercase()), Some(scheme), "any case");
        }
        assert_eq!(Scheme::from_name("Voidstar"), None);
        assert_eq!(Scheme::from_name(""), None);
    }

    #[test]
    fn lumen_keeps_its_current_colors_and_follows_the_system() {
        for (theme, egui) in [(Theme::Dark, Visuals::dark()), (Theme::Light, Visuals::light())] {
            let v = Scheme::Lumen.visuals(theme);
            assert_eq!(v.dark_mode, egui.dark_mode, "{theme:?}");
            assert_eq!((v.panel_fill, v.window_fill, v.selection), (egui.panel_fill, egui.window_fill, egui.selection), "{theme:?}");
            assert_eq!(v.widgets.inactive.bg_fill, egui.widgets.inactive.bg_fill, "{theme:?}");
            assert_eq!(v.widgets.noninteractive.fg_stroke, egui.widgets.noninteractive.fg_stroke, "{theme:?}");
            assert_ne!(v, egui, "{theme:?}: with the glass look");
            let [r, g, b, _] = egui.hyperlink_color.to_srgba_unmultiplied();
            let frame = Color32::from_rgba_unmultiplied(r, g, b, GLASS_FRAME_ALPHA);
            assert_eq!((v.widgets.inactive.bg_stroke.color, v.window_stroke.color), (frame, frame), "{theme:?}: framed in see-through link blue");
        }
    }

    #[test]
    fn every_scheme_is_glass() {
        for scheme in Scheme::ALL {
            for theme in [Theme::Dark, Theme::Light] {
                let v = scheme.visuals(theme);
                let frame = v.widgets.inactive.bg_stroke.color;
                assert!(frame.a() > 0 && frame.a() < 255, "{scheme:?} {theme:?}: buttons framed see-through, {frame:?}");
                assert_eq!(v.window_stroke.color, frame, "{scheme:?} {theme:?}: windows in the same frame");
                for w in [v.widgets.noninteractive, v.widgets.inactive, v.widgets.hovered, v.widgets.active, v.widgets.open] {
                    assert_eq!(w.corner_radius, egui::CornerRadius::same(GLASS_CORNERS), "{scheme:?} {theme:?}: rounded controls");
                }
                let windows = egui::CornerRadius::same(GLASS_WINDOW_CORNERS);
                assert_eq!((v.window_corner_radius, v.menu_corner_radius), (windows, windows), "{scheme:?} {theme:?}");
            }
        }
    }

    /// Below this contrast a button reads as part of the panel, told apart
    /// by its glass frame rather than its fill.
    const CLOSE_TO_THE_PANEL: f32 = 1.3;

    #[test]
    fn the_fixed_schemes_buttons_sit_close_to_the_panel() {
        for scheme in Scheme::ALL.into_iter().filter(|&s| s != Scheme::Lumen) {
            let v = scheme.visuals(Theme::Dark);
            let step = contrast(v.widgets.inactive.bg_fill, v.panel_fill);
            assert!(step < CLOSE_TO_THE_PANEL, "{scheme:?}: a button {step:.2} away from the panel");
        }
    }

    #[test]
    fn dracula_uses_the_editors_palette() {
        let v = Scheme::Dracula.visuals(Theme::Light);
        assert!(v.dark_mode, "a dark scheme, also while the system is light");
        assert_eq!(v.panel_fill, hex(0x282A36), "background");
        assert_eq!(v.widgets.noninteractive.fg_stroke.color, hex(0xF8F8F2), "foreground");
        assert_eq!(v.selection.bg_fill, hex(0xBD93F9), "purple");
        assert_eq!(v.hyperlink_color, hex(0x8BE9FD), "cyan");
        assert_eq!(v.error_fg_color, hex(0xFF5555), "red");
        assert_eq!(v.warn_fg_color, hex(0xFFB86C), "orange");
        assert_eq!(v.widgets.noninteractive.bg_stroke.color, hex(0x44475A), "current line, on separators");
        assert_eq!(v.window_stroke.color, glass_frame(0xBD93F9), "framed in see-through purple");
    }

    #[test]
    fn gentleman_uses_gentle_shells_pink() {
        let v = Scheme::Gentleman.visuals(Theme::Light);
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, hex(0x221820), "a dark plum, not gentle-shell's black bg");
        assert!(contrast(v.panel_fill, Color32::BLACK) > 1.15, "lifted from black");
        assert_eq!(v.widgets.active.fg_stroke.color, hex(0xF6EFF3), "text");
        assert_eq!(v.widgets.noninteractive.bg_stroke.color, hex(0x563040), "border, on separators");
        assert_eq!(v.selection.bg_fill, hex(0xF43888), "accent, the theme's own pink");
        assert_eq!(v.selection.stroke.color, hex(0x060407), "bg, readable on the pink");
        assert_eq!(v.hyperlink_color, hex(0xFF4F9A), "activePink");
        assert_eq!(v.error_fg_color, hex(0xFF718F), "error");
        assert_eq!(v.warn_fg_color, hex(0xF2B86D), "warning");
    }

    #[test]
    fn space_is_purple_glass() {
        let v = Scheme::Space.visuals(Theme::Light);
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, hex(0x221D25), "panel");
        assert_eq!(v.widgets.inactive.bg_fill, hex(0x2A2330), "buttons close to the panel");
        let border = v.widgets.inactive.bg_stroke.color;
        assert_eq!(border, SPACE_BORDER, "buttons framed in see-through lavender");
        assert!(border.a() < 255);
        assert_eq!(v.window_stroke.color, border);
        assert_eq!(v.selection.bg_fill, hex(0x7A4A96), "purple selection");
        assert_eq!(v.widgets.noninteractive.fg_stroke.color, hex(0xC9B3DD), "lavender text");
    }

    #[test]
    fn every_other_scheme_looks_the_same_whatever_the_system_uses() {
        let light = [Scheme::Sirius, Scheme::Altair];
        for scheme in Scheme::ALL.into_iter().filter(|&s| s != Scheme::Lumen) {
            assert_eq!(scheme.visuals(Theme::Dark), scheme.visuals(Theme::Light), "{scheme:?}");
            assert_eq!(scheme.visuals(Theme::Dark).dark_mode, !light.contains(&scheme), "{scheme:?}");
        }
    }

    #[test]
    fn every_scheme_keeps_text_readable() {
        for scheme in Scheme::ALL {
            for theme in [Theme::Dark, Theme::Light] {
                let v = scheme.visuals(theme);
                let text = contrast(v.widgets.noninteractive.fg_stroke.color, v.panel_fill);
                assert!(text >= 4.5, "{scheme:?} {theme:?}: text on the panel {text:.2}");
                let selected = contrast(v.selection.stroke.color, v.selection.bg_fill);
                assert!(selected >= 4.5, "{scheme:?} {theme:?}: text on a selected row {selected:.2}");
                let strong = contrast(v.strong_text_color(), v.window_fill);
                assert!(strong >= 4.5, "{scheme:?} {theme:?}: strong text in a popup {strong:.2}");
            }
        }
    }

    #[test]
    fn one_scheme_serves_both_system_themes_until_the_system_switch_is_on() {
        let one = Schemes { scheme: Scheme::Dracula, follow_system: false, light: Scheme::Sirius, dark: Scheme::Gentleman };
        assert_eq!((one.for_theme(Theme::Light), one.for_theme(Theme::Dark)), (Scheme::Dracula, Scheme::Dracula));
        let per_theme = Schemes { follow_system: true, ..one };
        assert_eq!((per_theme.for_theme(Theme::Light), per_theme.for_theme(Theme::Dark)), (Scheme::Sirius, Scheme::Gentleman));
        assert_eq!(Schemes::default(), Schemes { scheme: Scheme::Lumen, follow_system: false, light: Scheme::Lumen, dark: Scheme::Lumen });
    }

    #[test]
    fn applying_gives_the_context_each_system_themes_visuals() {
        let ctx = egui::Context::default();
        Schemes { scheme: Scheme::Lumen, follow_system: true, light: Scheme::Sirius, dark: Scheme::Gentleman }.apply(&ctx);
        assert_eq!(ctx.style_of(Theme::Light).visuals, Scheme::Sirius.visuals(Theme::Light));
        assert_eq!(ctx.style_of(Theme::Dark).visuals, Scheme::Gentleman.visuals(Theme::Dark));
        Schemes::default().apply(&ctx);
        assert_eq!(ctx.style_of(Theme::Light).visuals, Scheme::Lumen.visuals(Theme::Light), "back to Lumen's own");
        assert_eq!(ctx.style_of(Theme::Dark).visuals, Scheme::Lumen.visuals(Theme::Dark));
    }

    #[test]
    fn applying_keeps_the_rest_of_the_style() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|s| s.spacing.item_spacing = egui::vec2(7.0, 7.0));
        Schemes { scheme: Scheme::Dracula, ..Schemes::default() }.apply(&ctx);
        for theme in [Theme::Dark, Theme::Light] {
            assert_eq!(ctx.style_of(theme).spacing.item_spacing, egui::vec2(7.0, 7.0));
        }
    }
}
