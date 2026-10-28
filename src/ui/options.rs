//! The Options window, like File Pilot's: Ctrl+, (or ☰ ▸ Options) opens it
//! centred on the window. A search box on top narrows the rows by name and by
//! the words that point to them; each row names its option, says what it
//! does and holds its control, with a reset icon while it differs from the
//! default. Up/Down pick a row, Enter uses it, Escape closes (or goes back
//! from a list of choices).

use std::path::Path;

use eframe::egui;

use crate::behavior::{Behavior, CharKey, NewTab, Startup};
use crate::ui::theme::{Scheme, Schemes};

/// Every setting the window edits.
#[derive(Clone, PartialEq, Default)]
struct Prefs {
    schemes: Schemes,
    behavior: Behavior,
}

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
    Overscroll,
    ReverseScroll,
    CharKey,
    Startup,
    NewTab,
}

/// The behavior rows, after the color schemes, in File Pilot's order.
const BEHAVIOR: [Opt; 5] = [Opt::Overscroll, Opt::ReverseScroll, Opt::CharKey, Opt::Startup, Opt::NewTab];

impl Opt {
    fn label(self) -> &'static str {
        match self {
            Opt::System => "System Light / Dark Scheme",
            Opt::Scheme => "Color Scheme",
            Opt::Light => "Light Color Scheme",
            Opt::Dark => "Dark Color Scheme",
            Opt::Overscroll => "Scroll Beyond Last Item",
            Opt::ReverseScroll => "Reverse Scroll Direction",
            Opt::CharKey => "Character Key Action",
            Opt::Startup => "Load On Startup",
            Opt::NewTab => "New Tab Show",
        }
    }

    /// Words the search also finds the row by, lowercase.
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Opt::System => &["theme", "dark", "light", "system"],
            Opt::Scheme | Opt::Light | Opt::Dark => &["theme", "color", "palette"],
            Opt::Overscroll => &["scroll", "overscroll", "space", "end"],
            Opt::ReverseScroll => &["scroll", "wheel", "invert", "natural"],
            Opt::CharKey => &["key", "letter", "type", "filter", "hotkey", "jump"],
            Opt::Startup => &["start", "startup", "launch", "session", "open"],
            Opt::NewTab => &["tab", "open", "root", "home"],
        }
    }

    fn about(self) -> &'static str {
        match self {
            Opt::System => "Automatically adapt between light and dark color schemes based on system settings.",
            Opt::Scheme => "Applies a color palette to all UI elements.",
            Opt::Light => "Color theme applied when the system uses bright colors.",
            Opt::Dark => "Color theme applied when the system uses dark colors.",
            Opt::Overscroll => "Allows scrolling past the last item for extra space.",
            Opt::ReverseScroll => "Reverses the scroll direction in scrollable views.",
            Opt::CharKey => "Determines the action when you press raw character keys without modifiers.",
            Opt::Startup => "Previous session or location to load on startup.",
            Opt::NewTab => "Location to open in a new tab.",
        }
    }

    /// A switch's state; None for a row that opens a list.
    fn switched(self, prefs: &Prefs) -> Option<bool> {
        match self {
            Opt::System => Some(prefs.schemes.follow_system),
            Opt::Overscroll => Some(prefs.behavior.overscroll),
            Opt::ReverseScroll => Some(prefs.behavior.reverse_scroll),
            _ => None,
        }
    }

    fn flip(self, prefs: &mut Prefs) {
        match self {
            Opt::System => prefs.schemes.follow_system = !prefs.schemes.follow_system,
            Opt::Overscroll => prefs.behavior.overscroll = !prefs.behavior.overscroll,
            Opt::ReverseScroll => prefs.behavior.reverse_scroll = !prefs.behavior.reverse_scroll,
            _ => {}
        }
    }

    /// What a row that opens a list offers: the startup folders are the
    /// current tab's (`here`) and the one already chosen, if another.
    fn choices(self, prefs: &Prefs, here: &Path) -> Vec<Choice> {
        match self {
            Opt::Scheme | Opt::Light | Opt::Dark => Scheme::ALL.map(Choice::Scheme).to_vec(),
            Opt::CharKey => CharKey::ALL.map(Choice::CharKey).to_vec(),
            Opt::NewTab => NewTab::ALL.map(Choice::NewTab).to_vec(),
            Opt::Startup => {
                let mut all = vec![Startup::LastSession, Startup::Home, Startup::Folder(here.to_path_buf())];
                if let Startup::Folder(chosen) = &prefs.behavior.startup
                    && chosen != here
                {
                    all.push(Startup::Folder(chosen.clone()));
                }
                all.into_iter().map(Choice::Startup).collect()
            }
            Opt::System | Opt::Overscroll | Opt::ReverseScroll => Vec::new(),
        }
    }

    /// The value a row that opens a list holds.
    fn chosen(self, prefs: &Prefs) -> Option<Choice> {
        match self {
            Opt::Scheme => Some(Choice::Scheme(prefs.schemes.scheme)),
            Opt::Light => Some(Choice::Scheme(prefs.schemes.light)),
            Opt::Dark => Some(Choice::Scheme(prefs.schemes.dark)),
            Opt::CharKey => Some(Choice::CharKey(prefs.behavior.char_key)),
            Opt::Startup => Some(Choice::Startup(prefs.behavior.startup.clone())),
            Opt::NewTab => Some(Choice::NewTab(prefs.behavior.new_tab)),
            Opt::System | Opt::Overscroll | Opt::ReverseScroll => None,
        }
    }

    fn choose(self, prefs: &mut Prefs, choice: Choice) {
        match (self, choice) {
            (Opt::Scheme, Choice::Scheme(s)) => prefs.schemes.scheme = s,
            (Opt::Light, Choice::Scheme(s)) => prefs.schemes.light = s,
            (Opt::Dark, Choice::Scheme(s)) => prefs.schemes.dark = s,
            (Opt::CharKey, Choice::CharKey(c)) => prefs.behavior.char_key = c,
            (Opt::Startup, Choice::Startup(s)) => prefs.behavior.startup = s,
            (Opt::NewTab, Choice::NewTab(n)) => prefs.behavior.new_tab = n,
            _ => {}
        }
    }

    fn is_default(self, prefs: &Prefs) -> bool {
        let mut restored = prefs.clone();
        self.restore(&mut restored);
        restored == *prefs
    }

    /// Puts this option's default back, leaving the others.
    fn restore(self, prefs: &mut Prefs) {
        let default = Prefs::default();
        let (schemes, behavior) = (&mut prefs.schemes, &mut prefs.behavior);
        match self {
            Opt::System => schemes.follow_system = default.schemes.follow_system,
            Opt::Scheme => schemes.scheme = default.schemes.scheme,
            Opt::Light => schemes.light = default.schemes.light,
            Opt::Dark => schemes.dark = default.schemes.dark,
            Opt::Overscroll => behavior.overscroll = default.behavior.overscroll,
            Opt::ReverseScroll => behavior.reverse_scroll = default.behavior.reverse_scroll,
            Opt::CharKey => behavior.char_key = default.behavior.char_key,
            Opt::Startup => behavior.startup = default.behavior.startup,
            Opt::NewTab => behavior.new_tab = default.behavior.new_tab,
        }
    }
}

