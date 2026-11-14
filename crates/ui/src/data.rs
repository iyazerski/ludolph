//! What the windows read from the engine's tools and feeds
//! (`ludolph_engine::tools`, `ludolph_engine::feed`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A chat as `list_chats` and the feed summarize it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub title: Option<String>,
    pub status: Status,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub project: Option<Project>,
    /// The folder the chat's pi works in.
    pub cwd: String,
    /// A terminal pi has the session open, so sending is off.
    #[serde(default)]
    pub open_elsewhere: bool,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_message_preview: Option<String>,
    pub first_prompt: Option<String>,
}

/// A chat's state, as the engine names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Idle,
    Working,
    AwaitingInput,
    Errored,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

impl Chat {
    pub fn is_working(&self) -> bool {
        matches!(self.status, Status::Working | Status::AwaitingInput)
    }

    /// The title, or the first prompt until the chat has one.
    pub fn title(&self) -> &str {
        [&self.title, &self.first_prompt]
            .into_iter()
            .flatten()
            .map(|t| t.trim())
            .find(|t| !t.is_empty())
            .unwrap_or("New chat")
    }

    /// When the chat last moved, for sorting and its age.
    pub fn active_at(&self) -> DateTime<Utc> {
        self.last_message_at.unwrap_or(self.created_at)
    }
}

#[derive(Debug, Deserialize)]
pub struct Chats {
    pub chats: Vec<Chat>,
}

/// `chat_view`'s answer, read here only to load earlier messages.
#[derive(Debug, Deserialize)]
pub struct ChatView {
    pub read: Option<Read>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Read {
    pub messages: Vec<Message>,
    pub older_remaining: usize,
    pub turn_changes: Vec<TurnChanges>,
}

/// One frame of a chat's feed: the page's ids in order, with the messages
/// that are new or changed, or all of them on a `reset`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    #[serde(default)]
    pub reset: bool,
    pub chat: Chat,
    pub run: Option<Run>,
    pub pending_input: Option<Dialog>,
    pub turn_changes: Vec<TurnChanges>,
    pub older_remaining: usize,
    pub ids: Vec<String>,
    pub messages: Vec<Message>,
}

/// What a working chat's run waits on, besides the model.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// Messages pi gives the agent after its current step.
    pub steering: Vec<String>,
    /// Messages pi sends once the agent is done.
    pub follow_up: Vec<String>,
    pub retry: Option<Retry>,
    /// Why pi compacts the context.
    pub compacting: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retry {
    pub attempt: u32,
    pub max_attempts: u32,
    pub error: Option<String>,
    /// When the attempt starts.
    pub at: Option<DateTime<Utc>>,
}

/// The counts of a run's changes, for the button that opens them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TurnChanges {
    /// The run, for `turn_changes`.
    pub turn: String,
    /// The run's last message, which the button follows.
    pub after: String,
    pub files: usize,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The id of the message's first entry, or `live-<n>` until pi writes it.
    pub id: String,
    pub role: Role,
    /// Epoch milliseconds.
    pub created_at: i64,
    pub status: MessageStatus,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Complete,
    Streaming,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    /// A compaction, a branch summary, or an extension's message.
    System,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tool {
        id: String,
        /// pi's name for the tool: `bash`, `read`, or an extension's.
        name: String,
        /// What it works on, like a command or a path.
        subject: Option<String>,
        state: ToolState,
    },
    /// The message's `index`th image, which `chat_image` loads.
    Image {
        index: usize,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

impl Message {
    /// Whether pi has written the message, so its id and images stay.
    pub fn written(&self) -> bool {
        !self.id.starts_with("live-")
    }

    /// The message's prose, joined.
    pub fn text(&self) -> String {
        let texts: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        texts.join("\n\n")
    }

    /// The indexes of the message's images.
    pub fn images(&self) -> impl Iterator<Item = usize> {
        self.parts.iter().filter_map(|p| match p {
            Part::Image { index } => Some(*index),
            _ => None,
        })
    }

    /// Whether the message ends with prose, so `answer` has one.
    pub fn has_answer(&self) -> bool {
        matches!(self.parts.last(), Some(Part::Text { .. }))
    }

    /// The prose after the message's last tool call or error: the answer
    /// that ends the turn, as Markdown; `None` when it ends without prose.
    /// Thinking doesn't break it, since it isn't shown inline.
    pub fn answer(&self) -> Option<String> {
        let tail = self
            .parts
            .iter()
            .rposition(|p| matches!(p, Part::Tool { .. } | Part::Error { .. }))
            .map_or(0, |i| i + 1);
        let texts: Vec<&str> = self.parts[tail..]
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        (!texts.is_empty()).then(|| texts.join("\n\n"))
    }
}

/// A dialog an extension opened, as pi asks it (`docs/rpc-extension-ui.md`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Dialog {
    pub id: String,
    pub dialog: DialogKind,
    pub title: String,
    pub message: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    pub prefill: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogKind {
    Select,
    Confirm,
    Input,
    Editor,
}

/// `chat_image`'s answer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImage {
    pub mime_type: String,
    /// Base64.
    pub data: String,
}

