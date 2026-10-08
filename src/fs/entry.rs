use std::path::PathBuf;
use std::time::SystemTime;

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
        if self.is_dir {
            return "Folder".into();
        }
        match self.path.extension().map(|e| e.to_string_lossy()) {
            Some(ext) if !ext.is_empty() => format!("{} file", ext.to_uppercase()),
            _ => "File".into(),
        }
    }
}

/// Dot-files are hidden on Linux and macOS.
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}