/// A value in the list a row opens.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    Scheme(Scheme),
    CharKey(CharKey),
    Startup(Startup),
    NewTab(NewTab),
}

impl Choice {
    /// Its line in the list.
    fn name(&self) -> String {
        match self {
            Choice::Scheme(s) => s.name().to_owned(),
            Choice::CharKey(c) => c.name().to_owned(),
            Choice::NewTab(n) => n.name().to_owned(),
            Choice::Startup(Startup::LastSession) => "Last Session".to_owned(),
            Choice::Startup(Startup::Home) => "Home".to_owned(),
            Choice::Startup(Startup::Folder(path)) => format!("Folder: {}", path.display()),
        }
    }

    /// Its short form on the row: a folder by its own name.
    fn short(&self) -> String {
        match self {
            Choice::Startup(Startup::Folder(path)) => folder_name(path),
            other => other.name(),
        }
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The rows the window shows: the options that apply now whose name or an
/// alias holds every word of `search`, in any case.
fn listed(prefs: &Prefs, search: &str) -> Vec<Opt> {
    let schemes: &[Opt] = if prefs.schemes.follow_system { &[Opt::System, Opt::Light, Opt::Dark] } else { &[Opt::System, Opt::Scheme] };
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    schemes
        .iter()
        .chain(BEHAVIOR.iter())
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
    /// Ctrl+R: the picked row (or the listed option) back to its default.
    reset: bool,
}

impl Keys {
    fn read(ctx: &egui::Context) -> Self {
        use egui::{Key, Modifiers};
        ctx.input_mut(|i| Self {
            up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
            down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
            enter: i.consume_key(Modifiers::NONE, Key::Enter),
            escape: i.consume_key(Modifiers::NONE, Key::Escape),
            reset: i.consume_key(Modifiers::COMMAND, Key::R),
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
/// The window's height besides its rows: search box, separators, footer and margins.
const CHROME: f32 = 140.0;
const FOOTER: &str = "Up Down to navigate · Enter to use · Ctrl+R to reset · Escape to dismiss";

fn field_id() -> egui::Id {
    egui::Id::new("lumen_options_field")
}

#[derive(Default)]
pub struct Options {
    open: bool,
    search: String,
    /// The option whose choices the window lists, instead of the rows.
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
    fn use_row(&mut self, prefs: &mut Prefs, here: &Path, index: usize) {
        if let Some(opt) = self.list {
            if let Some(choice) = opt.choices(prefs, here).into_iter().nth(index) {
                opt.choose(prefs, choice);
            }
            return;
        }
        let Some(&opt) = listed(prefs, &self.search).get(index) else { return };
        if opt.switched(prefs).is_some() {
            opt.flip(prefs);
        } else {
            self.list = Some(opt);
            let chosen = opt.chosen(prefs);
            self.picked = opt.choices(prefs, here).iter().position(|c| Some(c) == chosen.as_ref()).unwrap_or(0);
        }
    }

    /// How many rows (or choices of the list) Up/Down go through.
    fn count(&self, prefs: &Prefs, here: &Path) -> usize {
        match self.list {
            Some(opt) => opt.choices(prefs, here).len(),
            None => listed(prefs, &self.search).len(),
        }
    }

    /// Keeps the pick on a row that is there.
    fn clamp_picked(&mut self, prefs: &Prefs, here: &Path) {
        self.picked = self.picked.min(self.count(prefs, here).saturating_sub(1));
    }

    /// Leaves the list for the rows, with the row it came from picked.
    fn back(&mut self, prefs: &Prefs) {
        if let Some(opt) = self.list.take() {
            self.picked = listed(prefs, &self.search).iter().position(|&o| o == opt).unwrap_or(0);
        }
    }

    /// Draws the window while open; a change to `schemes` is applied at once,
    /// one to `behavior` acts from the next frame. `here` is the current tab's
    /// folder, which Load On Startup offers.
    pub fn show(&mut self, ctx: &egui::Context, schemes: &mut Schemes, behavior: &mut Behavior, here: &Path) {
        if !self.open {
            return;
        }
        let mut prefs = Prefs { schemes: *schemes, behavior: behavior.clone() };
        self.frame(ctx, &mut prefs, here);
        if prefs.schemes != *schemes {
            *schemes = prefs.schemes;
            schemes.apply(ctx);
        }
        *behavior = prefs.behavior;
    }

    fn frame(&mut self, ctx: &egui::Context, prefs: &mut Prefs, here: &Path) {
        let keys = Keys::read(ctx);
        if keys.escape {
            if self.list.is_some() {
                self.back(prefs);
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
            self.clamp_picked(prefs, here);
            let count = self.count(prefs, here);
            if count > 0 && keys.down {
                self.picked = (self.picked + 1) % count;
            }
            if count > 0 && keys.up {
                self.picked = (self.picked + count - 1) % count;
            }
            if keys.enter {
                self.use_row(prefs, here, self.picked);
            }
            if keys.reset {
                let opt = self.list.or_else(|| listed(prefs, &self.search).get(self.picked).copied());
                if let Some(opt) = opt {
                    opt.restore(prefs);
                }
            }
        }
        // Enter may have opened or left the list: pick within what shows now.
        self.clamp_picked(prefs, here);
        let rows = listed(prefs, &self.search);

        let frame = egui::Frame::popup(&ctx.global_style());
        let width = WIDTH.min(ctx.content_rect().width() - frame.total_margin().sum().x).max(0.0);
        let (picked, list) = (self.picked, self.list);
        // The arrows bring the row they pick into view; the wheel is left alone.
        let follow = keys.up || keys.down;
        let search = &mut self.search;
        let mut ask = None;
        let mut typed = false;
        let shown: &Prefs = prefs;
        let modal = egui::Modal::new(egui::Id::new("lumen_options")).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            ui.style_mut().interaction.selectable_labels = false;
            match list {
                Some(opt) => {
                    ctx.memory_mut(|m| m.surrender_focus(field_id()));
                    ask = list_of(ui, opt, shown, here, picked);
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
                    // Rows past a short window scroll; the footer stays in view.
                    let room = (ctx.content_rect().height() - CHROME).max(0.0);
                    egui::ScrollArea::vertical().max_height(room).min_scrolled_height(room).show(ui, |ui| {
                        for (index, &opt) in rows.iter().enumerate() {
                            let (asked, row) = option_row(ui, opt, shown, index, picked == index);
                            if follow && picked == index {
                                row.scroll_to_me(None);
                            }
                            if let Some(asked) = asked {
                                ask = Some(asked);
                            }
                        }
                    });
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
                self.use_row(prefs, here, index);
            }
            Some(Ask::Reset(opt)) => opt.restore(prefs),
            Some(Ask::Back) => self.back(prefs),
            None => {}
        }
        if modal.should_close() {
            self.close();
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

/// The switch's track: a pill twice as long as it is high, nearly.
const SWITCH: egui::Vec2 = egui::vec2(34.0, 18.0);
/// The gap between the knob and the track's edge.
const KNOB_GAP: f32 = 2.5;

/// A switch, on or off: the knob rests at the right end when on.
fn switch(ui: &mut egui::Ui, on: bool) {
    let (rect, _) = ui.allocate_exact_size(SWITCH, egui::Sense::hover());
    let visuals = ui.visuals();
    let fill = if on { visuals.selection.bg_fill } else { visuals.widgets.inactive.bg_fill };
    let end = SWITCH.y / 2.0;
    ui.painter().rect_filled(rect, end, fill);
    let knob = if on { rect.right() - end } else { rect.left() + end };
    ui.painter().circle_filled(egui::pos2(knob, rect.center().y), end - KNOB_GAP, visuals.strong_text_color());
}

/// A row of the window: the name over what it does, the control on the right
/// and, while the option is not at its default, the reset icon after it.
/// Returns what a click asked for and the row itself.
fn option_row(ui: &mut egui::Ui, opt: Opt, prefs: &Prefs, index: usize, picked: bool) -> (Option<Ask>, egui::Response) {
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
                if opt.is_default(prefs) {
                    ui.allocate_space(egui::vec2(ui.spacing().interact_size.y, ui.spacing().interact_size.y));
                } else if symbol_button(ui, egui::Id::new(("lumen_options_reset", opt.label())), RESET, "Restore default setting.").clicked() {
                    ask = Some(Ask::Reset(opt));
                }
                match opt.switched(prefs) {
                    Some(on) => switch(ui, on),
                    None => {
                        let shown = opt.chosen(prefs).map_or_else(String::new, |c| c.short());
                        ui.label(format!("{shown} ›"));
                    }
                }
            });
        });
    });
    if ask.is_none() && row.response.clicked() {
        ask = Some(Ask::Use(index));
    }
    (ask, row.response)
}

/// The choices for `opt`, under a back button and its name.
fn list_of(ui: &mut egui::Ui, opt: Opt, prefs: &Prefs, here: &Path, picked: usize) -> Option<Ask> {
    let mut ask = None;
    ui.horizontal(|ui| {
        if symbol_button(ui, egui::Id::new("lumen_options_back"), BACK, "Back (Escape)").clicked() {
            ask = Some(Ask::Back);
        }
        ui.label(egui::RichText::new(opt.label()).strong());
    });
    ui.separator();
    let current = opt.chosen(prefs);
    for (index, choice) in opt.choices(prefs, here).into_iter().enumerate() {
        let name = choice.name();
        let line = egui::UiBuilder::new().id(egui::Id::new(("lumen_options_choice", index))).sense(egui::Sense::click());
        let row = ui.scope_builder(line, |ui| {
            ui.set_min_width(ui.available_width());
            let fill = if picked == index { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
            egui::Frame::new().fill(fill).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if current.as_ref() == Some(&choice) {
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
        texts_of(&run(ctx, events, |ui| options.show(ui.ctx(), schemes, &mut Behavior::default(), Path::new("/"))).1)
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
        assert_eq!(FOOTER, "Up Down to navigate · Enter to use · Ctrl+R to reset · Escape to dismiss", "File Pilot's footer, with Lumen's reset key");
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
        assert!(painted(&list, "Root") && painted(&list, "Current"), "wrapped to New Tab Show, the last row");
        // Back, and down from the last row wraps to the switch.
        key_back(&ctx, &mut options, &mut schemes);
        step(&ctx, &mut options, &mut schemes, key(Key::ArrowDown));
        step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        assert!(schemes.follow_system, "wrapped to the switch");
        // On Color Scheme's list, five up from Regulus wraps round to Gentleman, the last.
        step(&ctx, &mut options, &mut schemes, key(Key::Enter));
        let mut chosen = Schemes { follow_system: false, ..schemes };
        let (ctx, mut options, _) = opened(chosen);
        step(&ctx, &mut options, &mut chosen, key(Key::ArrowDown));
        step(&ctx, &mut options, &mut chosen, key(Key::Enter));
        for _ in 0..5 {
            step(&ctx, &mut options, &mut chosen, key(Key::ArrowUp));
        }
        step(&ctx, &mut options, &mut chosen, key(Key::Enter));
        assert_eq!(chosen.scheme, Scheme::Gentleman);
    }

    /// The window with behavior options as they are kept between frames.
    struct Window {
        ctx: egui::Context,
        options: Options,
        schemes: Schemes,
        behavior: Behavior,
        here: std::path::PathBuf,
    }

    impl Window {
        fn opened(behavior: Behavior) -> Self {
            let mut window = Window { ctx: egui::Context::default(), options: Options::default(), schemes: Schemes::default(), behavior, here: "/srv/work".into() };
            window.options.open();
            window.step(vec![]);
            window.step(vec![]);
            window
        }

        fn step(&mut self, events: Vec<Event>) -> Texts {
            let Window { ctx, options, schemes, behavior, here } = self;
            texts_of(&run(ctx, events, |ui| options.show(ui.ctx(), schemes, behavior, here)).1)
        }

        fn click_on(&mut self, text: &str) -> Texts {
            let texts = self.step(vec![]);
            self.step(click_at(at(&texts, text).center()));
            self.step(vec![])
        }

        fn back(&mut self) -> Texts {
            self.step(key(Key::Escape));
            self.step(vec![])
        }
    }

    const OVERSCROLL: &str = "Scroll Beyond Last Item";
    const REVERSE: &str = "Reverse Scroll Direction";
    const CHAR_KEY: &str = "Character Key Action";
    const STARTUP: &str = "Load On Startup";
    const NEW_TAB: &str = "New Tab Show";

    #[test]
    fn the_behavior_rows_follow_the_color_schemes_with_their_values() {
        let mut window = Window::opened(Behavior::default());
        let texts = window.step(vec![]);
        for text in [
            OVERSCROLL,
            "Allows scrolling past the last item for extra space.",
            REVERSE,
            "Reverses the scroll direction in scrollable views.",
            CHAR_KEY,
            "Determines the action when you press raw character keys without modifiers.",
            "Type In Filter ›",
            STARTUP,
            "Previous session or location to load on startup.",
            "Last Session ›",
            NEW_TAB,
            "Location to open in a new tab.",
            "Current ›",
        ] {
            at(&texts, text);
        }
        let tops: Vec<f32> = ["Color Scheme", OVERSCROLL, REVERSE, CHAR_KEY, STARTUP, NEW_TAB].iter().map(|t| at(&texts, t).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "in File Pilot's order, after the schemes: {tops:?}");
        assert!(!painted(&texts, RESET), "all at their defaults");
    }

    #[test]
    fn the_scroll_switches_flip_and_reset() {
        let mut window = Window::opened(Behavior::default());
        window.click_on(OVERSCROLL);
        assert!(window.behavior.overscroll && !window.behavior.reverse_scroll);
        let rows = window.click_on(REVERSE);
        assert!(window.behavior.overscroll && window.behavior.reverse_scroll);
        assert_eq!(rows.iter().filter(|(t, _)| t == RESET).count(), 2, "both changed rows can be reset");
        window.click_on(OVERSCROLL);
        assert!(!window.behavior.overscroll && window.behavior.reverse_scroll, "a second use flips it back");
        let rows = window.click_on(RESET);
        assert_eq!(window.behavior, Behavior::default());
        assert!(!painted(&rows, RESET));
        assert_eq!(window.schemes, Schemes::default(), "the schemes are left alone");
    }

    #[test]
    fn character_key_action_offers_file_pilots_three_values() {
        let mut window = Window::opened(Behavior::default());
        let list = window.click_on(CHAR_KEY);
        let names = ["Trigger Hotkey", "Type In Filter", "Select Typed Item"];
        let tops: Vec<f32> = names.iter().map(|n| at(&list, n).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "{tops:?}");
        assert!((at(&list, CURRENT).center().y - at(&list, "Type In Filter").center().y).abs() < 1.0, "the default is marked");
        window.click_on("Select Typed Item");
        assert_eq!(window.behavior.char_key, CharKey::SelectTypedItem);
        let rows = window.back();
        at(&rows, "Select Typed Item ›");
        window.click_on(RESET);
        assert_eq!(window.behavior.char_key, CharKey::TypeInFilter);
    }

    fn ctrl(key: Key) -> Vec<Event> {
        vec![
            Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND },
            Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::COMMAND },
        ]
    }

    /// Up or Down until `row` is picked: the rows in order, from the first.
    fn pick(window: &mut Window, row: Opt) {
        let index = listed(&Prefs { schemes: window.schemes, behavior: window.behavior.clone() }, "").iter().position(|&o| o == row).unwrap();
        for _ in 0..index {
            window.step(key(Key::ArrowDown));
        }
    }

    #[test]
    fn ctrl_r_resets_the_picked_row_only() {
        let changed = Behavior { overscroll: true, reverse_scroll: true, char_key: CharKey::SelectTypedItem, ..Behavior::default() };
        let mut window = Window::opened(changed);
        pick(&mut window, Opt::ReverseScroll);
        let rows = window.step(ctrl(Key::R));
        assert!(!window.behavior.reverse_scroll, "the picked row is back at its default");
        assert!(window.behavior.overscroll && window.behavior.char_key == CharKey::SelectTypedItem, "the others are kept");
        assert!(window.options.is_open(), "the window stays");
        assert_eq!(window.options.picked, listed(&Prefs::default(), "").iter().position(|&o| o == Opt::ReverseScroll).unwrap(), "the pick stays");
        assert_eq!(rows.iter().filter(|(t, _)| t == RESET).count(), 2, "{rows:?}");
        window.step(ctrl(Key::R));
        assert!(!window.behavior.reverse_scroll && window.behavior.overscroll, "on a row at its default it changes nothing");
        assert_eq!(window.schemes, Schemes::default());
    }

    #[test]
    fn ctrl_r_in_a_list_resets_its_option_and_moves_the_mark() {
        let mut window = Window::opened(Behavior { char_key: CharKey::SelectTypedItem, ..Behavior::default() });
        window.click_on(CHAR_KEY);
        let list = window.step(ctrl(Key::R));
        assert_eq!(window.behavior.char_key, CharKey::TypeInFilter);
        let list = if painted(&list, CURRENT) { list } else { window.step(vec![]) };
        assert!((at(&list, CURRENT).center().y - at(&list, "Type In Filter").center().y).abs() < 1.0, "the default is marked");
        assert!(painted(&list, "Select Typed Item"), "still in the list");
    }

    #[test]
    fn ctrl_r_with_text_in_the_search_types_nothing() {
        let mut window = Window::opened(Behavior { overscroll: true, ..Behavior::default() });
        window.step(vec![Event::Text("scroll".into())]);
        window.step(ctrl(Key::R));
        assert_eq!(window.options.search, "scroll");
        assert!(!window.behavior.overscroll, "Scroll Beyond Last Item is the first row shown");
    }

    #[test]
    fn new_tab_show_offers_root_home_and_current() {
        let mut window = Window::opened(Behavior::default());
        let list = window.click_on(NEW_TAB);
        let tops: Vec<f32> = ["Root", "Home", "Current"].iter().map(|n| at(&list, n).top()).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "{tops:?}");
        window.click_on("Root");
        assert_eq!(window.behavior.new_tab, NewTab::Root);
        // Enter on Home (one down from Root) chooses it too.
        window.step(key(Key::ArrowDown));
        window.step(key(Key::Enter));
        assert_eq!(window.behavior.new_tab, NewTab::Home);
        at(&window.back(), "Home ›");
    }

    #[test]
    fn load_on_startup_offers_the_last_session_home_and_the_current_folder() {
        let mut window = Window::opened(Behavior::default());
        let list = window.click_on(STARTUP);
        for name in ["Last Session", "Home", "Folder: /srv/work"] {
            at(&list, name);
        }
        window.click_on("Folder: /srv/work");
        assert_eq!(window.behavior.startup, Startup::Folder("/srv/work".into()));
        at(&window.back(), "work ›");
        // From another folder, the chosen one stays offered (and marked) next to it.
        window.here = "/home/u/Music".into();
        let list = window.click_on(STARTUP);
        at(&list, "Folder: /home/u/Music");
        assert!((at(&list, CURRENT).center().y - at(&list, "Folder: /srv/work").center().y).abs() < 1.0, "the chosen folder is marked");
        window.click_on("Home");
        assert_eq!(window.behavior.startup, Startup::Home);
        window.back();
        window.click_on(RESET);
        assert_eq!(window.behavior.startup, Startup::LastSession);
    }

    #[test]
    fn the_search_finds_the_behavior_rows_by_their_aliases() {
        let shown = |search: &str| {
            let mut window = Window::opened(Behavior::default());
            window.step(vec![Event::Text(search.into())]);
            let texts = window.step(vec![]);
            [OVERSCROLL, REVERSE, CHAR_KEY, STARTUP, NEW_TAB, SYSTEM].map(|row| painted(&texts, row))
        };
        assert_eq!(shown("scroll"), [true, true, false, false, false, false]);
        assert_eq!(shown("wheel"), [false, true, false, false, false, false]);
        assert_eq!(shown("letter"), [false, false, true, false, false, false]);
        assert_eq!(shown("session"), [false, false, false, true, false, false]);
        assert_eq!(shown("tab"), [false, false, false, false, true, false]);
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
            options.show(ui.ctx(), &mut schemes, &mut Behavior::default(), Path::new("/"));
            left = [Key::Escape, Key::ArrowDown, Key::ArrowUp, Key::Enter].map(|k| ui.ctx().input(|i| i.key_pressed(k))).to_vec();
        });
        assert_eq!(left, [false; 4], "left for no one else");
        assert!(!options.is_open());
        let events = [Key::Escape, Key::ArrowDown].into_iter().flat_map(key).collect();
        run(&ctx, events, |ui| {
            options.show(ui.ctx(), &mut schemes, &mut Behavior::default(), Path::new("/"));
            left = [Key::Escape, Key::ArrowDown].map(|k| ui.ctx().input(|i| i.key_pressed(k))).to_vec();
        });
        assert_eq!(left, [true; 2], "closed, it takes nothing");
    }
}
