//! The Lumen Setup screen, File Pilot's in Lumen's window: the logo, the
//! version, where Lumen is installed, four switches, then Modify (or
//! Install), Uninstall and Back. What the switches do is the platform's
//! [`System`]; this only shows the machine's state and asks for changes.

use std::path::PathBuf;

use eframe::egui;

use crate::menu::MenuIcon;
use crate::setup::{Origin, Switches, System};
use crate::ui::theme;

/// The column the screen is drawn in, like File Pilot's.
const WIDTH: f32 = 560.0;
/// The size of the logo's side.
const LOGO: f32 = 132.0;
/// The size of the big text buttons (Modify, Uninstall, Back).
const BUTTON_TEXT: f32 = 22.0;

/// What the user asked for, read by [`SetupScreen::handle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupEvent {
    Back,
    /// Apply the switches.
    Modify,
    /// Copy the running Lumen to the folder (a first install or an update).
    Install,
    /// Uninstall was pressed: ask first.
    AskUninstall,
    CancelUninstall,
    Uninstall,
    /// The folder button: show where Lumen is installed.
    OpenFolder,
    /// After uninstalling the running copy: close Lumen.
    Close,
}

/// What the app does after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Back to the folder list.
    Back,
    /// Back to the folder list, in this folder.
    OpenFolder(PathBuf),
    /// Close the window.
    Close,
}

/// The line under the buttons after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Done(String),
    Failed(String),
}

pub struct SetupScreen {
    origin: Origin,
    /// The switches as the machine has them.
    now: Switches,
    /// The switches as shown: Modify makes `now` this.
    pub want: Switches,
    /// The folder Install copies Lumen to, as typed.
    pub dir: String,
    note: Option<Note>,
    asking_uninstall: bool,
    /// The running copy was uninstalled: only Close is left.
    uninstalled: bool,
    logo: Option<egui::TextureHandle>,
}

impl SetupScreen {
    pub fn open(system: &dyn System) -> Self {
        let origin = system.origin();
        let now = system.read();
        let (want, dir) = match &origin {
            Origin::NotInstalled { default_dir } => {
                (Switches::INSTALL, default_dir.display().to_string())
            }
            Origin::Installed { record, .. } => (now, record.dir.display().to_string()),
            Origin::Package { exe } => (now, exe.parent().unwrap_or(exe).display().to_string()),
        };
        Self {
            origin,
            now,
            want,
            dir,
            note: None,
            asking_uninstall: false,
            uninstalled: false,
            logo: None,
        }
    }

    #[cfg(test)]
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    #[cfg(test)]
    pub fn note(&self) -> Option<&Note> {
        self.note.as_ref()
    }

    /// Reads the machine again, keeping the note.
    fn reload(&mut self, system: &dyn System) {
        let note = self.note.take();
        *self = Self {
            note,
            logo: self.logo.take(),
            ..Self::open(system)
        };
    }

    pub fn handle(&mut self, system: &dyn System, event: SetupEvent) -> Option<Outcome> {
        match event {
            SetupEvent::Back => return Some(Outcome::Back),
            SetupEvent::Close => return Some(Outcome::Close),
            SetupEvent::OpenFolder => return Some(Outcome::OpenFolder(PathBuf::from(&self.dir))),
            SetupEvent::AskUninstall => self.asking_uninstall = true,
            SetupEvent::CancelUninstall => self.asking_uninstall = false,
            SetupEvent::Modify => {
                self.note = Some(match system.apply(self.want) {
                    Ok(()) => Note::Done("Changes applied.".into()),
                    Err(err) => Note::Failed(err),
                });
                // After a failure the switches show what the machine has.
                self.reload(system);
            }
            SetupEvent::Install => {
                let dir = match &self.origin {
                    Origin::Installed { record, .. } => record.dir.clone(),
                    _ => PathBuf::from(self.dir.trim()),
                };
                let want = self.want;
                self.note = Some(match system.install(&dir, want) {
                    Ok(()) => Note::Done(format!(
                        "Lumen {} is installed in {}.",
                        env!("CARGO_PKG_VERSION"),
                        dir.display()
                    )),
                    Err(err) => Note::Failed(err),
                });
                let typed = self.dir.clone();
                self.reload(system);
                if matches!(self.origin, Origin::NotInstalled { .. }) {
                    // A failed install keeps what was typed and chosen.
                    (self.dir, self.want) = (typed, want);
                }
            }
            SetupEvent::Uninstall => {
                let running_it = matches!(
                    self.origin,
                    Origin::Installed {
                        running_it: true,
                        ..
                    }
                );
                match system.uninstall() {
                    Ok(()) if running_it => {
                        self.uninstalled = true;
                        self.note = Some(Note::Done(
                            "Lumen was uninstalled; your settings stay. Close Lumen to finish."
                                .into(),
                        ));
                    }
                    Ok(()) => {
                        self.note = Some(Note::Done(
                            "Lumen was uninstalled; your settings stay.".into(),
                        ));
                        self.reload(system);
                    }
                    Err(err) => {
                        self.note = Some(Note::Failed(err));
                        self.reload(system);
                    }
                }
                self.asking_uninstall = false;
            }
        }
        None
    }

