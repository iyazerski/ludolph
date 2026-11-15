use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::session::{Freshness, Summary};

/// The saved index's format. A file in any other format is ignored, and the
/// index is built again from the session files.
const FORMAT: u32 = 6;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    sessions: BTreeMap<PathBuf, Summary>,
    /// Match the last activity: pi records the same abort for Stop and engine shutdown.
    #[serde(default)]
    interrupted: BTreeMap<String, DateTime<Utc>>,
    /// A session may have several files; the most recently written copy wins.
    #[serde(skip)]
    ids: HashMap<String, PathBuf>,
}

impl Index {
    pub fn load(path: &Path) -> Result<Self> {
        let Some(saved) = ludolph_json_file::load::<serde_json::Value>(path)? else {
            return Ok(Self::default());
        };
        if saved["format"] != FORMAT {
            return Ok(Self::default());
        }
        let mut index =
            Self::deserialize(saved).with_context(|| format!("parsing {}", path.display()))?;
        for (path, summary) in &index.sessions {
            let best = index.ids.get(&summary.id);
            if best.is_none_or(|best| rank(path, summary) > rank(best, &index.sessions[best])) {
                index.ids.insert(summary.id.clone(), path.clone());
            }
        }
        Ok(index)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        // Borrowed for the write, so saving doesn't copy every row.
        #[derive(Serialize)]
        struct SavedRef<'a> {
            format: u32,
            #[serde(flatten)]
            index: &'a Index,
        }
        ludolph_json_file::save(
            path,
            &SavedRef {
                format: FORMAT,
                index: self,
            },
        )
    }

    pub fn refresh(&mut self, sessions_dir: &Path) -> Result<Vec<PathBuf>> {
        let files = session_files(sessions_dir)?;
        let mut changed: Vec<PathBuf> = self
            .sessions
            .keys()
            .filter(|path| files.binary_search(path).is_err())
            .cloned()
            .collect();
        for path in &changed {
            self.remove(path);
        }
        for path in files {
            if self.refresh_file(&path)? {
                changed.push(path);
            }
        }
        Ok(changed)
    }

    pub fn refresh_file(&mut self, path: &Path) -> Result<bool> {
        let freshness = match self.sessions.get_mut(path) {
            Some(summary) => summary.update(path),
            None => Ok(Freshness::Rewritten),
        };
        let summary = match freshness {
            Ok(Freshness::Unchanged) => return Ok(false),
            Ok(Freshness::Updated) => {
                let id = self.sessions[path].id.clone();
                self.elect(&id);
                return Ok(true);
            }
            Ok(Freshness::Rewritten) => Summary::read(path),
            Err(e) => Err(e),
        };
        let summary = match summary {
            Ok(summary) => summary,
            Err(e) if is_not_found(&e) => None,
            Err(e) => return Err(e),
        };
        Ok(match summary {
            Some(summary) => {
                let id = summary.id.clone();
                if let Some(old) = self.sessions.insert(path.to_owned(), summary)
                    && old.id != id
                {
                    self.elect(&old.id);
                }
                self.elect(&id);
                true
            }
            None => self.remove(path),
        })
    }

    fn remove(&mut self, path: &Path) -> bool {
        let Some(old) = self.sessions.remove(path) else {
            return false;
        };
        self.elect(&old.id);
        true
    }

    fn elect(&mut self, id: &str) {
        let files = self.sessions.iter().filter(|(_, s)| s.id == id);
        let best = files.max_by_key(|(path, summary)| rank(path, summary));
        let Some((path, summary)) = best else {
            self.ids.remove(id);
            self.interrupted.remove(id);
            return;
        };
        let activity = summary.activity();
        self.interrupted
            .retain(|chat, at| chat != id || activity <= *at);
        self.ids.insert(id.to_owned(), path.clone());
    }

    pub fn find(&self, id: &str) -> Option<(&Path, &Summary)> {
        let path = self.ids.get(id)?;
        Some((path.as_path(), &self.sessions[path]))
    }

    pub fn chats(&self) -> impl Iterator<Item = (&Path, &Summary)> {
        self.ids
            .values()
            .map(|path| (path.as_path(), &self.sessions[path]))
    }

    pub fn get(&self, path: &Path) -> Option<&Summary> {
        self.sessions.get(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Path, &Summary)> {
        self.sessions
            .iter()
            .map(|(path, summary)| (path.as_path(), summary))
    }

    pub fn is_interrupted(&self, id: &str) -> bool {
        self.interrupted.contains_key(id)
    }

    pub fn set_interrupted(&mut self, id: &str) {
        if let Some((_, summary)) = self.find(id) {
            let at = summary.activity();
            self.interrupted.insert(id.to_owned(), at);
        }
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

/// Break modification-time ties by path for deterministic copy selection.
fn rank<'a>(path: &'a PathBuf, summary: &Summary) -> (SystemTime, &'a PathBuf) {
    (summary.modified(), path)
}

pub fn is_session_file(sessions_dir: &Path, path: &Path) -> bool {
    let folder = path.parent();
    folder.and_then(Path::parent) == Some(sessions_dir)
        && path.extension().is_some_and(|ext| ext == "jsonl")
}

fn session_files(sessions_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let reading = || format!("reading {}", sessions_dir.display());
    let dirs = match std::fs::read_dir(sessions_dir) {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(files),
        Err(e) => return Err(e).with_context(reading),
    };
    for dir in dirs {
        let dir = dir.with_context(reading)?;
        if !dir.file_type().with_context(reading)?.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(dir.path()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(reading),
        };
        for entry in entries {
            let path = entry.with_context(reading)?.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == ErrorKind::NotFound)
}
