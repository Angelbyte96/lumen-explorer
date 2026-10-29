//! "Open With": the applications installed for a file's type, read the way
//! freedesktop desktops do (shared-mime-info globs, mimeapps.list,
//! mimeinfo.cache, .desktop files), without running anything. Not on
//! Windows: there the shell's own menu offers it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::i18n::tr;

pub use crate::menu::App;

/// Where freedesktop files live, most important first, and the user's language.
pub struct Dirs {
    /// $XDG_CONFIG_HOME, then $XDG_CONFIG_DIRS: mimeapps.list.
    pub config: Vec<PathBuf>,
    /// $XDG_DATA_HOME, then $XDG_DATA_DIRS: mime/, applications/.
    pub data: Vec<PathBuf>,
    /// $LC_ALL, $LC_MESSAGES or $LANG, for the apps' names.
    pub lang: String,
}

impl Dirs {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let home = crate::fs::home_dir().unwrap_or_default();
        let list = |name: &str, default: &str| -> Vec<PathBuf> {
            var(name).unwrap_or_else(|| default.to_owned()).split(':').filter(|d| !d.is_empty()).map(PathBuf::from).collect()
        };
        let mut config = vec![var("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from)];
        config.extend(list("XDG_CONFIG_DIRS", "/etc/xdg"));
        let mut data = vec![var("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)];
        data.extend(list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
        let lang = var("LC_ALL").or_else(|| var("LC_MESSAGES")).or_else(|| var("LANG")).unwrap_or_default();
        Self { config, data, lang }
    }
}

/// The applications that can open an entry called `name`, best first.
pub fn apps_for(dirs: &Dirs, name: &str, is_dir: bool) -> Vec<App> {
    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    let mime = if is_dir {
        "inode/directory".to_owned()
    } else {
        let globs: Vec<Glob> = dirs.data.iter().flat_map(|d| parse_globs2(&read(d.join("mime/globs2")))).collect();
        let Some(mime) = mime_of_name(&globs, name) else { return Vec::new() };
        mime
    };
    let mut subclasses = HashMap::new();
    let mut aliases = HashMap::new();
    for dir in &dirs.data {
        subclasses.extend(parse_pairs(&read(dir.join("mime/subclasses"))));
        aliases.extend(parse_pairs(&read(dir.join("mime/aliases"))));
    }
    let mime = aliases.get(&mime).and_then(|names| names.first()).cloned().unwrap_or(mime);
    let chain = mime_chain(&mime, &subclasses);
    let lists: Vec<MimeApps> = dirs
        .config
        .iter()
        .map(|d| d.join("mimeapps.list"))
        .chain(dirs.data.iter().map(|d| d.join("applications/mimeapps.list")))
        .map(|path| MimeApps::parse(&read(path)))
        .collect();
    let caches: Vec<_> = dirs.data.iter().map(|d| parse_lists(&read(d.join("applications/mimeinfo.cache")), "MIME Cache")).collect();
    app_ids(&chain, &lists, &caches)
        .into_iter()
        .filter_map(|id| {
            // The first data dir holding the file decides, so a user's copy
            // (even a hidden one) overrides the system's.
            let desktop = dirs.data.iter().map(|d| d.join("applications").join(&id)).find(|p| p.is_file())?;
            let name = desktop_name(&read(desktop.clone()), &dirs.lang)?;
            Some(App { name, desktop })
        })
        .fold(Vec::new(), |mut apps: Vec<App>, app| {
            // Two installs of one app (a package and a flatpak) look the same: keep the preferred.
            if !apps.iter().any(|a| a.name == app.name) {
                apps.push(app);
            }
            apps
        })
}

/// Terminals to try, in order: $TERMINAL, then the desktop's choice, then common ones.
pub fn terminals(user: Option<String>) -> Vec<String> {
    let usual = [
        "xdg-terminal-exec",
        "x-terminal-emulator",
        "gnome-terminal",
        "ptyxis",
        "kgx",
        "konsole",
        "xfce4-terminal",
        "kitty",
        "alacritty",
        "foot",
        "xterm",
    ];
    user.filter(|t| !t.is_empty()).into_iter().chain(usual.map(String::from)).collect()
}

/// Opens a terminal in `folder`: the first one installed. Each starts where it is launched.
pub fn open_terminal(folder: &Path) -> Result<(), String> {
    for terminal in terminals(std::env::var("TERMINAL").ok()) {
        match std::process::Command::new(&terminal).current_dir(folder).spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(tr!("Cannot open {0}: {1}", terminal, e)),
        }
    }
    Err(tr!("No terminal found: set $TERMINAL or install one").into())
}

/// Opens `file` with `app`, without waiting for it.
pub fn open_with(app: &App, file: &Path) -> Result<(), String> {
    let mut child = std::process::Command::new("gio")
        .arg("launch")
        .arg(&app.desktop)
        .arg(file)
        .spawn()
        .map_err(|e| tr!("Cannot open with {0}: gio: {1}", app.name, e))?;
    // Reaped in the background so no zombie is left behind.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// One line of shared-mime-info's globs2: `weight:type:pattern[:flags]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    pub weight: u32,
    pub mime: String,
    pub pattern: String,
    pub case_sensitive: bool,
}

pub fn parse_globs2(text: &str) -> Vec<Glob> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.splitn(4, ':');
            let weight = fields.next()?.parse().ok()?;
            let mime = fields.next()?.to_owned();
            let pattern = fields.next()?.to_owned();
            let case_sensitive = fields.next().is_some_and(|flags| flags.split(',').any(|f| f == "cs"));
            Some(Glob { weight, mime, pattern, case_sensitive })
        })
        .collect()
}

