//! File operations started by the user. Deleting always goes to the trash.

use std::path::Path;

/// Moves `path` to the trash. Never deletes permanently.
pub fn trash(path: &Path) -> Result<(), String> {
    // `trash` would report a missing file as a generic error; say it plainly.
    if std::fs::symlink_metadata(path).is_err() {
        return Err(format!("No longer exists: {}", path.display()));
    }
    trash::delete(path).map_err(|e| format!("Cannot move {} to trash: {e}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// An empty fixture folder under ~/pruebas-explorador (project safety rule).
    /// Unlike the lister fixtures these are wiped first: the operations change them.
    pub(crate) fn fresh_fixture(name: &str) -> PathBuf {
        let dir = crate::fs::home_dir()
            .expect("HOME must be set")
            .join("pruebas-explorador/test-fixtures/ops")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Removes this fixture's items from the trash so test runs do not pile up there.
    pub(crate) fn purge_trashed_from(dir: &Path) {
        let items: Vec<_> = trash::os_limited::list()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.original_parent == dir)
            .collect();
        let _ = trash::os_limited::purge_all(items);
    }

    fn trashed_names_from(dir: &Path) -> Vec<String> {
        trash::os_limited::list()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.original_parent == dir)
            .map(|item| item.name.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn trash_moves_file_and_folder_to_the_trash() {
        let dir = fresh_fixture("trash");
        std::fs::write(dir.join("old.txt"), "x").unwrap();
        std::fs::create_dir(dir.join("old_dir")).unwrap();

        trash(&dir.join("old.txt")).unwrap();
        trash(&dir.join("old_dir")).unwrap();

        assert!(!dir.join("old.txt").exists());
        assert!(!dir.join("old_dir").exists());
        let mut in_trash = trashed_names_from(&dir);
        in_trash.sort();
        assert_eq!(in_trash, ["old.txt", "old_dir"], "recoverable, not deleted");
        purge_trashed_from(&dir);
    }

    #[test]
    fn trashing_a_missing_path_is_an_error() {
        let dir = fresh_fixture("trash-missing");
        let err = trash(&dir.join("ghost.txt")).unwrap_err();
        assert!(err.contains("ghost.txt"), "error names the item: {err}");
    }
}