/// `polish_dictation`'s answer: what dictation heard, cleaned up.
#[derive(Debug, Deserialize)]
pub struct Polished {
    pub text: String,
}

/// `composer_info`: what the composer shows under the text.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerInfo {
    pub branch: Option<String>,
    /// Set for a git project, the only place the branch shows.
    pub checkout: Option<Checkout>,
    pub context_usage: Option<ContextUsage>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Checkout {
    Local,
    Worktree,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub window: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub label: Option<String>,
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Models {
    pub models: Vec<Model>,
    /// The model pi starts a chat on when none is picked.
    pub default: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Projects {
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The command that runs the skill when sent.
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Deserialize)]
pub struct Skills {
    pub skills: Vec<Skill>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub chat_id: String,
    /// Why the first message didn't go, though the chat was created.
    #[serde(default)]
    pub send_error: Option<String>,
}

/// A model and reasoning pick, as `set_chat_model` saves it.
#[derive(Debug, Deserialize)]
pub struct Picked {
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

/// A run's changes, as `turn_changes` answers.
#[derive(Debug, Deserialize)]
pub struct Diff {
    pub patch: String,
    pub truncated: bool,
}

/// The thinking levels `model` supports.
pub fn levels_of<'a>(models: &'a [Model], model: &str) -> &'a [String] {
    models
        .iter()
        .find(|m| m.id == model)
        .map_or(&[], |m| m.reasoning_levels.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_is_the_prose_after_the_last_tool_or_error() {
        let text = |t: &str| Part::Text { text: t.into() };
        let tool = || Part::Tool {
            id: "t".into(),
            name: "bash".into(),
            subject: Some("ls".into()),
            state: ToolState::Done,
        };
        let thinking = || Part::Thinking { text: "hm".into() };
        let error = || Part::Error {
            error: "boom".into(),
        };
        let cases = [
            (vec![text("a")], Some("a")),
            (
                vec![text("a"), tool(), text("b"), tool(), text("c")],
                Some("c"),
            ),
            (
                vec![tool(), text("b"), thinking(), text("c")],
                Some("b\n\nc"),
            ),
            (vec![text("a"), error(), text("b")], Some("b")),
            (vec![text("a"), tool()], None),
            (vec![], None),
        ];
        for (parts, want) in cases {
            let m = Message {
                id: "m".into(),
                role: Role::Assistant,
                created_at: 0,
                status: MessageStatus::Complete,
                parts,
            };
            assert_eq!(m.answer().as_deref(), want);
        }
    }
}

/// The engine's settings, as `get_settings` answers and `set_settings`
/// takes them, all at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The model for titles and dictation cleanup; pi's default if `None`.
    pub sidecar_model: Option<String>,
    pub notify: Notify,
    pub open_at_login: bool,
}

/// When the phone gets notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Notify {
    /// Only while nobody uses the Mac.
    Away,
    Always,
}

/// The Mac's link to the relay, as `get_phone_link` and `link_phone`
/// answer.
/// This Mac's link to the relay, a phone linking through it, and the relay
/// it links through.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PhoneLink {
    pub link: Option<RelayLink>,
    pub pairing: Option<Pairing>,
    pub relay: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayLink {
    pub relay: String,
    pub status: LinkStatus,
    /// Why the last try to connect failed, while it tries again.
    pub error: Option<String>,
}

/// A phone linking through this Mac.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Pairing {
    Starting,
    /// Waiting for a phone to open `url`, which the QR code holds.
    Waiting {
        url: String,
    },
    Linked,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Connecting,
    Connected,
    /// The relay forgot this Mac, so a phone must link again.
    Rejected,
}