/// Shell-style matching: `*` any run, `?` one character, `[...]` one of a set.
fn glob_match(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| glob_match(rest, &name[skip..])),
        Some(('?', rest)) => !name.is_empty() && glob_match(rest, &name[1..]),
        Some(('[', rest)) => {
            let Some(close) = rest.iter().position(|&c| c == ']') else {
                return name.first() == Some(&'[') && glob_match(rest, &name[1..]);
            };
            let Some(&c) = name.first() else { return false };
            let set = &rest[..close];
            let hit = (0..set.len()).any(|i| {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    (set[i]..=set[i + 2]).contains(&c)
                } else {
                    set[i] == c
                }
            });
            hit && glob_match(&rest[close + 1..], &name[1..])
        }
        Some((&p, rest)) => name.first() == Some(&p) && glob_match(rest, &name[1..]),
    }
}

/// The type of a file called `name`: the heaviest matching glob, then a
/// case-sensitive one, then the longest; case is ignored unless the glob says otherwise.
pub fn mime_of_name(globs: &[Glob], name: &str) -> Option<String> {
    let exact: Vec<char> = name.chars().collect();
    let folded: Vec<char> = name.to_lowercase().chars().collect();
    globs
        .iter()
        .filter(|glob| {
            if glob.case_sensitive {
                glob_match(&glob.pattern.chars().collect::<Vec<_>>(), &exact)
            } else {
                glob_match(&glob.pattern.to_lowercase().chars().collect::<Vec<_>>(), &folded)
            }
        })
        // Among equal weights a case-sensitive match beats a folded one ("x.C").
        .max_by_key(|glob| (glob.weight, glob.case_sensitive, glob.pattern.chars().count()))
        .map(|glob| glob.mime.clone())
}

/// `child parent` lines (subclasses) or `alias type` lines (aliases).
pub fn parse_pairs(text: &str) -> HashMap<String, Vec<String>> {
    let mut pairs: HashMap<String, Vec<String>> = HashMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.trim().split_once(' ') {
            pairs.entry(key.to_owned()).or_default().push(value.trim().to_owned());
        }
    }
    pairs
}

