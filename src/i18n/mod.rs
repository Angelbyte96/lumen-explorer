//! The interface's language. Strings stay in English in the code, wrapped in
//! `tr!("…")` or `tr!("…{0}…", value)`; `es` maps them to Spanish. The
//! Options window's Language row picks English, Spanish or the system's.

use std::fmt::Display;

mod es;

/// The Language option: what the settings file keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    /// The system's language when Lumen has it, else English.
    #[default]
    System,
    English,
    Spanish,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::System, Language::English, Language::Spanish];

    /// Its line in the Options window: a language by its own name.
    pub fn name(self) -> &'static str {
        match self {
            Language::System => tr!("System"),
            Language::English => "English",
            Language::Spanish => "Español",
        }
    }

    /// The word the settings file keeps.
    pub fn key(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::English => "english",
            Language::Spanish => "spanish",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.key() == key)
    }

    /// The language the interface shows for this option.
    pub fn resolve(self) -> Lang {
        match self {
            Language::System => system(),
            Language::English => Lang::En,
            Language::Spanish => Lang::Es,
        }
    }

    /// Shows the interface in this option's language from now on.
    pub fn apply(self) {
        set(self.resolve());
    }
}

/// A language the interface is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Es,
}

// The interface is drawn on one thread, but threads Lumen starts write
// messages too: one language for the whole process. Tests run in parallel,
// each on its own thread: there each thread has its own, English at first.
#[cfg(not(test))]
static SPANISH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static SPANISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set(lang: Lang) {
    #[cfg(not(test))]
    SPANISH.store(lang == Lang::Es, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    SPANISH.with(|s| s.set(lang == Lang::Es));
}

pub fn current() -> Lang {
    #[cfg(not(test))]
    let spanish = SPANISH.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    let spanish = SPANISH.with(|s| s.get());
    if spanish { Lang::Es } else { Lang::En }
}

/// `english` in the interface's language. Call it through `tr!` with a
/// literal: the catalog test finds every `tr!("…")` in the sources and checks
/// it has a translation.
pub fn translate(english: &'static str) -> &'static str {
    match current() {
        Lang::En => english,
        Lang::Es => es::get(english).unwrap_or(english),
    }
}

/// `template` with `{0}`, `{1}`… replaced by `values`, in any order.
pub fn fill(template: &str, values: &[&dyn Display]) -> String {
    let mut out = template.to_owned();
    for (index, value) in values.iter().enumerate() {
        out = out.replace(&format!("{{{index}}}"), &value.to_string());
    }
    out
}

/// `tr!("Move {0} items to the trash?", count)`: the translated template filled in.
macro_rules! tr {
    ($english:literal) => {
        $crate::i18n::translate($english)
    };
    ($english:literal, $($value:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::translate($english), &[$(&$value as &dyn ::std::fmt::Display),+])
    };
}
pub(crate) use tr;

/// The system's language: Spanish when the user's is, else English.
fn system() -> Lang {
    if system_tags().iter().any(|tag| is_spanish(tag)) { Lang::Es } else { Lang::En }
}

/// `es`, `es_AR.UTF-8`, `es-MX`…
fn is_spanish(tag: &str) -> bool {
    let tag = tag.trim().to_ascii_lowercase();
    tag == "es" || tag.starts_with("es_") || tag.starts_with("es-") || tag.starts_with("es.")
}

/// The language the user reads Windows in, as a tag.
#[cfg(windows)]
fn system_tags() -> Vec<String> {
    // LANG_SPANISH is the primary language of every Spanish LANGID.
    const LANG_SPANISH: u16 = 0x0a;
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    vec![if id & 0x3ff == LANG_SPANISH { "es".to_owned() } else { "en".to_owned() }]
}

/// The first of $LANGUAGE, $LC_ALL, $LC_MESSAGES and $LANG that is set, as
/// gettext reads them; $LANGUAGE is a list and counts only its first entry.
#[cfg(not(windows))]
fn system_tags() -> Vec<String> {
    let var = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
    from_env(var("LANGUAGE"), var("LC_ALL"), var("LC_MESSAGES"), var("LANG"))
}

#[cfg(not(windows))]
fn from_env(language: Option<String>, all: Option<String>, messages: Option<String>, lang: Option<String>) -> Vec<String> {
    let locale = all.or(messages).or(lang);
    // gettext ignores $LANGUAGE when the locale is C.
    let c = locale.as_deref().is_none_or(|l| l == "C" || l == "POSIX");
    let first = language.filter(|_| !c).and_then(|list| list.split(':').next().map(str::to_owned));
    first.or(locale).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr_is_english_by_default_and_spanish_once_set() {
        assert_eq!(current(), Lang::En);
        assert_eq!(tr!("System"), "System");
        set(Lang::Es);
        assert_eq!(tr!("System"), "Sistema");
        set(Lang::En);
        assert_eq!(tr!("System"), "System");
    }

    #[test]
    fn a_string_without_a_translation_stays_in_english() {
        set(Lang::Es);
        assert_eq!(translate("not in the catalog"), "not in the catalog");
        set(Lang::En);
    }

    #[test]
    fn tr_fills_numbered_values_in_the_translations_order() {
        assert_eq!(tr!("Folder: {0}", "/srv"), "Folder: /srv");
        assert_eq!(fill("{1} then {0}, {1}", &[&"a", &2]), "2 then a, 2");
        set(Lang::Es);
        assert_eq!(tr!("Folder: {0}", "/srv"), "Carpeta: /srv");
        set(Lang::En);
    }

    #[test]
    fn the_language_option_is_kept_as_a_word_and_resolves() {
        for language in Language::ALL {
            assert_eq!(Language::from_key(language.key()), Some(language));
        }
        assert_eq!(Language::from_key("klingon"), None);
        assert_eq!(Language::default(), Language::System);
        assert_eq!((Language::English.resolve(), Language::Spanish.resolve()), (Lang::En, Lang::Es));
        assert_eq!([Language::English.name(), Language::Spanish.name()], ["English", "Español"], "each by its own name");
        Language::Spanish.apply();
        assert_eq!((current(), Language::System.name()), (Lang::Es, "Sistema"));
        Language::English.apply();
        assert_eq!(current(), Lang::En);
    }

    #[test]
    fn spanish_tags_are_spanish_whatever_the_country() {
        for tag in ["es", "es_AR.UTF-8", "es-MX", "ES_es", "es.UTF-8"] {
            assert!(is_spanish(tag), "{tag}");
        }
        for tag in ["en_US.UTF-8", "C", "", "est", "eu_ES"] {
            assert!(!is_spanish(tag), "{tag}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn the_system_language_follows_gettexts_order() {
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(from_env(s("es:en"), s("en_US.UTF-8"), None, None), ["es"], "$LANGUAGE first");
        assert_eq!(from_env(s("es"), s("C"), None, None), ["C"], "not with the C locale");
        assert_eq!(from_env(None, None, s("es_AR.UTF-8"), s("en_US.UTF-8")), ["es_AR.UTF-8"], "$LC_MESSAGES before $LANG");
        assert_eq!(from_env(None, None, None, s("es_MX.UTF-8")), ["es_MX.UTF-8"]);
        assert!(from_env(None, None, None, None).is_empty());
    }

    /// Every `tr!("…")` literal in the sources.
    fn wrapped() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, found: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for literal in literals(&code(&text)) {
                        found.push((path.display().to_string(), literal));
                    }
                }
            }
        }
        let mut found = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        found
    }

    /// `text` without comments and without its tests, which compare with
    /// English strings of their own.
    fn code(text: &str) -> String {
        text.lines()
            .take_while(|line| line.trim() != "mod tests {")
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The string literals right after `tr!(` in `text`, unescaped.
    fn literals(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (at, _) in text.match_indices("tr!(\"") {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = &text[at + 5..];
            let mut literal = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => literal.push('\n'),
                        Some('t') => literal.push('\t'),
                        Some('u') => {
                            let code: String = chars.by_ref().skip(1).take_while(|&c| c != '}').collect();
                            literal.extend(u32::from_str_radix(&code, 16).ok().and_then(char::from_u32));
                        }
                        Some(other) => literal.push(other),
                        None => break,
                    },
                    c => literal.push(c),
                }
            }
            out.push(literal);
        }
        out
    }

    fn placeholders(text: &str) -> Vec<usize> {
        let mut found: Vec<usize> = (0..10).filter(|i| text.contains(&format!("{{{i}}}"))).collect();
        found.sort();
        found
    }

    #[test]
    fn the_catalog_scanner_reads_literals_after_tr_only() {
        let text = r#"tr!("A \"b\"") str!("no") tr("no") x(tr!("C {0}", 1)) tr!("D\u{2026}") ltr!("no")"#;
        assert_eq!(literals(text), ["A \"b\"", "C {0}", "D…"]);
        let file = "/// tr!(\"doc\")\nlet a = tr!(\"code\");\n    // tr!(\"comment\")\n#[cfg(test)]\nmod tests {\n    tr!(\"test\")\n}";
        assert_eq!(literals(&code(file)), ["code"]);
    }

    #[test]
    fn every_wrapped_string_has_a_spanish_translation_with_the_same_values() {
        let missing: Vec<String> = wrapped()
            .into_iter()
            .filter_map(|(file, english)| match es::get(&english) {
                None => Some(format!("{file}: {english:?} has no Spanish entry")),
                Some(spanish) if placeholders(spanish) != placeholders(&english) => Some(format!("{file}: {english:?} → {spanish:?} changes its {{n}} values")),
                Some(_) => None,
            })
            .collect();
        assert!(missing.is_empty(), "{}", missing.join("\n"));
    }

    #[test]
    fn the_catalog_holds_no_unused_or_repeated_entries() {
        let used: std::collections::HashSet<String> = wrapped().into_iter().map(|(_, e)| e).collect();
        let mut seen = std::collections::HashSet::new();
        for (english, _) in es::ENTRIES {
            assert!(seen.insert(*english), "{english:?} twice in the catalog");
            assert!(used.contains(*english), "{english:?} is in the catalog but no tr() uses it");
        }
    }
}
