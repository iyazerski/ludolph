use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::data::Chat;

const SHORTCUT: &str = "ctrl+alt+space";

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    seen: BTreeMap<String, Option<DateTime<Utc>>>,
    pub collapsed: BTreeSet<String>,
    pub new_chat: NewChat,
    pub shortcut: Option<String>,
    pub main_frame: Option<Frame>,
    pub show_details: bool,
    pub accent: Accent,
    pub text_size: TextSize,
    pub release_checked: Option<DateTime<Utc>>,
    /// The release whose notice was closed, so it isn't offered again.
    pub dismissed_release: Option<String>,
    #[cfg(windows)]
    pub managed_wsl: Option<String>,
}

/// A window's place, in physical pixels, as Rune reads and takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub position: (i32, i32),
    pub size: (u32, u32),
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            seen: BTreeMap::new(),
            collapsed: BTreeSet::new(),
            new_chat: NewChat::default(),
            shortcut: Some(SHORTCUT.to_owned()),
            main_frame: None,
            show_details: true,
            accent: Accent::default(),
            text_size: TextSize::default(),
            release_checked: None,
            dismissed_release: None,
            #[cfg(windows)]
            managed_wsl: None,
        }
    }
}

/// A page zoom for both windows: text, icons and spacing scale together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Smaller,
    #[default]
    Default,
    Larger,
    Largest,
}

impl TextSize {
    pub const ALL: [Self; 4] = [Self::Smaller, Self::Default, Self::Larger, Self::Largest];

    pub fn name(self) -> &'static str {
        match self {
            Self::Smaller => "Smaller",
            Self::Default => "Default",
            Self::Larger => "Larger",
            Self::Largest => "Largest",
        }
    }

    pub fn zoom(self) -> f32 {
        match self {
            Self::Smaller => 0.9,
            Self::Default => 1.0,
            Self::Larger => 1.15,
            Self::Largest => 1.3,
        }
    }

    /// The next size up (`step` 1) or down (-1), stopping at the ends.
    pub fn step(self, step: isize) -> Self {
        let at = Self::ALL.iter().position(|&s| s == self).unwrap_or(1);
        Self::ALL[at.saturating_add_signed(step).min(Self::ALL.len() - 1)]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Blue,
    Purple,
    Pink,
    Orange,
    Green,
    Gray,
}

impl Accent {
    pub const ALL: [Self; 6] = [
        Self::Blue,
        Self::Purple,
        Self::Pink,
        Self::Orange,
        Self::Green,
        Self::Gray,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Pink => "Pink",
            Self::Orange => "Orange",
            Self::Green => "Green",
            Self::Gray => "Gray",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NewChat {
    pub project: ProjectPick,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPick {
    #[default]
    Recent,
    NoProject,
    Project(String),
}

impl From<Option<String>> for ProjectPick {
    fn from(project: Option<String>) -> Self {
        project.map_or(Self::NoProject, Self::Project)
    }
}

impl Prefs {
    pub fn load(path: &Path) -> Self {
        ludolph_json_file::load::<Prefs>(path)
            .unwrap_or_else(|e| {
                eprintln!("{e:#}");
                None
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn prime(&mut self, chats: &[Chat]) -> bool {
        let mut changed = false;
        for c in chats {
            if !self.seen.contains_key(&c.id) {
                self.seen.insert(c.id.clone(), c.last_message_at);
                changed = true;
            }
        }
        changed
    }

    #[must_use]
    pub fn mark_seen(&mut self, chat: &Chat) -> bool {
        let Some(at) = chat.last_message_at else {
            return false;
        };
        self.seen.insert(chat.id.clone(), Some(at)) != Some(Some(at))
    }

    pub fn unseen(&self, chat: &Chat) -> bool {
        matches!(
            (self.seen.get(&chat.id), chat.last_message_at),
            (Some(Some(seen)), Some(at)) if at > *seen
        )
    }

    pub fn toggle_collapsed(&mut self, key: &str) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_owned());
        }
    }
}
