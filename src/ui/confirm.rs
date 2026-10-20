//! Modal yes/no dialog for destructive operations. Enter and Escape are read
//! by the app (see `input::read_confirm`); this only draws and handles clicks.
//!
//! A trash or delete question also shows what it is about, like File
//! Explorer's: the icon, name, type, dimensions, size and date of one entry,
//! or what several hold.

use std::path::Path;

use eframe::egui;

use crate::format::{format_size, format_time};
use crate::fs::Entry;
use crate::input::Confirm;
use crate::recycle_bin::Recycled;
use crate::ui::icons::{self, IconKind};
use crate::ui::preview::plural;
use crate::ui::theme;

/// Names listed for a removal of several entries before "and N more".
const NAMES_SHOWN: usize = 3;

/// What a trash or delete question is about, read when it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    pub icon: IconKind,
    /// The entry's name, or "3 items".
    pub name: String,
    /// Label and value lines under the name.
    pub facts: Vec<(&'static str, String)>,
}

impl Subject {
    pub fn of(entries: &[&Entry]) -> Self {
        match entries {
            [one] => Self::one(one),
            many => Self::many(many),
        }
    }

    /// Recycled entries, by the names they had and where they were.
    pub fn recycled(items: &[Recycled]) -> Self {
        if let [item] = items {
            let shown = Entry { name: item.name.clone(), path: item.original.clone(), is_dir: item.is_dir, size: None, modified: None, hidden: false };
            let mut facts = vec![("Original Location", item.location.clone())];
            if let Some(deleted) = item.deleted {
                facts.push(("Date Deleted", format_time(deleted)));
            }
            // A folder's record may hold no size.
            if !item.is_dir || item.size > 0 {
                facts.push(("Size", format_size(item.size)));
            }
            return Self { icon: icons::kind_of(&shown), name: item.name.clone(), facts };
        }
        let folders = items.iter().filter(|item| item.is_dir).count() as u64;
        let files = items.len() as u64 - folders;
        let contains = match (files, folders) {
            (0, n) => plural(n, "folder"),
            (n, 0) => plural(n, "file"),
            (f, d) => format!("{}, {}", plural(f, "file"), plural(d, "folder")),
        };
        let mut names = items.iter().take(NAMES_SHOWN).map(|item| item.name.as_str()).collect::<Vec<_>>().join(", ");
        if items.len() > NAMES_SHOWN {
            names += &format!(" and {} more", items.len() - NAMES_SHOWN);
        }
        let icon = if files == 0 { IconKind::Folder } else { IconKind::File };
        Self { icon, name: plural(items.len() as u64, "item"), facts: vec![("Contains", contains), ("Items", names)] }
    }

    fn one(entry: &Entry) -> Self {
        let icon = icons::kind_of(entry);
        let mut facts = vec![("Type", entry.kind())];
        if icon == IconKind::Image
            && let Some((width, height)) = dimensions(&entry.path)
        {
            facts.push(("Dimensions", format!("{width} × {height}")));
        }
        if let Some(size) = entry.size.filter(|_| !entry.is_dir) {
            facts.push(("Size", format_size(size)));
        }
        if let Some(modified) = entry.modified {
            facts.push(("Modified", format_time(modified)));
        }
        Self { icon, name: entry.name.clone(), facts }
    }

    fn many(entries: &[&Entry]) -> Self {
        let folders = entries.iter().filter(|e| e.is_dir).count() as u64;
        let files = entries.len() as u64 - folders;
        let contains = match (files, folders) {
            (0, n) => plural(n, "folder"),
            (n, 0) => plural(n, "file"),
            (f, d) => format!("{}, {}", plural(f, "file"), plural(d, "folder")),
        };
        let mut facts = vec![("Contains", contains)];
        // A folder's size is not known without walking it: only files' sizes add up.
        if folders == 0 {
            let total = entries.iter().filter_map(|e| e.size).sum();
            facts.push(("Size", format_size(total)));
        }
        let mut names = entries.iter().take(NAMES_SHOWN).map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ");
        if entries.len() > NAMES_SHOWN {
            names += &format!(" and {} more", entries.len() - NAMES_SHOWN);
        }
        facts.push(("Items", names));
        let icon = if files == 0 { IconKind::Folder } else { IconKind::File };
        Self { icon, name: plural(entries.len() as u64, "item"), facts }
    }
}

/// An image's width and height, read from its header only.
fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_dimensions().ok()
}

/// Everything the dialog says.
pub struct Prompt<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub subject: Option<&'a Subject>,
    pub hint: &'a str,
    pub yes: &'a str,
    pub no: &'a str,
    /// It cannot be undone: the confirming button and the edge are red.
    pub danger: bool,
}

