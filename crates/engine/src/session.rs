//! What the chat list needs from one pi session file, read incrementally.
//!
//! pi appends to its session files (`docs/session-format.md`), so a
//! [`Summary`] remembers how far it read and later reads only what's new. The
//! fields follow pi's own session list (`buildSessionInfo` in pi's
//! `session-manager.js`): the latest `session_info` names the session, and
//! user and assistant messages count as activity.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The most characters of a message kept as a preview.
const PREVIEW_CHARS: usize = 200;

/// One session file, as the chat list shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// pi's session id, from the file's header.
    pub id: String,
    /// The folder pi ran in, which is the chat's project.
    pub cwd: PathBuf,
    /// When pi created the session, from the header.
    pub created: DateTime<Utc>,
    /// The latest `session_info` name; an empty one clears it, as in pi.
    pub title: Option<String>,
    /// The first user message with text, for a chat that has no title yet.
    pub first_prompt: Option<String>,
    /// The last user message with text.
    pub last_prompt: Option<String>,
    /// The newest user or assistant message.
    pub last_activity: Option<DateTime<Utc>>,
    /// How the last message left the chat; `None` before the first one.
    pub outcome: Option<Outcome>,
    /// The last entry in the file, which pi resumes from.
    pub leaf: Option<String>,
    progress: Progress,
}

/// How the last user, assistant or tool message left a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The assistant finished its answer.
    Done,
    /// The assistant stopped on an error or the context limit.
    Failed,
    /// Someone stopped the run.
    Aborted,
    /// The run hadn't ended when the file was last written: it's either still
    /// working or it was interrupted.
    Unfinished,
}

/// What [`Summary::update`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// The file hasn't changed since the last read.
    Unchanged,
    /// The summary now includes what was appended.
    Updated,
    /// pi rewrote the file (it does when it migrates an old version), so it
    /// has to be read again with [`Summary::read`].
    Rewritten,
}

/// How far a summary has read its file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Progress {
    /// The header's format version, which a rewrite changes.
    version: u32,
    /// The end of the last complete line read.
    offset: u64,
    /// The file's size and modification time when it was last read, to skip
    /// opening a file that hasn't changed.
    len: u64,
    modified: SystemTime,
}

impl Summary {
    /// Read the session file at `path` from the start.
    ///
    /// Returns `None` if the file doesn't start with a complete pi session
    /// header, like a file pi hasn't finished writing.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let (mut reader, len, modified) = open(path)?;
        let mut buf = Vec::new();
        let Some(header) = read_header(&mut reader, &mut buf, path)? else {
            return Ok(None);
        };
        let mut summary = Self {
            id: header.id,
            cwd: header.cwd,
            created: header.timestamp,
            title: None,
            first_prompt: None,
            last_prompt: None,
            last_activity: None,
            outcome: None,
            leaf: None,
            progress: Progress {
                version: header.version,
                offset: header.len,
                len,
                modified,
            },
        };
        summary.read_entries(&mut reader, &mut buf, path)?;
        Ok(Some(summary))
    }

    /// Read what was appended to the file at `path` since the last read.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read.
    pub fn update(&mut self, path: &Path) -> Result<Freshness> {
        let (mut reader, len, modified) = open(path)?;
        if len == self.progress.len && modified == self.progress.modified {
            return Ok(Freshness::Unchanged);
        }
        if len < self.progress.offset {
            return Ok(Freshness::Rewritten);
        }
        let mut buf = Vec::new();
        match read_header(&mut reader, &mut buf, path)? {
            Some(header) if header.id == self.id && header.version == self.progress.version => {}
            _ => return Ok(Freshness::Rewritten),
        }
        reader
            .seek(SeekFrom::Start(self.progress.offset))
            .with_context(|| format!("reading {}", path.display()))?;
        self.progress.len = len;
        self.progress.modified = modified;
        self.read_entries(&mut reader, &mut buf, path)?;
        Ok(Freshness::Updated)
    }

    /// Read complete lines from the reader's position, and fold them in.
    fn read_entries(
        &mut self,
        reader: &mut BufReader<File>,
        buf: &mut Vec<u8>,
        path: &Path,
    ) -> Result<()> {
        while let Some(len) = read_line(reader, buf, path)? {
            self.progress.offset += len;
            // pi skips lines it can't parse too.
            if let Ok(entry) = serde_json::from_slice::<Entry>(buf) {
                self.add(entry);
            }
        }
        Ok(())
    }

    /// Fold one entry into the summary.
    fn add(&mut self, entry: Entry) {
        if entry.id.is_some() {
            self.leaf = entry.id;
        }
        match entry.kind {
            Kind::SessionInfo => {
                self.title = entry
                    .name
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty());
            }
            Kind::Message => {
                if let Some(message) = entry.message {
                    self.add_message(message, entry.timestamp);
                }
            }
            Kind::Session | Kind::Other => {}
        }
    }

    /// Fold one message into the summary; `entry_time` stands in for a
    /// message without a timestamp.
    fn add_message(&mut self, message: Message, entry_time: Option<DateTime<Utc>>) {
        let (Role::User | Role::Assistant | Role::ToolResult) = message.role else {
            return;
        };
        self.outcome = Some(match (message.role, message.stop_reason) {
            (Role::Assistant, Some(StopReason::Stop)) => Outcome::Done,
            (Role::Assistant, Some(StopReason::Error | StopReason::Length)) => Outcome::Failed,
            (Role::Assistant, Some(StopReason::Aborted)) => Outcome::Aborted,
            _ => Outcome::Unfinished,
        });
        if message.role == Role::ToolResult {
            return;
        }
        let time = message
            .timestamp
            .and_then(DateTime::from_timestamp_millis)
            .or(entry_time);
        if let Some(time) = time {
            self.last_activity = self.last_activity.max(Some(time));
        }
        if message.role == Role::User
            && let Some(text) = message.content.and_then(|content| preview(content.get()))
        {
            self.first_prompt.get_or_insert_with(|| text.clone());
            self.last_prompt = Some(text);
        }
    }
}

