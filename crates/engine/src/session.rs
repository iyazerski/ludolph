use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::jsonl::{self, Mark, Reopen, fix_surrogates, open, read_header, read_line};
use crate::skills;
use crate::text::one_line;

const PREVIEW_CHARS: usize = 200;

/// Keep the source until the caller has indexed the destination and removed all copies.
/// The copy is unpublished: the caller persists it to the returned path when it can index it at once.
pub(crate) fn relocate(
    path: &Path,
    sessions: &Path,
    cwd: &Path,
) -> Result<(tempfile::NamedTempFile, PathBuf)> {
    let cwd_text = cwd.to_str().context("the project path isn't UTF-8")?;
    let encoded = cwd_text
        .strip_prefix('/')
        .unwrap_or(cwd_text)
        .replace(['/', '\\', ':'], "-");
    let folder = sessions.join(format!("--{encoded}--"));
    std::fs::create_dir_all(&folder)?;
    let target = folder.join(path.file_name().context("the session has no filename")?);
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut header: serde_json::Value = serde_json::from_str(&line)?;
    if header["type"] != "session" {
        bail!("the session has no valid header");
    }
    header["cwd"] = serde_json::json!(cwd);
    let mut temporary = tempfile::NamedTempFile::new_in(&folder)?;
    temporary
        .as_file()
        .set_permissions(reader.get_ref().metadata()?.permissions())?;
    serde_json::to_writer(&mut temporary, &header)?;
    temporary.write_all(b"\n")?;
    std::io::copy(&mut reader, &mut temporary)?;
    temporary.as_file().sync_all()?;
    Ok((temporary, target))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub cwd: PathBuf,
    pub created: DateTime<Utc>,
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    /// pi's epoch-millisecond timestamp identifies which process submitted the prompt.
    pub last_user_at: Option<i64>,
    pub last_activity: Option<DateTime<Utc>>,
    pub outcome: Option<Outcome>,
    pub leaf: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    /// Unknown after compaction until the next answer reports usage.
    #[serde(default)]
    pub context_tokens: Option<u64>,
    /// Match pi's cumulative get_session_stats totals.
    #[serde(default)]
    pub spent: Spent,
    progress: Progress,
}

/// Include tool, compaction, branch-summary and explicit usage entries, not just assistant answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Spent {
    pub tokens: u64,
    pub cost: f64,
}

impl Spent {
    fn add(&mut self, usage: &Usage) {
        self.tokens += usage.input + usage.output + usage.cache_read + usage.cache_write;
        self.cost += usage.cost.total;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Aborted,
    /// Still running or interrupted; only the live process can distinguish them.
    Unfinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Unchanged,
    Updated,
    Rewritten,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Progress {
    version: u32,
    offset: u64,
    len: u64,
    modified: SystemTime,
}

impl Summary {
    /// Renames are not activity.
    pub fn activity(&self) -> DateTime<Utc> {
        self.last_activity.unwrap_or(self.created)
    }

    pub fn modified(&self) -> SystemTime {
        self.progress.modified
    }

    /// An incomplete header is not a session yet; retry it after the next write.
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
            last_user_at: None,
            last_activity: None,
            outcome: None,
            leaf: None,
            model: None,
            thinking: None,
            context_tokens: None,
            spent: Spent::default(),
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

    pub fn update(&mut self, path: &Path) -> Result<Freshness> {
        let mark = Mark {
            id: &self.id,
            version: self.progress.version,
            offset: self.progress.offset,
            len: self.progress.len,
            modified: self.progress.modified,
        };
        let (mut reader, len, modified) = match jsonl::reopen(path, &mark)? {
            Reopen::Unchanged => return Ok(Freshness::Unchanged),
            Reopen::Rewritten => return Ok(Freshness::Rewritten),
            Reopen::Appended {
                reader,
                len,
                modified,
            } => (reader, len, modified),
        };
        self.progress.len = len;
        self.progress.modified = modified;
        self.read_entries(&mut reader, &mut Vec::new(), path)?;
        Ok(Freshness::Updated)
    }

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
            Kind::ModelChange => {
                if let (Some(provider), Some(model)) = (entry.provider, entry.model_id) {
                    self.model = Some(format!("{provider}/{model}"));
                }
            }
            Kind::ThinkingLevelChange => {
                if entry.thinking_level.is_some() {
                    self.thinking = entry.thinking_level;
                }
            }
            Kind::Compaction => {
                self.context_tokens = None;
                self.add_usage(entry.usage.as_ref());
            }
            Kind::Usage | Kind::BranchSummary => self.add_usage(entry.usage.as_ref()),
            Kind::Other => {}
        }
    }

