//! The Options window, like File Pilot's: Ctrl+, (or ☰ ▸ Options) opens it
//! centred on the window. A search box on top narrows the rows by name and by
//! the words that point to them; each row names its option, says what it
//! does and holds its control, with a reset icon while it differs from the
//! default. Up/Down pick a row, Enter uses it, Escape closes (or goes back
//! from a list of choices).

use eframe::egui;

use crate::ui::theme::{Scheme, Schemes};

/// One option, a row of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opt {
    /// "System Light / Dark Scheme": a switch.
    System,
    /// One scheme for both system themes.
    Scheme,
    /// The scheme for the light and the dark system theme, with the switch on.
    Light,
    Dark,
}

impl Opt {
    fn label(self) -> &'static str {
        match self {
            Opt::System => "System Light / Dark Scheme",
            Opt::Scheme => "Color Scheme",
            Opt::Light => "Light Color Scheme",
            Opt::Dark => "Dark Color Scheme",
        }
    }

    /// Words the search also finds the row by, lowercase.
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Opt::System => &["theme", "dark", "light", "system"],
            Opt::Scheme | Opt::Light | Opt::Dark => &["theme", "color", "palette"],
        }
    }

    fn about(self) -> &'static str {
        match self {
            Opt::System => "Automatically adapt between light and dark color schemes based on system settings.",
            Opt::Scheme => "Applies a color palette to all UI elements.",
            Opt::Light => "Color theme applied when the system uses bright colors.",
            Opt::Dark => "Color theme applied when the system uses dark colors.",
        }
    }

    /// The scheme a row that opens the list holds.
    fn choice(self, schemes: &Schemes) -> Option<Scheme> {
        match self {
            Opt::System => None,
            Opt::Scheme => Some(schemes.scheme),
            Opt::Light => Some(schemes.light),
            Opt::Dark => Some(schemes.dark),
        }
    }

    fn choose(self, schemes: &mut Schemes, scheme: Scheme) {
        match self {
            Opt::System => {}
            Opt::Scheme => schemes.scheme = scheme,
            Opt::Light => schemes.light = scheme,
            Opt::Dark => schemes.dark = scheme,
        }
    }

    fn is_default(self, schemes: &Schemes) -> bool {
        let mut restored = *schemes;
        self.restore(&mut restored);
        restored == *schemes
    }

    /// Puts this option's default back, leaving the others.
    fn restore(self, schemes: &mut Schemes) {
        let default = Schemes::default();
        match self {
            Opt::System => schemes.follow_system = default.follow_system,
            Opt::Scheme => schemes.scheme = default.scheme,
            Opt::Light => schemes.light = default.light,
            Opt::Dark => schemes.dark = default.dark,
        }
    }
}

/// The rows the window shows: the options that apply now whose name or an
/// alias holds every word of `search`, in any case.
fn listed(schemes: &Schemes, search: &str) -> Vec<Opt> {
    let all: &[Opt] = if schemes.follow_system { &[Opt::System, Opt::Light, Opt::Dark] } else { &[Opt::System, Opt::Scheme] };
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    all.iter()
        .copied()
        .filter(|opt| words.iter().all(|w| opt.label().to_lowercase().contains(w) || opt.aliases().iter().any(|a| a.contains(w))))
        .collect()
}

/// Keys the window reads itself, before its search field sees them.
struct Keys {
    up: bool,
    down: bool,
    enter: bool,
    escape: bool,
}

impl Keys {
    fn read(ctx: &egui::Context) -> Self {
        use egui::{Key, Modifiers};
        ctx.input_mut(|i| Self {
            up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
            down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
            enter: i.consume_key(Modifiers::NONE, Key::Enter),
            escape: i.consume_key(Modifiers::NONE, Key::Escape),
        })
    }
}

/// What a click in the window asked for.
enum Ask {
    /// The row or choice at this index.
    Use(usize),
    /// The reset icon of a row.
    Reset(Opt),
    /// The back button of the list.
    Back,
}