/// `mime` and every type it is a kind of, nearest first: its parents from
/// `subclasses`, and text/plain for any text/*.
pub fn mime_chain(mime: &str, subclasses: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut chain = vec![mime.to_owned()];
    let mut next = 0;
    while let Some(current) = chain.get(next).cloned() {
        for parent in subclasses.get(&current).into_iter().flatten() {
            if !chain.contains(parent) {
                chain.push(parent.clone());
            }
        }
        next += 1;
    }
    if mime.starts_with("text/") && !chain.iter().any(|m| m == "text/plain") {
        chain.push("text/plain".into());
    }
    chain
}

/// The `[Section]` of a key=value file, as `key -> desktop ids` (";"-separated).
pub fn parse_lists(text: &str, section: &str) -> HashMap<String, Vec<String>> {
    let mut lists = HashMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name == section;
        } else if inside && let Some((key, value)) = line.split_once('=') {
            let ids = value.split(';').map(str::trim).filter(|id| !id.is_empty()).map(String::from).collect();
            lists.insert(key.trim().to_owned(), ids);
        }
    }
    lists
}

/// One mimeapps.list: the defaults, the associations added and removed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MimeApps {
    pub default: HashMap<String, Vec<String>>,
    pub added: HashMap<String, Vec<String>>,
    pub removed: HashMap<String, Vec<String>>,
}

impl MimeApps {
    pub fn parse(text: &str) -> Self {
        Self {
            default: parse_lists(text, "Default Applications"),
            added: parse_lists(text, "Added Associations"),
            removed: parse_lists(text, "Removed Associations"),
        }
    }
}