    fn add_usage(&mut self, usage: Option<&Usage>) {
        if let Some(usage) = usage {
            self.spent.add(usage);
        }
    }

    fn add_message(&mut self, message: Message, entry_time: Option<DateTime<Utc>>) {
        let (Role::User | Role::Assistant | Role::ToolResult) = message.role else {
            return;
        };
        self.add_usage(message.usage.as_ref());
        self.outcome = Some(match (message.role, message.stop_reason) {
            (Role::Assistant, Some(StopReason::Stop)) => Outcome::Done,
            (Role::Assistant, Some(StopReason::Error | StopReason::Length)) => Outcome::Failed,
            (Role::Assistant, Some(StopReason::Aborted)) => Outcome::Aborted,
            _ => Outcome::Unfinished,
        });
        if message.role == Role::ToolResult {
            return;
        }
        if message.role == Role::Assistant {
            if let (Some(provider), Some(model)) = (message.provider, message.model) {
                self.model = Some(format!("{provider}/{model}"));
            }
            // pi's estimate skips answers that failed, were stopped, or report no
            // usage.
            let counted = !matches!(
                message.stop_reason,
                Some(StopReason::Error | StopReason::Aborted)
            );
            let tokens = message
                .usage
                .filter(|_| counted)
                .map(|usage| usage.tokens());
            if let Some(tokens) = tokens.filter(|&tokens| tokens > 0) {
                self.context_tokens = Some(tokens);
            }
        }
        let time = message
            .timestamp
            .and_then(DateTime::from_timestamp_millis)
            .or(entry_time);
        if let Some(time) = time {
            self.last_activity = self.last_activity.max(Some(time));
        }
        if message.role == Role::User {
            self.last_user_at = message.timestamp;
        }
        if message.role == Role::User
            && let Some(text) = message.content.and_then(|content| preview(content.get()))
        {
            self.first_prompt.get_or_insert_with(|| text.clone());
            self.last_prompt = Some(text);
        }
    }
}

/// Keep content raw so indexing skips large outputs and images without copying them.
#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    name: Option<String>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
    provider: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    #[serde(rename = "thinkingLevel")]
    thinking_level: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message<'a> {
    role: Role,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    timestamp: Option<i64>,
    stop_reason: Option<StopReason>,
    provider: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cache_read: u64,
    #[serde(default)]
    cache_write: u64,
    total_tokens: Option<u64>,
    #[serde(default)]
    cost: Cost,
}

#[derive(Deserialize, Default)]
struct Cost {
    #[serde(default)]
    total: f64,
}

impl Usage {
    /// Match pi's calculateContextTokens estimate after an answer.
    fn tokens(&self) -> u64 {
        self.total_tokens
            .filter(|&total| total > 0)
            .unwrap_or(self.input + self.output + self.cache_read + self.cache_write)
    }
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    SessionInfo,
    Message,
    ModelChange,
    ThinkingLevelChange,
    Compaction,
    Usage,
    BranchSummary,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Role {
    User,
    Assistant,
    ToolResult,
    #[serde(other)]
    Other,
}

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
    let parsed = serde_json::from_str::<Content>(content).or_else(|_| {
        let mut fixed = content.as_bytes().to_vec();
        fix_surrogates(&mut fixed);
        serde_json::from_slice(&fixed)
    });
    let text = match parsed.ok()? {
        Content::Text(text) => text,
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter(|block| block.kind == "text")
            .filter_map(|block| block.text)
            .collect::<Vec<_>>()
            .join(" "),
    };
    one_line(&skills::as_typed(&text), PREVIEW_CHARS)
}