const WIDTH: f32 = 520.0;
/// Room kept on a row's right for its control.
const CONTROL: f32 = 170.0;
const RESET: &str = "↺";
const BACK: &str = "‹";
/// Marks the scheme in use in the list (the fonts have no check mark).
const CURRENT: &str = "•";
const FOOTER: &str = "Up Down to navigate · Enter to use · Escape to dismiss";

fn field_id() -> egui::Id {
    egui::Id::new("lumen_options_field")
}

#[derive(Default)]
pub struct Options {
    open: bool,
    search: String,
    /// The option whose schemes the window lists, instead of the rows.
    list: Option<Opt>,
    /// The row (or choice of the list) Up/Down picked: Enter uses it.
    picked: usize,
}

impl Options {
    pub fn open(&mut self) {
        *self = Self { open: true, ..Self::default() };
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn close(&mut self) {
        *self = Self::default();
    }

    /// Enter or a click on the row (or choice) `index`.
    fn use_row(&mut self, schemes: &mut Schemes, index: usize) {
        if let Some(opt) = self.list {
            if let Some(&scheme) = Scheme::ALL.get(index) {
                opt.choose(schemes, scheme);
            }
            return;
        }
        match listed(schemes, &self.search).get(index) {
            Some(Opt::System) => schemes.follow_system = !schemes.follow_system,
            Some(&opt) => {
                self.list = Some(opt);
                self.picked = opt.choice(schemes).and_then(|s| Scheme::ALL.iter().position(|&a| a == s)).unwrap_or(0);
            }
            None => {}
        }
    }

    /// Leaves the list for the rows, with the row it came from picked.
    fn back(&mut self, schemes: &Schemes) {
        if let Some(opt) = self.list.take() {
            self.picked = listed(schemes, &self.search).iter().position(|&o| o == opt).unwrap_or(0);
        }
    }

    /// Draws the window while open; a change to `schemes` is applied at once.
    pub fn show(&mut self, ctx: &egui::Context, schemes: &mut Schemes) {
        if !self.open {
            return;
        }
        let before = *schemes;
        let keys = Keys::read(ctx);
        if keys.escape {
            if self.list.is_some() {
                self.back(schemes);
            } else {
                self.close();
                return;
            }
        } else {
            if keys.up || keys.down {
                // egui moves the focus with the arrows at the end of the frame:
                // cancel that move, or the window would lose the keyboard.
                ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            let count = if self.list.is_some() { Scheme::ALL.len() } else { listed(schemes, &self.search).len() };
            self.picked = self.picked.min(count.saturating_sub(1));
            if count > 0 && keys.down {
                self.picked = (self.picked + 1) % count;
            }
            if count > 0 && keys.up {
                self.picked = (self.picked + count - 1) % count;
            }
            if keys.enter {
                self.use_row(schemes, self.picked);
            }
        }
        let rows = listed(schemes, &self.search);
        let count = if self.list.is_some() { Scheme::ALL.len() } else { rows.len() };
        self.picked = self.picked.min(count.saturating_sub(1));

        let frame = egui::Frame::popup(&ctx.global_style());
        let width = WIDTH.min(ctx.content_rect().width() - frame.total_margin().sum().x).max(0.0);
        let (picked, list) = (self.picked, self.list);
        let search = &mut self.search;
        let mut ask = None;
        let mut typed = false;
        let modal = egui::Modal::new(egui::Id::new("lumen_options")).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            ui.style_mut().interaction.selectable_labels = false;
            match list {
                Some(opt) => {
                    ctx.memory_mut(|m| m.surrender_focus(field_id()));
                    ask = list_of(ui, opt, schemes, picked);
                }
                None => {
                    ui.horizontal(|ui| {
                        crate::ui::icons::magnifier(ui);
                        let field = egui::TextEdit::singleline(search)
                            .id(field_id())
                            .hint_text("Select an option…")
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE)
                            .show(ui);
                        // The search keeps the keyboard while the rows show.
                        if !field.response.has_focus() {
                            field.response.request_focus();
                        }
                        typed = field.response.changed();
                    });
                    ui.separator();
                    for (index, &opt) in rows.iter().enumerate() {
                        if let Some(asked) = option_row(ui, opt, schemes, index, picked == index) {
                            ask = Some(asked);
                        }
                    }
                }
            }
            ui.separator();
            ui.weak(FOOTER);
        });
        if typed {
            self.picked = 0;
        }
        match ask {
            Some(Ask::Use(index)) => {
                self.picked = index;
                self.use_row(schemes, index);
            }
            Some(Ask::Reset(opt)) => opt.restore(schemes),
            Some(Ask::Back) => self.back(schemes),
            None => {}
        }
        if modal.should_close() {
            self.close();
        }
        if *schemes != before {
            schemes.apply(ctx);
        }
    }
}

