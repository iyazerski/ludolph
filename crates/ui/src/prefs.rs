//! What the app remembers between runs, as the phone app keeps it in local
//! storage: which turns were seen, folded projects, the last new chat's
//! picks, and the global shortcut, which the app registers itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::data::Chat;

/// The shortcut that shows and hides the quick window until you record another.
const SHORTCUT: &str = "ctrl+alt+space";

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Each chat's last activity the app has shown, so later turns read "Done".
    seen: BTreeMap<String, Option<DateTime<Utc>>>,
    /// Project sections folded in the list, by project id (`home` for none).
    pub collapsed: BTreeSet<String>,
    /// The picks of the last chat started here.
    pub new_chat: NewChat,
    /// The quick window's global shortcut, as Rune writes it (`ctrl+alt+space`),
    /// or `None` for none.
    pub shortcut: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            seen: BTreeMap::new(),
            collapsed: BTreeSet::new(),
            new_chat: NewChat::default(),
            shortcut: Some(SHORTCUT.to_owned()),
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
    /// The most recent chat's project.
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
    /// Load the prefs at `path`; a missing or broken file starts fresh.
    pub fn load(path: &Path) -> Self {
        ludolph_json_file::load::<Prefs>(path)
            .unwrap_or_else(|e| {
                eprintln!("{e:#}");
                None
            })
            .unwrap_or_default()
    }

    /// Count chats not met before as seen, so only later turns read "Done".
    /// Return whether any was new.
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

    /// Count the chat's latest turn as seen. Return whether it was new.
    #[must_use]
    pub fn mark_seen(&mut self, chat: &Chat) -> bool {
        let Some(at) = chat.last_message_at else {
            return false;
        };
        self.seen.insert(chat.id.clone(), Some(at)) != Some(Some(at))
    }

    /// Whether the chat moved since it was last shown here.
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