/// Desktop ids for `chain`, best first: for each type, the defaults and the
/// added associations of every mimeapps.list (most important first), then the
/// mimeinfo caches; ids a list removes are left out, and none appears twice.
pub fn app_ids(chain: &[String], lists: &[MimeApps], caches: &[HashMap<String, Vec<String>>]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for mime in chain {
        let removed: Vec<&String> = lists.iter().filter_map(|list| list.removed.get(mime)).flatten().collect();
        let defaults = lists.iter().filter_map(|list| list.default.get(mime));
        let added = lists.iter().filter_map(|list| list.added.get(mime));
        let cached = caches.iter().filter_map(|cache| cache.get(mime));
        for id in defaults.chain(added).chain(cached).flatten() {
            if !removed.contains(&id) && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

/// The name a .desktop file shows, in `lang` ("es_MX.UTF-8") when it has one;
/// `None` for anything that is not a visible application.
pub fn desktop_name(text: &str, lang: &str) -> Option<String> {
    let entry = parse_entry(text);
    if entry.get("Type").map(String::as_str) != Some("Application") || entry.get("Hidden").is_some_and(|v| v == "true") {
        return None;
    }
    // "es_MX.UTF-8@euro" -> "es_MX", then "es".
    let locale = lang.split(['.', '@']).next().unwrap_or("");
    let short = locale.split('_').next().unwrap_or("");
    [format!("Name[{locale}]"), format!("Name[{short}]"), "Name".to_owned()]
        .iter()
        .find_map(|key| entry.get(key).cloned())
}

/// The `[Desktop Entry]` group of a .desktop file, key by key.
fn parse_entry(text: &str) -> HashMap<String, String> {
    let mut entry = HashMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
        } else if inside && let Some((key, value)) = line.split_once('=') {
            entry.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBS: &str = "# comment\n\
        50:text/plain:*.txt\n\
        50:text/rust:*.rs\n\
        50:application/x-compressed-tar:*.tar.gz\n\
        50:application/gzip:*.gz\n\
        50:text/x-makefile:Makefile\n\
        50:text/x-csrc:*.C:cs\n\
        50:text/x-c++src:*.c\n\
        10:text/x-readme:README*\n\
        80:image/png:*.png\n";

    fn names(ids: &[String]) -> Vec<&str> {
        ids.iter().map(String::as_str).collect()
    }

    #[test]
    fn globs_are_read_line_by_line_with_their_flags() {
        let globs = parse_globs2(GLOBS);
        assert_eq!(globs.len(), 9, "the comment is skipped");
        assert_eq!(globs[0], Glob { weight: 50, mime: "text/plain".into(), pattern: "*.txt".into(), case_sensitive: false });
        assert!(globs[5].case_sensitive);
    }

    #[test]
    fn a_name_gets_the_type_of_its_best_glob() {
        let globs = parse_globs2(GLOBS);
        let mime = |name| mime_of_name(&globs, name);
        assert_eq!(mime("notes.txt").as_deref(), Some("text/plain"));
        assert_eq!(mime("NOTES.TXT").as_deref(), Some("text/plain"), "case is ignored");
        assert_eq!(mime("a.tar.gz").as_deref(), Some("application/x-compressed-tar"), "the longest wins");
        assert_eq!(mime("Makefile").as_deref(), Some("text/x-makefile"));
        assert_eq!(mime("README.md").as_deref(), Some("text/x-readme"), "wildcards anywhere");
        assert_eq!(mime("x.C").as_deref(), Some("text/x-csrc"), "case-sensitive glob");
        assert_eq!(mime("x.c").as_deref(), Some("text/x-c++src"));
        assert_eq!(mime("photo.PNG").as_deref(), Some("image/png"));
        assert_eq!(mime("noext"), None);
    }

    #[test]
    fn a_type_brings_its_parents_and_text_brings_plain_text() {
        let subclasses = parse_pairs("text/rust text/plain\napplication/x-shellscript application/x-executable\napplication/x-shellscript text/plain\n");
        assert_eq!(mime_chain("text/rust", &subclasses), ["text/rust", "text/plain"]);
        assert_eq!(
            mime_chain("application/x-shellscript", &subclasses),
            ["application/x-shellscript", "application/x-executable", "text/plain"]
        );
        assert_eq!(mime_chain("text/x-readme", &subclasses), ["text/x-readme", "text/plain"]);
        assert_eq!(mime_chain("image/png", &subclasses), ["image/png"]);
    }

    #[test]
    fn mimeapps_lists_have_three_sections() {
        let list = MimeApps::parse(
            "[Default Applications]\ntext/plain=nvim.desktop\n\n\
             [Added Associations]\ntext/plain=code.desktop;gedit.desktop;\n\
             [Removed Associations]\ntext/plain=vim.desktop;\n",
        );
        assert_eq!(names(&list.default["text/plain"]), ["nvim.desktop"]);
        assert_eq!(names(&list.added["text/plain"]), ["code.desktop", "gedit.desktop"]);
        assert_eq!(names(&list.removed["text/plain"]), ["vim.desktop"]);
    }

    #[test]
    fn apps_come_default_first_then_added_then_cached_without_removed_or_twice() {
        let user = MimeApps::parse("[Default Applications]\ntext/rust=code.desktop\n[Removed Associations]\ntext/plain=vim.desktop\n");
        let system = MimeApps::parse("[Default Applications]\ntext/plain=gedit.desktop\n[Added Associations]\ntext/plain=kate.desktop\n");
        let cache = parse_lists("[MIME Cache]\ntext/plain=vim.desktop;gedit.desktop;nano.desktop;\ntext/rust=code.desktop;\n", "MIME Cache");
        let chain = vec!["text/rust".to_owned(), "text/plain".to_owned()];
        assert_eq!(
            names(&app_ids(&chain, &[user, system], &[cache])),
            ["code.desktop", "gedit.desktop", "kate.desktop", "nano.desktop"]
        );
    }

    /// A fake XDG tree: one config dir and two data dirs (user, system).
    fn fake_dirs(name: &str) -> Dirs {
        let root = crate::fs::ops::tests::fresh_fixture(name);
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("system/mime/globs2", "50:text/plain:*.txt\n50:text/rust:*.rs\n");
        write("system/mime/subclasses", "text/rust text/plain\n");
        write("system/mime/aliases", "text/x-rust text/rust\n");
        write("system/applications/mimeinfo.cache", "[MIME Cache]\ntext/plain=gedit.desktop;vim.desktop;gone.desktop;\ninode/directory=nautilus.desktop;\n");
        write("system/applications/gedit.desktop", "[Desktop Entry]\nType=Application\nName=Text Editor\nName[es]=Editor de texto\n");
        write("system/applications/vim.desktop", "[Desktop Entry]\nType=Application\nName=Vim\n");
        write("system/applications/nautilus.desktop", "[Desktop Entry]\nType=Application\nName=Files\n");
        write("user/applications/code.desktop", "[Desktop Entry]\nType=Application\nName=Visual Studio Code\n");
        write("system/applications/com.visualstudio.code.desktop", "[Desktop Entry]\nType=Application\nName=Visual Studio Code\n");
        write("user/applications/vim.desktop", "[Desktop Entry]\nType=Application\nName=Vim\nHidden=true\n");
        write("config/mimeapps.list", "[Default Applications]\ntext/rust=code.desktop\n[Added Associations]\ntext/rust=com.visualstudio.code.desktop\n");
        Dirs {
            config: vec![root.join("config")],
            data: vec![root.join("user"), root.join("system")],
            lang: "es_MX.UTF-8".into(),
        }
    }

    fn app_names(apps: &[App]) -> Vec<&str> {
        apps.iter().map(|app| app.name.as_str()).collect()
    }

    #[test]
    fn the_apps_for_a_file_come_from_every_xdg_dir() {
        let dirs = fake_dirs("apps-xdg");
        let apps = apps_for(&dirs, "main.rs", false);
        assert_eq!(
            app_names(&apps),
            ["Visual Studio Code", "Editor de texto"],
            "a user's hidden copy hides vim; a missing file is skipped; the same name (a flatpak twin) shows once"
        );
        assert_eq!(apps[0].desktop, dirs.data[0].join("applications/code.desktop"), "the user's copy wins");
        assert_eq!(app_names(&apps_for(&dirs, "notes.TXT", false)), ["Editor de texto"]);
        assert_eq!(app_names(&apps_for(&dirs, "photos", true)), ["Files"], "folders are inode/directory");
        assert_eq!(apps_for(&dirs, "unknown.xyz", false), []);
    }

    #[test]
    fn the_terminal_is_the_users_choice_then_the_usual_ones() {
        let list = terminals(Some("kitty".into()));
        assert_eq!(list[..3], ["kitty", "xdg-terminal-exec", "x-terminal-emulator"]);
        assert!(list.contains(&"gnome-terminal".to_owned()) && list.contains(&"xterm".to_owned()));
        assert_eq!(terminals(None)[0], "xdg-terminal-exec");
        assert_eq!(terminals(Some(String::new()))[0], "xdg-terminal-exec");
    }

    #[test]
    fn a_desktop_file_names_itself_in_the_users_language() {
        let text = "[Desktop Entry]\nType=Application\nName=Text Editor\nName[es]=Editor de texto\nName[es_MX]=Editor (MX)\nExec=gedit %U\n\
                    [Desktop Action new]\nName=New Window\n";
        assert_eq!(desktop_name(text, "es_MX.UTF-8").as_deref(), Some("Editor (MX)"));
        assert_eq!(desktop_name(text, "es_AR.UTF-8").as_deref(), Some("Editor de texto"));
        assert_eq!(desktop_name(text, "C").as_deref(), Some("Text Editor"));
        assert_eq!(desktop_name("[Desktop Entry]\nType=Application\nName=X\nHidden=true\n", "C"), None, "deleted by the user");
        assert_eq!(desktop_name("[Desktop Entry]\nType=Link\nName=X\n", "C"), None);
    }
}