/// A square symbol that is a button: `hint` names it on hover.
fn symbol_button(ui: &mut egui::Ui, id: egui::Id, symbol: &str, hint: &str) -> egui::Response {
    let galley = egui::WidgetText::from(symbol).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button);
    let side = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let response = ui.interact(rect, id, egui::Sense::click());
    let color = if response.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response.on_hover_text(hint)
}

/// A switch, on or off.
fn switch(ui: &mut egui::Ui, on: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 18.0), egui::Sense::hover());
    let visuals = ui.visuals();
    let fill = if on { visuals.selection.bg_fill } else { visuals.widgets.inactive.bg_fill };
    ui.painter().rect_filled(rect, 9, fill);
    let knob = if on { rect.right() - 9.0 } else { rect.left() + 9.0 };
    ui.painter().circle_filled(egui::pos2(knob, rect.center().y), 6.5, visuals.strong_text_color());
}

/// A row of the window: the name over what it does, the control on the right
/// and, while the option is not at its default, the reset icon after it.
fn option_row(ui: &mut egui::Ui, opt: Opt, schemes: &Schemes, index: usize, picked: bool) -> Option<Ask> {
    let mut ask = None;
    let line = egui::UiBuilder::new().id(egui::Id::new(("lumen_options_row", opt.label()))).sense(egui::Sense::click());
    let row = ui.scope_builder(line, |ui| {
        ui.set_min_width(ui.available_width());
        let fill = if picked { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
        egui::Frame::new().fill(fill).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let width = ui.available_width();
            let text = (width - CONTROL).max(0.0);
            let left = ui.allocate_ui_with_layout(egui::vec2(text, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(text);
                ui.label(egui::RichText::new(opt.label()).strong());
                ui.weak(opt.about());
            });
            let at = left.response.rect;
            let right = egui::Rect::from_min_max(egui::pos2(at.right(), at.top()), egui::pos2(at.left() + width, at.bottom()));
            let controls = egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center));
            ui.scope_builder(controls, |ui| {
                // The reset slot is always taken, so the controls line up.
                if opt.is_default(schemes) {
                    ui.allocate_space(egui::vec2(ui.spacing().interact_size.y, ui.spacing().interact_size.y));
                } else if symbol_button(ui, egui::Id::new(("lumen_options_reset", opt.label())), RESET, "Restore default setting.").clicked() {
                    ask = Some(Ask::Reset(opt));
                }
                match opt.choice(schemes) {
                    Some(scheme) => {
                        ui.label(format!("{} ›", scheme.name()));
                    }
                    None => switch(ui, schemes.follow_system),
                }
            });
        });
    });
    if ask.is_none() && row.response.clicked() {
        ask = Some(Ask::Use(index));
    }
    ask
}

