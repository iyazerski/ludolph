use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// `provider/model`; None uses pi's default for titles and dictation cleanup.
    pub sidecar_model: Option<String>,
    pub notify: Notify,
    pub open_at_login: bool,
    pub expand_tools: bool,
    pub send_behavior: SendBehavior,
    pub tool_routing: bool,
    pub second_opinion: bool,
    /// In order of preference: a chat asks the first one from another model family.
    pub second_opinion_reviewers: Vec<ReviewModel>,
    pub browser_use: bool,
    pub computer_use: bool,
    /// `provider/model` for browser and computer use; None uses pi's default.
    pub gui_model: Option<String>,
    pub gui_thinking: Option<String>,
    /// `provider/model` ids the chat model picker offers; empty offers every model.
    pub chat_models: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sidecar_model: None,
            notify: Notify::default(),
            open_at_login: false,
            expand_tools: false,
            send_behavior: SendBehavior::default(),
            tool_routing: true,
            second_opinion: true,
            second_opinion_reviewers: Vec::new(),
            // They install tools and ask for macOS permissions, so only on request.
            browser_use: false,
            computer_use: false,
            gui_model: None,
            gui_thinking: None,
            chat_models: Vec::new(),
        }
    }
}

/// A provider-qualified reviewer and its selected pi thinking level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewModel {
    pub model: String,
    pub thinking: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendBehavior {
    /// Keep the message editable until its own turn starts.
    #[default]
    Queue,
    /// Hand it to pi after the current tool step.
    Steer,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Notify {
    #[default]
    Away,
    Always,
}

impl Settings {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("settings.json")
    }

    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = Self::path(state_dir);
        let Some(mut stored) = ludolph_json_file::load::<Map<String, Value>>(&path)? else {
            return Ok(Self::default());
        };
        // Ludolph 0.2.22 and earlier kept one Claude and one GPT reviewer.
        let legacy: Vec<Value> = ["secondOpinionClaude", "secondOpinionGpt"]
            .into_iter()
            .filter_map(|key| stored.remove(key))
            .filter(|reviewer| !reviewer.is_null())
            .collect();
        if !legacy.is_empty() {
            stored.insert("secondOpinionReviewers".to_owned(), legacy.into());
        }
        serde_json::from_value(stored.into()).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        ludolph_json_file::save(&Self::path(state_dir), self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_reviewers_load_in_their_old_order() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = r#"{"secondOpinion": true, "secondOpinionClaude": {"model": "anthropic/claude-opus", "thinking": "medium"},
            "secondOpinionGpt": {"model": "openai/gpt-sol", "thinking": "high"}}"#;
        std::fs::write(Settings::path(dir.path()), legacy).unwrap();
        let reviewer = |model: &str, thinking: &str| ReviewModel {
            model: model.to_owned(),
            thinking: thinking.to_owned(),
        };
        assert_eq!(
            Settings::load(dir.path()).unwrap().second_opinion_reviewers,
            [
                reviewer("anthropic/claude-opus", "medium"),
                reviewer("openai/gpt-sol", "high")
            ]
        );
    }
}