    /// Draws the screen; Escape is Back.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<SetupEvent> {
        let mut event = None;
        let escape = !self.uninstalled
            && ui.ctx().memory(|m| m.focused().is_none())
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if escape {
            event = Some(SetupEvent::Back);
        }
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(WIDTH);
                    ui.add_space(16.0);
                    self.header(ui);
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
                    if let Some(e) = self.place(ui) {
                        event = Some(e);
                    }
                    ui.add_space(14.0);
                    self.switches(ui);
                    ui.add_space(18.0);
                    if let Some(e) = self.actions(ui) {
                        event = Some(e);
                    }
                    if let Some(note) = &self.note {
                        ui.add_space(10.0);
                        let (text, color) = match note {
                            Note::Done(text) => (text, ui.visuals().strong_text_color()),
                            Note::Failed(text) => (text, theme::danger(ui.visuals())),
                        };
                        ui.label(egui::RichText::new(text).color(color));
                    }
                    ui.add_space(16.0);
                });
            });
        event
    }

    /// The logo, the name and the version line.
    fn header(&mut self, ui: &mut egui::Ui) {
        let logo = self.logo.get_or_insert_with(|| {
            let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/lumen.png"))
                .expect("assets/lumen.png is a valid PNG");
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            ui.ctx()
                .load_texture("lumen_setup_logo", image, egui::TextureOptions::LINEAR)
        });
        ui.add(egui::Image::new(&*logo).fit_to_exact_size(egui::vec2(LOGO, LOGO)));
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("LUMEN")
                .size(30.0)
                .strong()
                .extra_letter_spacing(2.0),
        );
        ui.add_space(12.0);
        ui.label(egui::RichText::new(version_line(&self.origin, self.uninstalled)).size(16.0));
    }

    /// "Installed Path:", the folder button and the path.
    fn place(&mut self, ui: &mut egui::Ui) -> Option<SetupEvent> {
        let mut event = None;
        let editable = matches!(self.origin, Origin::NotInstalled { .. }) && !self.uninstalled;
        ui.horizontal(|ui| {
            let label = if editable {
                "Install Path:"
            } else {
                "Installed Path:"
            };
            ui.label(egui::RichText::new(label).size(20.0));
            if !editable && !self.uninstalled {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let side = 28.0;
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
                    let color = if response.hovered() {
                        ui.visuals().strong_text_color()
                    } else {
                        ui.visuals().text_color()
                    };
                    crate::ui::icons::menu_icon(
                        ui.painter(),
                        rect.shrink(3.0),
                        MenuIcon::Open,
                        color,
                    );
                    crate::ui::focus_ring(ui, &response);
                    if response.on_hover_text("Open this folder").clicked() {
                        event = Some(SetupEvent::OpenFolder);
                    }
                });
            }
        });
        ui.add_space(4.0);
        let width = ui.available_width();
        if editable {
            ui.add(
                egui::TextEdit::singleline(&mut self.dir)
                    .desired_width(width)
                    .id_salt("lumen_setup_dir"),
            );
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut self.dir.as_str())
                    .desired_width(width)
                    .id_salt("lumen_setup_dir"),
            );
        }
        event
    }

    /// The four switches, two by two.
    fn switches(&mut self, ui: &mut egui::Ui) {
        let off = self.uninstalled;
        let package = matches!(self.origin, Origin::Package { .. });
        // Centred by the width it had last frame: a grid knows its width only once drawn.
        let width_id = egui::Id::new("lumen_setup_switches_width");
        let width = ui.data(|d| d.get_temp::<f32>(width_id)).unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
            let grid = egui::Grid::new("lumen_setup_switches")
                .num_columns(4)
                .spacing([14.0, 12.0])
                .show(ui, |ui| {
                    let pair = |ui: &mut egui::Ui, name: &str, on: &mut bool, enabled: bool| {
                        ui.label(egui::RichText::new(name).size(17.0));
                        ui.add_enabled(enabled, Toggle(on, name));
                    };
                    pair(
                        ui,
                        "Desktop shortcut:",
                        &mut self.want.desktop_shortcut,
                        !off,
                    );
                    pair(ui, "Set as default:", &mut self.want.default, !off);
                    ui.end_row();
                    pair(ui, "Right-click entries:", &mut self.want.right_click, !off);
                    // apt's /usr/bin is always on the PATH.
                    pair(ui, "Add to PATH:", &mut self.want.path, !off && !package);
                    ui.end_row();
                });
            ui.data_mut(|d| d.insert_temp(width_id, grid.response.rect.width()));
        });
    }

    /// Modify or Install, then Uninstall (or apt's note) and Back.
    fn actions(&mut self, ui: &mut egui::Ui) -> Option<SetupEvent> {
        let mut event = None;
        if self.uninstalled {
            if big_button(ui, "Close Lumen", true) {
                event = Some(SetupEvent::Close);
            };
            return event;
        }
        match &self.origin {
            Origin::NotInstalled { .. } => {
                if big_button(ui, "Install", !self.dir.trim().is_empty()) {
                    event = Some(SetupEvent::Install);
                }
            }
            Origin::Installed { record, running_it } => {
                if big_button(ui, "Modify", self.want != self.now) {
                    event = Some(SetupEvent::Modify);
                };
                if !running_it {
                    let text = format!(
                        "Install this copy (v{}) over v{}",
                        env!("CARGO_PKG_VERSION"),
                        record.version
                    );
                    if big_button(ui, &text, true) {
                        event = Some(SetupEvent::Install);
                    };
                }
            }
            Origin::Package { .. } => {
                if big_button(ui, "Modify", self.want != self.now) {
                    event = Some(SetupEvent::Modify);
                }
            }
        }
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        match &self.origin {
            Origin::Installed { .. } if self.asking_uninstall => {
                ui.label("Uninstall Lumen and remove its shortcuts? Your settings stay.");
                let answer = ui
                    .horizontal(|ui| {
                        // Centred: the two buttons' width, around the middle.
                        ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                        let danger = theme::danger(ui.visuals());
                        let yes = egui::Button::new(
                            egui::RichText::new("Uninstall")
                                .size(BUTTON_TEXT)
                                .color(danger),
                        )
                        .frame(false);
                        let yes = ui.add(yes);
                        crate::ui::focus_ring(ui, &yes);
                        ui.add_space(24.0);
                        if yes.clicked() {
                            Some(SetupEvent::Uninstall)
                        } else if big_button(ui, "Cancel", true) {
                            Some(SetupEvent::CancelUninstall)
                        } else {
                            None
                        }
                    })
                    .inner;
                if answer.is_some() {
                    event = answer;
                }
            }
            Origin::Installed { .. } => {
                if big_button(ui, "Uninstall", true) {
                    event = Some(SetupEvent::AskUninstall);
                }
            }
            Origin::Package { .. } => {
                ui.label("Lumen was installed with apt. To uninstall it, run:");
                ui.add(
                    egui::Label::new(egui::RichText::new(APT_REMOVE).monospace()).selectable(true),
                );
            }
            Origin::NotInstalled { .. } => {}
        }
        if big_button(ui, "Back", true) {
            event = Some(SetupEvent::Back);
        };
        event
    }
}