/// Open `path` with its size and modification time.
fn open(path: &Path) -> Result<(BufReader<File>, u64, SystemTime)> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    Ok((BufReader::new(file), meta.len(), meta.modified()?))
}

/// Read one complete line into `buf` and return its length in bytes.
///
/// Returns `None` at the end of the file, and for a last line without its
/// newline: pi may be writing it, so it's read once it's complete.
fn read_line(reader: &mut BufReader<File>, buf: &mut Vec<u8>, path: &Path) -> Result<Option<u64>> {
    buf.clear();
    let len = reader
        .read_until(b'\n', buf)
        .with_context(|| format!("reading {}", path.display()))?;
    if buf.last() != Some(&b'\n') {
        return Ok(None);
    }
    Ok(Some(
        u64::try_from(len).expect("a line is shorter than u64::MAX"),
    ))
}

/// A session file's first line.
struct Header {
    id: String,
    cwd: PathBuf,
    timestamp: DateTime<Utc>,
    version: u32,
    /// The header line's length in bytes.
    len: u64,
}

/// Read the first line as a session header; `None` if it isn't one.
fn read_header(
    reader: &mut BufReader<File>,
    buf: &mut Vec<u8>,
    path: &Path,
) -> Result<Option<Header>> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(rename = "type")]
        kind: Kind,
        id: String,
        cwd: PathBuf,
        timestamp: DateTime<Utc>,
        // Version 1 files have none (`docs/session-format.md`).
        #[serde(default = "first_version")]
        version: u32,
    }
    fn first_version() -> u32 {
        1
    }
    let Some(len) = read_line(reader, buf, path)? else {
        return Ok(None);
    };
    Ok(serde_json::from_slice::<Line>(buf)
        .ok()
        .filter(|line| line.kind == Kind::Session)
        .map(|line| Header {
            id: line.id,
            cwd: line.cwd,
            timestamp: line.timestamp,
            version: line.version,
            len,
        }))
}

/// One session entry, with only the fields a summary needs. Message content
/// stays raw, so long tool output and images are skipped, not copied.
#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    name: Option<String>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
}

/// A message entry's message, with its content left raw.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message<'a> {
    role: Role,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    timestamp: Option<i64>,
    stop_reason: Option<StopReason>,
}

/// An entry's type, with the ones a summary ignores as `Other`.
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Session,
    SessionInfo,
    Message,
    #[serde(other)]
    Other,
}

/// Who wrote a message (`docs/message-types.md`).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Role {
    User,
    Assistant,
    ToolResult,
    #[serde(other)]
    Other,
}

/// Why an assistant message ended (`docs/message-types.md`).
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum StopReason {
    Stop,
    Length,
    Error,
    Aborted,
    #[serde(other)]
    Other,
}

/// A message's text on one line and cut to [`PREVIEW_CHARS`]; `None` if it
/// has none, like a message that's only an image.
fn preview(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Content {
        Text(String),
        Blocks(Vec<Block>),
    }
    #[derive(Deserialize)]
    struct Block {
        #[serde(rename = "type")]
        kind: String,
        text: Option<String>,
    }
    let text = match serde_json::from_str::<Content>(content).ok()? {
        Content::Text(text) => text,
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter(|block| block.kind == "text")
            .filter_map(|block| block.text)
            .collect::<Vec<_>>()
            .join(" "),
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let line = words.join(" ");
    Some(match line.char_indices().nth(PREVIEW_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    })
}
