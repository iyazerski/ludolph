//! Chat transcripts from pi's session files, as the phone app and the
//! Companion show them (`chat_view`).
//!
//! A transcript is the path from the session's leaf back to its start
//! (`docs/session-format.md`, "Tree Structure"), so entries on abandoned
//! branches don't show. Each file is scanned once for its entries' ids,
//! parents, kinds and byte ranges, and then only for what pi appended. Only
//! the lines of the page asked for are parsed again, into just the fields a
//! message shows, so a long session with big tool output costs little.
//!
//! A message is a user prompt, a note like a compaction, or an assistant
//! turn: every assistant reply and tool result until the next prompt.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::DateTime;
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jsonl::{self, Mark, Reopen, fix_surrogates, read_header, read_line};
use crate::skills;
use crate::text::one_line;

/// How many files keep their scan.
const MAX_FILES: usize = 8;
/// The most characters of a tool call's subject, like a command, shown.
const TOOL_CHARS: usize = 160;

/// One message, as the phone app reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The id of the message's first entry, or `live-<n>` for a message
    /// pi hasn't written yet.
    pub id: String,
    pub role: Role,
    /// Epoch milliseconds.
    pub created_at: i64,
    pub status: MessageStatus,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    /// A note that's neither side's: a compaction, a branch summary, or an
    /// extension's message.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Complete,
    Streaming,
    Aborted,
}

/// One run of a message, in order. Consecutive tool calls share a group
/// until prose or an error ends it; thinking isn't shown and doesn't end one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Part {
    Text { text: String },
    Tools { tools: Vec<String> },
    Error { error: String },
}

/// What the running turn added, from pi's events.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tail<'a> {
    /// Finished messages, as pi's `AgentMessage` values.
    pub messages: &'a [Value],
    /// The message pi is writing.
    pub streaming: Option<&'a Value>,
}

/// The latest messages of a transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The transcript's messages, of which these are the last.
    pub total: usize,
    pub messages: Vec<Message>,
    /// The file's latest entry, which changes whenever pi writes one.
    pub leaf: Option<String>,
}

impl Page {
    /// How many messages come before this page.
    pub fn older(&self) -> usize {
        self.total - self.messages.len()
    }
}

/// The scans of recently read session files.
#[derive(Debug, Default)]
pub struct Transcripts {
    /// Least recently used first.
    files: Vec<(PathBuf, Scan)>,
}

impl Transcripts {
    /// The last `limit` messages of the transcript in `path`, followed by
    /// the messages of `tail` the file doesn't have yet. A chat pi hasn't
    /// written yet has no `path`.
    ///
    /// pi writes each message when it ends, while its events carry on, so
    /// the file and the tail overlap. A message is matched by its role, its
    /// timestamp and, for a tool result, its call: pi writes the timestamp it
    /// streamed.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read.
    pub fn page(&mut self, path: Option<&Path>, tail: Tail, limit: usize) -> Result<Page> {
        match path {
            Some(path) => self.scan(path)?.page(path, tail, limit),
            None => Scan::default().page(Path::new(""), tail, limit),
        }
    }

    /// `path`'s scan, brought up to date.
    fn scan(&mut self, path: &Path) -> Result<&mut Scan> {
        let cached = self.files.iter().position(|(p, _)| p == path);
        let scan = match cached {
            Some(at) => {
                let (_, mut scan) = self.files.remove(at);
                if !scan.update(path)? {
                    scan = Scan::read(path)?;
                }
                scan
            }
            None => {
                if self.files.len() == MAX_FILES {
                    self.files.remove(0);
                }
                Scan::read(path)?
            }
        };
        self.files.push((path.to_owned(), scan));
        Ok(&mut self.files.last_mut().expect("just pushed").1)
    }
}

