//! Filesystem layer. Knows nothing about the UI so it can be tested and ported on its own.

pub mod count;
pub mod entry;
pub mod index;
pub mod lister;
pub mod ops;
pub mod preview;
pub mod search;
#[cfg(windows)]
pub mod pdf;
pub mod thumbs;
pub mod transfer;
pub mod watch;

use std::path::{Path, PathBuf};

pub use entry::Entry;
pub use lister::{Lister, Listing, Readers};

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
    if let Some(marker) = crate::places::named_marker(input) {
        return marker;
    }
    let rooted = if cfg!(windows) { with_drive_root(input) } else { None };
    let input = rooted.as_deref().unwrap_or(input);
    let expanded = match (input.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') || rest.starts_with('\\') => {
            home.join(&rest[1..])
        }
        _ => PathBuf::from(input),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else if crate::places::is_marker(current) {
        // No folder to be relative to.
        return current.to_path_buf();
    } else {
        current.join(expanded)
    };
    normalize(&absolute)
}

/// A drive with no root (`C:`, `C:Users`) as from its top (`C:\`,
/// `C:\Users`), like Explorer: Windows reads `C:` alone as the folder the
/// program last used on that drive, not the drive itself.
fn with_drive_root(input: &str) -> Option<String> {
    match input.as_bytes() {
        [letter, b':', rest @ ..] if letter.is_ascii_alphabetic() && !matches!(rest.first(), Some(b'/' | b'\\')) => {
            Some(format!("{}:\\{}", *letter as char, &input[2..]))
        }
        _ => None,
    }
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
    fn this_pc_is_no_folder_so_nothing_resolves_against_it() {
        let pc = crate::places::this_pc();
        assert_eq!(resolve_input_path("docs", &pc, None), pc, "stays where it is");
        assert_eq!(resolve_input_path("../docs", &pc, None), pc);
        assert_eq!(resolve_input_path("", &pc, None), pc);
        let (etc, home) = if cfg!(windows) { (r"C:\Windows", r"C:\Users\u") } else { ("/etc", "/home/u") };
        assert_eq!(resolve_input_path(etc, &pc, None), Path::new(etc), "an absolute path still goes there");
        assert_eq!(resolve_input_path("~/docs", &pc, Some(Path::new(home))), Path::new(home).join("docs"));
        let typed = resolve_input_path("this pc", Path::new("/usr"), None);
        assert_eq!(typed, if cfg!(windows) { pc } else { PathBuf::from("/usr/this pc") }, "its name goes back to it, on Windows");
    }

    #[test]
    fn the_recycle_bin_is_no_folder_either_and_its_name_goes_to_it_on_windows() {
        let bin = crate::places::recycle_bin();
        assert_eq!(resolve_input_path("docs", &bin, None), bin, "stays where it is");
        assert_eq!(resolve_input_path("../docs", &bin, None), bin);
        let typed = resolve_input_path("recycle bin", Path::new("/usr"), None);
        assert_eq!(typed, if cfg!(windows) { bin } else { PathBuf::from("/usr/recycle bin") });
    }

    #[test]
    fn empty_input_keeps_current() {
        assert_eq!(resolve_input_path("", Path::new("/usr"), None), Path::new("/usr"));
    }

    #[test]
    fn a_drive_with_no_root_is_taken_from_its_top() {
        assert_eq!(with_drive_root("C:").as_deref(), Some("C:\\"));
        assert_eq!(with_drive_root("c:Users").as_deref(), Some("c:\\Users"));
        assert_eq!(with_drive_root("C:\\Users"), None, "already from the top");
        assert_eq!(with_drive_root("C:/Users"), None, "already from the top");
        assert_eq!(with_drive_root("docs"), None);
        assert_eq!(with_drive_root("1:"), None, "no drive letter");
    }

    #[cfg(windows)]
    #[test]
    fn a_typed_drive_opens_its_top_not_its_current_folder() {
        let current = Path::new("D:\\work");
        assert_eq!(resolve_input_path("C:", current, None), Path::new("C:\\"));
        assert_eq!(resolve_input_path("C:Users", current, None), Path::new("C:\\Users"));
        assert_eq!(resolve_input_path("C:\\Users", current, None), Path::new("C:\\Users"));
        assert_eq!(resolve_input_path("docs", current, None), Path::new("D:\\work\\docs"));
    }
}