pub fn show(ctx: &egui::Context, prompt: &Prompt) -> Option<Confirm> {
    let mut answer = None;
    let response = egui::Modal::new(egui::Id::new("lumen_confirm")).show(ctx, |ui| {
        let visuals = ui.visuals().clone();
        let color = if prompt.danger { theme::danger(&visuals) } else { theme::accent(&visuals) };
        ui.set_width(400.0);
        let body = ui.vertical(|ui| {
            ui.label(egui::RichText::new(prompt.title).heading().strong());
            ui.add_space(6.0);
            ui.label(prompt.message);
            if let Some(subject) = prompt.subject {
                ui.add_space(10.0);
                card(ui, subject);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.weak(prompt.hint);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Right to left: the confirming button ends up rightmost.
                    let yes = egui::Button::new(egui::RichText::new(prompt.yes).color(theme::on_accent(&visuals)).strong())
                        .fill(color)
                        .min_size(egui::vec2(96.0, 28.0));
                    if ui.add(yes).clicked() {
                        answer = Some(Confirm::Yes);
                    }
                    if ui.add(egui::Button::new(prompt.no).min_size(egui::vec2(80.0, 28.0))).clicked() {
                        answer = Some(Confirm::No);
                    }
                });
            });
        });
        // A colored edge on the left says which kind of question this is.
        let rect = body.response.rect;
        let edge = egui::Rect::from_min_max(rect.left_top() - egui::vec2(10.0, 0.0), rect.left_bottom() - egui::vec2(7.0, 0.0));
        ui.painter().rect_filled(edge, 1.5, color);
    });
    // Clicking outside the dialog cancels, like Escape.
    if answer.is_none() && response.should_close() {
        answer = Some(Confirm::No);
    }
    answer
}