/// What a transcript needs from a session file, without its content.
#[derive(Debug, Default)]
struct Scan {
    id: String,
    version: u32,
    offset: u64,
    len: u64,
    modified: Option<SystemTime>,
    entries: Vec<Meta>,
    by_id: HashMap<String, usize>,
    /// The last page's messages that are wholly in the file, by the first and
    /// last entry each spans.
    rendered: HashMap<(usize, usize), Message>,
}

/// One entry: where it sits in the tree and in the file.
#[derive(Debug)]
struct Meta {
    id: String,
    parent: Option<usize>,
    start: u64,
    len: u64,
    kind: Kind,
    /// What matches it to a live message.
    stamp: Option<Stamp>,
    /// A tool result that failed. Results are only rendered by this and
    /// their call, so their lines, often big, aren't read again.
    failed: bool,
}

/// How an entry joins the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// It starts a message: a prompt or a note.
    Starts,
    /// It joins the assistant turn that's going, or starts one.
    Turn,
    /// It doesn't show, like a model change or a system prompt.
    Hidden,
}

/// A message's role, timestamp and tool call, which a live message and the
/// entry pi writes for it share.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Stamp {
    role: Box<str>,
    time: i64,
    call: Option<Box<str>>,
}

impl Stamp {
    fn of(message: &Value) -> Option<Self> {
        Some(Self {
            role: message["role"].as_str()?.into(),
            time: message["timestamp"].as_i64()?,
            call: message["toolCallId"].as_str().map(Into::into),
        })
    }
}

/// A file entry or a live message, in transcript order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    File(usize),
    Live(usize),
}

impl Scan {
    /// Scan `path` from the start. A file without a complete header yet has
    /// no entries.
    fn read(path: &Path) -> Result<Self> {
        let (mut reader, len, modified) = jsonl::open(path)?;
        let mut buf = Vec::new();
        let Some(header) = read_header(&mut reader, &mut buf, path)? else {
            return Ok(Self::default());
        };
        let mut scan = Self {
            id: header.id,
            version: header.version,
            offset: header.len,
            len,
            modified: Some(modified),
            ..Self::default()
        };
        scan.read_entries(&mut reader, &mut buf, path)?;
        Ok(scan)
    }

    /// Scan what was appended since the last read; `false` if pi rewrote the
    /// file, so it needs a new scan.
    fn update(&mut self, path: &Path) -> Result<bool> {
        let Some(modified) = self.modified else {
            return Ok(false);
        };
        let mark = Mark {
            id: &self.id,
            version: self.version,
            offset: self.offset,
            len: self.len,
            modified,
        };
        match jsonl::reopen(path, &mark)? {
            Reopen::Unchanged => Ok(true),
            Reopen::Rewritten => Ok(false),
            Reopen::Appended {
                mut reader,
                len,
                modified,
            } => {
                self.len = len;
                self.modified = Some(modified);
                self.read_entries(&mut reader, &mut Vec::new(), path)?;
                Ok(true)
            }
        }
    }

    fn read_entries(
        &mut self,
        reader: &mut BufReader<File>,
        buf: &mut Vec<u8>,
        path: &Path,
    ) -> Result<()> {
        while let Some(len) = read_line(reader, buf, path)? {
            let start = self.offset;
            self.offset += len;
            // pi skips lines it can't parse too.
            let Ok(line) = serde_json::from_slice::<Line>(buf) else {
                continue;
            };
            let kind = line.kind();
            let stamp = line.stamp();
            let failed = line.message.as_ref().is_some_and(|m| m.is_error);
            let Some(id) = line.id.map(Cow::into_owned) else {
                continue;
            };
            let parent = line
                .parent_id
                .and_then(|p| self.by_id.get(p.as_ref()).copied());
            self.by_id.insert(id.clone(), self.entries.len());
            self.entries.push(Meta {
                id,
                parent,
                start,
                len,
                kind,
                stamp,
                failed,
            });
        }
        Ok(())
    }

