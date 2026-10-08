//! Folders the user keeps coming back to: bookmarks (and, later, places,
//! disks and recent folders). No UI here, so it is unit-testable.

use std::path::{Path, PathBuf};

/// Bookmarked folders, in the order they were added.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Bookmarks {
    paths: Vec<PathBuf>,
}

impl Bookmarks {
    #[allow(dead_code)] // The side panel (Phase 6 T4) lists them.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }

    /// Adds `path`, or removes it when already there. Returns `true` when added.
    pub fn toggle(&mut self, path: &Path) -> bool {
        let before = self.paths.len();
        self.paths.retain(|p| p != path);
        if self.paths.len() < before {
            return false;
        }
        self.paths.push(path.to_path_buf());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_adds_at_the_end_then_removes() {
        let mut b = Bookmarks::default();
        assert!(b.toggle(Path::new("/home/u/Descargas")));
        assert!(b.toggle(Path::new("/srv")));
        assert!(b.contains(Path::new("/srv")));
        assert_eq!(b.paths(), [PathBuf::from("/home/u/Descargas"), PathBuf::from("/srv")]);
        assert!(!b.toggle(Path::new("/home/u/Descargas")), "second toggle removes");
        assert!(!b.contains(Path::new("/home/u/Descargas")));
        assert_eq!(b.paths(), [PathBuf::from("/srv")]);
    }
}