/// A frameless button in big text, like File Pilot's; true when clicked.
fn big_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let button = egui::Button::new(egui::RichText::new(text).size(BUTTON_TEXT)).frame(false);
    let response = ui.add_enabled(enabled, button);
    crate::ui::focus_ring(ui, &response);
    response.clicked()
}

/// The command apt's Lumen is removed with.
pub const APT_REMOVE: &str = "sudo apt remove lumen";

/// "Installed version 0.7.0." and the like.
fn version_line(origin: &Origin, uninstalled: bool) -> String {
    let own = env!("CARGO_PKG_VERSION");
    match origin {
        _ if uninstalled => format!("Lumen {own} was uninstalled."),
        Origin::Installed { record, .. } => format!("Installed version {}.", record.version),
        Origin::Package { .. } => format!("Installed version {own} (apt)."),
        Origin::NotInstalled { .. } => format!("Lumen {own} is not installed."),
    }
}

fn toggle_id(name: &str) -> egui::Id {
    egui::Id::new(("lumen_setup_switch", name))
}

/// A switch like File Pilot's: a pill with a knob, ✓ on the right when on,
/// ✕ on the left when off.
struct Toggle<'a>(&'a mut bool, &'a str);

impl egui::Widget for Toggle<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let Toggle(on, name) = self;
        let size = egui::vec2(52.0, 28.0);
        let rect = ui.allocate_space(size).1;
        let mut response = ui.interact(rect, toggle_id(name), egui::Sense::click());
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, name)
        });
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.visuals();
        let track = if *on {
            theme::accent(visuals)
        } else {
            visuals.widgets.inactive.bg_fill
        };
        let knob = visuals.widgets.active.fg_stroke.color;
        let painter = ui.painter();
        painter.rect_filled(rect, rect.height() / 2.0, track);
        let radius = rect.height() / 2.0 - 4.0;
        let x = egui::lerp(
            (rect.left() + radius + 4.0)..=(rect.right() - radius - 4.0),
            t,
        );
        let center = egui::pos2(x, rect.center().y);
        painter.circle_filled(center, radius, knob);
        let mark = egui::Stroke::new(1.8, track);
        let r = radius * 0.45;
        if *on {
            painter.line(
                vec![
                    center + egui::vec2(-r, 0.0),
                    center + egui::vec2(-r * 0.3, r * 0.8),
                    center + egui::vec2(r, -r * 0.7),
                ],
                mark,
            );
        } else {
            painter.line_segment(
                [center + egui::vec2(-r, -r), center + egui::vec2(r, r)],
                mark,
            );
            painter.line_segment(
                [center + egui::vec2(-r, r), center + egui::vec2(r, -r)],
                mark,
            );
        }
        crate::ui::focus_ring(ui, &response);
        response
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;

    use super::*;
    use crate::setup::Record;

    /// A machine in memory: what it has, and what was asked of it.
    struct Fake {
        origin: RefCell<Origin>,
        has: RefCell<Switches>,
        /// What apply leaves on despite being asked (a failing switch).
        stuck: Option<Switches>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(origin: Origin, has: Switches) -> Self {
            Self {
                origin: RefCell::new(origin),
                has: RefCell::new(has),
                stuck: None,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    fn record(dir: &str) -> Record {
        Record {
            dir: PathBuf::from(dir),
            version: "0.6.0".into(),
        }
    }

    impl System for Fake {
        fn origin(&self) -> Origin {
            self.origin.borrow().clone()
        }
        fn read(&self) -> Switches {
            *self.has.borrow()
        }
        fn apply(&self, want: Switches) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("apply {want:?}"));
            if let Some(stuck) = self.stuck {
                *self.has.borrow_mut() = stuck;
                return Err("Cannot write mimeapps.list".into());
            }
            *self.has.borrow_mut() = want;
            Ok(())
        }
        fn install(&self, dir: &Path, want: Switches) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("install {}", dir.display()));
            if dir.as_os_str().is_empty() || dir.is_relative() {
                return Err(format!("Choose a full path, not {}", dir.display()));
            }
            *self.origin.borrow_mut() = Origin::Installed {
                record: Record {
                    dir: dir.to_path_buf(),
                    version: "0.6.0".into(),
                },
                running_it: false,
            };
            *self.has.borrow_mut() = want;
            Ok(())
        }
        fn uninstall(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("uninstall".into());
            *self.origin.borrow_mut() = Origin::NotInstalled {
                default_dir: PathBuf::from("/home/me/.local/share/lumen"),
            };
            *self.has.borrow_mut() = Switches::default();
            Ok(())
        }
    }

    fn installed(running_it: bool, has: Switches) -> Fake {
        Fake::new(
            Origin::Installed {
                record: record("/home/me/.local/share/lumen"),
                running_it,
            },
            has,
        )
    }

    /// The text of every label and button the screen drew.
    fn texts(screen: &mut SetupScreen) -> String {
        let ctx = egui::Context::default();
        let (_, shapes) = crate::ui::testing::run(&ctx, Vec::new(), |ui| screen.show(ui));
        let mut out = Vec::new();
        for clipped in shapes {
            collect_text(&clipped.shape, &mut out);
        }
        out.join("|")
    }

    fn collect_text(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    #[test]
    fn the_switches_start_as_the_machine_has_them() {
        let has = Switches {
            desktop_shortcut: false,
            default: true,
            right_click: true,
            path: true,
        };
        let screen = SetupScreen::open(&installed(true, has));
        assert_eq!(screen.want, has);
        assert_eq!(screen.dir, "/home/me/.local/share/lumen");
    }

    #[test]
    fn the_installed_screen_shows_file_pilots_lines() {
        let mut screen = SetupScreen::open(&installed(true, Switches::default()));
        let shown = texts(&mut screen);
        for line in [
            "LUMEN",
            "Installed version 0.6.0.",
            "Installed Path:",
            "/home/me/.local/share/lumen",
            "Desktop shortcut:",
            "Set as default:",
            "Right-click entries:",
            "Add to PATH:",
            "Modify",
            "Uninstall",
            "Back",
        ] {
            assert!(
                shown.split('|').any(|t| t == line),
                "{line} missing from {shown}"
            );
        }
        assert!(!shown.contains("Install this copy"), "{shown}");
    }

    #[test]
    fn modify_applies_the_switches_as_shown() {
        let fake = installed(true, Switches::default());
        let mut screen = SetupScreen::open(&fake);
        screen.want.default = true;
        assert_eq!(screen.handle(&fake, SetupEvent::Modify), None);
        assert_eq!(
            fake.read(),
            Switches {
                default: true,
                ..Switches::default()
            }
        );
        assert_eq!(screen.note(), Some(&Note::Done("Changes applied.".into())));
        assert_eq!(screen.want, fake.read());
    }

    #[test]
    fn a_switch_that_failed_shows_what_the_machine_kept() {
        let mut fake = installed(true, Switches::default());
        fake.stuck = Some(Switches {
            right_click: true,
            ..Switches::default()
        });
        let mut screen = SetupScreen::open(&fake);
        screen.want = Switches {
            default: true,
            right_click: true,
            ..Switches::default()
        };
        screen.handle(&fake, SetupEvent::Modify);
        assert_eq!(
            screen.note(),
            Some(&Note::Failed("Cannot write mimeapps.list".into()))
        );
        assert_eq!(
            screen.want,
            Switches {
                right_click: true,
                ..Switches::default()
            }
        );
    }

    #[test]
    fn not_installed_it_offers_install_with_file_pilots_presets() {
        let fake = Fake::new(
            Origin::NotInstalled {
                default_dir: PathBuf::from("/home/me/.local/share/lumen"),
            },
            Switches::default(),
        );
        let mut screen = SetupScreen::open(&fake);
        assert_eq!(
            screen.want,
            Switches {
                desktop_shortcut: false,
                default: true,
                right_click: true,
                path: true
            }
        );
        let shown = texts(&mut screen);
        for line in [
            "Lumen 0.6.0 is not installed.",
            "Install Path:",
            "Install",
            "Back",
        ] {
            assert!(
                shown.split('|').any(|t| t == line),
                "{line} missing from {shown}"
            );
        }
        assert!(
            !shown.split('|').any(|t| t == "Uninstall" || t == "Modify"),
            "{shown}"
        );
    }

    #[test]
    fn install_copies_to_the_folder_typed_and_shows_the_installed_screen() {
        let fake = Fake::new(
            Origin::NotInstalled {
                default_dir: PathBuf::from("/home/me/.local/share/lumen"),
            },
            Switches::default(),
        );
        let mut screen = SetupScreen::open(&fake);
        screen.dir = " /home/me/Apps/Lumen ".into();
        screen.want.desktop_shortcut = true;
        screen.handle(&fake, SetupEvent::Install);
        assert_eq!(*fake.calls.borrow(), ["install /home/me/Apps/Lumen"]);
        assert_eq!(
            fake.read(),
            Switches {
                desktop_shortcut: true,
                ..Switches::INSTALL
            }
        );
        assert_eq!(
            screen.note(),
            Some(&Note::Done(
                "Lumen 0.6.0 is installed in /home/me/Apps/Lumen.".into()
            ))
        );
        assert!(matches!(screen.origin(), Origin::Installed { .. }));
        assert_eq!(screen.want, fake.read());
    }

    #[test]
    fn a_failed_install_keeps_what_was_typed() {
        let fake = Fake::new(
            Origin::NotInstalled {
                default_dir: PathBuf::from("/home/me/.local/share/lumen"),
            },
            Switches::default(),
        );
        let mut screen = SetupScreen::open(&fake);
        screen.dir = "Apps".into();
        screen.want.desktop_shortcut = true;
        screen.handle(&fake, SetupEvent::Install);
        assert_eq!(
            screen.note(),
            Some(&Note::Failed("Choose a full path, not Apps".into()))
        );
        assert_eq!(screen.dir, "Apps");
        assert!(screen.want.desktop_shortcut);
    }

    #[test]
    fn uninstall_asks_first_and_cancel_changes_nothing() {
        let fake = installed(false, Switches::INSTALL);
        let mut screen = SetupScreen::open(&fake);
        screen.handle(&fake, SetupEvent::AskUninstall);
        let shown = texts(&mut screen);
        assert!(
            shown.contains("Uninstall Lumen and remove its shortcuts? Your settings stay."),
            "{shown}"
        );
        screen.handle(&fake, SetupEvent::CancelUninstall);
        assert!(fake.calls.borrow().is_empty());
        assert!(!texts(&mut screen).contains("Uninstall Lumen and remove"));
    }

    #[test]
    fn uninstalling_the_running_copy_leaves_only_close() {
        let fake = installed(true, Switches::INSTALL);
        let mut screen = SetupScreen::open(&fake);
        screen.handle(&fake, SetupEvent::AskUninstall);
        screen.handle(&fake, SetupEvent::Uninstall);
        assert_eq!(*fake.calls.borrow(), ["uninstall"]);
        assert_eq!(
            screen.note(),
            Some(&Note::Done(
                "Lumen was uninstalled; your settings stay. Close Lumen to finish.".into()
            ))
        );
        let shown = texts(&mut screen);
        assert!(shown.split('|').any(|t| t == "Close Lumen"), "{shown}");
        assert!(
            !shown
                .split('|')
                .any(|t| t == "Back" || t == "Install" || t == "Modify"),
            "{shown}"
        );
        assert_eq!(
            screen.handle(&fake, SetupEvent::Close),
            Some(Outcome::Close)
        );
    }

    #[test]
    fn uninstalling_from_another_copy_offers_install_again() {
        let fake = installed(false, Switches::INSTALL);
        let mut screen = SetupScreen::open(&fake);
        screen.handle(&fake, SetupEvent::Uninstall);
        assert!(matches!(screen.origin(), Origin::NotInstalled { .. }));
        assert_eq!(
            screen.note(),
            Some(&Note::Done(
                "Lumen was uninstalled; your settings stay.".into()
            ))
        );
    }

    #[test]
    fn another_copy_can_be_installed_over_the_installed_one() {
        let fake = installed(false, Switches::INSTALL);
        let mut screen = SetupScreen::open(&fake);
        let shown = texts(&mut screen);
        assert!(
            shown.contains("Install this copy (v0.6.0) over v0.6.0"),
            "{shown}"
        );
        screen.handle(&fake, SetupEvent::Install);
        assert_eq!(
            *fake.calls.borrow(),
            ["install /home/me/.local/share/lumen"]
        );
    }

    #[test]
    fn from_apt_it_names_the_remove_command_instead_of_uninstall() {
        let fake = Fake::new(
            Origin::Package {
                exe: PathBuf::from("/usr/bin/lumen"),
            },
            Switches {
                path: true,
                ..Switches::default()
            },
        );
        let mut screen = SetupScreen::open(&fake);
        let shown = texts(&mut screen);
        for line in [
            "Installed version 0.6.0 (apt).",
            "/usr/bin",
            "Lumen was installed with apt. To uninstall it, run:",
            APT_REMOVE,
            "Modify",
            "Back",
        ] {
            assert!(
                shown.split('|').any(|t| t == line),
                "{line} missing from {shown}"
            );
        }
        assert!(
            !shown.split('|').any(|t| t == "Uninstall" || t == "Install"),
            "{shown}"
        );
    }

    #[test]
    fn back_and_the_folder_button_leave_the_screen() {
        let fake = installed(true, Switches::default());
        let mut screen = SetupScreen::open(&fake);
        assert_eq!(screen.handle(&fake, SetupEvent::Back), Some(Outcome::Back));
        assert_eq!(
            screen.handle(&fake, SetupEvent::OpenFolder),
            Some(Outcome::OpenFolder(PathBuf::from(
                "/home/me/.local/share/lumen"
            )))
        );
    }

    #[test]
    fn escape_is_back() {
        let mut screen = SetupScreen::open(&installed(true, Switches::default()));
        let ctx = egui::Context::default();
        // A first frame lays the grid out (egui runs it twice, the second without input).
        crate::ui::testing::frame(&ctx, Vec::new(), |ui| screen.show(ui));
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let event = crate::ui::testing::frame(&ctx, vec![escape], |ui| screen.show(ui));
        assert_eq!(event, Some(SetupEvent::Back));
    }

    /// Frames until the layout stops moving: the grid's sizing pass, then its centring.
    fn settle(ctx: &egui::Context, screen: &mut SetupScreen) {
        for _ in 0..3 {
            crate::ui::testing::frame(ctx, Vec::new(), |ui| screen.show(ui));
        }
    }

    #[test]
    fn clicking_a_switch_flips_it() {
        let mut screen = SetupScreen::open(&installed(true, Switches::default()));
        let ctx = egui::Context::default();
        settle(&ctx, &mut screen);
        let rect = ctx
            .read_response(toggle_id("Desktop shortcut:"))
            .expect("the switch was drawn")
            .rect;
        crate::ui::testing::click(&ctx, rect.center(), |ui| screen.show(ui));
        assert_eq!(
            screen.want,
            Switches {
                desktop_shortcut: true,
                ..Switches::default()
            }
        );
    }

    #[test]
    fn from_apt_add_to_path_cannot_be_turned_off() {
        let fake = Fake::new(
            Origin::Package {
                exe: PathBuf::from("/usr/bin/lumen"),
            },
            Switches {
                path: true,
                ..Switches::default()
            },
        );
        let mut screen = SetupScreen::open(&fake);
        let ctx = egui::Context::default();
        settle(&ctx, &mut screen);
        let rect = ctx
            .read_response(toggle_id("Add to PATH:"))
            .expect("the switch was drawn")
            .rect;
        crate::ui::testing::click(&ctx, rect.center(), |ui| screen.show(ui));
        assert!(screen.want.path);
    }
}