/// The schemes to choose from for `opt`, under a back button and its name.
fn list_of(ui: &mut egui::Ui, opt: Opt, schemes: &Schemes, picked: usize) -> Option<Ask> {
    let mut ask = None;
    ui.horizontal(|ui| {
        if symbol_button(ui, egui::Id::new("lumen_options_back"), BACK, "Back (Escape)").clicked() {
            ask = Some(Ask::Back);
        }
        ui.label(egui::RichText::new(opt.label()).strong());
    });
    ui.separator();
    let current = opt.choice(schemes);
    for (index, scheme) in Scheme::ALL.into_iter().enumerate() {
        let line = egui::UiBuilder::new().id(egui::Id::new(("lumen_options_scheme", scheme.name()))).sense(egui::Sense::click());
        let row = ui.scope_builder(line, |ui| {
            ui.set_min_width(ui.available_width());
            let fill = if picked == index { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
            egui::Frame::new().fill(fill).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(scheme.name());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if current == Some(scheme) {
                            ui.label(CURRENT);
                        }
                    });
                });
            });
        });
        if row.response.clicked() {
            ask = Some(Ask::Use(index));
        }
    }
    ask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing::run;
    use crate::ui::theme::Scheme;
    use eframe::egui::{Event, Key, Modifiers};

    const FOOTER: &str = "Up Down to navigate · Enter to use · Escape to dismiss";
    const RESET: &str = "↺";
    const SYSTEM: &str = "System Light / Dark Scheme";

    type Texts = Vec<(String, egui::Rect)>;

    fn texts_of(shapes: &[egui::epaint::ClippedShape]) -> Texts {
        fn collect(shape: &egui::epaint::Shape, texts: &mut Texts) {
            match shape {
                egui::epaint::Shape::Text(t) => texts.push((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, texts)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        shapes.iter().for_each(|s| collect(&s.shape, &mut texts));
        texts
    }

    fn step(ctx: &egui::Context, options: &mut Options, schemes: &mut Schemes, events: Vec<Event>) -> Texts {
        texts_of(&run(ctx, events, |ui| options.show(ui.ctx(), schemes)).1)
    }

    /// The window opened and drawn, so its rows are laid out and the search has the focus.
    fn opened(schemes: Schemes) -> (egui::Context, Options, Schemes) {
        let ctx = egui::Context::default();
        let mut options = Options::default();
        let mut schemes = schemes;
        options.open();
        step(&ctx, &mut options, &mut schemes, vec![]);
        step(&ctx, &mut options, &mut schemes, vec![]);
        (ctx, options, schemes)
    }

    fn at(texts: &Texts, text: &str) -> egui::Rect {
        texts.iter().find(|(t, _)| t == text).map(|(_, r)| *r).unwrap_or_else(|| panic!("{text:?} not painted: {texts:?}"))
    }

    fn painted(texts: &Texts, text: &str) -> bool {
        texts.iter().any(|(t, _)| t == text)
    }

    fn key(key: Key) -> Vec<Event> {
        vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }, Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]
    }

    fn click_at(pos: egui::Pos2) -> Vec<Event> {
        let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        vec![Event::PointerMoved(pos), button(true), button(false)]
    }

    /// Clicks the text `text` and returns what the next frame paints.
    fn click_on(ctx: &egui::Context, options: &mut Options, schemes: &mut Schemes, text: &str) -> Texts {
        let texts = step(ctx, options, schemes, vec![]);
        step(ctx, options, schemes, click_at(at(&texts, text).center()));
        step(ctx, options, schemes, vec![])
    }

    fn typed(schemes: Schemes, search: &str) -> Texts {
        let (ctx, mut options, mut schemes) = opened(schemes);
        step(&ctx, &mut options, &mut schemes, vec![Event::Text(search.into())]);
        step(&ctx, &mut options, &mut schemes, vec![])
    }

    fn system_on() -> Schemes {
        Schemes { follow_system: true, ..Schemes::default() }
    }

    #[test]
    fn with_the_system_switch_off_the_rows_are_the_switch_and_one_color_scheme() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        let texts = step(&ctx, &mut options, &mut schemes, vec![]);
        for text in [
            "Select an option…",
            SYSTEM,
            "Automatically adapt between light and dark color schemes based on system settings.",
            "Color Scheme",
            "Applies a color palette to all UI elements.",
            "Lumen ›",
            FOOTER,
        ] {
            at(&texts, text);
        }
        assert!(at(&texts, SYSTEM).top() < at(&texts, "Color Scheme").top(), "the switch first");
        assert!(at(&texts, "Automatically adapt between light and dark color schemes based on system settings.").top() > at(&texts, SYSTEM).bottom() - 1.0, "the description under its name");
        for text in ["Light Color Scheme", "Dark Color Scheme"] {
            assert!(!painted(&texts, text), "{text} only with the switch on");
        }
    }

    #[test]
    fn with_the_system_switch_on_the_rows_are_the_switch_and_a_light_and_a_dark_scheme() {
        let (ctx, mut options, mut schemes) = opened(system_on());
        let texts = step(&ctx, &mut options, &mut schemes, vec![]);
        for text in [
            SYSTEM,
            "Light Color Scheme",
            "Color theme applied when the system uses bright colors.",
            "Dark Color Scheme",
            "Color theme applied when the system uses dark colors.",
            FOOTER,
        ] {
            at(&texts, text);
        }
        assert!(!painted(&texts, "Color Scheme"), "one scheme for both only with the switch off");
        let tops: Vec<f32> = [SYSTEM, "Light Color Scheme", "Dark Color Scheme"].iter().map(|t| at(&texts, t).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "in this order: {tops:?}");
    }

    #[test]
    fn the_search_narrows_the_rows_by_their_name_and_by_their_aliases() {
        let shown = |texts: &Texts| [SYSTEM, "Color Scheme"].map(|row| painted(texts, row));
        assert_eq!(shown(&typed(Schemes::default(), "color")), [false, true], "by name");
        assert_eq!(shown(&typed(Schemes::default(), "PALETTE")), [false, true], "by an alias of Color Scheme, any case");
        assert_eq!(shown(&typed(Schemes::default(), "system")), [true, false], "by an alias of the switch");
        assert_eq!(shown(&typed(Schemes::default(), "dark")), [true, false], "another alias of the switch");
        assert_eq!(shown(&typed(Schemes::default(), "theme")), [true, true], "an alias both share");
        let light = typed(system_on(), "light");
        assert_eq!([SYSTEM, "Light Color Scheme", "Dark Color Scheme"].map(|row| painted(&light, row)), [true, true, false]);
    }

    #[test]
    fn a_search_that_matches_nothing_shows_no_rows() {
        let texts = typed(Schemes::default(), "zzz");
        for row in [SYSTEM, "Color Scheme", "Light Color Scheme", "Dark Color Scheme"] {
            assert!(!painted(&texts, row), "{row} does not match");
        }
        assert!(painted(&texts, FOOTER), "the footer stays");
    }

    #[test]
    fn a_color_scheme_row_opens_the_list_marking_the_current_scheme_and_a_choice_applies_at_once() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        let list = click_on(&ctx, &mut options, &mut schemes, "Color Scheme");
        for scheme in Scheme::ALL {
            at(&list, scheme.name());
        }
        assert!(!painted(&list, SYSTEM), "the rows give way to the list");
        let mark = |list: &Texts, scheme: Scheme| (at(list, CURRENT).center().y - at(list, scheme.name()).center().y).abs() < 1.0;
        assert!(mark(&list, Scheme::Lumen), "the current scheme is marked");
        assert_eq!(list.iter().filter(|(t, _)| t == CURRENT).count(), 1, "and only that one");
        let list = click_on(&ctx, &mut options, &mut schemes, "Dracula");
        assert_eq!(schemes.scheme, Scheme::Dracula);
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            assert_eq!(ctx.style_of(theme).visuals, Scheme::Dracula.visuals(theme), "applied to {theme:?} at once");
        }
        assert!(mark(&list, Scheme::Dracula), "the mark follows the choice");
        assert!(options.is_open());
    }

    #[test]
    fn the_light_and_dark_rows_choose_their_own_scheme() {
        let (ctx, mut options, mut schemes) = opened(system_on());
        click_on(&ctx, &mut options, &mut schemes, "Light Color Scheme");
        click_on(&ctx, &mut options, &mut schemes, "Altair");
        assert_eq!((schemes.light, schemes.dark, schemes.scheme), (Scheme::Altair, Scheme::Lumen, Scheme::Lumen));
        assert_eq!(ctx.style_of(egui::Theme::Light).visuals, Scheme::Altair.visuals(egui::Theme::Light));
        key_back(&ctx, &mut options, &mut schemes);
        click_on(&ctx, &mut options, &mut schemes, "Dark Color Scheme");
        click_on(&ctx, &mut options, &mut schemes, "Gentleman");
        assert_eq!((schemes.light, schemes.dark), (Scheme::Altair, Scheme::Gentleman));
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals, Scheme::Gentleman.visuals(egui::Theme::Dark));
    }

    fn key_back(ctx: &egui::Context, options: &mut Options, schemes: &mut Schemes) -> Texts {
        step(ctx, options, schemes, key(Key::Escape));
        step(ctx, options, schemes, vec![])
    }

    #[test]
    fn the_system_switch_toggles_and_the_light_and_dark_schemes_take_over() {
        let chosen = Schemes { scheme: Scheme::Dracula, follow_system: false, light: Scheme::Altair, dark: Scheme::Gentleman };
        let (ctx, mut options, mut schemes) = opened(chosen);
        let rows = click_on(&ctx, &mut options, &mut schemes, SYSTEM);
        assert!(schemes.follow_system);
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals, Scheme::Gentleman.visuals(egui::Theme::Dark), "applied at once");
        assert_eq!(ctx.style_of(egui::Theme::Light).visuals, Scheme::Altair.visuals(egui::Theme::Light));
        assert!(painted(&rows, "Light Color Scheme") && !painted(&rows, "Color Scheme"));
        let rows = click_on(&ctx, &mut options, &mut schemes, SYSTEM);
        assert!(!schemes.follow_system);
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals, Scheme::Dracula.visuals(egui::Theme::Dark));
        assert!(painted(&rows, "Color Scheme") && !painted(&rows, "Light Color Scheme"));
        assert_eq!((schemes.scheme, schemes.light, schemes.dark), (Scheme::Dracula, Scheme::Altair, Scheme::Gentleman), "the other choices are kept");
    }

    #[test]
    fn the_reset_icon_is_on_a_changed_row_only_and_puts_the_default_back() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        assert!(!painted(&step(&ctx, &mut options, &mut schemes, vec![]), RESET), "nothing changed, nothing to reset");
        click_on(&ctx, &mut options, &mut schemes, "Color Scheme");
        click_on(&ctx, &mut options, &mut schemes, "Dracula");
        let rows = key_back(&ctx, &mut options, &mut schemes);
        let icon = at(&rows, RESET);
        assert_eq!(rows.iter().filter(|(t, _)| t == RESET).count(), 1, "on the changed row alone");
        assert!(icon.center().y > at(&rows, "Color Scheme").top() && icon.center().y < at(&rows, "Applies a color palette to all UI elements.").bottom(), "on the Color Scheme row");
        assert!(icon.left() > at(&rows, "Color Scheme").right(), "on its right");
        let rows = click_on(&ctx, &mut options, &mut schemes, RESET);
        assert_eq!(schemes, Schemes::default());
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals, Scheme::Lumen.visuals(egui::Theme::Dark), "Lumen's colors are back at once");
        assert!(!painted(&rows, RESET), "the row is at its default again");
        assert!(options.is_open());
    }

    #[test]
    fn the_reset_icon_of_the_switch_turns_it_off_and_keeps_the_schemes() {
        let chosen = Schemes { scheme: Scheme::Dracula, follow_system: true, light: Scheme::Lumen, dark: Scheme::Lumen };
        let (ctx, mut options, mut schemes) = opened(chosen);
        let rows = step(&ctx, &mut options, &mut schemes, vec![]);
        assert_eq!(rows.iter().filter(|(t, _)| t == RESET).count(), 1, "only the switch differs from its default");
        assert!(at(&rows, RESET).center().y < at(&rows, "Light Color Scheme").top(), "on the switch's row");
        click_on(&ctx, &mut options, &mut schemes, RESET);
        assert_eq!(schemes, Schemes { follow_system: false, ..chosen });
    }

    #[test]
    fn escape_goes_back_from_the_list_and_then_closes_the_window() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        click_on(&ctx, &mut options, &mut schemes, "Color Scheme");
        let rows = key_back(&ctx, &mut options, &mut schemes);
        assert!(options.is_open(), "the first Escape only leaves the list");
        assert!(painted(&rows, SYSTEM) && !painted(&rows, "Dracula"));
        step(&ctx, &mut options, &mut schemes, key(Key::Escape));
        assert!(!options.is_open(), "the second closes");
        assert!(!painted(&step(&ctx, &mut options, &mut schemes, vec![]), FOOTER));
    }

    #[test]
    fn the_back_button_leaves_the_list() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        click_on(&ctx, &mut options, &mut schemes, "Color Scheme");
        let rows = click_on(&ctx, &mut options, &mut schemes, "‹");
        assert!(options.is_open() && painted(&rows, SYSTEM));
    }

    #[test]
    fn the_arrows_pick_rows_and_enter_uses_them() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        // The first row is picked: Enter flips the switch.
        step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert!(schemes.follow_system);
        step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert!(!schemes.follow_system);
        // Down to Color Scheme, Enter opens its list on the current scheme.
        step(&ctx, &mut options, &mut schemes, key(Key::ArrowDown));
        let list = step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert!(painted(&list, "Dracula") && !painted(&list, SYSTEM));
        // Down from Lumen to Regulus, Enter chooses it.
        step(&ctx, &mut options, &mut schemes, key(Key::ArrowDown));
        step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert_eq!(schemes.scheme, Scheme::Regulus);
        assert!(options.is_open(), "the arrows and Enter never closed it");
        // Back on the rows with Color Scheme picked: Up goes to the switch, and Up again wraps to the last row.
        key_back(&ctx, &mut options, &mut schemes);
        step(&ctx, &mut options, &mut schemes, key(Key::ArrowUp));
        step(&ctx, &mut options, &mut schemes, key(Key::ArrowUp));
        let list = step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert!(painted(&list, "Dracula"), "wrapped to Color Scheme, the last row");
        // Five up from Regulus wraps round the list to Gentleman, the last.
        for _ in 0..5 {
            step(&ctx, &mut options, &mut schemes, key(Key::ArrowUp));
        }
        step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert_eq!(schemes.scheme, Scheme::Gentleman);
    }

    #[test]
    fn enter_uses_the_first_row_the_search_left() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        step(&ctx, &mut options, &mut schemes, vec![Event::Text("palette".into())]);
        let list = step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert!(painted(&list, "Dracula"), "Color Scheme was the only row");
        assert!(!schemes.follow_system);
    }

    #[test]
    fn it_uses_up_the_keys_it_reads_and_reads_none_while_closed() {
        let (ctx, mut options, mut schemes) = opened(Schemes::default());
        let mut left = Vec::new();
        let events = [Key::Escape, Key::ArrowDown, Key::ArrowUp, Key::Enter].into_iter().flat_map(key).collect();
        run(&ctx, events, |ui| {
            options.show(ui.ctx(), &mut schemes);
            left = [Key::Escape, Key::ArrowDown, Key::ArrowUp, Key::Enter].map(|k| ui.ctx().input(|i| i.key_pressed(k))).to_vec();
        });
        assert_eq!(left, [false; 4], "left for no one else");
        assert!(!options.is_open());
        let events = [Key::Escape, Key::ArrowDown].into_iter().flat_map(key).collect();
        run(&ctx, events, |ui| {
            options.show(ui.ctx(), &mut schemes);
            left = [Key::Escape, Key::ArrowDown].map(|k| ui.ctx().input(|i| i.key_pressed(k))).to_vec();
        });
        assert_eq!(left, [true; 2], "closed, it takes nothing");
    }
}
