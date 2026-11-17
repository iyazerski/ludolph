use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

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
    pub browser_use: bool,
    pub computer_use: bool,
    /// `provider/model` for browser and computer use; None uses pi's default.
    pub gui_model: Option<String>,
    pub gui_thinking: Option<String>,
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
            // They install tools and ask for macOS permissions, so only on request.
            browser_use: false,
            computer_use: false,
            gui_model: None,
            gui_thinking: None,
        }
    }
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
        Ok(ludolph_json_file::load(&Self::path(state_dir))?.unwrap_or_default())
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        ludolph_json_file::save(&Self::path(state_dir), self)
    }
}
