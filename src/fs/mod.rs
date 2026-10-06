//! Filesystem layer. Knows nothing about the UI so it can be tested and ported on its own.

pub mod entry;
pub mod lister;

use std::path::{Path, PathBuf};

pub use entry::Entry;
pub use lister::{ListError, Lister, Listing};

/// Home directory from the environment (`HOME` on Unix, `USERPROFILE` on Windows).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Turns what the user typed in the path bar into an absolute path.
/// Expands a leading `~` and resolves relative input against `current`.
/// Does not touch the disk: existence is checked by the lister.
pub fn resolve_input_path(input: &str, current: &Path, home: Option<&Path>) -> PathBuf {
    let input = input.trim();
    if input.is_empty() {
        return current.to_path_buf();
    }
    let expanded = match (input.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') || rest.starts_with('\\') => {
            home.join(&rest[1..])
        }
        _ => PathBuf::from(input),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        current.join(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_to_home() {
        let home = Path::new("/home/u");
        assert_eq!(resolve_input_path("~", Path::new("/tmp"), Some(home)), home);
        assert_eq!(
            resolve_input_path("~/docs", Path::new("/tmp"), Some(home)),
            Path::new("/home/u/docs")
        );
    }

    #[test]
    fn tilde_without_slash_is_a_literal_name() {
        let home = Path::new("/home/u");
        assert_eq!(
            resolve_input_path("~backup", Path::new("/tmp"), Some(home)),
            Path::new("/tmp/~backup")
        );
    }

    #[test]
    fn relative_input_joins_current_and_absolute_wins() {
        let cur = Path::new("/usr");
        assert_eq!(resolve_input_path("bin", cur, None), Path::new("/usr/bin"));
        assert_eq!(resolve_input_path("/etc", cur, None), Path::new("/etc"));
        assert_eq!(resolve_input_path("  /etc  ", cur, None), Path::new("/etc"));
    }

    #[test]
    fn empty_input_keeps_current() {
        assert_eq!(resolve_input_path("", Path::new("/usr"), None), Path::new("/usr"));
    }
}
