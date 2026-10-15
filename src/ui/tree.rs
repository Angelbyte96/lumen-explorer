//! The Tree view's unfolded folders: which ones are open and what was read
//! of each. Pure data; the app reads the folders on background threads.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::fs::Entry;
use crate::fs::lister::ListError;

/// What an unfolded folder holds, as far as it was read.
#[derive(Debug)]
pub enum Kids {
    /// The read is under way: the folder shows unfolded and empty.
    Loading,
    /// The entries in the lister's order (folders first, by name).
    Read(Vec<Entry>),
    /// The folder could not be read: it shows no children.
    Unreadable,
}

#[derive(Debug)]
struct Open {
    kids: Kids,
    /// The newest read asked for (0: none yet); older results are dropped.
    newest: u64,
    /// A read must be started (it was just unfolded, or its parent was listed again).
    wanted: bool,
}

#[derive(Debug, Default)]
pub struct Tree {
    open: HashMap<PathBuf, Open>,
}

impl Tree {
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    pub fn is_open(&self, folder: &Path) -> bool {
        self.open.contains_key(folder)
    }

    /// The children of an unfolded folder, once read.
    pub fn read(&self, folder: &Path) -> Option<&[Entry]> {
        match &self.open.get(folder)?.kids {
            Kids::Read(entries) => Some(entries),
            _ => None,
        }
    }

    /// Unfolds `folder`; its read is due.
    pub fn unfold(&mut self, folder: PathBuf) {
        self.open.entry(folder).or_insert(Open { kids: Kids::Loading, newest: 0, wanted: true });
    }

    /// Folds `folder` and forgets whatever was unfolded inside it.
    pub fn fold(&mut self, folder: &Path) {
        self.open.retain(|open, _| !open.starts_with(folder));
    }

    /// Folds everything.
    pub fn clear(&mut self) {
        self.open.clear();
    }

    /// Every unfolded folder is read again (what it holds may have changed).
    pub fn refresh(&mut self) {
        for open in self.open.values_mut() {
            open.wanted = true;
        }
    }

    /// The folders whose read is due, marked as asked for.
    pub fn take_wanted(&mut self) -> Vec<PathBuf> {
        let mut due = Vec::new();
        for (folder, open) in &mut self.open {
            if std::mem::take(&mut open.wanted) {
                due.push(folder.clone());
            }
        }
        due.sort();
        due
    }

    /// Notes that read `id` is the newest asked for `folder`.
    pub fn started(&mut self, folder: &Path, id: u64) {
        if let Some(open) = self.open.get_mut(folder) {
            open.newest = open.newest.max(id);
        }
    }

    /// Stores what read `id` found in `folder`; nothing if it was folded
    /// meanwhile or a newer read was asked for since. `true` when stored.
    pub fn set(&mut self, folder: &Path, id: u64, result: Result<Vec<Entry>, ListError>) -> bool {
        let Some(open) = self.open.get_mut(folder).filter(|open| id >= open.newest) else { return false };
        open.kids = result.map_or(Kids::Unreadable, Kids::Read);
        true
    }

    /// Forgets the unfolded folders that are gone: those neither among the
    /// `top` entries nor inside another unfolded folder that is still there.
    pub fn prune(&mut self, top: &[Entry]) {
        let mut frontier: Vec<PathBuf> = top.iter().filter(|e| e.is_dir && self.is_open(&e.path)).map(|e| e.path.clone()).collect();
        let mut kept = HashSet::new();
        while let Some(folder) = frontier.pop() {
            if kept.contains(&folder) {
                continue;
            }
            let inside = self.read(&folder).unwrap_or_default().iter().filter(|e| e.is_dir && self.is_open(&e.path));
            frontier.extend(inside.map(|e| e.path.clone()));
            kept.insert(folder);
        }
        self.open.retain(|folder, _| kept.contains(folder));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(parent: &str, name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from(parent).join(name),
            is_dir,
            size: None,
            modified: None,
            hidden: false,
        }
    }

    #[test]
    fn folding_a_folder_forgets_what_was_unfolded_inside_it_but_not_its_neighbours() {
        let mut tree = Tree::default();
        for folder in ["/x/a", "/x/a/b", "/x/ab", "/x/c"] {
            tree.unfold(folder.into());
        }
        tree.fold(Path::new("/x/a"));
        assert!(!tree.is_open(Path::new("/x/a")) && !tree.is_open(Path::new("/x/a/b")));
        assert!(tree.is_open(Path::new("/x/ab")), "a name that only starts the same is not inside");
        assert!(tree.is_open(Path::new("/x/c")));
    }

    #[test]
    fn pruning_keeps_folders_still_listed_and_drops_the_vanished_with_what_was_inside() {
        let mut tree = Tree::default();
        for folder in ["/x/a", "/x/gone", "/x/a/b", "/x/gone/c"] {
            tree.unfold(folder.into());
        }
        tree.set(Path::new("/x/a"), 1, Ok(vec![entry("/x/a", "b", true)]));
        tree.set(Path::new("/x/gone"), 1, Ok(vec![entry("/x/gone", "c", true)]));
        tree.prune(&[entry("/x", "a", true), entry("/x", "f.txt", false)]);
        assert!(tree.is_open(Path::new("/x/a")) && tree.is_open(Path::new("/x/a/b")));
        assert!(!tree.is_open(Path::new("/x/gone")) && !tree.is_open(Path::new("/x/gone/c")));
    }

    #[test]
    fn a_folder_is_asked_for_once_until_it_is_refreshed() {
        let mut tree = Tree::default();
        tree.unfold("/x/b".into());
        tree.unfold("/x/a".into());
        assert_eq!(tree.take_wanted(), [PathBuf::from("/x/a"), PathBuf::from("/x/b")]);
        assert!(tree.take_wanted().is_empty());
        tree.refresh();
        assert_eq!(tree.take_wanted().len(), 2, "every open folder is read again");
        tree.set(Path::new("/x/a"), 1, Ok(vec![]));
        tree.unfold("/x/a".into());
        assert!(tree.take_wanted().is_empty(), "unfolding an open folder keeps what was read");
        assert_eq!(tree.read(Path::new("/x/a")).map(<[Entry]>::len), Some(0));
    }

    #[test]
    fn a_result_for_a_folder_folded_meanwhile_is_dropped() {
        let mut tree = Tree::default();
        assert!(!tree.set(Path::new("/x/a"), 1, Ok(vec![])));
        assert!(tree.is_empty());
    }
}
