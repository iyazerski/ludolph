//! The chat list: a [`Summary`] of every pi session file, kept current by
//! reading only what changed, and saved so a restart doesn't read every file.

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
const FORMAT: u32 = 5;

/// Every session file pi has written, by path, and the chats whose run the
/// engine cut short.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    sessions: BTreeMap<PathBuf, Summary>,
    /// Chats whose run the engine cut short, by session id, with their last
    /// activity then; newer activity ends it. The file
    /// can't tell: pi writes the same abort for a stop and a shutdown.
    #[serde(default)]
    interrupted: BTreeMap<String, DateTime<Utc>>,
    /// Each session id's file. A session can have copies under other
    /// folders, as a tool that moves chats between folders can leave; the most recently
    /// written one is the chat.
    #[serde(skip)]
    ids: HashMap<String, PathBuf>,
}

impl Index {
    /// Load the index saved at `path`, or an empty one if there's none or it's
    /// in another format.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read or parsed.
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

    /// Save the index to `path`, replacing it atomically.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be written.
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

    /// Bring every session file under `sessions_dir` up to date, and return
    /// the paths whose rows were added, changed or removed.
    ///
    /// pi keeps one folder per working directory there, with a `.jsonl` file
    /// per session (`docs/sessions.md`).
    ///
    /// # Errors
    ///
    /// Fails if the folder or a session file can't be read.
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

    /// Bring one session file's row up to date, and return whether it changed.
    /// A file that's gone loses its row.
    ///
    /// # Errors
    ///
    /// Fails if the file exists but can't be read.
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

    /// Drop the row of the file at `path`, and return whether it had one.
    fn remove(&mut self, path: &Path) -> bool {
        let Some(old) = self.sessions.remove(path) else {
            return false;
        };
        self.elect(&old.id);
        true
    }

    /// Pick session `id`'s file again after one of its files changed, and
    /// end its interrupted mark if it has new activity or is gone.
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

    /// Chat `id`: its session file and summary.
    pub fn find(&self, id: &str) -> Option<(&Path, &Summary)> {
        let path = self.ids.get(id)?;
        Some((path.as_path(), &self.sessions[path]))
    }

    /// Every chat, by its session file and summary, once each.
    pub fn chats(&self) -> impl Iterator<Item = (&Path, &Summary)> {
        self.ids
            .values()
            .map(|path| (path.as_path(), &self.sessions[path]))
    }

    /// The summary of the session file at `path`.
    pub fn get(&self, path: &Path) -> Option<&Summary> {
        self.sessions.get(path)
    }

    /// Every session file and its summary, by path.
    pub fn iter(&self) -> impl Iterator<Item = (&Path, &Summary)> {
        self.sessions
            .iter()
            .map(|(path, summary)| (path.as_path(), summary))
    }

    /// Whether the engine cut chat `id`'s run short, and it has had no
    /// activity since.
    pub fn is_interrupted(&self, id: &str) -> bool {
        self.interrupted.contains_key(id)
    }

    /// Mark chat `id`'s run as cut short, as of its last activity. A chat pi
    /// hasn't written has no run to resume.
    pub fn set_interrupted(&mut self, id: &str) {
        if let Some((_, summary)) = self.find(id) {
            let at = summary.activity();
            self.interrupted.insert(id.to_owned(), at);
        }
    }

    /// The number of session files.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether there are no session files.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

/// How a session's copies rank: the one written last wins, then the later
/// path.
fn rank<'a>(path: &'a PathBuf, summary: &Summary) -> (SystemTime, &'a PathBuf) {
    (summary.modified(), path)
}

/// Whether `path` is where pi keeps a session under `sessions_dir`: a
/// `.jsonl` file in one of its folders, as `refresh` finds them.
pub fn is_session_file(sessions_dir: &Path, path: &Path) -> bool {
    let folder = path.parent();
    folder.and_then(Path::parent) == Some(sessions_dir)
        && path.extension().is_some_and(|ext| ext == "jsonl")
}

/// Every `.jsonl` file one folder down from `sessions_dir`, sorted.
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
            // Removed since the outer listing.
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

/// Whether `e` is a missing file, like a session deleted mid-read.
fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == ErrorKind::NotFound)
}
