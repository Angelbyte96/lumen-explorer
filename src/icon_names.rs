//! Which theme icon a list row asks for, as freedesktop file managers pick
//! it: a file's MIME type (from shared-mime-info's globs) gives its icon
//! name, then the type's generic icon, then `<media>-x-generic`; a folder
//! is `folder` unless it is a special one (Downloads: `folder-download`).
//! Not on Windows: there the shell gives every icon.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::apps::{Glob, mime_of_name, parse_globs2, parse_pairs};

/// What a file with no known type is.
const UNKNOWN: &str = "application/octet-stream";

/// shared-mime-info's tables, read once.
pub struct IconNames {
    globs: Vec<Glob>,
    aliases: HashMap<String, Vec<String>>,
    /// mime/icons and mime/generic-icons: type -> icon name.
    icons: HashMap<String, String>,
    generic: HashMap<String, String>,
    /// Special folders and their icon names.
    folders: Vec<(PathBuf, String)>,
}

/// `type:icon` lines (mime/icons, mime/generic-icons).
pub fn parse_icon_map(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.trim().split_once(':'))
        .map(|(mime, icon)| (mime.to_owned(), icon.to_owned()))
        .collect()
}

impl IconNames {
    /// Reads mime/ from each data dir (the first one naming a type wins).
    pub fn read(data: &[PathBuf], folders: Vec<(PathBuf, String)>) -> Self {
        let read = |dir: &PathBuf, file: &str| std::fs::read_to_string(dir.join("mime").join(file)).unwrap_or_default();
        let (mut aliases, mut icons, mut generic) = (HashMap::new(), HashMap::new(), HashMap::new());
        // Later dirs only fill in what earlier ones left out.
        for dir in data.iter().rev() {
            aliases.extend(parse_pairs(&read(dir, "aliases")));
            icons.extend(parse_icon_map(&read(dir, "icons")));
            generic.extend(parse_icon_map(&read(dir, "generic-icons")));
        }
        let globs = data.iter().flat_map(|dir| parse_globs2(&read(dir, "globs2"))).collect();
        Self { globs, aliases, icons, generic, folders }
    }

    /// Icon names for files with extension `ext` (lowercase, "" for none),
    /// best first.
    pub fn for_type(&self, ext: &str) -> Vec<String> {
        let found = (!ext.is_empty()).then(|| mime_of_name(&self.globs, &format!("x.{ext}"))).flatten();
        let mime = found.unwrap_or_else(|| UNKNOWN.to_owned());
        let mime = self.aliases.get(&mime).and_then(|real| real.first()).cloned().unwrap_or(mime);
        let media = mime.split('/').next().unwrap_or_default();
        let mut names = Vec::new();
        names.extend(self.icons.get(&mime).cloned());
        names.push(mime.replace('/', "-"));
        names.extend(self.generic.get(&mime).cloned());
        names.push(format!("{media}-x-generic"));
        let mut seen = Vec::new();
        names.retain(|name| !seen.contains(name) && {
            seen.push(name.clone());
            true
        });
        names
    }

    /// Icon names for the folder at `path`, best first.
    pub fn for_folder(&self, path: &Path) -> Vec<String> {
        let special = self.folders.iter().find(|(folder, _)| folder == path).map(|(_, icon)| icon.clone());
        special.into_iter().chain(["folder".to_owned()]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> IconNames {
        let dir = crate::fs::ops::tests::fresh_fixture(name);
        let (user, system) = (dir.join("user"), dir.join("system"));
        let write = |path: PathBuf, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            system.join("mime/globs2"),
            "# comment\n50:application/pdf:*.pdf\n50:application/vnd.debian.binary-package:*.deb\n50:text/plain:*.txt\n\
             50:application/x-old-zip:*.zip\n50:image/x-rare:*.rare\n50:application/gzip:*.gz\n50:application/x-compressed-tar:*.tar.gz\n",
        );
        write(system.join("mime/aliases"), "application/x-old-zip application/zip\n");
        write(system.join("mime/generic-icons"), "application/vnd.debian.binary-package:package-x-generic\napplication/zip:package-x-generic\n");
        write(system.join("mime/icons"), "application/pdf:system-pdf-icon\n");
        write(user.join("mime/icons"), "application/pdf:my-pdf-icon\n");
        let folders = vec![(PathBuf::from("/home/u"), "user-home".to_owned()), (PathBuf::from("/home/u/Descargas"), "folder-download".to_owned())];
        IconNames::read(&[user, system], folders)
    }

    #[test]
    fn a_type_asks_for_its_own_icon_then_its_generic_ones() {
        let names = fixture("icon-names-types");
        assert_eq!(names.for_type("deb"), ["application-vnd.debian.binary-package", "package-x-generic", "application-x-generic"]);
        assert_eq!(names.for_type("txt"), ["text-plain", "text-x-generic"]);
        assert_eq!(names.for_type("pdf"), ["my-pdf-icon", "application-pdf", "application-x-generic"], "mime/icons first; the user's data dir wins");
        assert_eq!(names.for_type("zip"), ["application-zip", "package-x-generic", "application-x-generic"], "an alias is its real type");
        assert_eq!(names.for_type("rare"), ["image-x-rare", "image-x-generic"]);
        assert_eq!(names.for_type("gz"), ["application-gzip", "application-x-generic"], "only the extension is known");
    }

    #[test]
    fn an_unknown_type_is_a_generic_file() {
        let names = fixture("icon-names-unknown");
        assert_eq!(names.for_type("xyz"), ["application-octet-stream", "application-x-generic"]);
        assert_eq!(names.for_type(""), ["application-octet-stream", "application-x-generic"]);
    }

    #[test]
    fn special_folders_have_their_own_icon() {
        let names = fixture("icon-names-folders");
        assert_eq!(names.for_folder(Path::new("/home/u/Descargas")), ["folder-download", "folder"]);
        assert_eq!(names.for_folder(Path::new("/home/u")), ["user-home", "folder"]);
        assert_eq!(names.for_folder(Path::new("/home/u/Descargas/old")), ["folder"]);
    }

    #[test]
    fn icon_maps_are_type_colon_icon_lines() {
        let map = parse_icon_map("a/b:icon-one\nbad line\nc/d:icon-two\n");
        assert_eq!(map.len(), 2);
        assert_eq!(map["c/d"], "icon-two");
    }
}
