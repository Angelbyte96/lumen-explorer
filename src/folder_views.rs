//! Saved view settings: the view and the order a folder opens with, kept
//! for that folder alone or for it and every folder below it. A folder with
//! no rule opens with the default.

use std::path::{Path, PathBuf};

use crate::app::Sort;
use crate::ui::view_size::ViewSize;

/// What a rule covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// This folder only.
    Folder,
    /// This folder and every folder below it without a rule of its own.
    Tree,
}

/// How a folder is shown: the slider's position and the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    pub view: ViewSize,
    pub sort: Sort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub folder: PathBuf,
    pub scope: Scope,
    pub look: Look,
}

/// Every rule, one per folder, in the order they were first saved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderViews(Vec<Rule>);

impl FolderViews {
    pub fn from_rules(rules: Vec<Rule>) -> Self {
        let mut views = Self::default();
        for rule in rules {
            views.set(&rule.folder, rule.scope, rule.look);
        }
        views
    }

    pub fn rules(&self) -> &[Rule] {
        &self.0
    }

    /// The rule `folder` opens with: its own, else the nearest folder above
    /// it saved with its subfolders.
    pub fn rule_for(&self, folder: &Path) -> Option<&Rule> {
        self.own(folder).or_else(|| {
            folder.ancestors().skip(1).find_map(|above| self.own(above).filter(|rule| rule.scope == Scope::Tree))
        })
    }

    fn own(&self, folder: &Path) -> Option<&Rule> {
        self.0.iter().find(|rule| rule.folder == folder)
    }

    /// Saves `look` for `folder` with `scope`, replacing its own rule.
    pub fn set(&mut self, folder: &Path, scope: Scope, look: Look) {
        match self.0.iter_mut().find(|rule| rule.folder == folder) {
            Some(rule) => (rule.scope, rule.look) = (scope, look),
            None => self.0.push(Rule { folder: folder.to_path_buf(), scope, look }),
        }
    }

    /// Forgets `folder`'s own rule; a rule above it may cover it again.
    pub fn remove(&mut self, folder: &Path) {
        self.0.retain(|rule| rule.folder != folder);
    }

    /// A change of view or order in `folder`: the rule it opens with keeps
    /// it. Returns false when no rule covers `folder`.
    pub fn update(&mut self, folder: &Path, look: Look) -> bool {
        let Some(rule) = self.rule_for(folder) else { return false };
        let (folder, scope) = (rule.folder.clone(), rule.scope);
        self.set(&folder, scope, look);
        true
    }

    /// `disk` with the rules this window added, changed or removed between
    /// `before` and `after`; the other windows' rules are kept.
    pub fn merge(disk: &Self, before: &Self, after: &Self) -> Self {
        let mut merged = disk.clone();
        for rule in &before.0 {
            if after.own(&rule.folder).is_none() {
                merged.remove(&rule.folder);
            }
        }
        for rule in &after.0 {
            if before.own(&rule.folder) != Some(rule) {
                merged.set(&rule.folder, rule.scope, rule.look);
            }
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SortColumn;

    fn look(percent: u8) -> Look {
        Look { view: ViewSize::new(percent), sort: Sort::default() }
    }

    fn rule(folder: &str, scope: Scope, percent: u8) -> Rule {
        Rule { folder: PathBuf::from(folder), scope, look: look(percent) }
    }

    #[test]
    fn a_folder_opens_with_its_own_rule_else_the_nearest_one_saved_with_subfolders() {
        let views = FolderViews::from_rules(vec![
            rule("/a", Scope::Tree, 100),
            rule("/a/b", Scope::Folder, 20),
            rule("/a/b/c", Scope::Tree, 65),
            rule("/x", Scope::Folder, 30),
        ]);
        let percent = |p: &str| views.rule_for(Path::new(p)).map(|r| r.look.view.percent());
        assert_eq!(percent("/a"), Some(100));
        assert_eq!(percent("/a/b"), Some(20), "its own rule wins");
        assert_eq!(percent("/a/b/d"), Some(100), "a folder-only rule does not reach below");
        assert_eq!(percent("/a/b/c/d/e"), Some(65), "the nearest one above");
        assert_eq!(percent("/x"), Some(30));
        assert_eq!(percent("/x/y"), None, "a folder-only rule covers no subfolder");
        assert_eq!(percent("/ab"), None, "a name that only starts the same is not below");
        assert_eq!(percent("/"), None);
    }

    #[test]
    fn set_replaces_the_folders_own_rule_and_remove_lets_the_one_above_cover_it() {
        let mut views = FolderViews::default();
        views.set(Path::new("/a"), Scope::Tree, look(100));
        views.set(Path::new("/a/b"), Scope::Folder, look(20));
        views.set(Path::new("/a/b"), Scope::Tree, look(30));
        assert_eq!(views.rules().len(), 2, "one rule per folder");
        assert_eq!(views.rule_for(Path::new("/a/b/c")).map(|r| r.look.view.percent()), Some(30));
        views.remove(Path::new("/a/b"));
        assert_eq!(views.rule_for(Path::new("/a/b")).map(|r| r.folder.clone()), Some(PathBuf::from("/a")));
        views.remove(Path::new("/nowhere"));
        assert_eq!(views.rules().len(), 1);
    }

    #[test]
    fn a_change_updates_the_rule_the_folder_opens_with_and_nothing_without_one() {
        let mut views = FolderViews::from_rules(vec![rule("/a", Scope::Tree, 100)]);
        let sorted = Look { view: ViewSize::new(65), sort: Sort { column: SortColumn::Size, descending: true } };
        assert!(views.update(Path::new("/a/b"), sorted), "a subfolder changes the rule above it");
        assert_eq!(views.rules(), [Rule { folder: PathBuf::from("/a"), scope: Scope::Tree, look: sorted }]);
        assert!(!views.update(Path::new("/elsewhere"), look(0)));
        assert_eq!(views.rules().len(), 1, "no rule is made by a change");
    }

    #[test]
    fn from_rules_keeps_the_last_rule_of_a_folder_named_twice() {
        let views = FolderViews::from_rules(vec![rule("/a", Scope::Folder, 20), rule("/b", Scope::Folder, 30), rule("/a", Scope::Tree, 65)]);
        assert_eq!(views.rules(), [rule("/a", Scope::Tree, 65), rule("/b", Scope::Folder, 30)]);
    }

    #[test]
    fn merging_keeps_the_other_windows_rules_and_this_windows_edits_win() {
        let before = FolderViews::from_rules(vec![rule("/kept", Scope::Folder, 20), rule("/gone", Scope::Folder, 20), rule("/edited", Scope::Folder, 20)]);
        let after = FolderViews::from_rules(vec![rule("/kept", Scope::Folder, 20), rule("/edited", Scope::Tree, 100), rule("/new", Scope::Folder, 30)]);
        let disk = FolderViews::from_rules(vec![
            rule("/kept", Scope::Folder, 65),
            rule("/gone", Scope::Folder, 20),
            rule("/edited", Scope::Folder, 20),
            rule("/theirs", Scope::Tree, 10),
        ]);
        let merged = FolderViews::merge(&disk, &before, &after);
        assert_eq!(
            merged.rules(),
            [rule("/kept", Scope::Folder, 65), rule("/edited", Scope::Tree, 100), rule("/theirs", Scope::Tree, 10), rule("/new", Scope::Folder, 30)],
            "untouched here: the file's; removed, edited and added here: this window's"
        );
    }
}
