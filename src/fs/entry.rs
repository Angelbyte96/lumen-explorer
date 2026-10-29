use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::i18n::tr;

/// One row in the file list. Built once per listing; the UI never touches the disk.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// `None` for directories and for entries whose metadata could not be read.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
}

impl Entry {
    /// "Folder", "PNG file", or "File" when there is no extension.
    pub fn kind(&self) -> String {
        kind_of(&self.path, self.is_dir)
    }
}

/// "Folder", "PNG file", or "File" when there is no extension, for any path.
pub fn kind_of(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return tr!("Folder").into();
    }
    match path.extension().map(|e| e.to_string_lossy()) {
        Some(ext) if !ext.is_empty() => tr!("{0} file", ext.to_uppercase()),
        _ => tr!("File").into(),
    }
}

/// Dot-files are hidden on Linux and macOS.
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}