    /// The last `limit` messages of the path to the leaf, followed by the
    /// messages of `tail` that the path doesn't have.
    fn page(&mut self, path: &Path, tail: Tail, limit: usize) -> Result<Page> {
        let mut chain = Vec::new();
        let mut at = self.entries.len().checked_sub(1);
        // A parent always comes before its child, so this ends.
        while let Some(i) = at {
            chain.push(i);
            at = self.entries[i].parent;
        }
        chain.reverse();
        // The file can be any number of entries ahead of the events.
        let written: HashSet<&Stamp> = chain
            .iter()
            .filter_map(|&i| self.entries[i].stamp.as_ref())
            .collect();
        let unwritten = |m: &&Value| Stamp::of(m).is_none_or(|stamp| !written.contains(&stamp));
        let streaming = tail.streaming.filter(unwritten);
        let live: Vec<&Value> = tail
            .messages
            .iter()
            .filter(unwritten)
            .chain(streaming)
            .collect();
        let streaming = streaming.map(|_| live.len() - 1);

        let file_items = chain.iter().map(|&i| (Item::File(i), self.entries[i].kind));
        let live_items = (live.iter().enumerate()).map(|(i, m)| (Item::Live(i), message_kind(m)));
        let groups = group(file_items.chain(live_items));
        let skip = groups.len().saturating_sub(limit);

        let mut reader = None;
        let previous = std::mem::take(&mut self.rendered);
        let mut messages = Vec::with_capacity(groups.len() - skip);
        for span in &groups[skip..] {
            let whole = match (span.first(), span.last()) {
                (Some(Item::File(first)), Some(Item::File(last))) => Some((*first, *last)),
                _ => None,
            };
            if let Some(message) = whole.and_then(|key| previous.get(&key)) {
                messages.push(message.clone());
                continue;
            }
            let mut pieces = Vec::with_capacity(span.len());
            for item in span {
                let piece = match *item {
                    Item::File(i) => self.piece(&mut reader, path, i)?,
                    Item::Live(i) => Piece::live(i, live[i], streaming == Some(i)),
                };
                pieces.extend(piece);
            }
            let Some(message) = render(&pieces) else {
                continue;
            };
            messages.push(message);
        }
        // Keep what this page rendered, and only that.
        for (span, message) in groups[skip..].iter().zip(&messages) {
            if let (Some(Item::File(first)), Some(Item::File(last))) = (span.first(), span.last()) {
                self.rendered.insert((*first, *last), message.clone());
            }
        }
        Ok(Page {
            total: groups.len(),
            messages,
            leaf: self.entries.last().map(|e| e.id.clone()),
        })
    }

    /// File entry `i`, read from `path` unless it's a tool result.
    fn piece(
        &self,
        reader: &mut Option<BufReader<File>>,
        path: &Path,
        i: usize,
    ) -> Result<Option<Piece>> {
        let meta = &self.entries[i];
        let stamp = meta.stamp.as_ref();
        if stamp.is_some_and(|s| &*s.role == "toolResult") {
            let message = MessageView {
                role: "toolResult".to_owned(),
                tool_call_id: stamp.and_then(|s| s.call.as_deref()).map(Into::into),
                is_error: meta.failed,
                ..MessageView::default()
            };
            return Ok(Some(Piece::file(&meta.id, EntryView::message(message))));
        }
        let reader = match reader {
            Some(reader) => reader,
            None => reader.insert(BufReader::new(
                File::open(path).with_context(|| format!("opening {}", path.display()))?,
            )),
        };
        reader.seek(SeekFrom::Start(meta.start))?;
        let len = usize::try_from(meta.len).context("a line longer than memory")?;
        let mut buf = vec![0; len];
        reader
            .read_exact(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        fix_surrogates(&mut buf);
        match serde_json::from_slice::<EntryView>(&buf) {
            Ok(entry) => Ok(Some(Piece::file(&meta.id, entry))),
            Err(e) => {
                eprintln!("{}: skipping entry {}: {e}", path.display(), meta.id);
                Ok(None)
            }
        }
    }
}

/// Split items into messages: each prompt or note starts one, and an
/// assistant turn runs until the next.
fn group(items: impl Iterator<Item = (Item, Kind)>) -> Vec<Vec<Item>> {
    let mut groups: Vec<Vec<Item>> = Vec::new();
    let mut in_turn = false;
    for (item, kind) in items {
        match kind {
            Kind::Hidden => continue,
            Kind::Turn if in_turn => {}
            Kind::Starts | Kind::Turn => groups.push(Vec::new()),
        }
        in_turn = kind == Kind::Turn;
        groups
            .last_mut()
            .expect("a group was just pushed")
            .push(item);
    }
    groups
}

/// An entry's fields a scan needs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    parent_id: Option<Cow<'a, str>>,
    display: Option<bool>,
    #[serde(borrow)]
    message: Option<Head<'a>>,
}

