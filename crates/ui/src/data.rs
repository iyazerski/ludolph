use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub title: Option<String>,
    pub status: Status,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub project: Option<Project>,
    pub cwd: String,
    #[serde(default)]
    pub open_elsewhere: bool,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_message_preview: Option<String>,
    pub first_prompt: Option<String>,
}

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

    pub fn title(&self) -> &str {
        [&self.title, &self.first_prompt]
            .into_iter()
            .flatten()
            .map(|t| t.trim())
            .find(|t| !t.is_empty())
            .unwrap_or("New chat")
    }

    pub fn active_at(&self) -> DateTime<Utc> {
        self.last_message_at.unwrap_or(self.created_at)
    }
}

#[derive(Debug, Deserialize)]
pub struct Chats {
    pub chats: Vec<Chat>,
}

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    #[serde(default)]
    pub reset: bool,
    pub chat: Chat,
    pub run: Option<Run>,
    pub failure: Option<String>,
    pub queued: Vec<Queued>,
    pub pending_input: Option<Dialog>,
    pub turn_changes: Vec<TurnChanges>,
    pub older_remaining: usize,
    pub ids: Vec<String>,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Queued {
    pub id: String,
    pub text: String,
    pub images: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
    pub retry: Option<Retry>,
    pub compacting: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retry {
    pub attempt: u32,
    pub max_attempts: u32,
    pub error: Option<String>,
    pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TurnChanges {
    pub turn: String,
    pub after: String,
    pub files: usize,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub created_at: i64,
    pub ended_at: i64,
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
        name: String,
        subject: Option<String>,
        state: ToolState,
        calls: Vec<NestedCall>,
    },
    Image {
        index: usize,
    },
    Error {
        error: String,
    },
    Note {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolState {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedCall {
    pub id: String,
    pub name: String,
    pub subject: Option<String>,
    pub state: ToolState,
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct ToolOutput {
    pub args: serde_json::Value,
    pub text: String,
    pub truncated: bool,
}

impl Message {
    pub fn written(&self) -> bool {
        !self.id.starts_with("live-")
    }

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

    pub fn images(&self) -> impl Iterator<Item = usize> {
        self.parts.iter().filter_map(|p| match p {
            Part::Image { index } => Some(*index),
            _ => None,
        })
    }

    pub fn has_answer(&self) -> bool {
        matches!(self.parts.last(), Some(Part::Text { .. }))
    }

    /// Copy only prose after the last tool or error; thinking does not split the final answer.
    pub fn answer(&self) -> Option<String> {
        let tail = self
            .parts
            .iter()
            .rposition(|p| {
                matches!(
                    p,
                    Part::Tool { .. } | Part::Error { .. } | Part::Note { .. }
                )
            })
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImage {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Deserialize)]
pub struct Polished {
    pub text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerInfo {
    pub branch: Option<String>,
    pub checkout: Option<Checkout>,
    pub context_usage: Option<ContextUsage>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub spent: Option<Spent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Checkout {
    Local,
    Worktree,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Spent {
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDetails {
    pub compactions: usize,
    pub spent: Spent,
    pub pull_requests: Vec<PullRequest>,
    pub changes: Option<ChangeCounts>,
    pub changes_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub url: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangeCounts {
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
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
#[serde(rename_all = "camelCase")]
pub struct Models {
    pub models: Vec<Model>,
    pub default: Option<String>,
    pub default_reasoning: String,
}

impl Models {
    pub fn fitted(
        &self,
        model: Option<&str>,
        reasoning: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        let previous = model.and_then(|id| self.models.iter().find(|model| model.id == id));
        let chosen = previous.or_else(|| {
            self.models
                .iter()
                .find(|model| Some(&model.id) == self.default.as_ref())
        });
        let Some(chosen) = chosen else {
            return (None, None);
        };
        let supported = |level: &str| chosen.reasoning_levels.iter().any(|l| l == level);
        let level = previous
            .and(reasoning)
            .filter(|level| supported(level))
            .or_else(|| {
                (Some(&chosen.id) == self.default.as_ref())
                    .then_some(self.default_reasoning.as_str())
                    .filter(|level| supported(level))
            })
            .or_else(|| supported("medium").then_some("medium"))
            .or_else(|| chosen.reasoning_levels.first().map(String::as_str));
        (Some(chosen.id.clone()), level.map(str::to_owned))
    }
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
}

#[derive(Debug, Deserialize)]
pub struct Skills {
    pub skills: Vec<Skill>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub chat_id: String,
    #[serde(default)]
    pub send_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Picked {
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Diff {
    pub patch: String,
    pub truncated: bool,
}

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
    fn new_chat_picks_use_available_history_or_pis_defaults() {
        let mut models: Models = serde_json::from_value(serde_json::json!({
            "models": [
                {"id":"openai/gpt","reasoningLevels":["off","medium","high"]},
                {"id":"fake/quick","reasoningLevels":["off"]}
            ],
            "default":"openai/gpt","defaultReasoning":"medium"
        }))
        .unwrap();
        for (model, level, expected) in [
            (Some("openai/gpt"), Some("high"), ("openai/gpt", "high")),
            (None, None, ("openai/gpt", "medium")),
            (
                Some("anthropic/opus"),
                Some("high"),
                ("openai/gpt", "medium"),
            ),
            (Some("fake/quick"), Some("high"), ("fake/quick", "off")),
            (Some("openai/gpt"), None, ("openai/gpt", "medium")),
        ] {
            let got = models.fitted(model, level);
            assert_eq!(
                (got.0.as_deref(), got.1.as_deref()),
                (Some(expected.0), Some(expected.1))
            );
        }
        models.default_reasoning = "high".to_owned();
        let got = models.fitted(None, None);
        assert_eq!(got.1.as_deref(), Some("high"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub sidecar_model: Option<String>,
    pub notify: Notify,
    pub open_at_login: bool,
    pub expand_tools: bool,
    pub send_behavior: SendBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendBehavior {
    Queue,
    Steer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Notify {
    Away,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PhoneLink {
    pub link: Option<RelayLink>,
    pub phones: Vec<String>,
    pub pairing: Option<Pairing>,
    pub relay: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayLink {
    pub relay: String,
    pub status: LinkStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Pairing {
    Starting,
    Waiting { url: String },
    Linked,
    Failed { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Connecting,
    Connected,
    Rejected,
}