/// The entry's icon beside its name and facts.
fn card(ui: &mut egui::Ui, subject: &Subject) {
    egui::Frame::new()
        .fill(theme::card(ui.visuals()))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                icons::paint(ui.painter(), rect, subject.icon);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&subject.name).strong()).truncate());
                    egui::Grid::new("lumen_confirm_facts").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                        for (label, value) in &subject.facts {
                            ui.weak(*label);
                            ui.add(egui::Label::new(value).truncate());
                            ui.end_row();
                        }
                    });
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn entry(path: &Path, is_dir: bool) -> Entry {
        let meta = std::fs::metadata(path).unwrap();
        Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_path_buf(),
            is_dir,
            size: (!is_dir).then_some(meta.len()),
            modified: meta.modified().ok(),
            hidden: false,
        }
    }

    fn labels(subject: &Subject) -> Vec<&str> {
        subject.facts.iter().map(|(label, _)| *label).collect()
    }

    fn fact<'a>(subject: &'a Subject, label: &str) -> &'a str {
        &subject.facts.iter().find(|(l, _)| *l == label).expect(label).1
    }

    #[test]
    fn one_image_shows_its_type_dimensions_size_and_date() {
        let dir = crate::fs::ops::tests::fresh_fixture("confirm-image");
        let path = dir.join("cover.png");
        image::RgbaImage::new(3, 2).save(&path).unwrap();
        let subject = Subject::of(&[&entry(&path, false)]);
        assert_eq!((subject.icon, subject.name.as_str()), (IconKind::Image, "cover.png"));
        assert_eq!(labels(&subject), ["Type", "Dimensions", "Size", "Modified"]);
        assert_eq!(fact(&subject, "Type"), "PNG file");
        assert_eq!(fact(&subject, "Dimensions"), "3 × 2");
        assert_eq!(fact(&subject, "Size"), format_size(std::fs::metadata(&path).unwrap().len()));
    }

    #[test]
    fn one_folder_or_unreadable_image_shows_no_size_or_dimensions() {
        let dir = crate::fs::ops::tests::fresh_fixture("confirm-folder");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("broken.png"), "not a png").unwrap();
        let folder = Subject::of(&[&entry(&dir.join("sub"), true)]);
        assert_eq!((folder.icon, labels(&folder)), (IconKind::Folder, vec!["Type", "Modified"]));
        let broken = Subject::of(&[&entry(&dir.join("broken.png"), false)]);
        assert_eq!(labels(&broken), ["Type", "Size", "Modified"]);
    }

    #[test]
    fn several_entries_show_what_they_hold() {
        let at = |name: &str, is_dir, size| Entry {
            name: name.into(),
            path: name.into(),
            is_dir,
            size,
            modified: Some(SystemTime::now()),
            hidden: false,
        };
        let (a, b, sub, c) = (at("a.txt", false, Some(1024)), at("b.txt", false, Some(1024)), at("sub", true, None), at("c.txt", false, Some(0)));
        let files = Subject::of(&[&a, &b]);
        assert_eq!(files.name, "2 items");
        assert_eq!(files.facts, [("Contains", "2 files".into()), ("Size", "2.0 KB".into()), ("Items", "a.txt, b.txt".into())]);
        let mixed = Subject::of(&[&a, &b, &sub, &c]);
        assert_eq!(
            mixed.facts,
            [("Contains", "3 files, 1 folder".into()), ("Items", "a.txt, b.txt, sub and 1 more".into())],
            "no size: a folder's is unknown"
        );
    }

    fn recycled(name: &str, location: &str, is_dir: bool, size: u64, unix: Option<u64>) -> Recycled {
        Recycled {
            info: format!("/bin/$I{name}").into(),
            data: format!("/bin/$R{name}").into(),
            original: Path::new(location).join(name),
            name: name.into(),
            location: location.into(),
            size,
            deleted: unix.map(|s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s)),
            is_dir,
        }
    }

    #[test]
    fn one_recycled_file_shows_where_it_was_when_it_went_and_its_size() {
        let item = recycled("cover.png", "/home/u/Pictures", false, 2048, Some(1_600_000_000));
        let subject = Subject::recycled(std::slice::from_ref(&item));
        assert_eq!((subject.icon, subject.name.as_str()), (IconKind::Image, "cover.png"), "by its extension");
        assert_eq!(labels(&subject), ["Original Location", "Date Deleted", "Size"]);
        assert_eq!(fact(&subject, "Original Location"), "/home/u/Pictures");
        assert_eq!(fact(&subject, "Date Deleted"), format_time(item.deleted.unwrap()));
        assert_eq!(fact(&subject, "Size"), "2.0 KB");
    }

    #[test]
    fn one_recycled_folder_with_no_date_or_size_shows_only_where_it_was() {
        let subject = Subject::recycled(&[recycled("docs", "/home/u", true, 0, None)]);
        assert_eq!((subject.icon, labels(&subject)), (IconKind::Folder, vec!["Original Location"]));
        let sized = Subject::recycled(&[recycled("docs", "/home/u", true, 10, None)]);
        assert_eq!(labels(&sized), ["Original Location", "Size"]);
    }

    #[test]
    fn several_recycled_entries_show_what_they_hold_and_their_names() {
        let items = [
            recycled("a.txt", "/x", false, 1, None),
            recycled("b.txt", "/y", false, 1, None),
            recycled("docs", "/z", true, 0, None),
            recycled("c.txt", "/x", false, 1, None),
            recycled("d.txt", "/x", false, 1, None),
        ];
        let three = Subject::recycled(&items[..3]);
        assert_eq!((three.icon, three.name.as_str()), (IconKind::File, "3 items"));
        assert_eq!(three.facts, [("Contains", "2 files, 1 folder".into()), ("Items", "a.txt, b.txt, docs".into())]);
        assert_eq!(Subject::recycled(&items).facts[1], ("Items", "a.txt, b.txt, docs and 2 more".into()));
        let folders = Subject::recycled(&items[2..3].iter().cloned().cycle().take(2).collect::<Vec<_>>());
        assert_eq!((folders.icon, folders.facts[0].1.as_str()), (IconKind::Folder, "2 folders"));
    }

    /// Every text the dialog painted this frame.
    fn texts(ctx: &egui::Context, prompt: &Prompt) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui.ctx(), prompt);
            });
            // No renderer here: the font atlas upload is dropped on purpose.
            output.textures_delta.clear();
            out = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        out
    }

    #[test]
    fn the_dialog_shows_its_title_message_and_subject() {
        let subject = Subject { icon: IconKind::File, name: "notes.txt".into(), facts: vec![("Size", "1.0 KB".into())] };
        let prompt = Prompt {
            title: "Delete Permanently",
            message: "Permanently delete “notes.txt”? This cannot be undone.",
            subject: Some(&subject),
            hint: "Enter to delete · Esc to cancel",
            yes: "Delete",
            no: "Cancel",
            danger: true,
        };
        let shown = texts(&egui::Context::default(), &prompt);
        for text in ["Delete Permanently", prompt.message, "notes.txt", "Size", "1.0 KB", prompt.hint, "Delete", "Cancel"] {
            assert!(shown.iter().any(|s| s == text), "{text:?} in {shown:?}");
        }
    }
}