/// A message entry's message, with only what decides its kind and stamp.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Head<'a> {
    #[serde(borrow)]
    role: Cow<'a, str>,
    display: Option<bool>,
    timestamp: Option<i64>,
    #[serde(borrow)]
    tool_call_id: Option<Cow<'a, str>>,
    #[serde(default)]
    is_error: bool,
}

impl Line<'_> {
    fn stamp(&self) -> Option<Stamp> {
        let message = self.message.as_ref()?;
        Some(Stamp {
            role: message.role.as_ref().into(),
            time: message.timestamp?,
            call: message.tool_call_id.as_deref().map(Into::into),
        })
    }

    fn kind(&self) -> Kind {
        match (&*self.kind, &self.message) {
            ("message", Some(message)) => role_kind(&message.role, message.display),
            ("compaction" | "branch_summary", _) => Kind::Starts,
            ("custom_message", _) if self.display == Some(true) => Kind::Starts,
            _ => Kind::Hidden,
        }
    }
}

fn role_kind(role: &str, display: Option<bool>) -> Kind {
    match role {
        "user" | "bashExecution" => Kind::Starts,
        "custom" if display == Some(true) => Kind::Starts,
        "assistant" | "toolResult" => Kind::Turn,
        _ => Kind::Hidden,
    }
}

fn message_kind(message: &Value) -> Kind {
    let role = message["role"].as_str().unwrap_or_default();
    role_kind(role, message["display"].as_bool())
}

/// An entry's fields a message shows. Fields left out, like file contents,
/// images and thinking, are skipped as they're parsed, not copied.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryView {
    #[serde(rename = "type", default)]
    kind: String,
    timestamp: Option<String>,
    message: Option<MessageView>,
    summary: Option<String>,
    #[serde(default)]
    content: Content,
}

impl EntryView {
    fn message(message: MessageView) -> Self {
        Self {
            kind: "message".to_owned(),
            message: Some(message),
            ..Self::default()
        }
    }
}

/// A message's fields a message shows (`docs/message-types.md`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    #[serde(default)]
    role: String,
    timestamp: Option<i64>,
    #[serde(default)]
    content: Content,
    stop_reason: Option<String>,
    error_message: Option<String>,
    /// A `bashExecution`'s command and exit code.
    command: Option<String>,
    exit_code: Option<i64>,
    tool_call_id: Option<String>,
    #[serde(default)]
    is_error: bool,
}

/// A message's content: a string, as one text block, or blocks.
#[derive(Debug, Default)]
struct Content(Vec<Block>);

/// A content block's fields a message shows.
#[derive(Debug, Deserialize)]
struct Block {
    #[serde(rename = "type", default)]
    kind: String,
    text: Option<String>,
    /// A tool call's id, name and the arguments its line names.
    id: Option<String>,
    name: Option<String>,
    arguments: Option<Arguments>,
}

