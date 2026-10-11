//! Filesystem layer. Knows nothing about the UI so it can be tested and ported on its own.

pub mod count;
pub mod entry;
pub mod lister;
pub mod ops;
pub mod preview;
#[cfg(windows)]
pub mod pdf;
pub mod transfer;
pub mod watch;

use std::path::{Path, PathBuf};

pub use entry::Entry;
pub use lister::{Lister, Listing};

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
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        current.join(expanded)
    };
    normalize(&absolute)
}

/// Lexically drops `.` and resolves `..` (like Dolphin, without following symlinks),
/// so the shown path is clean and Backspace always goes one level up.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            // `pop` on the root is a no-op, so `/..` stays `/`.
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
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
    fn dot_dot_is_normalized() {
        assert_eq!(resolve_input_path("..", Path::new("/usr/bin"), None), Path::new("/usr"));
        assert_eq!(
            resolve_input_path("../lib", Path::new("/usr/bin"), None),
            Path::new("/usr/lib")
        );
        assert_eq!(resolve_input_path("/usr/./bin/", Path::new("/"), None), Path::new("/usr/bin"));
        assert_eq!(resolve_input_path("/..", Path::new("/"), None), Path::new("/"));
        let home = Path::new("/home/u");
        assert_eq!(resolve_input_path("~/..", Path::new("/tmp"), Some(home)), Path::new("/home"));
    }

    #[test]
    fn empty_input_keeps_current() {
        assert_eq!(resolve_input_path("", Path::new("/usr"), None), Path::new("/usr"));
    }
}
