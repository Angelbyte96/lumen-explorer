//! The behavior options of the Options window, like File Pilot's: scrolling,
//! what a plain character key does, what opens on startup and in a new tab.

use std::path::PathBuf;

/// What a character key pressed without modifiers does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharKey {
    /// Runs the key's hotkey; Lumen has no single-key hotkeys yet, so the key does nothing.
    TriggerHotkey,
    /// Types into the filter of the folder.
    #[default]
    TypeInFilter,
    /// Selects the first item whose name starts with what was typed.
    SelectTypedItem,
}

impl CharKey {
    pub const ALL: [CharKey; 3] = [CharKey::TriggerHotkey, CharKey::TypeInFilter, CharKey::SelectTypedItem];

    pub fn name(self) -> &'static str {
        match self {
            CharKey::TriggerHotkey => "Trigger Hotkey",
            CharKey::TypeInFilter => "Type In Filter",
            CharKey::SelectTypedItem => "Select Typed Item",
        }
    }

    /// The word the settings file keeps.
    pub fn key(self) -> &'static str {
        match self {
            CharKey::TriggerHotkey => "trigger_hotkey",
            CharKey::TypeInFilter => "type_in_filter",
            CharKey::SelectTypedItem => "select_typed_item",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }
}

/// What a window opens on startup, when no folder was given to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Startup {
    /// The tabs of the last window closed.
    #[default]
    LastSession,
    /// The home folder.
    Home,
    /// A folder chosen in the Options window.
    Folder(PathBuf),
}

impl Startup {
    /// `last_session`, `home` or `folder <path>`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.split_once(' ') {
            Some(("folder", path)) if !path.trim().is_empty() => Some(Startup::Folder(PathBuf::from(path.trim()))),
            None if value == "last_session" => Some(Startup::LastSession),
            None if value == "home" => Some(Startup::Home),
            _ => None,
        }
    }
}

/// The folder a new tab opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewTab {
    /// `/` on Linux; the drives on Windows.
    Root,
    Home,
    /// The folder of the tab it was opened from.
    #[default]
    Current,
}

impl NewTab {
    pub const ALL: [NewTab; 3] = [NewTab::Root, NewTab::Home, NewTab::Current];

    pub fn name(self) -> &'static str {
        match self {
            NewTab::Root => "Root",
            NewTab::Home => "Home",
            NewTab::Current => "Current",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            NewTab::Root => "root",
            NewTab::Home => "home",
            NewTab::Current => "current",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|n| n.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Behavior {
    /// "Scroll Beyond Last Item".
    pub overscroll: bool,
    /// "Reverse Scroll Direction".
    pub reverse_scroll: bool,
    pub char_key: CharKey,
    pub startup: Startup,
    pub new_tab: NewTab,
}