#[derive(Debug, Deserialize)]
struct Arguments {
    path: Option<String>,
    command: Option<String>,
    pattern: Option<String>,
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // By hand, since an untagged enum would copy all of it, images too.
        struct ContentVisitor;
        impl<'de> Visitor<'de> for ContentVisitor {
            type Value = Content;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string or content blocks")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Content, E> {
                Ok(Content(vec![Block {
                    kind: "text".to_owned(),
                    text: Some(text.to_owned()),
                    id: None,
                    name: None,
                    arguments: None,
                }]))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Content, E> {
                Ok(Content::default())
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Content, A::Error> {
                let mut blocks = Vec::new();
                while let Some(block) = seq.next_element()? {
                    blocks.push(block);
                }
                Ok(Content(blocks))
            }
        }
        deserializer.deserialize_any(ContentVisitor)
    }
}

/// One entry or live message being rendered.
struct Piece {
    id: String,
    entry: EntryView,
    /// From the live tail, so a tool call without a result is still running.
    live: bool,
    streaming: bool,
}

impl Piece {
    fn file(id: &str, entry: EntryView) -> Self {
        Self {
            id: id.to_owned(),
            entry,
            live: false,
            streaming: false,
        }
    }

    /// Live message `i`; `None` if it isn't a message.
    fn live(i: usize, message: &Value, streaming: bool) -> Option<Self> {
        let message = MessageView::deserialize(message).ok()?;
        Some(Self {
            id: format!("live-{i}"),
            entry: EntryView::message(message),
            live: true,
            streaming,
        })
    }

    /// When it was written, in epoch milliseconds.
    fn time(&self) -> i64 {
        let message_time = self.entry.message.as_ref().and_then(|m| m.timestamp);
        message_time
            .or_else(|| {
                let time = self.entry.timestamp.as_deref()?;
                Some(DateTime::parse_from_rfc3339(time).ok()?.timestamp_millis())
            })
            .unwrap_or_default()
    }
}

/// One message from its pieces, which [`group`] put together; `None` if
/// none could be read.
fn render(pieces: &[Piece]) -> Option<Message> {
    let first = pieces.first()?;
    let mut message = Message {
        id: first.id.clone(),
        role: Role::System,
        created_at: first.time(),
        status: MessageStatus::Complete,
        parts: Vec::new(),
    };
    let entry = &first.entry;
    let role = entry.message.as_ref().map(|m| m.role.as_str());
    match (entry.kind.as_str(), role, &entry.message) {
        ("message", Some("user"), Some(m)) => {
            message.role = Role::User;
            message.parts = content_parts(&m.content);
            for part in &mut message.parts {
                if let Part::Text { text } = part
                    && let Cow::Owned(typed) = skills::as_typed(text)
                {
                    *text = typed;
                }
            }
        }
        ("message", Some("bashExecution"), Some(m)) => {
            message.role = Role::User;
            let mut line = tool_subject("exec", m.command.as_deref().unwrap_or_default());
            if m.exit_code.is_some_and(|code| code != 0) {
                line.push_str(" (error)");
            }
            message.parts = vec![Part::Tools { tools: vec![line] }];
        }
        ("message", Some("custom"), Some(m)) => message.parts = content_parts(&m.content),
        ("custom_message", ..) => message.parts = content_parts(&entry.content),
        ("compaction", ..) => {
            message.parts = vec![Part::Text {
                text: "Compacted the conversation.".to_owned(),
            }];
        }
        ("branch_summary", ..) => {
            let summary = entry.summary.as_deref().unwrap_or_default();
            message.parts = vec![Part::Text {
                text: format!("Came back from another branch: {summary}"),
            }];
        }
        _ => {
            message.role = Role::Assistant;
            render_turn(pieces, &mut message);
        }
    }
    Some(message)
}

