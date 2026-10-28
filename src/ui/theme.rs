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
/// schemes by name, with Lumen in Voidstar's place, then Dracula and Gentleman.
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
}

impl Scheme {
    /// Every scheme, in the order the Options window lists them.
    pub const ALL: [Scheme; 8] = [
        Scheme::Sirius,
        Scheme::Pollux,
        Scheme::Eridanus,
        Scheme::Lumen,
        Scheme::Regulus,
        Scheme::Altair,
        Scheme::Dracula,
        Scheme::Gentleman,
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
        }
    }

    /// The scheme named `name`, any case.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name))
    }

    /// What it looks like when the system uses `theme`: Lumen follows it,
    /// every other scheme is always light or always dark.
    pub fn visuals(self, theme: egui::Theme) -> Visuals {
        match self.palette() {
            None => theme.default_visuals(),
            Some(palette) => palette.visuals(),
        }
    }

    /// Its colors; Lumen has none of its own, it is egui's.
    ///
    /// File Pilot's built-in colors are not published: Sirius, Pollux,
    /// Eridanus, Regulus and Altair are Lumen's own take on each star until
    /// they are read from File Pilot. Dracula is draculatheme.com's palette,
    /// Gentleman is gentle-shell's Gentleman-Sexy.
    fn palette(self) -> Option<Palette> {
        Some(match self {
            Scheme::Lumen => return None,
            // A blue-white star: a cool, bright light scheme.
            Scheme::Sirius => Palette {
                dark: false,
                background: hex(0xF4F7FB),
                surface: hex(0xFFFFFF),
                inner: hex(0xFFFFFF),
                widget: hex(0xE3E9F2),
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
                widget: hex(0x3A322A),
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
                widget: hex(0x22304A),
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
                widget: hex(0x2F3245),
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
                widget: hex(0xEFE7DA),
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
                widget: hex(0x343746),
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
                background: hex(0x060407),
                surface: hex(0x100A0F),
                inner: hex(0x100A0F),
                widget: hex(0x2A1720),
                border: hex(0x563040),
                text: hex(0xD2CBD0),
                strong: hex(0xF6EFF3),
                accent: hex(0xBF0F50),
                on_accent: hex(0xF6EFF3),
                link: hex(0xFF4F9A),
                warning: hex(0xF2B86D),
                error: hex(0xFF718F),
            },
        })
    }
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
    /// Buttons and other controls at rest.
    widget: Color32,
    /// Separators and frames.
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
        v.window_stroke = stroke(1.0, self.border);
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
    fn the_schemes_are_file_pilots_with_lumen_for_voidstar_then_dracula_and_gentleman() {
        let names: Vec<&str> = Scheme::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["Sirius", "Pollux", "Eridanus", "Lumen", "Regulus", "Altair", "Dracula", "Gentleman"]);
        for scheme in Scheme::ALL {
            assert_eq!(Scheme::from_name(scheme.name()), Some(scheme));
            assert_eq!(Scheme::from_name(&scheme.name().to_uppercase()), Some(scheme), "any case");
        }
        assert_eq!(Scheme::from_name("Voidstar"), None);
        assert_eq!(Scheme::from_name(""), None);
    }

    #[test]
    fn lumen_keeps_its_current_colors_and_follows_the_system() {
        assert_eq!(Scheme::Lumen.visuals(Theme::Dark), Visuals::dark());
        assert_eq!(Scheme::Lumen.visuals(Theme::Light), Visuals::light());
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
        assert_eq!(v.window_stroke.color, hex(0x44475A), "current line");
    }

    #[test]
    fn gentleman_uses_gentle_shells_pink() {
        let v = Scheme::Gentleman.visuals(Theme::Light);
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, hex(0x060407), "bg");
        assert_eq!(v.window_fill, hex(0x100A0F), "bgPanel");
        assert_eq!(v.widgets.active.fg_stroke.color, hex(0xF6EFF3), "text");
        assert_eq!(v.window_stroke.color, hex(0x563040), "border");
        assert_eq!(v.selection.bg_fill, hex(0xBF0F50), "deepPink");
        assert_eq!(v.hyperlink_color, hex(0xFF4F9A), "activePink");
        assert_eq!(v.error_fg_color, hex(0xFF718F), "error");
        assert_eq!(v.warn_fg_color, hex(0xF2B86D), "warning");
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
        assert_eq!(ctx.style_of(Theme::Light).visuals, Visuals::light(), "back to Lumen's own");
        assert_eq!(ctx.style_of(Theme::Dark).visuals, Visuals::dark());
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
