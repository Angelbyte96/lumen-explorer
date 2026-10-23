//! Dragging entries onto a folder: what a drop there would do, decided
//! before anything is touched. The copy or move itself is a paste.

use std::path::{Path, PathBuf};

use crate::fs::transfer::Mode;

/// The keys held while dragging: Ctrl copies, Shift moves (Ctrl wins when
/// both are held), none leaves it to the disks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keys {
    pub ctrl: bool,
    pub shift: bool,
}

/// Entries being dragged: the paths, in the order the list shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drag {
    pub paths: Vec<PathBuf>,
}

impl Drag {
    /// What dropping on `target` would do, or `None` when it is refused: a
    /// dragged entry itself or a folder below a dragged folder, and a move
    /// into the folder every entry already is in.
    pub fn mode_for(&self, target: &Path, keys: Keys) -> Option<Mode> {
        let first = self.paths.first()?;
        // Component-wise: `/x/sub2` is not below `/x/sub`.
        if self.paths.iter().any(|path| target.starts_with(path)) {
            return None;
        }
        let mode = if keys.ctrl {
            Mode::Copy
        } else if keys.shift || same_disk(first, target) {
            Mode::Move
        } else {
            Mode::Copy
        };
        if mode == Mode::Move && self.paths.iter().all(|path| path.parent() == Some(target)) {
            return None;
        }
        Some(mode)
    }
}

/// `entry` (not followed if a link) and `folder` are on one disk. Unknown
/// counts as another disk: a copy leaves the source where it was.
#[cfg(unix)]
fn same_disk(entry: &Path, folder: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(entry), std::fs::metadata(folder)) {
        (Ok(entry), Ok(folder)) => entry.dev() == folder.dev(),
        _ => false,
    }
}

/// Windows: the same drive letter or network share.
#[cfg(windows)]
fn same_disk(entry: &Path, folder: &Path) -> bool {
    use std::path::Component;
    let prefix = |path: &Path| match path.components().next() {
        Some(Component::Prefix(prefix)) => Some(prefix.as_os_str().to_string_lossy().to_lowercase()),
        _ => None,
    };
    prefix(entry).is_some_and(|p| Some(p) == prefix(folder))
}

#[cfg(not(any(unix, windows)))]
fn same_disk(_entry: &Path, _folder: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    const NONE: Keys = Keys { ctrl: false, shift: false };
    const CTRL: Keys = Keys { ctrl: true, shift: false };
    const SHIFT: Keys = Keys { ctrl: false, shift: true };
    const BOTH: Keys = Keys { ctrl: true, shift: true };

    /// `dir` with `a.txt`, `sub/`, `sub/deep/` and `other/`.
    fn fixture(name: &str) -> PathBuf {
        let dir = fresh_fixture(name);
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::create_dir_all(dir.join("sub").join("deep")).unwrap();
        std::fs::create_dir(dir.join("other")).unwrap();
        dir
    }

    fn drag(paths: &[&Path]) -> Drag {
        Drag { paths: paths.iter().map(|p| p.to_path_buf()).collect() }
    }

    #[test]
    fn on_the_same_disk_a_drop_moves_unless_ctrl_copies() {
        let dir = fixture("drag-same-disk");
        let drag = drag(&[&dir.join("a.txt")]);
        let target = dir.join("other");
        assert_eq!(drag.mode_for(&target, NONE), Some(Mode::Move));
        assert_eq!(drag.mode_for(&target, CTRL), Some(Mode::Copy));
        assert_eq!(drag.mode_for(&target, SHIFT), Some(Mode::Move));
        assert_eq!(drag.mode_for(&target, BOTH), Some(Mode::Copy), "Ctrl wins");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn onto_another_disk_a_drop_copies_unless_shift_moves() {
        // /dev/shm is a tmpfs, never the disk the home folder is on.
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let dir = fixture("drag-other-disk");
        let drag = drag(&[&dir.join("a.txt")]);
        assert_eq!(drag.mode_for(shm, NONE), Some(Mode::Copy));
        assert_eq!(drag.mode_for(shm, SHIFT), Some(Mode::Move));
        assert_eq!(drag.mode_for(shm, CTRL), Some(Mode::Copy));
    }

    #[test]
    fn a_folder_never_takes_itself_or_a_folder_below_it() {
        let dir = fixture("drag-into-itself");
        let drag = drag(&[&dir.join("a.txt"), &dir.join("sub")]);
        for keys in [NONE, CTRL, SHIFT] {
            assert_eq!(drag.mode_for(&dir.join("sub"), keys), None, "itself");
            assert_eq!(drag.mode_for(&dir.join("sub").join("deep"), keys), None, "below it");
        }
        assert_eq!(drag.mode_for(&dir.join("other"), NONE), Some(Mode::Move), "a folder beside it does");
    }

    #[test]
    fn a_name_that_only_starts_like_a_dragged_folder_is_not_below_it() {
        let dir = fixture("drag-prefix");
        std::fs::create_dir(dir.join("sub2")).unwrap();
        let drag = drag(&[&dir.join("sub")]);
        assert_eq!(drag.mode_for(&dir.join("sub2"), NONE), Some(Mode::Move));
    }

    #[test]
    fn a_move_into_the_folder_it_is_in_is_refused_but_a_copy_makes_a_duplicate() {
        let dir = fixture("drag-same-folder");
        let drag = drag(&[&dir.join("a.txt"), &dir.join("other")]);
        assert_eq!(drag.mode_for(&dir, NONE), None);
        assert_eq!(drag.mode_for(&dir, SHIFT), None);
        assert_eq!(drag.mode_for(&dir, CTRL), Some(Mode::Copy), "as Paste: “a (copy).txt”");
    }

    #[test]
    fn entries_from_two_folders_move_when_one_of_them_is_elsewhere() {
        let dir = fixture("drag-two-folders");
        std::fs::write(dir.join("other").join("b.txt"), "B").unwrap();
        let drag = drag(&[&dir.join("a.txt"), &dir.join("other").join("b.txt")]);
        assert_eq!(drag.mode_for(&dir, NONE), Some(Mode::Move), "b.txt moves up, a.txt stays");
    }
}