/// Fill in an assistant turn: its prose, tool calls and errors in order.
fn render_turn(pieces: &[Piece], message: &mut Message) {
    enum Raw<'a> {
        Part(Part),
        /// A tool call's line, its id, and whether it's live.
        Call(String, Option<&'a str>, bool),
    }
    // Tool results come after their calls, so lines are finished at the end.
    let mut failed: HashMap<&str, bool> = HashMap::new();
    let mut raw = Vec::new();
    for piece in pieces {
        let Some(m) = &piece.entry.message else {
            continue;
        };
        match m.role.as_str() {
            "toolResult" => {
                if let Some(id) = &m.tool_call_id {
                    failed.insert(id, m.is_error);
                }
            }
            "assistant" => {
                for block in &m.content.0 {
                    match (block.kind.as_str(), &block.text) {
                        ("text", Some(text)) if !text.trim().is_empty() => {
                            raw.push(Raw::Part(Part::Text { text: text.clone() }));
                        }
                        ("toolCall", _) => {
                            let line = tool_line(block);
                            raw.push(Raw::Call(line, block.id.as_deref(), piece.live));
                        }
                        _ => {}
                    }
                }
                message.status = MessageStatus::Complete;
                match m.stop_reason.as_deref() {
                    Some("error") => {
                        let error = m.error_message.as_deref().unwrap_or("The model failed.");
                        raw.push(Raw::Part(Part::Error {
                            error: error.to_owned(),
                        }));
                    }
                    Some("length") => raw.push(Raw::Part(Part::Error {
                        error: "The reply hit the model's length limit.".to_owned(),
                    })),
                    Some("aborted") => message.status = MessageStatus::Aborted,
                    _ => {}
                }
                if piece.streaming {
                    message.status = MessageStatus::Streaming;
                }
            }
            _ => {}
        }
    }
    let parts = raw.into_iter().map(|raw| match raw {
        Raw::Part(part) => part,
        Raw::Call(line, id, live) => {
            let line = match id.and_then(|id| failed.get(id)) {
                Some(true) => format!("{line} (error)"),
                None if live => format!("{line} (running)"),
                _ => line,
            };
            Part::Tools { tools: vec![line] }
        }
    });
    message.parts = merge_tools(parts);
}

/// Join consecutive tool groups into one.
fn merge_tools(parts: impl IntoIterator<Item = Part>) -> Vec<Part> {
    let mut merged: Vec<Part> = Vec::new();
    for part in parts {
        match (merged.last_mut(), part) {
            (Some(Part::Tools { tools }), Part::Tools { tools: more }) => tools.extend(more),
            (_, part) => merged.push(part),
        }
    }
    merged
}

/// The text and images of a user or custom message's content.
fn content_parts(content: &Content) -> Vec<Part> {
    let parts = content
        .0
        .iter()
        .filter_map(|block| match block.kind.as_str() {
            "text" => Some(Part::Text {
                text: block.text.clone()?,
            }),
            "image" => Some(Part::Tools {
                tools: vec!["image".to_owned()],
            }),
            _ => None,
        });
    merge_tools(parts)
}

/// A tool call's line, `kind: subject`, named like the Companion's activity
/// rows. pi's built-in tools are in its `docs/tools.md`.
fn tool_line(call: &Block) -> String {
    let name = call.name.as_deref().unwrap_or("tool");
    let args = call.arguments.as_ref();
    let arg = |field: fn(&Arguments) -> &Option<String>| args.and_then(|a| field(a).as_deref());
    let path = arg(|a| &a.path);
    let pattern = arg(|a| &a.pattern).unwrap_or_default();
    match (name, path) {
        ("bash", _) => tool_subject("exec", arg(|a| &a.command).unwrap_or_default()),
        ("read", Some(path)) => tool_subject("read", path),
        ("write", Some(path)) => tool_subject("write", path),
        ("edit", Some(path)) => tool_subject("edit", path),
        ("ls", Some(path)) => tool_subject("ls", path),
        ("grep", Some(path)) => tool_subject("search", &format!("{pattern} in {path}")),
        ("grep", None) => tool_subject("search", pattern),
        ("find", _) => tool_subject("glob", pattern),
        _ => name.to_owned(),
    }
}

fn tool_subject(kind: &str, subject: &str) -> String {
    match one_line(subject, TOOL_CHARS) {
        Some(subject) => format!("{kind}: {subject}"),
        None => kind.to_owned(),
    }
}
